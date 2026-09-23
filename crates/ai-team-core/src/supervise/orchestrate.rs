//! One prompt, into a plan, into several nodes building slices in parallel.
//!
//! The split is D12's: the orchestrator *node* grounds the request and coordinates the
//! graph, the planner node alone shapes the ai-planner plan, and this module - Rust -
//! reads approved slices back, leases a worktree per slice, routes each to the seat whose
//! zone owns its paths, and starts one Pi process per lease (D10). Scheduling, budgets and
//! routing are the control plane's job; an agent spawning sibling agents would be the
//! same work with no guardrails around it.
//!
//! Nothing here stores a plan or a slice. `node_run.slice_key` is a reference into
//! ai-planner and that is the whole of the coupling (D4).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::model::{EventKind, NewEvent, NodeStatus};
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
        node_run_id: i64,
        worktree: &Path,
        prompt: &str,
    ) -> Result<crate::PiTurn> {
        let agent = store.agent(agent_id)?;
        let node = store.node_run(node_run_id)?;
        let team = store.team(agent.team_id)?;
        let roster = store.agents(agent.team_id)?;
        // Only the seats that may shape the plan get the tools that shape it. A maker
        // that can add slices can give itself work.
        let may_plan = agent.role == "planner";
        crate::PiSeat {
            agent: &agent,
            provider: node.provider,
            model: &node.model,
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
         browser or generic web search instead.\n\nOperator request:\n\n{prompt}"
    )
}

