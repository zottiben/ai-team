//! What every seat is doing, right now.
//!
//! The question a person in the chair actually has, and until now the window could only
//! answer it by opening a run and reading a node list - which is one run's worth of six
//! seats rather than six seats' worth of whatever they are each on.
//!
//! So this is the roster joined to the latest work each seat has, in one read. One read
//! because the window polls: six requests a tick is five too many, and a seat that arrives
//! a tick later than its neighbour makes the whole panel look unstable.
//!
//! Idle seats are included. A team of six where two are working is the thing being looked
//! at, and hiding the other four turns the roster into a mystery - "where is the frontend"
//! is not a question a dashboard should create.

use serde::Serialize;

use crate::error::Result;
use crate::model::NodeStatus;
use crate::store::Store;

/// What a seat is doing.
///
/// Derived rather than stored: `node_run.status` says what one attempt is doing, and this
/// says what the *seat* is doing, which is not the same once a seat has had three attempts
/// across two runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Doing {
    /// Dispatched, with its process still coming up. Nothing to say anything to yet.
    Starting,
    /// Mid-turn.
    Working,
    /// Waiting on a person. The one state worth interrupting somebody for.
    Parked,
    /// Its last attempt failed and nothing has replaced it.
    Failed,
    /// Finished its last piece of work and has nothing now.
    Idle,
    /// Never dispatched in this project.
    Untouched,
    /// Configured off, so it will not be given work at all.
    Disabled,
}

/// The newest durable event for one seat, reduced to what an operational card needs.
#[derive(Debug, Clone, Serialize)]
pub struct MemberActivity {
    pub kind: crate::model::EventKind,
    pub summary: String,
    pub at: String,
}

/// One seat, and what it is up to.
#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub agent_id: i64,
    pub role: String,
    pub name: String,
    /// What dispatch will use, which is not always what the roster says (D13).
    pub provider: String,
    pub model: String,
    pub read_only: bool,
    pub zone: String,

    pub doing: Doing,
    /// The node run this is about, when there is one.
    pub node_run_id: Option<i64>,
    pub run_id: Option<i64>,
    pub slice_key: Option<String>,
    /// Which task of that PR it is on (PW4), when the PR is built as tasks.
    pub task_key: Option<String>,
    pub branch: Option<String>,
    pub pushed_at: Option<String>,
    pub pr_url: Option<String>,
    pub merge_requested_at: Option<String>,
    pub delivery_claim: Option<String>,
    pub delivery_error: Option<String>,
    pub delivery: crate::DeliverySettings,
    pub attempt: i64,
    /// Why it stopped, when it stopped badly.
    pub blocked_reason: Option<String>,
    /// The last thing it said, so a card is worth reading rather than just coloured.
    pub last_said: Option<String>,
    /// The latest observable action, including a tool that has started but not finished.
    pub activity: Option<MemberActivity>,
    /// Whether it can be spoken to right now - a live process with a session.
    pub reachable: bool,
    /// The selected workspace run whose plan is waiting for this orchestrator's approval.
    pub approval_run_id: Option<i64>,
    /// Whether future turns would resume the latest Pi session.
    pub session_active: bool,
    /// A reset is claimed; for the orchestrator this includes its handoff turn.
    pub session_resetting_at: Option<String>,
    /// Latest provider-reported context occupancy for that session, including caches.
    pub context_tokens: Option<i64>,
    pub turns: i64,
    /// Every input token it has sent, cached or not. The rate-limit number (M3-S16).
    pub tokens_in: i64,
    pub tokens_out: i64,
}

/// The crew of one project.
///
/// Ordered by the roster's own `ord`, so the list does not reshuffle as work moves - a
/// panel whose rows swap places while you read it is a panel you have to re-read.
pub fn of_project(store: &Store, project_id: i64) -> Result<Vec<Member>> {
    of_workspace(store, project_id, None)
}

fn approval_run_id(
    store: &Store,
    project_id: i64,
    team_id: i64,
    worktree: Option<&std::path::Path>,
) -> Result<Option<i64>> {
    if let Some(worktree) = worktree {
        return Ok(store
            .blocked_run_in_workspace(
                project_id,
                team_id,
                worktree,
                [
                    crate::workflow::PLAN_APPROVAL_REASON,
                    crate::workflow::PLAN_APPROVAL_PREPARING_REASON,
                ],
            )?
            .map(|run| run.id));
    }
    Ok(store
        .blocked_run_for_project(
            project_id,
            team_id,
            [
                crate::workflow::PLAN_APPROVAL_REASON,
                crate::workflow::PLAN_APPROVAL_PREPARING_REASON,
            ],
        )?
        .map(|run| run.id))
}

