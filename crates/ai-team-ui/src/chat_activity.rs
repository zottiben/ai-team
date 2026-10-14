//! Small, allowlisted additions to the existing event feed. No raw arguments, result
//! objects, provider metadata, reasoning signatures or image data cross this boundary.
//! Missing evidence stays missing, including history recorded before host observations.

use ai_team_core::{Event, EventKind, PiEvent};
use serde::Serialize;
use serde_json::Value;

const TARGET_CHARS: usize = 1_024;
const OUTPUT_CHARS: usize = 4_000;
const OUTPUT_BLOCKS: usize = 64;

#[derive(Debug, Serialize)]
pub(crate) struct BoundedText {
    text: String,
    truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct ToolTarget {
    kind: &'static str,
    #[serde(flatten)]
    value: BoundedText,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Returned,
    Failed,
    Unknown,
}

#[derive(Debug, Serialize)]
pub(crate) struct ToolActivity {
    call_id: Option<String>,
    parent_call_id: Option<String>,
    name: Option<String>,
    target: Option<ToolTarget>,
    outcome: Option<Outcome>,
    /// Pi's explicit monotonic execute() duration, not a difference of batched dates.
    duration_ms: Option<f64>,
    /// Plain text only, and only for the native shell tools. Not an interactive PTY.
    output: Option<BoundedText>,
}

fn identity(value: Option<&Value>, bytes: usize) -> Option<String> {
    let text = value?.as_str()?;
    exact(text, bytes).map(str::to_owned)
}

fn exact(text: &str, bytes: usize) -> Option<&str> {
    // Never truncate identities: different calls with the same long prefix must not
    // become one call. A missing identity is an explicit unpaired row instead.
    (!text.trim().is_empty() && text.len() <= bytes && !text.chars().any(char::is_control))
        .then_some(text)
}

pub(crate) fn session(event: &Event) -> Option<String> {
    let (session, index) = event.stream_id.as_deref()?.rsplit_once(':')?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    index.parse::<u64>().ok()?;
    exact(session, 256).map(str::to_owned)
}

fn one_line(text: &str, limit: usize) -> BoundedText {
    let mut chars = text
        .split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| {
            std::iter::once(' ')
                .filter(move |_| index > 0)
                .chain(word.chars())
        })
        .filter(|ch| !ch.is_control());
    let value: String = chars.by_ref().take(limit).collect();
    BoundedText {
        text: value.trim_end().into(),
        truncated: chars.next().is_some(),
    }
}

fn target(data: &Value) -> Option<ToolTarget> {
    let args = data.get("args")?;
    ["path", "filePath", "command", "query", "pattern", "url"]
        .into_iter()
        .find_map(|key| {
            args.get(key)
                .and_then(Value::as_str)
                .map(|text| ToolTarget {
                    kind: if key == "filePath" { "path" } else { key },
                    value: one_line(text, TARGET_CHARS),
                })
        })
}

pub(crate) fn tool_summary(text: &str) -> String {
    let bounded = one_line(text, 240);
    if bounded.truncated {
        format!("{}…", bounded.text)
    } else {
        bounded.text
    }
}

pub(crate) fn target_summary(event: &Event) -> Option<String> {
    Some(
        target(event.payload.as_ref()?)?
            .value
            .text
            .chars()
            .take(240)
            .collect(),
    )
}

fn output(data: &Value) -> Option<BoundedText> {
    let blocks = data.pointer("/result/content")?.as_array()?;
    let mut remaining = OUTPUT_CHARS;
    let mut parts = Vec::new();
    let mut truncated = blocks.len() > OUTPUT_BLOCKS;
    for block in blocks.iter().rev().take(OUTPUT_BLOCKS) {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = block
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        if remaining <= usize::from(!parts.is_empty()) {
            truncated = true;
            break;
        }
        remaining -= usize::from(!parts.is_empty()); // newline between blocks
                                                     // Bound work as well as the response, including malicious control-only text.
        let mut from = text.len().saturating_sub((remaining + 1) * 4);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        truncated |= from > 0;
        let mut chars = text[from..]
            .chars()
            .rev()
            .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'));
        let mut tail: Vec<_> = chars.by_ref().take(remaining).collect();
        truncated |= chars.next().is_some();
        remaining -= tail.len();
        tail.reverse();
        parts.push(tail.into_iter().collect::<String>());
    }
    parts.reverse();
    let text = parts.join("\n");
    (!text.is_empty()).then_some(BoundedText { text, truncated })
}

