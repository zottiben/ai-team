//! Writing a Pi turn into the event table.
//!
//! The same contract `eve/ingest.rs` holds, against a different stream: many events
//! arrive, few become rows, and ingesting the same batch twice must not double anything.
//!
//! Pi does not label its events, so the id is synthesised from the session and the
//! event's position in it. That is enough for the property rule 12 actually needs - the
//! partial unique index on `event.eve_event_id` plus `INSERT OR IGNORE` makes re-ingest
//! free - and it is honest about where it comes from: a Pi stream is a pipe read once, so
//! the case this protects against is a resumed session replaying its start, not a
//! reconnect mid-turn.

use rusqlite::params;

use super::event::{Disposition, PiEvent};
use crate::error::Result;
use crate::eve::TerminalState;
use crate::model::Usage;
use crate::store::Store;
use crate::util::now;

/// What one ingested batch amounted to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PiIngested {
    /// Events that produced a row.
    pub recorded: usize,
    /// Events already in the table. A replayed session start makes this non-zero, and
    /// that is the mechanism working rather than a problem.
    pub duplicates: usize,
    /// Transport detail deliberately not kept - the per-token deltas, mostly.
    pub ignored: usize,
    /// Usage summed out of the batch and added to the node.
    pub usage: Usage,
    /// Steps seen, which is what `node_run.turns` counts.
    pub steps: i64,
    /// The terminal state, when the turn reached one.
    pub terminal: Option<TerminalState>,
}

