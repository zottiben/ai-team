//! Saying something to one seat.
//!
//! Two quite different acts, and the whole design is about not letting them look like one.
//!
//! A seat **mid-turn** is a process reading one prompt, so a message has nowhere to land
//! inside what it is doing. It is kept and given to that seat next, on the same session,
//! so it arrives with the conversation behind it (D21) - the same machinery a submitted
//! review uses to steer the node that wrote the code (M3-S15). That is not immediate, and
//! nothing is lost.
//!
//! A seat that is **idle** has no turn to wait for. Reaching it means leasing a worktree
//! and driving one - which is `ait run --worktree` for one agent rather than for the
//! orchestrator. That takes minutes and it is *starting work*, not a remark.
//!
//! A single box that silently did either would be a surprise waiting to happen, so which
//! one is about to happen is answerable before anybody presses send.

use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::{Agent, NodeStatus, RunStatus, RunTrigger};
use crate::store::Store;

/// What saying something to this seat would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Would {
    /// Wait for the turn already going, then be the next thing that seat is given.
    ///
    /// eve could land a message inside a running turn, because there was an HTTP session
    /// to post to. A Pi turn is a child process reading one prompt, so there is nowhere
    /// for a message to land mid-turn - and pretending otherwise would be a box that
    /// swallows what somebody typed. It is kept and delivered next, on the same session,
    /// so it arrives with the conversation behind it.
    Queue,
    /// Start a direct maker turn. Minutes, and a worktree.
    StartWork,
    /// Start the full team workflow through the orchestrator control plane.
    Coordinate,
    /// Continue the exact run whose plan is waiting for this person's approval.
    ApprovePlan,
    /// Nothing: the seat is switched off, so it would never be given the message.
    Nothing,
}

/// Everything needed to reach a seat, read in one synchronous window.
///
/// A `rusqlite::Connection` is `Send` but not `Sync`, so the awaits below cannot hold one -
/// the caller reads this, drops its lock, and then talks.
#[derive(Debug, Clone)]
pub struct Target {
    pub agent: Agent,
    pub project_id: i64,
    pub project_slug: String,
    /// The repository to lease from, when a turn has to be started.
    pub repo: Option<String>,
    /// A turn in progress, when there is one: the node run and its Pi session.
    pub live: Option<(i64, String)>,
    /// The selected workspace's coherent approval-held run, for the orchestrator only.
    pub approval_run_id: Option<i64>,
}

impl Target {
    pub fn would(&self) -> Would {
        if !self.agent.enabled {
            Would::Nothing
        } else if self.live.is_some() {
            Would::Queue
        } else if self.agent.role == crate::ROOT_ROLE {
            if self.approval_run_id.is_some() {
                Would::ApprovePlan
            } else {
                Would::Coordinate
            }
        } else {
            Would::StartWork
        }
    }
}

/// Look up what it would take to reach a seat.
pub fn target(store: &Store, agent_id: i64) -> Result<Target> {
    target_in(store, agent_id, None)
}

