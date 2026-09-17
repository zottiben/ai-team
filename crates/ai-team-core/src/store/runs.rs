//! Runs, and the node runs under them.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::model::{EventKind, NodeRun, NodeStatus, Provider, Run, RunStatus, RunTrigger, Usage};
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
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(Error::invalid("a run needs a prompt"));
        }
        let project = self.project(project_id)?;
        let guardrails = match project.team_id {
            Some(team_id) => self.team(team_id)?.guardrails,
            None => return Err(Error::NoTeam(project.slug)),
        };
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO run
                   (project_id, team_id, prompt, trigger, parallel_width, budget_tokens,
                    budget_seconds, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    project_id,
                    project.team_id,
                    prompt,
                    trigger,
                    guardrails.parallel_width,
                    guardrails.budget_tokens_run,
                    guardrails.budget_seconds_run,
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
        let at = now();

        // A retry is a new row, not an edit. Losing the first attempt would lose the
        // evidence of what went wrong, which is what analytics is made of.
        let attempt: i64 = self.db().conn().query_row(
            "SELECT COALESCE(MAX(attempt) + 1, 1) FROM node_run
              WHERE run_id = ?1 AND agent_id = ?2 AND COALESCE(slice_key, '') = COALESCE(?3, '')",
            params![run_id, agent_id, slice_key],
            |r| r.get(0),
        )?;

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO node_run
                   (run_id, agent_id, role, provider, model, attempt, slice_key,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    run_id,
                    agent_id,
                    agent.role,
                    resolution.provider,
                    resolution.model,
                    attempt,
                    slice_key,
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

    pub fn set_node_session(&mut self, node_run_id: i64, session_id: &str) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET session_id = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![node_run_id, session_id, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
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

    /// Mark a node blocked. Its siblings are untouched on purpose: a failed node fails
    /// its own branch without poisoning the run (M2-S10).
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

const RUN_SELECT: &str = "SELECT id, project_id, team_id, prompt, status, trigger, plan_slug, \
     parallel_width, budget_tokens, budget_seconds, blocked_reason, started_at, ended_at, rev, \
     created_at, updated_at FROM run";

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        project_id: r.get(1)?,
        team_id: r.get(2)?,
        prompt: r.get(3)?,
        status: r.get(4)?,
        trigger: r.get(5)?,
        plan_slug: non_empty(r.get(6)?),
        parallel_width: r.get(7)?,
        budget_tokens: r.get(8)?,
        budget_seconds: r.get(9)?,
        blocked_reason: non_empty(r.get(10)?),
        started_at: non_empty(r.get(11)?),
        ended_at: non_empty(r.get(12)?),
        rev: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

const NODE_SELECT: &str = "SELECT id, run_id, agent_id, role, provider, model, status, attempt, \
     slice_key, worktree_path, branch, lease_id, session_id, eve_port, stream_cursor, \
     tokens_in, tokens_out, tokens_cache_read, tokens_cache_write, turns, blocked_reason, \
     started_at, ended_at, rev, created_at, updated_at FROM node_run";

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
        session_id: non_empty(r.get(12)?),
        eve_port: r.get(13)?,
        stream_cursor: r.get(14)?,
        usage: Usage {
            tokens_in: r.get(15)?,
            tokens_out: r.get(16)?,
            cache_read: r.get(17)?,
            cache_write: r.get(18)?,
        },
        turns: r.get(19)?,
        blocked_reason: non_empty(r.get(20)?),
        started_at: non_empty(r.get(21)?),
        ended_at: non_empty(r.get(22)?),
        rev: r.get(23)?,
        created_at: r.get(24)?,
        updated_at: r.get(25)?,
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
        let team = s.seed_default_team(project.id).unwrap();
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
}
