//! One prompt, into a plan, into several PRs building in parallel.
//!
//! The split is D12's: the orchestrator *node* grounds the request and coordinates the
//! graph, the planner node alone shapes the ai-planner plan, and this module - Rust -
//! reads approved slices back, leases a worktree per slice, and hands each to its crew:
//! the seats its tasks name, or the seat whose zone owns its paths when it has none.
//! How one PR is then built lives in `build`. Scheduling, budgets and routing are the
//! control plane's job; an agent spawning sibling agents would be the same work with no
//! guardrails around it.
//!
//! Nothing here stores a plan or a slice. `node_run.slice_key` is a reference into
//! ai-planner and that is the whole of the coupling (D4).

use std::path::{Path, PathBuf};

use super::build::{run_pr, truncate_reason, Assignment, PrTask, Start, VerifierSeat};
use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::model::{Agent, EventKind, NewEvent, NodeStatus};
use crate::neighbours::{git, Planner, Slice, Worktrees};
use crate::store::Store;

use crate::supervise::outcome::{outcome_status, TurnOutcome};
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

/// An interrupted PR taken back - its lease reattached, its seats' claims held - and not
/// yet building.
pub(crate) struct ResumedPr(PrTask);

impl ResumedPr {
    /// Finish the PR where it stopped.
    pub(crate) async fn build(self) -> Result<Dispatched> {
        // Boxed: a whole PR's build is a large future, and every caller of this one would
        // otherwise carry it inline.
        Box::pin(run_pr(self.0)).await
    }
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
    /// The run's plan, named to every seat's ai-planner server. `None` before it exists,
    /// when no seat gets the planning tools at all.
    pub plan: Option<String>,
    pub sources: Vec<crate::ContextSource>,
    pub registry: ModelRegistry,
    /// The Pi to run, when it is not `pi` on the `PATH`. Only tests set it.
    pub pi: Option<PathBuf>,
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
        node_run_id: i64,
        worktree: &Path,
        prompt: &str,
    ) -> Result<crate::PiTurn> {
        self.seated(store, agent_id, node_run_id, worktree, |seat| {
            seat.turn(prompt)
        })
    }

    /// The turn the coordinating seat takes to restack a pull request (PW11): the same
    /// seat, writing this once.
    pub(super) fn restack_seat(
        &self,
        store: &Store,
        agent_id: i64,
        node_run_id: i64,
        worktree: &Path,
        prompt: &str,
    ) -> Result<crate::PiTurn> {
        self.seated(store, agent_id, node_run_id, worktree, |seat| {
            seat.restack_turn(prompt)
        })
    }

    fn seated(
        &self,
        store: &Store,
        agent_id: i64,
        node_run_id: i64,
        worktree: &Path,
        take: impl FnOnce(&crate::PiSeat<'_>) -> Result<crate::PiTurn>,
    ) -> Result<crate::PiTurn> {
        let agent = store.agent(agent_id)?;
        let node = store.node_run(node_run_id)?;
        let team = store.team(agent.team_id)?;
        let roster = store.agents(agent.team_id)?;
        // Only the seats that may shape the plan get the tools that shape it. A maker
        // that can add slices can give itself work.
        let may_plan = agent.role == "planner";
        let mut turn = take(&crate::PiSeat {
            agent: &agent,
            provider: node.provider,
            model: &node.model,
            worktree,
            support: &self.support,
            sources: &self.sources,
            plan: self.plan.as_deref().map(|plan| crate::PiPlanAccess {
                root: &self.plan_root,
                plan,
                may_write: may_plan,
            }),
            team: &team,
            roster: &roster,
        })?;
        turn.program.clone_from(&self.pi);
        Ok(turn)
    }
}

/// Everything a run needs that does not change between its nodes.
#[derive(Debug)]
pub struct Orchestrator {
    pub db_path: PathBuf,
    /// Where the guard and the per-seat MCP configs live. Never a lease.
    pub project_dir: PathBuf,
    /// The checkout the plan and the worktree pool belong to.
    pub repo: PathBuf,
    pub run_id: i64,
    pub team_id: i64,
    pub registry: ModelRegistry,
    pub parallel_width: usize,
    pub planner: Planner,
    pub worktrees: Worktrees,
}

fn grounding_prompt(prompt: &str) -> String {
    format!(
        "Ground this request for the planner seat. Read the repository and every \
         required ClickUp or Figma source before writing a delegation brief. If a \
         required context server or tool is unavailable, answer with \
         `CONTEXT_UNAVAILABLE:` followed by the source and reason; do not use a \
         browser or generic web search instead.\n\n\
         Begin the brief with one line naming the work, which becomes the title of the \
         plan the planner writes:\n\n```\nPlan: <a short title for this work>\n```\n\n\
         Operator request:\n\n{prompt}"
    )
}

/// The title the orchestrator gave the work in its brief (`Plan: ...`), or the first line
/// of the request when it gave none.
fn plan_title(brief: &str, prompt: &str) -> String {
    const LIMIT: usize = 80;
    let named = brief.lines().find_map(|line| {
        let line = line.trim().trim_matches(['*', '#', '`', ' ']);
        let (label, title) = line.split_once(':')?;
        label
            .trim_matches(['*', '`', ' '])
            .eq_ignore_ascii_case("plan")
            .then(|| title.trim().trim_matches(['*', '`', '"', ' ']).to_string())
    });
    let title = named
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| prompt.lines().next().unwrap_or(prompt).trim().to_string());
    if title.chars().count() <= LIMIT {
        return title;
    }
    let clipped: String = title.chars().take(LIMIT - 1).collect();
    let cut = clipped.rfind(' ').unwrap_or(clipped.len());
    format!("{}…", clipped[..cut].trim_end())
}

fn planner_prompt(prompt: &str, brief: &str, plan: &str, title: &str) -> String {
    format!(
        "This run's plan is `{plan}` - \"{title}\" - and ai-team has already created it. \
         Every ai-planner call you make acts on it; shape it, and do not start another or \
         touch any other plan. Write its sections, decisions and slices from the \
         orchestrator's grounded delegation brief below, and leave every buildable slice \
         ready. Do not write source code or dispatch agents.\n\nOriginal request:\n\n\
         {prompt}\n\n---\n\nOrchestrator delegation brief:\n\n{brief}"
    )
}

impl Orchestrator {
    /// What every Pi turn in this run shares.
    pub fn rig(&self) -> Rig {
        Rig {
            support: self.project_dir.clone(),
            plan_root: self.repo.clone(),
            plan: self.planner.plan_slug().map(str::to_string),
            sources: self.registry.context_sources(),
            registry: self.registry.clone(),
            pi: None,
        }
    }

    /// The same, for a run whose plan is `plan` - or, `None`, has none yet.
    fn rig_for(&self, plan: Option<&str>) -> Rig {
        Rig {
            plan: plan.map(str::to_string),
            ..self.rig()
        }
    }