/// Look up a seat from one workspace, so activity in a sibling checkout cannot receive
/// a message meant for this one.
pub fn target_in(store: &Store, agent_id: i64, worktree: Option<&Path>) -> Result<Target> {
    let agent = store.agent(agent_id)?;
    let team = store.team(agent.team_id)?;
    let project_id = team
        .project_id
        .ok_or_else(|| Error::invalid("that team is not attached to a project"))?;
    let project = store.project(project_id)?;

    let repo = store
        .project_repos(project_id)?
        .into_iter()
        .find_map(|repo| repo.main_path);

    // The newest node run for this role that still looks alive. Newest first, because a
    // retry is a new row and only the latest can still be running (D2).
    let mut live = None;
    let runs = match worktree {
        Some(worktree) => store.runs_in_workspace(project_id, worktree, 20)?,
        None => store.runs(Some(project_id), 20)?,
    };
    let approval_run_id = if agent.role != crate::ROOT_ROLE {
        None
    } else if let Some(worktree) = worktree {
        store
            .blocked_run_in_workspace(
                project_id,
                agent.team_id,
                worktree,
                [
                    crate::workflow::PLAN_APPROVAL_REASON,
                    crate::workflow::PLAN_APPROVAL_PREPARING_REASON,
                ],
            )?
            .map(|run| run.id)
    } else {
        store
            .blocked_run_for_project(
                project_id,
                agent.team_id,
                [
                    crate::workflow::PLAN_APPROVAL_REASON,
                    crate::workflow::PLAN_APPROVAL_PREPARING_REASON,
                ],
            )?
            .map(|run| run.id)
    };
    'outer: for run in &runs {
        let mut nodes = store.node_runs(run.id)?;
        nodes.reverse();
        for node in nodes {
            if node.role != agent.role {
                continue;
            }
            // Only a running or parked node is worth trying. A finished one still has its
            // port recorded, and offering to talk to it would be a box that swallows what
            // somebody typed (M3-S15).
            if !matches!(node.status, NodeStatus::Running | NodeStatus::Parked) {
                break 'outer;
            }
            // A session is all it takes now: there is no port to reach and no token to
            // present. A node that is running without one has not streamed its first
            // line yet, which is a turn too young to say anything to.
            if node.session_retired_at.is_none() && node.session_resetting_at.is_none() {
                if let Some(session) = node.session_id.clone() {
                    live = Some((node.id, session));
                }
            }
            break 'outer;
        }
    }

    Ok(Target {
        agent,
        project_id,
        project_slug: project.slug,
        repo,
        live,
        approval_run_id,
    })
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reached", rename_all = "snake_case")]
pub enum Reached {
    /// It was mid-turn, so the message is waiting for it.
    ///
    /// `waiting` is everything queued for that seat, not just this message: somebody who
    /// says three things to a busy agent should be told three are waiting.
    Queued { node_run_id: i64, waiting: usize },
    /// A turn was started for it.
    ///
    /// Deliberately carries no run id. The caller spawns the turn and returns before the
    /// run row exists, so any number here would be invented - and an invented id reads as
    /// a real one (M3-S17). The run appears in the window on the next tick.
    Started,
    /// The orchestrator started a new full team workflow.
    Coordinating,
    /// An existing approval-held run resumed into maker dispatch.
    Continued { run_id: i64 },
    /// Nothing was done, and why.
    Refused { because: String },
}

/// Keep a message for a seat that is mid-turn.
///
/// Synchronous, and that is the point: there is nothing to reach out to. The message goes
/// in the queue and the seat's next turn is given it, on the same session, so it arrives
/// with the conversation behind it rather than as an instruction from nowhere.
pub fn queue(store: &mut Store, target: &Target, message: &str) -> Result<Reached> {
    let Some((node_run_id, _)) = target.live.clone() else {
        return Err(Error::invalid("that seat has no turn in progress"));
    };
    let waiting = store.queue_node_message(node_run_id, target.agent.id, message)?;
    Ok(Reached::Queued {
        node_run_id,
        waiting,
    })
}

