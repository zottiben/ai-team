//! Human build approval against the owned engine, serialized with every planning writer.

mod routing;

use std::path::Path;
use std::process::{Command, Stdio};

use ai_planner_core as planner;
use rusqlite::{params, Connection};

use crate::planning::{PlanAccess, PlanAction};
use crate::{ChatBuildApproval, ChatBuildReview, ChatBuildStart, Error, Result, Store};

use super::{engine, snapshot};

impl Store {
    pub async fn chat_build_review(
        &mut self,
        chat_id: i64,
        node_id: i64,
    ) -> Result<ChatBuildReview> {
        let chat = self.chat(chat_id)?;
        self.validate_build_checkout(&chat).await?;
        let run = self.node_run(node_id)?.run_id;
        let execution = self
            .chat_team_run(run)?
            .ok_or_else(|| Error::invalid("this is not a team execution"))?;
        let path = self.planning_path()?;
        self.db_mut().write(|tx| {
            awaiting(tx, chat_id, node_id, execution.rev)?;
            let store = engine::open(&path, false)?
                .ok_or_else(|| Error::invalid("this chat has no plan"))?;
            let plan = snapshot(&store, &chat)?;
            let (head, dirty) = checkout(Path::new(&chat.workspace_path))?;
            let roster = roster(tx, run)?;
            let roster_revision = roster_revision(&roster)?;
            Ok(ChatBuildReview {
                execution,
                plan,
                roster,
                roster_revision,
                head,
                dirty,
            })
        })
    }

    /// Not a PlanAction and not an agent MCP tool. The authenticated human surface
    /// supplies exactly the review it displayed; answering a question never calls this.
    pub async fn approve_chat_build(
        &mut self,
        chat_id: i64,
        node_id: i64,
        approval: &ChatBuildApproval,
    ) -> Result<ChatBuildStart> {
        let chat = self.chat(chat_id)?;
        self.validate_build_checkout(&chat).await?;
        let path = self.planning_path()?;
        let run = self.run(self.node_run(node_id)?.run_id)?;
        self.db_mut().write(|tx| {
            awaiting(tx, chat_id, node_id, approval.expect_control_revision)?;
            let store = engine::open(&path, false)?.ok_or_else(|| Error::invalid("this chat has no plan"))?;
            let plan = snapshot(&store, &chat)?;
            if plan.revision != approval.expect_plan_revision {
                return Err(Error::invalid("the reviewed plan changed; refresh before approving"));
            }
            let bundle = plan.bundle.ok_or_else(|| Error::invalid("this chat has no plan"))?;
            let ready = approved_slices(&bundle)?;
            let roster = roster(tx, run.id)?;
            if roster_revision(&roster)? != approval.expect_roster_revision { return Err(Error::invalid("the reviewed team changed; refresh before approving")); }
            if !roster.iter().any(|agent| agent.role == crate::VERIFIER_ROLE && agent.enabled && agent.read_only) {
                return Err(Error::invalid("a build needs an enabled read-only verifier"));
            }
            let assignments = ready.iter().map(|slice| routing::maker(&roster, slice, Path::new(&chat.workspace_path))).collect::<Result<Vec<_>>>()?;
            let (head, dirty) = checkout(Path::new(&chat.workspace_path))?;
            if head != approval.expect_head { return Err(Error::invalid("the reviewed checkout HEAD changed; review it again")); }
            if !dirty.is_empty() { return Err(Error::invalid("the checkout is dirty; commit or stash your files explicitly before approving a build")); }
            for (slice, agent) in ready.iter().zip(assignments) {
                tx.execute("INSERT INTO chat_build_slice (run_id, slice_key, planner_slice_id, approved_rev, assigned_agent_id, assigned_agent_rev, agent_snapshot, branch, lease_holder)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![run.id, slice.key, slice.id, slice.rev, agent.id, agent.rev,
                        serde_json::to_string(agent)?, format!("ai-team/chat-{chat_id}/run-{}/slice-{}", run.id, slice.id),
                        format!("ai-team chat-{chat_id} run-{} slice-{}", run.id, slice.id)])?;
            }
            tx.execute("UPDATE chat_team_run SET phase = 'building', base_sha = ?2, approved_revision = ?3, reason = NULL, quiescent = 0, rev = rev + 1 WHERE run_id = ?1",
                params![run.id, head, plan.revision])?;
            tx.execute("UPDATE run SET status = 'running', plan_slug = ?3, blocked_reason = NULL, rev = rev + 1, updated_at = ?2 WHERE id = ?1", params![run.id, crate::now(), bundle.plan.slug])?;
            tx.execute("INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json) VALUES (?1, ?2, ?3, 'note', 'human', 'Approved the reviewed team build', ?4)",
                params![run.id, node_id, crate::now(), serde_json::json!({"head":head,"plan_revision":plan.revision,"roster_revision":approval.expect_roster_revision,"slices":ready.iter().map(|s| (&s.key,s.rev)).collect::<Vec<_>>()}).to_string()])?;
            Ok(ChatBuildStart { chat_id, node_id, run_id: run.id, revision: approval.expect_control_revision + 1 })
        })
    }

    async fn validate_build_checkout(&mut self, chat: &crate::Chat) -> Result<()> {
        let repo = self
            .project_repos(chat.project_id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| Error::invalid("this project has no checkout"))?;
        crate::Worktrees::at(repo)
            .resolve(Path::new(&chat.workspace_path))
            .await?;
        Ok(())
    }
}