    /// Phase one: the orchestrator grounds the request, then delegates plan construction
    /// to the planner seat. They are deliberately separate visible nodes: the
    /// orchestrator coordinates the graph; only the planner can shape its board.
    pub async fn plan<F>(
        &self,
        store: &mut Store,
        prompt: &str,
        mut on_event: F,
    ) -> Result<TurnOutcome>
    where
        F: FnMut(&crate::PiEvent) + Send,
    {
        let agents = store.agents(self.team_id)?;
        let orchestrator = agents
            .iter()
            .find(|agent| agent.role == ROOT_ROLE && agent.enabled)
            .ok_or_else(|| Error::invalid("the team has no enabled orchestrator"))?;
        let planner = agents
            .iter()
            .find(|agent| agent.role == "planner" && agent.enabled)
            .ok_or_else(|| Error::invalid("the team has no enabled planner"))?;

        let grounding_prompt = grounding_prompt(prompt);
        let mut brief = String::new();
        let mut context_failure = None;
        // No plan yet, so no planning tools: the orchestrator grounds the request in the
        // repository and its context, and the board is the planner's.
        let (coordinator, coordinator_outcome) = match take_fresh_turn(
            store,
            &self.rig_for(None),
            self.run_id,
            orchestrator.id,
            None,
            &self.repo,
            &grounding_prompt,
            |event| {
                if let Some(message) = event.assistant_message() {
                    brief = message;
                }
                if let Some(failure) = event.context_tool_failure() {
                    context_failure = Some(failure);
                }
                on_event(event);
            },
        )
        .await?
        {
            FreshTurn::Finished(finished) => *finished,
            FreshTurn::Unavailable { reason } => {
                return Err(Error::invalid(format!("MODEL_UNAVAILABLE: {reason}")));
            }
        };
        let coordinator_status = outcome_status(&coordinator_outcome);
        if coordinator_status != NodeStatus::Done {
            return Ok(coordinator_outcome);
        }
        if let Some(failure) = context_failure {
            store.block_node(coordinator.id, &failure)?;
            return Err(Error::invalid(failure));
        }
        if brief.trim().is_empty() {
            store.block_node(
                coordinator.id,
                "the orchestrator produced no delegation brief",
            )?;
            return Err(Error::invalid(
                "the orchestrator produced no delegation brief",
            ));
        }
        if brief.contains("CONTEXT_UNAVAILABLE:") {
            store.block_node(coordinator.id, brief.trim())?;
            return Err(Error::invalid(brief.trim()));
        }

        self.delegate(store, planner.id, prompt, &brief, &mut on_event)
            .await
    }

    /// The planner's turn: the orchestrator's brief in, this run's plan out.
    ///
    /// The plan is created here, before the turn, so the planner can be pointed at it by
    /// name (`AI_PLANNER_PLAN`) rather than left to create one and hope later calls find
    /// it. Based on the default branch: every slice copies the plan's base as it is added,
    /// and pull requests target the trunk, not the branch this checkout coordinates on.
    async fn delegate<F>(
        &self,
        store: &mut Store,
        planner_id: i64,
        prompt: &str,
        brief: &str,
        on_event: &mut F,
    ) -> Result<TurnOutcome>
    where
        F: FnMut(&crate::PiEvent) + Send,
    {
        if crate::skills::find(&self.repo, crate::skills::PLANNING_SKILL).is_none() {
            store.append_event(
                self.run_id,
                NewEvent::new(
                    EventKind::Note,
                    format!(
                        "planning without the {} skill: it is not installed here. `ait \
                         doctor` says how to add it",
                        crate::skills::PLANNING_SKILL
                    ),
                )
                .by("orchestrator"),
            )?;
        }
        let trunk = git::trunk(&self.repo).await.ok();
        let title = plan_title(brief, prompt);
        let plan = self
            .planner
            .create(&title, trunk.as_ref().map(|trunk| trunk.name.as_str()))
            .await?;
        store.set_run_plan(self.run_id, &plan)?;
        store.append_event(
            self.run_id,
            NewEvent::new(EventKind::Note, format!("this run's plan is {plan}")).by("orchestrator"),
        )?;
        let planner_prompt = planner_prompt(prompt, brief, &plan, &title);
        let mut context_failure = None;
        let (planning, planner_outcome) = match take_fresh_turn(
            store,
            &self.rig_for(Some(&plan)),
            self.run_id,
            planner_id,
            None,
            &self.repo,
            &planner_prompt,
            |event| {
                if let Some(failure) = event.context_tool_failure() {
                    context_failure = Some(failure);
                }
                on_event(event);
            },
        )
        .await?
        {
            FreshTurn::Finished(finished) => *finished,
            FreshTurn::Unavailable { reason } => {
                return Err(Error::invalid(format!("MODEL_UNAVAILABLE: {reason}")));
            }
        };
        if let Some(failure) = context_failure {
            store.block_node(planning.id, &failure)?;
            return Err(Error::invalid(failure));
        }
        // This checkout's plan too, for `aip` in a terminal and the window's board: asked
        // for by name on the run's fresh branch, it is the answer that branch remembers.
        self.planner
            .clone()
            .for_plan(plan.clone())
            .current()
            .await?;
        // Its stack made explicit before anybody approves it: every PR's branch, under
        // the plan's name, and the base a `Stacks on:` line asks for.
        crate::workspace::settle_stack(&self.planner, &plan, crate::stack::Names::Own).await?;
        Ok(planner_outcome)
    }

    /// Resume the exact orchestrator conversation after a human approves its planner's
    /// board. Rust still decides what is dispatched; the model only acknowledges the
    /// transition and calls out any last coordination concern in the same session.
    pub async fn acknowledge_plan_approval(&self, store: &mut Store) -> Result<TurnOutcome> {
        let coordinator = store
            .node_runs(self.run_id)?
            .into_iter()
            .rev()
            .find(|node| node.role == ROOT_ROLE && node.session_id.is_some())
            .ok_or_else(|| Error::invalid("the run has no orchestrator session to resume"))?;
        let agent_id = coordinator
            .agent_id
            .ok_or_else(|| Error::invalid("the run's orchestrator seat no longer exists"))?;
        store.set_node_status(coordinator.id, NodeStatus::Running)?;
        let outcome = take_turn(
            store,
            &self.rig(),
            agent_id,
            coordinator.id,
            &self.repo,
            "The human approved the planner's plan. Acknowledge the approved plan and \
             identify any final routing concern. Do not edit the plan, dispatch agents, \
             or build code; Rust will now route its ready slices.",
        )
        .await?;
        store.set_node_status(coordinator.id, outcome_status(&outcome))?;
        Ok(outcome)
    }

