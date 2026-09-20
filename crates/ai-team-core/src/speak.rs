//! Saying something to one seat.
//!
//! Two quite different acts, and the whole design is about not letting them look like one.
//!
//! A seat **mid-turn** already has an eve process and a session, so a message lands in the
//! middle of what it is doing - the same machinery a submitted review uses to steer the
//! node that wrote the code (M3-S15). That is immediate, and it is an interruption.
//!
//! A seat that is **idle** has no process. Reaching it means starting a turn: generating
//! the project, leasing a worktree, and driving one turn - which is `ait run --worktree`
//! for one agent rather than for the orchestrator. That takes minutes and it is *starting
//! work*, not interrupting it.
//!
//! A single box that silently did either would be a surprise waiting to happen, so which
//! one is about to happen is answerable before anybody presses send.

use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::{Agent, NodeStatus, RunStatus, RunTrigger};
use crate::store::Store;
use crate::supervise::{EveClient, EveEnv, Supervisor};

/// What saying something to this seat would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Would {
    /// Land in the middle of a turn already going.
    Interrupt,
    /// Start a turn. Minutes, and a worktree.
    StartWork,
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
    /// A live session, when there is one: node run, port, token, session.
    pub live: Option<(i64, u16, String, String)>,
}

impl Target {
    pub fn would(&self) -> Would {
        if !self.agent.enabled {
            Would::Nothing
        } else if self.live.is_some() {
            Would::Interrupt
        } else {
            Would::StartWork
        }
    }
}

/// Look up what it would take to reach a seat.
pub fn target(store: &Store, agent_id: i64) -> Result<Target> {
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
    'outer: for run in store.runs(Some(project_id), 20)? {
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
            if let (Some(port), Some(token), Some(session)) = (
                node.eve_port,
                node.eve_token.clone(),
                node.session_id.clone(),
            ) {
                if let Ok(port) = u16::try_from(port) {
                    live = Some((node.id, port, token, session));
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
    })
}

/// What happened.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "reached", rename_all = "snake_case")]
pub enum Reached {
    /// It was mid-turn and took the message.
    Interrupted { node_run_id: i64 },
    /// A turn was started for it.
    ///
    /// Deliberately carries no run id. The caller spawns the turn and returns before the
    /// run row exists, so any number here would be invented - and an invented id reads as
    /// a real one (M3-S17). The run appears in the window on the next tick.
    Started,
    /// Nothing was done, and why.
    Refused { because: String },
}

/// Deliver a message to a live seat.
///
/// Takes no `Store` for the usual reason. The liveness claim in [`Target`] is "worth
/// trying" rather than proven - a process can have gone since the row was read - so the
/// failure here is normal and has to say something useful.
pub async fn interrupt(target: &Target, message: &str) -> Result<Reached> {
    let Some((node_run_id, port, token, session)) = target.live.clone() else {
        return Err(Error::invalid("that seat has no turn in progress"));
    };
    let client = EveClient::new(port, &token);
    if !client.healthy().await {
        return Err(Error::invalid(format!(
            "{} was working a moment ago but its process has gone - say it again to start a \
             fresh turn",
            target.agent.role
        )));
    }
    client.follow_up(&session, message).await?;
    Ok(Reached::Interrupted { node_run_id })
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
    let registry = crate::machine::ModelRegistry::load()?;

    let (agent, team_id, project_id, project_slug, repo) = {
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
            found.agent.team_id,
            found.project_id,
            found.project_slug.clone(),
            repo,
        )
    };

    // A run, so the turn is recorded like any other work rather than happening invisibly.
    // What was said is the prompt, because that is what it is.
    let run = store.create_run(project_id, &message, RunTrigger::Manual)?;

    // Always regenerated: the team rows are the source of truth, and a stale project is how
    // a seat ends up running the model you changed an hour ago (D2).
    let project_dir = store.agents_dir(&project_slug)?;
    let generated = store.generate_project_for_machine(team_id, &project_dir, &registry)?;
    generated.write()?;

    // One eve process per leased worktree (D10). A lease is borrowed - `awt return` cleans
    // it - so nothing here expects the directory to survive.
    let lease = crate::Worktrees::at(&repo)
        .lease(&format!("ai-team:{}", agent.role))
        .await
        .map_err(|error| Error::invalid(format!("could not lease a worktree: {error}")))?;

    let env = EveEnv {
        worktree: lease.path().to_path_buf(),
        token: crate::mint_token(),
        provider_keys: registry.provider_environment(&generated.required_env)?,
        // Not working a plan, so the plan tools stay unconfigured and fail closed rather
        // than writing to whichever plan the cwd resolves to.
        plan_root: None,
        plan_slug: None,
    };

    let outcome = drive(&mut store, &project_dir, &env, run.id, &agent, &message).await;

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
        "A person is asking you directly. Do this:\n\n{}\n\nYou are in a worktree leased \
         for you alone - work only inside it. This turn is not part of a plan, so the \
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