fn planner_prompt(prompt: &str, brief: &str, trunk: Option<&str>) -> String {
    // ai-planner bases a new plan on whatever branch the checkout is on, and this one is on
    // the run's own branch - somewhere to coordinate, not something a pull request can
    // target. Said up front because every slice copies the plan's base as it is added.
    let base = trunk.map_or_else(String::new, |trunk| {
        format!(
            " Pass `base_branch: \"{trunk}\"` to `create_plan`: pull requests target {trunk}, \
             not the branch this checkout is on."
        )
    });
    format!(
        "Create a new ai-planner plan specifically for this run with `create_plan`.{base} \
         Do not append this work to an unrelated existing plan. Build the slices from the \
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
            sources: self.registry.context_sources(),
            registry: self.registry.clone(),
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
        let (coordinator, coordinator_outcome) = match take_fresh_turn(
            store,
            &self.rig(),
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
        let trunk = git::trunk(&self.repo).await.ok();
        let planner_prompt = planner_prompt(
            prompt,
            brief,
            trunk.as_ref().map(|trunk| trunk.name.as_str()),
        );
        let mut context_failure = None;
        let mut created = None;
        let (planning, planner_outcome) = match take_fresh_turn(
            store,
            &self.rig(),
            self.run_id,
            planner_id,
            None,
            &self.repo,
            &planner_prompt,
            |event| {
                if let Some(failure) = event.context_tool_failure() {
                    context_failure = Some(failure);
                }
                if let Some(slug) = event.created_plan() {
                    created = Some(slug);
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
        if outcome_status(&planner_outcome) == NodeStatus::Done {
            self.adopt_plan(store, created, trunk.as_ref()).await?;
        }
        Ok(planner_outcome)
    }

    /// Make the plan the planner just wrote this run's plan, and this checkout's.
    ///
    /// Named from `create_plan`'s own answer, never from `aip current`: ai-planner
    /// resolves a checkout to the plan it has resolved to most often, so a checkout that
    /// planned before keeps answering with the old plan and the run would build that.
    async fn adopt_plan(
        &self,
        store: &mut Store,
        created: Option<String>,
        trunk: Option<&git::Trunk>,
    ) -> Result<()> {
        let slug = if let Some(slug) = created {
            slug
        } else {
            let current = self.planner.current().await?.plan;
            store.append_event(
                self.run_id,
                NewEvent::new(
                    EventKind::Note,
                    format!(
                        "the planner did not create a plan, so this run builds {current}, \
                         the plan this checkout resolves to"
                    ),
                )
                .by("orchestrator"),
            )?;
            current
        };
        crate::workspace::adopt_plan(&self.planner, &self.repo, &slug, trunk).await?;
        store.set_run_plan(self.run_id, &slug)?;
        Ok(())
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
                match self.prepare(store, &slice, agent_id, &role).await {
                    Ok(node) => tasks.push(tokio::spawn(run_one(node))),
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
    async fn prepare(
        &self,
        store: &mut Store,
        slice: &Slice,
        agent_id: i64,
        role: &str,
    ) -> Result<NodeTask> {
        // Resolve this before claiming the slice. An error after the external claim would
        // otherwise leave work assigned to a lease that is immediately returned.
        let verifier = self.verifier(store)?;
        let branch = slice_branch(slice);
        // Before a lease is taken or a model started: `checkout -B main` in a lease would
        // reset the trunk to whatever the slice was based on.
        if let Ok(trunk) = git::trunk(&self.repo).await {
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
        let holder = lease_holder(self.run_id, &slice.key, role);
        let lease = self.worktrees.lease(&holder).await?;
        let worktree = lease.path().to_path_buf();
        let prepared = async {
            // Name the checkout before the agent starts and keep gate output untracked.
            // The plan's base is binding for stacked work; a lease's incidental HEAD is not.
            git::ignore_build_output(&worktree).await?;
            git::prepare_branch_from(&worktree, &branch, slice.base_branch.as_deref()).await?;
            crate::gates::prepare_dependencies(&worktree).await
        }
        .await;
        let setup = match prepared {
            Ok(setup) => setup,
            Err(error) => {
                let reason = format!("dependency/worktree preparation failed: {error}");
                let _ = self
                    .planner
                    .set_status(&slice.key, "blocked", Some(&reason))
                    .await;
                let _ = lease.release().await;
                return Err(Error::invalid(reason));
            }
        };
        if !setup.is_empty() {
            store.append_event(
                self.run_id,
                NewEvent::new(
                    EventKind::Note,
                    format!(
                        "{} prepared dependencies with {}",
                        slice.key,
                        setup.join(", ")
                    ),
                )
                .by("orchestrator"),
            )?;
        }

        // Claim through ai-planner, in the leased worktree, so the board shows where the
        // work is actually happening and a second run is told the slice is taken.
        if !self.planner.claim(&slice.key, &worktree).await? {
            let _ = lease.release().await;
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
            branch,
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
            verifier,
        })
    }

    /// Reattach one interrupted maker to its exact lease and Pi session.
    ///
    /// No branch is prepared and no dependencies are reinstalled here: both could
    /// overwrite evidence left by the interrupted process. The persisted node, planner
    /// claim and awt lease must all agree before a model is started.
    pub async fn resume_node(&self, store: &mut Store, node_id: i64) -> Result<Dispatched> {
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
        if node.supervisor_pid != Some(i64::from(std::process::id())) {
            return Err(Error::invalid(
                "this process has not claimed the interrupted turn",
            ));
        }
        let agent_id = node
            .agent_id
            .ok_or_else(|| Error::invalid("the interrupted node's seat no longer exists"))?;
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
        let branch = node.branch.clone().unwrap_or_else(|| slice_branch(&slice));
        let holder = lease_holder(self.run_id, &slice_key, &node.role);
        let lease = self.worktrees.resume(&worktree, &holder).await?;
        let verifier = self.verifier(store)?;
        let prompt = format!(
            "Resume {slice_key} after the local ai-team supervisor was interrupted. Keep \
             the work already present in this checkout, read the person's queued replies, \
             and complete the same task. Do not restart or discard the existing diff.\n\n{}",
            slice_prompt(&slice, &crate::house::read_for(&worktree, &slice.touches()))
        );
        store.append_event(
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
        )?;

        resume_one(ResumeTask {
            db_path: self.db_path.clone(),
            rig: self.rig(),
            agent_id,
            registry: self.registry.clone(),
            node,
            slice_key,
            title: slice.title,
            branch,
            prompt,
            worktree,
            lease,
            planner: self.planner.clone(),
            run_id: self.run_id,
            max_repairs: store.run(self.run_id)?.max_repairs,
            verifier,
        })
        .await
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

/// Everything one parallel node needs, owned outright so it can move to its own task.
struct NodeTask {
    db_path: PathBuf,
    rig: Rig,
    agent_id: i64,
    registry: ModelRegistry,
    slice_key: String,
    title: String,
    branch: String,
    prompt: String,
    worktree: PathBuf,
    lease: crate::neighbours::Lease,
    planner: Planner,
    run_id: i64,
    max_repairs: i64,
    verifier: Option<VerifierSeat>,
}

struct ResumeTask {
    db_path: PathBuf,
    rig: Rig,
    agent_id: i64,
    registry: ModelRegistry,
    node: crate::model::NodeRun,
    slice_key: String,
    title: String,
    branch: String,
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
        branch,
        prompt,
        worktree,
        lease,
        planner,
        run_id,
        max_repairs,
        verifier,
    } = task;

    let mut store = Store::open(&db_path)?;

    let attempted = match attempt_until_accepted(AttemptArgs {
        store: &mut store,
        rig: &rig,
        agent_id,
        registry: &registry,
        run_id,
        slice_key: &slice_key,
        branch: &branch,
        worktree: &worktree,
        max_repairs,
        verifier: verifier.as_ref(),
        first: prompt,
    })
    .await
    {
        Ok(attempted) => attempted,
        Err(error) => {
            return Err(
                release_failed_attempt(&planner, &slice_key, &worktree, lease, error).await,
            );
        }
    };
    finish_attempt(
        &mut store,
        Completion {
            db_path,
            planner,
            slice_key,
            title,
            branch,
            worktree,
            lease,
            run_id,
        },
        attempted,
    )
    .await
}

async fn resume_one(task: ResumeTask) -> Result<Dispatched> {
    let ResumeTask {
        db_path,
        rig,
        agent_id,
        registry,
        node,
        slice_key,
        title,
        branch,
        prompt,
        worktree,
        lease,
        planner,
        run_id,
        max_repairs,
        verifier,
    } = task;
    let mut store = Store::open(&db_path)?;
    let attempted = match resume_attempt_until_accepted(
        AttemptArgs {
            store: &mut store,
            rig: &rig,
            agent_id,
            registry: &registry,
            run_id,
            slice_key: &slice_key,
            branch: &branch,
            worktree: &worktree,
            max_repairs,
            verifier: verifier.as_ref(),
            first: prompt,
        },
        node.clone(),
    )
    .await
    {
        Ok(attempted) => attempted,
        Err(error) => {
            // Recovery must be retryable too. Keep both the Pi session address and the
            // dirty lease; returning it here would make awt clean away partial work.
            let _ = store.release_node_supervision(node.id, i64::from(std::process::id()));
            lease.preserve();
            return Err(error);
        }
    };
    finish_attempt(
        &mut store,
        Completion {
            db_path,
            planner,
            slice_key,
            title,
            branch,
            worktree,
            lease,
            run_id,
        },
        attempted,
    )
    .await
}

struct Completion {
    db_path: PathBuf,
    planner: Planner,
    slice_key: String,
    title: String,
    branch: String,
    worktree: PathBuf,
    lease: crate::neighbours::Lease,
    run_id: i64,
}

async fn finish_attempt(
    store: &mut Store,
    completion: Completion,
    attempted: Attempted,
) -> Result<Dispatched> {
    let Completion {
        db_path,
        planner,
        slice_key,
        title,
        branch,
        worktree,
        lease,
        run_id,
    } = completion;
    let Attempted {
        outcome,
        rejection,
        attempt,
        node_run_id,
        role,
        exhausted,
        changed,
    } = attempted;
    let (mut status, fallout) = settle_status(
        store,
        run_id,
        node_run_id,
        &role,
        &slice_key,
        &outcome,
        rejection.as_deref(),
        attempt,
        exhausted,
    )?;
    // Close the reply window before committing and returning the lease. Leaving the row
    // `running` during landing lets the UI accept a message after the final queue check,
    // when no supervisor remains to deliver it.
    store.set_node_status(node_run_id, status)?;

    let landed_branch = land(
        store,
        &planner,
        Landing {
            worktree: &worktree,
            node_run_id,
            slice_key: &slice_key,
            title: &title,
            branch: &branch,
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
    // Delivery and notification are deliberately best-effort. The node row and event
    // stream are the truth; neither remote system may turn accepted local work into
    // failed work.
    let _ = notify_node(store, node_run_id, &slice_key, &role, status);

    let dispatched = Dispatched {
        slice_key,
        role,
        worktree,
        node_run_id,
        status,
        outcome,
        branch: landed_branch,
    };
    release_and_deliver(lease, &db_path, &dispatched).await?;
    Ok(dispatched)
}

async fn release_failed_attempt(
    planner: &Planner,
    slice_key: &str,
    worktree: &Path,
    lease: crate::neighbours::Lease,
    error: Error,
) -> Error {
    let _ = planner.release(slice_key, worktree).await;
    let _ = planner
        .log(
            &format!("ai-team: {slice_key} failed - {error}"),
            Some(slice_key),
        )
        .await;
    let _ = lease.release().await;
    error
}

async fn release_and_deliver(
    lease: crate::neighbours::Lease,
    db_path: &Path,
    dispatched: &Dispatched,
) -> Result<()> {
    // Returned explicitly so a failure to return is reported rather than swallowed by
    // the Drop fallback. Remote delivery uses the surviving branch and never needs to
    // hold the scarce maker lease while GitHub checks run.
    lease.release().await?;
    if dispatched.status == NodeStatus::Done && dispatched.branch.is_some() {
        crate::delivery::automatic_delivery(db_path, dispatched.node_run_id).await;
    }
    Ok(())
}

fn notify_node(
    store: &mut Store,
    node_run_id: i64,
    slice_key: &str,
    role: &str,
    status: NodeStatus,
) -> Result<()> {
    let node = store.node_run(node_run_id)?;
    let run = store.run(node.run_id)?;
    let (kind, title, body) = match status {
        NodeStatus::Done => (
            "completed",
            format!("{role} finished"),
            format!("{slice_key} is built and ready to review."),
        ),
        NodeStatus::Parked => (
            "input_required",
            format!("{role} needs your input"),
            node.blocked_reason
                .unwrap_or_else(|| format!("{slice_key} is waiting for your reply.")),
        ),
        NodeStatus::Failed | NodeStatus::Blocked => (
            "failed",
            format!("{role} stopped"),
            node.blocked_reason
                .unwrap_or_else(|| format!("{slice_key} did not finish.")),
        ),
        NodeStatus::Cancelled => (
            "follow_up",
            format!("{role} was cancelled"),
            format!("{slice_key} was cancelled before it finished."),
        ),
        NodeStatus::Queued | NodeStatus::Running => return Ok(()),
    };
    let project = store.project(run.project_id)?;
    store.notify_once(crate::model::NewNotification {
        dedupe_key: format!("node:{node_run_id}:{}", status.as_str()),
        project_id: run.project_id,
        workspace_path: run.workspace_path,
        run_id: Some(run.id),
        node_run_id: Some(node_run_id),
        kind: kind.into(),
        title: format!("{} · {title}", project.name),
        body,
        action_path: None,
    })?;
    Ok(())
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

fn lease_holder(run_id: i64, slice_key: &str, role: &str) -> String {
    format!("ai-team run-{run_id} {slice_key} {role}")
}

fn slice_branch(slice: &Slice) -> String {
    slice
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map_or_else(
            || format!("ai-team/{}", slice.key.to_lowercase()),
            str::to_string,
        )
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
        branch,
        changed,
    } = work;
    if *status != NodeStatus::Done {
        return Ok(None);
    }
    match git::commit_paths(
        worktree,
        branch,
        &format!("{slice_key}: {}", title.trim()),
        changed,
    )
    .await
    {
        Ok(Some(sha)) => {
            store.attach_worktree(node_run_id, &worktree.to_string_lossy(), Some(branch), None)?;

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
                Some(branch),
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

            let _ = planner.set_branch(slice_key, branch).await;
            let _ = planner
                .log(
                    &format!("ai-team built {slice_key} on {branch} ({})", &sha[..12]),
                    Some(slice_key),
                )
                .await;
            Ok(Some(branch.to_string()))
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

    /// A provider/runtime turn failed before there was any work to verify. Preserve its
    /// diagnostic, but do not spend a repair on gates for an implementation that never ran.
    fn failed(
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
            exhausted: false,
            changed,
        }
    }

    /// Stopped because every eligible subscription account was unavailable. This is a
    /// terminal rejection, but not a repair-budget exhaustion.
    fn unavailable(
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
            exhausted: false,
            changed,
        }
    }

    fn quota_unavailable(
        outcome: TurnOutcome,
        node: &crate::model::NodeRun,
        attempt: i64,
        changed: Vec<String>,
        slice_key: &str,
        quota: &str,
    ) -> Attempted {
        Attempted::unavailable(
            outcome,
            node,
            attempt,
            changed,
            format!(
                "{slice_key} cannot continue: every reachable allowed model subscription is unavailable; last response: {quota}"
            ),
        )
    }

    fn refused(
        node: &crate::model::NodeRun,
        attempt: i64,
        changed: Vec<String>,
        reason: String,
    ) -> Attempted {
        Attempted::stopped(TurnOutcome::default(), node, attempt, changed, reason)
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
    branch: &'a str,
    worktree: &'a Path,
    max_repairs: i64,
    verifier: Option<&'a VerifierSeat>,
    first: String,
}

struct AttemptStart<'a> {
    rig: &'a Rig,
    agent_id: i64,
    registry: &'a ModelRegistry,
    run_id: i64,
    slice_key: &'a str,
    branch: &'a str,
    worktree: &'a Path,
    resolution: Option<&'a crate::ModelResolution>,
    instruction: &'a str,
}

enum StartedAttempt {
    Turn(crate::model::NodeRun, TurnOutcome),
    Refused(crate::model::NodeRun, String),
}

async fn start_attempt(store: &mut Store, start: AttemptStart<'_>) -> Result<StartedAttempt> {
    let node = open_attempt(store, &start)?;
    if let Some(exceeded) = crate::guardrails::run_may_continue(store, start.run_id)? {
        return Ok(StartedAttempt::Refused(node, exceeded.reason));
    }
    store.set_node_status(node.id, NodeStatus::Running)?;
    match take_turn(
        store,
        start.rig,
        start.agent_id,
        node.id,
        start.worktree,
        start.instruction,
    )
    .await
    {
        Ok(turn) => Ok(StartedAttempt::Turn(node, turn)),
        Err(error) => {
            store.set_node_status(node.id, NodeStatus::Failed)?;
            Err(error)
        }
    }
}

struct AttemptReview<'a> {
    rig: &'a Rig,
    agent_id: i64,
    node: &'a crate::model::NodeRun,
    worktree: &'a Path,
    verifier: Option<&'a VerifierSeat>,
    attempt: i64,
    turn: TurnOutcome,
    changed: Vec<String>,
}

enum ReviewedAttempt {
    Finished(Box<Attempted>),
    Rejected(Box<(TurnOutcome, Vec<String>, String)>),
}

async fn review_attempt(store: &mut Store, review: AttemptReview<'_>) -> Result<ReviewedAttempt> {
    let AttemptReview {
        rig,
        agent_id,
        node,
        worktree,
        verifier,
        attempt,
        mut turn,
        mut changed,
    } = review;
    loop {
        // Captured before `check` runs the gates: running them writes build output into
        // the worktree, and that is ai-team's mess rather than the agent's work.
        for path in crate::neighbours::git::changed_paths(worktree).await? {
            if !changed.contains(&path) {
                changed.push(path);
            }
        }
        if outcome_status(&turn) != NodeStatus::Done {
            let reason = crate::supervise::outcome::provider_diagnostic(&turn)
                .unwrap_or_else(|| "the model turn failed before completing".to_string());
            return Ok(ReviewedAttempt::Finished(Box::new(Attempted::failed(
                turn, node, attempt, changed, reason,
            ))));
        }
        if store.waiting_for_node(agent_id, node.id)? > 0 {
            turn = take_replied_turn(store, rig, agent_id, node.id, worktree).await?;
            continue;
        }

        let verdict = check(store, node.id, worktree, rig, verifier).await?;
        // Checks can take minutes. A reply written during them still belongs in this
        // conversation and must be applied before accepting and returning the lease.
        if store.waiting_for_node(agent_id, node.id)? > 0 {
            turn = take_replied_turn(store, rig, agent_id, node.id, worktree).await?;
            continue;
        }
        return Ok(match verdict {
            Verdict::Accepted => ReviewedAttempt::Finished(Box::new(Attempted::settled(
                turn, node, attempt, changed,
            ))),
            Verdict::Unavailable(reason) => ReviewedAttempt::Finished(Box::new(
                Attempted::unavailable(turn, node, attempt, changed, reason),
            )),
            Verdict::Rejected(reason) => {
                ReviewedAttempt::Rejected(Box::new((turn, changed, reason)))
            }
        });
    }
}

struct RejectedAttempt<'a> {
    run_id: i64,
    slice_key: &'a str,
    node: &'a crate::model::NodeRun,
    attempt: i64,
    max_repairs: i64,
    turn: TurnOutcome,
    changed: Vec<String>,
    reason: String,
}

enum RepairDecision {
    Stop(Box<Attempted>),
    Continue(String, Vec<String>),
}

fn after_rejection(store: &mut Store, rejected: RejectedAttempt<'_>) -> Result<RepairDecision> {
    let RejectedAttempt {
        run_id,
        slice_key,
        node,
        attempt,
        max_repairs,
        turn,
        changed,
        reason,
    } = rejected;
    record_rejection(
        store, run_id, node.id, slice_key, &node.role, attempt, &reason,
    )?;
    if let Some(exceeded) = crate::guardrails::node_may_continue(store, node.id)? {
        return Ok(RepairDecision::Stop(Box::new(Attempted::stopped(
            turn,
            node,
            attempt,
            changed,
            format!("{}; last rejection: {reason}", exceeded.reason),
        ))));
    }
    if attempt >= max_repairs {
        return Ok(RepairDecision::Stop(Box::new(Attempted::stopped(
            turn, node, attempt, changed, reason,
        ))));
    }
    Ok(RepairDecision::Continue(reason, changed))
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
    drive_attempts(args, None).await
}

/// Continue an interrupted maker on its existing node row, session and checkout before
/// rejoining the ordinary check/repair loop. The interrupted attempt is evidence, not a
/// failed repair, so it keeps its original attempt number.
async fn resume_attempt_until_accepted(
    args: AttemptArgs<'_>,
    node: crate::model::NodeRun,
) -> Result<Attempted> {
    let turn =
        take_replied_turn(args.store, args.rig, args.agent_id, node.id, args.worktree).await?;
    drive_attempts(args, Some((node, turn))).await
}

async fn drive_attempts(
    args: AttemptArgs<'_>,
    mut resumed: Option<(crate::model::NodeRun, TurnOutcome)>,
) -> Result<Attempted> {
    let AttemptArgs {
        store,
        rig,
        agent_id,
        registry,
        run_id,
        slice_key,
        branch,
        worktree,
        max_repairs,
        verifier,
        first,
    } = args;

    let mut attempt = resumed
        .as_ref()
        .map_or(0, |(node, _)| node.attempt.saturating_sub(1));
    let mut instruction = first;
    let (mut changed, mut exhausted_providers) = (Vec::new(), Vec::new());
    let mut runtime_resolution: Option<crate::ModelResolution> = None;

    loop {
        let (node, turn) = if let Some(resumed) = resumed.take() {
            resumed
        } else {
            match start_attempt(
                store,
                AttemptStart {
                    rig,
                    agent_id,
                    registry,
                    run_id,
                    slice_key,
                    branch,
                    worktree,
                    resolution: runtime_resolution.as_ref(),
                    instruction: &instruction,
                },
            )
            .await?
            {
                StartedAttempt::Turn(node, turn) => (node, turn),
                StartedAttempt::Refused(node, reason) => {
                    return Ok(Attempted::refused(&node, attempt, changed, reason));
                }
            }
        };
        if let Some(quota) = crate::supervise::quota_exhaustion(&turn) {
            runtime_resolution = record_quota_and_resolve(
                store,
                &mut exhausted_providers,
                QuotaFailover::new(registry, run_id, agent_id, slice_key, &node, &quota),
            )?;
            if runtime_resolution.is_some() {
                // Repeat the instruction without spending a repair or running gates.
                continue;
            }
            return Ok(Attempted::quota_unavailable(
                turn, &node, attempt, changed, slice_key, &quota,
            ));
        }

        // Keep this node and session alive through replies and checks. A background turn
        // after this function returned would race `awt return` cleaning its checkout.
        let (turn, next_changed, reason) = match review_attempt(
            store,
            AttemptReview {
                rig,
                agent_id,
                node: &node,
                worktree,
                verifier,
                attempt,
                turn,
                changed,
            },
        )
        .await?
        {
            ReviewedAttempt::Finished(finished) => return Ok(*finished),
            ReviewedAttempt::Rejected(rejected) => *rejected,
        };

        match after_rejection(
            store,
            RejectedAttempt {
                run_id,
                slice_key,
                node: &node,
                attempt,
                max_repairs,
                turn,
                changed: next_changed,
                reason,
            },
        )? {
            RepairDecision::Stop(stopped) => return Ok(*stopped),
            RepairDecision::Continue(reason, next_changed) => {
                attempt += 1;
                instruction = repair_prompt(slice_key, &reason);
                changed = next_changed;
            }
        }
    }
}

struct QuotaFailover<'a> {
    registry: &'a ModelRegistry,
    run_id: i64,
    agent_id: i64,
    subject: &'a str,
    node: &'a crate::model::NodeRun,
    quota: &'a str,
}

impl<'a> QuotaFailover<'a> {
    fn new(
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

fn record_quota_and_resolve(
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

enum FreshTurn {
    Finished(Box<(crate::model::NodeRun, TurnOutcome)>),
    Unavailable { reason: String },
}

/// Start a fresh node and move it across subscription accounts only when Pi reports a
/// recognized quota boundary. Generic model/network/tool failures return normally and
/// retain their ordinary failure semantics.
#[allow(clippy::too_many_arguments)]
async fn take_fresh_turn<F>(
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
                store.dispatch_with_resolution(run_id, agent_id, slice_key, resolution)?
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
            store.set_node_status(node.id, outcome_status(&outcome))?;
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

async fn take_replied_turn(
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
async fn take_turn(
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

/// The work one node produced, and where it lives.
struct Landing<'a> {
    worktree: &'a Path,
    node_run_id: i64,
    slice_key: &'a str,
    title: &'a str,
    branch: &'a str,
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
fn open_attempt(store: &mut Store, start: &AttemptStart<'_>) -> Result<crate::model::NodeRun> {
    let node = match start.resolution {
        Some(resolution) => store.dispatch_with_resolution(
            start.run_id,
            start.agent_id,
            Some(start.slice_key),
            resolution,
        )?,
        None => store.dispatch(
            start.run_id,
            start.agent_id,
            Some(start.slice_key),
            start.registry,
        )?,
    };
    store.attach_worktree(
        node.id,
        &start.worktree.to_string_lossy(),
        Some(start.branch),
        None,
    )?;
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
}

/// The answer to "is this actually done?".
enum Verdict {
    Accepted,
    Rejected(String),
    Unavailable(String),
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

    // Captured off the stream rather than read back out of the event table. A node's
    // Note events also carry ai-team's own bookkeeping - the model-fallback notice, for
    // one - and parsing that as the verifier's answer rejects work it never looked at.
    let mut said = String::new();
    match take_fresh_turn(
        store,
        rig,
        run_id,
        verifier.agent_id,
        None,
        worktree,
        &verify_prompt(&evidence),
        |event| {
            if let Some(message) = event.assistant_message() {
                said = message;
            }
        },
    )
    .await
    {
        Ok(FreshTurn::Finished(_)) => Ok(read_verdict(&said)),
        Ok(FreshTurn::Unavailable { reason }) => Ok(Verdict::Unavailable(format!(
            "the verifier could not run because {reason}"
        ))),
        Err(error) => {
            // A verifier that could not run has not approved anything.
            Ok(Verdict::Rejected(format!(
                "the verifier could not run: {error}"
            )))
        }
    }
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
        assert_eq!(slice_branch(&planned), "feature/stack-one");

        planned.branch = None;
        assert_eq!(slice_branch(&planned), "ai-team/s1");
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

    #[tokio::test]
    async fn a_provider_error_skips_gates_verifier_and_repairs() {
        let (mut store, team_id) = team();
        let project_id = store.team(team_id).unwrap().project_id.unwrap();
        let run = store
            .create_run(project_id, "build S1", crate::RunTrigger::Manual)
            .unwrap();
        let agents = store.agents(team_id).unwrap();
        let maker = agents
            .iter()
            .find(|agent| agent.role == "frontend")
            .unwrap();
        let verifier = agents
            .iter()
            .find(|agent| agent.role == crate::VERIFIER_ROLE)
            .unwrap();
        let registry = ModelRegistry::local_only();
        let node = store
            .dispatch(run.id, maker.id, Some("S1"), &registry)
            .unwrap();

        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "ai-team test"]);
        std::fs::write(
            repo.path().join("package.json"),
            r#"{"scripts":{"test":"touch gate-ran"}}"#,
        )
        .unwrap();
        git(&["add", "package.json"]);
        git(&["commit", "-qm", "fixture"]);

        let rig = Rig {
            support: repo.path().into(),
            plan_root: repo.path().into(),
            sources: Vec::new(),
            registry,
        };
        let reviewed = review_attempt(
            &mut store,
            AttemptReview {
                rig: &rig,
                agent_id: maker.id,
                node: &node,
                worktree: repo.path(),
                verifier: Some(&VerifierSeat {
                    agent_id: verifier.id,
                }),
                attempt: 0,
                turn: TurnOutcome {
                    provider_message: Some(
                        "Not logged in · Please run /login\nNot logged in · Please run /login"
                            .into(),
                    ),
                    terminal: Some(crate::TerminalState::Failed),
                    ..TurnOutcome::default()
                },
                changed: Vec::new(),
            },
        )
        .await
        .unwrap();

        let ReviewedAttempt::Finished(finished) = reviewed else {
            panic!("a provider error must finish rather than enter the repair path");
        };
        assert_eq!(finished.attempt, 0);
        assert_eq!(
            finished.rejection.as_deref(),
            Some(
                "The selected provider is not signed in. Open Settings and sign in before retrying this slice."
            )
        );
        assert!(!repo.path().join("gate-ran").exists(), "a gate ran");
        assert_eq!(
            store.node_runs(run.id).unwrap().len(),
            1,
            "a verifier or repair node was dispatched"
        );
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
