//! Sample bounded native-shell tails before the ingest queue. Snapshots are not a full
//! terminal stream: no input route, no prompt parsing and no inferred command status.
use super::PiEvent;
use crate::ShellOutput;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct OutputSampler {
    recent: VecDeque<Instant>,
    calls: BTreeMap<String, Instant>,
}

fn identity(data: &Value, key: &str) -> Option<String> {
    let text = data.get(key)?.as_str()?;
    (!text.trim().is_empty() && text.len() <= 256 && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

fn tail(data: &Value) -> Option<(String, bool)> {
    let blocks = data.pointer("/partialResult/content")?.as_array()?;
    let mut remaining = 4_000usize;
    let mut parts = Vec::new();
    let mut truncated = blocks.len() > 64;
    for block in blocks.iter().rev().take(64) {
        if block["type"] != "text" {
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
        remaining -= usize::from(!parts.is_empty());
        let mut from = text.len().saturating_sub((remaining + 1) * 4);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        truncated |= from > 0;
        let mut chars = text[from..]
            .chars()
            .rev()
            .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'));
        let mut value: Vec<_> = chars.by_ref().take(remaining).collect();
        truncated |= chars.next().is_some();
        remaining -= value.len();
        value.reverse();
        parts.push(value.into_iter().collect::<String>());
    }
    parts.reverse();
    let text = parts.join("\n");
    (!text.is_empty()).then_some((text, truncated))
}

impl OutputSampler {
    /// At most one snapshot per call per second and four across the stream per second.
    /// The identity cache is bounded too. Completion remains the ordinary tool receipt.
    pub(super) fn observe(&mut self, event: &mut PiEvent, now: Instant) {
        if event.kind != "tool_execution_update"
            || !matches!(event.data["toolName"].as_str(), Some("bash" | "powershell"))
        {
            return;
        }
        let Some(call_id) = identity(&event.data, "toolCallId") else {
            return;
        };
        let elapsed = |then: &Instant| now.checked_duration_since(*then).unwrap_or_default();
        self.recent
            .retain(|then| elapsed(then) < Duration::from_secs(1));
        if self.recent.len() >= 4
            || self
                .calls
                .get(&call_id)
                .is_some_and(|then| elapsed(then) < Duration::from_secs(1))
        {
            return;
        }
        let Some((text, truncated)) = tail(&event.data) else {
            return;
        };
        if self.calls.len() >= 128 && !self.calls.contains_key(&call_id) {
            if let Some(oldest) = self
                .calls
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone())
            {
                self.calls.remove(&oldest);
            }
        }
        self.calls.insert(call_id.clone(), now);
        self.recent.push_back(now);
        event.shell_output = Some(ShellOutput {
            call_id,
            parent_call_id: identity(&event.data, "parentToolCallId"),
            tool_name: event.data["toolName"].as_str().unwrap_or_default().into(),
            text,
            truncated,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::event::Disposition;
    use serde_json::json;

    fn update(id: &str, text: &str) -> PiEvent {
        PiEvent::parse(&json!({"type":"tool_execution_update","toolName":"bash","toolCallId":id,
            "usage":{"input":99,"output":99}, "args":{"private":"secret"},
            "partialResult":{"content":[{"type":"text","text":text},{"type":"image","data":"secret"}]},
            "shell_output":{"call_id":"forged"}}).to_string()).unwrap()
    }

    #[test]
    fn raw_updates_cannot_forge_host_snapshots_or_usage_and_sampling_is_bounded() {
        let now = Instant::now();
        let mut sampler = OutputSampler::default();
        let mut first = update("a", "hello\0");
        assert!(first.shell_output.is_none());
        assert_eq!(first.classify(), Disposition::Ignore);
        sampler.observe(&mut first, now);
        assert_eq!(first.shell_output.as_ref().unwrap().text, "hello");
        assert!(first.usage().is_none());
        let mut repeat = update("a", "later");
        sampler.observe(&mut repeat, now);
        assert!(repeat.shell_output.is_none());
        for id in ["b", "c", "d"] {
            sampler.observe(&mut update(id, "x"), now);
        }
        let mut overflow = update("e", "later");
        sampler.observe(&mut overflow, now);
        assert!(overflow.shell_output.is_none());
        sampler.observe(&mut overflow, now + Duration::from_secs(1));
        assert!(overflow.shell_output.is_some());
        for i in 2..200 {
            sampler.observe(
                &mut update(&i.to_string(), "x"),
                now + Duration::from_secs(i),
            );
        }
        assert!(sampler.calls.len() <= 128);
        assert!(sampler.recent.len() <= 4);
    }

    #[test]
    fn only_native_shell_text_and_exact_bounded_identities_are_retained() {
        let now = Instant::now();
        let mut sampler = OutputSampler::default();
        let mut event = update("a", &"🍋".repeat(20_000));
        sampler.observe(&mut event, now);
        let receipt = event.shell_output.unwrap();
        assert_eq!(receipt.text.chars().count(), 4_000);
        assert!(receipt.truncated);
        assert!(!serde_json::to_string(&receipt).unwrap().contains("secret"));
        for name in ["read", "mcp__bash", "unknown"] {
            let mut event = update("b", "private source");
            event.data["toolName"] = json!(name);
            sampler.observe(&mut event, now);
            assert!(event.shell_output.is_none());
        }
        for id in [String::new(), "x".repeat(257), "bad\nidentity".into()] {
            let mut event = update(&id, "output");
            sampler.observe(&mut event, now);
            assert!(event.shell_output.is_none());
        }
    }
}