pub(crate) fn tool(event: &Event) -> Option<ToolActivity> {
    if !matches!(event.kind, EventKind::ToolCall | EventKind::ToolResult) {
        return None;
    }
    let data = event.payload.as_ref()?;
    let end = event.kind == EventKind::ToolResult;
    let name = identity(data.get("toolName"), 128);
    let outcome = end.then(|| {
        if PiEvent::tool_result_failed(data) {
            Outcome::Failed
        } else if data.get("isError").and_then(Value::as_bool) == Some(false)
            || data.pointer("/result/isError").and_then(Value::as_bool) == Some(false)
        {
            Outcome::Returned
        } else {
            Outcome::Unknown
        }
    });
    let duration_ms = end
        .then(|| data.get("durationMs").and_then(Value::as_f64))
        .flatten()
        .filter(|duration| {
            duration.is_finite() && *duration >= 0.0 && *duration <= 9_007_199_254_740_991.0
        });
    let output = (end && matches!(name.as_deref(), Some("bash" | "powershell")))
        .then(|| output(data))
        .flatten();
    Some(ToolActivity {
        call_id: identity(data.get("toolCallId"), 256),
        parent_call_id: identity(data.get("parentToolCallId"), 256),
        name,
        target: target(data),
        outcome,
        duration_ms,
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn projected(kind: EventKind, data: Value) -> Value {
        serde_json::to_value(tool(&Event {
            id: 1,
            run_id: 1,
            node_run_id: Some(2),
            at: "coarse".into(),
            kind,
            actor: Some("agent".into()),
            summary: "tool".into(),
            payload: Some(data),
            stream_id: Some("session:4".into()),
            observation: None,
        }))
        .unwrap()
    }

    #[test]
    fn summary_bounds_do_not_invent_truncation_at_the_exact_limit() {
        assert_eq!(tool_summary(&"x".repeat(240)), "x".repeat(240));
        assert_eq!(
            tool_summary(&"x".repeat(241)),
            format!("{}…", "x".repeat(240))
        );
        assert_eq!(tool_summary(" read\n file\t"), "read file");
    }

    #[test]
    fn identities_are_never_clipped_or_inferred_from_names() {
        let value = projected(
            EventKind::ToolCall,
            json!({"toolName":"read", "toolCallId":"a".repeat(257),
            "parentToolCallId":"parent", "args":{"path":"🍋".repeat(2_000), "content":"private write"}}),
        );
        assert!(value["call_id"].is_null());
        assert_eq!(value["parent_call_id"], "parent");
        assert_eq!(
            value["target"]["text"].as_str().unwrap().chars().count(),
            TARGET_CHARS
        );
        assert_eq!(value["target"]["truncated"], true);
        assert!(value["outcome"].is_null());
        assert!(!value.to_string().contains("private write"));
    }

    #[test]
    fn output_is_a_bounded_plain_text_tail_not_an_arbitrary_result() {
        let value = projected(
            EventKind::ToolResult,
            json!({"toolName":"bash", "isError":false,
            "result":{"content":[{"type":"text", "text":format!("{}\nlast\u{0000}", "🍋".repeat(10_000))},
                {"type":"image", "data":"secret"}, {"type":"text", "text":"finish"}], "details":{"other":"secret"}}}),
        );
        let text = value["output"]["text"].as_str().unwrap();
        assert_eq!(text.chars().count(), OUTPUT_CHARS);
        assert!(text.ends_with("\nlast\nfinish"));
        assert!(!text.contains('\0'));
        assert_eq!(value["output"]["truncated"], true);
        assert!(!value.to_string().contains("secret"));
        assert_eq!(value["outcome"], "returned");
        let read = projected(
            EventKind::ToolResult,
            json!({"toolName":"read", "isError":false,
            "result":{"content":[{"type":"text", "text":"private source"}]}}),
        );
        assert!(read["output"].is_null());
    }

    #[test]
    fn result_bounds_also_cover_many_blocks_and_control_only_output() {
        let blocks: Vec<_> = (0..1_000)
            .map(|i| json!({"type":"text", "text":i.to_string()}))
            .collect();
        let value = projected(
            EventKind::ToolResult,
            json!({"toolName":"bash", "result":{"content":blocks}}),
        );
        assert_eq!(
            value["output"]["text"].as_str().unwrap().lines().count(),
            OUTPUT_BLOCKS
        );
        assert_eq!(value["output"]["truncated"], true);
        let value = projected(
            EventKind::ToolResult,
            json!({"toolName":"bash", "result":{"content":[
            {"type":"text", "text":"\u{0000}".repeat(100_000)}]}}),
        );
        assert!(value["output"].is_null());
    }

    #[test]
    fn only_explicit_duration_and_structured_outcomes_are_used() {
        for duration in [json!(-1), json!("2"), json!(null), json!(1e100)] {
            let value = projected(
                EventKind::ToolResult,
                json!({"toolName":"mcp", "durationMs":duration}),
            );
            assert!(value["duration_ms"].is_null());
            assert_eq!(value["outcome"], "unknown");
        }
        let failure = projected(
            EventKind::ToolResult,
            json!({"toolName":"mcp", "isError":false,
            "durationMs":0, "result":{"details":{"error":"adapter failed"}}}),
        );
        assert_eq!(failure["outcome"], "failed");
        assert_eq!(failure["duration_ms"], 0.0);
        let quoted = projected(
            EventKind::ToolResult,
            json!({"toolName":"bash", "isError":false,
            "result":{"content":[{"type":"text", "text":"example: Error: no"}]}}),
        );
        assert_eq!(quoted["outcome"], "returned");
    }
}
