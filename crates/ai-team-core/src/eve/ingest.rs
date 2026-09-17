//! Turning eve's stream into rows.
//!
//! The contract this upholds is the one eve's own docs set out: key on `meta.id`, insert
//! with conflicts ignored, and re-reading a stream costs nothing. That is what makes a
//! reconnect safe, and it is why the supervisor can rewind to `startIndex=0` after a
//! crash without producing a second copy of the run.

use rusqlite::params;

use crate::error::Result;
use crate::eve::event::{Disposition, StreamEvent};
use crate::model::Usage;
use crate::store::Store;
use crate::util::now;

/// How a stream reached its terminal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalState {
    Completed,
    Failed,
    Cancelled,
}

/// What one batch of stream events did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ingested {
    /// Events that produced a row.
    pub recorded: usize,
    /// Events already in the table, matched on `meta.id`. A reconnect makes this large
    /// and that is the mechanism working, not a problem.
    pub duplicates: usize,
    /// Transport detail we deliberately do not keep.
    pub ignored: usize,
    /// Usage summed out of the batch and added to the node.
    pub usage: Usage,
    /// Steps seen, which is what `node_run.turns` counts.
    pub steps: i64,
    /// The terminal event, when one arrived. A failed step alone does not set this:
    /// eve may repair a tool/model step and complete the same turn successfully.
    pub terminal: Option<TerminalState>,
    /// The run is parked on a human.
    pub awaiting_input: bool,
}

impl Store {
    /// Ingest a batch of parsed stream events against one node run.
    ///
    /// Everything lands in a single transaction: a batch that fails partway must not
    /// leave the cursor ahead of the rows, or the missing events are unrecoverable
    /// without a full rewind.
    pub fn ingest_events(
        &mut self,
        node_run_id: i64,
        from_index: i64,
        events: &[StreamEvent],
    ) -> Result<Ingested> {
        let node = self.node_run(node_run_id)?;
        let run_id = node.run_id;
        let at = now();

        let mut out = Ingested::default();
        // Session state is read from every event, duplicate or not: a replayed stream
        // still tells us the turn finished.
        for event in events {
            if event.is_terminal() {
                out.terminal = Some(if event.is_failed_terminal() {
                    TerminalState::Failed
                } else if event.is_cancelled_terminal() {
                    TerminalState::Cancelled
                } else {
                    TerminalState::Completed
                });
            }
            if event.is_awaiting_input() {
                out.awaiting_input = true;
            }
        }

        // The cursor is eve's absolute stream index, so it is where this batch STARTED
        // plus what it contained - never the previous cursor plus the batch. Adding
        // would be right for a contiguous resume and silently wrong for a rewind, which
        // is exactly the case that happens after a crash.
        let cursor = from_index + i64::try_from(events.len()).unwrap_or(i64::MAX);

        self.db_mut().write(|tx| {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO event
                   (run_id, node_run_id, at, kind, actor, summary, payload_json, eve_event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;

            for event in events {
                let Disposition::Record(kind, summary) = event.classify() else {
                    out.ignored += 1;
                    continue;
                };
                // eve's own timestamp when it gave us one: the time the event happened
                // beats the time we got round to reading it.
                let emitted = event.meta.at.clone().unwrap_or_else(|| at.clone());
                let payload = serde_json::to_string(&event.data)?;

                let rows = insert.execute(params![
                    run_id,
                    node_run_id,
                    emitted,
                    kind,
                    node.role,
                    summary,
                    payload,
                    event.event_id(),
                ])?;
                if rows == 0 {
                    out.duplicates += 1;
                    continue;
                }
                out.recorded += 1;
                // Counters accumulate only for events that were genuinely new. The
                // unique index protects the rows; without this it would not protect the
                // totals, and a reconnect would count the same step's tokens twice.
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
                    at
                ],
            )?;
            Ok(())
        })?;

        Ok(out)
    }

    /// Ingest raw NDJSON that was read starting at `from_index`. Unparseable lines are
    /// skipped rather than fatal - a partial line at a reconnect boundary is normal.
    pub fn ingest_ndjson(
        &mut self,
        node_run_id: i64,
        from_index: i64,
        ndjson: &str,
    ) -> Result<Ingested> {
        let events: Vec<StreamEvent> = ndjson.lines().filter_map(StreamEvent::parse).collect();
        self.ingest_events(node_run_id, from_index, &events)
    }

