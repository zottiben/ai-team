//! eve's stream vocabulary, and how it maps onto ours.
//!
//! eve emits ~25 event types; `EventKind` has 10. That is deliberate. The stream is a
//! protocol between two processes and will grow arms and legs as eve versions; our
//! `event` table is the permanent record analytics and post-mortems read, and it should
//! not inherit every distinction the transport happens to make this month.
//!
//! So this module is the seam. Everything eve says arrives here as [`StreamEvent`];
//! what we keep is decided in one place, by [`StreamEvent::classify`].

use serde::{Deserialize, Serialize};

use crate::model::{EventKind, Usage};

/// One line of eve's NDJSON stream.
///
/// `data` is kept as raw JSON rather than an enum per type: eve adds fields between
/// versions, and a strict shape here would reject a stream that is merely newer than we
/// are. What we actually depend on is read out deliberately, below.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub meta: StreamMeta,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamMeta {
    /// The `evt_`-prefixed ULID. Absent on events written before eve's stream version
    /// 20, which is why this is an `Option` and the column is nullable.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
}

/// What we do with one stream event.
#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// Write a row, with this kind and summary.
    Record(EventKind, String),
    /// Interesting to the supervisor but not worth a row - the per-token deltas and
    /// input-streaming chatter that would otherwise be 95% of the table.
    Ignore,
}

