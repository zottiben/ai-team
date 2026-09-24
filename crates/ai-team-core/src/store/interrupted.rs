//! Runs whose supervising process is gone.
//!
//! A run is driven by one ai-team process - `ait run`, the window, the daemon - and its
//! rows say `planning` or `running` for as long as that process is doing it. When the
//! process dies, nothing else changes them: a crash, a closed terminal and a laptop that
//! slept all left a run "planning" for good, and Today reporting "the team is working" on
//! a team nobody was running. Every surface read the rows, and the rows were stale.
//!
//! So a run with no live process left in it is settled once, the first time it is seen.
//! A maker's turn with its Pi session and lease intact is left as it is - resuming it
//! carries on the same conversation in the same worktree - and the run waits, blocked as
//! interrupted, for somebody to resume it. Everything else stopped for good.

use rusqlite::{params, OptionalExtension};

use crate::error::Result;
use crate::model::{EventKind, NodeRun, NodeStatus, Run, RunStatus};
use crate::store::Store;
use crate::util::now;

/// Why a run is blocked when its process stopped mid-turn and a turn can be resumed.
/// Resuming is allowed from exactly this state (see `workflow::resume_interrupted_node_at`).
pub const INTERRUPTED_REASON: &str = "Interrupted: the ai-team process running it stopped";

/// A run found with no live process, and what became of it.
#[derive(Debug, Clone)]
pub struct Abandoned {
    pub run: Run,
    /// Turns that can be resumed where they were: the run is blocked, waiting on them.
    pub resumable: Vec<NodeRun>,
    /// Turns that stopped for good, now failed.
    pub stopped: Vec<NodeRun>,
    /// Of those, the builds whose pull request stops with them: a writing seat's turn on a
    /// slice, in a worktree, with no resumable turn left on that slice. Their claim and
    /// lease go back, as a failed build's do. A restack's or a verifier's turn is in some
    /// other build's worktree, and never is.
    pub released: Vec<NodeRun>,
    /// The last process recorded as running it.
    pub pid: i64,
}

impl Store {
    /// Settle every run whose supervising process is gone. `alive` asks whether a process
    /// is running; it is a parameter so tests can say which ones are.
    ///
    /// A run counts only on evidence: it recorded a process, and neither it nor any of its
    /// open turns has one alive. A resumed turn runs under the window's process while the
    /// run still names the one that died, and that run is being worked on. A run that
    /// never recorded a process says nothing either way and is left alone.
    ///
    /// Each run is settled in one write that first checks it is still open, so two
    /// processes keeping the clock settle it once.
    pub fn settle_abandoned_runs(&mut self, alive: impl Fn(i64) -> bool) -> Result<Vec<Abandoned>> {
        let open: Vec<Run> = self
            .runs(None, i64::MAX)?
            .into_iter()
            .filter(|run| {
                matches!(
                    run.status,
                    RunStatus::Queued | RunStatus::Planning | RunStatus::Running
                )
            })
            .collect();
        let mut settled = Vec::new();
        for run in open {
            let turns: Vec<NodeRun> = self
                .node_runs(run.id)?
                .into_iter()
                .filter(|node| matches!(node.status, NodeStatus::Queued | NodeStatus::Running))
                .collect();
            let pids: Vec<i64> = run
                .supervisor_pid
                .into_iter()
                .chain(turns.iter().filter_map(|node| node.supervisor_pid))
                .collect();
            if pids.is_empty() || pids.iter().any(|pid| alive(*pid)) {
                continue;
            }
            let (resumable, stopped): (Vec<NodeRun>, Vec<NodeRun>) = turns
                .into_iter()
                .partition(|node| self.resumable(node).unwrap_or(false));
            let pid = run
                .supervisor_pid
                .or(pids.last().copied())
                .unwrap_or_default();
            if self.settle_one(&run, &resumable, &stopped, pid)? {
                let stopped: Vec<NodeRun> = stopped
                    .iter()
                    .map(|node| self.node_run(node.id))
                    .collect::<Result<_>>()?;
                let mut released = Vec::new();
                for node in &stopped {
                    let builds = match node.agent_id {
                        Some(id) => !self.agent(id)?.read_only,
                        None => false,
                    };
                    let carries_on = resumable
                        .iter()
                        .any(|other| other.slice_key == node.slice_key);
                    if builds
                        && node.slice_key.is_some()
                        && node.worktree_path.is_some()
                        && !carries_on
                    {
                        released.push(node.clone());
                    }
                }
                settled.push(Abandoned {
                    run: self.run(run.id)?,
                    resumable,
                    stopped,
                    released,
                    pid,
                });
            }
        }
        Ok(settled)
    }

    /// Whether a turn can carry on where it stopped: a maker's, with its Pi session and
    /// the worktree it was building in. The same test the window's Resume is offered on.
    fn resumable(&self, node: &NodeRun) -> Result<bool> {
        let writes = match node.agent_id {
            Some(id) => !self.agent(id)?.read_only,
            None => false,
        };
        Ok(node.status == NodeStatus::Running
            && writes
            && node.session_id.is_some()
            && node.session_retired_at.is_none()
            && node.session_resetting_at.is_none()
            && node.slice_key.is_some()
            && node.worktree_path.is_some())
    }