impl Store {
    /// Ingest a batch of parsed Pi events against one node run.
    ///
    /// `from_index` is where this batch starts in the session's stream, so the cursor is
    /// `from_index + batch.len()` and never `cursor + batch.len()` - the second is right
    /// for a contiguous resume and silently wrong for a replay, which is the case that
    /// actually happens.
    pub fn ingest_pi_events(
        &mut self,
        node_run_id: i64,
        session_id: &str,
        from_index: i64,
        events: &[PiEvent],
    ) -> Result<PiIngested> {
        let node = self.node_run(node_run_id)?;
        let run_id = node.run_id;
        let at = now();

        let mut out = PiIngested::default();
        // Read from every event, new or duplicate: a replayed stream still tells us the
        // turn finished.
        for event in events {
            if event.is_terminal() {
                out.terminal = Some(TerminalState::Completed);
            }
            if event.is_failure() {
                out.terminal = Some(TerminalState::Failed);
            }
        }

        let cursor = from_index + i64::try_from(events.len()).unwrap_or(i64::MAX);

        self.db_mut().write(|tx| {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO event
                   (run_id, node_run_id, at, kind, actor, summary, payload_json, eve_event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;

            for (offset, event) in events.iter().enumerate() {
                let Disposition::Record(kind, summary) = event.classify() else {
                    out.ignored += 1;
                    continue;
                };
                let index = from_index + i64::try_from(offset).unwrap_or(0);
                let id = format!("{session_id}:{index}");
                let payload = serde_json::to_string(&event.data)?;

                let rows = insert.execute(params![
                    run_id,
                    node_run_id,
                    at,
                    kind,
                    node.role,
                    summary,
                    payload,
                    id,
                ])?;
                if rows == 0 {
                    out.duplicates += 1;
                    continue;
                }
                out.recorded += 1;
                // Counters accumulate only for rows that were genuinely new. The unique
                // index protects the rows; without this it would not protect the totals,
                // and a replay would count the same step's tokens twice.
                if let Some(usage) = event.usage() {
                    out.usage += usage;
                    out.steps += 1;
                }
            }
            drop(insert);

            tx.execute(
                "UPDATE node_run
                    SET stream_cursor      = ?2,
                        tokens_in          = tokens_in + ?3,
                        tokens_out         = tokens_out + ?4,
                        tokens_cache_read  = tokens_cache_read + ?5,
                        tokens_cache_write = tokens_cache_write + ?6,
                        turns              = turns + ?7,
                        rev = rev + 1,
                        updated_at = ?8
                  WHERE id = ?1",
                params![
                    node_run_id,
                    cursor,
                    out.usage.tokens_in,
                    out.usage.tokens_out,
                    out.usage.cache_read,
                    out.usage.cache_write,
                    out.steps,
                    at,
                ],
            )?;
            Ok(())
        })?;

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, RunTrigger};

    fn event(line: &str) -> PiEvent {
        PiEvent::parse(line).expect("a valid line")
    }

    /// A node run to ingest against.
    fn node(store: &mut Store) -> i64 {
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let agent = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        let run = store
            .create_run(project.id, "build it", RunTrigger::Manual)
            .unwrap();
        let registry = crate::ModelRegistry::local_only();
        store
            .dispatch(run.id, agent.id, None, &registry)
            .unwrap()
            .id
    }

    fn turn() -> Vec<PiEvent> {
        vec![
            event(r#"{"type":"session","id":"s1","cwd":"/w"}"#),
            event(r#"{"type":"turn_start","message":{"model":"claude-sonnet-5"}}"#),
            event(r#"{"type":"message_update","delta":"th"}"#),
            event(r#"{"type":"tool_execution_start","toolName":"read","args":{}}"#),
            event(r#"{"type":"tool_execution_end","toolName":"read","result":{"content":[]}}"#),
            event(
                r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],
                    "stopReason":"stop","usage":{"input":1000,"output":50,"cacheRead":600,"cacheWrite":300}}}"#,
            ),
            event(r#"{"type":"agent_settled"}"#),
        ]
    }

    #[test]
    fn a_turn_becomes_rows_and_the_deltas_do_not() {
        let mut store = Store::memory().unwrap();
        let node_run = node(&mut store);

        let out = store.ingest_pi_events(node_run, "s1", 0, &turn()).unwrap();

        // step, tool_call, tool_result, cost, done - and the message_end inside turn_end
        // is the same event, so five rows, not seven.
        assert_eq!(out.recorded, 5, "{out:?}");
        assert!(out.ignored >= 2, "the deltas and bookends are not rows");
        assert_eq!(out.terminal, Some(TerminalState::Completed));
    }

    #[test]
    fn the_cache_subsets_are_taken_out_of_the_input_total() {
        // Rule 12 in Pi's spelling. 1000 total, 600 read, 300 written - 100 uncached.
        let mut store = Store::memory().unwrap();
        let node_run = node(&mut store);

        let out = store.ingest_pi_events(node_run, "s1", 0, &turn()).unwrap();
        assert_eq!(out.usage.tokens_in, 100, "{out:?}");
        assert_eq!(out.usage.tokens_out, 50);
        assert_eq!(out.usage.cache_read, 600);
        assert_eq!(out.usage.cache_write, 300);

        let stored = store.node_run(node_run).unwrap();
        assert_eq!(stored.usage.tokens_in, 100);
        assert_eq!(stored.usage.cache_read, 600);
    }

    #[test]
    fn ingesting_the_same_batch_twice_changes_nothing() {
        // The property rule 12 is actually about: re-ingest must be free, in the rows and
        // in the totals. The unique index protects the first; only counting genuinely new
        // rows protects the second.
        let mut store = Store::memory().unwrap();
        let node_run = node(&mut store);

        let first = store.ingest_pi_events(node_run, "s1", 0, &turn()).unwrap();
        let again = store.ingest_pi_events(node_run, "s1", 0, &turn()).unwrap();

        assert_eq!(first.recorded, 5);
        assert_eq!(again.recorded, 0, "{again:?}");
        assert_eq!(again.duplicates, 5);
        assert_eq!(again.usage, Usage::default(), "tokens counted twice");
        assert_eq!(again.steps, 0);

        let stored = store.node_run(node_run).unwrap();
        assert_eq!(stored.usage.tokens_in, 100, "the totals moved on a replay");
        assert_eq!(stored.turns, 1);
    }

    #[test]
    fn the_cursor_is_where_the_batch_started_plus_what_it_held() {
        // Never the previous cursor plus the batch: right for a contiguous resume, and
        // silently wrong for a replay, which is the case that actually happens.
        let mut store = Store::memory().unwrap();
        let node_run = node(&mut store);
        let events = turn();

        store
            .ingest_pi_events(node_run, "s1", 0, &events[..3])
            .unwrap();
        assert_eq!(store.node_run(node_run).unwrap().stream_cursor, 3);

        // A replay from the beginning rewinds the cursor rather than advancing it.
        store
            .ingest_pi_events(node_run, "s1", 0, &events[..2])
            .unwrap();
        assert_eq!(store.node_run(node_run).unwrap().stream_cursor, 2);
    }

    #[test]
    fn two_sessions_on_one_node_do_not_collide() {
        // The id is synthesised from session and index, so a second session's event 0 is
        // not the first session's event 0 - which a bare index would make it.
        let mut store = Store::memory().unwrap();
        let node_run = node(&mut store);

        store.ingest_pi_events(node_run, "s1", 0, &turn()).unwrap();
        let second = store.ingest_pi_events(node_run, "s2", 0, &turn()).unwrap();
        assert_eq!(second.recorded, 5, "a retry's evidence was swallowed");
        assert_eq!(second.duplicates, 0);
    }

    #[test]
    fn a_failing_turn_is_terminal_as_failed() {
        let mut store = Store::memory().unwrap();
        let node_run = node(&mut store);
        let events = vec![
            event(r#"{"type":"session","id":"s1"}"#),
            event(r#"{"type":"agent_error","error":"the model refused"}"#),
        ];

        let out = store.ingest_pi_events(node_run, "s1", 0, &events).unwrap();
        assert_eq!(out.terminal, Some(TerminalState::Failed));
    }
}
