//! One prompt, into a plan, into several nodes building slices in parallel.
//!
//! The split is D14's: the orchestrator *node* turns the prompt into an ai-planner plan,
//! and this module - Rust - reads the ready slices back, leases a worktree per slice,
//! routes each to the seat whose zone owns its paths, and starts one eve process per
//! lease (D10). Scheduling, budgets and routing are the control plane's job; an agent
//! spawning sibling agents would be the same work with no guardrails around it.
//!
//! Nothing here stores a plan or a slice. `node_run.slice_key` is a reference into
//! ai-planner and that is the whole of the coupling (D4).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::model::{EventKind, NewEvent, NodeStatus};
use crate::neighbours::{git, Planner, Slice, Worktrees};
use crate::store::Store;
use crate::supervise::process::EveEnv;
use crate::supervise::supervisor::{outcome_status, Supervisor, TurnOutcome};
use crate::ROOT_ROLE;

/// What one dispatched slice did.
#[derive(Debug, Clone)]
pub struct Dispatched {
    pub slice_key: String,
    pub role: String,
    pub worktree: PathBuf,
    pub node_run_id: i64,
    pub status: NodeStatus,
    pub outcome: TurnOutcome,
    /// The branch its work landed on, if it produced any.
    pub branch: Option<String>,
}

/// What a whole orchestrated run did.
#[derive(Debug, Default)]
pub struct Orchestration {
    /// Slices that were ready but had no seat owning their paths.
    pub unrouted: Vec<(String, String)>,
    pub dispatched: Vec<Dispatched>,
}

/// What a Pi turn needs that is the same for every seat in a run (D20).
///
/// The eve equivalent was a generated project directory plus an `EveEnv`; this is the
/// whole of what replaced them. `support` holds the guard and the per-seat MCP configs
/// and is deliberately never a lease - a guard or an allow-list a node can edit is not
/// one.
#[derive(Debug, Clone)]
pub struct Rig {
    pub support: PathBuf,
    /// The checkout whose plan this run works from. Never a lease: a plan written inside
    /// a copy is a plan nobody finds again.
    pub plan_root: PathBuf,
    pub sources: Vec<crate::ContextSource>,
    pub registry: ModelRegistry,
}

impl Rig {
    /// The turn one seat would take.
    ///
    /// The model comes from the registry rather than the row, because a denied preference
    /// falls back (D13) and what a run actually used is a fact about that run.
    fn seat(
        &self,
        store: &Store,
        agent_id: i64,
        worktree: &Path,
        prompt: &str,
    ) -> Result<crate::PiTurn> {
        let agent = store.agent(agent_id)?;
        let team = store.team(agent.team_id)?;
        let roster = store.agents(agent.team_id)?;
        let (effective, _) = self.registry.resolve_agents(std::slice::from_ref(&agent))?;
        let resolved = effective.first().unwrap_or(&agent);
        // Only the seats that may shape the plan get the tools that shape it. A maker
        // that can add slices can give itself work.
        let may_plan = agent.role == ROOT_ROLE || agent.role == "planner";
        crate::PiSeat {
            agent: &agent,
            provider: resolved.provider,
            model: &resolved.model,
            worktree,
            support: &self.support,
            sources: &self.sources,
            plan: Some((self.plan_root.as_path(), may_plan)),
            team: &team,
            roster: &roster,
        }
        .turn(prompt)
    }
}

/// Everything a run needs that does not change between its nodes.
#[derive(Debug)]
pub struct Orchestrator {
    pub db_path: PathBuf,
    /// The generated eve project. Shared: built once, started once per lease.
    pub project_dir: PathBuf,
    /// The checkout the plan and the worktree pool belong to.
    pub repo: PathBuf,
    pub run_id: i64,
    pub team_id: i64,
    pub registry: ModelRegistry,
    pub required_env: Vec<&'static str>,
    pub parallel_width: usize,
    pub planner: Planner,
    pub worktrees: Worktrees,
}

impl Orchestrator {
    /// What every Pi turn in this run shares.
    pub fn rig(&self) -> Rig {
        Rig {
            support: self.project_dir.clone(),
            plan_root: self.repo.clone(),
            sources: self.registry.context_sources(),
            registry: self.registry.clone(),
        }
    }

    /// A supervisor for the one-off install and build, before any node starts.
    ///
    /// Bound to the repository rather than a lease: `eve build` evaluates every authored
    /// module, so it needs a valid environment, but it is not doing a node's work and
    /// must not hold a worktree while it runs.
    pub fn builder(&self) -> Result<Supervisor> {
        Ok(Supervisor::new(&self.project_dir, self.env(&self.repo)?))
    }

    /// Phase one: the orchestrator node reads the prompt and writes the plan.
    ///
    /// It runs against the repository rather than a lease, because a plan written inside
    /// a leased copy is a plan nobody finds again, and because planning reads the code it
    /// is planning against.
    pub async fn plan<F>(&self, store: &mut Store, prompt: &str, on_event: F) -> Result<TurnOutcome>
    where
        F: FnMut(&crate::PiEvent) + Send,
    {
        let orchestrator = store
            .agents(self.team_id)?
            .into_iter()
            .find(|agent| agent.role == ROOT_ROLE)
            .ok_or_else(|| Error::invalid("the team has no orchestrator"))?;

        let node = store.dispatch(self.run_id, orchestrator.id, None, &self.registry)?;
        store.attach_worktree(node.id, &self.repo.to_string_lossy(), None, None)?;
        store.set_node_status(node.id, NodeStatus::Running)?;

        let turn = self
            .rig()
            .seat(store, orchestrator.id, &self.repo, prompt)?;
        let outcome = match crate::run_pi_turn(store, node.id, &turn, on_event).await {
            Ok((_, outcome)) => outcome,
            Err(error) => {
                store.set_node_status(node.id, NodeStatus::Failed)?;
                return Err(error);
            }
        };
        store.set_node_status(node.id, outcome_status(&outcome))?;
        Ok(outcome)
    }

