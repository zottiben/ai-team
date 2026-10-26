//! Commands ai-team runs for the operator, and the receipts they leave.
//!
//! A pty tells you when a *shell* exited, not when a command in it finished, and nothing
//! in a terminal stream distinguishes a prompt from output that looks like one. Reading
//! the status of a typed command back out of a scrollback means parsing prompts, or
//! writing markers into somebody's dotfiles, and both are guesses about a surface the
//! operator owns.
//!
//! So a pad does not type. It starts its own pty running exactly that command, which
//! makes the process *be* the command: the exit status is the command's status, and the
//! measured lifetime is its duration, with no protocol to agree and nothing of the
//! operator's touched. A shell they are using - with half a line typed into it, or four
//! minutes into a build - is never written to.
//!
//! Outcomes are observed, not inferred from silence. A receipt remains available until
//! its idempotent durable write is acknowledged, even after its PTY is retired.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::Terminals;
use crate::error::Result;

/// A command that has just been started.
#[derive(Debug, Clone, Serialize)]
pub struct ManagedCommand {
    /// This run's identity, minted by the caller *with the record that authorised it* and
    /// handed in here. The row and this deck agree on it, so a receipt recorded later
    /// lands on the run it belongs to rather than on a command that merely has the same
    /// text - and a start this deck observed is still named by the row even when the write
    /// that was to say so never completed.
    pub token: String,
    pub session: u64,
    pub label: String,
    pub command: String,
    pub worktree: String,
}

/// How a managed command is going, while it is going.
#[derive(Debug, Clone, Serialize)]
pub struct CommandReceipt {
    pub token: String,
    pub session: u64,
    pub label: String,
    pub command: String,
    pub duration_ms: u64,
    /// Lines of output seen so far. A floor, not a total, once `lines_exact` is false.
    pub lines: u64,
    pub lines_exact: bool,
}

/// A command that has ended, handed out once so it is recorded once.
#[derive(Debug, Clone, Serialize)]
pub struct SettledCommand {
    pub token: String,
    /// `None` when the session went away before its status could be read - closed by
    /// hand, or taken down with the app. An unknown outcome is said, never assumed to be
    /// a success.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub lines: u64,
    pub lines_exact: bool,
}

/// One managed run, and how far this deck has read its output.
#[derive(Debug)]
pub(super) struct Managed {
    pub(super) session: u64,
    pub(super) label: String,
    pub(super) command: String,
    pub(super) worktree: String,
    cursor: u64,
    lines: u64,
    lines_exact: bool,
    /// Whether the last byte seen was something other than a newline, so the final
    /// partial line is counted once at the end rather than on every poll.
    open_line: bool,
    /// Kept until the caller confirms durable persistence, including across PTY retirement.
    settled: Option<SettledCommand>,
}

impl Terminals {
    /// Start a command in its own pty, in this worktree, under the token its record
    /// already carries.
    ///
    /// The token comes in rather than being minted here because the record is written
    /// first: a press is on paper, with its token, before any process exists, so there is
    /// no moment in which something is running that nothing names.
    pub fn start_command(
        &self,
        worktree: &Path,
        label: &str,
        command: &str,
        token: &str,
    ) -> Result<ManagedCommand> {
        self.start_command_with(
            worktree,
            label,
            command,
            token,
            |terminals, worktree, label| terminals.open_command(worktree, label, command),
        )
    }