fn roster(conn: &Connection, run: i64) -> Result<Vec<crate::Agent>> {
    let team: Option<i64> =
        conn.query_row("SELECT team_id FROM run WHERE id = ?1", [run], |row| {
            row.get(0)
        })?;
    super::super::agents::agents_in(
        conn,
        team.ok_or_else(|| Error::invalid("this execution's team no longer exists"))?,
    )
}

fn roster_revision(agents: &[crate::Agent]) -> Result<String> {
    Ok(crate::update::sha256(&serde_json::to_vec(agents)?))
}

fn awaiting(conn: &Connection, chat: i64, node: i64, revision: i64) -> Result<()> {
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_team_run t JOIN chat c ON c.id = t.chat_id JOIN run r ON r.id = t.run_id
        WHERE t.chat_id = ?1 AND t.control_node_id = ?2 AND t.rev = ?3 AND t.phase = 'awaiting_approval' AND t.approved_revision IS NULL
        AND t.supervisor_pid IS NULL AND c.active_node_id = ?2 AND c.archived = 0 AND c.stop_requested = 0 AND r.status = 'blocked'
        AND NOT EXISTS(SELECT 1 FROM node_run n WHERE n.run_id = r.id AND (n.status IN ('queued','running') OR n.pi_pid IS NOT NULL)))",
        params![chat, node, revision], |row| row.get(0))?;
    if !valid {
        return Err(Error::invalid(
            "this chat is not at the reviewed approval pause; refresh before approving",
        ));
    }
    Ok(())
}

fn approved_slices(bundle: &planner::PlanBundle) -> Result<Vec<&planner::Slice>> {
    if bundle
        .questions
        .iter()
        .any(|question| question.status == "open")
    {
        return Err(Error::invalid(
            "answer the plan's open questions before approving; answers alone do not start a build",
        ));
    }
    if bundle.plan.status.is_terminal() {
        return Err(Error::invalid("a finished plan cannot start a build"));
    }
    let ready: Vec<_> = bundle
        .slices
        .iter()
        .filter(|slice| slice.status == planner::Status::Ready)
        .collect();
    if ready.is_empty() {
        return Err(Error::invalid("this plan has no ready slices to approve"));
    }
    if ready.iter().any(|slice| {
        slice.claimed_by.is_some()
            || slice.branch.is_some()
            || slice.base_branch.is_some()
            || slice.pr_url.is_some()
            || slice.worktree_path.is_some()
    }) {
        return Err(Error::invalid("ready work already has claim/delivery evidence; recover that work or propose a new slice"));
    }
    Ok(ready)
}

/// A status check must not run fsmonitor hooks while holding the team writer lock.
/// The filesystem is not locked: dispatch rechecks it and always uses this exact SHA.
pub(in crate::store) fn checkout(path: &Path) -> Result<(String, String)> {
    let head = git(path, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let dirty = git(
        path,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if head != git(path, &["rev-parse", "--verify", "HEAD^{commit}"])? {
        return Err(Error::invalid(
            "the checkout changed during review; refresh and retry",
        ));
    }
    Ok((head, dirty))
}

pub(super) fn prepared(path: &Path, base: &str, branch: &str) -> Result<()> {
    let (head, dirty) = checkout(path)?;
    if head != base
        || !dirty.is_empty()
        || git(path, &["symbolic-ref", "--short", "HEAD"])? != branch
    {
        return Err(Error::invalid("the lease is not clean on its approved base and draft branch; inspect it before recovering"));
    }
    Ok(())
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(path)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "cannot review checkout: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

pub(super) fn freeze(
    conn: &Connection,
    chat: i64,
    access: PlanAccess,
    action: &PlanAction,
) -> Result<()> {
    let approved: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat c JOIN chat_team_run t ON t.control_node_id = c.active_node_id
        WHERE c.id = ?1 AND t.approved_revision IS NOT NULL AND t.phase != 'finished')", [chat], |row| row.get(0))?;
    if !approved {
        return Ok(());
    }
    if matches!(
        action,
        PlanAction::AppendLog { .. }
            | PlanAction::OpenQuestion { .. }
            | PlanAction::AnswerQuestion { .. }
    ) {
        return Ok(());
    }
    if access == PlanAccess::Maker
        && matches!(
            action,
            PlanAction::SetSliceStatus {
                status: planner::Status::Active
                    | planner::Status::Blocked
                    | planner::Status::InReview,
                ..
            }
        )
    {
        return Ok(());
    }
    Err(Error::invalid("approved work is frozen until this build releases the chat; completion and claim release belong to the controller"))
}
