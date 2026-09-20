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
    pub branch: Option<String>,
    pub attempt: i64,
    /// Why it stopped, when it stopped badly.
    pub blocked_reason: Option<String>,
    /// The last thing it said, so a card is worth reading rather than just coloured.
    pub last_said: Option<String>,
    /// Whether it can be spoken to right now - a live process with a session.
    pub reachable: bool,
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
    let project = store.project(project_id)?;
    let Some(team_id) = project.team_id else {
        return Ok(Vec::new());
    };

    let registry = crate::machine::ModelRegistry::load().ok();

    // Every node run in this project, newest first, so the first one seen for a role is its
    // latest. Bounded because a long-lived project has thousands and only the newest per
    // seat matters here.
    let runs = store.runs(Some(project_id), 60)?;
    let mut latest: std::collections::HashMap<String, (crate::model::NodeRun, i64)> =
        std::collections::HashMap::new();
    for run in &runs {
        for node in store.node_runs(run.id)? {
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
                NodeStatus::Running | NodeStatus::Queued => Doing::Working,
                NodeStatus::Parked => Doing::Parked,
                NodeStatus::Failed | NodeStatus::Blocked => Doing::Failed,
                NodeStatus::Done | NodeStatus::Cancelled => Doing::Idle,
            },
        };

        // Reachable means a live process with a session, not merely a recorded port: the
        // columns say a process *was* started, and the ordinary case is that it finished
        // (M3-S15). Confirming it properly means a request per seat, which this read will
        // not do - so this is "worth trying", and the attempt reports the truth.
        let reachable = found.is_some_and(|(node, _)| {
            node.eve_port.is_some() && node.session_id.is_some() && doing != Doing::Idle
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
            branch: found.and_then(|(node, _)| node.branch.clone()),
            attempt: found.map_or(0, |(node, _)| node.attempt),
            blocked_reason: found.and_then(|(node, _)| node.blocked_reason.clone()),
            last_said: found
                .map(|(node, run)| (node.id, *run))
                .and_then(|(node_id, run_id)| last_said(store, run_id, node_id)),
            reachable,
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

/// The last thing a node said, in words.
///
/// Read from the event log's summaries, which is what they are for - a one-line
/// human-readable account of each step. Not from the assistant stream: that is captured
/// during a turn and is not queryable after it (rule 8).
fn last_said(store: &Store, run_id: i64, node_run_id: i64) -> Option<String> {
    let events = store.events(run_id, None, 400).ok()?;
    events
        .iter()
        .rev()
        .filter(|event| event.node_run_id == Some(node_run_id))
        // A tool call is machinery; a note, a step or a verdict is something said.
        .find(|event| {
            matches!(
                event.kind,
                crate::model::EventKind::Note
                    | crate::model::EventKind::Step
                    | crate::model::EventKind::Done
                    | crate::model::EventKind::Failed
            ) && !event.summary.trim().is_empty()
        })
        .map(|event| event.summary.clone())
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

    fn member<'a>(crew: &'a [Member], role: &str) -> &'a Member {
        crew.iter()
            .find(|member| member.role == role)
            .unwrap_or_else(|| panic!("no {role} in {:?}", crew.iter().map(|m| &m.role)))
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
        store
            .append_event(
                run.id,
                NewEvent::new(crate::model::EventKind::Note, "added the subtract function")
                    .on_node(node.id),
            )
            .unwrap();
        // A tool call is machinery, not something said.
        store
            .append_event(
                run.id,
                NewEvent::new(crate::model::EventKind::ToolCall, "bash").on_node(node.id),
            )
            .unwrap();

        let crew = of_project(&store, project).unwrap();
        assert_eq!(
            member(&crew, "backend").last_said.as_deref(),
            Some("added the subtract function")
        );
    }

    #[test]
    fn a_seat_that_has_finished_is_not_offered_as_reachable() {
        // Its port is still recorded; the process is gone. Offering to talk to it would be
        // a box that swallows what somebody typed.
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
        store.attach_eve(node.id, 4321, "tok").unwrap();
        store.set_node_session(node.id, "sess-1").unwrap();

        store.set_node_status(node.id, NodeStatus::Running).unwrap();
        let live = of_project(&store, project).unwrap();
        assert!(member(&live, "backend").reachable);

        store.set_node_status(node.id, NodeStatus::Done).unwrap();
        let finished = of_project(&store, project).unwrap();
        assert!(!member(&finished, "backend").reachable);
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
