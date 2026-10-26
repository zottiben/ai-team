use super::tests::seed;
use crate::{
    ChatMode, FollowupKind, MachineProfile, ModelChoice, ModelRegistry, NodeStatus, Provider,
    Reasoning,
};

fn registry() -> ModelRegistry {
    ModelRegistry::new(
        MachineProfile::parse(
            &crate::DEFAULT_MACHINE_PROFILE
                .replace("local = false", "local = true")
                .replace("openai = false", "openai = true"),
        )
        .unwrap(),
    )
}
fn choice(provider: Provider, model: &str, thinking: bool) -> ModelChoice {
    ModelChoice {
        provider,
        model: model.into(),
        runtime_provider: crate::pi_provider(provider).into(),
        context: "32k".into(),
        context_tokens: 32000,
        max_output: "8k".into(),
        thinking,
        images: false,
    }
}

#[test]
fn a_different_provider_starts_fresh_instead_of_resuming_another_accounts_session() {
    let (mut store, _dir, chat) = seed();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry())
        .unwrap();
    store
        .set_node_session(first.node_id, "local-session")
        .unwrap();
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let rev = store.chat(chat.id).unwrap().rev;
    store
        .set_chat_model(
            chat.id,
            &choice(Provider::OpenAi, "fixture-other", true),
            rev,
            &registry(),
        )
        .unwrap();
    let next = store
        .begin_chat_turn(chat.id, "next", "next", &registry())
        .unwrap();
    let next = store.node_run(next.node_id).unwrap();
    assert_eq!(next.session_id, None);
    assert_eq!(next.stream_cursor, 0);
    assert!(store
        .chat_turn_context(chat.id, next.id)
        .unwrap()
        .contains("first"));
    assert_eq!(
        store.node_run(first.node_id).unwrap().session_id.as_deref(),
        Some("local-session")
    );
}

#[test]
fn model_selection_during_work_preserves_active_and_queued_snapshots() {
    let (mut store, _dir, chat) = seed();
    let registry = registry();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    store
        .set_node_session(first.node_id, "local-session")
        .unwrap();
    let queued = store
        .queue_chat_followup(
            chat.id,
            first.node_id,
            "next",
            "queued",
            FollowupKind::FollowUp,
        )
        .unwrap();
    let active = serde_json::to_value(store.node_run(first.node_id).unwrap()).unwrap();
    let rev = store.chat(chat.id).unwrap().rev;
    let changed = store
        .set_chat_model(
            chat.id,
            &choice(Provider::OpenAi, "fixture-other", false),
            rev,
            &registry,
        )
        .unwrap();
    assert_eq!(changed.active_node_id, Some(first.node_id));
    assert!(!changed.stop_requested);
    assert_eq!(changed.reasoning, Reasoning::None);
    assert_eq!(
        serde_json::to_value(store.node_run(first.node_id).unwrap()).unwrap(),
        active
    );
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1);
    assert!(store
        .db_mut()
        .write(|tx| {
            tx.execute(
                "UPDATE chat_followup SET model_settings_json=NULL WHERE id=?1",
                [queued.id],
            )?;
            Ok(())
        })
        .is_err());
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let next = store
        .send_chat_followup(chat.id, queued.id, &registry)
        .unwrap();
    let node = store.node_run(next.node_id).unwrap();
    assert_eq!(node.provider, Provider::Local);
    assert_eq!(node.model, "test-model");
    assert_eq!(node.reasoning, Some(Reasoning::High));
    assert_eq!(node.session_id.as_deref(), Some("local-session"));
    assert!(
        !store
            .send_chat_followup(chat.id, queued.id, &registry)
            .unwrap()
            .started
    );
    store
        .finish_chat_turn(chat.id, next.node_id, NodeStatus::Done, None)
        .unwrap();
    let later = store
        .begin_chat_turn(chat.id, "later", "later", &registry)
        .unwrap();
    let node = store.node_run(later.node_id).unwrap();
    assert_eq!(node.provider, Provider::OpenAi);
    assert_eq!(node.model, "fixture-other");
    assert_eq!(node.reasoning, Some(Reasoning::None));
    assert!(node.session_id.is_none());
}

