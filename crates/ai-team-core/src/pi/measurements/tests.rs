use super::*;
use crate::pi::event::Disposition;
use serde_json::json;

fn observed(sampler: &mut LiveSampler, now: Instant, value: Value) -> PiEvent {
    let mut event = PiEvent::parse(&value.to_string()).unwrap();
    assert!(event.telemetry.is_none());
    sampler.observe(&mut event, now);
    super::super::observation::ObservationClock::new().observe(&mut event);
    event
}

fn usage(output: u64) -> Value {
    json!({"input":10,"output":output,"cacheRead":20,"cacheWrite":5,"totalTokens":35 + output})
}

#[test]
fn actual_json_updates_are_bounded_cumulative_observations_not_spend() {
    let mut sampler = LiveSampler::default();
    let now = Instant::now();
    let zero = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0});
    let start = observed(
        &mut sampler,
        now,
        json!({"type":"message_start","message":{"role":"assistant","usage":zero}}),
    );
    assert!(start.telemetry.is_none() && start.observation.is_none());
    let first = observed(
        &mut sampler,
        now,
        json!({"type":"message_update","usage":usage(2),"telemetry":{"kind":"forged"},"private":"private delta"}),
    );
    assert!(first.usage().is_none() && first.context_tokens().is_none());
    assert!(first.telemetry_only() && first.observation.is_some());
    assert!(matches!(
        first.telemetry,
        Some(Telemetry::Usage {
            sequence: Some(1),
            phase: UsagePhase::Live,
            ..
        })
    ));
    assert_eq!(
        observed(
            &mut sampler,
            now + Duration::from_millis(999),
            json!({"type":"message_update","usage":usage(3)})
        )
        .classify(),
        Disposition::Ignore
    );
    let same = observed(
        &mut sampler,
        now + Duration::from_secs(1),
        json!({"type":"message_update","usage":usage(2)}),
    );
    assert!(same.telemetry.is_none());
    let end = observed(
        &mut sampler,
        now,
        json!({"type":"message_end","message":{"role":"assistant","usage":usage(4),"content":[{"type":"text","text":"answer"}]}}),
    );
    assert!(!end.telemetry_only());
    assert!(end.usage().is_none());
    let step = observed(
        &mut sampler,
        now,
        json!({"type":"turn_end","message":{"role":"assistant","usage":usage(4)}}),
    );
    assert_eq!(step.usage().unwrap().tokens_out, 4);
    assert!(matches!(
        step.telemetry,
        Some(Telemetry::Usage {
            sequence: Some(1),
            phase: UsagePhase::Step,
            ..
        })
    ));
    let zero_step = observed(
        &mut sampler,
        now,
        json!({"type":"turn_end","message":{"role":"assistant","usage":zero}}),
    );
    assert!(matches!(
        zero_step.telemetry,
        Some(Telemetry::Usage {
            sequence: None,
            tokens: TokenObservation {
                output: Some(0),
                ..
            },
            ..
        })
    ));
}

#[test]
fn scope_requires_a_real_assistant_start_and_never_borrows_a_previous_message() {
    let mut sampler = LiveSampler::default();
    let now = Instant::now();
    for index in 0..3 {
        let at = now + Duration::from_secs(index * 2);
        let orphan = observed(
            &mut sampler,
            at,
            json!({"type":"message_update","usage":usage(index + 1)}),
        );
        assert!(matches!(
            orphan.telemetry,
            Some(Telemetry::Usage { sequence: None, .. })
        ));
        observed(
            &mut sampler,
            at,
            json!({"type":"message_start","message":{"role":"assistant"}}),
        );
        let end = observed(
            &mut sampler,
            at,
            json!({"type":"message_end","message":{"role":"assistant","usage":usage(7)}}),
        );
        assert!(
            matches!(end.telemetry, Some(Telemetry::Usage { sequence: Some(n), .. }) if n == index + 1)
        );
    }
    let mut restarted = LiveSampler::default();
    let orphan = observed(
        &mut restarted,
        now,
        json!({"type":"message_update","usage":usage(1)}),
    );
    assert!(matches!(
        orphan.telemetry,
        Some(Telemetry::Usage { sequence: None, .. })
    ));
}