    /// The slices that are ready, nobody holds, and some seat owns.
    ///
    /// Unroutable slices are reported rather than guessed at: dispatching a slice to a
    /// seat that does not own its paths is how two agents end up in one file.
    /// The slices the board currently offers. Fetched separately from routing them so
    /// no database borrow is held across the call to ai-planner - which would make every
    /// future containing this one unspawnable, since a connection is not `Sync`.
    pub async fn offered(&self) -> Result<Vec<Slice>> {
        self.planner.slices().await
    }

    /// Which of those this team can build, and who builds each.
    pub fn routable(&self, store: &Store, slices: Vec<Slice>) -> Result<Routing> {
        route(store, self.team_id, slices)
    }

    /// Phase two: build the ready slices, several at a time.
    ///
    /// One seat is never dispatched twice at once - a seat is a person-shaped thing with
    /// one worktree, and two turns writing through the same zone is the collision the
    /// roster exists to prevent.
    pub async fn build_slices<F>(
        &self,
        store: &mut Store,
        mut on_progress: F,
    ) -> Result<Orchestration>
    where
        F: FnMut(&str) + Send,
    {
        let offered = self.offered().await?;
        let (routed, unrouted) = self.routable(store, offered)?;
        let mut out = Orchestration {
            unrouted,
            dispatched: Vec::new(),
        };
        for (key, reason) in &out.unrouted {
            store.append_event(
                self.run_id,
                NewEvent::new(EventKind::Note, format!("{key} not dispatched: {reason}"))
                    .by("orchestrator"),
            )?;
        }
        if routed.is_empty() {
            return Ok(out);
        }

        let width = self.parallel_width.max(1);
        let mut queue = routed.into_iter();
        let mut wave: Vec<(Slice, i64, String)> = Vec::new();
        let mut busy: Vec<String> = Vec::new();

        // Waves rather than a rolling pool: a wave is bounded, easy to report, and the
        // same shape the Console will draw. Width and one-turn-per-seat both apply.
        loop {
            for item in queue.by_ref() {
                if busy.contains(&item.2) {
                    continue;
                }
                busy.push(item.2.clone());
                wave.push(item);
                if wave.len() == width {
                    break;
                }
            }
            if wave.is_empty() {
                break;
            }

            let mut tasks = Vec::new();
            for (slice, agent_id, role) in wave.drain(..) {
                on_progress(&format!("{} -> {role}", slice.key));
                let node = self.prepare(store, &slice, agent_id).await?;
                tasks.push(tokio::spawn(run_one(node)));
            }

            for task in tasks {
                match task.await {
                    Ok(Ok(done)) => out.dispatched.push(done),
                    Ok(Err(error)) => {
                        // One node failing does not abandon its siblings: the others are
                        // in their own worktrees and their work is still worth having.
                        store.append_event(
                            self.run_id,
                            NewEvent::new(EventKind::Failed, format!("node failed: {error}"))
                                .by("orchestrator"),
                        )?;
                    }
                    Err(join) => {
                        store.append_event(
                            self.run_id,
                            NewEvent::new(EventKind::Failed, format!("node panicked: {join}"))
                                .by("orchestrator"),
                        )?;
                    }
                }
            }
            busy.clear();
        }
        Ok(out)
    }

    /// Lease, claim and register one slice, ready to be run on its own task.
    async fn prepare(&self, store: &mut Store, slice: &Slice, agent_id: i64) -> Result<NodeTask> {
        let lease = self
            .worktrees
            .lease(&format!("ai-team:{}", slice.key))
            .await?;
        let worktree = lease.path().to_path_buf();
        // Before anything runs in it: the gates write build output into this worktree,
        // and a repo that does not already ignore it would get it committed.
        git::ignore_build_output(&worktree).await?;

        // Claim through ai-planner, in the leased worktree, so the board shows where the
        // work is actually happening and a second run is told the slice is taken.
        if !self.planner.claim(&slice.key, &worktree).await? {
            return Err(Error::invalid(format!(
                "{} is claimed by another worktree",
                slice.key
            )));
        }

        Ok(NodeTask {
            db_path: self.db_path.clone(),
            rig: self.rig(),
            agent_id,
            registry: self.registry.clone(),
            slice_key: slice.key.clone(),
            title: slice.title.clone(),
            // Read from the lease, not the main checkout: a branch that changes the
            // house rules should be judged by the rules it is proposing.
            // Narrowed to the paths this slice touches, so a nested AGENTS.md governing
            // the directory being edited is included and the rest of a monorepo is not.
            prompt: slice_prompt(slice, &crate::house::read_for(&worktree, &slice.touches())),
            worktree,
            lease,
            planner: self.planner.clone(),
            run_id: self.run_id,
            // Read off the run, never back through the team: what this run was allowed
            // to spend is a fact about this run (D2).
            max_repairs: store.run(self.run_id)?.max_repairs,
            verifier: self.verifier(store)?,
        })
    }