    /// The same, with the pty opened by the caller.
    ///
    /// The seam exists for tests: the production path starts the operator's login shell,
    /// which sources their rc files and writes their history, and a suite must never do
    /// that. A fixture opens a fixed program instead and the rest of this is identical.
    pub(crate) fn start_command_with(
        &self,
        worktree: &Path,
        label: &str,
        command: &str,
        token: &str,
        open: impl FnOnce(&Terminals, &Path, &str) -> Result<u64>,
    ) -> Result<ManagedCommand> {
        let session = open(self, worktree, label)?;
        self.retire_finished_commands(worktree, session);
        let token = token.to_owned();
        let worktree = worktree.to_string_lossy().into_owned();
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                token.clone(),
                Managed {
                    session,
                    label: label.to_owned(),
                    command: command.to_owned(),
                    worktree: worktree.clone(),
                    cursor: 0,
                    lines: 0,
                    lines_exact: true,
                    open_line: false,
                    settled: None,
                },
            );
        Ok(ManagedCommand {
            token,
            session,
            label: label.to_owned(),
            command: command.to_owned(),
            worktree,
        })
    }

    /// Read every managed command forward. Finished receipts are replayed until their
    /// durable write is acknowledged; the store's settlement write is idempotent.
    pub fn poll_commands(
        &self,
        worktree: Option<&Path>,
    ) -> (Vec<CommandReceipt>, Vec<SettledCommand>) {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut commands = self
            .commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut live = Vec::new();
        let mut settled = Vec::new();
        for (token, managed) in commands.iter_mut() {
            if let Some(outcome) = &managed.settled {
                settled.push(outcome.clone());
                continue;
            }
            let Some(session) = sessions.get(&managed.session) else {
                // The pty is gone: closed by hand, or by an update. There is no status to
                // read and inventing one would turn a killed build into a pass.
                let outcome = SettledCommand {
                    token: token.clone(),
                    exit_code: None,
                    duration_ms: 0,
                    lines: managed.total_lines(),
                    lines_exact: false,
                };
                managed.settled = Some(outcome.clone());
                settled.push(outcome);
                continue;
            };
            let chunk = session.chunk(managed.cursor);
            managed.advance(&chunk.text, chunk.dropped);
            managed.cursor = chunk.cursor;
            let duration_ms = session.ran_for().as_millis().try_into().unwrap_or(u64::MAX);
            if chunk.done {
                let outcome = SettledCommand {
                    token: token.clone(),
                    exit_code: chunk.status,
                    duration_ms,
                    lines: managed.total_lines(),
                    lines_exact: managed.lines_exact,
                };
                managed.settled = Some(outcome.clone());
                settled.push(outcome);
                continue;
            }
            if worktree.is_none_or(|path| managed.worktree == path.to_string_lossy()) {
                live.push(CommandReceipt {
                    token: token.clone(),
                    session: managed.session,
                    label: managed.label.clone(),
                    command: managed.command.clone(),
                    duration_ms,
                    lines: managed.total_lines(),
                    lines_exact: managed.lines_exact,
                });
            }
        }
        live.sort_by_key(|receipt| receipt.session);
        (live, settled)
    }

    /// Release a receipt only after the store durably accepted it (including a replay).
    pub fn acknowledge_command(&self, token: &str) {
        let mut commands = self
            .commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if commands
            .get(token)
            .is_some_and(|command| command.settled.is_some())
        {
            commands.remove(token);
        }
    }

    /// Close the oldest finished command sessions in a checkout.
    ///
    /// A session is kept until something closes it, so every press would otherwise leave
    /// a pty and its megabyte of scrollback behind for as long as the app runs. The last
    /// few stay because their output is the reason the pad was pressed; a running one is
    /// never touched, and nor is a shell.
    fn retire_finished_commands(&self, worktree: &Path, keep_newest: u64) {
        const RETAINED: usize = 5;
        // Cache outcomes before retiring scrollback, even when no window polled them.
        self.poll_commands(None);
        let worktree = worktree.to_string_lossy();
        let mut finished: Vec<u64> = {
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sessions
                .iter()
                .filter(|(id, session)| {
                    // Settled *and* drained, so the close below returns rather than
                    // waiting on a pipe: housekeeping must never be what makes the next
                    // press take five seconds.
                    let output = session
                        .output
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    **id != keep_newest
                        && session.kind == super::SessionKind::Command
                        && session.worktree == worktree
                        && output.settled
                        && output.drained
                })
                .map(|(id, _)| *id)
                .collect()
        };
        finished.sort_unstable();
        let retiring = finished.len().saturating_sub(RETAINED);
        for id in finished.into_iter().take(retiring) {
            // A close that will not drain keeps its session and its scrollback; this is
            // housekeeping, so there is nothing here to report and nothing to retry.
            let _ = self.close(id);
        }
    }

    /// Whether this process started a given command run. A row it cannot account for is
    /// one from an earlier app session, which is reported as unknown rather than as still
    /// running for ever.
    pub fn knows_command(&self, token: &str) -> bool {
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(token)
    }

    /// Wait for a managed command, for callers that need its outcome now.
    pub fn await_command(&self, token: &str, within: Duration) -> Option<SettledCommand> {
        let deadline = std::time::Instant::now() + within;
        loop {
            let (_, settled) = self.poll_commands(None);
            if let Some(found) = settled.into_iter().find(|item| item.token == token) {
                return Some(found);
            }
            if !self.knows_command(token) || std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Managed {
    fn advance(&mut self, text: &str, dropped: u64) {
        if dropped > 0 {
            // Bytes the ring discarded before this deck read them. The count that follows
            // is a floor from here on, and says so rather than looking precise.
            self.lines_exact = false;
        }
        if text.is_empty() {
            return;
        }
        self.lines += text.matches('\n').count() as u64;
        self.open_line = !text.ends_with('\n');
    }

    /// Output lines, counting a last line with no newline after it.
    fn total_lines(&self) -> u64 {
        self.lines + u64::from(self.open_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed program rather than the operator's login shell: the suite must not source
    /// their rc files or write their history (AGENTS.md).
    fn fixture(script: &str) -> impl Fn(&Terminals, &Path, &str) -> Result<u64> + '_ {
        move |terminals, worktree, _label| {
            terminals.open_program_as(
                worktree,
                "/bin/sh",
                &["-c".into(), script.into()],
                super::super::SessionKind::Command,
            )
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_managed_command_reports_its_own_exit_status_and_measured_duration() {
        // The point of the whole arrangement: the process is the command, so this status
        // is the command's rather than a shell's, and nothing was typed anywhere.
        let dir = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let started = terminals
            .start_command_with(
                dir.path(),
                "Check",
                "exit 7",
                "token-from-the-record",
                fixture("printf 'one\\ntwo\\n'; sleep 0.05; exit 7"),
            )
            .unwrap();
        assert_eq!(
            started.token, "token-from-the-record",
            "the run is named by the record that authorised it, not by the spawn"
        );

        let settled = terminals
            .await_command(&started.token, Duration::from_secs(30))
            .expect("a managed command settles");
        assert_eq!(settled.exit_code, Some(7));
        assert_eq!(settled.lines, 2);
        assert!(settled.lines_exact);
        assert!(settled.duration_ms >= 50, "{settled:?}");
        assert!(
            !terminals.poll_commands(None).1.is_empty(),
            "an observed outcome must survive a failed persistence attempt"
        );
        assert!(terminals.knows_command(&started.token));
        terminals.acknowledge_command(&started.token);
        assert!(!terminals.knows_command(&started.token));
        assert!(terminals.poll_commands(None).1.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_session_that_went_away_settles_as_unknown_rather_than_as_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let started = terminals
            .start_command_with(dir.path(), "Wait", "cat", "token-wait", fixture("cat"))
            .unwrap();
        terminals.close(started.session).unwrap();

        let (live, settled) = terminals.poll_commands(None);
        assert!(live.is_empty());
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].exit_code, None);
        assert!(!settled[0].lines_exact);
        assert_eq!(terminals.poll_commands(None).1.len(), 1);
        terminals.acknowledge_command(&started.token);
        assert!(terminals.poll_commands(None).1.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_running_command_is_listed_for_its_own_worktree_with_what_it_has_printed() {
        let mine = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let started = terminals
            .start_command_with(
                mine.path(),
                "Watch",
                "tail",
                "token-watch",
                fixture("printf 'first\\nsecond'; cat"),
            )
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (live, _) = terminals.poll_commands(Some(mine.path()));
            if live.first().is_some_and(|receipt| receipt.lines == 2) {
                assert_eq!(live[0].command, "tail");
                assert_eq!(live[0].session, started.session);
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{live:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            terminals.poll_commands(Some(other.path())).0.is_empty(),
            "a checkout lists its own commands"
        );
        terminals.close(started.session).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_checkout_keeps_its_recent_command_output_and_lets_the_rest_go() {
        // A session lives until something closes it, so without this every press leaves
        // a pty and its scrollback behind for the life of the app.
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let mut started = Vec::new();
        for index in 0..8 {
            let run = terminals
                .start_command_with(
                    dir.path(),
                    "Check",
                    "true",
                    &format!("token-retire-{index}"),
                    fixture(&format!("printf 'run {index}\n'")),
                )
                .unwrap();
            terminals
                .await_command(&run.token, Duration::from_secs(30))
                .expect("each command settles");
            started.push(run.session);
        }
        let shell = terminals.open_program(dir.path(), "/bin/cat", &[]).unwrap();
        let elsewhere = terminals
            .start_command_with(
                other.path(),
                "Check",
                "true",
                "token-elsewhere",
                fixture("true"),
            )
            .unwrap();

        let live: Vec<u64> = terminals.list(None).into_iter().map(|s| s.id).collect();
        let outcomes = terminals.poll_commands(None).1;
        for index in 0..8 {
            assert!(
                outcomes
                    .iter()
                    .any(|outcome| outcome.token == format!("token-retire-{index}")
                        && outcome.exit_code == Some(0)),
                "retirement must retain unacknowledged outcomes"
            );
        }
        assert!(live.contains(&shell), "a shell is never retired");
        assert!(
            live.contains(&elsewhere.session),
            "another checkout's commands are not this one's to close"
        );
        let kept: Vec<_> = started.iter().filter(|id| live.contains(id)).collect();
        assert_eq!(
            kept.len(),
            6,
            "the newest few, plus the one just started: started={started:?} live={live:?}"
        );
        assert_eq!(kept.last(), started.last().as_ref());
        terminals.close(shell).unwrap();
    }

    #[test]
    fn a_line_count_past_discarded_output_is_a_floor_and_says_so() {
        let mut managed = Managed {
            session: 1,
            label: "Check".into(),
            command: "x".into(),
            worktree: "/w".into(),
            cursor: 0,
            lines: 0,
            lines_exact: true,
            open_line: false,
            settled: None,
        };
        managed.advance("one\ntwo\n", 0);
        assert_eq!((managed.total_lines(), managed.lines_exact), (2, true));
        managed.advance("three", 0);
        assert_eq!(
            (managed.total_lines(), managed.lines_exact),
            (3, true),
            "a last line without a newline still counts, once"
        );
        managed.advance("", 0);
        assert_eq!(managed.total_lines(), 3);
        managed.advance("four\n", 4_096);
        assert_eq!(
            (managed.total_lines(), managed.lines_exact),
            (3, false),
            "output discarded between reads makes the count a floor, and it says so \
             rather than reading as a total"
        );
    }
}
