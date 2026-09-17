//! Build it, start it, drive a turn, and land the result in the database.
//!
//! This is the layer the CLI, the server and the desktop shell all call. It owns the
//! ordering - generate, install, build, start, drive, ingest - and nothing above it
//! needs to know that ordering exists.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::eve::{StreamEvent, TerminalState};
use crate::model::{EventKind, NewEvent, Usage};
use crate::store::Store;
use crate::supervise::client::{approvals_in, Approval, EveClient};
use crate::supervise::process::{run_streaming, EveEnv, EveProcess, ProgressLine};
use crate::supervise::Flow;

/// Events are written in batches rather than one at a time: a transaction per event
/// makes a busy turn crawl, and a batch this size bounds what a crash can lose to a
/// couple of seconds of stream.
const FLUSH_EVERY: usize = 25;

/// How a build is getting on, for a progress bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProgress {
    pub phase: BuildPhase,
    pub line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPhase {
    Install,
    Build,
}

impl BuildPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            BuildPhase::Install => "install",
            BuildPhase::Build => "build",
        }
    }
}

/// What one turn did.
#[derive(Debug, Clone, Default)]
pub struct TurnOutcome {
    pub recorded: usize,
    pub duplicates: usize,
    pub usage: Usage,
    pub steps: i64,
    /// How the turn ended, or `None` while it is parked/in flight.
    pub terminal: Option<TerminalState>,
    /// The turn is parked on these.
    pub approvals: Vec<Approval>,
}

/// One generated eve project, and the process serving it.
#[derive(Debug)]
pub struct Supervisor {
    project_dir: PathBuf,
    env: EveEnv,
    process: Option<EveProcess>,
}

impl Supervisor {
    pub fn new(project_dir: impl Into<PathBuf>, env: EveEnv) -> Supervisor {
        Supervisor {
            project_dir: project_dir.into(),
            env,
            process: None,
        }
    }

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// Install dependencies and build, reporting every line.
    ///
    /// `npm ci` is preferred and `npm install` is the fallback, because a generated
    /// `package.json` whose lockfile has not caught up makes `ci` refuse - and the
    /// lockfile is not something the team rows describe.
    pub async fn install_and_build<F>(&self, mut on_progress: F) -> Result<()>
    where
        F: FnMut(BuildProgress) + Send,
    {
        if !self.project_dir.join("package.json").exists() {
            return Err(Error::invalid(format!(
                "{} has no package.json - run `ait agents generate` first",
                self.project_dir.display()
            )));
        }

        if !self.project_dir.join("node_modules").exists() {
            // `npm ci` needs a lockfile, and a freshly generated project has none - it
            // is written by the install itself. Trying it anyway "works", because the
            // fallback catches it, but it first dumps ~100 lines of npm's usage text
            // into the progress stream and into the run's event log.
            let args: &[&str] = if self.project_dir.join("package-lock.json").exists() {
                &["ci", "--no-audit", "--no-fund"]
            } else {
                &["install", "--no-audit", "--no-fund"]
            };
            run_streaming("npm", args, &self.project_dir, None, |line| {
                on_progress(progress(BuildPhase::Install, line));
            })
            .await?;
        }

        // The environment is passed to the build because `eve build` evaluates every
        // authored module - a module that reads a variable at import time would fail
        // here without it.
        run_streaming(
            "npx",
            &["eve", "build"],
            &self.project_dir,
            Some(&self.env),
            |line| on_progress(progress(BuildPhase::Build, line)),
        )
        .await
    }

    /// Start the built output and wait for it to serve.
    pub async fn start(&mut self) -> Result<EveClient> {
        if !self.project_dir.join(".output").exists() {
            return Err(Error::invalid(format!(
                "{} has no .output - build it before starting it",
                self.project_dir.display()
            )));
        }
        let mut process = EveProcess::start(&self.project_dir, &self.env)?;
        process.wait_until_ready().await?;
        let client = process.client();
        self.process = Some(process);
        Ok(client)
    }

    /// The client for the running process, if there is one.
    pub fn client(&self) -> Option<EveClient> {
        self.process.as_ref().map(EveProcess::client)
    }

    pub fn port(&self) -> Option<u16> {
        self.process.as_ref().map(EveProcess::port)
    }