#[test]
fn model_selection_checks_policy_revision_and_ownership_and_keeps_same_provider_sessions() {
    let (mut store, _dir, chat) = seed();
    let model = choice(Provider::Local, "new-local", true);
    assert!(store
        .set_chat_model(
            chat.id,
            &choice(Provider::OpenAi, "denied", true),
            chat.rev,
            &ModelRegistry::local_only()
        )
        .is_err());
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry())
        .unwrap();
    store
        .set_node_session(first.node_id, "local-session")
        .unwrap();
    store
        .ingest_pi_events(
            first.node_id,
            "local-session",
            12,
            &[crate::PiEvent::parse(r#"{"type":"agent_settled"}"#).unwrap()],
        )
        .unwrap();
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    assert!(store
        .set_chat_model(chat.id, &model, chat.rev, &registry())
        .is_err());
    let rev = store.chat(chat.id).unwrap().rev;
    store
        .set_chat_model(chat.id, &model, rev, &registry())
        .unwrap();
    let next = store
        .begin_chat_turn(chat.id, "next", "next", &registry())
        .unwrap();
    let node = store.node_run(next.node_id).unwrap();
    assert_eq!(node.model, "new-local");
    assert_eq!(node.session_id.as_deref(), Some("local-session"));
    assert_eq!(node.stream_cursor, 13);
    store
        .finish_chat_turn(chat.id, next.node_id, NodeStatus::Done, None)
        .unwrap();
    store.archive_chat(chat.id, true).unwrap();
    assert!(store
        .set_chat_model(
            chat.id,
            &model,
            store.chat(chat.id).unwrap().rev,
            &registry()
        )
        .is_err());
    store.archive_chat(chat.id, false).unwrap();
    store
        .set_chat_mode(chat.id, ChatMode::Team, store.chat(chat.id).unwrap().rev)
        .unwrap();
    assert!(store
        .set_chat_model(
            chat.id,
            &model,
            store.chat(chat.id).unwrap().rev,
            &registry()
        )
        .is_err());
}

#[test]
fn historical_queued_work_cannot_gain_an_inferred_model_from_a_later_selection() {
    let (mut store, _dir, chat) = seed();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry())
        .unwrap();
    store.db_mut().write(|tx| { tx.execute("INSERT INTO chat_followup(chat_id,after_node_id,request_id,body,kind,created_at) VALUES(?1,?2,'historical','keep','follow_up','fixture')", rusqlite::params![chat.id,first.node_id])?; Ok(()) }).unwrap();
    assert!(store
        .set_chat_model(
            chat.id,
            &choice(Provider::Local, "new", true),
            store.chat(chat.id).unwrap().rev,
            &registry()
        )
        .unwrap_err()
        .to_string()
        .contains("historical queued"));
    assert_eq!(store.chat(chat.id).unwrap().model, "test-model");
}

#[test]
fn a_queued_choice_that_loses_permission_is_not_silently_rerouted() {
    let (mut store, _dir, chat) = seed();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry())
        .unwrap();
    let queued = store
        .queue_chat_followup(
            chat.id,
            first.node_id,
            "next",
            "queued",
            FollowupKind::FollowUp,
        )
        .unwrap();
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let changed_policy = ModelRegistry::new(
        MachineProfile::parse(
            &crate::DEFAULT_MACHINE_PROFILE.replace("openai = false", "openai = true"),
        )
        .unwrap(),
    );
    assert!(store
        .send_chat_followup(chat.id, queued.id, &changed_policy)
        .is_err());
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1);
    assert_eq!(
        store.chat_followup(chat.id, queued.id).unwrap().state,
        "queued"
    );
}

#[test]
fn populated_schema_50_upgrades_without_backfilling_or_changing_existing_rows() {
    use rusqlite::{types::Value, Connection};
    fn rows(conn: &Connection) -> std::collections::BTreeMap<String, Vec<Vec<Value>>> {
        let names = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name!='schema_migrations' ORDER BY name").unwrap()
            .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        names
            .into_iter()
            .map(|name| {
                let mut query = conn
                    .prepare(&format!(
                        "SELECT * FROM \"{}\" ORDER BY rowid",
                        name.replace('"', "\"\"")
                    ))
                    .unwrap();
                let columns = query.column_count();
                let data = query
                    .query_map([], |row| {
                        (0..columns).map(|column| row.get(column)).collect()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<Vec<Value>>>>()
                    .unwrap();
                (name, data)
            })
            .collect()
    }
    let (mut store, dir, chat) = seed();
    let first = store
        .begin_chat_turn(chat.id, "original prompt", "first", &registry())
        .unwrap();
    store
        .queue_chat_followup(
            chat.id,
            first.node_id,
            "original queued body",
            "next",
            FollowupKind::FollowUp,
        )
        .unwrap();
    let path = dir.path().join("schema50.db");
    store
        .db()
        .conn()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let old = Connection::open(&path).unwrap();
    // Reconstruct the preceding version's actual schema, not just its version stamp.
    old.execute_batch("DROP TRIGGER chat_followup_model_immutable; ALTER TABLE chat_followup DROP COLUMN model_settings_json; DELETE FROM schema_migrations WHERE version=51;").unwrap();
    let mut expected = rows(&old);
    for row in expected.get_mut("chat_followup").unwrap() {
        row.push(Value::Null);
    }
    assert_eq!(
        old.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        50
    );
    drop(old);
    let upgraded = crate::Store::open(&path).unwrap();
    assert_eq!(upgraded.schema_version().unwrap(), 51);
    assert_eq!(rows(upgraded.db().conn()), expected);
    assert_eq!(
        upgraded.chat_followups(chat.id).unwrap()[0].body,
        "original queued body"
    );
    assert_eq!(
        upgraded
            .db()
            .conn()
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(upgraded
        .db()
        .conn()
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
    assert!(upgraded
        .db()
        .conn()
        .execute("UPDATE chat_followup SET model_settings_json='{}'", [])
        .is_err());
    assert_eq!(
        rows(crate::Store::open(&path).unwrap().db().conn()),
        expected
    );
}

#[test]
fn forward_model_migration_preserves_unrecorded_rows_and_rejects_snapshot_edits() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE chat_followup(id INTEGER PRIMARY KEY,body TEXT); INSERT INTO chat_followup VALUES(1,'original');").unwrap();
    db.execute_batch(include_str!("../../migrations/051_followup_models.sql"))
        .unwrap();
    let row: (String, Option<String>) = db
        .query_row(
            "SELECT body,model_settings_json FROM chat_followup",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("original".into(), None));
    assert!(db
        .execute(
            "UPDATE chat_followup SET model_settings_json='{}' WHERE id=1",
            []
        )
        .is_err());
}