/// Each seat's newest row among `runs`, and the run it is in.
///
/// A pull request's worktree shows that PR's crew, not everything its run did (PW1), so
/// from one only the rows that built or checked its PR count.
fn latest_by_role(
    store: &Store,
    runs: &[crate::model::Run],
    worktree: Option<&std::path::Path>,
) -> Result<std::collections::HashMap<String, (crate::model::NodeRun, i64)>> {
    let scope = worktree
        .map(|worktree| store.workspace_scope(worktree))
        .transpose()?;
    let mut latest: std::collections::HashMap<String, (crate::model::NodeRun, i64)> =
        std::collections::HashMap::new();
    for run in runs {
        for node in store.node_runs(run.id)? {
            if scope
                .as_ref()
                .is_some_and(|scope| !scope.covers(&node, run))
            {
                continue;
            }
            // Runs come back newest first, and node runs within one ascend - so a later
            // attempt in the same run replaces an earlier one, and an older run never
            // replaces a newer.
            let keep = match latest.get(&node.role) {
                None => true,
                Some((seen, seen_run)) => *seen_run == run.id && node.id > seen.id,
            };
            if keep {
                latest.insert(node.role.clone(), (node, run.id));
            }
        }
    }
    Ok(latest)
}

/// The crew as seen from one checkout.
///
/// Every configured seat remains visible, but activity comes only from what belongs to
/// that checkout: the runs started in it, wherever their pull requests were built - or,
/// in a pull request's own worktree, the turns that built that PR there.
pub fn of_workspace(
    store: &Store,
    project_id: i64,
    worktree: Option<&std::path::Path>,
) -> Result<Vec<Member>> {
    let project = store.project(project_id)?;
    let Some(team_id) = project.team_id else {
        return Ok(Vec::new());
    };

    let registry = crate::machine::ModelRegistry::load().ok();
    let delivery = store.team(team_id)?.delivery;

    // Every node run in this project, newest first, so the first one seen for a role is its
    // latest. Bounded because a long-lived project has thousands and only the newest per
    // seat matters here.
    let runs = match worktree {
        Some(worktree) => store.runs_in_workspace(project_id, worktree, 60)?,
        None => store.runs(Some(project_id), 60)?,
    };
    let approval_run_id = approval_run_id(store, project_id, team_id, worktree)?;
    let latest = latest_by_role(store, &runs, worktree)?;

    let mut crew = Vec::new();
    for agent in store.agents(team_id)? {
        let resolved = registry
            .as_ref()
            .and_then(|registry| registry.resolve(&agent).ok());
        let (provider, model) = resolved.map_or_else(
            || (agent.provider.as_str().to_string(), agent.model.clone()),
            |resolved| (resolved.provider.as_str().to_string(), resolved.model),
        );

        let found = latest.get(&agent.role);
        let doing = match (agent.enabled, found) {
            (false, _) => Doing::Disabled,
            (true, None) => Doing::Untouched,
            (true, Some((node, _))) => match node.status {
                NodeStatus::Running => Doing::Working,
                // Dispatched but not yet driving: the process is still being built and
                // there is nothing to talk to. Calling it working made this panel disagree
                // with what talking to it would actually do.
                NodeStatus::Queued => Doing::Starting,
                NodeStatus::Parked => Doing::Parked,
                NodeStatus::Failed | NodeStatus::Blocked => Doing::Failed,
                NodeStatus::Done | NodeStatus::Cancelled => Doing::Idle,
            },
        };

        // Pi has no port: its resumable address is the session id. Keep this identical to
        // `speak::target`; otherwise a seat visibly working is labelled as "start work"
        // while the server correctly queues a continuation for its live conversation.
        let reachable = found.is_some_and(|(node, _)| {
            node.session_id.is_some()
                && node.session_retired_at.is_none()
                && node.session_resetting_at.is_none()
                && matches!(node.status, NodeStatus::Running | NodeStatus::Parked)
        });

        crew.push(Member {
            agent_id: agent.id,
            role: agent.role.clone(),
            name: agent.name,
            provider,
            model,
            read_only: agent.read_only,
            zone: agent.zone,
            doing,
            node_run_id: found.map(|(node, _)| node.id),
            run_id: found.map(|(_, run)| *run),
            slice_key: found.and_then(|(node, _)| node.slice_key.clone()),
            task_key: found.and_then(|(node, _)| node.task_key.clone()),
            branch: found.and_then(|(node, _)| node.branch.clone()),
            pushed_at: found.and_then(|(node, _)| node.pushed_at.clone()),
            pr_url: found.and_then(|(node, _)| node.pr_url.clone()),
            merge_requested_at: found.and_then(|(node, _)| node.merge_requested_at.clone()),
            delivery_claim: found.and_then(|(node, _)| node.delivery_claim.clone()),
            delivery_error: found.and_then(|(node, _)| node.delivery_error.clone()),
            delivery,
            attempt: found.map_or(0, |(node, _)| node.attempt),
            blocked_reason: found.and_then(|(node, _)| node.blocked_reason.clone()),
            last_said: found.and_then(|(node, _)| last_said(store, node.id)),
            activity: found.and_then(|(node, _)| latest_activity(store, node.id)),
            reachable,
            approval_run_id: (agent.role == crate::ROOT_ROLE)
                .then_some(approval_run_id)
                .flatten(),
            session_active: found.is_some_and(|(node, _)| {
                node.session_id.is_some()
                    && node.session_retired_at.is_none()
                    && node.session_resetting_at.is_none()
            }),
            session_resetting_at: found.and_then(|(node, _)| node.session_resetting_at.clone()),
            context_tokens: found.and_then(|(node, _)| {
                node.session_retired_at
                    .is_none()
                    .then_some(node.context_tokens)
                    .flatten()
            }),
            turns: found.map_or(0, |(node, _)| node.turns),
            tokens_in: found.map_or(0, |(node, _)| {
                // Every input token sent, cached or not: the rate-limit number rather than
                // the billable one (M3-S16).
                node.usage.tokens_in + node.usage.cache_read + node.usage.cache_write
            }),
            tokens_out: found.map_or(0, |(node, _)| node.usage.tokens_out),
        });
    }

    Ok(crew)
}

