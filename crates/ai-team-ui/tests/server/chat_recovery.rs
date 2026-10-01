//! Socket-level attachment wiring; core runtime tests cover actual surviving children.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "chat_recovery/processes.rs"]
mod processes;

use super::*;
use ai_team_core::{
    ChatMode, ChatTeamPhase, Db, ModelRegistry, NewChat, NewProject, Provider, Reasoning, Store,
};

fn interrupted(
    store: &mut Store,
    workspace: &std::path::Path,
) -> (i64, ai_team_core::ChatSubmission) {
    let project = store
        .create_project(NewProject {
            name: format!(
                "Recovery {}",
                workspace.file_name().unwrap().to_string_lossy()
            ),
            ..Default::default()
        })
        .unwrap();
    store
        .seed_default_team(project.id, &ai_team_core::RoleModelDefault::local_floor())
        .unwrap();
    let chat = store
        .create_chat_in_mode(
            NewChat {
                project_id: project.id,
                workspace: workspace.to_owned(),
                provider: Provider::Local,
                model: "fixture".into(),
                reasoning: Reasoning::High,
            },
            ChatMode::Team,
        )
        .unwrap();
    let turn = store
        .begin_chat_turn(
            chat.id,
            "Keep the interrupted work",
            "request",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    // Simulate a controller that claimed but exited before any child spawn. Its host
    // PID is still live: the independent task lock, not host liveness, is decisive.
    Db::open(store.path())
        .unwrap()
        .conn()
        .execute(
            "UPDATE chat_team_run SET child_epoch = 1, rev = rev + 1 WHERE run_id = ?1",
            [turn.run_id],
        )
        .unwrap();
    (chat.id, turn)
}

#[test]
fn binding_recovers_abandoned_team_tasks_without_resuming_them() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("team.db");
    let mut store = Store::init(&db).unwrap();
    let (chat, turn) = interrupted(&mut store, dir.path());
    std::fs::write(dir.path().join("kept.txt"), "uncommitted human work").unwrap();
    let h = Harness::bound(
        ServeOptions {
            store: Some(Store::open(&db).unwrap()),
            db_path: Some(dir.path().join("not-the-store.db")),
            ..Default::default()
        },
        None,
    );
    let recovered = store.chat_team_run(turn.run_id).unwrap().unwrap();
    assert!(
        recovered.quiescent,
        "server startup did not recover the abandoned team task"
    );
    assert_eq!(recovered.phase, ChatTeamPhase::Blocked);
    assert_eq!(store.chat(chat).unwrap().active_node_id, Some(turn.node_id));
    assert_eq!(store.node_runs(turn.run_id).unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("kept.txt")).unwrap(),
        "uncommitted human work"
    );
    assert!(!dir.path().join("not-the-store.db").exists());
    let detail = h.get(&format!("/api/chats/{chat}")).json();
    assert_eq!(detail["team_recovery"]["state"], "quiescent");
    assert_eq!(
        detail["can_resume"], false,
        "never offer a solo resume for a team"
    );
    assert_eq!(
        store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        recovered.rev
    );
}

#[test]
fn later_attachment_is_authenticated_once_and_admission_recovers_only_its_chat() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("team.db");
    let h = Harness::watching(&db);
    assert!(!db.exists());
    let mut store = Store::init(&db).unwrap();
    let (chat, turn) = interrupted(&mut store, dir.path());
    let revision = store.chat_team_run(turn.run_id).unwrap().unwrap().rev;
    assert_eq!(h.get_anonymous(&format!("/api/chats/{chat}")).status, 401);
    assert_eq!(
        store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        revision
    );
    assert_eq!(
        h.get(&format!("/api/chats/{chat}")).json()["team_recovery"]["state"],
        "quiescent"
    );
    let one = dir.path().join("one");
    let two = dir.path().join("two");
    std::fs::create_dir(&one).unwrap();
    std::fs::create_dir(&two).unwrap();
    let (first, first_turn) = interrupted(&mut store, &one);
    let (second, second_turn) = interrupted(&mut store, &two);
    assert_eq!(
        h.get(&format!("/api/chats/{first}")).json()["team_recovery"]["state"],
        "recoverable"
    );
    assert!(
        !store
            .chat_team_run(first_turn.run_id)
            .unwrap()
            .unwrap()
            .quiescent,
        "ordinary refresh is not a recovery loop"
    );
    let response = h.post(
        &format!("/api/chats/{first}/messages"),
        r#"{"message":"must not start another agent","request_id":"new"}"#,
    );
    assert_eq!(response.status, 400);
    assert!(response.body.contains("Team execution controls"));
    assert!(
        store
            .chat_team_run(first_turn.run_id)
            .unwrap()
            .unwrap()
            .quiescent
    );
    assert!(
        !store
            .chat_team_run(second_turn.run_id)
            .unwrap()
            .unwrap()
            .quiescent
    );
    assert_eq!(
        store.chat(first).unwrap().active_node_id,
        Some(first_turn.node_id)
    );
    assert_eq!(
        store.chat(second).unwrap().active_node_id,
        Some(second_turn.node_id)
    );
    assert_eq!(store.chat_turns(first).unwrap().len(), 1);
    assert_eq!(store.node_runs(first_turn.run_id).unwrap().len(), 1);
}

