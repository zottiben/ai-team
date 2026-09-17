//! The event stream: append-only, and the only record of what happened inside a turn.
//!
//! There is no `update_event` and there never will be - the table refuses UPDATE at the
//! trigger level. Everything the Console renders, everything Analytics computes and
//! everything a post-mortem reads comes out of here, so a row that could be rewritten
//! would make all three untrustworthy.

use rusqlite::{params, Row};

use crate::error::{Error, Result};
use crate::model::{Event, NewEvent};
use crate::store::{non_empty, Store};
use crate::util::now;

impl Store {
    pub fn append_event(&mut self, run_id: i64, new: NewEvent) -> Result<Event> {
        let at = now();
        let payload = new
            .payload
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    run_id,
                    new.node_run_id,
                    at,
                    new.kind,
                    new.actor,
                    new.summary,
                    payload
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.event(id)
    }

    pub fn event(&self, id: i64) -> Result<Event> {
        self.db()
            .conn()
            .query_row(
                &format!("{EVENT_SELECT} WHERE id = ?1"),
                params![id],
                event_from_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Error::invalid(format!("no event {id}")),
                other => other.into(),
            })
    }

    /// A run's events in the order they happened. `after` is an id rather than a
    /// timestamp so a client streaming over SSE can resume exactly where it stopped -
    /// two events in the same second are common, and a timestamp cursor would replay or
    /// skip them.
    pub fn events(&self, run_id: i64, after: Option<i64>, limit: i64) -> Result<Vec<Event>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{EVENT_SELECT} WHERE run_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3"
        ))?;
        let rows = stmt
            .query_map(params![run_id, after.unwrap_or(0), limit], event_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn node_events(&self, node_run_id: i64, limit: i64) -> Result<Vec<Event>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{EVENT_SELECT} WHERE node_run_id = ?1 ORDER BY id LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![node_run_id, limit], event_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The approvals a run is currently parked on: every request without a resolution.
    pub fn pending_approvals(&self, run_id: i64) -> Result<Vec<Event>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{EVENT_SELECT}
              WHERE run_id = ?1
                AND kind = 'approval_request'
                AND NOT EXISTS (
                    SELECT 1 FROM event r
                     WHERE r.run_id = event.run_id
                       AND r.kind = 'approval_resolved'
                       AND r.id > event.id
                       AND COALESCE(r.node_run_id, -1) = COALESCE(event.node_run_id, -1))
              ORDER BY id"
        ))?;
        let rows = stmt
            .query_map(params![run_id], event_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn event_count(&self, run_id: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT COUNT(*) FROM event WHERE run_id = ?1",
            params![run_id],
            |r| r.get(0),
        )?)
    }
}

const EVENT_SELECT: &str = "SELECT id, run_id, node_run_id, at, kind, actor, summary, \
     payload_json FROM event";

fn event_from_row(r: &Row<'_>) -> rusqlite::Result<Event> {
    let payload: Option<String> = r.get(7)?;
    Ok(Event {
        id: r.get(0)?,
        run_id: r.get(1)?,
        node_run_id: r.get(2)?,
        at: r.get(3)?,
        kind: r.get(4)?,
        actor: non_empty(r.get(5)?),
        summary: r.get(6)?,
        // A payload that will not parse is not worth failing a whole event feed over -
        // the summary is the part a human reads, and it is still intact.
        payload: payload.and_then(|p| serde_json::from_str(&p).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventKind, NewProject, RunTrigger};

    fn run() -> (Store, i64) {
        let mut s = Store::memory().unwrap();
        let p = s
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        s.seed_default_team(p.id).unwrap();
        let run = s.create_run(p.id, "ship it", RunTrigger::Manual).unwrap();
        (s, run.id)
    }

    #[test]
    fn events_come_back_in_the_order_they_happened() {
        let (mut s, run_id) = run();
        for i in 0..5 {
            s.append_event(run_id, NewEvent::new(EventKind::Step, format!("step {i}")))
                .unwrap();
        }
        let events = s.events(run_id, None, 100).unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].summary, "step 0");
        assert_eq!(events[4].summary, "step 4");
    }

    #[test]
    fn the_cursor_is_an_id_so_a_reconnect_does_not_replay_or_skip() {
        let (mut s, run_id) = run();
        // All in the same second, which is exactly when a timestamp cursor goes wrong.
        let ids: Vec<i64> = (0..4)
            .map(|i| {
                s.append_event(run_id, NewEvent::new(EventKind::Step, format!("s{i}")))
                    .unwrap()
                    .id
            })
            .collect();

        let rest = s.events(run_id, Some(ids[1]), 100).unwrap();
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].id, ids[2]);
    }

    #[test]
    fn a_payload_round_trips_as_json() {
        let (mut s, run_id) = run();
        let event = s
            .append_event(
                run_id,
                NewEvent::new(EventKind::ToolCall, "bash: cargo test")
                    .by("backend")
                    .with(serde_json::json!({ "tool": "bash", "argv": ["cargo", "test"] })),
            )
            .unwrap();

        assert_eq!(event.actor.as_deref(), Some("backend"));
        assert_eq!(event.payload.unwrap()["tool"], "bash");
    }

    #[test]
    fn an_event_cannot_be_rewritten_through_the_store() {
        let (mut s, run_id) = run();
        let event = s
            .append_event(run_id, NewEvent::new(EventKind::Step, "as it happened"))
            .unwrap();

        // There is no update method; going around the store hits the trigger.
        let direct = s.db().conn().execute(
            "UPDATE event SET summary = 'revised' WHERE id = ?1",
            params![event.id],
        );
        assert!(direct.is_err());
        assert_eq!(s.event(event.id).unwrap().summary, "as it happened");
    }

    #[test]
    fn pending_approvals_are_the_ones_nobody_answered() {
        let (mut s, run_id) = run();
        let team = s.project(1).unwrap().team_id.unwrap();
        let agent = s.agents(team).unwrap()[2].id;
        let node = s
            .dispatch(
                run_id,
                agent,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        let other = s
            .dispatch(
                run_id,
                s.agents(team).unwrap()[3].id,
                Some("PR2"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();

        s.append_event(
            run_id,
            NewEvent::new(EventKind::ApprovalRequest, "commit to main?").on_node(node.id),
        )
        .unwrap();
        s.append_event(
            run_id,
            NewEvent::new(EventKind::ApprovalRequest, "rm -rf build?").on_node(other.id),
        )
        .unwrap();

        assert_eq!(s.pending_approvals(run_id).unwrap().len(), 2);

        // Answering one must not clear the other.
        s.append_event(
            run_id,
            NewEvent::new(EventKind::ApprovalResolved, "approved").on_node(node.id),
        )
        .unwrap();

        let still_waiting = s.pending_approvals(run_id).unwrap();
        assert_eq!(still_waiting.len(), 1);
        assert_eq!(still_waiting[0].summary, "rm -rf build?");
    }

    #[test]
    fn deleting_the_run_takes_its_events_with_it() {
        let (mut s, run_id) = run();
        s.append_event(run_id, NewEvent::new(EventKind::Step, "x"))
            .unwrap();
        assert_eq!(s.event_count(run_id).unwrap(), 1);

        s.delete_project(1).unwrap();
        assert_eq!(s.event_count(run_id).unwrap(), 0);
    }
}
