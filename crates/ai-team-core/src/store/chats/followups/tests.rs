use super::*;
use crate::{ChatMode, PiEvent};

#[test]
fn queue_is_exact_idempotent_and_not_a_legacy_seat_message() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let registry = ModelRegistry::local_only();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    assert!(store
        .queue_chat_followup(chat.id, turn.node_id + 1, "next", "q", FollowupKind::Steer)
        .is_err());
    let q = store
        .queue_chat_followup(chat.id, turn.node_id, " next ", "q", FollowupKind::FollowUp)
        .unwrap();
    assert_eq!(q.state, "queued");
    assert!(q.node_id.is_none());
    assert!(!store.chat(chat.id).unwrap().stop_requested);
    assert_eq!(
        store
            .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::FollowUp)
            .unwrap()
            .id,
        q.id
    );
    assert!(store
        .queue_chat_followup(
            chat.id,
            turn.node_id,
            "changed",
            "q",
            FollowupKind::FollowUp
        )
        .is_err());
    assert!(store
        .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::Steer)
        .is_err());
    assert!(store
        .queue_chat_followup(
            chat.id,
            turn.node_id,
            "another",
            "q2",
            FollowupKind::FollowUp
        )
        .is_err());
    assert!(store.send_chat_followup(chat.id, q.id, &registry).is_err());
    assert!(store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .is_none());
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1);
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    // An ordinary message, mode switch or archive must not jump ahead of this queue.
    assert!(store
        .begin_chat_turn(chat.id, "jump", "jump", &registry)
        .is_err());
    assert!(store.archive_chat(chat.id, true).is_err());
    assert!(store
        .set_chat_mode(chat.id, ChatMode::Team, store.chat(chat.id).unwrap().rev)
        .is_err());
    let next = store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .unwrap();
    assert!(next.started);
    assert_ne!(next.node_id, turn.node_id);
    assert_eq!(
        store.chat_followup(chat.id, q.id).unwrap().state,
        "starting"
    );
    // Lost HTTP response cannot dispatch a second process or re-stop a newer turn.
    assert!(
        !store
            .send_chat_followup(chat.id, q.id, &registry)
            .unwrap()
            .started
    );
    assert!(store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::FollowUp)
            .unwrap()
            .id,
        q.id
    );
    assert!(!store.chat(chat.id).unwrap().stop_requested);
}

#[test]
fn steer_waits_for_draining_and_explicit_stop_withdraws_it() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let registry = ModelRegistry::local_only();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    let q = store
        .queue_chat_followup(chat.id, turn.node_id, "steer", "q", FollowupKind::Steer)
        .unwrap();
    assert!(store.chat(chat.id).unwrap().stop_requested);
    assert!(store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .is_none());
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Cancelled, None)
        .unwrap();
    let next = store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .unwrap();
    assert_eq!(
        store.chat_followup(chat.id, q.id).unwrap().node_id,
        Some(next.node_id)
    );
    let stopped = store
        .queue_chat_followup(
            chat.id,
            next.node_id,
            "must not run",
            "q2",
            FollowupKind::Steer,
        )
        .unwrap();
    store.request_chat_stop(chat.id, next.node_id).unwrap();
    assert!(store
        .chat_events(chat.id, 0, 500)
        .unwrap()
        .iter()
        .any(|event| event.summary
            == "Queued instruction cancelled by Stop turn; it will not be sent."));
    assert_eq!(
        store.chat_followup(chat.id, stopped.id).unwrap().state,
        "cancelled"
    );
    store
        .finish_chat_turn(chat.id, next.node_id, NodeStatus::Cancelled, None)
        .unwrap();
    assert!(store
        .advance_chat_followup(chat.id, next.node_id, &registry)
        .unwrap()
        .is_none());
    assert!(store
        .send_chat_followup(chat.id, stopped.id, &registry)
        .is_err());
}