    /// Resume the orchestrator one final time so its context is checkpointed before the
    /// session address is retired. A prose promise does not count: the event stream must
    /// contain a successful ai-planner `write_handoff` tool result.
    pub async fn handoff_before_session_reset(
        &self,
        store: &mut Store,
        node_id: i64,
    ) -> Result<()> {
        let node = store.node_run(node_id)?;
        if node.run_id != self.run_id || node.role != ROOT_ROLE {
            return Err(Error::invalid("that node is not this run's orchestrator"));
        }
        let agent_id = node
            .agent_id
            .ok_or_else(|| Error::invalid("the run's orchestrator seat no longer exists"))?;
        let plan = store.run(self.run_id)?.plan_slug.ok_or_else(|| {
            Error::invalid(
                "the orchestrator session was kept because this run has no ai-planner plan to hand off",
            )
        })?;
        let instruction = format!(
            "Your Pi session is about to be reset. Before it is retired, call the \
             ai-planner `write_handoff` tool for plan `{plan}`. Record the real current \
             state, only gates evidenced in this conversation, and the next concrete \
             action. Do not merely describe a handoff in prose: make the tool call. Do \
             not edit code or reshape the plan."
        );
        let turn = self
            .rig()
            .seat(store, agent_id, node_id, &self.repo, &instruction)?;
        store.set_node_status(node_id, NodeStatus::Running)?;
        let mut wrote_handoff = false;
        let outcome = match crate::run_pi_turn(store, node_id, &turn, |event| {
            wrote_handoff |= event.wrote_planner_handoff();
        })
        .await
        {
            Ok((_, outcome)) => outcome,
            Err(error) => {
                store.set_node_status(node_id, NodeStatus::Failed)?;
                return Err(error);
            }
        };
        let status = outcome_status(&outcome);
        store.set_node_status(node_id, status)?;
        if status != NodeStatus::Done || !wrote_handoff {
            return Err(Error::invalid(
                "the orchestrator session was kept because its ai-planner handoff did not succeed",
            ));
        }
        store.retire_node_session(node_id)?;
        Ok(())
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
    pub(crate) fn routable(&self, store: &Store, slices: Vec<Slice>) -> Result<Routing> {
        route(store, self.team_id, slices)
    }

    /// What a person should know about each PR's crew before approving a plan (PW4).
    ///
    /// Asked of the plan as written, whatever its slices' board status: a slice held for
    /// approval is exactly the one worth checking. Each entry is the slice, what to know,
    /// and whether it stops that PR being built. Given the slices rather than fetching
    /// them, for the reason `offered` and `routable` are separate.
    pub(crate) fn crew_findings(
        &self,
        store: &Store,
        slices: &[Slice],
    ) -> Result<Vec<(String, String, bool)>> {
        let roster = store.agents(self.team_id)?;
        let mut findings: Vec<(String, String, bool)> = crate::stack::problems(slices)
            .into_iter()
            .map(|(key, problem)| (key, problem, true))
            .collect();
        for slice in slices
            .iter()
            .filter(|slice| !matches!(slice.status.as_str(), "done" | "deferred"))
        {
            match crew_for(store, self.team_id, &roster, slice)? {
                Crew::Seated(_, notes) => findings.extend(
                    notes
                        .into_iter()
                        .map(|note| (slice.key.clone(), note, false)),
                ),
                Crew::Unseated(reason) => findings.push((slice.key.clone(), reason, true)),
            }
        }
        Ok(findings)
    }

    /// Phase two: build the ready slices, several at a time.
    ///
    /// Each PR is built by its crew, one seat at a time, in a checkout of its own: the
    /// run's own for a one-PR plan, a held lease otherwise (PW3). PRs run side by side up
    /// to the run's width, and a seat may be at work in two of them at once - they are
    /// different checkouts (PW6). The board is read again after every wave, because a
    /// PR that stacks on another is built once that one is (PW9).
    pub async fn build_slices<F>(
        &self,
        store: &mut Store,
        mut on_progress: F,
    ) -> Result<Orchestration>
    where
        F: FnMut(&str) + Send,
    {
        if let Some(plan) = self.planner.plan_slug() {
            crate::workspace::settle_stack(&self.planner, plan, crate::stack::Names::Keep).await?;
        }
        let mut out = Orchestration::default();
        let mut attempted: Vec<String> = Vec::new();
        let mut reported: Vec<String> = Vec::new();
        let width = self.parallel_width.max(1);

        loop {
            let offered = self.offered().await?;
            let in_place = offered
                .iter()
                .filter(|slice| slice.status != "deferred")
                .count()
                == 1;
            let Routing {
                routed,
                unrouted,
                notes,
                waiting,
            } = self.routable(store, offered)?;

            // Said once per run, not once per wave.
            for (key, reason) in unrouted {
                if attempted.contains(&key) || reported.contains(&key) {
                    continue;
                }
                store.append_event(
                    self.run_id,
                    NewEvent::new(EventKind::Note, format!("{key} not dispatched: {reason}"))
                        .by("orchestrator"),
                )?;
                reported.push(key.clone());
                out.unrouted.push((key, reason));
            }
            for (key, note) in notes {
                if !reported.contains(&format!("{key}: {note}")) {
                    store.append_event(
                        self.run_id,
                        NewEvent::new(EventKind::Note, format!("{key}: {note}")).by("orchestrator"),
                    )?;
                    reported.push(format!("{key}: {note}"));
                }
            }

            // Anything this run already took once is not taken again: a PR that failed
            // is blocked on the board, and one whose preparation failed is reported.
            let wave: Vec<(Slice, Vec<Assignment>)> = routed
                .into_iter()
                .filter(|(slice, _)| !attempted.contains(&slice.key))
                .take(width)
                .collect();
            if wave.is_empty() {
                // What is still waiting now never saw its parent built in this run.
                for (key, reason) in waiting {
                    store.append_event(
                        self.run_id,
                        NewEvent::new(EventKind::Note, format!("{key} waiting: {reason}"))
                            .by("orchestrator"),
                    )?;
                    out.unrouted.push((key, reason));
                }
                break;
            }

            for (slice, _) in &wave {
                attempted.push(slice.key.clone());
            }
            self.run_wave(store, wave, in_place, &mut out, &mut on_progress)
                .await?;
        }
        Ok(out)
    }

    /// Build one wave of PRs side by side, and wait for all of them.
    async fn run_wave<F>(
        &self,
        store: &mut Store,
        wave: Vec<(Slice, Vec<Assignment>)>,
        in_place: bool,
        out: &mut Orchestration,
        on_progress: &mut F,
    ) -> Result<()>
    where
        F: FnMut(&str) + Send,
    {
        let mut tasks = Vec::new();
        for (slice, crew) in wave {
            on_progress(&format!(
                "{} -> {}",
                slice.key,
                crew_roles(&crew).join(", ")
            ));
            match self.prepare(store, &slice, crew, in_place).await {
                Ok(pr) => tasks.push(tokio::spawn(run_pr(pr))),
                Err(error) => {
                    let reason = format!("preparation failed: {error}");
                    store.append_event(
                        self.run_id,
                        NewEvent::new(
                            EventKind::Failed,
                            format!("{} not dispatched: {reason}", slice.key),
                        )
                        .by("orchestrator"),
                    )?;
                    out.unrouted.push((slice.key, reason));
                }
            }
        }

        for task in tasks {
            match task.await {
                Ok(Ok(done)) => out.dispatched.push(done),
                Ok(Err(error)) => {
                    // One PR failing does not abandon its siblings: the others are in
                    // their own worktrees and their work is still worth having.
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
        Ok(())
    }

    /// Put one PR in its checkout, on its branch, claimed - ready to build on its own task.
    ///
    /// A one-PR plan builds in the run's own checkout (PW3): it is on a fresh branch off
    /// the default one, and a lease would be a second copy of the same thing. Anything
    /// more gives each PR a leased worktree, kept after the build - it is where review
    /// comments are worked on and what a stacked PR builds beside - and returned when the
    /// PR is done with (PW10).
    async fn prepare(
        &self,
        store: &mut Store,
        slice: &Slice,
        crew: Vec<Assignment>,
        in_place: bool,
    ) -> Result<PrTask> {
        // Resolve this before claiming the slice. An error after the external claim would
        // otherwise leave work assigned to a lease that is immediately returned.
        let verifier = self.verifier(store)?;
        let branch = slice_branch(slice, self.planner.plan_slug());
        let trunk = git::trunk(&self.repo).await.ok();
        // Before a checkout is touched or a model started: building on the default branch
        // is only ever the run's choice.
        if let Some(trunk) = &trunk {
            let allowed = store.run(self.run_id)?.on_default_branch;
            if let Err(error) =
                crate::workspace::refuse_default_branch(&branch, &trunk.name, allowed)
            {
                let _ = self
                    .planner
                    .set_status(&slice.key, "blocked", Some(&error.to_string()))
                    .await;
                return Err(error);
            }
        }
        let start = start_point(slice.base_branch.as_deref(), trunk.as_ref());

        let (worktree, lease) = if in_place {
            // The operator's own checkout: anything they left there is theirs, not work
            // to commit into somebody's pull request.
            crate::workspace::ensure_clean(&self.repo).await?;
            (self.repo.clone(), None)
        } else {
            let holder = lease_holder(self.run_id, &slice.key, &crew_roles(&crew).join("+"));
            let lease = self.worktrees.lease(&holder).await?;
            (lease.path().to_path_buf(), Some(lease))
        };
        let prepared = async {
            // Name the checkout before the agent starts and keep gate output untracked.
            git::ignore_build_output(&worktree).await?;
            let placed = git::put_on_branch(&worktree, &branch, &start).await?;
            // Measured from where the branch left its base, so a PR continued after review
            // is judged whole rather than from its latest commit.
            let base = git::merge_base(&worktree, &start, "HEAD").await?;
            let setup = crate::gates::prepare_dependencies(&worktree).await?;
            Ok::<_, Error>((placed, base, setup))
        }
        .await;
        let (placed, base, setup) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let reason = format!("dependency/worktree preparation failed: {error}");
                let _ = self
                    .planner
                    .set_status(&slice.key, "blocked", Some(&reason))
                    .await;
                if let Some(lease) = lease {
                    let _ = lease.release().await;
                }
                return Err(Error::invalid(reason));
            }
        };
        let place = if in_place {
            "the run's own checkout".to_string()
        } else {
            worktree.display().to_string()
        };
        let how = match placed {
            git::Placed::Started => format!("starts {branch} from {start}"),
            git::Placed::Continued => format!("continues {branch}, which already has work"),
        };
        self.say(store, format!("{} {how} in {place}", slice.key))?;
        if !setup.is_empty() {
            self.say(
                store,
                format!(
                    "{} prepared dependencies with {}",
                    slice.key,
                    setup.join(", ")
                ),
            )?;
        }

        // Claim through ai-planner, in the PR's checkout, so the board shows where the
        // work is actually happening and a second run is told the slice is taken.
        if !self.planner.claim(&slice.key, &worktree).await? {
            if let Some(lease) = lease {
                let _ = lease.release().await;
            }
            return Err(Error::invalid(format!(
                "{} is claimed by another worktree",
                slice.key
            )));
        }

        Ok(PrTask {
            db_path: self.db_path.clone(),
            rig: self.rig(),
            registry: self.registry.clone(),
            slice: slice.clone(),
            crew,
            branch,
            base,
            worktree,
            lease,
            planner: self.planner.clone(),
            run_id: self.run_id,
            // Read off the run, never back through the team: what this run was allowed
            // to spend is a fact about this run (D2).
            max_repairs: store.run(self.run_id)?.max_repairs,
            verifier,
            start: Start::Fresh,
        })
    }

    /// Reattach an interrupted PR to its exact lease and Pi sessions, ready to finish.
    ///
    /// No branch is prepared and no dependencies are reinstalled here: both could
    /// overwrite evidence left by the interrupted process. The persisted node, planner
    /// claim and awt lease must all agree before a model is started. The PR picks up
    /// where it stopped: the interrupted turn first, then the tasks after it, then the
    /// check - which is [`ResumedPr::build`], separate so that whoever asked for the
    /// resume hears whether it could start before the build takes its minutes.
    pub(crate) async fn prepare_resume(
        &self,
        store: &mut Store,
        node_id: i64,
    ) -> Result<ResumedPr> {
        let node = store.node_run(node_id)?;
        if node.run_id != self.run_id || node.status != NodeStatus::Running {
            return Err(Error::invalid(
                "that node is no longer an interrupted running turn",
            ));
        }
        if node.session_id.is_none() || node.session_retired_at.is_some() {
            return Err(Error::invalid(
                "that interrupted turn has no active Pi session",
            ));
        }
        let pid = i64::from(std::process::id());
        if node.supervisor_pid != Some(pid) {
            return Err(Error::invalid(
                "this process has not claimed the interrupted turn",
            ));
        }
        let slice_key = node
            .slice_key
            .clone()
            .ok_or_else(|| Error::invalid("only interrupted maker slices can be resumed"))?;
        let worktree = node
            .worktree_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| Error::invalid("the interrupted node has no leased worktree"))?;
        let slice = self.planner.slice(&slice_key).await?;
        if !slice.worktree_path.as_deref().is_some_and(|claimed| {
            crate::neighbours::same_worktree(claimed, &worktree.to_string_lossy())
        }) {
            return Err(Error::invalid(
                "the planner claim no longer points at the interrupted worktree",
            ));
        }
        let roster = store.agents(self.team_id)?;
        let crew = match crew_for(store, self.team_id, &roster, &slice)? {
            Crew::Seated(crew, _) => crew,
            Crew::Unseated(reason) => {
                return Err(Error::invalid(format!(
                    "{slice_key} can no longer be built by this team: {reason}"
                )));
            }
        };
        // The rest of the PR's seats that were waiting on the same dead process. Taken
        // over now, because a reply delivered to any of them has to be supervised here.
        for other in store.node_runs(self.run_id)? {
            if other.id != node.id
                && other.slice_key.as_deref() == Some(slice_key.as_str())
                && other.status == NodeStatus::Running
                && other
                    .supervisor_pid
                    .is_some_and(|owner| owner != pid && !crate::util::process_is_alive(owner))
            {
                store.claim_node_supervision(other.id, pid, other.supervisor_pid)?;
            }
        }
        let branch = node
            .branch
            .clone()
            .unwrap_or_else(|| slice_branch(&slice, self.planner.plan_slug()));
        let base = pr_base(&worktree, &slice).await?;
        let verifier = self.verifier(store)?;
        let max_repairs = store.run(self.run_id)?.max_repairs;
        // Last, because nothing may fail while the lease is held here: a `Lease` dropped
        // on an error goes back to the pool, and this one is the PR's until it merges.
        // A one-PR plan was built in the run's own checkout, which nobody leased.
        let lease = if crate::neighbours::same_worktree(
            &worktree.to_string_lossy(),
            &self.repo.to_string_lossy(),
        ) {
            None
        } else {
            let holder = lease_holder(self.run_id, &slice_key, &crew_roles(&crew).join("+"));
            Some(self.worktrees.resume(&worktree, &holder).await?)
        };
        // Not worth the lease, for the same reason.
        let _ = store.append_event(
            self.run_id,
            NewEvent::new(
                EventKind::Note,
                format!(
                    "resuming interrupted {slice_key} turn in {}",
                    worktree.display()
                ),
            )
            .on_node(node.id)
            .by("ai-team"),
        );

        Ok(ResumedPr(PrTask {
            db_path: self.db_path.clone(),
            rig: self.rig(),
            registry: self.registry.clone(),
            slice,
            crew,
            branch,
            base,
            worktree,
            lease,
            planner: self.planner.clone(),
            run_id: self.run_id,
            max_repairs,
            verifier,
            start: Start::Resume(node),
        }))
    }

    /// Work a review's comments into a pull request where it was built (PW10).
    ///
    /// The PR keeps its worktree after it is built, so this goes back into it - on its
    /// branch, continuing the conversation of the seat whose work the comments are about -
    /// and checks the whole PR again, exactly as a build does.
    pub async fn follow_up(
        &self,
        store: &mut Store,
        target: &crate::review::FollowUp,
        comments: String,
    ) -> Result<Dispatched> {
        let slice = self.planner.slice(&target.slice_key).await?;
        let roster = store.agents(self.team_id)?;
        let crew = match crew_for(store, self.team_id, &roster, &slice)? {
            Crew::Seated(crew, _) => crew,
            Crew::Unseated(reason) => {
                return Err(Error::invalid(format!(
                    "{} can no longer be built by this team: {reason}",
                    target.slice_key
                )));
            }
        };
        let lease = if crate::neighbours::same_worktree(
            &target.worktree.to_string_lossy(),
            &self.repo.to_string_lossy(),
        ) {
            None
        } else {
            Some(self.worktrees.reattach(&target.worktree).await?)
        };
        let base = pr_base(&target.worktree, &slice).await?;
        let verifier = self.verifier(store)?;
        store.append_event(
            self.run_id,
            NewEvent::new(
                EventKind::Note,
                format!(
                    "{} takes review comments in {}",
                    target.slice_key,
                    target.worktree.display()
                ),
            )
            .by("ai-team"),
        )?;
        Box::pin(run_pr(PrTask {
            db_path: self.db_path.clone(),
            rig: self.rig(),
            registry: self.registry.clone(),
            slice,
            crew,
            branch: target.branch.clone(),
            base,
            worktree: target.worktree.clone(),
            lease,
            planner: self.planner.clone(),
            run_id: self.run_id,
            max_repairs: store.run(self.run_id)?.max_repairs,
            verifier,
            start: Start::FollowUp {
                node: target.node.clone(),
                comments,
            },
        }))
        .await
    }

    /// A note from the orchestrator on this run.
    fn say(&self, store: &mut Store, note: String) -> Result<()> {
        store.append_event(
            self.run_id,
            NewEvent::new(EventKind::Note, note).by("orchestrator"),
        )?;
        Ok(())
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
            .map(|agent| VerifierSeat { agent_id: agent.id }))
    }
}

