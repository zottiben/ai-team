//! Restacks: a stacked pull request rebased onto where its parent is now (PW11).

use rusqlite::params;

use crate::error::{Error, Result};
use crate::model::{EventKind, NewRestack, NodeRun, NodeStatus, Run, RunTrigger};
use crate::store::Store;
use crate::util::now;

/// How many restacks of one branch onto one commit may fail to run before the watch
/// stops starting them and waits for the parent to move, or for a person.
const MAX_FAILED_ATTEMPTS: usize = 3;

impl Store {
    /// Start a restack: its run, the orchestrator's node in the PR's worktree, and the
    /// note that says what it is for - or nothing, when it must not start.
    ///
    /// It must not while anything else is at work in that worktree, or waiting there on a
    /// person: one writer at a time (PW6), and a parked turn's branch is somebody's. And
    /// not twice for the same branch onto the same commit: one that needed a person does
    /// not try again every time the watch looks, and the parent moving again is what
    /// starts the next (D15). Only a restack that could not run at all is tried again.
    ///
    /// One write, checked and created together, because `ait ui` and `ait daemon` both
    /// keep the clock: two watches looking at once must start one restack, not two.
    pub fn open_restack(&mut self, restack: NewRestack<'_>) -> Result<Option<(Run, NodeRun)>> {
        let project = self.project(restack.project_id)?;
        let team_id = project
            .team_id
            .ok_or_else(|| Error::NoTeam(project.slug.clone()))?;
        let guardrails = self.team(team_id)?.guardrails;
        let agent = self.agent(restack.agent_id)?;
        let workspace = restack
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| restack.workspace.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let pid = i64::from(std::process::id());
        let summary = format!(
            "{} restacks {} onto {}",
            agent.role,
            restack.slice_key,
            short(restack.onto)
        );
        // Structured, so whether this was tried is a query and not a reading of the prose.
        let payload = serde_json::to_string(&serde_json::json!({
            "restack": {
                "slice": restack.slice_key,
                "branch": restack.branch,
                "onto": restack.onto,
            }
        }))?;
        let at = now();

        let created = self.db_mut().write(|tx| {
            if at_work(tx, restack.worktree)? {
                return Ok(None);
            }
            if tried(tx, &restack)? {
                return Ok(None);
            }

            tx.execute(
                "INSERT INTO run
                   (project_id, team_id, prompt, status, trigger, plan_slug, workspace_path,
                    parallel_width, budget_tokens, budget_seconds, max_repairs,
                    budget_tokens_node, budget_seconds_node, max_turns_node, on_failure,
                    supervisor_pid, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?15)",
                params![
                    restack.project_id,
                    team_id,
                    restack.prompt,
                    RunTrigger::Review,
                    restack.plan,
                    workspace,
                    guardrails.budget_tokens_run,
                    guardrails.budget_seconds_run,
                    guardrails.max_repairs,
                    guardrails.budget_tokens_node,
                    guardrails.budget_seconds_node,
                    guardrails.max_turns_node,
                    guardrails.on_failure,
                    pid,
                    at
                ],
            )?;
            let run_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO node_run
                   (run_id, agent_id, role, provider, model, status, attempt, slice_key,
                    worktree_path, branch, supervisor_pid, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 1, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    run_id,
                    restack.agent_id,
                    agent.role,
                    restack.resolution.provider,
                    restack.resolution.model,
                    restack.slice_key,
                    restack.worktree,
                    restack.branch,
                    pid,
                    at
                ],
            )?;
            let node_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                 VALUES (?1, ?2, ?3, ?4, 'ai-team', ?5, ?6)",
                params![run_id, node_id, at, EventKind::Note, summary, payload],
            )?;
            Ok(Some((run_id, node_id)))
        })?;

        created
            .map(|(run_id, node_id)| Ok((self.run(run_id)?, self.node_run(node_id)?)))
            .transpose()
    }

    /// Whether a turn is at work in `worktree`, or waiting there on a person.
    pub fn worktree_at_work(&self, worktree: &str) -> Result<bool> {
        at_work(self.db().conn(), worktree)
    }

    /// Pin the remote commit a node's push may replace, for a branch it rewrote.
    pub fn set_push_replaces(&mut self, node_run_id: i64, sha: &str) -> Result<NodeRun> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET push_replaces = ?2, rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![node_run_id, sha, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchNodeRun(node_run_id.to_string()));
            }
            Ok(())
        })?;
        self.node_run(node_run_id)
    }
}