/// Start a turn for one seat, and drive it.
///
/// Long. The caller is expected to spawn this: a turn takes minutes, and an HTTP request
/// that waits for one is a request that times out (M3-S12).
pub async fn start_turn(agent_id: i64, message: String) -> Result<i64> {
    let db = crate::default_db_path()?;
    // Owned rather than borrowed: `&mut Store` is `Send` (a shared `&Store` is not, because
    // a connection is not `Sync`), so a task may hold one across awaits - which is how
    // `workflow::run` does the same job.
    let mut store = Store::open(&db)?;

    let (agent, project_id, project_slug, repo) = {
        let found = target(&store, agent_id)?;
        if !found.agent.enabled {
            return Err(Error::invalid(format!(
                "{} is switched off, so it will not be given work",
                found.agent.role
            )));
        }
        let repo = found.repo.clone().ok_or_else(|| {
            Error::invalid("that project has no checkout, so there is nowhere to work")
        })?;
        (
            found.agent.clone(),
            found.project_id,
            found.project_slug.clone(),
            repo,
        )
    };
    if agent.role == crate::ROOT_ROLE {
        return Err(Error::invalid(
            "the orchestrator coordinates team runs; it cannot start planless direct work",
        ));
    }

    // A run, so the turn is recorded like any other work rather than happening invisibly.
    // What was said is the prompt, because that is what it is.
    let run = store.create_run_in(
        project_id,
        &message,
        RunTrigger::Manual,
        Some(Path::new(&repo)),
    )?;

    // Where the guard and this seat's MCP config live. Never the lease: a guard a node can
    // edit is not a guard.
    let support = store.support_dir(&project_slug)?;

    // One Pi process per leased worktree (D10). A lease is borrowed - `awt return` cleans
    // it - so nothing here expects the directory to survive.
    let lease = crate::Worktrees::at(&repo)
        .lease(&format!("ai-team:{}", agent.role))
        .await
        .map_err(|error| Error::invalid(format!("could not lease a worktree: {error}")))?;

    let outcome = drive(&mut store, &support, lease.path(), run.id, &agent, &message).await;

    if outcome.is_ok() {
        record_work(
            &mut store,
            &agent,
            run.id,
            project_id,
            lease.path(),
            &message,
        )
        .await?;
    }

    // Returned whatever happened: a lease held by a crashed turn is a worktree nobody else
    // can have.
    let _ = lease.release().await;

    match outcome {
        Ok(()) => {
            store.set_run_status(run.id, RunStatus::Done)?;
            Ok(run.id)
        }
        Err(error) => {
            store.block_run(run.id, &error.to_string())?;
            store.set_run_status(run.id, RunStatus::Failed)?;
            Err(error)
        }
    }
}

/// Start one seat directly in a checkout the operator already chose.
///
/// Unlike [`start_turn`], this does not lease, commit, or return the worktree. A human task
/// worktree is persistent state owned by its user; cleaning it when the turn ends would
/// destroy exactly the work this action was asked to produce.
pub async fn start_turn_in(
    agent_id: i64,
    message: String,
    worktree: std::path::PathBuf,
) -> Result<i64> {
    let db = crate::default_db_path()?;
    let mut store = Store::open(&db)?;
    let found = target(&store, agent_id)?;
    if !found.agent.enabled {
        return Err(Error::invalid(format!(
            "{} is switched off, so it will not be given work",
            found.agent.role
        )));
    }
    if found.agent.role == crate::ROOT_ROLE {
        return Err(Error::invalid(
            "the orchestrator coordinates team runs; it cannot start planless direct work",
        ));
    }
    let repo = found.repo.as_deref().ok_or_else(|| {
        Error::invalid("that project has no checkout, so there is nowhere to work")
    })?;
    let worktree = crate::Worktrees::at(repo).resolve(&worktree).await?;
    let run = store.create_run_in(
        found.project_id,
        &message,
        RunTrigger::Manual,
        Some(&worktree),
    )?;
    let support = store.support_dir(&found.project_slug)?;

    match drive(
        &mut store,
        &support,
        &worktree,
        run.id,
        &found.agent,
        &message,
    )
    .await
    {
        Ok(()) => {
            store.set_run_status(run.id, RunStatus::Done)?;
            Ok(run.id)
        }
        Err(error) => {
            store.block_run(run.id, &error.to_string())?;
            store.set_run_status(run.id, RunStatus::Failed)?;
            Err(error)
        }
    }
}

/// Keep the work, and make it reviewable.
///
/// Committed *before* the lease goes back, because `awt return` cleans and resets the
/// worktree - so anything not on a branch by now is gone (D14). The first version of this
/// skipped it, the agent did the work, and the work was discarded.
///
/// Named for the seat and the run rather than for a slice, because there is no slice: this
/// turn came from somebody talking to an agent, not from the plan.
async fn record_work(
    store: &mut Store,
    agent: &Agent,
    run_id: i64,
    project_id: i64,
    worktree: &Path,
    message: &str,
) -> Result<()> {
    let branch = format!("ai-team/{}-{}", agent.role.to_lowercase(), run_id);
    let subject = message.lines().next().unwrap_or(message).trim();

    let note = |store: &mut Store, kind, text: String| -> Result<()> {
        store.append_event(
            run_id,
            crate::model::NewEvent::new(kind, text).by(&agent.role),
        )?;
        Ok(())
    };

    match keep(worktree, &branch, subject).await {
        Ok(Some(sha)) => {
            note(
                store,
                crate::model::EventKind::Note,
                format!("committed to {branch} ({})", &sha[..7.min(sha.len())]),
            )?;
            // Reviewable, for the same reason a slice's work is: a branch nobody can comment
            // on is a branch you have to go and find in a terminal.
            let author = store
                .node_runs(run_id)?
                .into_iter()
                .next_back()
                .map(|found| found.id);
            let _ = store.open_review(project_id, subject, Some(run_id), author, Some(&branch));
        }
        // Nothing changed. Worth recording rather than silently succeeding: a turn that
        // edited nothing did not do what it was asked.
        Ok(None) => note(
            store,
            crate::model::EventKind::Note,
            "the turn finished without changing a file".to_string(),
        )?,
        Err(error) => note(
            store,
            crate::model::EventKind::Failed,
            format!("could not keep the work: {error}"),
        )?,
    }
    Ok(())
}

