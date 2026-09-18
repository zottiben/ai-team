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
use crate::supervise::supervisor::{outcome_status, run_turn, Supervisor, TurnOutcome};
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
        F: FnMut(&crate::eve::StreamEvent),
    {
        let orchestrator = store
            .agents(self.team_id)?
            .into_iter()
            .find(|agent| agent.role == ROOT_ROLE)
            .ok_or_else(|| Error::invalid("the team has no orchestrator"))?;

        let node = store.dispatch(self.run_id, orchestrator.id, None, &self.registry)?;
        store.attach_worktree(node.id, &self.repo.to_string_lossy(), None, None)?;
        store.set_node_status(node.id, NodeStatus::Running)?;

        let env = self.env(&self.repo)?;
        let mut supervisor = Supervisor::new(&self.project_dir, env);
        let client = supervisor.start().await?;

        let outcome = match run_turn(store, node.id, &client, prompt, on_event).await {
            Ok((_, outcome)) => outcome,
            Err(error) => {
                // Stop the process before surfacing the failure: `?` here would leave a
                // Node server holding a port for the rest of the run.
                let _ = supervisor.stop().await;
                store.set_node_status(node.id, NodeStatus::Failed)?;
                return Err(error);
            }
        };
        supervisor.stop().await?;
        store.set_node_status(node.id, outcome_status(&outcome))?;
        Ok(outcome)
    }

    /// The slices that are ready, nobody holds, and some seat owns.
    ///
    /// Unroutable slices are reported rather than guessed at: dispatching a slice to a
    /// seat that does not own its paths is how two agents end up in one file.
    pub async fn routable(&self, store: &Store) -> Result<Routing> {
        route(store, self.team_id, self.planner.slices().await?)
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
        let (routed, unrouted) = self.routable(store).await?;
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

        // Claim through ai-planner, in the leased worktree, so the board shows where the
        // work is actually happening and a second run is told the slice is taken.
        if !self.planner.claim(&slice.key, &worktree).await? {
            return Err(Error::invalid(format!(
                "{} is claimed by another worktree",
                slice.key
            )));
        }

        let node = store.dispatch(self.run_id, agent_id, Some(&slice.key), &self.registry)?;
        store.attach_worktree(node.id, &worktree.to_string_lossy(), None, None)?;
        store.set_node_status(node.id, NodeStatus::Running)?;

        Ok(NodeTask {
            db_path: self.db_path.clone(),
            project_dir: self.project_dir.clone(),
            env: self.env(&worktree)?,
            node_run_id: node.id,
            slice_key: slice.key.clone(),
            title: slice.title.clone(),
            role: node.role,
            prompt: slice_prompt(slice),
            worktree,
            lease,
            planner: self.planner.clone(),
        })
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
    project_dir: PathBuf,
    env: EveEnv,
    node_run_id: i64,
    slice_key: String,
    title: String,
    role: String,
    prompt: String,
    worktree: PathBuf,
    lease: crate::neighbours::Lease,
    planner: Planner,
}

/// Drive one slice to completion in its own worktree, on its own connection.
///
/// Its own `Store`: SQLite is in WAL with a busy timeout, so parallel nodes write
/// concurrently, and sharing one `&mut Store` across tasks would serialise them anyway.
async fn run_one(task: NodeTask) -> Result<Dispatched> {
    let NodeTask {
        db_path,
        project_dir,
        env,
        node_run_id,
        slice_key,
        title,
        role,
        prompt,
        worktree,
        lease,
        planner,
    } = task;

    let mut store = Store::open(&db_path)?;
    let mut supervisor = Supervisor::new(&project_dir, env);

    let result = async {
        let client = supervisor.start().await?;
        let (_, outcome) = run_turn(&mut store, node_run_id, &client, &prompt, |_| {}).await?;
        Ok::<_, Error>(outcome)
    }
    .await;

    // Stop the process before anything else can fail: the lease is returned on drop, but
    // an eve server holding a port is not.
    let _ = supervisor.stop().await;

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            store.set_node_status(node_run_id, NodeStatus::Failed)?;
            // Hand the slice back so the next run can pick it up, and say why on the
            // board rather than only in ai-team's own event log.
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

    let mut status = outcome_status(&outcome);

    // Commit before the lease goes back. `awt return` cleans and resets the worktree, so
    // this is the only thing standing between the work and losing it - and a branch in
    // the shared object store is what a human reviews afterwards.
    let mut branch = None;
    if status == NodeStatus::Done {
        let name = format!("ai-team/{}", slice_key.to_lowercase());
        match git::commit_all(&worktree, &name, &format!("{slice_key}: {}", title.trim())).await {
            Ok(Some(sha)) => {
                branch = Some(name.clone());
                store.attach_worktree(
                    node_run_id,
                    &worktree.to_string_lossy(),
                    Some(&name),
                    None,
                )?;
                let _ = planner.set_branch(&slice_key, &name).await;
                let _ = planner
                    .log(
                        &format!("ai-team built {slice_key} on {name} ({})", &sha[..12]),
                        Some(&slice_key),
                    )
                    .await;
            }
            Ok(None) => {
                // A turn that reports done but changed no file did not build the slice.
                // Calling that success puts a green row on the board for work nobody did.
                status = NodeStatus::Failed;
                store.block_node(node_run_id, "the turn finished without changing a file")?;
            }
            Err(error) => {
                status = NodeStatus::Failed;
                store.block_node(node_run_id, &format!("could not commit its work: {error}"))?;
            }
        }
    }
    store.set_node_status(node_run_id, status)?;

    // The board follows what actually happened. A parked node is still somebody's, so
    // its claim stays; a finished one goes to review for a human to look at.
    match status {
        NodeStatus::Done => {
            let _ = planner
                .set_status(&slice_key, "in_review", Some("built by ai-team"))
                .await;
            let _ = planner.release(&slice_key, &worktree).await;
        }
        NodeStatus::Parked => {}
        _ => {
            let _ = planner.release(&slice_key, &worktree).await;
        }
    }

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

/// What a seat is actually asked to do with a slice.
fn slice_prompt(slice: &Slice) -> String {
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
    fn the_prompt_carries_the_scope_and_the_demo() {
        let prompt = slice_prompt(&slice("PR1", "Add the export.\n\nTouches: crates/**"));
        assert!(prompt.contains("PR1 - a slice"));
        assert!(prompt.contains("Add the export."));
        assert!(prompt.contains("it runs"), "the demo is what done means");
        assert!(prompt.contains("leased for this slice alone"));
    }
}
