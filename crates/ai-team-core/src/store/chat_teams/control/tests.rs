use super::*;
use crate::{ChatMode, ModelRegistry, NewChat, NewProject, Provider, Reasoning};

fn setup() -> (tempfile::TempDir, Store, TeamControl, Arc<Ownership>) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Control".into(),
            ..Default::default()
        })
        .unwrap();
    store.seed_default_team(project.id).unwrap();
    let chat = store
        .create_chat_in_mode(
            NewChat {
                project_id: project.id,
                workspace: dir.path().into(),
                provider: Provider::Local,
                model: "fake".into(),
                reasoning: Reasoning::High,
            },
            ChatMode::Team,
        )
        .unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "plan", "one", &ModelRegistry::local_only())
        .unwrap();
    let (control, owner) = store
        .claim_chat_team_planning(chat.id, turn.node_id)
        .unwrap();
    (dir, store, control, owner)
}

#[test]
fn a_stale_callback_in_the_same_process_cannot_park_or_release_the_new_phase() {
    let (_dir, mut store, mut control, _owner) = setup();
    let mut stale = control;
    store.start_chat_team_planner(&mut control).unwrap();
    assert!(store
        .park_chat_team_planning(&mut stale, ChatTeamPhase::Blocked, "old callback")
        .is_err());
    assert_eq!(
        store.chat_team_run(control.run_id).unwrap().unwrap().phase,
        ChatTeamPhase::Planning
    );
    assert_eq!(
        store.chat(control.chat_id).unwrap().active_node_id,
        Some(control.node_id)
    );
    store
        .park_chat_team_planning(&mut control, ChatTeamPhase::Blocked, "needs input")
        .unwrap();
    assert!(store.check_chat_team_control(&stale).is_err());
}

#[test]
fn stop_wins_the_race_with_a_successful_planning_pause() {
    let (_dir, mut store, mut control, _owner) = setup();
    store
        .request_chat_stop(control.chat_id, control.node_id)
        .unwrap();
    assert_eq!(
        store
            .park_chat_team_planning(&mut control, ChatTeamPhase::AwaitingApproval, "ready")
            .unwrap(),
        ChatTeamPhase::Finished
    );
    assert!(store
        .chat(control.chat_id)
        .unwrap()
        .active_node_id
        .is_none());
    assert_eq!(
        store.run(control.run_id).unwrap().status,
        crate::RunStatus::Cancelled
    );
}

#[test]
fn planning_cleanup_never_releases_build_bookkeeping() {
    let (_dir, mut store, mut control, _owner) = setup();
    store.db_mut().write(|tx| {
        tx.execute("INSERT INTO chat_build_slice (run_id, slice_key, planner_slice_id, approved_rev) VALUES (?1, 'S1', 1, 1)", [control.run_id])?;
        Ok(())
    }).unwrap();
    assert!(store
        .park_chat_team_planning(&mut control, ChatTeamPhase::Finished, "stop")
        .is_err());
    assert_eq!(
        store.chat(control.chat_id).unwrap().active_node_id,
        Some(control.node_id)
    );
}