#[test]
fn counts_keep_missing_invalid_and_inconsistent_context_unknown() {
    let read = TokenObservation::read;
    assert_eq!(
        read(&usage(4)),
        TokenObservation {
            input: Some(10),
            output: Some(4),
            cache_read: Some(20),
            cache_write: Some(5),
            context: Some(35),
            context_source: Some(ContextSource::ReportedTotal)
        }
    );
    assert_eq!(
        read(&json!({"input":10,"cacheRead":20,"cacheWrite":5})).context,
        Some(35)
    );
    assert_eq!(
        read(&json!({"totalTokens":39,"output":4})).context,
        Some(35)
    );
    for value in [
        json!({"input":10}),
        json!({"totalTokens":3,"output":4}),
        json!({"input":10,"cacheRead":20,"cacheWrite":5,"totalTokens":9,"output":4}),
    ] {
        assert_eq!(read(&value).context, None);
    }
    for invalid in [
        json!(-1),
        json!(2.5),
        json!(SAFE + 1),
        json!("12"),
        Value::Null,
    ] {
        let counts = read(
            &json!({"input":invalid,"output":invalid,"cacheRead":invalid,"cacheWrite":invalid}),
        );
        assert!(!counts.useful(UsagePhase::Step));
    }
    assert_eq!(
        read(&json!({"input":SAFE,"cacheRead":1,"cacheWrite":0})).context,
        None
    );
}

#[test]
fn compaction_is_explicit_bounded_and_post_context_stays_estimated() {
    let mut sampler = LiveSampler::default();
    let now = Instant::now();
    let start = observed(
        &mut sampler,
        now,
        json!({"type":"compaction_start","reason":"threshold"}),
    );
    assert!(start.telemetry_only());
    let end = observed(
        &mut sampler,
        now,
        json!({"type":"compaction_end","reason":"threshold","aborted":false,"willRetry":false,
        "result":{"tokensBefore":1234,"estimatedTokensAfter":456,"summary":"private","details":{"secret":true},"firstKeptEntryId":"private"}}),
    );
    assert_eq!(
        end.telemetry,
        Some(Telemetry::Compaction {
            phase: CompactionPhase::Completed,
            sequence: Some(1),
            reason: Some(CompactionReason::Threshold),
            tokens_before: Some(1234),
            estimated_tokens_after: Some(456)
        })
    );
    assert!(!serde_json::to_string(&end.telemetry)
        .unwrap()
        .contains("private"));
    assert!(end.usage().is_none());
    for (flags, phase) in [
        (json!({"aborted":true}), CompactionPhase::Aborted),
        (json!({"willRetry":true}), CompactionPhase::Retrying),
        (
            json!({"errorMessage":"private error"}),
            CompactionPhase::Failed,
        ),
        (json!({}), CompactionPhase::Unknown),
    ] {
        let mut value = flags;
        value["type"] = json!("compaction_end");
        value["reason"] = json!("private reason");
        let event = observed(&mut sampler, now, value);
        assert_eq!(
            event.telemetry,
            Some(Telemetry::Compaction {
                phase,
                sequence: None,
                reason: None,
                tokens_before: None,
                estimated_tokens_after: None
            })
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_real_reader_captures_json_usage_and_compaction_on_one_host_clock() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("offline-pi");
    std::fs::write(&script, concat!("#!/bin/sh\nprintf '%s\\n' ",
        "'{\"type\":\"message_start\",\"message\":{\"role\":\"assistant\"}}' ",
        "'{\"type\":\"message_update\",\"usage\":{\"output\":2},\"private\":\"not exported\"}' ",
        "'{\"type\":\"message_update\",\"usage\":{\"output\":3}}' ",
        "'{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"usage\":{\"output\":4}}}' ",
        "'{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"usage\":{\"output\":4}}}' ",
        "'{\"type\":\"compaction_start\",\"reason\":\"manual\"}' ",
        "'{\"type\":\"compaction_end\",\"reason\":\"manual\",\"aborted\":true}' ",
        "'{\"type\":\"agent_settled\"}'\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut turn = crate::PiTurn::new(dir.path(), "offline telemetry fixture");
    turn.program = Some(script);
    let mut process = crate::PiProcess::start(&turn).unwrap();
    let mut samples = Vec::new();
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        process.drive(|event| {
            if event.telemetry.is_some() {
                samples.push(event.clone());
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(outcome.settled && !outcome.failed);
    assert_eq!(outcome.usage.unwrap().tokens_out, 4);
    assert_eq!(samples.len(), 5);
    let clock = &samples[0].observation.as_ref().unwrap().clock_id;
    assert!(samples
        .iter()
        .all(|event| &event.observation.as_ref().unwrap().clock_id == clock));
    assert!(samples
        .windows(2)
        .all(|pair| pair[0].observation.as_ref().unwrap().elapsed_ms
            <= pair[1].observation.as_ref().unwrap().elapsed_ms));
}

#[test]
fn overlapping_compaction_starts_do_not_pair_an_arbitrary_end() {
    let mut sampler = LiveSampler::default();
    let now = Instant::now();
    for _ in 0..3 {
        observed(
            &mut sampler,
            now,
            json!({"type":"compaction_start","reason":"manual"}),
        );
    }
    let end = observed(
        &mut sampler,
        now,
        json!({"type":"compaction_end","reason":"manual"}),
    );
    assert!(matches!(
        end.telemetry,
        Some(Telemetry::Compaction { sequence: None, .. })
    ));
}