    /// Whether the verifier is a genuinely second opinion.
    ///
    /// A checker on the same model as the maker shares its blind spots: it tends to find
    /// the mistakes that model does not make and miss the ones it does. That is a team
    /// configuration choice, not an error, so this reports rather than refuses.
    pub fn verifier_note(&self, store: &Store) -> Result<Option<String>> {
        let agents = store.agents(self.team_id)?;
        let Some(verifier) = agents
            .iter()
            .find(|agent| agent.role == crate::VERIFIER_ROLE && agent.enabled)
        else {
            return Ok(Some(
                "this team has no verifier, so the project's own gates are the whole check"
                    .to_string(),
            ));
        };

        // Compare what will actually run, not what the rows prefer: the machine profile
        // may have collapsed two different preferences onto one allowed provider.
        let effective = |agent: &crate::model::Agent| {
            self.registry
                .resolve(agent)
                .map(|resolved| format!("{}/{}", resolved.provider, resolved.model))
        };
        let checker = effective(verifier)?;
        let shared: Vec<&str> = agents
            .iter()
            .filter(|agent| agent.enabled && !agent.read_only)
            .filter(|agent| effective(agent).is_ok_and(|model| model == checker))
            .map(|agent| agent.role.as_str())
            .collect();

        Ok((!shared.is_empty()).then(|| {
            format!(
                "the verifier runs {checker}, the same model as {} - a second opinion \
                 from one model shares its blind spots",
                shared.join(", ")
            )
        }))
    }

    /// The seat that checks the makers, if the team has one.
    ///
    /// A team without a verifier is a team whose gates are the whole check, which is a
    /// legitimate choice and not an error - the seat is configuration, like every other.
    fn verifier(&self, store: &Store) -> Result<Option<VerifierSeat>> {
        Ok(store
            .agents(self.team_id)?
            .into_iter()
            .find(|agent| agent.role == crate::VERIFIER_ROLE && agent.enabled)
            .map(|agent| VerifierSeat {
                agent_id: agent.id,
                registry: self.registry.clone(),
            }))
    }

    fn env(&self, worktree: &Path) -> Result<EveEnv> {
        Ok(EveEnv {
            worktree: worktree.to_path_buf(),
            token: crate::supervise::process::mint_token(),
            provider_keys: self.registry.provider_environment(&self.required_env)?,
            plan_root: Some(self.repo.clone()),
            plan_slug: self.planner.plan_slug().map(ToString::to_string),
        })
    }
}

/// Everything one parallel node needs, owned outright so it can move to its own task.
struct NodeTask {
    db_path: PathBuf,
    rig: Rig,
    agent_id: i64,
    registry: ModelRegistry,
    slice_key: String,
    title: String,
    prompt: String,
    worktree: PathBuf,
    lease: crate::neighbours::Lease,
    planner: Planner,
    run_id: i64,
    max_repairs: i64,
    verifier: Option<VerifierSeat>,
}

/// Drive one slice to completion in its own worktree, on its own connection.
///
/// Its own `Store`: SQLite is in WAL with a busy timeout, so parallel nodes write
/// concurrently, and sharing one `&mut Store` across tasks would serialise them anyway.
async fn run_one(task: NodeTask) -> Result<Dispatched> {
    let NodeTask {
        db_path,
        rig,
        agent_id,
        registry,
        slice_key,
        title,
        prompt,
        worktree,
        lease,
        planner,
        run_id,
        max_repairs,
        verifier,
    } = task;

    let mut store = Store::open(&db_path)?;

    let Attempted {
        outcome,
        rejection,
        attempt,
        node_run_id,
        role,
        exhausted,
        changed,
    } = match attempt_until_accepted(AttemptArgs {
        store: &mut store,
        rig: &rig,
        agent_id,
        registry: &registry,
        run_id,
        slice_key: &slice_key,
        worktree: &worktree,
        max_repairs,
        verifier: verifier.as_ref(),
        first: prompt,
    })
    .await
    {
        Ok(attempted) => attempted,
        Err(error) => {
            let _ = planner.release(&slice_key, &worktree).await;
            let _ = planner
                .log(
                    &format!("ai-team: {slice_key} failed - {error}"),
                    Some(&slice_key),
                )
                .await;
            let _ = lease.release().await;
            return Err(error);
        }
    };
    let (mut status, fallout) = settle_status(
        &mut store,
        run_id,
        node_run_id,
        &role,
        &slice_key,
        &outcome,
        rejection.as_deref(),
        attempt,
        exhausted,
    )?;

    let branch = land(
        &mut store,
        &planner,
        Landing {
            worktree: &worktree,
            node_run_id,
            slice_key: &slice_key,
            title: &title,
            changed: &changed,
        },
        &mut status,
    )
    .await?;
    store.set_node_status(node_run_id, status)?;

    update_board(
        &planner,
        &slice_key,
        &worktree,
        status,
        fallout,
        rejection.as_deref(),
    )
    .await;

    let dispatched = Dispatched {
        slice_key,
        role,
        worktree,
        node_run_id,
        status,
        outcome,
        branch,
    };
    // Returned explicitly so a failure to return is reported rather than swallowed by
    // the Drop fallback.
    lease.release().await?;
    Ok(dispatched)
}