impl StreamEvent {
    pub fn parse(line: &str) -> Option<StreamEvent> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        serde_json::from_str(line).ok()
    }

    /// The id to dedupe on, when eve gave us one.
    pub fn event_id(&self) -> Option<&str> {
        self.meta.id.as_deref()
    }

    /// Does this event end the turn? The supervisor stops reading on these.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "turn.completed"
                | "turn.failed"
                | "turn.cancelled"
                | "session.waiting"
                | "session.failed"
                | "session.completed"
        )
    }

    /// Does this event mean a human is being asked for something?
    pub fn is_awaiting_input(&self) -> bool {
        self.kind == "input.requested"
    }

    /// Token usage, when the event carries it.
    ///
    /// eve reports usage on `step.completed`. Cache reads and writes are pulled out
    /// separately because folding them into input makes every cold node look like a
    /// runaway - the measured warm Claude step read 61,416 cached tokens.
    pub fn usage(&self) -> Option<Usage> {
        if self.kind != "step.completed" {
            return None;
        }
        // A step that reported no usage still happened, and `turns` counts steps.
        // Returning None here would quietly stop counting turns for any provider that
        // omits the block.
        let Some(usage) = self.data.get("usage") else {
            return Some(Usage::default());
        };
        let read = |keys: &[&str]| -> i64 {
            keys.iter()
                .find_map(|k| usage.get(*k).and_then(serde_json::Value::as_i64))
                .unwrap_or(0)
        };
        Some(Usage {
            // eve has spelled these both ways across versions; accept either rather
            // than silently recording zeros.
            tokens_in: read(&["inputTokens", "promptTokens"]),
            tokens_out: read(&["outputTokens", "completionTokens"]),
            cache_read: read(&["cachedInputTokens", "cacheReadInputTokens"]),
            cache_write: read(&["cacheCreationInputTokens", "cacheWriteInputTokens"]),
        })
    }

    /// The session id eve assigned, when the event carries one.
    pub fn session_id(&self) -> Option<&str> {
        self.data
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
    }

    /// Whether to keep this event, and as what.
    ///
    /// The summary is what shows in TablePlus and in the Console feed, so it is built to
    /// be read at a glance rather than to be complete - the full payload is on the row
    /// beside it.
    pub fn classify(&self) -> Disposition {
        let text = |key: &str| self.data.get(key).and_then(serde_json::Value::as_str);

        match self.kind.as_str() {
            "step.started" => Disposition::Record(
                EventKind::Step,
                match text("modelId") {
                    Some(model) => format!("step on {model}"),
                    None => "step".to_string(),
                },
            ),
            "step.completed" => {
                let reason = text("finishReason").unwrap_or("done");
                Disposition::Record(EventKind::Cost, format!("step finished ({reason})"))
            }
            "step.failed" => Disposition::Record(EventKind::Failed, self.failure("step failed")),

            "actions.requested" => Disposition::Record(EventKind::ToolCall, self.tool_summary()),
            "action.result" => Disposition::Record(
                EventKind::ToolResult,
                if self.tool_failed() {
                    format!("{} failed", self.tool_name())
                } else {
                    format!("{} returned", self.tool_name())
                },
            ),

            "input.requested" => Disposition::Record(
                EventKind::ApprovalRequest,
                self.first_request_summary()
                    .unwrap_or_else(|| "waiting for a human".to_string()),
            ),
            "input.resolved" | "authorization.resolved" => Disposition::Record(
                EventKind::ApprovalResolved,
                text("outcome")
                    .map_or_else(|| "answered".to_string(), |o| format!("answered: {o}")),
            ),

            "turn.completed" => Disposition::Record(EventKind::Done, "turn completed".to_string()),
            "turn.cancelled" => Disposition::Record(EventKind::Note, "turn cancelled".to_string()),
            "turn.failed" => Disposition::Record(EventKind::Failed, self.failure("turn failed")),
            "session.failed" => {
                Disposition::Record(EventKind::Failed, self.failure("session failed"))
            }
            "session.completed" => {
                Disposition::Record(EventKind::Done, "session completed".to_string())
            }

            "message.completed" => Disposition::Record(
                EventKind::Note,
                truncate(text("message").unwrap_or("(no text)"), 200),
            ),

            "compaction.completed" => {
                Disposition::Record(EventKind::Note, "context compacted".to_string())
            }
            "context.cleared" => {
                Disposition::Record(EventKind::Note, "context cleared".to_string())
            }

            // Everything else is transport detail: per-token deltas, input streaming,
            // partial tool snapshots, and the session/turn bookkeeping the supervisor
            // acts on but nobody needs a row for.
            _ => Disposition::Ignore,
        }
    }

    /// eve nests the settled call under `data.result`, so a top-level lookup finds
    /// nothing and every result reads as an anonymous "tool".
    fn tool_name(&self) -> String {
        let direct = self.data.get("toolName");
        let nested = self.data.get("result").and_then(|r| r.get("toolName"));
        direct
            .or(nested)
            .or_else(|| self.data.get("name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool")
            .to_string()
    }

    /// Did the call fail? eve reports it as `data.status`, not a boolean.
    fn tool_failed(&self) -> bool {
        if self
            .data
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            return true;
        }
        matches!(
            self.data.get("status").and_then(serde_json::Value::as_str),
            Some("failed" | "error")
        )
    }

    /// `actions.requested` carries an array; one line naming them beats one row each.
    fn tool_summary(&self) -> String {
        let Some(actions) = self
            .data
            .get("actions")
            .and_then(serde_json::Value::as_array)
        else {
            return self.tool_name();
        };
        let names: Vec<&str> = actions
            .iter()
            .filter_map(|a| {
                a.get("toolName")
                    .or_else(|| a.get("name"))
                    .and_then(serde_json::Value::as_str)
            })
            .collect();
        match names.len() {
            0 => "tool call".to_string(),
            1 => names[0].to_string(),
            _ => format!("{} calls: {}", names.len(), names.join(", ")),
        }
    }

    fn first_request_summary(&self) -> Option<String> {
        let requests = self.data.get("requests")?.as_array()?;
        let first = requests.first()?;
        let text = first
            .get("prompt")
            .or_else(|| first.get("question"))
            .or_else(|| first.get("title"))
            .and_then(serde_json::Value::as_str)?;
        Some(truncate(text, 200))
    }

    /// eve puts failures in `{ code, message, details? }`.
    fn failure(&self, fallback: &str) -> String {
        let message = self
            .data
            .get("message")
            .and_then(serde_json::Value::as_str)
            .or_else(|| self.data.get("error").and_then(serde_json::Value::as_str));
        let code = self.data.get("code").and_then(serde_json::Value::as_str);
        match (code, message) {
            (Some(code), Some(message)) => truncate(&format!("{code}: {message}"), 300),
            (None, Some(message)) => truncate(message, 300),
            (Some(code), None) => format!("{fallback} ({code})"),
            (None, None) => fallback.to_string(),
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    // One line: a summary with a newline in it wrecks every table that renders it.
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max.saturating_sub(1)).collect::<String>() + "\u{2026}"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> StreamEvent {
        StreamEvent::parse(line).expect("parses")
    }

    #[test]
    fn a_blank_or_broken_line_is_skipped_not_fatal() {
        // The stream is NDJSON over a socket; a partial line at a reconnect boundary
        // must not take the supervisor down.
        assert!(StreamEvent::parse("").is_none());
        assert!(StreamEvent::parse("   ").is_none());
        assert!(StreamEvent::parse("{\"type\":").is_none());
    }

    #[test]
    fn an_unknown_event_type_is_ignored_rather_than_rejected() {
        // eve will add event types. A newer stream must not fail an older ai-team.
        let e = event(r#"{"type":"something.new","data":{},"meta":{"id":"evt_1"}}"#);
        assert_eq!(e.classify(), Disposition::Ignore);
        assert_eq!(e.event_id(), Some("evt_1"));
    }

    #[test]
    fn an_event_without_meta_id_still_parses() {
        // Pre-version-20 events come through with no id and cannot be deduplicated.
        let e = event(r#"{"type":"turn.completed","data":{}}"#);
        assert_eq!(e.event_id(), None);
        assert!(e.is_terminal());
    }

    #[test]
    fn usage_keeps_cache_reads_out_of_input() {
        let e = event(
            r#"{"type":"step.completed","data":{"finishReason":"stop","usage":{
                 "inputTokens":1200,"outputTokens":400,
                 "cachedInputTokens":61416,"cacheCreationInputTokens":2686}},
               "meta":{"id":"evt_2"}}"#,
        );
        let usage = e.usage().expect("step.completed carries usage");
        assert_eq!(usage.tokens_in, 1_200);
        assert_eq!(usage.cache_read, 61_416);
        assert_eq!(usage.cache_write, 2_686);
        // The measured warm Claude step: 61k of it is cached and not billable.
        assert_eq!(usage.billable(), 1_200 + 400 + 2_686);
    }

    #[test]
    fn usage_accepts_either_spelling_and_defaults_to_zero() {
        let e = event(
            r#"{"type":"step.completed","data":{"usage":{
                 "promptTokens":10,"completionTokens":5,"cacheReadInputTokens":7}}}"#,
        );
        let usage = e.usage().unwrap();
        assert_eq!(
            (usage.tokens_in, usage.tokens_out, usage.cache_read),
            (10, 5, 7)
        );
        assert_eq!(usage.cache_write, 0);

        // A step with no usage block at all is not an error.
        assert_eq!(
            event(r#"{"type":"step.completed","data":{}}"#).usage(),
            Some(Usage::default())
        );
        // And no other event type reports usage.
        assert!(event(r#"{"type":"turn.completed","data":{}}"#)
            .usage()
            .is_none());
    }

    #[test]
    fn the_deltas_that_would_flood_the_table_are_ignored() {
        for kind in [
            "message.delta",
            "reasoning.delta",
            "action.input.appended",
            "action.partial",
            "turn.started",
            "session.started",
        ] {
            let e = event(&format!(r#"{{"type":"{kind}","data":{{}}}}"#));
            assert_eq!(
                e.classify(),
                Disposition::Ignore,
                "{kind} must not be a row"
            );
        }
    }

    #[test]
    fn a_multi_tool_request_is_one_row_naming_them() {
        let e = event(
            r#"{"type":"actions.requested","data":{"actions":[
                 {"toolName":"read_file"},{"toolName":"bash"}]}}"#,
        );
        assert_eq!(
            e.classify(),
            Disposition::Record(EventKind::ToolCall, "2 calls: read_file, bash".into())
        );
    }

    #[test]
    fn a_tool_result_names_its_tool_from_where_eve_actually_puts_it() {
        // The shape a real turn produced: the settled call is nested under `result`,
        // and a top-level lookup silently yields "tool returned" for everything.
        let ok = event(
            r#"{"type":"action.result","status":"completed","data":{"status":"completed",
                 "result":{"callId":"c1","kind":"tool-result","toolName":"bash",
                           "output":{"exitCode":0}}}}"#,
        );
        assert_eq!(
            ok.classify(),
            Disposition::Record(EventKind::ToolResult, "bash returned".into())
        );

        let failed = event(
            r#"{"type":"action.result","data":{"status":"failed",
                 "result":{"toolName":"write_file"}}}"#,
        );
        assert_eq!(
            failed.classify(),
            Disposition::Record(EventKind::ToolResult, "write_file failed".into())
        );

        // The older flat shape still works, so an upgrade does not lose names.
        let flat = event(r#"{"type":"action.result","data":{"toolName":"bash","isError":true}}"#);
        assert_eq!(
            flat.classify(),
            Disposition::Record(EventKind::ToolResult, "bash failed".into())
        );
    }

    #[test]
    fn a_failure_carries_its_code_and_message() {
        let e = event(
            r#"{"type":"turn.failed","data":{"code":"model_error","message":"upstream 503"}}"#,
        );
        assert_eq!(
            e.classify(),
            Disposition::Record(EventKind::Failed, "model_error: upstream 503".into())
        );
        // And a failure with neither still says something useful.
        let bare = event(r#"{"type":"turn.failed","data":{}}"#);
        assert_eq!(
            bare.classify(),
            Disposition::Record(EventKind::Failed, "turn failed".into())
        );
    }

    #[test]
    fn an_approval_request_summarises_what_is_being_asked() {
        let e = event(
            r#"{"type":"input.requested","data":{"requests":[
                 {"requestId":"req_A","prompt":"Commit to slice/PR1?"}]}}"#,
        );
        assert!(e.is_awaiting_input());
        assert_eq!(
            e.classify(),
            Disposition::Record(EventKind::ApprovalRequest, "Commit to slice/PR1?".into())
        );
    }

    #[test]
    fn summaries_are_one_line_and_bounded() {
        let long = "word ".repeat(200);
        let e = event(&format!(
            r#"{{"type":"message.completed","data":{{"message":"{long}"}}}}"#
        ));
        let Disposition::Record(_, summary) = e.classify() else {
            panic!("message.completed is a row");
        };
        assert_eq!(summary.chars().count(), 200);
        // A newline in a summary wrecks every table that renders it.
        assert!(!summary.contains('\n'));

        let wrapped =
            event("{\"type\":\"message.completed\",\"data\":{\"message\":\"first\\nsecond\"}}");
        let Disposition::Record(_, summary) = wrapped.classify() else {
            panic!("row");
        };
        assert_eq!(summary, "first second");
    }

    #[test]
    fn the_terminal_events_are_the_ones_that_end_a_turn() {
        for kind in [
            "turn.completed",
            "turn.failed",
            "turn.cancelled",
            "session.waiting",
        ] {
            assert!(
                event(&format!(r#"{{"type":"{kind}"}}"#)).is_terminal(),
                "{kind}"
            );
        }
        for kind in ["step.started", "action.result", "input.requested"] {
            assert!(
                !event(&format!(r#"{{"type":"{kind}"}}"#)).is_terminal(),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_cancelled_turn_is_a_note_not_a_failure() {
        // eve is explicit that cancellation is not a failure, and recording it as one
        // would make the gate-pass-rate metric lie.
        let e = event(r#"{"type":"turn.cancelled","data":{}}"#);
        assert_eq!(
            e.classify(),
            Disposition::Record(EventKind::Note, "turn cancelled".into())
        );
    }
}