/// Build, start and take one turn.
async fn drive(
    store: &mut Store,
    project_dir: &Path,
    env: &EveEnv,
    run_id: i64,
    agent: &Agent,
    message: &str,
) -> Result<()> {
    let mut supervisor = Supervisor::new(project_dir, env.clone());
    supervisor.install_and_build(|_| {}).await?;

    store.set_run_status(run_id, RunStatus::Running)?;
    let registry = crate::machine::ModelRegistry::load()?;
    let node_run_id = store.dispatch(run_id, agent.id, None, &registry)?.id;

    let client = supervisor.start().await?;
    if let Some(port) = supervisor.port() {
        // Recorded as soon as it serves, so the window can reach this turn - including to
        // say something else to it while it runs (M3-S12).
        store.attach_eve(node_run_id, port, &env.token)?;
    }

    // Running, not queued. The first version left it queued for the whole turn, which meant
    // the crew panel called it working (queued is about to work) while `target` refused to
    // treat it as live - so interrupting a seat you could watch working started a second
    // turn instead. One state, read the same way by both.
    store.set_node_status(node_run_id, NodeStatus::Running)?;

    let result = crate::supervise::run_turn(
        store,
        node_run_id,
        &client,
        &as_instruction(message, &crate::house::read_for(&env.worktree, &[])),
        |_| {},
    )
    .await;

    match result {
        Ok((_, outcome)) => {
            store.set_node_status(node_run_id, crate::supervise::outcome_status(&outcome))?;
            Ok(())
        }
        Err(error) => {
            store.block_node(node_run_id, &error.to_string())?;
            store.set_node_status(node_run_id, NodeStatus::Failed)?;
            Err(error)
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
    fn an_idle_seat_would_start_work_rather_than_interrupt() {
        // The distinction the whole slice is about: these are different acts and a single
        // box that silently did either would be a surprise waiting to happen.
        let (store, _, team) = seeded();
        let target = target(&store, seat(&store, team, "backend")).unwrap();
        assert_eq!(target.would(), Would::StartWork);
        assert!(target.live.is_none());
    }

    #[test]
    fn a_seat_mid_turn_would_be_interrupted() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.attach_eve(node.id, 4321, "tok").unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let target = target(&store, backend).unwrap();
        assert_eq!(target.would(), Would::Interrupt);
        assert_eq!(target.live.as_ref().map(|live| live.0), Some(node.id));
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
        store.attach_eve(node.id, 4321, "tok").unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Parked).unwrap();

        assert_eq!(target(&store, backend).unwrap().would(), Would::Interrupt);
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
        store.attach_eve(node.id, 4321, "tok").unwrap();
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
        store.attach_eve(first.id, 1111, "tok").unwrap();
        store.set_node_session(first.id, "old").unwrap();
        store
            .set_node_status(first.id, NodeStatus::Running)
            .unwrap();

        let second = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.attach_eve(second.id, 2222, "tok").unwrap();
        store.set_node_session(second.id, "new").unwrap();
        store
            .set_node_status(second.id, NodeStatus::Running)
            .unwrap();

        let live = target(&store, backend).unwrap().live.unwrap();
        assert_eq!(live.0, second.id);
        assert_eq!(live.3, "new");
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
        store.attach_eve(node.id, 4321, "tok").unwrap();
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

    #[tokio::test]
    async fn interrupting_a_seat_that_is_not_live_says_so_rather_than_pretending() {
        let (store, _, team) = seeded();
        let target = target(&store, seat(&store, team, "backend")).unwrap();
        let error = interrupt(&target, "hello").await.unwrap_err().to_string();
        assert!(error.contains("no turn in progress"), "{error}");
    }

    #[tokio::test]
    async fn a_process_that_has_gone_since_the_row_was_read_says_what_to_do() {
        // The liveness claim is "worth trying", not proven, so this failure is normal.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");
        let node = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        // A port nothing is listening on.
        store.attach_eve(node.id, 1, "tok").unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let target = target(&store, backend).unwrap();
        let error = interrupt(&target, "hello").await.unwrap_err().to_string();
        assert!(error.contains("its process has gone"), "{error}");
        assert!(error.contains("start a fresh turn"), "{error}");
    }
}