    /// Has the process died under us?
    pub fn is_alive(&mut self) -> bool {
        self.process.as_mut().is_some_and(EveProcess::is_alive)
    }

    pub async fn stop(&mut self) -> Result<()> {
        match self.process.take() {
            Some(process) => process.stop().await,
            None => Ok(()),
        }
    }
}

fn progress(phase: BuildPhase, line: ProgressLine) -> BuildProgress {
    BuildProgress {
        phase,
        line: line.text,
    }
}

/// Record a build line as an event on a run, so the UI can show a bar over the same
/// stream it shows everything else.
pub fn record_build_progress(
    store: &mut Store,
    run_id: i64,
    progress: &BuildProgress,
) -> Result<()> {
    // Bounded: a build emits thousands of lines and the interesting ones are the phase
    // boundaries and anything that looks like a failure.
    let text = progress.line.trim();
    if text.is_empty() {
        return Ok(());
    }
    store.append_event(
        run_id,
        NewEvent::new(
            EventKind::Build,
            format!("{}: {text}", progress.phase.as_str()),
        )
        .by("supervisor"),
    )?;
    Ok(())
}

/// Drive one turn to its end, ingesting as it goes.
///
/// `session` must already exist. Streaming starts from the node's stored cursor, so
/// calling this again after a crash resumes rather than replaying - and even if it did
/// replay, the dedupe on `meta.id` makes that free.
pub async fn drive_turn<F>(
    store: &mut Store,
    node_run_id: i64,
    client: &EveClient,
    session: &str,
    mut on_event: F,
) -> Result<TurnOutcome>
where
    F: FnMut(&StreamEvent),
{
    let from_index = store.node_run(node_run_id)?.stream_cursor;

    let mut outcome = TurnOutcome::default();
    let mut buffer: Vec<StreamEvent> = Vec::new();
    let mut next_index = from_index;
    let mut failure: Option<Error> = None;

    {
        // Everything below borrows `store` mutably for the length of the stream, which
        // is why the flush is a closure rather than a method.
        let store = &mut *store;
        let outcome = &mut outcome;
        let failure = &mut failure;

        client
            .stream(session, from_index, |event| {
                on_event(&event);
                outcome.approvals.extend(approvals_in(&event));

                let terminal = event.is_terminal();
                let parked = event.is_awaiting_input();
                buffer.push(event);

                if buffer.len() >= FLUSH_EVERY || terminal || parked {
                    match store.ingest_events(node_run_id, next_index, &buffer) {
                        Ok(batch) => {
                            next_index += i64::try_from(buffer.len()).unwrap_or(0);
                            outcome.recorded += batch.recorded;
                            outcome.duplicates += batch.duplicates;
                            outcome.usage += batch.usage;
                            outcome.steps += batch.steps;
                            if batch.terminal.is_some() {
                                outcome.terminal = batch.terminal;
                            }
                            buffer.clear();
                        }
                        Err(e) => {
                            *failure = Some(e);
                            return Flow::Stop;
                        }
                    }
                }

                // Parking is an end to *this* read: the turn will not produce another
                // event until a human answers, and holding the connection open would
                // just burn the idle timeout.
                if terminal || parked {
                    Flow::Stop
                } else {
                    Flow::Continue
                }
            })
            .await?;
    }

    if let Some(error) = failure {
        return Err(error);
    }

    // Whatever the stream ended on without reaching a flush boundary.
    if !buffer.is_empty() {
        let batch = store.ingest_events(node_run_id, next_index, &buffer)?;
        outcome.recorded += batch.recorded;
        outcome.duplicates += batch.duplicates;
        outcome.usage += batch.usage;
        outcome.steps += batch.steps;
        if batch.terminal.is_some() {
            outcome.terminal = batch.terminal;
        }
    }

    Ok(outcome)
}

/// Start a session for a node and drive its first turn.
pub async fn run_turn<F>(
    store: &mut Store,
    node_run_id: i64,
    client: &EveClient,
    prompt: &str,
    on_event: F,
) -> Result<(String, TurnOutcome)>
where
    F: FnMut(&StreamEvent),
{
    // Reuse the node's session when it already has one, so a resumed node continues its
    // conversation rather than starting a second one with no history.
    let session = if let Some(existing) = store.node_run(node_run_id)?.session_id {
        client.follow_up(&existing, prompt).await?;
        existing
    } else {
        let session = client.start_session(prompt).await?;
        store.set_node_session(node_run_id, &session)?;
        session
    };

    let outcome = drive_turn(store, node_run_id, client, &session, on_event).await?;
    Ok((session, outcome))
}

