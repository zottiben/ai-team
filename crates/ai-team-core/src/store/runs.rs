//! Runs, and the node runs under them.

use std::path::Path;

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::machine::{ModelRegistry, ModelResolution};
use crate::model::{
    DeliveryAction, EventKind, NodeRun, NodeStatus, Provider, Run, RunStatus, RunTrigger, Usage,
};
use crate::store::{non_empty, Store};
use crate::util::now;

impl Store {
    /// Start a run. The team's guardrails are copied onto the row here and never read
    /// back through the team again: what a run was allowed to spend is a fact about that
    /// run, and raising a budget tomorrow must not rewrite what happened today.
    pub fn create_run(
        &mut self,
        project_id: i64,
        prompt: &str,
        trigger: RunTrigger,
    ) -> Result<Run> {
        self.create_run_in(project_id, prompt, trigger, None)
    }

    /// Start a run rooted in one checkout.
    ///
    /// The root is not the same as every node's worktree: makers may fan out into leased
    /// worktrees while their run remains visible in the checkout where the operator
    /// started it. Callers that omit it get the project's main checkout.
    pub fn create_run_in(
        &mut self,
        project_id: i64,
        prompt: &str,
        trigger: RunTrigger,
        workspace: Option<&Path>,
    ) -> Result<Run> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(Error::invalid("a run needs a prompt"));
        }
        let project = self.project(project_id)?;
        let guardrails = match project.team_id {
            Some(team_id) => self.team(team_id)?.guardrails,
            None => return Err(Error::NoTeam(project.slug)),
        };
        // Stored resolved, like every checkout path a run or node row holds: they are
        // compared in SQL, where `/var` and `/private/var` are two different strings (D12).
        let workspace_path = match workspace {
            Some(path) => Some(resolved(path)),
            None => self
                .project_repos(project_id)?
                .into_iter()
                .find_map(|repo| repo.main_path)
                .map(|path| resolved(Path::new(&path))),
        };
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO run
                   (project_id, team_id, prompt, trigger, workspace_path, parallel_width,
                    budget_tokens, budget_seconds, max_repairs, budget_tokens_node,
                    budget_seconds_node, max_turns_node, on_failure, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
                params![
                    project_id,
                    project.team_id,
                    prompt,
                    trigger,
                    workspace_path,
                    guardrails.parallel_width,
                    guardrails.budget_tokens_run,
                    guardrails.budget_seconds_run,
                    guardrails.max_repairs,
                    guardrails.budget_tokens_node,
                    guardrails.budget_seconds_node,
                    guardrails.max_turns_node,
                    guardrails.on_failure,
                    at
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.run(id)
    }

    pub fn run(&self, id: i64) -> Result<Run> {
        self.db()
            .conn()
            .query_row(
                &format!("{RUN_SELECT} WHERE id = ?1"),
                params![id],
                run_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchRun(id.to_string()))
    }

    pub fn runs(&self, project_id: Option<i64>, limit: i64) -> Result<Vec<Run>> {
        let (sql, args): (String, Vec<Box<dyn rusqlite::ToSql>>) = match project_id {
            Some(id) => (
                format!("{RUN_SELECT} WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2"),
                vec![Box::new(id), Box::new(limit)],
            ),
            None => (
                format!("{RUN_SELECT} ORDER BY id DESC LIMIT ?1"),
                vec![Box::new(limit)],
            ),
        };
        let mut stmt = self.db().conn().prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), run_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// What one checkout's surfaces are about (PW1).
    ///
    /// The pull request a worktree holds is read from the newest maker turn that built in
    /// it. A worktree holding a PR whose run started somewhere else is that PR's worktree;
    /// anything else - the project's own checkout, one the operator made, a one-PR plan
    /// built in place - is scoped by the runs started in it. The project's checkout is the
    /// top of every tree and never a PR's, whatever an older run did in it.
    pub(crate) fn workspace_scope(&self, workspace: &Path) -> Result<WorkspaceScope> {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let path = workspace.to_string_lossy().into_owned();
        let same = |other: &str| crate::neighbours::same_worktree(other, &path);
        let pr = match self.latest_pr_node_in(&path)? {
            Some(node) => {
                let run = self.run(node.run_id)?;
                let started_here = run.workspace_path.as_deref().is_some_and(same);
                let main = self
                    .project_repos(run.project_id)?
                    .iter()
                    .any(|repo| repo.main_path.as_deref().is_some_and(same));
                if started_here || main {
                    None
                } else {
                    run.plan_slug.zip(node.slice_key)
                }
            }
            None => None,
        };
        Ok(WorkspaceScope { path, pr })
    }

    /// Newest runs that belong to one checkout (see [`WorkspaceScope`]).
    ///
    /// This filters before applying the limit. Filtering a bounded project-wide list in
    /// Rust lets a busy sibling workspace hide this one's latest run entirely.
    pub fn runs_in_workspace(
        &self,
        project_id: i64,
        workspace: &Path,
        limit: i64,
    ) -> Result<Vec<Run>> {
        let scope = self.workspace_scope(workspace)?;
        let (path, plan, slice) = scope.params();
        let mut stmt = self.db().conn().prepare(&format!(
            "{RUN_SELECT} WHERE project_id = ?1 AND {}
             ORDER BY id DESC LIMIT ?5",
            WorkspaceScope::run_sql("run", 2)
        ))?;
        let rows = stmt
            .query_map(params![project_id, path, plan, slice, limit], run_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The plan a checkout is working on: its newest run's that has one (see
    /// [`WorkspaceScope`]), or `None` where ai-team has run nothing with a plan.
    ///
    /// Asked of ai-team's own runs rather than of ai-planner, which answers with whichever
    /// plan it has resolved in the checkout most - an older one, once a checkout has had
    /// a few. The window's board then showed that plan's finished slices, and the PRs the
    /// latest run built, with their Push and Open PR, were nowhere on it (rule 7).
    pub fn plan_in_workspace(&self, project_id: i64, workspace: &Path) -> Result<Option<String>> {
        let scope = self.workspace_scope(workspace)?;
        let (path, plan, slice) = scope.params();
        Ok(self
            .db()
            .conn()
            .query_row(
                &format!(
                    "SELECT plan_slug FROM run WHERE project_id = ?1 AND plan_slug IS NOT NULL
                        AND {}
                      ORDER BY id DESC LIMIT 1",
                    WorkspaceScope::run_sql("run", 2)
                ),
                params![project_id, path, plan, slice],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// The newest maker row a PR has had in any run of its plan: where it stopped and whose
    /// work it was, for a run that picks the PR up again. Asked before that run opens a row
    /// of its own for the PR, so the answer is always an earlier run's.
    pub fn last_turn_on(&self, plan: &str, slice_key: &str) -> Result<Option<NodeRun>> {
        let id: Option<i64> = self
            .db()
            .conn()
            .query_row(
                "SELECT n.id FROM node_run n JOIN run r ON r.id = n.run_id
                  WHERE r.plan_slug = ?1 AND n.slice_key = ?2 AND n.role <> ?3
                  ORDER BY n.id DESC LIMIT 1",
                params![plan, slice_key, crate::VERIFIER_ROLE],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| self.node_run(id)).transpose()
    }

    /// The plans a project's work is in: the newest run's plan in each checkout ai-team has
    /// run in, newest first. Asked of ai-team's runs for the reason
    /// [`Store::plan_in_workspace`] is - ai-planner, asked without a plan, answers with
    /// whichever it has resolved most, which is how Today came to list a finished plan's
    /// questions instead of the one being built (rule 7).
    pub fn plans_in_use(&self, project_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.db().conn().prepare(
            "SELECT plan_slug FROM run
              WHERE id IN (SELECT MAX(id) FROM run
                            WHERE project_id = ?1 AND plan_slug IS NOT NULL
                            GROUP BY workspace_path)
              ORDER BY id DESC",
        )?;
        let mut plans: Vec<String> = Vec::new();
        for plan in stmt.query_map(params![project_id], |row| row.get::<_, String>(0))? {
            let plan = plan?;
            // Two checkouts on the same plan - a run and its follow-up - ask it once.
            if !plans.contains(&plan) {
                plans.push(plan);
            }
        }
        Ok(plans)
    }

    /// A run's rows as seen from one checkout: all of them from a checkout the run started
    /// in, and only the ones that built or checked the PR a PR's worktree holds.
    pub fn nodes_in_workspace(&self, run_id: i64, workspace: &Path) -> Result<Vec<NodeRun>> {
        let scope = self.workspace_scope(workspace)?;
        let run = self.run(run_id)?;
        Ok(self
            .node_runs(run_id)?
            .into_iter()
            .filter(|node| scope.covers(node, &run))
            .collect())
    }

    /// Whether one run belongs to a checkout (see [`WorkspaceScope`]). What the window may
    /// open, reply to, resume or deliver from a checkout is what it lists there.
    pub fn run_in_workspace(&self, run_id: i64, workspace: &Path) -> Result<bool> {
        let scope = self.workspace_scope(workspace)?;
        let (path, plan, slice) = scope.params();
        Ok(self.db().conn().query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM run WHERE id = ?1 AND {})",
                WorkspaceScope::run_sql("run", 2)
            ),
            params![run_id, path, plan, slice],
            |row| row.get(0),
        )?)
    }

    /// The newest run in this checkout blocked for one exact reason.
    ///
    /// This is a direct query rather than a scan of the latest N runs. Crew and Talk must
    /// agree whether approval is pending even after a busy workspace has accumulated more
    /// history than either surface displays.
    pub fn blocked_run_in_workspace(
        &self,
        project_id: i64,
        team_id: i64,
        workspace: &Path,
        reasons: [&str; 2],
    ) -> Result<Option<Run>> {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        self.db()
            .conn()
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE project_id = ?1 AND team_id = ?2
                       AND workspace_path = ?3 AND status = 'blocked'
                       AND plan_slug IS NOT NULL AND blocked_reason IN (?4, ?5)
                     ORDER BY id DESC LIMIT 1"
                ),
                params![
                    project_id,
                    team_id,
                    workspace.to_string_lossy().as_ref(),
                    reasons[0],
                    reasons[1]
                ],
                run_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Newest blocked run for this project/team and exact control-plane reason.
    pub fn blocked_run_for_project(
        &self,
        project_id: i64,
        team_id: i64,
        reasons: [&str; 2],
    ) -> Result<Option<Run>> {
        self.db()
            .conn()
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE project_id = ?1 AND team_id = ?2
                       AND status = 'blocked' AND plan_slug IS NOT NULL
                       AND blocked_reason IN (?3, ?4) ORDER BY id DESC LIMIT 1"
                ),
                params![project_id, team_id, reasons[0], reasons[1]],
                run_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// A control-plane blocked run that owns this exact ai-planner plan, regardless of
    /// which checkout started it. Sibling worktrees may resolve to one plan, so guarding
    /// only by workspace would allow one of them to orphan the other's continuation.
    pub fn blocked_run_for_plan(
        &self,
        project_id: i64,
        team_id: i64,
        plan_slug: &str,
        reasons: [&str; 2],
    ) -> Result<Option<Run>> {
        self.db()
            .conn()
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE project_id = ?1 AND team_id = ?2
                       AND status = 'blocked' AND plan_slug = ?3
                       AND blocked_reason IN (?4, ?5)
                     ORDER BY id DESC LIMIT 1"
                ),
                params![project_id, team_id, plan_slug, reasons[0], reasons[1]],
                run_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// A run already coordinating or building this exact plan. This closes the short
    /// interval before a maker claims the first slice, when the board still looks ready
    /// but another click must not start a duplicate workflow.
    pub fn active_run_for_plan(
        &self,
        project_id: i64,
        team_id: i64,
        plan_slug: &str,
    ) -> Result<Option<Run>> {
        self.db()
            .conn()
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE project_id = ?1 AND team_id = ?2
                       AND plan_slug = ?3 AND status IN ('queued', 'planning', 'running')
                     ORDER BY id DESC LIMIT 1"
                ),
                params![project_id, team_id, plan_slug],
                run_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Reserve this run as the approval owner for its plan before mutating ai-planner.
    /// The conditional update and competing-owner check share one SQLite transaction, so
    /// a legacy adoption in another process cannot pass the same gap.
    pub fn begin_plan_hold(
        &mut self,
        id: i64,
        preparing_reason: &str,
        ready_reason: &str,
    ) -> Result<Run> {
        if self.run(id)?.plan_slug.is_none() {
            let reason = "plan approval requires a plan to approve";
            self.block_run(id, reason)?;
            return Err(Error::invalid(reason));
        }
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE run AS target
                    SET status = 'blocked', blocked_reason = ?2, rev = rev + 1,
                        updated_at = ?4
                  WHERE target.id = ?1 AND target.plan_slug IS NOT NULL
                    AND NOT EXISTS (
                        SELECT 1 FROM run AS owner
                         WHERE owner.id != target.id
                           AND owner.project_id = target.project_id
                           AND owner.team_id = target.team_id
                           AND owner.plan_slug = target.plan_slug
                           AND (
                               owner.status = 'running'
                               OR (owner.status = 'blocked'
                                   AND owner.blocked_reason IN (?2, ?3))
                           )
                    )",
                params![id, preparing_reason, ready_reason, at],
            )?)
        })?;
        if changed == 0 {
            let reason = "another run already owns plan approval for this plan";
            self.block_run(id, reason)?;
            return Err(Error::invalid(reason));
        }
        self.run(id)
    }

    /// Attach a pre-run/session approval board to the blocked orchestrator turn that made
    /// it. If a current run already owns the plan, return that instead. Both decisions are
    /// made in one transaction so another process cannot create two owners.
    pub fn adopt_legacy_plan_approval(
        &mut self,
        project_id: i64,
        team_id: i64,
        workspace: &Path,
        plan_slug: &str,
        preparing_reason: &str,
        ready_reason: &str,
    ) -> Result<Run> {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let path = workspace.to_string_lossy();
        let at = now();
        let id = self.db_mut().write(|tx| {
            let existing = tx
                .query_row(
                    "SELECT id FROM run
                      WHERE project_id = ?1 AND team_id = ?2 AND plan_slug = ?3
                        AND status = 'blocked' AND blocked_reason IN (?4, ?5)
                      ORDER BY id DESC LIMIT 1",
                    params![
                        project_id,
                        team_id,
                        plan_slug,
                        preparing_reason,
                        ready_reason
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                return Ok(id);
            }
            let active = tx
                .query_row(
                    "SELECT id FROM run
                      WHERE project_id = ?1 AND team_id = ?2 AND plan_slug = ?3
                        AND status IN ('queued', 'planning', 'running')
                      ORDER BY id DESC LIMIT 1",
                    params![project_id, team_id, plan_slug],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            if let Some(id) = active {
                return Err(Error::invalid(format!(
                    "run #{id} already owns this plan and is not waiting for approval"
                )));
            }

            let legacy = tx
                .query_row(
                    "SELECT run.id FROM run
                      WHERE run.project_id = ?1 AND run.team_id = ?2
                        AND run.workspace_path = ?3 AND run.status = 'blocked'
                        AND run.plan_slug IS NULL AND run.blocked_reason IS NULL
                        AND EXISTS (
                            SELECT 1 FROM node_run
                             WHERE node_run.run_id = run.id
                               AND node_run.role = 'orchestrator'
                               AND node_run.status = 'done'
                               AND node_run.session_id IS NOT NULL
                               AND node_run.id = (
                                   SELECT MAX(latest.id) FROM node_run AS latest
                                    WHERE latest.run_id = run.id
                                      AND latest.role = 'orchestrator'
                               )
                        )
                      ORDER BY run.id DESC LIMIT 1",
                    params![project_id, team_id, path.as_ref()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    Error::invalid(
                        "this approval board has no orchestrator run that can be continued",
                    )
                })?;
            tx.execute(
                "UPDATE run SET plan_slug = ?2, blocked_reason = ?3,
                                rev = rev + 1, updated_at = ?4
                  WHERE id = ?1 AND plan_slug IS NULL AND status = 'blocked'",
                params![legacy, plan_slug, ready_reason, at],
            )?;
            Ok(legacy)
        })?;
        self.run(id)
    }

    /// Replace one blocked reason only while this run still owns the expected state.
    /// Approval may recover a stale preparing state in another process, so callers must
    /// not overwrite that claim when their external ai-planner operation eventually ends.
    pub fn transition_blocked_run(
        &mut self,
        id: i64,
        expected_reason: &str,
        next_reason: &str,
    ) -> Result<bool> {
        let at = now();
        self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE run SET blocked_reason = ?3, rev = rev + 1, updated_at = ?4
                  WHERE id = ?1 AND status = 'blocked' AND blocked_reason = ?2",
                params![id, expected_reason, next_reason, at],
            )? == 1)
        })
    }

    pub fn set_run_status(&mut self, id: i64, status: RunStatus) -> Result<Run> {
        let at = now();
        self.db_mut().write(|tx| {
            // started_at and ended_at are set from the status rather than by the caller,
            // so a run can never be `done` with no end time.
            let changed = tx.execute(
                "UPDATE run
                    SET status = ?2,
                        started_at = CASE WHEN started_at IS NULL AND ?2 IN ('planning','running')
                                          THEN ?3 ELSE started_at END,
                        ended_at   = CASE WHEN ?2 IN ('done','failed','cancelled')
                                          THEN ?3 ELSE NULL END,
                        rev = rev + 1,
                        updated_at = ?3
                  WHERE id = ?1",
                params![id, status, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchRun(id.to_string()));
            }
            Ok(())
        })?;
        self.run(id)
    }

    /// Close a blocked run whose stopped work a later run has finished: done, with nothing
    /// left blocking it, and ended when it stopped rather than now. Only while it is still
    /// blocked, so a run somebody took back in the meantime stays theirs.
    pub fn close_blocked_run(&mut self, id: i64) -> Result<bool> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE run SET status = 'done', blocked_reason = NULL,
                                ended_at = COALESCE(ended_at, ?2), rev = rev + 1, updated_at = ?2
                  WHERE id = ?1 AND status = 'blocked'",
                params![id, at],
            )?)
        })?;
        Ok(changed > 0)
    }

    pub fn fail_run(&mut self, id: i64, reason: &str) -> Result<Run> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE run SET status = 'failed', blocked_reason = ?2, ended_at = ?3,
                                rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![id, reason, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchRun(id.to_string()));
            }
            Ok(())
        })?;
        self.run(id)
    }

    pub fn block_run(&mut self, id: i64, reason: &str) -> Result<Run> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE run SET status = 'blocked', blocked_reason = ?2, rev = rev + 1,
                                updated_at = ?3
                  WHERE id = ?1",
                params![id, reason, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchRun(id.to_string()));
            }
            Ok(())
        })?;
        self.run(id)
    }

    /// Atomically claim a plan-approval continuation. Two windows may render the same
    /// button; only the first is allowed to resume and dispatch the run.
    pub fn begin_plan_approval(
        &mut self,
        id: i64,
        expected_reason: &str,
        not_newer_than: Option<&str>,
    ) -> Result<Run> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE run
                    SET status = 'running', blocked_reason = NULL, rev = rev + 1,
                        updated_at = ?4
                  WHERE id = ?1 AND status = 'blocked' AND blocked_reason = ?2
                    AND plan_slug IS NOT NULL
                    AND (?3 IS NULL OR updated_at <= ?3)",
                params![id, expected_reason, not_newer_than, at],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid(
                "that run is not waiting for plan approval; refresh its current state",
            ));
        }
        self.run(id)
    }

    /// Atomically claim plan approval and put the person's direction into the exact
    /// orchestrator conversation. If either write fails, the run remains approval-held.
    pub fn begin_plan_approval_with_direction(
        &mut self,
        id: i64,
        expected_reason: &str,
        node_run_id: i64,
        agent_id: i64,
        body: &str,
        not_newer_than: Option<&str>,
    ) -> Result<Run> {
        let body = body.trim();
        if body.is_empty() {
            return Err(Error::invalid("say something"));
        }
        let node = self.node_run(node_run_id)?;
        if node.run_id != id || node.agent_id != Some(agent_id) {
            return Err(Error::invalid(
                "that orchestrator conversation does not belong to this run",
            ));
        }
        let at = now();
        let summary = super::conversation_summary(body);
        let payload = serde_json::to_string(&serde_json::json!({
            "conversation": "reply",
            "body": body,
        }))?;
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE run
                    SET status = 'running', blocked_reason = NULL, rev = rev + 1,
                        updated_at = ?4
                  WHERE id = ?1 AND status = 'blocked' AND blocked_reason = ?2
                    AND plan_slug IS NOT NULL
                    AND (?3 IS NULL OR updated_at <= ?3)",
                params![id, expected_reason, not_newer_than, at],
            )?;
            if changed == 0 {
                return Err(Error::invalid(
                    "that run is not waiting for plan approval; refresh its current state",
                ));
            }
            tx.execute(
                "INSERT INTO pending_message (agent_id, node_run_id, body, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![agent_id, node_run_id, body, at],
            )?;
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                 VALUES (?1, ?2, ?3, 'note', 'human', ?4, ?5)",
                params![id, node_run_id, at, summary, payload],
            )?;
            Ok(())
        })?;
        self.run(id)
    }

    /// Let this run land work on the default branch itself (PW2). Only ever switched on,
    /// at creation, because the operator asked for it for this run.
    pub fn set_run_on_default_branch(&mut self, id: i64) -> Result<Run> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE run SET on_default_branch = 1, rev = rev + 1, updated_at = ?2
                  WHERE id = ?1",
                params![id, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchRun(id.to_string()));
            }
            Ok(())
        })?;
        self.run(id)
    }

    /// A run still coordinating or building in this checkout, driven by a live process.
    ///
    /// Asked before a run row exists, so a refusal leaves nothing behind to explain.
    /// `alive` decides whether a recorded supervisor is still running: a crashed run never
    /// marks itself finished, and must not hold the checkout forever.
    pub fn busy_run_in_workspace(
        &self,
        project_id: i64,
        workspace: &Path,
        alive: impl Fn(i64) -> bool,
    ) -> Result<Option<Run>> {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let path = workspace.to_string_lossy();
        let mut stmt = self.db().conn().prepare(&format!(
            "{RUN_SELECT} WHERE project_id = ?1 AND workspace_path = ?2
               AND status IN ('queued', 'planning', 'running')
               AND supervisor_pid IS NOT NULL
             ORDER BY id DESC"
        ))?;
        let runs = stmt
            .query_map(params![project_id, path.as_ref()], run_from_row)?
            .collect::<rusqlite::Result<Vec<Run>>>()?;
        Ok(runs
            .into_iter()
            .find(|run| run.supervisor_pid.is_some_and(&alive)))
    }

    /// Record this process as the one driving a run, unless another live run already
    /// holds the same checkout.
    ///
    /// One transaction, so two windows starting a run in one checkout at the same moment
    /// cannot both win: the second waits on the write lock and then sees the first.
    pub fn supervise_run(&mut self, id: i64, pid: i64, alive: impl Fn(i64) -> bool) -> Result<()> {
        let at = now();
        self.db_mut().write(|tx| {
            let (project_id, workspace): (i64, Option<String>) = tx
                .query_row(
                    "SELECT project_id, workspace_path FROM run WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| Error::NoSuchRun(id.to_string()))?;
            if let Some(workspace) = workspace {
                let mut stmt = tx.prepare(
                    "SELECT id, status, supervisor_pid FROM run
                      WHERE project_id = ?1 AND workspace_path = ?2 AND id != ?3
                        AND status IN ('queued', 'planning', 'running')
                        AND supervisor_pid IS NOT NULL",
                )?;
                let holders = stmt
                    .query_map(params![project_id, workspace, id], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?
                    .collect::<rusqlite::Result<Vec<(i64, RunStatus, i64)>>>()?;
                if let Some((other, status, other_pid)) =
                    holders.into_iter().find(|(_, _, pid)| alive(*pid))
                {
                    return Err(Error::invalid(workspace_busy(other, status, other_pid)));
                }
            }
            tx.execute(
                "UPDATE run SET supervisor_pid = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![id, pid, at],
            )?;
            Ok(())
        })
    }

    /// Record the process driving a run that does not hold its checkout: a review
    /// follow-up works in a pull request's own worktree, and runs even while another run
    /// is building in the checkout above it.
    pub fn set_run_supervisor(&mut self, id: i64, pid: i64) -> Result<()> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE run SET supervisor_pid = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![id, pid, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchRun(id.to_string()));
            }
            Ok(())
        })
    }

    pub fn set_run_plan(&mut self, id: i64, plan_slug: &str) -> Result<Run> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE run SET plan_slug = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![id, plan_slug, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchRun(id.to_string()));
            }
            Ok(())
        })?;
        self.run(id)
    }

    /// Attach an immediately-built plan only if no other live run already owns it.
    /// UI mutexes prevent ordinary double-clicks, while this transaction closes the
    /// cross-process race between a desktop window, browser window, CLI, or daemon.
    pub fn claim_run_plan(&mut self, id: i64, plan_slug: &str) -> Result<Run> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE run AS target
                    SET plan_slug = ?2, rev = rev + 1, updated_at = ?3
                  WHERE target.id = ?1
                    AND NOT EXISTS (
                        SELECT 1 FROM run AS other
                         WHERE other.id != target.id
                           AND other.project_id = target.project_id
                           AND other.team_id = target.team_id
                           AND other.plan_slug = ?2
                           AND other.status IN ('queued', 'planning', 'running')
                    )",
                params![id, plan_slug, at],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid(
                "another active run already owns this plan; open that run instead",
            ));
        }
        self.run(id)
    }

    /// Dispatch one slice to one seat.
    ///
    /// `role`, `provider` and `model` are copied onto the row rather than joined from
    /// `agent`: an agent can be renamed, repointed or deleted, and a year-old run must
    /// still be able to say who did the work and on what.
    pub fn dispatch(
        &mut self,
        run_id: i64,
        agent_id: i64,
        slice_key: Option<&str>,
        registry: &ModelRegistry,
    ) -> Result<NodeRun> {
        let agent = self.agent(agent_id)?;
        // The picker and generated project are not a permission boundary. Resolve again
        // here so a scheduled, unattended run cannot reach a denied account (D8).
        let resolution = registry.resolve(&agent)?;
        self.dispatch_with_resolution(run_id, agent_id, slice_key, None, &resolution)
    }

    /// Dispatch one task of a PR (PW4). The task key is a reference into the slice's
    /// scope, like the slice key is into the plan, and attempts are counted per task: the
    /// second task a seat builds in a PR is not its second try at the first.
    pub fn dispatch_task(
        &mut self,
        run_id: i64,
        agent_id: i64,
        slice_key: &str,
        task_key: Option<&str>,
        registry: &ModelRegistry,
    ) -> Result<NodeRun> {
        let agent = self.agent(agent_id)?;
        let resolution = registry.resolve(&agent)?;
        self.dispatch_with_resolution(run_id, agent_id, Some(slice_key), task_key, &resolution)
    }

    /// Dispatch with an explicit runtime resolution after a recognized account quota
    /// boundary. The roster remains unchanged; this row records what this attempt truly
    /// used.
    pub(crate) fn dispatch_with_resolution(
        &mut self,
        run_id: i64,
        agent_id: i64,
        slice_key: Option<&str>,
        task_key: Option<&str>,
        resolution: &ModelResolution,
    ) -> Result<NodeRun> {
        let agent = self.agent(agent_id)?;
        let at = now();

        // A retry is a new row, not an edit. Losing the first attempt would lose the
        // evidence of what went wrong, which is what analytics is made of.
        let attempt: i64 = self.db().conn().query_row(
            "SELECT COALESCE(MAX(attempt) + 1, 1) FROM node_run
              WHERE run_id = ?1 AND agent_id = ?2 AND COALESCE(slice_key, '') = COALESCE(?3, '')
                AND COALESCE(task_key, '') = COALESCE(?4, '')",
            params![run_id, agent_id, slice_key, task_key],
            |r| r.get(0),
        )?;

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO node_run
                   (run_id, agent_id, role, provider, model, attempt, slice_key, task_key,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    run_id,
                    agent_id,
                    agent.role,
                    resolution.provider,
                    resolution.model,
                    attempt,
                    slice_key,
                    task_key,
                    at
                ],
            )?;
            let node_id = tx.last_insert_rowid();
            if let Some(summary) = resolution.notice() {
                let payload = serde_json::to_string(&serde_json::json!({
                    "requested": {
                        "provider": resolution.requested_provider,
                        "model": resolution.requested_model,
                    },
                    "selected": {
                        "provider": resolution.provider,
                        "model": resolution.model,
                    },
                    "reason": resolution.fallback_reason,
                }))?;
                tx.execute(
                    "INSERT INTO event
                       (run_id, node_run_id, at, kind, actor, summary, payload_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        run_id,
                        node_id,
                        at,
                        EventKind::Note,
                        agent.role,
                        summary,
                        payload
                    ],
                )?;
            }
            Ok(node_id)
        })?;

        self.node_run(id)
    }

    /// The newest maker turn that built a pull request in a worktree, if any did.
    ///
    /// How the window learns which PR a leased worktree holds, and which run's checkout
    /// it belongs under, without keeping a table of its own: the rows that did the work
    /// already say where they did it. A maker's row names the branch it built on; the
    /// verifier's check of the same PR names the PR but builds nothing.
    pub fn latest_pr_node_in(&self, worktree: &str) -> Result<Option<NodeRun>> {
        self.db()
            .conn()
            .query_row(
                &format!(
                    "{NODE_SELECT} WHERE worktree_path = ?1 AND slice_key IS NOT NULL
                       AND branch IS NOT NULL
                     ORDER BY id DESC LIMIT 1"
                ),
                params![worktree],
                node_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn node_run(&self, id: i64) -> Result<NodeRun> {
        self.db()
            .conn()
            .query_row(
                &format!("{NODE_SELECT} WHERE id = ?1"),
                params![id],
                node_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchNodeRun(id.to_string()))
    }

    pub fn node_runs(&self, run_id: i64) -> Result<Vec<NodeRun>> {
        let mut stmt = self
            .db()
            .conn()
            .prepare(&format!("{NODE_SELECT} WHERE run_id = ?1 ORDER BY id"))?;
        let rows = stmt
            .query_map(params![run_id], node_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Record the worktree a node is working in, and the lease that protects it (D3).
    pub fn attach_worktree(
        &mut self,
        node_run_id: i64,
        worktree_path: &str,
        branch: Option<&str>,
        lease_id: Option<&str>,
    ) -> Result<NodeRun> {
        let worktree_path = resolved(Path::new(worktree_path));
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET worktree_path = ?2, branch = ?3, lease_id = ?4,
                                     rev = rev + 1, updated_at = ?5
                  WHERE id = ?1",
                params![node_run_id, worktree_path, branch, lease_id, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    /// Atomically claim one publishing boundary. The prerequisite is part of the write,
    /// so two app windows cannot both push/create/merge the same branch.
    pub fn claim_delivery(&mut self, node_run_id: i64, action: DeliveryAction) -> Result<bool> {
        let node = self.node_run(node_run_id)?;
        let already_done = match action {
            DeliveryAction::Push => node.pushed_at.is_some(),
            DeliveryAction::Pr => node.pr_url.is_some(),
            DeliveryAction::Merge => node.merge_requested_at.is_some(),
        };
        if already_done {
            return Ok(false);
        }
        let prerequisite = match action {
            DeliveryAction::Push => "1 = 1",
            DeliveryAction::Pr => "pushed_at IS NOT NULL",
            DeliveryAction::Merge => "pr_url IS NOT NULL",
        };
        let sql = format!(
            "UPDATE node_run
                SET delivery_claim = ?2, delivery_claimed_at = ?3, delivery_error = NULL,
                    rev = rev + 1, updated_at = ?3
              WHERE id = ?1 AND status = 'done' AND branch IS NOT NULL
                AND (delivery_claim IS NULL OR delivery_claimed_at <= ?4)
                AND {prerequisite}"
        );
        let at = now();
        let stale = crate::util::rfc3339_in(-5 * 60);
        let changed = self
            .db_mut()
            .write(|tx| Ok(tx.execute(&sql, params![node_run_id, action, at, stale])?))?;
        if changed == 0 {
            return Err(Error::invalid(format!(
                "{} is not ready for {} delivery",
                node.slice_key.as_deref().unwrap_or("that node"),
                action.as_str()
            )));
        }
        Ok(true)
    }

    pub fn complete_delivery(
        &mut self,
        node_run_id: i64,
        action: DeliveryAction,
        value: Option<&str>,
    ) -> Result<NodeRun> {
        let at = now();
        let (column, stored) = match action {
            DeliveryAction::Push => ("pushed_at", at.as_str()),
            DeliveryAction::Pr => (
                "pr_url",
                value.ok_or_else(|| Error::invalid("a PR needs its URL"))?,
            ),
            DeliveryAction::Merge => ("merge_requested_at", at.as_str()),
        };
        let sql = format!(
            "UPDATE node_run SET {column} = ?2, delivery_claim = NULL,
                    delivery_claimed_at = NULL, delivery_error = NULL,
                    rev = rev + 1, updated_at = ?3
              WHERE id = ?1 AND delivery_claim = ?4"
        );
        let changed = self
            .db_mut()
            .write(|tx| Ok(tx.execute(&sql, params![node_run_id, stored, at, action])?))?;
        if changed == 0 {
            return Err(Error::invalid("that delivery action is no longer claimed"));
        }
        self.node_run(node_run_id)
    }

    pub fn fail_delivery(
        &mut self,
        node_run_id: i64,
        action: DeliveryAction,
        reason: &str,
    ) -> Result<NodeRun> {
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE node_run SET delivery_claim = NULL, delivery_claimed_at = NULL,
                        delivery_error = ?3, rev = rev + 1, updated_at = ?4
                  WHERE id = ?1 AND delivery_claim = ?2",
                params![node_run_id, action, reason, now()],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid("that delivery action is no longer claimed"));
        }
        self.node_run(node_run_id)
    }

    /// Carry a seat's conversation onto its next row in the same PR.
    ///
    /// A retry is a new row (D2), and a new row starts a new Pi session - which hands a
    /// seat repairing its own work, or building its second task, a prompt and none of the
    /// conversation behind it. So the session moves forward, and its stream cursor with
    /// it: events are keyed `<session>:<index>`, and a row that restarted the index at 0
    /// would collide with the rows before it and have its events silently ignored (rule
    /// 12). A retired session stays retired; the row starts fresh.
    pub fn continue_session(&mut self, node_run_id: i64, from: i64) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE node_run
                    SET session_id = (SELECT session_id FROM node_run WHERE id = ?2),
                        stream_cursor = (SELECT stream_cursor FROM node_run WHERE id = ?2),
                        context_tokens = (SELECT context_tokens FROM node_run WHERE id = ?2),
                        rev = rev + 1, updated_at = ?3
                  WHERE id = ?1
                    AND EXISTS (SELECT 1 FROM node_run
                                 WHERE id = ?2 AND session_id IS NOT NULL
                                   AND session_retired_at IS NULL
                                   AND session_resetting_at IS NULL)",
                params![node_run_id, from, at],
            )?;
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    pub fn set_node_session(&mut self, node_run_id: i64, session_id: &str) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run
                    SET session_retired_at = NULL,
                        session_resetting_at = CASE
                            WHEN session_id = ?2 THEN session_resetting_at
                            ELSE NULL
                        END,
                        context_tokens = CASE
                            WHEN session_id = ?2 AND session_retired_at IS NULL
                                THEN context_tokens
                            ELSE NULL
                        END,
                        session_id = ?2,
                        rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![node_run_id, session_id, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    pub fn claim_node_session_reset(&mut self, node_run_id: i64) -> Result<NodeRun> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE node_run
                    SET session_resetting_at = ?2, rev = rev + 1, updated_at = ?2
                  WHERE id = ?1 AND session_id IS NOT NULL
                    AND session_retired_at IS NULL AND session_resetting_at IS NULL
                    AND status NOT IN ('queued', 'running')",
                params![node_run_id, at],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid(
                "that node has no idle active Pi session to reset",
            ));
        }
        self.node_run(node_run_id)
    }

    pub fn cancel_node_session_reset(&mut self, node_run_id: i64) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE node_run SET session_resetting_at = NULL, rev = rev + 1,
                                     updated_at = ?2 WHERE id = ?1",
                params![node_run_id, at],
            )?;
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    pub fn retire_node_session(&mut self, node_run_id: i64) -> Result<NodeRun> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE node_run
                    SET session_retired_at = ?2, session_resetting_at = NULL,
                        rev = rev + 1, updated_at = ?2
                  WHERE id = ?1 AND session_id IS NOT NULL AND session_retired_at IS NULL",
                params![node_run_id, at],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid(
                "that node has no active Pi session to reset",
            ));
        }
        self.node_run(node_run_id)
    }

    pub fn set_node_status(&mut self, node_run_id: i64, status: NodeStatus) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run
                    SET status = ?2,
                        started_at = CASE WHEN started_at IS NULL AND ?2 = 'running'
                                          THEN ?3 ELSE started_at END,
                        ended_at   = CASE WHEN ?2 IN ('done','failed','cancelled')
                                          THEN ?3 ELSE NULL END,
                        supervisor_pid = CASE WHEN ?2 = 'running'
                                              THEN supervisor_pid ELSE NULL END,
                        rev = rev + 1,
                        updated_at = ?3
                  WHERE id = ?1",
                params![node_run_id, status, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    /// Settle a node as failed, saying why. `blocked_reason` is what every surface reads
    /// for why a turn did not finish; a failed row without one is a failure nobody can act on.
    pub fn fail_node(&mut self, node_run_id: i64, reason: &str) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET status = 'failed', blocked_reason = ?2, ended_at = ?3,
                                     supervisor_pid = NULL, rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![node_run_id, reason, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    /// Claim supervision of one running node with a compare-and-swap on its previous
    /// owner. A restarted app passes the dead pid it observed; two windows cannot both
    /// turn that observation into a live continuation.
    pub fn claim_node_supervision(
        &mut self,
        node_run_id: i64,
        supervisor_pid: i64,
        previous_pid: Option<i64>,
    ) -> Result<NodeRun> {
        let node = self.node_run(node_run_id)?;
        if node.supervisor_pid == Some(supervisor_pid) {
            return Ok(node);
        }
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE node_run
                    SET supervisor_pid = ?2, rev = rev + 1, updated_at = ?4
                  WHERE id = ?1 AND status = 'running'
                    AND session_retired_at IS NULL
                    AND supervisor_pid IS ?3",
                params![node_run_id, supervisor_pid, previous_pid, now()],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid(
                "that turn is already supervised or is no longer running",
            ));
        }
        self.node_run(node_run_id)
    }

    /// Give an interrupted node back without closing its session or discarding its
    /// worktree. The pid predicate prevents an old recovery task from clearing a newer
    /// supervisor's claim.
    pub fn release_node_supervision(
        &mut self,
        node_run_id: i64,
        supervisor_pid: i64,
    ) -> Result<NodeRun> {
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE node_run
                    SET supervisor_pid = NULL, rev = rev + 1, updated_at = ?3
                  WHERE id = ?1 AND supervisor_pid = ?2",
                params![node_run_id, supervisor_pid, now()],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::invalid(
                "that process no longer supervises this turn",
            ));
        }
        self.node_run(node_run_id)
    }

    /// Mark a node blocked. Its siblings are untouched on purpose: a failed node fails
    /// its own branch without poisoning the run (M2-S10).
    /// Record where this node's supervised eve is listening, and the secret it checks.
    ///
    /// Written as soon as the process serves, so a window opened mid-run can reach an
    /// agent the terminal started - including to answer a question it is parked on.
    pub fn attach_eve(&mut self, node_run_id: i64, port: u16, token: &str) -> Result<()> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET eve_port = ?2, eve_token = ?3, rev = rev + 1,
                        updated_at = ?4
                  WHERE id = ?1",
                params![node_run_id, i64::from(port), token, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })
    }

    pub fn block_node(&mut self, node_run_id: i64, reason: &str) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET status = 'blocked', blocked_reason = ?2, ended_at = ?3,
                                     rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![node_run_id, reason, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    /// Add one turn's usage. Additive rather than absolute, because eve reports per-step
    /// and a caller that had to track the running total would eventually get it wrong.
    pub fn record_usage(&mut self, node_run_id: i64, usage: Usage, turns: i64) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run
                    SET tokens_in          = tokens_in + ?2,
                        tokens_out         = tokens_out + ?3,
                        tokens_cache_read  = tokens_cache_read + ?4,
                        tokens_cache_write = tokens_cache_write + ?5,
                        turns              = turns + ?6,
                        rev = rev + 1,
                        updated_at = ?7
                  WHERE id = ?1",
                params![
                    node_run_id,
                    usage.tokens_in,
                    usage.tokens_out,
                    usage.cache_read,
                    usage.cache_write,
                    turns,
                    at
                ],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }

    /// Everything a run has spent, cache reads kept apart.
    pub fn run_usage(&self, run_id: i64) -> Result<Usage> {
        self.db()
            .conn()
            .query_row(
                "SELECT COALESCE(SUM(tokens_in), 0), COALESCE(SUM(tokens_out), 0),
                        COALESCE(SUM(tokens_cache_read), 0), COALESCE(SUM(tokens_cache_write), 0)
                   FROM node_run WHERE run_id = ?1",
                params![run_id],
                |r| {
                    Ok(Usage {
                        tokens_in: r.get(0)?,
                        tokens_out: r.get(1)?,
                        cache_read: r.get(2)?,
                        cache_write: r.get(3)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Has this run spent past what it was allowed? Checked before dispatching the next
    /// node rather than after the fact, which is the only point at which stopping helps.
    pub fn run_over_budget(&self, run_id: i64) -> Result<bool> {
        let run = self.run(run_id)?;
        let Some(budget) = run.budget_tokens else {
            return Ok(false);
        };
        Ok(self.run_usage(run_id)?.billable() >= budget)
    }
}

/// A checkout path as the filesystem resolves it, or as written when it does not exist.
pub(crate) fn resolved(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Which rows one checkout's surfaces are about (PW1).
///
/// A checkout a run started in owns that run: its orchestrator, its planner, and every
/// pull request it built, wherever each was built. A pull request's worktree starts
/// nothing - it holds one PR, and owns only the rows that built that PR there: its makers'
/// turns and the verifier's checks. So the run list there is the runs that built it, and
/// its crew, spend and gates are that PR's, not the whole run's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceScope {
    path: String,
    /// The plan and slice of the pull request this worktree holds, when it is one's.
    pr: Option<(String, String)>,
}

impl WorkspaceScope {
    /// Path, plan and slice, bound in that order to the numbered parameters the SQL below
    /// is written with. Plan and slice are NULL for a checkout that is not a PR's.
    pub(crate) fn params(&self) -> (&str, Option<&str>, Option<&str>) {
        (
            &self.path,
            self.pr.as_ref().map(|(plan, _)| plan.as_str()),
            self.pr.as_ref().map(|(_, slice)| slice.as_str()),
        )
    }

    /// Whether run `run` belongs: it started here, or it built the PR held here, here.
    /// Takes the path, plan and slice as `?first`, `?first + 1` and `?first + 2`.
    pub(crate) fn run_sql(run: &str, first: usize) -> String {
        let (path, plan, slice) = (first, first + 1, first + 2);
        format!(
            "({run}.workspace_path = ?{path}
              OR {run}.id IN (SELECT built.run_id FROM node_run built
                               JOIN run owner ON owner.id = built.run_id
                              WHERE built.worktree_path = ?{path}
                                AND built.slice_key = ?{slice}
                                AND owner.plan_slug = ?{plan}))"
        )
    }

    /// Whether node `node` of run `run` belongs: for a PR's worktree, the rows that built
    /// or checked that PR there; for any other checkout, every row of the runs started in
    /// it. Parameters as for [`WorkspaceScope::run_sql`].
    pub(crate) fn node_sql(node: &str, run: &str, first: usize) -> String {
        let (path, plan, slice) = (first, first + 1, first + 2);
        format!(
            "(CASE WHEN ?{slice} IS NULL THEN {run}.workspace_path = ?{path}
                   ELSE {node}.worktree_path = ?{path} AND {node}.slice_key = ?{slice}
                        AND {run}.plan_slug = ?{plan} END)"
        )
    }

    /// The same question as [`WorkspaceScope::node_sql`], for rows already read.
    pub(crate) fn covers(&self, node: &NodeRun, run: &Run) -> bool {
        match &self.pr {
            None => run
                .workspace_path
                .as_deref()
                .is_some_and(|root| crate::neighbours::same_worktree(root, &self.path)),
            Some((plan, slice)) => {
                node.worktree_path
                    .as_deref()
                    .is_some_and(|here| crate::neighbours::same_worktree(here, &self.path))
                    && node.slice_key.as_deref() == Some(slice.as_str())
                    && run.plan_slug.as_deref() == Some(plan.as_str())
            }
        }
    }
}

/// Why a checkout cannot take a second run yet. Shared by the check made before a run row
/// exists and the one that closes the race after.
pub(crate) fn workspace_busy(run_id: i64, status: RunStatus, pid: i64) -> String {
    format!(
        "run #{run_id} is still {} in this checkout (process {pid}). Starting another here \
         would switch its branch underneath it; let it finish, or stop that process first.",
        status.as_str()
    )
}

const RUN_SELECT: &str = "SELECT id, project_id, team_id, prompt, status, trigger, plan_slug, \
     workspace_path, parallel_width, budget_tokens, budget_seconds, max_repairs, \
     budget_tokens_node, budget_seconds_node, max_turns_node, on_failure, blocked_reason, \
     started_at, ended_at, rev, created_at, updated_at, on_default_branch, supervisor_pid \
     FROM run";

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        project_id: r.get(1)?,
        team_id: r.get(2)?,
        prompt: r.get(3)?,
        status: r.get(4)?,
        trigger: r.get(5)?,
        plan_slug: non_empty(r.get(6)?),
        workspace_path: non_empty(r.get(7)?),
        parallel_width: r.get(8)?,
        budget_tokens: r.get(9)?,
        budget_seconds: r.get(10)?,
        max_repairs: r.get(11)?,
        budget_tokens_node: r.get(12)?,
        budget_seconds_node: r.get(13)?,
        max_turns_node: r.get(14)?,
        on_failure: r.get(15)?,
        blocked_reason: non_empty(r.get(16)?),
        started_at: non_empty(r.get(17)?),
        ended_at: non_empty(r.get(18)?),
        rev: r.get(19)?,
        created_at: r.get(20)?,
        updated_at: r.get(21)?,
        on_default_branch: r.get(22)?,
        supervisor_pid: r.get(23)?,
    })
}

