use super::*;
use crate::{
    chat::team::children, chat_changes::checkout, ModelRegistry, NewChat, NewProject, Provider,
    Reasoning,
};

#[tokio::test]
async fn inspection_keeps_already_drained_child_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: dir.path().into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let snapshot = Snapshot {
        workspace: chat.workspace_path,
        head: None,
        branch: Some("main".into()),
        fingerprint: "reviewed".into(),
        action: checkout::Action::Stage {
            path: "file".into(),
        },
        remote: None,
    };
    let op = store.record_checkout_preview(chat.id, &snapshot).unwrap();
    let owner = store
        .claim_checkout_operation(chat.id, op.id, op.rev, false)
        .unwrap();
    let child = store
        .begin_chat_child(
            &owner,
            "command",
            "finished",
            Some(dir.path()),
            &children::boot().unwrap(),
        )
        .unwrap();
    store.finish_chat_child(&owner, child.id).unwrap();
    store
        .db_mut()
        .write(|tx| {
            tx.execute(
                "UPDATE checkout_child SET ended_at='2000-01-01T00:00:00Z' WHERE id=?1",
                [child.id],
            )?;
            Ok(())
        })
        .unwrap();
    drop(owner);
    let rev = store.checkout_operation(chat.id, op.id).unwrap().rev;
    // This fixture deliberately has no Git repository: inspection fails, but historical
    // drain evidence must not be rewritten even during an unsuccessful inspection.
    assert!(checkout::inspect(&mut store, chat.id, op.id, rev)
        .await
        .is_err());
    let ended: String = store
        .db()
        .conn()
        .query_row(
            "SELECT ended_at FROM checkout_child WHERE id=?1",
            [child.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ended, "2000-01-01T00:00:00Z");
}

#[tokio::test]
async fn checkout_receipts_exclude_other_chats_and_unknown_spawn_intents_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let make = || NewChat {
        project_id: project.id,
        workspace: dir.path().into(),
        provider: Provider::Local,
        model: "fixture".into(),
        reasoning: Reasoning::High,
    };
    let chat = store.create_chat(make()).unwrap();
    let other = store.create_chat(make()).unwrap();
    let snapshot = Snapshot {
        workspace: chat.workspace_path.clone(),
        head: None,
        branch: Some("main".into()),
        fingerprint: "reviewed".into(),
        action: checkout::Action::Stage {
            path: "file".into(),
        },
        remote: None,
    };
    let first = store.record_checkout_preview(chat.id, &snapshot).unwrap();
    let second = store.record_checkout_preview(other.id, &snapshot).unwrap();
    let owner = store
        .claim_checkout_operation(chat.id, first.id, first.rev, false)
        .unwrap();
    let mut connection = Store::open(store.path()).unwrap();
    assert!(connection
        .claim_checkout_operation(other.id, second.id, second.rev, false)
        .is_err());
    assert!(
        connection
            .claim_checkout_operation(chat.id, first.id, first.rev + 1, true)
            .is_err(),
        "a live owner's inode lock cannot be stolen"
    );
    assert!(connection
        .begin_chat_turn(
            other.id,
            "do not start",
            "blocked",
            &ModelRegistry::local_only()
        )
        .is_err());
    let child = store
        .begin_chat_child(
            &owner,
            "command",
            "never spawned",
            Some(dir.path()),
            &children::boot().unwrap(),
        )
        .unwrap();
    assert!(
        store.chat_children(first.id).unwrap().is_empty(),
        "operator work must not borrow a team journal"
    );
    assert_eq!(store.checkout_children(first.id).unwrap()[0].id, child.id);
    drop(owner);
    let before = connection.checkout_operation(chat.id, first.id).unwrap();
    let error = checkout::inspect(&mut connection, chat.id, first.id, before.rev)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("intent"), "{error}");
    let held = connection.checkout_operation(chat.id, first.id).unwrap();
    assert_eq!(held.state, "inspection");
    assert!(checkout::acknowledge(
        &mut connection,
        chat.id,
        first.id,
        held.rev,
        "reviewed",
        "cannot assume it never spawned"
    )
    .await
    .is_err());
    assert!(connection.checkout_available(other.id).is_err());
    assert_eq!(
        connection.checkout_children(first.id).unwrap()[0].state,
        "intent"
    );
    assert!(connection.chat_turns(chat.id).unwrap().is_empty());
}
