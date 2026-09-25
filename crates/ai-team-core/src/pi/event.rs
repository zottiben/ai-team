//! Pi's stream vocabulary, and how it maps onto ours.
//!
//! The same seam `eve/event.rs` was: a coding harness emits far more event types than
//! ai-team's `event` table has kinds, and it should. The stream is a protocol between two
//! processes and will move between versions; the table is the permanent record analytics
//! and post-mortems read. What we keep is decided here, in one place.
//!
//! Pi writes NDJSON on **stdout** rather than over HTTP, which is why there is no client,
//! no port and no token on this side (D20). One line is one event.

use serde::{Deserialize, Serialize};

use crate::model::{EventKind, Usage};

/// One line of Pi's NDJSON stream.
///
/// Everything but the discriminant stays raw. Pi adds fields between versions and a
/// strict shape here would reject a stream that is merely newer than we are - the same
/// reasoning that kept eve's payload as `Value`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// What we do with one stream event.
#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// Write a row, with this kind and summary.
    Record(EventKind, String),
    /// Interesting to the supervisor but not worth a row - the per-token deltas that
    /// would otherwise be most of the table.
    Ignore,
}

impl PiEvent {
    pub fn parse(line: &str) -> Option<PiEvent> {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            return None;
        }
        serde_json::from_str(line).ok()
    }

    /// The session Pi opened, which is how a later turn resumes this one.
    ///
    /// Pi has no server to reattach to, so this is the whole of what "the session" means
    /// on this runtime: an id handed back to `--session-id`.
    pub fn session_id(&self) -> Option<&str> {
        if self.kind != "session" {
            return None;
        }
        self.data.get("id").and_then(serde_json::Value::as_str)
    }

    /// Does this event end the turn? The supervisor stops reading on it.
    ///
    /// `agent_settled` rather than `agent_end`: Pi emits `agent_end` with `willRetry`,
    /// and a turn that is about to retry has not finished. Stopping at the first
    /// `agent_end` would record a retry's first attempt as the whole turn.
    pub fn is_terminal(&self) -> bool {
        self.kind == "agent_settled"
    }

    /// Whether the turn failed, as Pi reports it rather than as we infer it.
    pub fn is_failure(&self) -> bool {
        matches!(self.kind.as_str(), "agent_error" | "turn_error")
    }

    /// Whether this provider attempt ended in an error.
    ///
    /// Pi still emits `agent_settled` after a provider adapter returns an error. That
    /// settles the stream, not the work: treating it as completion ran gates and spent
    /// repairs after Claude had only said "Not logged in". `Some(false)` matters because
    /// Pi may retry before settling and a later successful attempt supersedes the error.
    pub(super) fn provider_turn_failed(&self) -> Option<bool> {
        if self.kind != "turn_end" {
            return None;
        }
        self.data
            .pointer("/message/stopReason")
            .and_then(serde_json::Value::as_str)
            .map(|reason| reason == "error")
    }

    /// The assistant's own words for this event, when it produced any.
    ///
    /// Read from `message.content`, which is where a completed assistant message puts its
    /// text. This is the channel rule 8 requires a verdict to be read from - the model's
    /// answer, captured from the stream rather than filtered back out of the event table,
    /// where ai-team's own dispatch notices also live.
    pub fn assistant_message(&self) -> Option<String> {
        if self.kind != "message_end" && self.kind != "turn_end" {
            return None;
        }
        let message = self.data.get("message")?;
        if message.get("role")?.as_str()? != "assistant" {
            return None;
        }
        let text: String = message
            .get("content")?
            .as_array()?
            .iter()
            .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    /// The usage figures this event carries: on the event itself, or on its message.
    fn usage_figures(&self) -> Option<&serde_json::Value> {
        self.data
            .get("usage")
            .or_else(|| self.data.get("message")?.get("usage"))
    }

    /// Every token of the prompt this step sent, cached or not.
    ///
    /// The trap rule 12 records. Pi reports `cacheRead` and `cacheWrite` beside `input`,
    /// not inside it - a Claude step is `input 1, cacheRead 68881, cacheWrite 257,
    /// output 319, totalTokens 69458`, a ChatGPT one `input 688, cacheRead 49664,
    /// output 331, totalTokens 50683` - so reading `input` as the prompt says a session
    /// 69K tokens deep holds one. `totalTokens` is the figure every provider sums alike,
    /// so the prompt is read from it, and from the parts only when it is missing.
    fn prompt_tokens(usage: &serde_json::Value) -> i64 {
        let read = |key: &str| {
            usage
                .get(key)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        match read("totalTokens") {
            total if total > 0 => (total - read("output")).max(0),
            _ => read("input") + read("cacheRead") + read("cacheWrite"),
        }
    }

    /// What this step spent, with the cached prompt kept apart from the uncached.
    ///
    /// Storing the cache inside the input column would charge a cached prefix twice and
    /// make every cost comparison wrong in the same direction.
    pub fn usage(&self) -> Option<Usage> {
        let usage = self.usage_figures()?;
        let read = |key: &str| {
            usage
                .get(key)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        let cache_read = read("cacheRead");
        let cache_write = read("cacheWrite");
        Some(Usage {
            // Clamped at zero rather than subtracted blind: a total smaller than its own
            // parts must not turn into a negative token count.
            tokens_in: (Self::prompt_tokens(usage) - cache_read - cache_write).max(0),
            tokens_out: read("output"),
            cache_read,
            cache_write,
        })
    }

    /// How full the session is now: the whole prompt of its latest step, cached or not.
    pub fn context_tokens(&self) -> Option<i64> {
        if self.kind != "turn_end" {
            return None;
        }
        Some(Self::prompt_tokens(self.data.get("message")?.get("usage")?))
    }

    /// The model this step ran on, for the step summary.
    fn model(&self) -> Option<&str> {
        let direct = self
            .data
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(serde_json::Value::as_str);
        direct.or_else(|| self.data.get("model").and_then(serde_json::Value::as_str))
    }

    fn tool_name(&self) -> &str {
        self.data
            .get("toolName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("a tool")
    }

    /// Whether a finished tool call reported failure.
    ///
    /// Pi marks it on the result rather than as its own event type, so a result that is
    /// not inspected reads as success - which is how a turn full of refused tool calls
    /// ends up recorded as a clean run.
    fn tool_failed(&self) -> bool {
        let result = self.data.get("result");
        let flagged = result
            .and_then(|r| r.get("isError"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        flagged
            || result
                .and_then(|r| r.get("error"))
                .is_some_and(|e| !e.is_null())
    }

    /// A required planning-context call that failed. Browser and generic web fallbacks
    /// are withheld, so this becomes a visible input-needed state rather than a cue to
    /// route around the source the operator asked the plan to use.
    pub fn wrote_planner_handoff(&self) -> bool {
        self.kind == "tool_execution_end"
            && !self.tool_failed()
            && matches!(
                self.tool_name(),
                "write_handoff" | "ai-planner_write_handoff"
            )
    }

    pub fn context_tool_failure(&self) -> Option<String> {
        if self.kind != "tool_execution_end" || !self.tool_failed() {
            return None;
        }
        let tool = self.tool_name();
        if crate::context::CLICKUP_READ_TOOLS.contains(&tool)
            || crate::context::FIGMA_READ_TOOLS.contains(&tool)
        {
            Some(format!("CONTEXT_UNAVAILABLE: {tool} failed"))
        } else {
            None
        }
    }

    /// One line of what happened, or nothing worth a row.
    pub fn classify(&self) -> Disposition {
        match self.kind.as_str() {
            "turn_start" => Disposition::Record(
                EventKind::Step,
                match self.model() {
                    Some(model) => format!("step on {model}"),
                    None => "step".to_string(),
                },
            ),
            "turn_end" => Disposition::Record(
                EventKind::Cost,
                match self
                    .data
                    .get("message")
                    .and_then(|m| m.get("stopReason"))
                    .and_then(serde_json::Value::as_str)
                {
                    Some(reason) => format!("step finished ({reason})"),
                    None => "step finished".to_string(),
                },
            ),

            "tool_execution_start" => {
                Disposition::Record(EventKind::ToolCall, self.tool_name().to_string())
            }
            "tool_execution_end" => Disposition::Record(
                EventKind::ToolResult,
                if self.tool_failed() {
                    format!("{} failed", self.tool_name())
                } else {
                    format!("{} returned", self.tool_name())
                },
            ),

            // The assistant's prose. Recorded as a Note so the crew panel can show what a
            // seat last said, and deliberately clipped - the full text is in the payload.
            "message_end" => match self.assistant_message() {
                Some(text) => Disposition::Record(EventKind::Note, one_line(&text)),
                None => Disposition::Ignore,
            },

            // Pi's JSON stream strips the growing partial message but retains the completed
            // thinking block. Recording only its end makes reasoning visible before the
            // following tool runs without writing one database row per streamed token.
            "message_update"
                if self
                    .data
                    .pointer("/assistantMessageEvent/type")
                    .and_then(serde_json::Value::as_str)
                    == Some("thinking_end") =>
            {
                let thinking = self
                    .data
                    .pointer("/assistantMessageEvent/content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("thinking");
                Disposition::Record(EventKind::Cost, one_line(thinking))
            }

            "agent_error" | "turn_error" => Disposition::Record(
                EventKind::Failed,
                self.data
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("the turn failed")
                    .to_string(),
            ),
            "agent_settled" => Disposition::Record(EventKind::Done, "turn settled".to_string()),

            // Everything else is transport: per-token deltas, the echo of our own prompt,
            // and the start/stop bookends that carry nothing the pair above does not.
            _ => Disposition::Ignore,
        }
    }
}

/// A summary line: first sentence-ish, on one line, bounded.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(160).collect();
    if flat.chars().count() > 160 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> PiEvent {
        PiEvent::parse(line).expect("a valid stream line")
    }

    #[test]
    fn a_blank_or_non_json_line_is_not_an_event() {
        // Pi's stdout carries npm notices and the occasional warning; a parser that
        // treats those as a malformed stream turns a working turn into a failed one.
        assert!(PiEvent::parse("").is_none());
        assert!(PiEvent::parse("   ").is_none());
        assert!(PiEvent::parse("npm notice run pi").is_none());
    }

    #[test]
    fn the_session_line_carries_the_id_a_later_turn_resumes() {
        let e = event(r#"{"type":"session","version":3,"id":"01a0-bd7f","cwd":"/w"}"#);
        assert_eq!(e.session_id(), Some("01a0-bd7f"));
        // Only the session line claims to be one.
        assert_eq!(event(r#"{"type":"turn_start"}"#).session_id(), None);
    }

    #[test]
    fn a_turn_ends_at_settled_not_at_agent_end() {
        // `agent_end` carries `willRetry`, so a retrying turn emits it and carries on.
        // Stopping there records the first attempt as the whole turn.
        assert!(!event(r#"{"type":"agent_end","willRetry":true}"#).is_terminal());
        assert!(!event(r#"{"type":"agent_end","willRetry":false}"#).is_terminal());
        assert!(event(r#"{"type":"agent_settled"}"#).is_terminal());
    }

    #[test]
    fn a_settled_provider_error_is_still_a_failed_turn() {
        let failed = event(
            r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"error",
                "errorMessage":"Claude Code returned an error result: Not logged in"}}"#,
        );
        let recovered =
            event(r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"stop"}}"#);

        assert_eq!(failed.provider_turn_failed(), Some(true));
        assert_eq!(recovered.provider_turn_failed(), Some(false));
        let settled = event(r#"{"type":"agent_settled"}"#);
        assert_eq!(settled.provider_turn_failed(), None);
        assert_eq!(
            settled.classify(),
            Disposition::Record(EventKind::Done, "turn settled".into())
        );
    }

    #[test]
    fn a_claude_steps_cache_figures_sit_beside_its_input() {
        // A real claude-subscription step, as Pi reported it. The cached prefix is not
        // inside `input`: reading `input` as the prompt said a seat 69K tokens into its
        // session held 1, and taking the cache out of it clamped every step's uncached
        // input to nothing.
        let e = event(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[],
                "usage":{"input":1,"output":319,"cacheRead":68881,"cacheWrite":257,
                         "totalTokens":69458}}}"#,
        );
        let usage = e.usage().expect("usage on turn_end");
        assert_eq!(usage.tokens_in, 1, "uncached input");
        assert_eq!(usage.tokens_out, 319);
        assert_eq!(usage.cache_read, 68_881);
        assert_eq!(usage.cache_write, 257);
        assert_eq!(e.context_tokens(), Some(69_139));
    }

    #[test]
    fn a_chatgpt_steps_cached_prompt_is_counted_once() {
        // A real openai-codex step: the same shape, so the same reading.
        let e = event(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[],
                "usage":{"input":688,"output":331,"cacheRead":49664,"cacheWrite":0,
                         "totalTokens":50683}}}"#,
        );
        let usage = e.usage().unwrap();
        assert_eq!(usage.tokens_in, 688);
        assert_eq!(usage.cache_read, 49_664);
        assert_eq!(e.context_tokens(), Some(50_352));
    }

    #[test]
    fn a_provider_that_counts_the_cache_inside_input_reads_the_same() {
        // `totalTokens` is the figure every provider sums alike, so the prompt is read
        // from it: were `input` ever a total with the cache inside, nothing is charged
        // twice.
        let e = event(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[],
                "usage":{"input":14068,"output":4,"cacheRead":9000,"cacheWrite":5000,
                         "totalTokens":14072}}}"#,
        );
        assert_eq!(e.usage().unwrap().tokens_in, 68);
        assert_eq!(e.context_tokens(), Some(14_068));
    }

    #[test]
    fn without_a_total_the_cache_is_added_to_input_as_pi_reports_it() {
        let e = event(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[],
                "usage":{"input":10,"output":1,"cacheRead":9000,"cacheWrite":5000}}}"#,
        );
        assert_eq!(e.usage().unwrap().tokens_in, 10);
        assert_eq!(e.context_tokens(), Some(14_010));
    }

    #[test]
    fn usage_that_would_go_negative_clamps_instead_of_wrapping() {
        // Subtraction on a stream we do not control: a total smaller than its parts must
        // not underflow into billions.
        let e = event(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[],
                "usage":{"input":10,"output":1,"cacheRead":9000,"cacheWrite":5000,
                         "totalTokens":11}}}"#,
        );
        assert_eq!(e.usage().unwrap().tokens_in, 0);
    }

    #[test]
    fn the_assistant_s_own_words_are_read_from_the_stream() {
        // Rule 8: a verdict is captured here, never by filtering Note rows back out of
        // the event table, where ai-team's own dispatch notices also live.
        let e = event(
            r#"{"type":"message_end","message":{"role":"assistant",
                "content":[{"type":"text","text":"VERDICT: pass"}]}}"#,
        );
        assert_eq!(e.assistant_message().as_deref(), Some("VERDICT: pass"));
    }

    #[test]
    fn the_prompt_we_sent_is_not_the_assistant_talking() {
        // Pi echoes the user turn as a message too. Reading it as the model's answer
        // would let a prompt containing "VERDICT: pass" verify itself.
        let e = event(
            r#"{"type":"message_end","message":{"role":"user",
                "content":[{"type":"text","text":"VERDICT: pass"}]}}"#,
        );
        assert_eq!(e.assistant_message(), None);
        assert_eq!(e.classify(), Disposition::Ignore);
    }

    #[test]
    fn a_failed_tool_call_is_recorded_as_failed() {
        // Pi marks failure on the result rather than as its own event type, so a result
        // nobody inspects reads as success - and a turn of refused calls looks clean.
        let ok =
            event(r#"{"type":"tool_execution_end","toolName":"bash","result":{"content":[]}}"#);
        assert_eq!(
            ok.classify(),
            Disposition::Record(EventKind::ToolResult, "bash returned".into())
        );

        let bad = event(
            r#"{"type":"tool_execution_end","toolName":"bash","result":{"isError":true,"content":[]}}"#,
        );
        assert_eq!(
            bad.classify(),
            Disposition::Record(EventKind::ToolResult, "bash failed".into())
        );
    }

    #[test]
    fn only_a_successful_planner_handoff_call_counts() {
        let written = event(
            r#"{"type":"tool_execution_end","toolName":"write_handoff","result":{"content":[]}}"#,
        );
        assert!(written.wrote_planner_handoff());
        let failed = event(
            r#"{"type":"tool_execution_end","toolName":"write_handoff","result":{"isError":true}}"#,
        );
        assert!(!failed.wrote_planner_handoff());
    }

    #[test]
    fn a_failed_required_context_tool_is_an_explicit_planning_stop() {
        let context = event(
            r#"{"type":"tool_execution_end","toolName":"clickup_get_task","result":{"isError":true}}"#,
        );
        assert_eq!(
            context.context_tool_failure().as_deref(),
            Some("CONTEXT_UNAVAILABLE: clickup_get_task failed")
        );
        let ordinary =
            event(r#"{"type":"tool_execution_end","toolName":"bash","result":{"isError":true}}"#);
        assert!(ordinary.context_tool_failure().is_none());
    }

    #[test]
    fn a_step_names_the_model_it_ran_on() {
        let e = event(
            r#"{"type":"turn_start","message":{"role":"assistant","model":"claude-sonnet-5"}}"#,
        );
        assert_eq!(
            e.classify(),
            Disposition::Record(EventKind::Step, "step on claude-sonnet-5".into())
        );
    }

    #[test]
    fn completed_thinking_is_one_live_row_without_token_delta_noise() {
        let e = event(
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_end","content":"Checking the failing test"}}"#,
        );
        assert_eq!(
            e.classify(),
            Disposition::Record(EventKind::Cost, "Checking the failing test".into())
        );
    }

    #[test]
    fn token_deltas_are_not_rows() {
        // 21 of the 37 events in a trivial turn are deltas. Recording them would make the
        // table unreadable and tell analytics nothing it cannot get from the totals.
        for kind in [
            "message_update",
            "message_start",
            "agent_start",
            "agent_end",
        ] {
            let e = event(&format!(r#"{{"type":"{kind}"}}"#));
            assert_eq!(e.classify(), Disposition::Ignore, "{kind}");
        }
    }

    #[test]
    fn a_long_answer_is_summarised_on_one_line() {
        let long = "a ".repeat(400);
        let e = event(&format!(
            r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"{long}"}}]}}}}"#
        ));
        let Disposition::Record(kind, summary) = e.classify() else {
            panic!("expected a note");
        };
        assert_eq!(kind, EventKind::Note);
        assert!(summary.chars().count() <= 161, "{}", summary.len());
        assert!(!summary.contains('\n'));
    }
}