#[test]
fn failure_holds_the_queue_across_restart_for_explicit_send_or_cancel() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let registry = ModelRegistry::local_only();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    let q = store
        .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::FollowUp)
        .unwrap();
    store
        .finish_chat_turn(
            chat.id,
            turn.node_id,
            NodeStatus::Failed,
            Some("fixture failure"),
        )
        .unwrap();
    assert!(store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .is_none());
    let mut reopened = Store::open(store.path()).unwrap();
    assert_eq!(
        reopened.chat_followup(chat.id, q.id).unwrap().state,
        "queued"
    );
    let next = reopened
        .send_chat_followup(chat.id, q.id, &registry)
        .unwrap();
    assert!(next.started);
    assert!(
        !store
            .send_chat_followup(chat.id, q.id, &registry)
            .unwrap()
            .started
    );
    assert!(store.cancel_chat_followup(chat.id, q.id).is_err());
}

#[test]
fn delivery_needs_the_exact_user_echo_not_success_prose_or_another_turn() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let registry = ModelRegistry::local_only();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    let q = store
        .queue_chat_followup(chat.id, first.node_id, "next", "q", FollowupKind::FollowUp)
        .unwrap();
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let next = store.send_chat_followup(chat.id, q.id, &registry).unwrap();
    store
        .prepare_chat_followup_prompt(next.node_id, "rules\nnext")
        .unwrap();
    let event = |role, text| {
        PiEvent::parse(&serde_json::json!({"type":"message_end","message":{"role":role,"content":[{"type":"text","text":text}]}}).to_string()).unwrap()
    };
    for (node, role, text) in [
        (next.node_id, "assistant", "rules\nnext"),
        (next.node_id, "user", "next"),
        (first.node_id, "user", "rules\nnext"),
    ] {
        store
            .ingest_pi_events(node, "fixture", 0, &[event(role, text)])
            .unwrap();
        assert_eq!(
            store.chat_followup(chat.id, q.id).unwrap().state,
            "starting"
        );
    }
    store
        .ingest_pi_events(
            next.node_id,
            "next-session",
            0,
            &[event("user", "rules\nnext")],
        )
        .unwrap();
    let delivered = store.chat_followup(chat.id, q.id).unwrap();
    assert_eq!(delivered.state, "delivered");
    assert!(delivered.delivered_at.is_some());
    let count = store.chat_events(chat.id, 0, 500).unwrap().len();
    store
        .ingest_pi_events(
            next.node_id,
            "next-session",
            0,
            &[event("user", "rules\nnext")],
        )
        .unwrap();
    assert_eq!(store.chat_events(chat.id, 0, 500).unwrap().len(), count);
}

#[test]
fn recovery_does_not_disarm_an_already_requested_steer() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let registry = ModelRegistry::local_only();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    store
        .queue_chat_followup(chat.id, turn.node_id, "instead", "q", FollowupKind::Steer)
        .unwrap();
    // The supervisor died before it could drain/settle the predecessor.
    store
        .db_mut()
        .write(|tx| {
            tx.execute(
                "UPDATE node_run SET supervisor_pid=NULL,pi_pid=NULL WHERE id=?1",
                [turn.node_id],
            )?;
            Ok(())
        })
        .unwrap();
    store.claim_chat_resume(chat.id, turn.node_id).unwrap();
    assert!(
        store.chat(chat.id).unwrap().stop_requested,
        "recovery must finish the requested stop, not re-run the old prompt"
    );
    assert!(store
        .advance_chat_followup(chat.id, turn.node_id, &registry)
        .unwrap()
        .is_none());
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Cancelled, None)
        .unwrap();
    assert!(
        store
            .advance_chat_followup(chat.id, turn.node_id, &registry)
            .unwrap()
            .unwrap()
            .started
    );
}

#[test]
fn concurrent_consumers_claim_only_one_process_receipt() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let registry = ModelRegistry::local_only();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    let q = store
        .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::FollowUp)
        .unwrap();
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = store.path().to_path_buf();
            std::thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                barrier.wait();
                store
                    .send_chat_followup(chat.id, q.id, &ModelRegistry::local_only())
                    .unwrap()
            })
        })
        .collect();
    let receipts: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(receipts.iter().filter(|r| r.started).count(), 1);
    assert_eq!(receipts[0].node_id, receipts[1].node_id);
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 2);
}

