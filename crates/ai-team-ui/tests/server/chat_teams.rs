//! Authenticated exact-controller commands; no model or neighbour process is launched.
use super::*;
use ai_team_core::{ChatMode, ChatTeamPhase, Store};

#[test]
fn team_detail_and_pause_controls_do_not_masquerade_as_a_solo_turn() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("team.db");
    let mut store = Store::init(&db).unwrap();
    let (chat, turn) = chat_recovery::interrupted(&mut store, dir.path());
    let h = Harness::bound(
        ServeOptions {
            store: Some(Store::open(&db).unwrap()),
            ..Default::default()
        },
        None,
    );
    let route = format!("/api/chats/{chat}");
    let detail = h.get(&route).json();
    assert_eq!(detail["state"], "team_blocked");
    let target = detail["team_recovery"]["target"].clone();
    let stop = format!("{route}/team/stop");
    let other = chat + 100;
    assert_eq!(
        h.post(
            &format!("/api/chats/{other}/team/stop"),
            &target.to_string()
        )
        .status,
        400
    );
    assert_eq!(
        h.post(
            &format!("{route}/stop"),
            &format!(r#"{{"node_id":{}}}"#, turn.node_id)
        )
        .status,
        400
    );
    let mut stale = target.clone();
    stale["expect_revision"] = 0.into();
    assert_eq!(h.post(&stop, &stale.to_string()).status, 400);
    assert_eq!(store.chat(chat).unwrap().active_node_id, Some(turn.node_id));
    assert_eq!(h.post(&stop, &target.to_string()).status, 200);
    assert_eq!(
        store.chat_team_run(turn.run_id).unwrap().unwrap().phase,
        ChatTeamPhase::Finished
    );
    let paused = store.chat(chat).unwrap();
    assert!(paused.active_node_id.is_none());
    let mode = format!("{route}/mode");
    assert_eq!(
        h.post(&mode, r#"{"mode":"single","expect_revision":0}"#)
            .status,
        400
    );
    assert_eq!(
        h.post(
            &mode,
            &format!(r#"{{"mode":"single","expect_revision":{}}}"#, paused.rev)
        )
        .status,
        200
    );
    assert_eq!(store.chat(chat).unwrap().mode, ChatMode::Single);
    assert_eq!(store.chat_turns(chat).unwrap().len(), 1);
    assert_eq!(
        h.get(&route).json()["team_builds"][0]["execution"]["run_id"],
        turn.run_id
    );
}

#[test]
fn explicit_team_process_recovery_advances_only_its_exact_target() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    // Created after server attachment: ordinary GET reports but does not drain it.
    let (chat, turn) = chat_recovery::interrupted(&mut store, dir.path());
    let route = format!("/api/chats/{chat}");
    let before = h.get(&route).json();
    assert_eq!(before["state"], "team_interrupted");
    let target = before["team_recovery"]["target"].clone();
    assert_eq!(
        h.post(&format!("{route}/team/recover"), &target.to_string())
            .status,
        200
    );
    assert_eq!(h.get(&route).json()["state"], "team_blocked");
    assert!(store.chat_team_run(turn.run_id).unwrap().unwrap().quiescent);
    assert_eq!(
        h.post(&format!("{route}/team/recover"), &target.to_string())
            .status,
        400
    );
    assert_eq!(store.chat_turns(chat).unwrap().len(), 1);
    assert_eq!(store.node_runs(turn.run_id).unwrap().len(), 1);
}

#[test]
fn every_team_command_requires_authentication_and_the_path_chat() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let (chat, turn) = chat_recovery::interrupted(&mut store, dir.path());
    let target = serde_json::json!({"chat_id":chat,"run_id":turn.run_id,"node_id":turn.node_id,"expect_revision":2});
    for action in [
        "stop",
        "recover",
        "reconcile",
        "continue",
        "close",
        "approve",
        "review",
    ] {
        let body = match action {
            "continue" => {
                serde_json::json!({"target":target,"slice_key":"S1","expect_slice_revision":1})
            }
            "close" => {
                serde_json::json!({"target":target,"expect_plan_revision":0,"expect_slices":{},"reason":"keep"})
            }
            "approve" => {
                serde_json::json!({"target":target,"approval":{"expect_control_revision":2,"expect_plan_revision":0,"expect_roster_revision":"none","expect_head":"none"}})
            }
            _ => target.clone(),
        };
        let route = format!("/api/chats/{chat}/team/{action}");
        // Use the real server address, but no valid bearer token.
        let mut stream = TcpStream::connect(h.addr).unwrap();
        let request = format!("POST {route} HTTP/1.1\r\nHost: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nConnection: close\r\n\r\n{body}", h.addr, body.to_string().len());
        stream.write_all(request.as_bytes()).unwrap();
        assert_eq!(read_response(stream).status, 401, "{action}");
        assert_eq!(
            h.post(
                &format!("/api/chats/{}/team/{action}", chat + 100),
                &body.to_string()
            )
            .status,
            400,
            "{action}"
        );
    }
    assert_eq!(store.chat_turns(chat).unwrap().len(), 1);
    assert!(store.chat_build_slices(turn.run_id).unwrap().is_empty());
}