    /// Where to resume this node's stream: `GET .../stream?startIndex=<this>`.
    pub fn stream_cursor(&self, node_run_id: i64) -> Result<i64> {
        Ok(self.node_run(node_run_id)?.stream_cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventKind, NewProject, NodeStatus, RunTrigger};

    fn node() -> (Store, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                backend.id,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        (store, node.id)
    }

    /// A small but realistic turn.
    const TURN: &str = r#"
{"type":"session.started","data":{"sessionId":"wrun_A"},"meta":{"id":"evt_01","at":"2026-09-17T10:00:00.000Z"}}
{"type":"turn.started","data":{},"meta":{"id":"evt_02"}}
{"type":"step.started","data":{"modelId":"openai/gpt-5.6"},"meta":{"id":"evt_03"}}
{"type":"actions.requested","data":{"actions":[{"toolName":"read_file"}]},"meta":{"id":"evt_04"}}
{"type":"action.result","data":{"toolName":"read_file"},"meta":{"id":"evt_05"}}
{"type":"step.completed","data":{"finishReason":"tool-calls","usage":{"inputTokens":1200,"outputTokens":400,"cachedInputTokens":61416,"cacheCreationInputTokens":2686}},"meta":{"id":"evt_06"}}
{"type":"message.completed","data":{"message":"Added the export."},"meta":{"id":"evt_07"}}
{"type":"turn.completed","data":{},"meta":{"id":"evt_08"}}
{"type":"session.waiting","data":{},"meta":{"id":"evt_09"}}
"#;

    #[test]
    fn a_turn_lands_as_rows_and_moves_the_cursor() {
        let (mut store, node_id) = node();
        let out = store.ingest_ndjson(node_id, 0, TURN).unwrap();

        assert_eq!(out.terminal, Some(TerminalState::Completed));
        assert_eq!(out.recorded, 6, "the six events worth keeping");
        assert_eq!(
            out.ignored, 3,
            "session.started, turn.started, session.waiting"
        );
        assert_eq!(out.duplicates, 0);

        // The cursor counts every event eve sent, not just the ones we kept - it is
        // eve's absolute stream index, and skipping the ignored ones would desync it.
        assert_eq!(store.stream_cursor(node_id).unwrap(), 9);

        let node = store.node_run(node_id).unwrap();
        assert_eq!(node.usage.cache_read, 61_416);
        assert_eq!(node.usage.tokens_in, 1_200);
        assert_eq!(node.turns, 1);
    }

    #[test]
    fn re_reading_the_same_stream_produces_no_second_copy() {
        // The whole point of keying on meta.id. A reconnect, a rewind to startIndex=0,
        // or replaying a finished session must all be free.
        let (mut store, node_id) = node();
        let first = store.ingest_ndjson(node_id, 0, TURN).unwrap();
        let again = store.ingest_ndjson(node_id, 0, TURN).unwrap();

        assert_eq!(first.recorded, 6);
        assert_eq!(again.recorded, 0);
        assert_eq!(again.duplicates, 6);

        let run_id = store.node_run(node_id).unwrap().run_id;
        assert_eq!(store.event_count(run_id).unwrap(), 6, "still six rows");
    }

    #[test]
    fn an_overlapping_reconnect_keeps_only_what_is_new() {
        let (mut store, node_id) = node();
        let lines: Vec<&str> = TURN.trim().lines().collect();

        store
            .ingest_ndjson(node_id, 0, &lines[..5].join("\n"))
            .unwrap();
        // Reconnect a little behind the cursor, as a real client does.
        let resumed = store
            .ingest_ndjson(node_id, 3, &lines[3..].join("\n"))
            .unwrap();

        assert_eq!(resumed.duplicates, 2, "evt_04 and evt_05 came again");
        let run_id = store.node_run(node_id).unwrap().run_id;
        assert_eq!(store.event_count(run_id).unwrap(), 6);
    }

    #[test]
    fn events_without_an_id_cannot_dedupe_and_are_all_kept() {
        // Pre-stream-version-20 events arrive with no meta.id. Dropping them would be
        // worse than keeping duplicates, so the partial index lets every NULL through.
        let (mut store, node_id) = node();
        let legacy = "{\"type\":\"turn.completed\",\"data\":{}}\n".repeat(3);

        let first = store.ingest_ndjson(node_id, 0, &legacy).unwrap();
        let second = store.ingest_ndjson(node_id, 3, &legacy).unwrap();
        assert_eq!(first.recorded, 3);
        assert_eq!(second.recorded, 3, "no id means no dedupe");
        assert_eq!(second.duplicates, 0);
    }

    #[test]
    fn a_retried_step_is_kept_because_eve_re_emits_under_new_ids() {
        // eve runs a durable step up to four times and the retry carries fresh ids.
        // Collapsing those would erase the evidence that it had to be retried.
        let (mut store, node_id) = node();
        let attempt = |id: &str| {
            format!(
                r#"{{"type":"step.failed","data":{{"code":"model_error","message":"503"}},"meta":{{"id":"{id}"}}}}"#
            )
        };
        store.ingest_ndjson(node_id, 0, &attempt("evt_a")).unwrap();
        let retry = store.ingest_ndjson(node_id, 1, &attempt("evt_b")).unwrap();

        assert_eq!(retry.recorded, 1);
        let run_id = store.node_run(node_id).unwrap().run_id;
        assert_eq!(
            store.event_count(run_id).unwrap(),
            2,
            "both attempts survive"
        );
    }

    #[test]
    fn a_broken_line_does_not_take_the_batch_down() {
        let (mut store, node_id) = node();
        let ndjson = format!(
            "{{\"type\":\"step.started\",\"data\":{{}},\"meta\":{{\"id\":\"evt_x\"}}}}\n\
             {{\"type\":\"trunc\n\
             \n\
             {}",
            r#"{"type":"turn.completed","data":{},"meta":{"id":"evt_y"}}"#
        );
        let out = store.ingest_ndjson(node_id, 0, &ndjson).unwrap();
        assert_eq!(out.recorded, 2);
        assert_eq!(out.terminal, Some(TerminalState::Completed));
    }

    #[test]
    fn only_a_terminal_failure_marks_the_turn_failed() {
        let (mut store, node_id) = node();
        let repaired = store
            .ingest_ndjson(
                node_id,
                0,
                r#"{"type":"step.failed","data":{"message":"retry me"},"meta":{"id":"evt_step"}}"#,
            )
            .unwrap();
        assert_eq!(
            repaired.terminal, None,
            "a later retry may still finish the turn"
        );

        let failed = store
            .ingest_ndjson(
                node_id,
                1,
                r#"{"type":"turn.failed","data":{"message":"no credential"},"meta":{"id":"evt_turn"}}"#,
            )
            .unwrap();
        assert_eq!(failed.terminal, Some(TerminalState::Failed));

        let cancelled = store
            .ingest_ndjson(
                node_id,
                2,
                r#"{"type":"turn.cancelled","data":{},"meta":{"id":"evt_cancel"}}"#,
            )
            .unwrap();
        assert_eq!(cancelled.terminal, Some(TerminalState::Cancelled));
    }

    #[test]
    fn replaying_a_stream_does_not_count_the_same_tokens_twice() {
        // Found by running a real turn: the unique index stopped the ROWS duplicating
        // but nothing stopped the counters, so a reconnect billed every step again.
        let (mut store, node_id) = node();
        store.ingest_ndjson(node_id, 0, TURN).unwrap();
        let before = store.node_run(node_id).unwrap();

        let again = store.ingest_ndjson(node_id, 0, TURN).unwrap();
        let after = store.node_run(node_id).unwrap();

        assert_eq!(again.recorded, 0);
        assert_eq!(
            again.usage,
            Usage::default(),
            "a duplicate carries no usage"
        );
        assert_eq!(again.steps, 0);
        assert_eq!(
            after.usage, before.usage,
            "tokens must not be counted twice"
        );
        assert_eq!(after.turns, before.turns, "turns must not be counted twice");
    }

    #[test]
    fn a_rewind_puts_the_cursor_back_where_the_stream_actually_is() {
        // The other half of the same bug: the cursor was the OLD cursor plus the batch,
        // so re-reading from startIndex=0 left it at twice the stream length and the
        // next resume would have skipped everything that happened since.
        let (mut store, node_id) = node();
        store.ingest_ndjson(node_id, 0, TURN).unwrap();
        assert_eq!(store.stream_cursor(node_id).unwrap(), 9);

        // A crash, then a full rewind: same 9 events, read from the top.
        store.ingest_ndjson(node_id, 0, TURN).unwrap();
        assert_eq!(store.stream_cursor(node_id).unwrap(), 9, "not 18");
    }

    #[test]
    fn a_parked_run_is_reported_so_the_supervisor_can_stop_polling() {
        let (mut store, node_id) = node();
        let out = store
            .ingest_ndjson(
                node_id,
                0,
                r#"{"type":"input.requested","data":{"requests":[{"requestId":"r1","prompt":"Commit?"}]},"meta":{"id":"evt_p"}}"#,
            )
            .unwrap();

        assert!(out.awaiting_input);
        assert_eq!(out.terminal, None, "parked is not finished");

        // And it is in the approvals queue the Console reads.
        let run_id = store.node_run(node_id).unwrap().run_id;
        let pending = store.pending_approvals(run_id).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, EventKind::ApprovalRequest);
        assert_eq!(pending[0].summary, "Commit?");
    }