    /// Write one run's settlement, if it is still open. Returns whether it was.
    fn settle_one(
        &mut self,
        run: &Run,
        resumable: &[NodeRun],
        stopped: &[NodeRun],
        pid: i64,
    ) -> Result<bool> {
        let at = now();
        let gone = format!("its ai-team process (pid {pid}) exited");
        self.db_mut().write(|tx| {
            let still_open: Option<i64> = tx
                .query_row(
                    "SELECT id FROM run WHERE id = ?1
                        AND status IN ('queued', 'planning', 'running')",
                    params![run.id],
                    |r| r.get(0),
                )
                .optional()?;
            if still_open.is_none() {
                return Ok(false);
            }
            for node in stopped {
                let why = if node.status == NodeStatus::Queued {
                    format!("never started: {gone}")
                } else {
                    format!("stopped mid-turn: {gone}")
                };
                tx.execute(
                    "UPDATE node_run SET status = 'failed', blocked_reason = ?2, ended_at = ?3,
                                         rev = rev + 1, updated_at = ?3
                      WHERE id = ?1 AND status IN ('queued', 'running')",
                    params![node.id, why, at],
                )?;
            }
            let (summary, kind) = if resumable.is_empty() {
                tx.execute(
                    "UPDATE run SET status = 'failed', blocked_reason = ?2, ended_at = ?3,
                                    rev = rev + 1, updated_at = ?3
                      WHERE id = ?1",
                    params![run.id, format!("Stopped: {gone}"), at],
                )?;
                (format!("run stopped: {gone}"), EventKind::Failed)
            } else {
                tx.execute(
                    "UPDATE run SET status = 'blocked', blocked_reason = ?2, rev = rev + 1,
                                    updated_at = ?3
                      WHERE id = ?1",
                    params![run.id, INTERRUPTED_REASON, at],
                )?;
                let turns = resumable
                    .iter()
                    .map(|node| {
                        let task = node
                            .task_key
                            .as_deref()
                            .map_or(String::new(), |task| format!(" {task}"));
                        format!(
                            "{} on {}{task}",
                            node.role,
                            node.slice_key.as_deref().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!("run interrupted: {gone}. Resume {turns} to carry on"),
                    EventKind::Note,
                )
            };
            tx.execute(
                "INSERT INTO event (run_id, at, kind, actor, summary)
                 VALUES (?1, ?2, ?3, 'ai-team', ?4)",
                params![run.id, at, kind, summary],
            )?;
            Ok(true)
        })
    }

    /// Take an interrupted run back: running again, driven by `pid`. Refused unless it
    /// is blocked as interrupted - which is what makes two Resume clicks start one resume.
    pub fn resume_interrupted_run(&mut self, id: i64, pid: i64) -> Result<Run> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE run SET status = 'running', blocked_reason = NULL, supervisor_pid = ?2,
                                rev = rev + 1, updated_at = ?3
                  WHERE id = ?1 AND status = 'blocked' AND blocked_reason = ?4",
                params![id, pid, at, INTERRUPTED_REASON],
            )?)
        })?;
        if changed == 0 {
            return Err(crate::error::Error::invalid(
                "that run is no longer waiting to be resumed",
            ));
        }
        self.run(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::ModelRegistry;
    use crate::model::{NewProject, RunTrigger};

    const GONE: i64 = 4_000_001;
    const LIVE: i64 = 4_000_002;

    fn alive(pid: i64) -> bool {
        pid == LIVE
    }

    struct Crash {
        store: Store,
        project: i64,
        team: i64,
    }

    fn crash() -> Crash {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        Crash {
            store,
            project: project.id,
            team: team.id,
        }
    }

    impl Crash {
        fn run(&mut self, status: RunStatus, supervisor: Option<i64>) -> Run {
            let run = self
                .store
                .create_run(self.project, "add a --loud option", RunTrigger::Manual)
                .unwrap();
            if let Some(pid) = supervisor {
                self.store.set_run_supervisor(run.id, pid).unwrap();
            }
            self.store.set_run_status(run.id, status).unwrap()
        }

        /// A turn by `role`, left `status`; a maker's with its session and lease.
        fn turn(&mut self, run: &Run, role: &str, status: NodeStatus, pid: Option<i64>) -> NodeRun {
            let agent = self
                .store
                .agents(self.team)
                .unwrap()
                .into_iter()
                .find(|agent| agent.role == role)
                .unwrap();
            let slice = (role != "orchestrator").then_some("PR1");
            let node = self
                .store
                .dispatch(run.id, agent.id, slice, &ModelRegistry::local_only())
                .unwrap();
            if slice.is_some() {
                self.store
                    .attach_worktree(node.id, "/awt/1", Some("p/pr1"), None)
                    .unwrap();
            }
            self.store.set_node_session(node.id, "session-1").unwrap();
            self.store
                .set_node_status(node.id, NodeStatus::Running)
                .unwrap();
            if let Some(pid) = pid {
                self.store
                    .claim_node_supervision(node.id, pid, None)
                    .unwrap();
            }
            self.store.set_node_status(node.id, status).unwrap()
        }

        fn settle(&mut self) -> Vec<Abandoned> {
            self.store.settle_abandoned_runs(alive).unwrap()
        }
    }

    #[test]
    fn a_run_whose_process_died_while_planning_stopped_and_says_why() {
        let mut c = crash();
        let run = c.run(RunStatus::Planning, Some(GONE));
        let planning = c.turn(&run, "orchestrator", NodeStatus::Running, None);

        let settled = c.settle();

        assert_eq!(settled.len(), 1);
        let run = c.store.run(run.id).unwrap();
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.ended_at.is_some());
        assert!(run.blocked_reason.unwrap().contains("pid 4000001"));
        let planning = c.store.node_run(planning.id).unwrap();
        assert_eq!(planning.status, NodeStatus::Failed);
        assert!(planning
            .blocked_reason
            .unwrap()
            .starts_with("stopped mid-turn"));
        // Once: settling again finds nothing to settle.
        assert!(c.settle().is_empty());
    }

    #[test]
    fn a_maker_interrupted_mid_turn_is_left_to_resume_and_the_run_waits_for_it() {
        let mut c = crash();
        let run = c.run(RunStatus::Running, Some(GONE));
        let building = c.turn(&run, "backend", NodeStatus::Running, Some(GONE));
        let never = c.turn(&run, "frontend", NodeStatus::Queued, None);

        let settled = c.settle();

        assert_eq!(settled[0].resumable.len(), 1);
        let run = c.store.run(run.id).unwrap();
        assert_eq!(run.status, RunStatus::Blocked);
        assert_eq!(run.blocked_reason.as_deref(), Some(INTERRUPTED_REASON));
        // Untouched: resuming it carries on the same session in the same worktree.
        assert_eq!(
            c.store.node_run(building.id).unwrap().status,
            NodeStatus::Running
        );
        let never = c.store.node_run(never.id).unwrap();
        assert_eq!(never.status, NodeStatus::Failed);
        assert!(never.blocked_reason.unwrap().starts_with("never started"));

        let resumed = c.store.resume_interrupted_run(run.id, LIVE).unwrap();
        assert_eq!(resumed.status, RunStatus::Running);
        assert_eq!(resumed.supervisor_pid, Some(LIVE));
        assert!(resumed.blocked_reason.is_none());
        // Two Resume clicks start one resume.
        assert!(c.store.resume_interrupted_run(run.id, LIVE).is_err());
    }

    #[test]
    fn a_run_somebody_is_still_driving_is_left_alone() {
        let mut c = crash();
        let live = c.run(RunStatus::Running, Some(LIVE));
        c.turn(&live, "backend", NodeStatus::Running, None);
        // Resumed from the window: the run still names the process that died, and the
        // turn names the window's, which is alive.
        let resumed = c.run(RunStatus::Running, Some(GONE));
        c.turn(&resumed, "backend", NodeStatus::Running, Some(LIVE));
        // Never recorded a process: no evidence either way.
        let unknown = c.run(RunStatus::Running, None);
        c.turn(&unknown, "orchestrator", NodeStatus::Running, None);

        assert!(c.settle().is_empty());
        for run in [live, resumed, unknown] {
            assert_eq!(c.store.run(run.id).unwrap().status, RunStatus::Running);
        }
    }

    #[test]
    fn only_a_build_that_stopped_for_good_gives_its_pr_back() {
        let mut c = crash();
        // A maker that stopped before its session began: its PR stops with it.
        let run = c.run(RunStatus::Running, Some(GONE));
        let unstarted = c.turn(&run, "backend", NodeStatus::Queued, None);
        // A restack's orchestrator in a PR's worktree: that PR is still in review, and its
        // worktree and claim are not this turn's to give back.
        let restack = c.run(RunStatus::Running, Some(GONE));
        let orchestrator = c
            .store
            .agents(c.team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "orchestrator")
            .unwrap();
        let restacking = c
            .store
            .dispatch(
                restack.id,
                orchestrator.id,
                Some("PR1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        c.store
            .attach_worktree(restacking.id, "/awt/1", Some("p/pr1"), None)
            .unwrap();
        c.store
            .set_node_status(restacking.id, NodeStatus::Running)
            .unwrap();

        let settled = c.settle();

        let released = |run: &Run| {
            settled
                .iter()
                .find(|abandoned| abandoned.run.id == run.id)
                .unwrap()
                .released
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(released(&run), [unstarted.id]);
        assert!(released(&restack).is_empty());
    }

    #[test]
    fn a_finished_run_is_history_whatever_became_of_its_process() {
        let mut c = crash();
        let done = c.run(RunStatus::Done, Some(GONE));
        c.turn(&done, "backend", NodeStatus::Done, Some(GONE));

        assert!(c.settle().is_empty());
        assert_eq!(c.store.run(done.id).unwrap().status, RunStatus::Done);
    }
}