/// Who builds each ready slice, task by task; which nobody can, and why; and what is worth
/// a look before they start.
pub(crate) struct Routing {
    routed: Vec<(Slice, Vec<Assignment>)>,
    unrouted: Vec<(String, String)>,
    notes: Vec<(String, String)>,
    /// Slices that stack on one not built yet, and why. Not a failure: they build once
    /// their parent does, in this run or a later one.
    waiting: Vec<(String, String)>,
}

/// Decide who builds what.
///
/// Separated from fetching them so the decision is testable without ai-planner
/// installed: this is the rule that keeps two agents out of one file, and it deserves
/// more than a demo exercising it.
fn route(store: &Store, team_id: i64, slices: Vec<Slice>) -> Result<Routing> {
    let roster = store.agents(team_id)?;
    let mut routing = Routing {
        routed: Vec::new(),
        unrouted: Vec::new(),
        notes: Vec::new(),
        waiting: Vec::new(),
    };
    // The whole plan, not only what is ready: whether a slice can build depends on the
    // slice it stacks on.
    let stack_problems = crate::stack::problems(&slices);

    let mut ready: Vec<&Slice> = slices
        .iter()
        .filter(|slice| slice.is_dispatchable())
        .collect();
    // `ord` is the plan's declared sequence - the order a human put the slices in - and
    // the stack is the only dependency ai-planner records, so it is read from bases.
    ready.sort_by_key(|slice| slice.ord);

    for slice in ready {
        if let Some((_, problem)) = stack_problems.iter().find(|(key, _)| *key == slice.key) {
            routing.unrouted.push((slice.key.clone(), problem.clone()));
            continue;
        }
        if let Some(reason) = crate::stack::waiting_on(slice, &slices) {
            routing.waiting.push((slice.key.clone(), reason));
            continue;
        }
        let slice = slice.clone();
        match crew_for(store, team_id, &roster, &slice)? {
            Crew::Seated(crew, notes) => {
                routing
                    .notes
                    .extend(notes.into_iter().map(|note| (slice.key.clone(), note)));
                routing.routed.push((slice, crew));
            }
            Crew::Unseated(reason) => routing.unrouted.push((slice.key.clone(), reason)),
        }
    }
    Ok(routing)
}

