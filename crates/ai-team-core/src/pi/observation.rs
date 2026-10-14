//! Receive-time evidence, before batching or callbacks can delay persistence. This is
//! not tool execution time: pipes can buffer. A fresh clock per drive also prevents a
//! resumed process from being subtracted from its predecessor's monotonic offset.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::event::{Disposition, PiEvent};
use crate::EventObservation;

pub(super) struct ObservationClock {
    id: String,
    started: Instant,
}

impl ObservationClock {
    pub(super) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            id: format!(
                "{}:{}:{}",
                std::process::id(),
                OffsetDateTime::now_utc().unix_timestamp_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            started: Instant::now(),
        }
    }

    pub(super) fn observe(&self, event: &mut PiEvent) {
        if matches!(event.classify(), Disposition::Ignore) {
            return;
        }
        event.observation = self.sample();
    }

    fn sample(&self) -> Option<EventObservation> {
        let now = OffsetDateTime::now_utc();
        Some(EventObservation {
            clock_id: self.id.clone(),
            elapsed_ms: self.started.elapsed().as_millis().try_into().ok()?,
            at: now
                .replace_nanosecond(now.nanosecond() / 1_000_000 * 1_000_000)
                .ok()?
                .format(&Rfc3339)
                .ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_are_host_owned_and_clocks_cannot_cross_a_restart() {
        let mut event = PiEvent::parse(r#"{"type":"tool_execution_start","toolCallId":"c","toolName":"read","observation":{"clock_id":"forged","elapsed_ms":99,"at":"forged"}}"#).unwrap();
        assert!(event.observation.is_none());
        assert_eq!(
            event.data["observation"]["clock_id"], "forged",
            "the original wire evidence is retained separately"
        );
        let clock = ObservationClock::new();
        clock.observe(&mut event);
        let first = event.observation.clone().unwrap();
        assert_eq!(
            serde_json::to_value(&event).unwrap()["observation"]["clock_id"],
            "forged",
            "host receipts never serialize into provider payloads"
        );
        let at = OffsetDateTime::parse(&first.at, &Rfc3339).unwrap();
        assert_eq!(at.nanosecond() % 1_000_000, 0);
        clock.observe(&mut event);
        let next = event.observation.as_ref().unwrap();
        assert_eq!(next.clock_id, first.clock_id);
        assert!(next.elapsed_ms >= first.elapsed_ms);
        ObservationClock::new().observe(&mut event);
        assert_ne!(event.observation.unwrap().clock_id, first.clock_id);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_real_pipe_reader_observes_events_before_callbacks_and_batching() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("offline-pi");
        std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"tool_execution_start\",\"toolName\":\"read\",\"toolCallId\":\"c\"}'\nwhile [ ! -f released ]; do sleep 0.01; done\nsleep 0.025\nprintf '%s\\n' '{\"type\":\"tool_execution_end\",\"toolName\":\"read\",\"toolCallId\":\"c\",\"isError\":false}' '{\"type\":\"agent_settled\"}'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut turn = crate::PiTurn::new(dir.path(), "offline fixture");
        turn.program = Some(script);
        let mut process = crate::PiProcess::start(&turn).unwrap();
        let mut observations = Vec::new();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            process.drive(|event| {
                observations.push(
                    event
                        .observation
                        .clone()
                        .expect("recorded event has a host receipt"),
                );
                if event.kind == "tool_execution_start" {
                    std::fs::write(dir.path().join("released"), "yes").unwrap();
                }
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(outcome.settled && !outcome.failed);
        assert_eq!(observations.len(), 3);
        assert_eq!(observations[0].clock_id, observations[1].clock_id);
        assert!(observations[1].elapsed_ms > observations[0].elapsed_ms);
    }

    #[test]
    fn ignored_token_deltas_do_not_allocate_receipts() {
        let mut event = PiEvent::parse(r#"{"type":"message_update","delta":"a"}"#).unwrap();
        ObservationClock::new().observe(&mut event);
        assert!(event.observation.is_none());
    }
}