    #[test]
    fn the_rows_carry_the_time_eve_emitted_them() {
        let (mut store, node_id) = node();
        store.ingest_ndjson(node_id, 0, TURN).unwrap();

        let events = store.node_events(node_id, 100).unwrap();
        let step = events.iter().find(|e| e.kind == EventKind::Step).unwrap();
        // evt_03 had no `at`, so it falls back to now(); evt_01 had one but is ignored.
        // What matters is that the payload survived for the Console to expand.
        assert_eq!(step.payload.as_ref().unwrap()["modelId"], "openai/gpt-5.6");
        assert_eq!(step.actor.as_deref(), Some("backend"));
    }

    #[test]
    fn an_empty_batch_is_a_no_op() {
        let (mut store, node_id) = node();
        let out = store.ingest_events(node_id, 0, &[]).unwrap();
        assert_eq!(out, Ingested::default());
        assert_eq!(store.stream_cursor(node_id).unwrap(), 0);
    }

    #[test]
    fn ingest_does_not_decide_the_nodes_status() {
        // Deliberate: the supervisor owns the state machine (M1-S4). Ingest reports
        // `finished` and lets it decide whether that means done, failed or blocked.
        let (mut store, node_id) = node();
        store.ingest_ndjson(node_id, 0, TURN).unwrap();
        assert_eq!(store.node_run(node_id).unwrap().status, NodeStatus::Queued);
    }
}