/// Slices paired with the seat that will build them, and the ones nobody can.
type Routing = (Vec<(Slice, i64, String)>, Vec<(String, String)>);

/// Decide who builds what.
///
/// Separated from fetching them so the decision is testable without ai-planner
/// installed: this is the rule that keeps two agents out of one file, and it deserves
/// more than a demo exercising it.
fn route(store: &Store, team_id: i64, slices: Vec<Slice>) -> Result<Routing> {
    let mut routed = Vec::new();
    let mut unrouted = Vec::new();

    let mut slices: Vec<Slice> = slices.into_iter().filter(Slice::is_dispatchable).collect();
    // `ord` is the plan's declared sequence, and ai-planner has no dependency edges - so
    // the order a human put the slices in is the only ordering intent there is.
    slices.sort_by_key(|slice| slice.ord);

    for slice in slices {
        let paths = slice.touches();
        if paths.is_empty() {
            unrouted.push((slice.key.clone(), "names no paths to route on".to_string()));
            continue;
        }
        let owner = paths
            .iter()
            .find_map(|path| store.agent_for_path(team_id, path).transpose())
            .transpose()?;
        match owner {
            Some(agent) => routed.push((slice, agent.id, agent.role)),
            None => unrouted.push((
                slice.key.clone(),
                format!("no seat owns {}", paths.join(", ")),
            )),
        }
    }
    Ok((routed, unrouted))
}

/// Commit the work onto a branch, before the lease goes back.
///
/// `awt return` cleans and resets the worktree, so this is the only thing standing
/// between the work and losing it - and a branch in the shared object store is what a
/// human reviews afterwards. Downgrades `status` if there turns out to be nothing there.
async fn land(
    store: &mut Store,
    planner: &Planner,
    work: Landing<'_>,
    status: &mut NodeStatus,
) -> Result<Option<String>> {
    let Landing {
        worktree,
        node_run_id,
        slice_key,
        title,
        changed,
    } = work;
    if *status != NodeStatus::Done {
        return Ok(None);
    }
    let name = format!("ai-team/{}", slice_key.to_lowercase());
    match git::commit_paths(
        worktree,
        &name,
        &format!("{slice_key}: {}", title.trim()),
        changed,
    )
    .await
    {
        Ok(Some(sha)) => {
            store.attach_worktree(node_run_id, &worktree.to_string_lossy(), Some(&name), None)?;

            // A review, so the work is *reviewable*. Nothing opened one before this, which
            // meant the Review surface was always empty in real use - the team produced
            // branches and there was nothing to comment on. Opened here rather than when
            // somebody asks, because the point is to see a diff as the team works rather
            // than to remember to ask for one afterwards.
            let node = store.node_run(node_run_id)?;
            let project_id = store.run(node.run_id)?.project_id;
            if let Err(error) = store.open_review(
                project_id,
                &format!("{slice_key}: {}", title.trim()),
                Some(node.run_id),
                Some(node_run_id),
                Some(&name),
            ) {
                // Not fatal: the work is committed and on a branch. A review that could not
                // be opened costs a surface, not the slice.
                store.append_event(
                    node.run_id,
                    NewEvent::new(
                        EventKind::Note,
                        format!("could not open a review for {slice_key}: {error}"),
                    )
                    .on_node(node_run_id),
                )?;
            }

            let _ = planner.set_branch(slice_key, &name).await;
            let _ = planner
                .log(
                    &format!("ai-team built {slice_key} on {name} ({})", &sha[..12]),
                    Some(slice_key),
                )
                .await;
            Ok(Some(name))
        }
        Ok(None) => {
            // A turn that reports done but changed no file did not build the slice.
            // Calling that success puts a green row on the board for work nobody did.
            *status = NodeStatus::Failed;
            store.block_node(node_run_id, "the turn finished without changing a file")?;
            Ok(None)
        }
        Err(error) => {
            *status = NodeStatus::Failed;
            store.block_node(node_run_id, &format!("could not commit its work: {error}"))?;
            Ok(None)
        }
    }
}

/// How a node's work ended, after however many repairs it took.
struct Attempted {
    outcome: TurnOutcome,
    /// Why it was still rejected when the budget ran out, if it was.
    rejection: Option<String>,
    attempt: i64,
    /// The row for the attempt that actually produced this outcome.
    node_run_id: i64,
    role: String,
    /// True when it stopped because it ran out of budget or repairs rather than
    /// because it finished. Decides whether the team's failure policy applies.
    exhausted: bool,
    /// What the turns actually touched, captured before the gates ran.
    changed: Vec<String>,
}

impl Attempted {
    /// Accepted, or parked on a question - either way, nobody is waiting on a repair.
    fn settled(
        outcome: TurnOutcome,
        node: &crate::model::NodeRun,
        attempt: i64,
        changed: Vec<String>,
    ) -> Attempted {
        Attempted {
            outcome,
            rejection: None,
            attempt,
            node_run_id: node.id,
            role: node.role.clone(),
            exhausted: false,
            changed,
        }
    }

    /// Stopped without being accepted: out of repairs, or out of budget.
    fn stopped(
        outcome: TurnOutcome,
        node: &crate::model::NodeRun,
        attempt: i64,
        changed: Vec<String>,
        reason: String,
    ) -> Attempted {
        Attempted {
            outcome,
            rejection: Some(reason),
            attempt,
            node_run_id: node.id,
            role: node.role.clone(),
            exhausted: true,
            changed,
        }
    }
}