/// How a direct message is put to an agent.
///
/// Framing this turn is not optional. A seat's instructions tell it to work through the
/// plan, and this turn has no plan - the plan tools are deliberately unconfigured, because
/// writing to whichever plan the cwd resolves to would be worse. Without being told, the
/// first agent asked went looking for a plan, got "AI_TEAM_PLAN_ROOT is not set", and
/// stopped without touching a file.
///
/// So it says three things: this came from a person, there is no plan here, and the work is
/// the message.
fn as_instruction(said: &str, house: &[crate::house::Rules]) -> String {
    format!(
        "A person is asking you directly. Do this:\n\n{}\n\nYou are in the selected \
         worktree - work only inside it. This turn is not part of a plan, so the \
         planning tools are unavailable on purpose; do not look for a plan or a slice, and \
         do not try to record one. Run the project's own checks before you call it done, and \
         say plainly if they do not pass. Your work is kept for you when the turn ends, so \
         do not commit.{}",
        said.trim(),
        crate::house::section(house)
    )
}

/// Put whatever changed on a branch, so it survives the lease being returned.
/// Commit whatever changed, under a subject git will not complain about.
async fn keep(worktree: &Path, branch: &str, subject: &str) -> Result<Option<String>> {
    let changed = crate::neighbours::git::changed_paths(worktree).await?;
    // Clipped on a character boundary, because what somebody typed is not a commit subject
    // and slicing bytes through a multi-byte character panics.
    let subject: String = subject.chars().take(72).collect();
    crate::neighbours::git::commit_paths(worktree, branch, &subject, &changed).await
}