/// A slice's crew, or why it has none.
enum Crew {
    /// The seats that build it, in task order, and anything worth noting about them.
    Seated(Vec<Assignment>, Vec<String>),
    Unseated(String),
}

/// The seats that build one slice.
///
/// Task lines name their owners, and the owner is the authority (PW5): a task is never
/// quietly handed to another seat. An owner the team cannot use leaves the slice unbuilt
/// and says why; a path outside its owner's zone is only noted, because the planner may
/// have meant it. A slice with no task lines is one piece of work for the seat whose zone
/// owns its paths, as every slice was before - and one nobody owns is reported rather than
/// guessed at, because guessing is how two agents end up in one file.
fn crew_for(store: &Store, team_id: i64, roster: &[Agent], slice: &Slice) -> Result<Crew> {
    let listed = slice.tasks();
    if !listed.problems.is_empty() {
        return Ok(Crew::Unseated(format!(
            "its tasks could not be read: {}",
            listed.problems.join("; ")
        )));
    }

    if listed.tasks.is_empty() {
        let paths = slice.touches();
        if paths.is_empty() {
            return Ok(Crew::Unseated("names no paths to route on".to_string()));
        }
        let owner = paths
            .iter()
            .find_map(|path| store.agent_for_path(team_id, path).transpose())
            .transpose()?;
        return Ok(match owner {
            Some(agent) => Crew::Seated(
                vec![Assignment {
                    task: None,
                    agent_id: agent.id,
                    role: agent.role,
                }],
                Vec::new(),
            ),
            None => Crew::Unseated(format!("no seat owns {}", paths.join(", "))),
        });
    }

    let findings = crate::tasks::check(&listed.tasks, roster);
    let blocking: Vec<&str> = findings
        .iter()
        .filter(|finding| finding.blocking)
        .map(|finding| finding.message.as_str())
        .collect();
    if !blocking.is_empty() {
        return Ok(Crew::Unseated(blocking.join("; ")));
    }
    let mut crew = Vec::new();
    for task in listed.tasks {
        // Present and able to build: `check` refuses anything else.
        let Some(owner) = roster
            .iter()
            .find(|agent| agent.role.eq_ignore_ascii_case(&task.owner))
        else {
            return Ok(Crew::Unseated(format!(
                "{} has no owner on this team",
                task.key
            )));
        };
        crew.push(Assignment {
            agent_id: owner.id,
            role: owner.role.clone(),
            task: Some(task),
        });
    }
    Ok(Crew::Seated(
        crew,
        findings
            .into_iter()
            .map(|finding| finding.message)
            .collect(),
    ))
}