const NODE_SELECT: &str = "SELECT id, run_id, agent_id, role, provider, model, status, attempt, \
     slice_key, worktree_path, branch, lease_id, session_id, eve_port, eve_token, stream_cursor, \
     tokens_in, tokens_out, tokens_cache_read, tokens_cache_write, turns, blocked_reason, \
     started_at, ended_at, rev, created_at, updated_at, context_tokens, session_retired_at, \
     session_resetting_at, pushed_at, pr_url, merge_requested_at, delivery_claim, \
     delivery_claimed_at, delivery_error, supervisor_pid, task_key, push_replaces FROM node_run";

fn node_from_row(r: &Row<'_>) -> rusqlite::Result<NodeRun> {
    Ok(NodeRun {
        id: r.get(0)?,
        run_id: r.get(1)?,
        agent_id: r.get(2)?,
        role: r.get(3)?,
        provider: r.get::<_, String>(4)?.parse().unwrap_or(Provider::Local),
        model: r.get(5)?,
        status: r.get(6)?,
        attempt: r.get(7)?,
        slice_key: non_empty(r.get(8)?),
        worktree_path: non_empty(r.get(9)?),
        branch: non_empty(r.get(10)?),
        lease_id: non_empty(r.get(11)?),
        pushed_at: non_empty(r.get(30)?),
        pr_url: non_empty(r.get(31)?),
        merge_requested_at: non_empty(r.get(32)?),
        delivery_claim: non_empty(r.get(33)?),
        delivery_claimed_at: non_empty(r.get(34)?),
        delivery_error: non_empty(r.get(35)?),
        push_replaces: non_empty(r.get(38)?),
        session_id: non_empty(r.get(12)?),
        session_retired_at: non_empty(r.get(28)?),
        session_resetting_at: non_empty(r.get(29)?),
        supervisor_pid: r.get(36)?,
        task_key: non_empty(r.get(37)?),
        context_tokens: r.get(27)?,
        eve_port: r.get(13)?,
        eve_token: non_empty(r.get(14)?),
        stream_cursor: r.get(15)?,
        usage: Usage {
            tokens_in: r.get(16)?,
            tokens_out: r.get(17)?,
            cache_read: r.get(18)?,
            cache_write: r.get(19)?,
        },
        turns: r.get(20)?,
        blocked_reason: non_empty(r.get(21)?),
        started_at: non_empty(r.get(22)?),
        ended_at: non_empty(r.get(23)?),
        rev: r.get(24)?,
        created_at: r.get(25)?,
        updated_at: r.get(26)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewProject;

    fn seeded() -> (Store, i64, i64) {
        let mut s = Store::memory().unwrap();
        let project = s
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = s
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        (s, project.id, team.id)
    }

    fn backend(s: &Store, team_id: i64) -> i64 {
        s.agents(team_id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap()
            .id
    }

    #[test]
    fn interrupted_node_supervision_is_claimed_once_and_only_its_owner_can_release_it() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = s
            .dispatch(
                run.id,
                backend(&s, team),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        s.set_node_session(node.id, "session-1").unwrap();
        s.set_node_status(node.id, NodeStatus::Running).unwrap();

        let claimed = s.claim_node_supervision(node.id, 101, None).unwrap();
        assert_eq!(claimed.supervisor_pid, Some(101));
        assert!(s.claim_node_supervision(node.id, 202, None).is_err());
        assert!(s.release_node_supervision(node.id, 202).is_err());

        let released = s.release_node_supervision(node.id, 101).unwrap();
        assert_eq!(released.supervisor_pid, None);
        let reclaimed = s.claim_node_supervision(node.id, 202, None).unwrap();
        assert_eq!(reclaimed.supervisor_pid, Some(202));
        let done = s.set_node_status(node.id, NodeStatus::Done).unwrap();
        assert_eq!(done.supervisor_pid, None);
    }

    #[test]
    fn delivery_boundaries_are_ordered_claimed_and_idempotent() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = s
            .dispatch(
                run.id,
                backend(&s, team),
                Some("S1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        s.attach_worktree(node.id, "/tmp/widget", Some("ai-team/s1"), None)
            .unwrap();
        s.set_node_status(node.id, NodeStatus::Done).unwrap();

        assert!(s.claim_delivery(node.id, DeliveryAction::Pr).is_err());
        assert!(s.claim_delivery(node.id, DeliveryAction::Push).unwrap());
        assert!(s.claim_delivery(node.id, DeliveryAction::Push).is_err());
        s.complete_delivery(node.id, DeliveryAction::Push, None)
            .unwrap();
        assert!(!s.claim_delivery(node.id, DeliveryAction::Push).unwrap());
        assert!(s.claim_delivery(node.id, DeliveryAction::Pr).unwrap());
        s.complete_delivery(
            node.id,
            DeliveryAction::Pr,
            Some("https://github.com/acme/widget/pull/7"),
        )
        .unwrap();
        assert!(s.claim_delivery(node.id, DeliveryAction::Merge).unwrap());
        let merged = s
            .complete_delivery(node.id, DeliveryAction::Merge, None)
            .unwrap();
        assert!(merged.pushed_at.is_some());
        assert_eq!(
            merged.pr_url.as_deref(),
            Some("https://github.com/acme/widget/pull/7")
        );
        assert!(merged.merge_requested_at.is_some());
    }

    #[test]
    fn a_run_snapshots_the_teams_guardrails() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship the picker", RunTrigger::Manual)
            .unwrap();

        assert_eq!(run.parallel_width, 2);
        assert_eq!(run.budget_tokens, Some(2_000_000));
        assert_eq!(run.status, RunStatus::Queued);

        // Raising the team's budget afterwards must not change what this run may spend.
        s.db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE team SET budget_tokens_run = 99 WHERE id = ?1",
                    params![team],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(s.run(run.id).unwrap().budget_tokens, Some(2_000_000));
    }

    #[test]
    fn a_run_remembers_the_workspace_that_started_it() {
        let (mut s, project, _) = seeded();
        let run = s
            .create_run_in(
                project,
                "ship it here",
                RunTrigger::Manual,
                Some(Path::new("/tmp/widget-task")),
            )
            .unwrap();

        assert_eq!(run.workspace_path.as_deref(), Some("/tmp/widget-task"));
    }

    #[test]
    fn a_legacy_board_is_adopted_by_the_orchestrator_run_that_made_it() {
        let (mut s, project, team) = seeded();
        let workspace = tempfile::tempdir().unwrap();
        let run = s
            .create_run_in(
                project,
                "make the old plan",
                RunTrigger::Manual,
                Some(workspace.path()),
            )
            .unwrap();
        let orchestrator = s
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == crate::ROOT_ROLE)
            .unwrap();
        let node = s
            .dispatch(run.id, orchestrator.id, None, &ModelRegistry::local_only())
            .unwrap();
        s.set_node_session(node.id, "legacy-session").unwrap();
        s.set_node_status(node.id, NodeStatus::Done).unwrap();
        s.block_run(run.id, "Planning needs context - ClickUp is not connected")
            .unwrap();
        assert!(s
            .adopt_legacy_plan_approval(
                project,
                team,
                workspace.path(),
                "legacy-plan",
                "Preparing plan approval",
                "Plan ready for approval",
            )
            .is_err());
        s.db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE run SET blocked_reason = NULL WHERE id = ?1",
                    params![run.id],
                )?;
                Ok(())
            })
            .unwrap();

        let adopted = s
            .adopt_legacy_plan_approval(
                project,
                team,
                workspace.path(),
                "legacy-plan",
                "Preparing plan approval",
                "Plan ready for approval",
            )
            .unwrap();
        assert_eq!(adopted.id, run.id);
        assert_eq!(adopted.plan_slug.as_deref(), Some("legacy-plan"));
        assert_eq!(
            adopted.blocked_reason.as_deref(),
            Some("Plan ready for approval")
        );
    }

    #[test]
    fn workspace_limit_is_applied_after_workspace_filtering() {
        let (mut s, project, _) = seeded();
        let task = Path::new("/tmp/widget-task");
        let expected = s
            .create_run_in(project, "task", RunTrigger::Manual, Some(task))
            .unwrap();
        for prompt in ["main one", "main two", "main three"] {
            s.create_run_in(
                project,
                prompt,
                RunTrigger::Manual,
                Some(Path::new("/tmp/widget-main")),
            )
            .unwrap();
        }

        let found = s.runs_in_workspace(project, task, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, expected.id);
    }

    #[test]
    fn a_pr_worktree_lists_the_runs_that_built_its_pr_and_not_its_slots_history() {
        let (mut s, project, team) = seeded();
        let main = Path::new("/tmp/widget-main");
        let slot = "/tmp/awt/widget/1/widget";
        let registry = ModelRegistry::local_only();
        let backend = backend(&s, team);

        // An earlier plan's PR built in this slot, which awt later handed out again.
        let earlier = s
            .create_run_in(project, "earlier", RunTrigger::Manual, Some(main))
            .unwrap();
        s.set_run_plan(earlier.id, "earlier-plan").unwrap();
        let old = s
            .dispatch_task(earlier.id, backend, "PR1", None, &registry)
            .unwrap();
        s.attach_worktree(old.id, slot, Some("earlier-plan/pr1"), None)
            .unwrap();

        // The PR it holds now, built by one run and followed up by another.
        let mut now = Vec::new();
        for prompt in ["build", "address review"] {
            let run = s
                .create_run_in(project, prompt, RunTrigger::Manual, Some(main))
                .unwrap();
            s.set_run_plan(run.id, "csv").unwrap();
            let node = s
                .dispatch_task(run.id, backend, "PR2", Some("T1"), &registry)
                .unwrap();
            s.attach_worktree(node.id, slot, Some("csv/pr2"), None)
                .unwrap();
            now.push(run.id);
        }

        let found: Vec<i64> = s
            .runs_in_workspace(project, Path::new(slot), 10)
            .unwrap()
            .into_iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(found, [now[1], now[0]]);
        // The checkout they started in still has all three.
        assert_eq!(s.runs_in_workspace(project, main, 10).unwrap().len(), 3);
    }

    #[test]
    fn a_checkout_works_on_the_plan_its_latest_run_worked_on() {
        // ai-planner, asked which plan a checkout is on, answered with the plan it had
        // resolved there most - an older one - so the window's board showed that plan's
        // finished slices, and the PRs the latest run built, with their Push and Open PR,
        // were nowhere on it.
        let (mut s, project, team) = seeded();
        let main = Path::new("/tmp/widget-main");
        let slot = "/tmp/awt/widget/1/widget";
        let registry = ModelRegistry::local_only();
        let backend = backend(&s, team);
        assert_eq!(s.plan_in_workspace(project, main).unwrap(), None);

        for (prompt, plan) in [("first", "older-plan"), ("second", "review-tab")] {
            let run = s
                .create_run_in(project, prompt, RunTrigger::Manual, Some(main))
                .unwrap();
            s.set_run_plan(run.id, plan).unwrap();
            let node = s
                .dispatch_task(run.id, backend, "PR1", Some("T1"), &registry)
                .unwrap();
            s.attach_worktree(node.id, slot, Some(&format!("{plan}/pr1")), None)
                .unwrap();
        }
        // A run that has not written its plan yet does not hide the one before it.
        s.create_run_in(project, "planning", RunTrigger::Manual, Some(main))
            .unwrap();

        assert_eq!(
            s.plan_in_workspace(project, main).unwrap().as_deref(),
            Some("review-tab")
        );
        // A PR's worktree works on the plan of the run that built the PR it holds.
        assert_eq!(
            s.plan_in_workspace(project, Path::new(slot))
                .unwrap()
                .as_deref(),
            Some("review-tab")
        );
    }

    /// On macOS a tempdir under `/var/folders` is `/private/var/folders` (D12); the explicit
    /// link makes the same true on Linux, so this bites on both CI legs.
    #[cfg(unix)]
    #[test]
    fn checkout_paths_are_stored_the_way_the_filesystem_resolves_them() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let resolved = real.canonicalize().unwrap().to_string_lossy().into_owned();

        let mut s = Store::memory().unwrap();
        let project = s
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = s
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        s.attach_repo(
            project.id,
            crate::model::NewRepo {
                main_path: Some(link.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();

        // A run with no checkout named starts in the project's, resolved.
        let run = s
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        assert_eq!(run.workspace_path.as_deref(), Some(resolved.as_str()));

        let node = s
            .dispatch(
                run.id,
                backend(&s, team.id),
                Some("PR1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        let node = s
            .attach_worktree(node.id, &link.to_string_lossy(), Some("csv/pr1"), None)
            .unwrap();
        assert_eq!(node.worktree_path.as_deref(), Some(resolved.as_str()));

        // And so it is found by whichever spelling it is asked for.
        assert!(s.run_in_workspace(run.id, &link).unwrap());
        assert_eq!(s.runs_in_workspace(project.id, &link, 5).unwrap().len(), 1);
    }

    #[test]
    fn a_blocked_plan_is_found_across_sibling_workspaces() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run_in(
                project,
                "plan it",
                RunTrigger::Manual,
                Some(Path::new("/tmp/widget-first")),
            )
            .unwrap();
        s.set_run_plan(run.id, "shared-plan").unwrap();
        s.block_run(run.id, "Preparing plan approval").unwrap();

        // The caller may be standing in another checkout that resolves to the same plan.
        // Plan identity, not workspace identity, keeps it from orphaning this run.
        assert_eq!(
            s.blocked_run_for_plan(
                project,
                team,
                "shared-plan",
                ["Plan ready for approval", "Preparing plan approval"],
            )
            .unwrap()
            .map(|found| found.id),
            Some(run.id)
        );
    }

    #[test]
    fn an_active_run_owns_its_plan_before_any_slice_is_claimed() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "build it", RunTrigger::Manual)
            .unwrap();
        s.claim_run_plan(run.id, "shared-plan").unwrap();
        s.set_run_status(run.id, RunStatus::Running).unwrap();
        let duplicate = s
            .create_run(project, "build it again", RunTrigger::Manual)
            .unwrap();
        assert!(s.claim_run_plan(duplicate.id, "shared-plan").is_err());
        assert!(s.run(duplicate.id).unwrap().plan_slug.is_none());

        assert_eq!(
            s.active_run_for_plan(project, team, "shared-plan")
                .unwrap()
                .map(|found| found.id),
            Some(run.id)
        );

        s.fail_run(run.id, "lease failed").unwrap();
        assert!(s
            .active_run_for_plan(project, team, "shared-plan")
            .unwrap()
            .is_none());
    }

    #[test]
    fn only_one_run_can_begin_holding_one_plans_slices() {
        let (mut s, project, _) = seeded();
        let first = s.create_run(project, "first", RunTrigger::Manual).unwrap();
        let second = s.create_run(project, "second", RunTrigger::Manual).unwrap();
        for id in [first.id, second.id] {
            s.set_run_plan(id, "shared-plan").unwrap();
        }

        s.begin_plan_hold(
            first.id,
            "Preparing plan approval",
            "Plan ready for approval",
        )
        .unwrap();
        assert!(s
            .begin_plan_hold(
                second.id,
                "Preparing plan approval",
                "Plan ready for approval",
            )
            .is_err());
        assert_eq!(s.run(second.id).unwrap().status, RunStatus::Blocked);

        s.begin_plan_approval(first.id, "Preparing plan approval", None)
            .unwrap();
        let third = s.create_run(project, "third", RunTrigger::Manual).unwrap();
        s.set_run_plan(third.id, "shared-plan").unwrap();
        assert!(s
            .begin_plan_hold(
                third.id,
                "Preparing plan approval",
                "Plan ready for approval",
            )
            .is_err());
    }

    #[test]
    fn only_one_window_can_continue_a_run_waiting_for_plan_approval() {
        let (mut s, project, _) = seeded();
        let run = s
            .create_run(project, "plan it", RunTrigger::Manual)
            .unwrap();
        s.set_run_plan(run.id, "widget-plan").unwrap();
        s.block_run(run.id, "Plan ready for approval").unwrap();

        let continued = s
            .begin_plan_approval(run.id, "Plan ready for approval", None)
            .unwrap();
        assert_eq!(continued.status, RunStatus::Running);
        assert!(continued.blocked_reason.is_none());
        assert_eq!(continued.plan_slug.as_deref(), Some("widget-plan"));
        assert!(s
            .begin_plan_approval(run.id, "Plan ready for approval", None)
            .is_err());
        assert_eq!(s.runs(Some(project), 10).unwrap().len(), 1);
    }

    #[test]
    fn a_project_with_no_team_cannot_start_a_run() {
        let mut s = Store::memory().unwrap();
        let p = s
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(matches!(
            s.create_run(p.id, "do a thing", RunTrigger::Manual),
            Err(Error::NoTeam(_))
        ));
    }

    #[test]
    fn resetting_a_session_retires_its_address_without_deleting_it() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let registry = ModelRegistry::local_only();
        let node = s
            .dispatch(run.id, backend(&s, team), Some("S1"), &registry)
            .unwrap();
        s.set_node_session(node.id, "session-old").unwrap();
        s.set_node_status(node.id, NodeStatus::Running).unwrap();
        assert!(s.claim_node_session_reset(node.id).is_err());
        s.set_node_status(node.id, NodeStatus::Done).unwrap();

        let claimed = s.claim_node_session_reset(node.id).unwrap();
        assert!(claimed.session_resetting_at.is_some());
        let resumed = s.set_node_session(node.id, "session-old").unwrap();
        assert!(resumed.session_resetting_at.is_some());
        let retired = s.retire_node_session(node.id).unwrap();
        assert_eq!(retired.session_id.as_deref(), Some("session-old"));
        assert!(retired.session_retired_at.is_some());
        assert!(retired.session_resetting_at.is_none());

        let fresh = s.set_node_session(node.id, "session-new").unwrap();
        assert_eq!(fresh.session_id.as_deref(), Some("session-new"));
        assert!(fresh.session_retired_at.is_none());
    }

    #[test]
    fn a_run_cannot_be_done_without_an_end_time() {
        let (mut s, project, _) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        assert!(run.started_at.is_none());

        let running = s.set_run_status(run.id, RunStatus::Running).unwrap();
        assert!(running.started_at.is_some());
        assert!(running.ended_at.is_none());

        let done = s.set_run_status(run.id, RunStatus::Done).unwrap();
        assert!(done.ended_at.is_some());
        // The original start survives the transition.
        assert_eq!(done.started_at, running.started_at);
    }

    #[test]
    fn a_retry_is_a_new_row_so_the_first_attempt_survives() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let agent = backend(&s, team);

        let first = s
            .dispatch(
                run.id,
                agent,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        assert_eq!(first.attempt, 1);
        s.block_node(first.id, "verifier rejected it").unwrap();

        let second = s
            .dispatch(
                run.id,
                agent,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        assert_eq!(second.attempt, 2);
        assert_ne!(second.id, first.id);

        // The evidence of the first failure is still there.
        let first = s.node_run(first.id).unwrap();
        assert_eq!(first.status, NodeStatus::Blocked);
        assert_eq!(
            first.blocked_reason.as_deref(),
            Some("verifier rejected it")
        );
        assert_eq!(s.node_runs(run.id).unwrap().len(), 2);
    }

    #[test]
    fn dispatch_enforces_the_machine_profile_and_records_a_visible_fallback() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Scheduled)
            .unwrap();
        let agent_id = backend(&s, team);
        s.set_agent_model(agent_id, Provider::ZAi, "glm-4.6")
            .unwrap();

        // The default machine policy denies every account provider and permits local.
        // Scheduled is deliberate: this is the path that has no picker or human nearby.
        let node = s
            .dispatch(
                run.id,
                agent_id,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        assert_eq!(node.provider, Provider::Local);
        assert_eq!(node.model, "auto");
        // Team intent stays portable; only this run's historical snapshot is resolved.
        assert_eq!(s.agent(agent_id).unwrap().provider, Provider::ZAi);

        let events = s.node_events(node.id, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Note);
        assert!(events[0].summary.contains("zai/glm-4.6 -> local/auto"));
        assert_eq!(
            events[0].payload.as_ref().unwrap()["requested"]["provider"],
            "zai"
        );
    }

    #[test]
    fn dispatch_refuses_when_the_preference_and_every_fallback_are_denied() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Scheduled)
            .unwrap();
        let source = "version=1\nfallback=[\"claude\",\"openai\",\"zai\",\"local\"]\n\
                      [providers]\nclaude=false\nopenai=false\nzai=false\nlocal=false\n";
        let registry = crate::ModelRegistry::new(crate::MachineProfile::parse(source).unwrap());

        let error = s
            .dispatch(run.id, backend(&s, team), Some("PR1"), &registry)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("no allowed, implemented fallback"),
            "{error}"
        );
        assert!(s.node_runs(run.id).unwrap().is_empty());
    }

    #[test]
    fn a_node_records_who_did_the_work_even_after_the_agent_changes() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let agent = backend(&s, team);

        let node = s
            .dispatch(
                run.id,
                agent,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        assert_eq!(node.provider, Provider::Local);

        s.set_agent_model(agent, Provider::ZAi, "glm-4.6").unwrap();
        // The historical row still says what it actually ran on.
        assert_eq!(s.node_run(node.id).unwrap().provider, Provider::Local);
    }

    #[test]
    fn usage_accumulates_and_cache_reads_stay_out_of_the_budget() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = s
            .dispatch(
                run.id,
                backend(&s, team),
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();

        // The measured cold-then-warm Claude shape.
        s.record_usage(
            node.id,
            Usage {
                tokens_in: 3_000,
                tokens_out: 900,
                cache_read: 0,
                cache_write: 61_416,
            },
            1,
        )
        .unwrap();
        let after = s
            .record_usage(
                node.id,
                Usage {
                    tokens_in: 1_200,
                    tokens_out: 400,
                    cache_read: 61_416,
                    cache_write: 2_686,
                },
                1,
            )
            .unwrap();

        assert_eq!(after.turns, 2);
        assert_eq!(after.usage.cache_read, 61_416);
        assert_eq!(after.usage.tokens_in, 4_200);

        let total = s.run_usage(run.id).unwrap();
        assert_eq!(total.cache_read, 61_416);
        // 4200 in + 1300 out + 64102 cache writes. The 61k cache *read* is excluded.
        assert_eq!(total.billable(), 4_200 + 1_300 + 64_102);
    }

    #[test]
    fn a_run_reports_when_it_is_past_its_budget() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        s.db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE run SET budget_tokens = 5000 WHERE id = ?1",
                    params![run.id],
                )?;
                Ok(())
            })
            .unwrap();
        let node = s
            .dispatch(
                run.id,
                backend(&s, team),
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();

        assert!(!s.run_over_budget(run.id).unwrap());
        s.record_usage(
            node.id,
            Usage {
                tokens_in: 4_000,
                tokens_out: 1_000,
                cache_read: 900_000,
                cache_write: 0,
            },
            1,
        )
        .unwrap();
        // Exactly at the budget counts as spent; the enormous cache read does not.
        assert!(s.run_over_budget(run.id).unwrap());
    }

    #[test]
    fn blocking_one_node_leaves_its_siblings_alone() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let backend_id = backend(&s, team);
        let frontend_id = s
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "frontend")
            .unwrap()
            .id;

        let a = s
            .dispatch(
                run.id,
                backend_id,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        let b = s
            .dispatch(
                run.id,
                frontend_id,
                Some("PR2"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        s.set_node_status(b.id, NodeStatus::Running).unwrap();

        s.block_node(a.id, "gate failed twice").unwrap();

        assert_eq!(s.node_run(a.id).unwrap().status, NodeStatus::Blocked);
        assert_eq!(s.node_run(b.id).unwrap().status, NodeStatus::Running);
        // And the run itself is untouched until someone decides it is blocked.
        assert_eq!(s.run(run.id).unwrap().status, RunStatus::Queued);
    }

    #[test]
    fn a_worktree_and_its_lease_are_recorded_together() {
        let (mut s, project, team) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = s
            .dispatch(
                run.id,
                backend(&s, team),
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();

        let attached = s
            .attach_worktree(node.id, "/tmp/wt/PR1", Some("slice/PR1"), Some("lease-7"))
            .unwrap();
        assert_eq!(attached.worktree_path.as_deref(), Some("/tmp/wt/PR1"));
        assert_eq!(attached.lease_id.as_deref(), Some("lease-7"));
    }

    #[test]
    fn one_live_run_holds_a_checkout_and_a_dead_one_does_not() {
        let (mut s, project, _) = seeded();
        let checkout = tempfile::tempdir().unwrap();
        let first = s
            .create_run_in(
                project,
                "plan it",
                RunTrigger::Manual,
                Some(checkout.path()),
            )
            .unwrap();
        s.set_run_status(first.id, RunStatus::Planning).unwrap();
        s.supervise_run(first.id, 4242, |_| true).unwrap();
        assert_eq!(s.run(first.id).unwrap().supervisor_pid, Some(4242));

        let alive = |pid: i64| pid == 4242;
        let busy = s
            .busy_run_in_workspace(project, checkout.path(), alive)
            .unwrap()
            .expect("the planning run holds its checkout");
        assert_eq!(busy.id, first.id);

        // The race a pre-check cannot close: a second run already created in the same
        // checkout is refused when it tries to take it.
        let second = s
            .create_run_in(
                project,
                "another",
                RunTrigger::Manual,
                Some(checkout.path()),
            )
            .unwrap();
        let refused = s.supervise_run(second.id, 5151, alive).unwrap_err();
        assert!(
            refused.to_string().contains(&format!("run #{}", first.id)),
            "{refused}"
        );
        assert_eq!(s.run(second.id).unwrap().supervisor_pid, None);

        // A crashed run never marks itself finished. Once its process is gone it holds
        // nothing, or one crash would lock the checkout for good.
        assert!(s
            .busy_run_in_workspace(project, checkout.path(), |_| false)
            .unwrap()
            .is_none());
        s.supervise_run(second.id, 5151, |_| false).unwrap();

        // A finished run holds nothing either, whatever its process is doing.
        s.set_run_status(first.id, RunStatus::Done).unwrap();
        s.set_run_status(second.id, RunStatus::Done).unwrap();
        assert!(s
            .busy_run_in_workspace(project, checkout.path(), |_| true)
            .unwrap()
            .is_none());

        // Another checkout of the same project is somebody else's to hold.
        let elsewhere = tempfile::tempdir().unwrap();
        let third = s
            .create_run_in(
                project,
                "elsewhere",
                RunTrigger::Manual,
                Some(elsewhere.path()),
            )
            .unwrap();
        s.set_run_status(third.id, RunStatus::Running).unwrap();
        s.supervise_run(third.id, 6161, |_| true).unwrap();
        assert!(s
            .busy_run_in_workspace(project, checkout.path(), |_| true)
            .unwrap()
            .is_none());
    }

    #[test]
    fn working_on_the_default_branch_is_the_runs_own_flag_and_off_by_default() {
        let (mut s, project, _) = seeded();
        let run = s
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        assert!(!run.on_default_branch);
        assert!(
            s.set_run_on_default_branch(run.id)
                .unwrap()
                .on_default_branch
        );
    }
}
