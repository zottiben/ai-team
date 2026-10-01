//! Attachment/admission recovery is process draining, never permission to dispatch.
use crate::{ChatBuildRecovery, Result, Store};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRecoveryState {
    Active,
    Quiescent,
    PendingDispatch,
    Recoverable,
    Recovered,
    NeedsInspection,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRecoveryEntry {
    /// The observed revision, advanced after a successful drain. Not new approval.
    pub target: ChatBuildRecovery,
    pub state: ChatRecoveryState,
    pub reason: Option<String>,
}

/// One snapshot, one attempt per abandoned execution. A failed/uncertain attempt
/// parks for inspection rather than generating an automatic retry/event loop.
pub async fn recover_abandoned_chat_teams(db: &Path) -> Result<Vec<ChatRecoveryEntry>> {
    recover(db, None).await
}

/// The admission boundary scans only this chat, never the project's latest run.
pub async fn recover_abandoned_chat_team(db: &Path, chat: i64) -> Result<Vec<ChatRecoveryEntry>> {
    recover(db, Some(chat)).await
}

async fn recover(db: &Path, chat: Option<i64>) -> Result<Vec<ChatRecoveryEntry>> {
    let path = db.to_owned();
    let mut entries =
        tokio::task::spawn_blocking(move || Store::open(&path)?.chat_team_recovery_scan(chat))
            .await
            .map_err(|error| {
                crate::Error::invalid(format!("recovery discovery stopped: {error}"))
            })??;
    for entry in &mut entries {
        if entry.state != ChatRecoveryState::Recoverable {
            continue;
        }
        match super::recover_chat_team_processes(db, &entry.target).await {
            Ok(execution) => {
                entry.target.expect_revision = execution.rev;
                entry.state = ChatRecoveryState::Recovered;
                entry.reason = execution.reason;
            }
            Err(error) => {
                // The lock + exact revision in claim_interrupted_team remain the
                // authority. Discovery itself cannot steal a racing controller.
                entry.state = ChatRecoveryState::NeedsInspection;
                entry.reason = Some(error.to_string());
            }
        }
    }
    Ok(entries)
}