/// The roles in a crew, each once, in the order they first build.
fn crew_roles(crew: &[Assignment]) -> Vec<String> {
    let mut roles: Vec<String> = Vec::new();
    for assignment in crew {
        if !roles.contains(&assignment.role) {
            roles.push(assignment.role.clone());
        }
    }
    roles
}

/// The commit an interrupted PR was built on: where its branch left the one it stacks on.
async fn pr_base(worktree: &Path, slice: &Slice) -> Result<String> {
    let trunk = git::trunk(worktree).await.ok();
    let start = start_point(slice.base_branch.as_deref(), trunk.as_ref());
    git::merge_base(worktree, &start, "HEAD").await
}

fn lease_holder(run_id: i64, slice_key: &str, role: &str) -> String {
    format!("ai-team run-{run_id} {slice_key} {role}")
}

/// The run and slice a lease was taken for, when ai-team took it for a pull request.
pub(crate) fn held_by(holder: &str) -> Option<(i64, String)> {
    let mut words = holder.strip_prefix("ai-team run-")?.split_whitespace();
    let run_id = words.next()?.parse().ok()?;
    let slice_key = words.next()?.to_string();
    Some((run_id, slice_key))
}

/// The branch a slice builds on: the plan's name for it, or `<plan>/<key>` - settled onto
/// the plan before a build, so this fallback is for a plan read without one.
fn slice_branch(slice: &Slice, plan: Option<&str>) -> String {
    slice
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map_or_else(
            || match plan {
                Some(plan) => crate::stack::default_branch(plan, &slice.key),
                None => format!("ai-team/{}", slice.key.to_lowercase()),
            },
            str::to_string,
        )
}

/// Where a PR's branch starts: the default branch as origin has it, since a local copy is
/// only as current as the last pull, or the branch of the PR it stacks on, as it is.
fn start_point(base: Option<&str>, trunk: Option<&git::Trunk>) -> String {
    match (base.map(str::trim).filter(|base| !base.is_empty()), trunk) {
        (Some(base), Some(trunk)) if base == trunk.name => trunk.start_point.clone(),
        (Some(base), _) => base.to_string(),
        (None, Some(trunk)) => trunk.start_point.clone(),
        (None, None) => "HEAD".to_string(),
    }
}

pub(super) struct QuotaFailover<'a> {
    registry: &'a ModelRegistry,
    run_id: i64,
    agent_id: i64,
    subject: &'a str,
    node: &'a crate::model::NodeRun,
    quota: &'a str,
}

impl<'a> QuotaFailover<'a> {
    pub(super) fn new(
        registry: &'a ModelRegistry,
        run_id: i64,
        agent_id: i64,
        subject: &'a str,
        node: &'a crate::model::NodeRun,
        quota: &'a str,
    ) -> Self {
        Self {
            registry,
            run_id,
            agent_id,
            subject,
            node,
            quota,
        }
    }
}

pub(super) fn record_quota_and_resolve(
    store: &mut Store,
    exhausted_providers: &mut Vec<crate::Provider>,
    failover: QuotaFailover<'_>,
) -> Result<Option<crate::ModelResolution>> {
    let QuotaFailover {
        registry,
        run_id,
        agent_id,
        subject,
        node,
        quota,
    } = failover;
    let provider = node.provider;
    if !exhausted_providers.contains(&provider) {
        exhausted_providers.push(provider);
    }
    let detail = format!("{provider} subscription unavailable: {quota}");
    store.block_node(node.id, &truncate_reason(&detail))?;
    store.set_node_status(node.id, NodeStatus::Blocked)?;
    store.append_event(
        run_id,
        NewEvent::new(
            EventKind::Note,
            format!("{subject}: {detail}; looking for an eligible fallback"),
        )
        .on_node(node.id)
        .by(&node.role)
        .with(serde_json::json!({
            "availability": "quota_exhausted",
            "provider": provider,
            "model": node.model,
            "reason": quota,
        })),
    )?;

    let agent = store.agent(agent_id)?;
    registry.quota_fallback(
        &agent,
        exhausted_providers,
        &format!("{provider} subscription quota was exhausted"),
    )
}

pub(super) enum FreshTurn {
    Finished(Box<(crate::model::NodeRun, TurnOutcome)>),
    Unavailable { reason: String },
}

/// Start a fresh node and move it across subscription accounts only when Pi reports a
/// recognized quota boundary. Generic model/network/tool failures return normally and
/// retain their ordinary failure semantics.
#[allow(clippy::too_many_arguments)]
pub(super) async fn take_fresh_turn<F>(
    store: &mut Store,
    rig: &Rig,
    run_id: i64,
    agent_id: i64,
    slice_key: Option<&str>,
    worktree: &Path,
    instruction: &str,
    mut on_event: F,
) -> Result<FreshTurn>
where
    F: FnMut(&crate::PiEvent) + Send,
{
    let mut exhausted_providers = Vec::new();
    let mut resolution: Option<crate::ModelResolution> = None;

    loop {
        let node = match resolution.as_ref() {
            Some(resolution) => {
                store.dispatch_with_resolution(run_id, agent_id, slice_key, None, resolution)?
            }
            None => store.dispatch(run_id, agent_id, slice_key, &rig.registry)?,
        };
        store.attach_worktree(node.id, &worktree.to_string_lossy(), None, None)?;
        store.set_node_status(node.id, NodeStatus::Running)?;
        let turn = rig.seat(store, agent_id, node.id, worktree, instruction)?;
        let outcome = match crate::run_pi_turn(store, node.id, &turn, |event| on_event(event)).await
        {
            Ok((_, outcome)) => outcome,
            Err(error) => {
                store.set_node_status(node.id, NodeStatus::Failed)?;
                return Err(error);
            }
        };

        let Some(quota) = crate::supervise::quota_exhaustion(&outcome) else {
            match outcome_status(&outcome) {
                NodeStatus::Failed => record_failed_turn(store, run_id, &node, &outcome)?,
                status => {
                    store.set_node_status(node.id, status)?;
                }
            }
            return Ok(FreshTurn::Finished(Box::new((node, outcome))));
        };
        resolution = record_quota_and_resolve(
            store,
            &mut exhausted_providers,
            QuotaFailover {
                registry: &rig.registry,
                run_id,
                agent_id,
                subject: &node.role,
                node: &node,
                quota: &quota,
            },
        )?;
        if resolution.is_none() {
            return Ok(FreshTurn::Unavailable {
                reason: format!(
                    "every reachable allowed model subscription is unavailable; last response: {quota}"
                ),
            });
        }
    }
}