/// The newest durable event, whether it is speech, a command, or a result.
fn latest_activity(store: &Store, node_run_id: i64) -> Option<MemberActivity> {
    let event = store.latest_node_event(node_run_id).ok()??;
    Some(MemberActivity {
        kind: event.kind,
        summary: event.summary,
        at: event.at,
    })
}

/// The last thing a node said, in words.
///
/// Read directly from the end of this node's event log. A bounded run-wide first page can
/// never answer this for a long turn: after 400 events it would freeze forever.
fn last_said(store: &Store, node_run_id: i64) -> Option<String> {
    let event = store.latest_node_message_event(node_run_id).ok()??;
    (!event.summary.trim().is_empty()).then_some(event.summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewEvent, NewProject, RunTrigger};
    use crate::ModelRegistry;

    fn seeded() -> (Store, i64, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
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

    fn member<'a>(crew: &'a [Member], role: &str) -> &'a Member {
        crew.iter()
            .find(|member| member.role == role)
            .unwrap_or_else(|| panic!("no {role} in {:?}", crew.iter().map(|m| &m.role)))
    }

    #[test]
    fn a_pr_worktree_shows_the_crew_of_its_pr_not_of_its_run() {
        // One run builds PR1 and PR2 in two worktrees. Seen from PR2's, the backend built
        // nothing: its turns were PR1's, in the other worktree.
        let (mut store, project, team) = seeded();
        let registry = ModelRegistry::local_only();
        let run = store
            .create_run_in(
                project,
                "two PRs",
                RunTrigger::Manual,
                Some(std::path::Path::new("/tmp/widget-main")),
            )
            .unwrap();
        store.set_run_plan(run.id, "csv").unwrap();
        for (role, slice, worktree) in [
            ("backend", "PR1", "/tmp/pool/1"),
            ("frontend", "PR2", "/tmp/pool/2"),
        ] {
            let node = store
                .dispatch_task(
                    run.id,
                    seat(&store, team, role),
                    slice,
                    Some("T1"),
                    &registry,
                )
                .unwrap();
            store
                .attach_worktree(node.id, worktree, Some(&format!("csv/{slice}")), None)
                .unwrap();
            store.set_node_status(node.id, NodeStatus::Done).unwrap();
        }

        let pr2 = of_workspace(&store, project, Some(std::path::Path::new("/tmp/pool/2"))).unwrap();
        assert_eq!(member(&pr2, "frontend").slice_key.as_deref(), Some("PR2"));
        assert_eq!(member(&pr2, "backend").doing, Doing::Untouched);

        // The checkout the run started in still has all of it.
        let main = of_workspace(
            &store,
            project,
            Some(std::path::Path::new("/tmp/widget-main")),
        )
        .unwrap();
        assert_eq!(member(&main, "backend").slice_key.as_deref(), Some("PR1"));
        assert_eq!(member(&main, "frontend").slice_key.as_deref(), Some("PR2"));
    }

    #[test]
    fn every_seat_is_listed_including_the_ones_doing_nothing() {
        // A team of six where two are working is the thing being looked at. Hiding the other
        // four turns the roster into a mystery.
        let (store, project, _) = seeded();
        let crew = of_project(&store, project).unwrap();

        assert_eq!(crew.len(), 6);
        assert!(crew.iter().all(|member| member.doing == Doing::Untouched));
    }

    #[test]
    fn the_order_is_the_rosters_own_so_the_panel_does_not_reshuffle() {
        // A list whose rows swap places while you read it is a list you have to re-read.
        let (store, project, _) = seeded();
        let crew = of_project(&store, project).unwrap();
        let roles: Vec<&str> = crew.iter().map(|member| member.role.as_str()).collect();
        assert_eq!(roles.first(), Some(&"orchestrator"));
        assert_eq!(roles.get(2), Some(&"backend"));
    }

    #[test]
    fn the_orchestrator_exposes_the_exact_run_waiting_for_plan_approval() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        store.set_run_plan(run.id, "widget-plan").unwrap();
        store
            .block_run(run.id, crate::workflow::PLAN_APPROVAL_REASON)
            .unwrap();

        let crew = of_project(&store, project).unwrap();
        assert_eq!(member(&crew, "orchestrator").approval_run_id, Some(run.id));
        assert!(crew
            .iter()
            .filter(|member| member.role != "orchestrator")
            .all(|member| member.approval_run_id.is_none()));
        assert_eq!(
            seat(&store, team, "orchestrator"),
            member(&crew, "orchestrator").agent_id
        );
    }

    #[test]
    fn approval_does_not_disappear_behind_recent_workspace_history() {
        let (mut store, project, _) = seeded();
        let workspace = tempfile::tempdir().unwrap();
        let held = store
            .create_run_in(
                project,
                "approve me",
                RunTrigger::Manual,
                Some(workspace.path()),
            )
            .unwrap();
        store.set_run_plan(held.id, "widget-plan").unwrap();
        store
            .block_run(held.id, crate::workflow::PLAN_APPROVAL_REASON)
            .unwrap();
        for index in 0..65 {
            store
                .create_run_in(
                    project,
                    &format!("later {index}"),
                    RunTrigger::Manual,
                    Some(workspace.path()),
                )
                .unwrap();
        }

        let crew = of_workspace(&store, project, Some(workspace.path())).unwrap();
        assert_eq!(member(&crew, "orchestrator").approval_run_id, Some(held.id));
    }

    #[test]
    fn a_seat_mid_turn_is_working_and_says_which_slice() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let crew = of_project(&store, project).unwrap();
        let backend = member(&crew, "backend");
        assert_eq!(backend.doing, Doing::Working);
        assert_eq!(backend.slice_key.as_deref(), Some("S1"));
        assert_eq!(backend.run_id, Some(run.id));
    }

    #[test]
    fn workspace_crew_keeps_leased_nodes_with_the_run_that_dispatched_them() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run_in(
                project,
                "ship it",
                RunTrigger::Manual,
                Some(std::path::Path::new("/tmp/widget-task")),
            )
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store
            .attach_worktree(node.id, "/tmp/pool/3", Some("ai-team/S1"), None)
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();

        let task = of_workspace(
            &store,
            project,
            Some(std::path::Path::new("/tmp/widget-task")),
        )
        .unwrap();
        let main = of_workspace(
            &store,
            project,
            Some(std::path::Path::new("/tmp/widget-main")),
        )
        .unwrap();
        assert_eq!(member(&task, "backend").doing, Doing::Working);
        assert_eq!(member(&main, "backend").doing, Doing::Untouched);
    }

    #[test]
    fn a_seat_that_finished_is_idle_rather_than_still_working() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Done).unwrap();

        let crew = of_project(&store, project).unwrap();
        assert_eq!(member(&crew, "backend").doing, Doing::Idle);
    }

    #[test]
    fn a_parked_seat_is_its_own_state_because_it_is_waiting_on_a_person() {
        // The one state worth interrupting somebody for, so it cannot be folded into
        // "working".
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Parked).unwrap();

        let crew = of_project(&store, project).unwrap();
        assert_eq!(member(&crew, "backend").doing, Doing::Parked);
    }

    #[test]
    fn a_failed_seat_says_why() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store.block_node(node.id, "gates rejected it").unwrap();
        store.set_node_status(node.id, NodeStatus::Failed).unwrap();

        let crew = of_project(&store, project).unwrap();
        let backend = member(&crew, "backend");
        assert_eq!(backend.doing, Doing::Failed);
        assert_eq!(backend.blocked_reason.as_deref(), Some("gates rejected it"));
    }

    #[test]
    fn the_latest_attempt_is_the_one_shown() {
        // A retry is a new row (D2), so a seat with three attempts has three rows and only
        // the newest describes what it is doing.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = seat(&store, team, "backend");

        let first = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store.set_node_status(first.id, NodeStatus::Failed).unwrap();
        let second = store
            .dispatch(run.id, backend, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        store
            .set_node_status(second.id, NodeStatus::Running)
            .unwrap();

        let crew = of_project(&store, project).unwrap();
        let found = member(&crew, "backend");
        assert_eq!(found.doing, Doing::Working);
        assert_eq!(found.node_run_id, Some(second.id));
    }

    #[test]
    fn a_disabled_seat_says_so_rather_than_looking_merely_idle() {
        // It will never be given work, which is different from having none.
        let (mut store, project, team) = seeded();
        let backend = seat(&store, team, "backend");
        let agent = store.agent(backend).unwrap();
        let mut update = crate::model::NewAgent::from(&agent);
        update.enabled = false;
        store.update_agent(backend, update).unwrap();

        let crew = of_project(&store, project).unwrap();
        assert_eq!(member(&crew, "backend").doing, Doing::Disabled);
    }

    #[test]
    fn a_card_carries_the_last_thing_the_seat_said() {
        // So it is worth reading rather than just coloured.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        // Long Pi sessions routinely pass the event-read bound. The card must read from
        // the end of this node's transcript rather than pinning itself to its first page.
        for index in 0..401 {
            store
                .append_event(
                    run.id,
                    NewEvent::new(crate::model::EventKind::ToolCall, format!("bash {index}"))
                        .on_node(node.id),
                )
                .unwrap();
        }
        store
            .append_event(
                run.id,
                NewEvent::new(crate::model::EventKind::Note, "added the subtract function")
                    .on_node(node.id),
            )
            .unwrap();
        // A later tool call is machinery, not something said, but it is current activity.
        store
            .append_event(
                run.id,
                NewEvent::new(crate::model::EventKind::ToolCall, "bash").on_node(node.id),
            )
            .unwrap();

        let crew = of_project(&store, project).unwrap();
        let backend = member(&crew, "backend");
        assert_eq!(
            backend.last_said.as_deref(),
            Some("added the subtract function")
        );
        let activity = backend.activity.as_ref().expect("latest activity");
        assert_eq!(activity.kind, crate::model::EventKind::ToolCall);
        assert_eq!(activity.summary, "bash");
    }

    #[test]
    fn a_seat_that_has_finished_is_not_offered_as_reachable() {
        // Its session remains recorded, but the process is gone. Generic Talk starts new
        // work; replies in Agent activity are offered only while this turn is live.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();

        store.set_node_status(node.id, NodeStatus::Running).unwrap();
        let live = of_project(&store, project).unwrap();
        assert!(member(&live, "backend").reachable);

        store.set_node_status(node.id, NodeStatus::Done).unwrap();
        let finished = of_project(&store, project).unwrap();
        assert!(!member(&finished, "backend").reachable);
    }

    #[test]
    fn resetting_and_retired_sessions_are_not_presented_as_resumable() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                seat(&store, team, "backend"),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Done).unwrap();
        assert!(member(&of_project(&store, project).unwrap(), "backend").session_active);

        store.claim_node_session_reset(node.id).unwrap();
        let resetting = of_project(&store, project).unwrap();
        assert!(!member(&resetting, "backend").session_active);
        assert!(member(&resetting, "backend").session_resetting_at.is_some());

        store.retire_node_session(node.id).unwrap();
        let retired = of_project(&store, project).unwrap();
        assert!(!member(&retired, "backend").session_active);
        assert!(member(&retired, "backend").session_resetting_at.is_none());
    }

    #[test]
    fn a_project_with_no_team_has_no_crew_rather_than_an_error() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Bare".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(of_project(&store, project.id).unwrap().is_empty());
    }
}
