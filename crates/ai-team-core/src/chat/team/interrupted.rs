//! Explicitly stop an abandoned journalled execution. Never dispatch, reset source,
//! return a lease, or treat process recovery as model completion.
use super::children;
use crate::{ChatBuildRecovery, ChatTeamRun, Error, Result, Store};
use std::path::Path;

pub async fn recover_chat_team_processes(
    db: &Path,
    target: &ChatBuildRecovery,
) -> Result<ChatTeamRun> {
    let mut store = Store::open(db)?;
    let owner = store.claim_interrupted_team(target)?;
    let result = drain(&mut store, &owner).await;
    let problem = result.as_ref().err().map(ToString::to_string);
    let execution = store
        .finish_interrupted_team(&owner, problem.as_deref())
        .map_err(|cleanup| {
            Error::invalid(format!(
                "{}; saving process recovery also failed: {cleanup}",
                problem.as_deref().unwrap_or("Processes drained")
            ))
        })?;
    result?;
    Ok(execution)
}

async fn drain(store: &mut Store, owner: &super::ownership::Ownership) -> Result<()> {
    let records = store.chat_children(owner.run)?;
    let mut errors = Vec::new();
    for record in &records {
        if record.run != owner.run {
            return Err(Error::invalid("foreign child evidence"));
        }
        if record.state == "drained" {
            continue;
        }
        match children::drain(record).await {
            Ok(()) => {
                if let Err(error) = store.finish_chat_child(owner, record.id) {
                    errors.push(error.to_string());
                }
            }
            Err(error) => errors.push(format!("child {}: {error}", record.id)),
        }
    }
    // An old/unrecorded Pi identity is not erased merely because journalled commands
    // are gone. Keep the whole chat while any member may still be writing.
    for member in store.chat_team_members(owner.run)? {
        if let Some(pid) = member.node.pi_pid {
            if member.pi_alive() && children::live_writer(pid).await? {
                errors.push("a member process is still alive or unidentified; retain its session and checkout".into());
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::invalid(errors.join("; ")))
    }
}