#[test]
fn unsupported_evidence_stays_visible_without_taking_down_other_chats() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("team.db");
    let mut store = Store::init(&db).unwrap();
    let (chat, turn) = interrupted(&mut store, dir.path());
    Db::open(&db)
        .unwrap()
        .conn()
        .execute(
            "UPDATE chat_team_run SET child_journal = 0 WHERE run_id = ?1",
            [turn.run_id],
        )
        .unwrap();
    let before = store.chat_team_run(turn.run_id).unwrap().unwrap();
    let h = Harness::watching(&db);
    let detail = h.get(&format!("/api/chats/{chat}")).json();
    assert_eq!(detail["team_recovery"]["state"], "needs_inspection");
    assert_eq!(detail["can_resume"], false);
    assert!(detail["team_recovery"]["reason"]
        .as_str()
        .unwrap()
        .contains("older execution"));
    assert_eq!(
        store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        before.rev
    );
    let project = store.chat(chat).unwrap().project_id;
    let solo = store
        .create_chat(NewChat {
            project_id: project,
            workspace: dir.path().to_owned(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    assert_eq!(
        h.get(&format!("/api/chats/{}", solo.id)).json()["state"],
        "empty"
    );
}

#[test]
fn an_attachment_scan_failure_is_visible_without_a_background_retry_loop() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("team.db");
    let mut store = Store::init(&db).unwrap();
    let (chat, turn) = interrupted(&mut store, dir.path());
    // A bad discovery row fails the whole scan, rather than one process drain.
    Db::open(&db)
        .unwrap()
        .conn()
        .execute(
            "UPDATE chat_team_run SET child_epoch = 'invalid' WHERE run_id = ?1",
            [turn.run_id],
        )
        .unwrap();
    let h = Harness::watching(&db);
    Db::open(&db)
        .unwrap()
        .conn()
        .execute(
            "UPDATE chat_team_run SET child_epoch = 1 WHERE run_id = ?1",
            [turn.run_id],
        )
        .unwrap();
    let response = h.get(&format!("/api/chats/{chat}"));
    assert_eq!(response.status, 200);
    assert!(
        response.json()["recovery_error"]
            .as_str()
            .is_some_and(|message| message.contains("child_epoch")),
        "startup discovery failed silently: {}",
        response.body
    );
    assert!(
        !store.chat_team_run(turn.run_id).unwrap().unwrap().quiescent,
        "GET silently retried a failed attachment pass"
    );
}

#[test]
fn a_failed_bind_does_not_initiate_process_recovery() {
    let occupied = Harness::start();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("team.db");
    let mut store = Store::init(&db).unwrap();
    let (_, turn) = interrupted(&mut store, dir.path());
    let before = store.chat_team_run(turn.run_id).unwrap().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert!(runtime
        .block_on(Server::bind(ServeOptions {
            port: occupied.addr.port(),
            store: Some(Store::open(&db).unwrap()),
            credentials: ai_team_core::CredentialStore::isolated(),
            ..Default::default()
        }))
        .is_err());
    assert_eq!(
        store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        before.rev
    );
}