/// Settle a turn that did not finish as what it was: which seat, on which model, and
/// what Pi said - on its row and as an event the window shows. Everything Pi wrote goes in
/// the event's payload, because a reason is one line and the evidence is not.
fn record_failed_turn(
    store: &mut Store,
    run_id: i64,
    node: &crate::model::NodeRun,
    outcome: &TurnOutcome,
) -> Result<()> {
    let said = crate::supervise::outcome::provider_diagnostic(outcome)
        .unwrap_or_else(|| "Pi said nothing about why".to_string());
    // Nothing streamed means Pi stopped before the model was reached.
    let how = if outcome.recorded == 0 {
        "could not run"
    } else {
        "stopped before finishing"
    };
    let reason = super::build::truncate_reason(&format!(
        "{} on {}/{} {how}: {said}",
        node.role,
        node.provider.as_str(),
        node.model
    ));
    store.fail_node(node.id, &reason)?;
    store.append_event(
        run_id,
        NewEvent::new(EventKind::Failed, &reason)
            .on_node(node.id)
            .by(&node.role)
            .with(serde_json::json!({ "pi": outcome.provider_message })),
    )?;
    Ok(())
}

pub(super) async fn take_replied_turn(
    store: &mut Store,
    rig: &Rig,
    agent_id: i64,
    node_run_id: i64,
    worktree: &Path,
) -> Result<TurnOutcome> {
    take_turn(
        store,
        rig,
        agent_id,
        node_run_id,
        worktree,
        "Continue the same task, taking the person's reply into account.",
    )
    .await
}

/// One turn against one node row, with the process always stopped afterwards.
/// One turn by one seat, in its lease.
///
/// There is nothing to start and nothing to stop: a Pi turn is a child process that
/// exits when it is done, and the process group goes with it (D20). The eve version of
/// this function existed largely to make sure a server was not left holding a port.
pub(super) async fn take_turn(
    store: &mut Store,
    rig: &Rig,
    agent_id: i64,
    node_run_id: i64,
    worktree: &Path,
    instruction: &str,
) -> Result<TurnOutcome> {
    let pid = i64::from(std::process::id());
    let node = store.node_run(node_run_id)?;
    match node.supervisor_pid {
        Some(owner) if owner == pid => {}
        None => {
            store.claim_node_supervision(node_run_id, pid, None)?;
        }
        Some(_) => {
            return Err(Error::invalid(
                "that turn is already supervised by another ai-team process",
            ));
        }
    }
    // Anything said to this seat while it was busy goes in front of the instruction, in
    // the order it was said (M9-S42). A correction is only worth anything before the work
    // it corrects, so it leads rather than trails.
    let waiting = store.take_pending_for(agent_id, node_run_id)?;
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
    let turn = rig.seat(store, agent_id, node_run_id, worktree, &instruction)?;
    let (_, outcome) = crate::run_pi_turn(store, node_run_id, &turn, |_| {}).await?;
    Ok(outcome)
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
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        (store, team.id)
    }

    /// A Pi that cannot start the seat's provider, as Pi 0.87 says so: an extension's
    /// warning, then its own `Error:` line, on stderr, and nothing on stdout.
    const PI_WITHOUT_THE_PROVIDER: &str = r#"#!/bin/sh
