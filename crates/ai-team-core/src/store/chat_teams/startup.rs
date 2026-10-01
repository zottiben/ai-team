#[cfg(test)]
mod tests;

use crate::chat::team::ownership;
use crate::{
    ChatBuildRecovery, ChatRecoveryEntry, ChatRecoveryState, ChatTeamPhase, Result, Store,
};

struct Observation {
    target: ChatBuildRecovery,
    phase: ChatTeamPhase,
    journalled: bool,
    epoch: i64,
    quiescent: bool,
    pid: Option<i64>,
    identity: Option<String>,
    undrained: bool,
    reason: Option<String>,
}

impl Store {
    /// Read-only discovery. OS liveness is necessarily a later observation; every
    /// mutation must still acquire the task lock and CAS this exact DB revision.
    pub fn chat_team_recovery_scan(&self, chat: Option<i64>) -> Result<Vec<ChatRecoveryEntry>> {
        let mut query = self.db().conn().prepare("SELECT t.chat_id,t.run_id,t.control_node_id,t.rev,t.phase,t.controller_protocol,t.child_journal,t.child_epoch,t.quiescent,t.supervisor_pid,t.supervisor_identity,t.reason,
            EXISTS(SELECT 1 FROM chat_child x WHERE x.run_id = t.run_id AND x.state != 'drained')
            FROM chat_team_run t JOIN chat c ON c.id = t.chat_id
            WHERE c.active_node_id = t.control_node_id AND c.archived = 0 AND (?1 IS NULL OR c.id = ?1) ORDER BY t.run_id")?;
        let rows = query
            .query_map([chat], |row| {
                Ok(Observation {
                    target: ChatBuildRecovery {
                        chat_id: row.get(0)?,
                        run_id: row.get(1)?,
                        node_id: row.get(2)?,
                        expect_revision: row.get(3)?,
                    },
                    phase: row.get(4)?,
                    journalled: row.get::<_, bool>(5)? && row.get::<_, bool>(6)?,
                    epoch: row.get(7)?,
                    quiescent: row.get(8)?,
                    pid: row.get(9)?,
                    identity: row.get(10)?,
                    reason: row.get(11)?,
                    undrained: row.get(12)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|row| {
                let (state, reason) = classify(self.path(), &row).unwrap_or_else(|error| {
                    (ChatRecoveryState::NeedsInspection, Some(error.to_string()))
                });
                Ok(ChatRecoveryEntry {
                    target: row.target,
                    state,
                    reason,
                })
            })
            .collect()
    }
}

fn classify(
    db: &std::path::Path,
    row: &Observation,
) -> Result<(ChatRecoveryState, Option<String>)> {
    use ChatRecoveryState::{Active, NeedsInspection, PendingDispatch, Quiescent, Recoverable};
    if !row.journalled {
        return Ok((NeedsInspection, Some("This older execution has no complete controller/child journal; inspect it without automatic cleanup.".into())));
    }
    let path = ownership::lock_path(db, row.target.run_id)?;
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Ok((
            NeedsInspection,
            Some("A controller lock is a symlink; leave this execution alone.".into()),
        ));
    }
    if ownership::held(&path)? {
        return Ok((Active, None));
    }
    if row.quiescent && !row.undrained {
        return Ok((Quiescent, row.reason.clone()));
    }
    if row.phase == ChatTeamPhase::Grounding
        && row.epoch == 0
        && row.pid.is_some()
        && !row.undrained
    {
        // This is a reservation, not a claimed task. A live creator may still hand
        // it to its driver. Explicit human recovery may win, startup may not guess.
        if crate::chat::process_matches(row.pid, row.identity.as_deref()) {
            return Ok((PendingDispatch, Some("The admitting host may still dispatch this reservation; no controller task is claimed yet.".into())));
        }
        return Ok((Recoverable, None));
    }
    if row.phase == ChatTeamPhase::Building && row.pid.is_none() && !row.undrained {
        return Ok((PendingDispatch, Some("Approved work has not claimed a build controller; startup will not dispatch or cancel it.".into())));
    }
    if row.pid.is_some()
        && row.epoch > 0
        && matches!(
            row.phase,
            ChatTeamPhase::Grounding
                | ChatTeamPhase::Planning
                | ChatTeamPhase::Building
                | ChatTeamPhase::Blocked
        )
    {
        return Ok((Recoverable, None));
    }
    Ok((
        NeedsInspection,
        Some(row.reason.clone().unwrap_or_else(|| {
            "Uncertain process evidence is parked for explicit inspection; no automatic retry."
                .into()
        })),
    ))
}
