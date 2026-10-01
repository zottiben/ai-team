//! Compare-and-swap authority for the planning controller, independent of Pi node status.

#[cfg(test)]
mod tests;

use rusqlite::{params, Connection};

use crate::chat::team::{ownership::Ownership, TeamControl};
use crate::{ChatTeamPhase, Error, NodeStatus, Result, Store};
use std::sync::Arc;

impl Store {
    pub(crate) fn claim_chat_team_planning(
        &mut self,
        chat: i64,
        node: i64,
    ) -> Result<(TeamControl, Arc<Ownership>)> {
        let run = self.node_run(node)?.run_id;
        let ownership = Ownership::acquire(self.path(), run)?;
        let team = self
            .chat_team_run(run)?
            .filter(|team| team.chat_id == chat && team.control_node_id == node)
            .ok_or_else(|| {
                Error::invalid("that node does not control this chat's team execution")
            })?;
        let pid = i64::from(std::process::id());
        let identity = crate::chat::process_identity(pid)
            .ok_or_else(|| Error::invalid("could not identify the team controller process"))?;
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE chat_team_run SET controller_protocol = 1, child_journal = 1, child_epoch = child_epoch + 1, quiescent = 0, rev = rev + 1 WHERE run_id = ?1 AND rev = 1 AND phase = 'grounding'
                 AND supervisor_pid = ?2 AND supervisor_identity = ?3
                 AND controller_protocol = 1 AND child_journal = 1 AND child_epoch = 0
                 AND EXISTS (SELECT 1 FROM chat c JOIN run r ON r.id = ?1
                     WHERE c.id = chat_team_run.chat_id AND c.active_node_id = chat_team_run.control_node_id
                     AND c.archived = 0 AND r.status = 'running')",
                params![run, pid, identity],
            )?;
            if changed != 1 { return Err(Error::invalid("this team controller was already started or needs explicit recovery")); }
            Ok(())
        })?;
        ownership.bind(self)?;
        Ok((
            TeamControl {
                chat_id: chat,
                run_id: run,
                node_id: node,
                revision: team.rev + 1,
                owner: Some(pid),
            },
            ownership,
        ))
    }

    pub(crate) fn check_chat_team_control(&self, control: &TeamControl) -> Result<()> {
        check(self.db().conn(), control)
    }

    pub(crate) fn start_chat_team_planner(&mut self, control: &mut TeamControl) -> Result<()> {
        self.db_mut().write(|tx| {
            check(tx, control)?;
            let changed = tx.execute(
                "UPDATE chat_team_run SET phase = 'planning', rev = rev + 1 WHERE run_id = ?1 AND phase = 'grounding'
                 AND EXISTS (SELECT 1 FROM chat WHERE id = ?2 AND stop_requested = 0)",
                params![control.run_id, control.chat_id],
            )?;
            if changed != 1 { return Err(Error::invalid("this execution stopped before planning")); }
            Ok(())
        })?;
        control.revision += 1;
        Ok(())
    }

    pub(crate) fn settle_chat_team_member(
        &mut self,
        control: &TeamControl,
        node: i64,
        status: NodeStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        if !matches!(
            status,
            NodeStatus::Done | NodeStatus::Failed | NodeStatus::Cancelled
        ) {
            return Err(Error::invalid(
                "a planning attempt must settle in a terminal state",
            ));
        }
        let member = self
            .chat_team_members(control.run_id)?
            .into_iter()
            .find(|member| member.node.id == node)
            .ok_or_else(|| Error::invalid("this attempt does not belong to the controller"))?;
        if member.pi_alive() {
            return Err(Error::invalid(
                "reap this Pi process before settling its attempt",
            ));
        }
        self.db_mut().write(|tx| {
            check(tx, control)?;
            tx.execute(
                "UPDATE node_run SET status = ?2, blocked_reason = ?3, pi_pid = NULL, supervisor_pid = NULL,
                    ended_at = ?4, updated_at = ?4, rev = rev + 1 WHERE id = ?1",
                params![node, status, reason, crate::now()],
            )?;
            tx.execute("UPDATE chat_team_node SET pi_identity = NULL, live_text = '' WHERE node_id = ?1", [node])?;
            Ok(())
        })
    }

    /// Only a quiescent planning controller can pause. Build/lease cleanup needs its
    /// own lifecycle and must never silently pass through this no-lease boundary.
    pub(crate) fn park_chat_team_planning(
        &mut self,
        control: &mut TeamControl,
        phase: ChatTeamPhase,
        reason: &str,
    ) -> Result<ChatTeamPhase> {
        if !matches!(
            phase,
            ChatTeamPhase::AwaitingApproval | ChatTeamPhase::Blocked | ChatTeamPhase::Finished
        ) {
            return Err(Error::invalid("invalid planning pause"));
        }
        if self
            .chat_team_members(control.run_id)?
            .iter()
            .any(crate::ChatTeamMember::pi_alive)
        {
            return Err(Error::invalid(
                "a team process is still alive; keep checkout ownership until it is reaped",
            ));
        }
        let actual = self.db_mut().write(|tx| {
            check(tx, control)?;
            let leases: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_slice WHERE run_id = ?1)", [control.run_id], |row| row.get(0))?;
            if leases { return Err(Error::invalid("a build needs lease-aware cleanup, not planning cleanup")); }
            let undrained: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_child WHERE run_id = ?1 AND state != 'drained')", [control.run_id], |row| row.get(0))?;
            if undrained { return Err(Error::invalid("planning has undrained child evidence; retain the checkout for recovery")); }
            let stopped: bool = tx.query_row("SELECT stop_requested FROM chat WHERE id = ?1", [control.chat_id], |row| row.get(0))?;
            let cancelled = stopped || phase == ChatTeamPhase::Finished;
            let actual = if cancelled { ChatTeamPhase::Finished } else { phase };
            let message = if cancelled { "Stopped by you. The chat, plan and working files are kept." } else { reason };
            let at = crate::now();
            tx.execute("UPDATE chat_team_run SET phase = ?2, reason = ?3, supervisor_pid = NULL,
                supervisor_identity = NULL, quiescent = child_journal, rev = rev + 1 WHERE run_id = ?1", params![control.run_id, actual, message])?;
            tx.execute("UPDATE run SET status = ?2, blocked_reason = ?3, ended_at = ?4, updated_at = ?5,
                plan_slug = CASE WHEN ?6 = 'awaiting_approval' THEN ?7 ELSE plan_slug END, rev = rev + 1 WHERE id = ?1",
                params![control.run_id, if cancelled { "cancelled" } else { "blocked" }, message, cancelled.then_some(&at), at,
                    actual, format!("chat-{}", control.chat_id)])?;
            tx.execute("UPDATE node_run SET status = ?2, blocked_reason = ?3, pi_pid = NULL, supervisor_pid = NULL,
                ended_at = ?4, updated_at = ?4, rev = rev + 1 WHERE run_id = ?1 AND status IN ('queued', 'running')",
                params![control.run_id, if cancelled { "cancelled" } else { "failed" }, message, at])?;
            tx.execute("UPDATE chat SET active_node_id = CASE WHEN ?2 THEN NULL ELSE active_node_id END,
                stop_requested = 0, live_text = '', supervisor_identity = NULL, pi_identity = NULL,
                rev = rev + 1, updated_at = ?3 WHERE id = ?1", params![control.chat_id, cancelled, at])?;
            tx.execute("INSERT INTO event (run_id, node_run_id, at, kind, actor, summary)
                VALUES (?1, ?2, ?3, 'note', 'ai-team', ?4)", params![control.run_id, control.node_id, at, message])?;
            Ok(actual)
        })?;
        control.revision += 1;
        control.owner = None;
        Ok(actual)
    }

    /// Stop an idle approval/failed-planning pause. Active workers must observe the
    /// stop flag and reap their own children first. This never returns a build lease.
    pub fn stop_chat_team_planning(
        &mut self,
        chat: i64,
        node: i64,
        expect_revision: i64,
    ) -> Result<()> {
        let run = self.node_run(node)?.run_id;
        let team = self
            .chat_team_run(run)?
            .filter(|team| team.chat_id == chat && team.control_node_id == node)
            .ok_or_else(|| {
                Error::invalid("that node does not control this chat's team execution")
            })?;
        if team.rev != expect_revision
            || team.supervisor_pid.is_some()
            || !matches!(
                team.phase,
                ChatTeamPhase::AwaitingApproval | ChatTeamPhase::Blocked
            )
        {
            return Err(Error::invalid("this team is not at an idle planning pause; stop its active controller and refresh"));
        }
        let mut control = TeamControl {
            chat_id: chat,
            run_id: run,
            node_id: node,
            revision: team.rev,
            owner: None,
        };
        self.park_chat_team_planning(&mut control, ChatTeamPhase::Finished, "Stopped by you.")?;
        Ok(())
    }
}

fn check(conn: &Connection, control: &TeamControl) -> Result<()> {
    let current: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_team_run t JOIN chat c ON c.id = t.chat_id JOIN run r ON r.id = t.run_id
         WHERE t.run_id = ?1 AND t.chat_id = ?2 AND t.control_node_id = ?3 AND t.rev = ?4 AND t.supervisor_pid IS ?5
           AND c.active_node_id = t.control_node_id AND c.archived = 0 AND r.status IN ('running', 'blocked')
           AND t.phase IN ('grounding', 'planning', 'awaiting_approval', 'blocked'))",
        params![control.run_id, control.chat_id, control.node_id, control.revision, control.owner], |row| row.get(0),
    )?;
    if !current {
        return Err(Error::invalid(
            "this controller no longer owns the chat's planning execution",
        ));
    }
    Ok(())
}
