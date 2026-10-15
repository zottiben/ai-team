//! Sample cumulative transport counters, never sum message updates as spend. JSON mode
//! puts message_update.usage at the root and removes the growing partial message.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::PiEvent;
use crate::telemetry::{
    CompactionPhase, CompactionReason, ContextSource, Telemetry, TokenObservation, UsagePhase,
};

const SAFE: u64 = 9_007_199_254_740_991;

#[derive(Default)]
pub(super) struct LiveSampler {
    next_message: u64,
    message: Option<u64>,
    ended: Option<u64>,
    last_sample: Option<Instant>,
    last_tokens: Option<TokenObservation>,
    next_compaction: u64,
    compaction: Option<(u64, Option<CompactionReason>)>,
    compaction_open: bool,
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n <= SAFE)
}

fn increment(counter: &mut u64) -> Option<u64> {
    *counter = counter.saturating_add(1);
    (*counter <= SAFE).then_some(*counter)
}

impl TokenObservation {
    fn read(value: &Value) -> Self {
        let input = number(&value["input"]);
        let output = number(&value["output"]);
        let cache_read = number(&value["cacheRead"]);
        let cache_write = number(&value["cacheWrite"]);
        let parts = input
            .zip(cache_read)
            .zip(cache_write)
            .and_then(|((a, b), c)| a.checked_add(b)?.checked_add(c))
            .filter(|n| *n <= SAFE);
        let reported = number(&value["totalTokens"])
            .zip(output)
            .and_then(|(total, out)| total.checked_sub(out));
        let (context, context_source) = match (reported, parts) {
            (Some(total), Some(parts)) if total != parts => (None, None),
            (Some(total), _) => (Some(total), Some(ContextSource::ReportedTotal)),
            (None, Some(parts)) if value.get("totalTokens").is_none() => {
                (Some(parts), Some(ContextSource::CompleteParts))
            }
            _ => (None, None),
        };
        Self {
            input,
            output,
            cache_read,
            cache_write,
            context,
            context_source,
        }
    }

    fn useful(&self, phase: UsagePhase) -> bool {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.context,
        ]
        .into_iter()
        .flatten()
        .any(|value| phase != UsagePhase::Live || value > 0)
    }
}

impl LiveSampler {
    pub(super) fn observe(&mut self, event: &mut PiEvent, now: Instant) {
        // These fields are host-only even when an extension prints a forged JSON key.
        event.telemetry = None;
        if matches!(event.kind.as_str(), "session" | "agent_settled") {
            self.compaction = None;
            self.compaction_open = false;
        }
        match event.kind.as_str() {
            "session" | "turn_start" | "agent_settled" => {
                self.message = None;
                self.ended = None;
                self.last_tokens = None;
            }
            "message_start" => {
                self.message = None;
                let role = event.data.pointer("/message/role").and_then(Value::as_str);
                if role == Some("user") {
                    self.ended = None;
                }
                if role == Some("assistant") {
                    self.ended = None;
                    self.message = increment(&mut self.next_message);
                    self.last_tokens = None;
                    self.usage(event, now, UsagePhase::Live);
                }
            }
            "message_update" => self.usage(event, now, UsagePhase::Live),
            "message_end"
                if event.data.pointer("/message/role").and_then(Value::as_str)
                    == Some("assistant") =>
            {
                self.usage(event, now, UsagePhase::MessageEnd);
                self.ended = self.message.take();
            }
            "turn_end" => {
                self.usage(event, now, UsagePhase::Step);
                self.message = None;
                self.ended = None;
                self.last_tokens = None;
            }
            "compaction_start" | "compaction_end" => self.compact(event),
            _ => {}
        }
    }

    fn usage(&mut self, event: &mut PiEvent, now: Instant, phase: UsagePhase) {
        if event
            .data
            .pointer("/message/role")
            .and_then(Value::as_str)
            .is_some_and(|role| role != "assistant")
        {
            return;
        }
        let Some(value) = event
            .data
            .get("usage")
            .or_else(|| event.data.pointer("/message/usage"))
        else {
            return;
        };
        let tokens = TokenObservation::read(value);
        if !tokens.useful(phase) {
            return;
        }
        if phase == UsagePhase::Live {
            if self.last_tokens.as_ref() == Some(&tokens)
                || self.last_sample.is_some_and(|last| {
                    now.saturating_duration_since(last) < Duration::from_secs(1)
                })
            {
                return;
            }
            self.last_sample = Some(now);
            self.last_tokens = Some(tokens.clone());
        }
        event.telemetry = Some(Telemetry::Usage {
            phase,
            sequence: if phase == UsagePhase::Step {
                self.ended.or(self.message)
            } else {
                self.message
            },
            tokens,
        });
    }

    fn compact(&mut self, event: &mut PiEvent) {
        let reason = match event.data["reason"].as_str() {
            Some("manual") => Some(CompactionReason::Manual),
            Some("threshold") => Some(CompactionReason::Threshold),
            Some("overflow") => Some(CompactionReason::Overflow),
            _ => None,
        };
        let start = event.kind == "compaction_start";
        let sequence = if start {
            let next = increment(&mut self.next_compaction);
            if std::mem::replace(&mut self.compaction_open, true) {
                self.compaction = None;
                None
            } else {
                self.compaction = next.map(|n| (n, reason));
                next
            }
        } else {
            self.compaction_open = false;
            self.compaction
                .take()
                .filter(|(_, began)| *began == reason)
                .map(|(n, _)| n)
        };
        let phase = if start {
            CompactionPhase::Started
        } else if event.data["aborted"] == true {
            CompactionPhase::Aborted
        } else if event.data["willRetry"] == true {
            CompactionPhase::Retrying
        } else if event.data["errorMessage"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        {
            CompactionPhase::Failed
        } else if event.data["aborted"] == false
            && event.data["willRetry"] == false
            && event.data["result"].is_object()
        {
            CompactionPhase::Completed
        } else {
            CompactionPhase::Unknown
        };
        event.telemetry = Some(Telemetry::Compaction {
            phase,
            sequence,
            reason,
            tokens_before: (!start)
                .then(|| number(&event.data["result"]["tokensBefore"]))
                .flatten(),
            estimated_tokens_after: (!start)
                .then(|| number(&event.data["result"]["estimatedTokensAfter"]))
                .flatten(),
        });
    }
}

#[cfg(test)]
mod tests;
