//! A bounded, replaceable text preview, not another copy of the append-only transcript.

use std::collections::BTreeMap;

use super::PiEvent;

const LIMIT: usize = 64_000;

#[derive(Default)]
pub(super) struct LiveText {
    blocks: BTreeMap<u64, String>,
}

impl LiveText {
    pub(super) fn accept(&mut self, event: &PiEvent) {
        if matches!(event.kind.as_str(), "message_start" | "message_end")
            && event
                .data
                .pointer("/message/role")
                .and_then(|role| role.as_str())
                == Some("assistant")
        {
            self.blocks.clear();
        }
        if event.kind != "message_update" {
            return;
        }
        let Some(update) = event.data.get("assistantMessageEvent") else {
            return;
        };
        let Some(index) = update
            .get("contentIndex")
            .and_then(serde_json::Value::as_u64)
        else {
            return;
        };
        if index >= 64 {
            return;
        }
        let kind = update.get("type").and_then(|kind| kind.as_str());
        match kind {
            Some("text_delta") => {
                let used: usize = self.blocks.values().map(String::len).sum();
                if let Some(delta) = update.get("delta").and_then(|text| text.as_str()) {
                    self.blocks
                        .entry(index)
                        .or_default()
                        .push_str(bounded(delta, LIMIT.saturating_sub(used)));
                }
            }
            Some("text_end") => {
                if let Some(text) = update.get("content").and_then(|text| text.as_str()) {
                    let used: usize = self
                        .blocks
                        .iter()
                        .filter(|(key, _)| **key != index)
                        .map(|(_, text)| text.len())
                        .sum();
                    self.blocks
                        .insert(index, bounded(text, LIMIT.saturating_sub(used)).to_string());
                }
            }
            _ => {}
        }
    }

    pub(super) fn text(&self) -> String {
        self.blocks.values().cloned().collect::<Vec<_>>().join("\n")
    }
}

fn bounded(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_are_replaced_by_authoritative_blocks_then_the_completed_message() {
        let mut live = LiveText::default();
        for line in [
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hel"}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"lo"}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":1,"delta":"not display text"}}"#,
        ] {
            live.accept(&PiEvent::parse(line).unwrap());
        }
        assert_eq!(live.text(), "Hello");
        live.accept(&PiEvent::parse(r#"{"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"Hello!"}}"#).unwrap());
        assert_eq!(live.text(), "Hello!");
        live.accept(
            &PiEvent::parse(
                r#"{"type":"message_end","message":{"role":"assistant","content":[]}}"#,
            )
            .unwrap(),
        );
        assert_eq!(live.text(), "");
        assert_eq!(bounded("a🙂", 3), "a");
    }
}