/// Everything the build-check-repair loop needs. A struct because the alternative is
/// eleven positional arguments, where swapping two `&str`s compiles.
struct AttemptArgs<'a> {
    store: &'a mut Store,
    rig: &'a Rig,
    agent_id: i64,
    registry: &'a ModelRegistry,
    run_id: i64,
    slice_key: &'a str,
    worktree: &'a Path,
    max_repairs: i64,
    verifier: Option<&'a VerifierSeat>,
    first: String,
}

/// Build, check, repair, until it is accepted or the repair budget is spent.
///
/// The maker takes a turn; the project's own gates run against what it left behind; the
/// verifier reads the result. Anything short of both passing goes back to the maker with
/// the evidence attached, because "it failed" without the output is not something a
/// model can act on.
///
/// All of it happens inside the lease: the worktree is reset when it goes back, so there
/// is no checking the work afterwards.
async fn attempt_until_accepted(args: AttemptArgs<'_>) -> Result<Attempted> {
    let AttemptArgs {
        store,
        rig,
        agent_id,
        registry,
        run_id,
        slice_key,
        worktree,
        max_repairs,
        verifier,
        first,
    } = args;

    let mut attempt = 0;
    let mut instruction = first;
    let mut changed: Vec<String> = Vec::new();

    loop {
        let node = open_attempt(store, run_id, agent_id, slice_key, registry, worktree)?;
        let node_run_id = node.id;
        if let Some(exceeded) = crate::guardrails::run_may_continue(store, run_id)? {
            return Ok(Attempted::stopped(
                TurnOutcome::default(),
                &node,
                attempt,
                changed,
                exceeded.reason,
            ));
        }
        store.set_node_status(node_run_id, NodeStatus::Running)?;

        let turn = match take_turn(store, rig, agent_id, node_run_id, worktree, &instruction).await
        {
            Ok(turn) => turn,
            Err(error) => {
                store.set_node_status(node_run_id, NodeStatus::Failed)?;
                return Err(error);
            }
        };

        // Captured here, before `check` runs the gates: running them writes build output
        // into the worktree, and that is ai-team's mess rather than the agent's work.
        for path in crate::neighbours::git::changed_paths(worktree).await? {
            if !changed.contains(&path) {
                changed.push(path);
            }
        }

        // A parked turn is waiting on a person, and checking an unfinished thing would
        // reject it for not being finished. Leave it parked with its question intact.
        if outcome_status(&turn) != NodeStatus::Done {
            return Ok(Attempted::settled(turn, &node, attempt, changed));
        }

        let reason = match check(store, node_run_id, worktree, rig, verifier).await? {
            Verdict::Accepted => return Ok(Attempted::settled(turn, &node, attempt, changed)),
            Verdict::Rejected(reason) => reason,
        };

        record_rejection(
            store,
            run_id,
            node_run_id,
            slice_key,
            &node.role,
            attempt,
            &reason,
        )?;

        // A cap reached mid-repair stops the repairing. Otherwise a node in a retry
        // loop spends the whole run's budget proving it cannot do the slice.
        if let Some(exceeded) = crate::guardrails::node_may_continue(store, node_run_id)? {
            return Ok(Attempted::stopped(
                turn,
                &node,
                attempt,
                changed,
                format!("{}; last rejection: {reason}", exceeded.reason),
            ));
        }
        if attempt >= max_repairs {
            return Ok(Attempted::stopped(turn, &node, attempt, changed, reason));
        }
        attempt += 1;
        instruction = repair_prompt(slice_key, &reason);
    }
}

/// One turn against one node row, with the process always stopped afterwards.
/// One turn by one seat, in its lease.
///
/// There is nothing to start and nothing to stop: a Pi turn is a child process that
/// exits when it is done, and the process group goes with it (D20). The eve version of
/// this function existed largely to make sure a server was not left holding a port.
async fn take_turn(
    store: &mut Store,
    rig: &Rig,
    agent_id: i64,
    node_run_id: i64,
    worktree: &Path,
    instruction: &str,
) -> Result<TurnOutcome> {
    // Anything said to this seat while it was busy goes in front of the instruction, in
    // the order it was said (M9-S42). A correction is only worth anything before the work
    // it corrects, so it leads rather than trails.
    let waiting = store.take_pending(agent_id)?;
    let instruction = if waiting.is_empty() {
        instruction.to_string()
    } else {
        store.append_event(
            store.node_run(node_run_id)?.run_id,
            crate::model::NewEvent::new(
                crate::model::EventKind::Note,
                format!(
                    "{} message(s) said while this seat was busy were delivered with this turn",
                    waiting.len()
                ),
            ),
        )?;
        format!(
            "Someone said this to you while you were working. Take it into account:\n\n{}\n\n---\n\n{instruction}",
            waiting.join("\n\n")
        )
    };
    let turn = rig.seat(store, agent_id, worktree, &instruction)?;
    let (_, outcome) = crate::run_pi_turn(store, node_run_id, &turn, |_| {}).await?;
    Ok(outcome)
}

/// The work one node produced, and where it lives.
struct Landing<'a> {
    worktree: &'a Path,
    node_run_id: i64,
    slice_key: &'a str,
    title: &'a str,
    changed: &'a [String],
}