#[test]
fn a_queue_does_not_bypass_team_control_or_current_provider_policy() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    store
        .seed_default_team(chat.project_id, &crate::RoleModelDefault::local_floor())
        .unwrap();
    store
        .set_chat_mode(chat.id, ChatMode::Team, chat.rev)
        .unwrap();
    let team = store
        .begin_chat_turn(chat.id, "plan", "team", &ModelRegistry::local_only())
        .unwrap();
    assert!(store
        .queue_chat_followup(
            chat.id,
            team.node_id,
            "build now",
            "bad",
            FollowupKind::Steer
        )
        .is_err());
    assert!(!store.chat(chat.id).unwrap().stop_requested);

    let (mut store, _dir, chat) = super::super::tests::seed();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &ModelRegistry::local_only())
        .unwrap();
    let q = store
        .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::FollowUp)
        .unwrap();
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    let denied =
        ModelRegistry::new(crate::MachineProfile::parse(crate::DEFAULT_MACHINE_PROFILE).unwrap());
    assert!(store.send_chat_followup(chat.id, q.id, &denied).is_err());
    assert_eq!(store.chat_followup(chat.id, q.id).unwrap().state, "queued");
    assert!(store.chat(chat.id).unwrap().active_node_id.is_none());
}

#[test]
fn detail_receipts_are_bounded_and_held_start_failures_are_visible() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &ModelRegistry::local_only())
        .unwrap();
    for index in 0..12 {
        let q = store
            .queue_chat_followup(
                chat.id,
                turn.node_id,
                "old",
                &index.to_string(),
                FollowupKind::FollowUp,
            )
            .unwrap();
        store.cancel_chat_followup(chat.id, q.id).unwrap();
    }
    let q = store
        .queue_chat_followup(
            chat.id,
            turn.node_id,
            "next",
            "next",
            FollowupKind::FollowUp,
        )
        .unwrap();
    assert_eq!(store.chat_followups(chat.id).unwrap().len(), 10);
    assert_eq!(
        store.chat_followups(chat.id).unwrap().last().unwrap().id,
        q.id
    );
    store
        .record_chat_followup_problem(chat.id, turn.node_id, "provider denied")
        .unwrap();
    assert_eq!(store.chat_followup(chat.id, q.id).unwrap().state, "queued");
    assert!(store
        .chat_events(chat.id, 0, 500)
        .unwrap()
        .iter()
        .any(|event| event.summary.contains("Follow-up held: provider denied")));
}

#[test]
fn another_chat_cannot_claim_cancel_or_read_a_queue() {
    let (mut store, _dir, chat) = super::super::tests::seed();
    let other = store
        .create_chat(crate::NewChat {
            project_id: chat.project_id,
            workspace: chat.workspace_path.clone().into(),
            provider: chat.provider,
            model: chat.model.clone(),
            reasoning: chat.reasoning,
        })
        .unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "first", "first", &ModelRegistry::local_only())
        .unwrap();
    let q = store
        .queue_chat_followup(chat.id, turn.node_id, "next", "q", FollowupKind::FollowUp)
        .unwrap();
    assert!(store
        .queue_chat_followup(
            other.id,
            turn.node_id,
            "wrong chat",
            "wrong",
            FollowupKind::Steer
        )
        .is_err());
    assert!(store.chat_followup(other.id, q.id).is_err());
    assert!(store.cancel_chat_followup(other.id, q.id).is_err());
    assert!(store
        .send_chat_followup(other.id, q.id, &ModelRegistry::local_only())
        .is_err());
    store.cancel_chat_followup(chat.id, q.id).unwrap();
    assert_eq!(
        store.chat_followup(chat.id, q.id).unwrap().state,
        "cancelled"
    );
}
