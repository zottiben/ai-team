use super::fixture::Fixture;
use ai_team_core::{
    drive_chat_team_planning, Chat, ChatMode, ChatTeamPhase, ModelRegistry, NewChat, NodeStatus,
    Store,
};

pub(super) async fn run(f: &Fixture, store: &mut Store, source: &Chat) -> anyhow::Result<()> {
    let chat = store.create_chat(NewChat {
        project_id: source.project_id,
        workspace: f.repo.clone(),
        provider: source.provider,
        model: source.model.clone(),
        reasoning: source.reasoning,
    })?;
    store.set_chat_mode(chat.id, ChatMode::Team, chat.rev)?;
    std::fs::write(
        f.root.join("context-failure"),
        "required ClickUp is unavailable",
    )?;
    let submission = store.begin_chat_turn(
        chat.id,
        "Plan using the required ticket",
        "context-failure",
        &ModelRegistry::local_only(),
    )?;
    let failure = drive_chat_team_planning(&f.db, chat.id, submission.node_id)
        .await
        .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("CONTEXT_UNAVAILABLE: clickup via mcp"),
        "{failure}"
    );
    let execution = store.chat_team_run(submission.run_id)?.unwrap();
    assert_eq!(execution.phase, ChatTeamPhase::Blocked);
    assert!(execution.quiescent);
    assert_eq!(
        store.chat_team_members(submission.run_id)?.len(),
        1,
        "no planner after failed grounding"
    );
    assert_eq!(
        store.node_run(submission.node_id)?.status,
        NodeStatus::Failed
    );
    assert!(!f.root.join("pool-calls").exists());
    store.stop_chat_team_planning(chat.id, submission.node_id, execution.rev)?;
    store.archive_chat(chat.id, true)?;
    std::fs::remove_file(f.root.join("context-failure"))?;
    Ok(())
}