/// Move the slice to where this node left it.
///
/// Best-effort throughout: the work is already committed by the time this runs, and a
/// board that could not be updated must not turn a finished node into a failed one.
async fn update_board(
    planner: &Planner,
    slice_key: &str,
    worktree: &Path,
    status: NodeStatus,
    fallout: Option<crate::Fallout>,
    rejection: Option<&str>,
) {
    match status {
        NodeStatus::Done => {
            let _ = planner
                .set_status(slice_key, "in_review", Some("built by ai-team"))
                .await;
            let _ = planner.release(slice_key, worktree).await;
        }
        // Escalated work is still somebody's, so the claim stays and the board says why
        // it is waiting rather than quietly offering it to the next run.
        NodeStatus::Parked => {
            if fallout == Some(crate::Fallout::Escalate) {
                let _ = planner.set_status(slice_key, "blocked", rejection).await;
            }
        }
        // A failed slice goes back to `blocked` carrying its reason rather than staying
        // `ready`: ready would hand the next run the same slice with no memory of why it
        // did not work, and it must not look buildable when it is not.
        _ => {
            let _ = planner.set_status(slice_key, "blocked", rejection).await;
            let _ = planner.release(slice_key, worktree).await;
        }
    }
}

/// Turn a stopped attempt into a node status, applying the team's failure policy.
///
/// All three policies leave the siblings alone - that is the property this slice exists
/// to guarantee. They differ only in what happens to *this* branch.
#[allow(clippy::too_many_arguments)]
fn settle_status(
    store: &mut Store,
    run_id: i64,
    node_run_id: i64,
    role: &str,
    slice_key: &str,
    outcome: &TurnOutcome,
    rejection: Option<&str>,
    attempt: i64,
    exhausted: bool,
) -> Result<(NodeStatus, Option<crate::Fallout>)> {
    let status = outcome_status(outcome);
    let Some(reason) = rejection else {
        return Ok((status, None));
    };

    // It stopped without being accepted. Recording it as done would put a branch on the
    // board that nothing checked.
    let policy = crate::Fallout::of(store.run(run_id)?.on_failure);
    let status = match policy {
        // Escalate parks it: somebody asked to be asked, so the work stays claimed and
        // its branch is still written for them to look at.
        crate::Fallout::Escalate => NodeStatus::Parked,
        crate::Fallout::AbortBranch => NodeStatus::Failed,
    };
    let detail = if exhausted {
        format!("stopped after {attempt} repair(s): {reason}")
    } else {
        reason.to_string()
    };
    store.block_node(node_run_id, &detail)?;
    store.append_event(
        run_id,
        NewEvent::new(
            EventKind::Note,
            format!("{slice_key}: {} - {detail}", policy.as_str()),
        )
        .on_node(node_run_id)
        .by(role),
    )?;
    Ok((status, Some(policy)))
}

/// Open a row for one attempt.
///
/// A fresh row per attempt, never an edit (D2). The first attempt's evidence is what
/// analytics is made of, and reusing one row would also hand the next turn the previous
/// session id and stream cursor - which is exactly how a repaired turn ends up reading a
/// stream that has already finished.
///
/// The row is opened before the budget is checked, so a refusal has somewhere to be
/// recorded; it is only marked running once the budget allows it.
fn open_attempt(
    store: &mut Store,
    run_id: i64,
    agent_id: i64,
    slice_key: &str,
    registry: &ModelRegistry,
    worktree: &Path,
) -> Result<crate::model::NodeRun> {
    let node = store.dispatch(run_id, agent_id, Some(slice_key), registry)?;
    store.attach_worktree(node.id, &worktree.to_string_lossy(), None, None)?;
    Ok(node)
}

/// Keep what this attempt was told, on this attempt's own row.
///
/// Each rejected attempt carries its own verdict rather than the last one overwriting
/// what the earlier ones found.
fn record_rejection(
    store: &mut Store,
    run_id: i64,
    node_run_id: i64,
    slice_key: &str,
    role: &str,
    attempt: i64,
    reason: &str,
) -> Result<()> {
    store.append_event(
        run_id,
        NewEvent::new(
            EventKind::Note,
            format!("{slice_key} rejected on attempt {}", attempt + 1),
        )
        .on_node(node_run_id)
        .by(role)
        .with(serde_json::json!({ "reason": reason })),
    )?;
    store.block_node(node_run_id, &truncate_reason(reason))?;
    store.set_node_status(node_run_id, NodeStatus::Failed)?;
    Ok(())
}

/// A rejection can be a whole build log; `blocked_reason` is a column somebody reads in
/// a table. The full evidence is on the event beside it.
fn truncate_reason(reason: &str) -> String {
    let first = reason
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(reason);
    if first.chars().count() <= 200 {
        return first.to_string();
    }
    first.chars().take(199).collect::<String>() + "…"
}

/// The seat that checks the makers, resolved once per run.
#[derive(Debug, Clone)]
struct VerifierSeat {
    agent_id: i64,
    registry: ModelRegistry,
}

/// The answer to "is this actually done?".
enum Verdict {
    Accepted,
    Rejected(String),
}