/// Wait for a process to come back after a crash, then keep going from the cursor.
pub async fn reattach(
    store: &mut Store,
    node_run_id: i64,
    client: &EveClient,
    timeout: Duration,
) -> Result<Option<String>> {
    client.wait_until_healthy(timeout).await?;
    Ok(store.node_run(node_run_id)?.session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, RunTrigger};

    fn store_with_node() -> (Store, i64, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        let agent = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                agent.id,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        (store, run.id, node.id)
    }

    #[test]
    fn build_lines_become_events_on_the_run() {
        let (mut store, run_id, _) = store_with_node();
        record_build_progress(
            &mut store,
            run_id,
            &BuildProgress {
                phase: BuildPhase::Install,
                line: "added 43 packages".into(),
            },
        )
        .unwrap();
        record_build_progress(
            &mut store,
            run_id,
            &BuildProgress {
                phase: BuildPhase::Build,
                line: "built output at .output".into(),
            },
        )
        .unwrap();
        // Blank lines are noise, not progress.
        record_build_progress(
            &mut store,
            run_id,
            &BuildProgress {
                phase: BuildPhase::Build,
                line: "   ".into(),
            },
        )
        .unwrap();

        let events = store.events(run_id, None, 100).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.kind == EventKind::Build));
        assert_eq!(events[0].summary, "install: added 43 packages");
        assert_eq!(events[1].summary, "build: built output at .output");
    }

    #[tokio::test]
    async fn starting_without_a_build_says_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut supervisor = Supervisor::new(
            dir.path(),
            EveEnv {
                worktree: dir.path().to_path_buf(),
                token: "t".into(),
                provider_keys: vec![],
            },
        );
        let err = supervisor.start().await.unwrap_err().to_string();
        assert!(err.contains(".output"), "{err}");
    }

    #[tokio::test]
    async fn building_without_a_generated_project_says_to_generate_it() {
        let dir = tempfile::tempdir().unwrap();
        let supervisor = Supervisor::new(
            dir.path(),
            EveEnv {
                worktree: dir.path().to_path_buf(),
                token: "t".into(),
                provider_keys: vec![],
            },
        );
        let err = supervisor
            .install_and_build(|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ait agents generate"), "{err}");
    }

    #[test]
    fn a_crash_leaves_enough_to_resume_from() {
        // "Restart and reattach cleanly": everything needed to continue is a row, not
        // in-memory state, so losing the process loses nothing but the process.
        let (mut store, _, node_id) = store_with_node();
        store.set_node_session(node_id, "wrun_A").unwrap();

        // A turn ran and was partly ingested before the supervisor died.
        let ndjson = "\
{\"type\":\"step.started\",\"data\":{},\"meta\":{\"id\":\"evt_1\"}}
{\"type\":\"actions.requested\",\"data\":{\"actions\":[{\"toolName\":\"bash\"}]},\"meta\":{\"id\":\"evt_2\"}}
";
        store.ingest_ndjson(node_id, 0, ndjson).unwrap();

        let node = store.node_run(node_id).unwrap();
        assert_eq!(node.session_id.as_deref(), Some("wrun_A"));
        assert_eq!(node.stream_cursor, 2, "resume asks eve for ?startIndex=2");

        // And re-reading from the top after a restart costs nothing, because the ids
        // are the dedupe key - so a supervisor that cannot remember where it was may
        // safely rewind to 0.
        let replay = store.ingest_ndjson(node_id, 0, ndjson).unwrap();
        assert_eq!(replay.recorded, 0);
        assert_eq!(store.node_run(node_id).unwrap().stream_cursor, 2);
    }

    #[test]
    fn a_node_with_no_session_yet_has_nothing_to_reattach_to() {
        let (store, _, node_id) = store_with_node();
        assert!(store.node_run(node_id).unwrap().session_id.is_none());
    }

    #[test]
    fn a_phase_names_itself_for_the_event_log() {
        assert_eq!(BuildPhase::Install.as_str(), "install");
        assert_eq!(BuildPhase::Build.as_str(), "build");
    }
}