echo '[pi-web-access] Dynamic tool activation requires Pi 0.86.1 or newer.' >&2
echo 'Error: Unknown provider "llama.cpp". Use --list-models to see available providers/models.' >&2
exit 1
"#;

    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_pi_could_not_start_says_what_pi_said_and_on_which_model() {
        use std::os::unix::fs::PermissionsExt as _;

        let (mut store, team) = team();
        let project = store.team(team).unwrap().project_id.unwrap();
        let run = store
            .create_run(project, "add a --loud option", crate::RunTrigger::Manual)
            .unwrap();
        let orchestrator = store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == ROOT_ROLE)
            .unwrap();
        let support = tempfile::tempdir().unwrap();
        let pi = support.path().join("pi");
        std::fs::write(&pi, PI_WITHOUT_THE_PROVIDER).unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let rig = Rig {
            support: support.path().into(),
            plan_root: support.path().into(),
            plan: None,
            sources: Vec::new(),
            registry: ModelRegistry::local_only(),
            pi: Some(pi),
        };

        let FreshTurn::Finished(finished) = take_fresh_turn(
            &mut store,
            &rig,
            run.id,
            orchestrator.id,
            None,
            support.path(),
            "ground it",
            |_| {},
        )
        .await
        .unwrap() else {
            panic!("a turn that failed to start is not a quota to fail over");
        };
        let (node, _) = *finished;

        let node = store.node_run(node.id).unwrap();
        assert_eq!(node.status, NodeStatus::Failed);
        let reason = node.blocked_reason.unwrap();
        // Pi's own error leads, and the seat's model is named: that is what says why.
        assert_eq!(
            reason,
            "orchestrator on local/auto could not run: Error: Unknown provider \"llama.cpp\". \
             Use --list-models to see available providers/models."
        );
        let failed = store
            .node_events(node.id, 50)
            .unwrap()
            .into_iter()
            .find(|event| event.kind == EventKind::Failed)
            .expect("the failure is on the turn's own record");
        assert_eq!(failed.summary, reason);
        // Everything Pi wrote is kept as evidence, the extension's line included.
        let payload = failed.payload.unwrap().to_string();
        assert!(payload.contains("[pi-web-access]"), "{payload}");
    }

    #[test]
    fn a_plan_is_titled_by_the_orchestrator_or_else_by_the_request() {
        let brief = "**Plan:** Export the ledger as CSV\n\nOutcome: ...";
        assert_eq!(plan_title(brief, "whatever"), "Export the ledger as CSV");
        assert_eq!(plan_title("plan: `Nest worktrees`", "x"), "Nest worktrees");
        // No title line: the request's first line names it.
        assert_eq!(
            plan_title("Outcome: a thing", "Add a --shout flag\nand a test"),
            "Add a --shout flag"
        );
        // A long one is clipped at a word.
        let long = plan_title("", &"word ".repeat(40));
        assert!(long.chars().count() <= 80, "{long}");
        assert!(long.ends_with("word…"), "{long}");
    }

    #[test]
    fn a_lease_says_which_run_and_pull_request_it_is_held_for() {
        let holder = lease_holder(12, "PR2", "frontend+backend");
        assert_eq!(held_by(&holder), Some((12, "PR2".to_string())));
        // Somebody else's lease, or a sentence where a holder would be.
        assert_eq!(held_by("medusa"), None);
        assert_eq!(held_by("orphaned: machine restarted while in use"), None);
        assert_eq!(held_by("ai-team run-x PR2 backend"), None);
    }

    #[test]
    fn a_lease_holder_names_the_run_slice_and_role() {
        assert_eq!(
            lease_holder(7, "S1", "frontend"),
            "ai-team run-7 S1 frontend"
        );
    }

    #[test]
    fn a_slice_uses_the_branch_the_plan_declared() {
        let mut planned = slice("S1", "Touches: crates/**");
        planned.branch = Some("feature/stack-one".into());
        assert_eq!(slice_branch(&planned, Some("csv")), "feature/stack-one");

        // Named for the plan too: every plan has an S1, and a branch two plans share is one
        // whose review a later run would reset.
        planned.branch = None;
        assert_eq!(slice_branch(&planned, Some("csv")), "csv/s1");
        assert_eq!(slice_branch(&planned, None), "ai-team/s1");
    }

    #[test]
    fn a_slice_goes_to_the_seat_whose_zone_owns_its_paths() {
        let (store, team_id) = team();
        let Routing {
            routed, unrouted, ..
        } = route(
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
                .map(|(slice, crew)| (slice.key.as_str(), crew_roles(crew)))
                .collect::<Vec<_>>(),
            [
                ("PR2", vec!["frontend".to_string()]),
                ("PR1", vec!["backend".to_string()])
            ]
        );
        assert!(routed.iter().all(|(_, crew)| crew[0].task.is_none()));
    }

    #[test]
    fn a_slice_nobody_owns_is_reported_rather_than_given_to_somebody() {
        // Guessing an owner is how two agents end up in one file, and how work lands in
        // a zone its seat never agreed to hold.
        let (store, team_id) = team();
        let Routing {
            routed, unrouted, ..
        } = route(
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

        let Routing {
            routed, unrouted, ..
        } = route(&store, team_id, vec![taken, done]).unwrap();
        assert!(routed.is_empty());
        // Not "unroutable" either: there is nothing wrong with them, they are just not
        // this run's to take.
        assert!(unrouted.is_empty(), "{unrouted:?}");
    }

    #[test]
    fn a_pr_with_tasks_is_built_by_the_seats_they_name_in_their_order() {
        let (store, team_id) = team();
        let Routing {
            routed,
            unrouted,
            notes,
            ..
        } = route(
            &store,
            team_id,
            vec![slice(
                "PR1",
                "As an operator, I want a range picker.\n\n## Tasks\n\
                 - T1 [backend] Store the range - Touches: crates/widget/src/range.rs\n\
                 - T2 [frontend] Pick the range - Touches: ui/src/Range.tsx, crates/widget/src/api.rs\n\
                 - T3 [backend] Export it - Touches: crates/widget/src/export.rs\n\n\
                 Touches: crates/**, ui/**",
            )],
        )
        .unwrap();

        assert!(unrouted.is_empty(), "{unrouted:?}");
        let crew = &routed[0].1;
        assert_eq!(
            crew.iter()
                .map(|assignment| (
                    assignment.task.as_ref().unwrap().key.as_str(),
                    assignment.role.as_str()
                ))
                .collect::<Vec<_>>(),
            [("T1", "backend"), ("T2", "frontend"), ("T3", "backend")]
        );
        // One seat, once, in the order it first builds: that is what a wave holds.
        assert_eq!(crew_roles(crew), ["backend", "frontend"]);
        // The owner stands (PW5); a path outside its zone is only pointed out.
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].1.contains("crates/widget/src/api.rs"), "{notes:?}");
    }

    #[test]
    fn a_stacked_pr_waits_for_its_parent_and_a_broken_stack_is_reported() {
        let (store, team_id) = team();
        let stacked = |key: &str, status: &str, branch: &str, base: &str| {
            let mut slice = slice(key, "Touches: crates/**");
            slice.status = status.into();
            slice.branch = Some(branch.into());
            slice.base_branch = Some(base.into());
            slice
        };
        let parent = stacked("PR1", "active", "p/pr1", "main");
        let child = stacked("PR2", "ready", "p/pr2", "p/pr1");
        let sibling = stacked("PR3", "ready", "p/pr3", "main");
        let mut orphan = stacked("PR4", "ready", "p/pr4", "main");
        orphan.scope_md = Some("Stacks on: PR9\n\nTouches: crates/**".into());

        let Routing {
            routed,
            unrouted,
            waiting,
            ..
        } = route(
            &store,
            team_id,
            vec![parent.clone(), child.clone(), sibling.clone(), orphan],
        )
        .unwrap();
        // The sibling builds now; the child waits for PR1 to be built, not merged.
        assert_eq!(
            routed
                .iter()
                .map(|(s, _)| s.key.as_str())
                .collect::<Vec<_>>(),
            ["PR3"]
        );
        assert_eq!(waiting[0].0, "PR2");
        assert!(waiting[0].1.contains("stacks on PR1"), "{waiting:?}");
        assert_eq!(unrouted[0].0, "PR4");
        assert!(unrouted[0].1.contains("PR9"), "{unrouted:?}");

        let mut built = parent;
        built.status = "in_review".into();
        let Routing { routed, .. } = route(&store, team_id, vec![built, child, sibling]).unwrap();
        assert_eq!(
            routed
                .iter()
                .map(|(s, _)| s.key.as_str())
                .collect::<Vec<_>>(),
            ["PR2", "PR3"]
        );
    }

    #[test]
    fn a_prs_branch_starts_from_the_default_branch_as_origin_has_it() {
        let trunk = git::Trunk {
            name: "main".into(),
            start_point: "origin/main".into(),
        };
        assert_eq!(start_point(Some("main"), Some(&trunk)), "origin/main");
        assert_eq!(start_point(None, Some(&trunk)), "origin/main");
        assert_eq!(start_point(Some("p/pr1"), Some(&trunk)), "p/pr1");
        assert_eq!(start_point(Some("main"), None), "main");
        assert_eq!(start_point(None, None), "HEAD");
    }

    #[test]
    fn a_pr_whose_tasks_the_team_cannot_build_is_reported_not_rerouted() {
        let (store, team_id) = team();
        let Routing {
            routed, unrouted, ..
        } = route(
            &store,
            team_id,
            vec![
                slice(
                    "PR1",
                    "## Tasks\n- T1 [designer] Mock it up - Touches: design/**",
                ),
                slice(
                    "PR2",
                    "## Tasks\n- T1 [verifier] Build it - Touches: crates/**",
                ),
                slice("PR3", "## Tasks\n- T1 Build it - Touches: crates/**"),
            ],
        )
        .unwrap();

        assert!(routed.is_empty());
        let reasons: Vec<&str> = unrouted.iter().map(|(_, reason)| reason.as_str()).collect();
        assert!(reasons[0].contains("no such seat"), "{reasons:?}");
        assert!(reasons[1].contains("cannot edit"), "{reasons:?}");
        assert!(reasons[2].contains("could not be read"), "{reasons:?}");
    }
}