/// Run the project's own gates, then let the verifier read what the maker left.
///
/// Gates first, because they are cheap, objective, and their output is the evidence the
/// verifier reasons from. A model asked to judge without them is guessing.
async fn check(
    store: &mut Store,
    node_run_id: i64,
    worktree: &Path,
    rig: &Rig,
    verifier: Option<&VerifierSeat>,
) -> Result<Verdict> {
    let gates = crate::gates::discover_gates(worktree);
    if gates.is_empty() {
        // No manifest, so nothing to run. Say so rather than treating silence as a pass:
        // the verifier still reads the diff, and the human should know what was skipped.
        store.append_event(
            store.node_run(node_run_id)?.run_id,
            NewEvent::new(
                EventKind::Note,
                "no gates found in this worktree - nothing automatic was run",
            )
            .on_node(node_run_id),
        )?;
    }

    let results = crate::gates::run_gates(worktree, &gates).await?;
    let evidence = crate::gates::evidence(&results);
    let run_id = store.node_run(node_run_id)?.run_id;
    for result in &results {
        store.append_event(
            run_id,
            NewEvent::new(
                if result.passed {
                    EventKind::Note
                } else {
                    EventKind::Failed
                },
                format!(
                    "gate {} `{}` {}",
                    result.gate.kind.as_str(),
                    result.gate.command(),
                    if result.passed { "passed" } else { "failed" }
                ),
            )
            .on_node(node_run_id)
            // Structured, not only prose. Analytics reads `passed` and `gate` from
            // here; a rate computed by matching English in `summary` breaks the first
            // time somebody improves the wording (rule 8).
            .with(serde_json::json!({
                "gate": result.gate.kind.as_str(),
                "command": result.gate.command(),
                "passed": result.passed,
                "output": result.output,
            })),
        )?;
    }

    if !gates.is_empty() && !crate::gates::all_passed(&results) {
        // A failing gate is already a rejection with evidence attached. Asking a model to
        // confirm it would spend a turn to reach the same answer.
        return Ok(Verdict::Rejected(evidence));
    }

    let Some(verifier) = verifier else {
        // No verifier seat on this team. The gates are the whole check, and they passed.
        return Ok(Verdict::Accepted);
    };

    let node = store.dispatch(run_id, verifier.agent_id, None, &verifier.registry)?;
    store.attach_worktree(node.id, &worktree.to_string_lossy(), None, None)?;
    store.set_node_status(node.id, NodeStatus::Running)?;

    // Captured off the stream rather than read back out of the event table. A node's
    // Note events also carry ai-team's own bookkeeping - the model-fallback notice, for
    // one - and parsing that as the verifier's answer rejects work it never looked at.
    let mut said = String::new();
    let verdict = async {
        let turn = rig.seat(
            store,
            verifier.agent_id,
            worktree,
            &verify_prompt(&evidence),
        )?;
        let (_, outcome) = crate::run_pi_turn(store, node.id, &turn, |event| {
            if let Some(message) = event.assistant_message() {
                said = message;
            }
        })
        .await?;
        Ok::<_, Error>(outcome)
    }
    .await;

    let outcome = match verdict {
        Ok(outcome) => outcome,
        Err(error) => {
            store.set_node_status(node.id, NodeStatus::Failed)?;
            // A verifier that could not run has not approved anything.
            return Ok(Verdict::Rejected(format!(
                "the verifier could not run: {error}"
            )));
        }
    };
    store.set_node_status(node.id, outcome_status(&outcome))?;
    Ok(read_verdict(&said))
}

/// Read the verifier's answer.
///
/// Fails closed: anything that is not an explicit pass is a rejection. A verifier whose
/// reply could not be read has not approved the work, and defaulting the other way is
/// how an unchecked branch reaches somebody who trusted that this ran.
fn read_verdict(said: &str) -> Verdict {
    let lower = said.to_lowercase();
    if let Some(at) = lower.rfind("verdict:") {
        let tail = &lower[at + "verdict:".len()..];
        if tail.trim_start().starts_with("pass") {
            return Verdict::Accepted;
        }
    }
    Verdict::Rejected(if said.trim().is_empty() {
        "the verifier said nothing".to_string()
    } else {
        said.trim().to_string()
    })
}

fn verify_prompt(evidence: &str) -> String {
    format!(
        "Check the work in this worktree. `git diff HEAD` and `git status` show what \
         changed.\n\n\
         The project's own gates have already been run:\n\n{evidence}\n\n\
         Decide whether this is actually done, and answer with `VERDICT: pass` or \
         `VERDICT: reject` on its own line."
    )
}

fn repair_prompt(slice_key: &str, reason: &str) -> String {
    format!(
        "Your work on {slice_key} was rejected. Fix it in this worktree.\n\n\
         {reason}\n\n\
         Address the specific problem above rather than starting over, and run the \
         project's own checks yourself before you finish."
    )
}