/// Whether this branch has been restacked onto this commit already, as far as it matters.
///
/// Done, or asked a person, or still going: tried. Failed is a turn that could not run - a
/// model out of reach - which is worth another go, a few times.
fn tried(conn: &rusqlite::Connection, restack: &NewRestack<'_>) -> Result<bool> {
    let mut attempts = conn.prepare(
        "SELECT n.status FROM event e
           JOIN run r ON r.id = e.run_id
           JOIN node_run n ON n.id = e.node_run_id
          WHERE r.project_id = ?1
            AND json_extract(e.payload_json, '$.restack.branch') = ?2
            AND json_extract(e.payload_json, '$.restack.onto') = ?3",
    )?;
    let attempts = attempts
        .query_map(
            params![restack.project_id, restack.branch, restack.onto],
            |r| r.get::<_, NodeStatus>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let decided = attempts.iter().any(|status| *status != NodeStatus::Failed);
    Ok(decided || attempts.len() >= MAX_FAILED_ATTEMPTS)
}

/// Whether anything is at work in `worktree`, or waiting there on a person.
///
/// A turn whose supervisor died is neither - it is the crash that left it `running` - and
/// counting it would hold the worktree for good.
fn at_work(conn: &rusqlite::Connection, worktree: &str) -> Result<bool> {
    let mut rows = conn.prepare(
        "SELECT status, supervisor_pid FROM node_run
          WHERE worktree_path = ?1 AND status IN ('queued', 'running', 'parked')",
    )?;
    let found = rows
        .query_map(params![worktree], |r| {
            Ok((r.get::<_, NodeStatus>(0)?, r.get::<_, Option<i64>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(found.into_iter().any(|(status, supervisor)| {
        status == NodeStatus::Parked || supervisor.is_none_or(crate::util::process_is_alive)
    }))
}

/// A commit as a person reads one in a sentence.
fn short(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::ModelRegistry;

    struct Stack {
        store: Store,
        project: i64,
        orchestrator: i64,
        backend: i64,
        checkout: tempfile::TempDir,
    }

    fn stack() -> Stack {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let seat = |role: &str| {
            store
                .agents(team.id)
                .unwrap()
                .into_iter()
                .find(|agent| agent.role == role)
                .unwrap()
                .id
        };
        let (orchestrator, backend) = (seat("orchestrator"), seat("backend"));
        Stack {
            store,
            project: project.id,
            orchestrator,
            backend,
            checkout: tempfile::tempdir().unwrap(),
        }
    }

    impl Stack {
        fn open(&mut self, worktree: &str, onto: &str) -> Option<(Run, NodeRun)> {
            let agent = self.store.agent(self.orchestrator).unwrap();
            let resolution = ModelRegistry::local_only().resolve(&agent).unwrap();
            self.store
                .open_restack(NewRestack {
                    project_id: self.project,
                    workspace: self.checkout.path(),
                    plan: "shout",
                    slice_key: "PR2",
                    branch: "shout/pr2",
                    worktree,
                    onto,
                    prompt: "Restack PR2 onto main: PR1 merged",
                    agent_id: self.orchestrator,
                    resolution: &resolution,
                })
                .unwrap()
        }

        /// A maker's turn in `worktree`, left in `status`.
        fn maker(&mut self, worktree: &str, status: NodeStatus, supervisor: Option<i64>) {
            let run = self
                .store
                .create_run(self.project, "build", RunTrigger::Manual)
                .unwrap();
            let node = self
                .store
                .dispatch_task(
                    run.id,
                    self.backend,
                    "PR2",
                    Some("T1"),
                    &ModelRegistry::local_only(),
                )
                .unwrap();
            self.store
                .attach_worktree(node.id, worktree, Some("shout/pr2"), None)
                .unwrap();
            // Supervision is claimed on a running turn, before it settles.
            self.store
                .set_node_status(node.id, NodeStatus::Running)
                .unwrap();
            if let Some(pid) = supervisor {
                self.store
                    .claim_node_supervision(node.id, pid, None)
                    .unwrap();
            }
            self.store.set_node_status(node.id, status).unwrap();
        }
    }

    #[test]
    fn a_restack_is_its_own_run_by_the_orchestrator_in_the_prs_worktree() {
        let mut s = stack();

        let (run, node) = s.open("/awt/2", "0123456789abcdef").unwrap();

        assert_eq!(run.trigger, RunTrigger::Review);
        assert_eq!(run.plan_slug.as_deref(), Some("shout"));
        assert_eq!(
            run.workspace_path.as_deref(),
            Some(s.checkout.path().canonicalize().unwrap().to_str().unwrap())
        );
        assert_eq!(node.role, "orchestrator");
        assert_eq!(node.slice_key.as_deref(), Some("PR2"));
        assert_eq!(node.worktree_path.as_deref(), Some("/awt/2"));
        assert_eq!(node.branch.as_deref(), Some("shout/pr2"));
        assert_eq!(node.status, NodeStatus::Queued);
        assert_eq!(node.supervisor_pid, Some(i64::from(std::process::id())));
        let said = s.store.node_events(node.id, 10).unwrap();
        assert_eq!(said[0].summary, "orchestrator restacks PR2 onto 01234567");
    }

    #[test]
    fn a_restack_waits_for_whoever_is_at_work_or_waiting_in_its_worktree() {
        let mut s = stack();
        let me = i64::from(std::process::id());

        s.maker("/awt/2", NodeStatus::Running, Some(me));
        assert!(s.open("/awt/2", "aaaa").is_none(), "under a running turn");

        let mut s = stack();
        s.maker("/awt/2", NodeStatus::Parked, None);
        assert!(
            s.open("/awt/2", "aaaa").is_none(),
            "under a turn waiting on a person"
        );

        // Somebody else's worktree is nothing to wait for.
        assert!(s.open("/awt/3", "aaaa").is_some());
    }

    #[test]
    fn a_worktree_is_at_work_while_a_live_turn_or_a_person_holds_it() {
        let mut s = stack();
        let me = i64::from(std::process::id());
        assert!(!s.store.worktree_at_work("/awt/2").unwrap());

        s.maker("/awt/2", NodeStatus::Running, Some(me));
        assert!(s.store.worktree_at_work("/awt/2").unwrap());
        s.maker("/awt/3", NodeStatus::Parked, None);
        assert!(s.store.worktree_at_work("/awt/3").unwrap());
        s.maker("/awt/4", NodeStatus::Done, Some(me));
        assert!(!s.store.worktree_at_work("/awt/4").unwrap());
    }

    #[test]
    fn a_turn_whose_supervisor_died_does_not_hold_a_restack_off_for_good() {
        let mut s = stack();
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let dead = i64::from(gone.id());
        gone.wait().unwrap();

        s.maker("/awt/2", NodeStatus::Running, Some(dead));

        assert!(s.open("/awt/2", "aaaa").is_some());
    }

    #[test]
    fn a_branch_is_restacked_onto_one_commit_once_and_a_second_watch_starts_nothing() {
        let mut s = stack();

        let (_, first) = s.open("/awt/2", "aaaa").unwrap();
        // Another process watching at the same moment.
        assert!(s.open("/awt/2", "aaaa").is_none());

        // It needed a person. Looking again does not ask again...
        s.store
            .set_node_status(first.id, NodeStatus::Parked)
            .unwrap();
        s.store
            .block_node(first.id, "which side of two.txt?")
            .unwrap();
        assert!(s.open("/awt/2", "aaaa").is_none());
        // ...the parent moving on does.
        assert!(s.open("/awt/2", "bbbb").is_some());
    }

    #[test]
    fn a_restack_that_could_not_run_is_tried_again_but_not_for_ever() {
        let mut s = stack();

        // The model was unreachable: nothing was decided, so the next look tries again.
        for _ in 0..3 {
            let (_, node) = s.open("/awt/2", "aaaa").expect("tried again");
            s.store
                .set_node_status(node.id, NodeStatus::Failed)
                .unwrap();
        }
        // Three times is a pattern, not a blip. It waits for the parent to move, or for a
        // person, rather than starting a run every time the watch looks.
        assert!(s.open("/awt/2", "aaaa").is_none());
    }
}