/// Take one turn for a seat somebody spoke to.
async fn drive(
    store: &mut Store,
    support: &Path,
    worktree: &Path,
    run_id: i64,
    agent: &Agent,
    message: &str,
) -> Result<()> {
    store.set_run_status(run_id, RunStatus::Running)?;
    let registry = crate::machine::ModelRegistry::load()?;
    let node_run_id = store.dispatch(run_id, agent.id, None, &registry)?.id;
    let branch = crate::current_branch(worktree).await;
    store.attach_worktree(
        node_run_id,
        &worktree.to_string_lossy(),
        branch.as_deref(),
        None,
    )?;

    // Running, not queued. The first version left it queued for the whole turn, which meant
    // the crew panel called it working (queued is about to work) while `target` refused to
    // treat it as live - so speaking to a seat you could watch working started a second
    // turn instead. One state, read the same way by both.
    store.set_node_status(node_run_id, NodeStatus::Running)?;

    // Anything said while this seat was busy goes in front of what was just said, in the
    // order it was said. Taken in the same transaction it is marked delivered in, so two
    // turns starting together cannot both act on "stop adding tests".
    let mut said = store.take_pending_for(agent.id, node_run_id)?;
    said.push(message.to_string());
    let message = said.join("\n\n");

    let team = store.team(agent.team_id)?;
    let roster = store.agents(agent.team_id)?;
    let (effective, _) = registry.resolve_agents(std::slice::from_ref(agent))?;
    let resolved = effective.first().unwrap_or(agent);

    let mut turn = crate::PiSeat {
        agent,
        provider: resolved.provider,
        model: &resolved.model,
        worktree,
        support,
        sources: &registry.context_sources(),
        // No plan behind this turn, so the planning tools stay absent rather than pointing
        // at whichever plan the working directory resolves to.
        plan: None,
        team: &team,
        roster: &roster,
    }
    .turn(as_instruction(
        &message,
        &crate::house::read_for(worktree, &[]),
    ))?;

    // Resume the seat's own conversation when it has one, so a second thing said to it
    // arrives with the first behind it rather than as an instruction from nowhere.
    if let Some(session) = store.node_run(node_run_id)?.session_id {
        turn.session_id = Some(session);
    }

    loop {
        match crate::run_pi_turn(store, node_run_id, &turn, |_| {}).await {
            Ok((_, outcome)) => {
                // Replies written while this process was working belong to this exact
                // conversation. Resume before the caller commits or returns a borrowed
                // lease; a detached worker would otherwise race the cleanup.
                let replies = store.take_pending_for(agent.id, node_run_id)?;
                if !replies.is_empty() {
                    turn.prompt = as_instruction(
                        &format!(
                            "A person replied in this conversation:\n\n{}\n\nContinue the same work and respond to them.",
                            replies.join("\n\n")
                        ),
                        &crate::house::read_for(worktree, &[]),
                    );
                    turn.session_id = store.node_run(node_run_id)?.session_id;
                    continue;
                }
                store.set_node_status(node_run_id, crate::supervise::outcome_status(&outcome))?;
                return Ok(());
            }
            Err(error) => {
                store.block_node(node_run_id, &error.to_string())?;
                store.set_node_status(node_run_id, NodeStatus::Failed)?;
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::ModelRegistry;
    use crate::model::{NewProject, NewRepo};

    fn seeded() -> (Store, i64, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .attach_repo(
                project.id,
                NewRepo {
                    main_path: Some("/tmp".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        (store, project.id, team.id)
    }

    fn seat(store: &Store, team: i64, role: &str) -> i64 {
        store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == role)
            .unwrap()
            .id
    }

    #[test]
    fn an_idle_maker_would_start_direct_work_rather_than_interrupt() {
        // Direct planless work is a maker operation. The orchestrator owns the team
        // workflow and must never enter this path.
        let (store, _, team) = seeded();
        let target = target(&store, seat(&store, team, "backend")).unwrap();
        assert_eq!(target.would(), Would::StartWork);
        assert!(target.live.is_none());
    }

    #[test]
    fn an_idle_orchestrator_would_coordinate_the_team() {
        let (store, _, team) = seeded();
        let target = target(&store, seat(&store, team, "orchestrator")).unwrap();

        assert_eq!(target.would(), Would::Coordinate);
        assert!(target.live.is_none());
    }

    #[test]
    fn an_orchestrator_with_a_plan_awaiting_approval_would_continue_that_run() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        store.set_run_plan(run.id, "widget-plan").unwrap();
        store
            .block_run(run.id, crate::workflow::PLAN_APPROVAL_REASON)
            .unwrap();
        let orchestrator = seat(&store, team, "orchestrator");
        let node = store
            .dispatch(run.id, orchestrator, None, &ModelRegistry::local_only())
            .unwrap();
        store
            .set_node_session(node.id, "orchestrator-session")
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Done).unwrap();

        let target = target(&store, orchestrator).unwrap();
        assert_eq!(target.would(), Would::ApprovePlan);
        assert_eq!(target.approval_run_id, Some(run.id));
    }

    #[test]
    fn a_seat_mid_turn_would_have_the_message_queued() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let target = target(&store, backend).unwrap();
        assert_eq!(target.would(), Would::Queue);
        assert_eq!(target.live.as_ref().map(|live| live.0), Some(node.id));
    }

    #[test]
    fn workspace_talk_reaches_a_leased_node_from_its_initiating_checkout() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run_in(
                project,
                "ship it",
                RunTrigger::Manual,
                Some(Path::new("/tmp/widget-task")),
            )
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store
            .attach_worktree(node.id, "/tmp/pool/4", Some("ai-team/S1"), None)
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        assert_eq!(
            target_in(&store, backend, Some(Path::new("/tmp/widget-task")))
                .unwrap()
                .would(),
            Would::Queue
        );
        assert_eq!(
            target_in(&store, backend, Some(Path::new("/tmp")))
                .unwrap()
                .would(),
            Would::StartWork
        );
    }

    #[test]
    fn a_parked_seat_is_reachable_because_that_is_when_you_most_want_to_talk() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Parked).unwrap();

        assert_eq!(target(&store, backend).unwrap().would(), Would::Queue);
    }

    #[test]
    fn a_finished_seat_is_not_live_however_recently_it_ran() {
        // Its port is still in the row; the process is gone. Treating it as live would be a
        // box that swallows what somebody typed.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Done).unwrap();

        let target = target(&store, backend).unwrap();
        assert!(target.live.is_none());
        assert_eq!(target.would(), Would::StartWork);
    }

    #[test]
    fn a_seat_that_is_switched_off_would_do_nothing() {
        // It will never be given work, so a message to it would go nowhere - said before
        // somebody types one.
        let (mut store, _, team) = seeded();
        let backend = seat(&store, team, "backend");
        let agent = store.agent(backend).unwrap();
        let mut update = crate::model::NewAgent::from(&agent);
        update.enabled = false;
        store.update_agent(backend, update).unwrap();

        assert_eq!(target(&store, backend).unwrap().would(), Would::Nothing);
    }

    #[test]
    fn only_the_newest_attempt_counts_as_live() {
        // A retry is a new row, and an older attempt's recorded port belongs to a process
        // that has certainly gone.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");

        let first = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(first.id, "old").unwrap();
        store
            .set_node_status(first.id, NodeStatus::Running)
            .unwrap();

        let second = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(second.id, "new").unwrap();
        store
            .set_node_status(second.id, NodeStatus::Running)
            .unwrap();

        let live = target(&store, backend).unwrap().live.unwrap();
        assert_eq!(live.0, second.id);
        assert_eq!(live.1, "new");
    }

    #[test]
    fn a_queued_node_is_not_treated_as_live() {
        // It has no session yet, so there is nothing to say anything to. This is the state
        // `drive` used to leave a node in for a whole turn, which made the crew panel and
        // this disagree about whether a seat could be spoken to.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, None, &ModelRegistry::local_only())
            .unwrap();
        assert_eq!(store.node_run(node.id).unwrap().status, NodeStatus::Queued);
        assert!(target(&store, backend).unwrap().live.is_none());

        // And once it is actually running, with a session, it is.
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();
        assert!(target(&store, backend).unwrap().live.is_some());
    }

    #[test]
    fn a_plan_less_turn_is_told_that_it_has_no_plan() {
        // The first agent asked went looking for a plan, got "AI_TEAM_PLAN_ROOT is not set"
        // from tools that are unconfigured on purpose, and stopped without touching a file.
        let framed = as_instruction("Add a subtract function.", &[]);

        assert!(framed.contains("Add a subtract function."));
        assert!(framed.contains("not part of a plan"), "{framed}");
        assert!(framed.contains("do not look for a plan"), "{framed}");
        // And that a person is asking, which is why there is no slice to read.
        assert!(framed.contains("asking you directly"), "{framed}");
        // Committing is ai-team's job here, as everywhere (D14).
        assert!(framed.contains("do not commit"), "{framed}");
    }

    #[test]
    fn queueing_for_a_seat_that_is_not_live_says_so_rather_than_pretending() {
        let (mut store, _, team) = seeded();
        let target = target(&store, seat(&store, team, "backend")).unwrap();
        let error = queue(&mut store, &target, "hello").unwrap_err().to_string();
        assert!(error.contains("no turn in progress"), "{error}");
    }

    #[test]
    fn a_message_to_a_busy_seat_waits_and_says_how_many_are_waiting() {
        // Somebody who says three things to a working agent should be told three are
        // waiting, not told three times that one is.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let target = target(&store, backend).unwrap();
        let first = queue(&mut store, &target, "use i64").unwrap();
        assert_eq!(
            first,
            Reached::Queued {
                node_run_id: node.id,
                waiting: 1
            }
        );
        let second = queue(&mut store, &target, "and stop adding tests").unwrap();
        assert_eq!(
            second,
            Reached::Queued {
                node_run_id: node.id,
                waiting: 2
            }
        );
    }

    #[test]
    fn a_conversation_reply_is_visible_and_waits_for_the_same_seat() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();

        let waiting = store
            .queue_conversation(
                node.id,
                backend,
                "Yes - use the smaller type.\nThen continue.",
            )
            .unwrap();
        assert_eq!(waiting, 1);
        assert_eq!(
            store.take_pending_for(backend, node.id).unwrap(),
            ["Yes - use the smaller type.\nThen continue."]
        );

        let event = store.events(run.id, None, 10).unwrap().pop().unwrap();
        assert_eq!(event.actor.as_deref(), Some("human"));
        assert_eq!(event.node_run_id, Some(node.id));
        assert_eq!(
            event
                .payload
                .unwrap()
                .get("body")
                .and_then(serde_json::Value::as_str),
            Some("Yes - use the smaller type.\nThen continue.")
        );
    }

    #[test]
    fn a_waiting_message_is_delivered_once_and_in_order() {
        // Delivered in the same transaction it is read in: two turns starting together
        // must not both act on "stop adding tests".
        let (mut store, _, team) = seeded();
        let backend = seat(&store, team, "backend");

        store.queue_legacy_message(backend, "first").unwrap();
        store.queue_legacy_message(backend, "second").unwrap();
        assert_eq!(store.waiting_for(backend).unwrap(), 2);

        let taken = store.take_legacy_pending(backend).unwrap();
        assert_eq!(taken, ["first", "second"]);
        assert_eq!(store.waiting_for(backend).unwrap(), 0);
        assert!(store.take_legacy_pending(backend).unwrap().is_empty());
    }

    #[test]
    fn one_seat_s_messages_do_not_reach_another() {
        let (mut store, _, team) = seeded();
        let backend = seat(&store, team, "backend");
        let frontend = seat(&store, team, "frontend");

        store
            .queue_legacy_message(backend, "for the backend")
            .unwrap();
        assert!(store.take_legacy_pending(frontend).unwrap().is_empty());
        assert_eq!(
            store.take_legacy_pending(backend).unwrap(),
            ["for the backend"]
        );
    }

    #[test]
    fn a_legacy_unscoped_reply_is_delivered_after_migration() {
        let (mut store, project, team) = seeded();
        let backend = seat(&store, team, "backend");
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();

        store
            .queue_legacy_message(backend, "said before schema 11")
            .unwrap();
        assert_eq!(
            store.take_pending_for(backend, node.id).unwrap(),
            ["said before schema 11"]
        );
    }

    #[test]
    fn two_workspaces_cannot_take_each_others_replies() {
        let (mut store, project, team) = seeded();
        let backend = seat(&store, team, "backend");
        let first_run = store
            .create_run(project, "first", RunTrigger::Manual)
            .unwrap();
        let second_run = store
            .create_run(project, "second", RunTrigger::Manual)
            .unwrap();
        let registry = ModelRegistry::local_only();
        let first = store
            .dispatch(first_run.id, backend, Some("S1"), &registry)
            .unwrap();
        let second = store
            .dispatch(second_run.id, backend, Some("S2"), &registry)
            .unwrap();

        store
            .queue_node_message(first.id, backend, "only the first")
            .unwrap();
        assert!(store
            .take_pending_for(backend, second.id)
            .unwrap()
            .is_empty());
        assert_eq!(
            store.take_pending_for(backend, first.id).unwrap(),
            ["only the first"]
        );
    }

    #[test]
    fn a_message_survives_the_process_it_was_aimed_at() {
        // On eve this was a failure case: the message went to a live HTTP session, so a
        // process that had gone since the row was read meant the message had nowhere to
        // land and the caller was told to say it again.
        //
        // Queueing removes the failure rather than handling it. The message is for the
        // seat's next turn, not for this process, so whether that process is still up is
        // not a question worth asking - and nothing anybody typed is lost to a race.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let target = target(&store, backend).unwrap();
        queue(&mut store, &target, "use i64").unwrap();

        // The turn ends, successfully or not. The message is still there for the next one.
        store.set_node_status(node.id, NodeStatus::Failed).unwrap();
        assert_eq!(
            store.take_pending_for(backend, node.id).unwrap(),
            ["use i64"]
        );
    }
}