/// What a seat is actually asked to do with a slice.
fn slice_prompt(slice: &Slice, house: &[crate::house::Rules]) -> String {
    use std::fmt::Write as _;

    let mut prompt = format!("Build this slice: {} - {}\n", slice.key, slice.title);
    if let Some(scope) = &slice.scope_md {
        let _ = write!(prompt, "\nScope:\n{}\n", scope.trim());
    }
    if let Some(demo) = &slice.demo_md {
        let _ = write!(prompt, "\nIt is done when this is true:\n{}\n", demo.trim());
    }
    prompt.push_str(
        "\nYou are in a worktree leased for this slice alone. Work only inside it, run \
         the project's own checks before you call it done, and say so plainly if they \
         do not pass.\n",
    );
    // Last, and after the worktree rules, so the closest thing to the model's final
    // attention is the standard its work will be judged against.
    prompt.push_str(&crate::house::section(house));
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neighbours::Slice;

    fn slice(key: &str, scope: &str) -> Slice {
        serde_json::from_value(serde_json::json!({
            "key": key,
            "title": "a slice",
            "status": "ready",
            "ord": 10,
            "scope_md": scope,
            "demo_md": "it runs",
        }))
        .unwrap()
    }

    fn team() -> (Store, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        (store, team.id)
    }

    #[test]
    fn a_slice_goes_to_the_seat_whose_zone_owns_its_paths() {
        let (store, team_id) = team();
        let (routed, unrouted) = route(
            &store,
            team_id,
            vec![
                slice("PR2", "Touches: ui/src/App.tsx"),
                slice("PR1", "Touches: crates/widget/src/lib.rs"),
            ],
        )
        .unwrap();

        assert!(unrouted.is_empty(), "{unrouted:?}");
        // Both routed, and in the plan's own order rather than the order they arrived.
        assert_eq!(
            routed
                .iter()
                .map(|(slice, _, role)| (slice.key.as_str(), role.as_str()))
                .collect::<Vec<_>>(),
            [("PR2", "frontend"), ("PR1", "backend")]
        );
    }

    #[test]
    fn a_slice_nobody_owns_is_reported_rather_than_given_to_somebody() {
        // Guessing an owner is how two agents end up in one file, and how work lands in
        // a zone its seat never agreed to hold.
        let (store, team_id) = team();
        let (routed, unrouted) = route(
            &store,
            team_id,
            vec![
                slice("PR1", "Touches: docs/architecture.md"),
                slice("PR2", "no trailer at all"),
            ],
        )
        .unwrap();

        assert!(routed.is_empty());
        assert_eq!(unrouted[0].0, "PR1");
        assert!(unrouted[0].1.contains("no seat owns"), "{unrouted:?}");
        assert_eq!(unrouted[1].0, "PR2");
        assert!(unrouted[1].1.contains("names no paths"), "{unrouted:?}");
    }

    #[test]
    fn a_slice_somebody_else_holds_is_left_alone() {
        let (store, team_id) = team();
        let mut taken = slice("PR1", "Touches: crates/**");
        taken.claimed_by = Some("another worktree".into());
        let mut done = slice("PR2", "Touches: ui/**");
        done.status = "done".into();

        let (routed, unrouted) = route(&store, team_id, vec![taken, done]).unwrap();
        assert!(routed.is_empty(), "{routed:?}");
        // Not "unroutable" either: there is nothing wrong with them, they are just not
        // this run's to take.
        assert!(unrouted.is_empty(), "{unrouted:?}");
    }

    #[test]
    fn a_verdict_that_is_not_an_explicit_pass_is_a_rejection() {
        // Fails closed on purpose. A verifier whose answer could not be read has not
        // approved anything, and defaulting the other way is how an unchecked branch
        // reaches the person who trusted that this ran.
        assert!(matches!(
            read_verdict("Looks right to me.\nVERDICT: pass"),
            Verdict::Accepted
        ));
        assert!(matches!(read_verdict("verdict:   PASS"), Verdict::Accepted));

        for said in [
            "VERDICT: reject - subtract() returns 0",
            "This all looks good and complete!",
            "I could not find the files.",
            "",
        ] {
            assert!(
                matches!(read_verdict(said), Verdict::Rejected(_)),
                "{said:?} must not read as approval"
            );
        }

        // The last verdict wins: a model that reconsiders mid-answer means the later one.
        assert!(matches!(
            read_verdict("VERDICT: pass\n...actually no.\nVERDICT: reject"),
            Verdict::Rejected(_)
        ));
    }

    #[test]
    fn a_rejection_carries_what_the_maker_needs_to_act_on() {
        let Verdict::Rejected(reason) = read_verdict("VERDICT: reject - subtract() returns 0")
        else {
            panic!("expected a rejection");
        };
        assert!(reason.contains("subtract() returns 0"));

        // And that reason is what reaches the maker, not a bare "it failed".
        let prompt = repair_prompt("PR1", &reason);
        assert!(prompt.contains("PR1"));
        assert!(prompt.contains("subtract() returns 0"));
        assert!(prompt.contains("rather than starting over"));
    }

    #[test]
    fn the_prompt_carries_the_scope_and_the_demo() {
        let prompt = slice_prompt(&slice("PR1", "Add the export.\n\nTouches: crates/**"), &[]);
        assert!(prompt.contains("PR1 - a slice"));
        assert!(prompt.contains("Add the export."));
        assert!(prompt.contains("it runs"), "the demo is what done means");
        assert!(prompt.contains("leased for this slice alone"));

        // A repo with no house rules gets no section at all: most have none, and an
        // empty heading spends context telling a model nothing.
        assert!(!prompt.contains("house rules"));

        let with_house = slice_prompt(
            &slice("PR1", "Add the export.\n\nTouches: crates/**"),
            &[crate::house::Rules {
                path: "AGENTS.md".into(),
                body: "Never use an em dash.".into(),
                truncated: false,
            }],
        );
        assert!(with_house.contains("Never use an em dash"), "{with_house}");
        // After the worktree rules, so the last thing read is the standard the work is
        // judged against.
        assert!(
            with_house.find("leased for this slice").unwrap()
                < with_house.find("Never use an em dash").unwrap()
        );
    }
}
