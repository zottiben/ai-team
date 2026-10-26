//! A real terminal, one per worktree.
//!
//! Not a shell reimplementation and not a skelly replacement: enough to run a gate by
//! hand and read what it says. That means a genuine pty rather than piped stdio, because
//! everything worth running here - cargo, npm, git - checks whether it is talking to a
//! terminal and produces different output when it is not. A "terminal" built on pipes
//! shows you colourless, unprogressed output that does not match what the same command
//! prints in your own shell, which is the one thing it exists to reproduce.
//!
//! The session outlives the window. `ait ui` and the desktop shell come and go; a
//! `cargo test` that takes four minutes should not die because somebody closed a tab. So
//! output accumulates in a ring buffer the surface reads from a cursor, in exactly the
//! way eve's stream is ingested - a reconnect re-reads from where it left off rather than
//! starting a new process.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

mod commands;
mod update;
pub use commands::{CommandReceipt, ManagedCommand, SettledCommand};
use serde::Serialize;
pub use update::TerminalUpdate;

use crate::error::{Error, Result};

/// How much scrollback a session keeps.
///
/// A megabyte is a long `cargo build` and a short `npm ci`. The cap exists because a
/// session survives the window: something has to stop a loop printing for an hour from
/// taking the machine with it.
const SCROLLBACK: usize = 1024 * 1024;

/// What a reader gets back.
#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    /// Bytes as text. Lossy on purpose: a terminal stream is not guaranteed UTF-8 - a
    /// build printing a raw byte must not stop the pane rendering everything after it.
    pub text: String,
    /// Where to read from next. Absolute, not a delta: a client that reconnects asks from
    /// a number it already has, and a relative cursor would double-count on a rewind.
    pub cursor: u64,
    /// True once the process has ended.
    pub done: bool,
    /// Its exit code, once there is one.
    pub status: Option<i32>,
    /// How many bytes the ring discarded before this reader's cursor reached them.
    ///
    /// A clamped cursor used to look like a clean read, so a pane showed a seam it could
    /// not explain and a line count derived from it was quietly short. Said rather than
    /// smoothed over: the bytes are gone either way, but the gap is a fact about the
    /// output and anything counting lines has to know its total is a floor.
    pub dropped: u64,
}

/// What a session is for.
///
/// A tab strip has to tell the operator's own shell from a command ai-team is running on
/// their behalf, and neither may be confused with a sign-in the app started. Carried as a
/// field rather than read back out of the label, because a label is prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// The operator's own login shell, typed into by hand.
    Shell,
    /// One command ai-team started, in its own pty, from an explicit press.
    Command,
    /// A session the app owns: a provider sign-in, or an exact program.
    App,
}

/// One pty and the process in it.
struct Session {
    /// Kept so the pane can be resized; dropping it closes the terminal.
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    closing: bool,
    label: String,
    kind: SessionKind,
    /// Everything the process has written, capped.
    output: Arc<Mutex<Ring>>,
    done: Arc<Mutex<Option<i32>>>,
    /// When the process started and, once it has, when it ended - both on the monotonic
    /// clock, because a managed command's duration is a measurement rather than the
    /// difference of two wall-clock strings written a batch apart.
    started: Instant,
    ended: Arc<Mutex<Option<Instant>>>,
    /// Which worktree it is rooted in, so a surface can find its own.
    worktree: String,
}

/// A bounded byte log that remembers how much it has dropped.
///
/// The offset is what makes a cursor meaningful after a trim: a client reading from 900k
/// when the ring has discarded the first 200k must be told it has fallen behind rather
/// than silently handed the wrong bytes.
#[derive(Debug, Default)]
struct Ring {
    bytes: Vec<u8>,
    dropped: u64,
    drained: bool,
    settled: bool,
    problem: Option<String>,
    update_permit: Option<crate::UpdatePermit>,
}

impl Ring {
    fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        if self.bytes.len() > SCROLLBACK {
            let excess = self.bytes.len() - SCROLLBACK;
            self.bytes.drain(..excess);
            self.dropped += excess as u64;
        }
        debug_assert!(self.bytes.len() <= SCROLLBACK);
    }

    fn total(&self) -> u64 {
        self.dropped + self.bytes.len() as u64
    }

    /// Everything from `cursor` on, the new cursor, and how much was missed.
    fn read_from(&self, cursor: u64) -> (String, u64, u64) {
        // A cursor behind what the ring still holds is clamped forward. The alternative is
        // an error the surface cannot act on - the bytes are gone either way. What it is
        // not is silent: the clamp distance is reported so a pane can say it lost output.
        let start = cursor.max(self.dropped);
        // The difference cannot exceed the ring's own length, which is a `usize` already.
        let offset = usize::try_from(start - self.dropped).unwrap_or(usize::MAX);
        let slice = self.bytes.get(offset..).unwrap_or_default();
        (
            String::from_utf8_lossy(slice).into_owned(),
            self.total(),
            start - cursor,
        )
    }
}

/// Every terminal this process has open.
#[derive(Debug)]
pub struct Terminals {
    sessions: Mutex<HashMap<u64, Session>>,
    /// The commands this process started, until each one's outcome has been taken.
    commands: Mutex<HashMap<String, commands::Managed>>,
    next_id: AtomicU64,
    scope: String,
}

impl Default for Terminals {
    fn default() -> Self {
        Self {
            sessions: Mutex::default(),
            commands: Mutex::default(),
            next_id: AtomicU64::default(),
            scope: crate::mint_token(),
        }
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("worktree", &self.worktree)
            .finish_non_exhaustive()
    }
}

/// A session as the window lists it.
#[derive(Debug, Clone, Serialize)]
pub struct Listed {
    pub id: u64,
    pub worktree: String,
    pub done: bool,
    pub status: Option<i32>,
    /// What this session is for, and what it was started as. A tab names a session with
    /// these rather than guessing from its output.
    pub kind: SessionKind,
    pub label: String,
    /// How long the process has been running, or ran for. Measured, not derived from
    /// timestamps.
    pub duration_ms: u64,
}

impl Session {
    /// What a reader sees of this session right now.
    fn chunk(&self, cursor: u64) -> Chunk {
        let (text, cursor, dropped) = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read_from(cursor);
        let status = *self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Chunk {
            text,
            cursor,
            done: status.is_some(),
            status,
            dropped,
        }
    }

    fn listed(&self, id: u64) -> Listed {
        let status = *self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Listed {
            id,
            worktree: self.worktree.clone(),
            done: status.is_some(),
            status,
            kind: self.kind,
            label: self.label.clone(),
            duration_ms: self.ran_for().as_millis().try_into().unwrap_or(u64::MAX),
        }
    }

    /// How long the process has run. Final once it has ended, so a receipt read twice
    /// does not keep growing after the thing it measures has stopped.
    fn ran_for(&self) -> std::time::Duration {
        self.ended
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(self.started)
    }
}

impl Terminals {
    pub fn new() -> Terminals {
        Terminals::default()
    }

    /// Keep updates out of a terminal's lifetime, including inherited output pipes.
    /// Finished sessions may retain scrollback without retaining the permit.
    pub fn protect_update(&self, id: u64, permit: crate::UpdatePermit) -> Result<()> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = sessions
            .get(&id)
            .ok_or_else(|| Error::invalid("terminal not found"))?;
        let mut output = session
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !output.drained || !output.settled {
            output.update_permit = Some(permit);
        }
        Ok(())
    }

    /// Open a shell in a worktree.
    ///
    /// The operator's own login shell, because this is their terminal and it should
    /// behave the way theirs does - aliases, prompt and all. On macOS that is zsh (D12),
    /// which is why the shell is read from the environment rather than assumed.
    pub fn open(&self, worktree: &Path) -> Result<u64> {
        self.open_running(worktree, None)
    }

    /// The same, running one command instead of waiting for you to type.
    ///
    /// This is how the setup page signs a provider in (D25). It is not a weakening of
    /// D17: `claude auth login` and `codex login` install nothing, touch only the
    /// operator's own accounts, and cannot run unattended - they open a browser and wait
    /// for a person. Installing a neighbour is still a command that is copied and never
    /// run.
    ///
    /// The shell is a login shell so the command is looked up the way it would be in a
    /// terminal, which matters for the same reason `launch_path` does: a window launched
    /// from Finder has been handed launchd's PATH.
    pub fn open_running(&self, worktree: &Path, command: Option<&str>) -> Result<u64> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut spawn = CommandBuilder::new(shell);
        if let Some(line) = command {
            spawn.args(["-l", "-c", line]);
        }
        self.open_builder(
            worktree,
            spawn,
            if command.is_some() {
                "Provider sign-in"
            } else {
                "Shell"
            },
            if command.is_some() {
                SessionKind::App
            } else {
                SessionKind::Shell
            },
        )
    }

    /// Run one command in a pty of its own, labelled with what the operator pressed.
    ///
    /// Not a keystroke into a shell somebody is using: a pad that typed into the
    /// operator's session would race whatever they had half-typed, and the pty's exit
    /// status would belong to the shell rather than to the command. Here the process *is*
    /// the command, so its status and its measured lifetime are the command's own.
    ///
    /// The shell is a login shell for the same reason `open_running` uses one - the
    /// command has to resolve the way it would in the operator's own terminal.
    pub fn open_command(&self, worktree: &Path, label: &str, command: &str) -> Result<u64> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut spawn = CommandBuilder::new(shell);
        spawn.args(["-l", "-c", command]);
        self.open_builder(worktree, spawn, label, SessionKind::Command)
    }

    /// Open one program with exact arguments, without putting them through a shell.
    ///
    /// The OAuth setup route uses this for Pi. Its config path comes from the machine's
    /// data directory, which can contain spaces or shell metacharacters; passing an argv
    /// means none of those become syntax and the route never grows into a loopback shell.
    pub fn open_program(&self, worktree: &Path, program: &str, args: &[String]) -> Result<u64> {
        self.open_program_as(worktree, program, args, SessionKind::App)
    }

    /// The same, as a session of a given kind.
    ///
    /// Only a fixture needs this: a test must not start the operator's login shell
    /// (AGENTS.md), but a managed command opened as anything else would not be the thing
    /// under test - the kind is what tells a tab, and this module's housekeeping, that a
    /// session is one ai-team ran rather than one somebody is typing into.
    pub(crate) fn open_program_as(
        &self,
        worktree: &Path,
        program: &str,
        args: &[String],
        kind: SessionKind,
    ) -> Result<u64> {
        let mut spawn = CommandBuilder::new(program);
        spawn.args(args);
        self.open_builder(worktree, spawn, program, kind)
    }

    /// An explicit context-source login, not a coding turn or an ordinary user shell.
    /// Keep Pi's auth storage/extensions, but only discover the supplied MCP definition.
    pub fn open_context_auth(&self, home: &Path, config: &Path) -> Result<u64> {
        let mut spawn = CommandBuilder::new("pi");
        spawn.args([
            "--no-session",
            "--no-context-files",
            "--no-skills",
            "--no-builtin-tools",
            "--no-approve",
            "--mcp-config",
        ]);
        spawn.arg(config);
        spawn.env("PI_MCP_CONFIG_MODE", "exclusive");
        for key in crate::pi::METERED_MODEL_ENV {
            spawn.env_remove(key);
        }
        self.open_builder(home, spawn, "Pi context authentication", SessionKind::App)
    }

    fn open_builder(
        &self,
        worktree: &Path,
        mut spawn: CommandBuilder,
        label: &str,
        kind: SessionKind,
    ) -> Result<u64> {
        let system = NativePtySystem::default();
        let pair = system
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::invalid(format!("could not open a terminal: {error}")))?;

        spawn.cwd(worktree);
        // Told what it is, or programs guess "dumb" and stop colouring anything.
        spawn.env("TERM", "xterm-256color");

        // Both ends of the master are taken *before* anything is started. They depend on
        // the pty, not on the child, and taking them afterwards left the one hole this
        // whole module is arranged against: a process running with nothing holding it,
        // because the call that was to read it failed and dropped the handle. Nothing
        // below this line can fail, so an error from here provably ran nothing - which is
        // what lets a caller say so instead of guessing.
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| Error::invalid(format!("could not read the terminal: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::invalid(format!("could not write to the terminal: {error}")))?;

        let child = pair.slave.spawn_command(spawn).map_err(|error| {
            Error::invalid(format!("could not start {label} in a terminal: {error}"))
        })?;
        // Dropped immediately: while this end is open the terminal never reports EOF, so
        // a pane whose shell has exited would sit there looking alive.
        drop(pair.slave);

        #[cfg(unix)]
        let group = child.process_id();
        let child = Arc::new(Mutex::new(child));
        let output = Arc::new(Mutex::new(Ring::default()));
        let done = Arc::new(Mutex::new(None));
        let started = Instant::now();
        let ended: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        // A blocking thread rather than a task: portable-pty's reader is synchronous, and
        // a blocking read on an async runtime stalls every other request on that worker.
        {
            let output = Arc::clone(&output);
            std::thread::spawn(move || pump(reader, &output));
        }
        {
            let done = Arc::clone(&done);
            let ended = Arc::clone(&ended);
            let output = Arc::clone(&output);
            let child = Arc::clone(&child);
            std::thread::spawn(move || {
                // Reaping and signalling share this handle/lock. A copied PID could
                // otherwise be reused between wait completing and a close request.
                let status = loop {
                    let result = child
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .try_wait();
                    match result {
                        Ok(Some(status)) => break Some(status.exit_code().cast_signed()),
                        Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
                        Err(error) => {
                            output
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .problem = Some(format!("waiting for the terminal: {error}"));
                            break None;
                        }
                    }
                };
                // The end time is recorded before the status, so a reader that sees a
                // finished session never sees it without the measurement that goes with
                // it - a receipt with an exit code and no duration reads like a bug.
                *ended
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
                *done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(status.unwrap_or(-1));
                if status.is_some() {
                    // macOS can revoke the PTY when its leader exits even while a
                    // descendant still owns the group. EOF alone is not its drain.
                    #[cfg(unix)]
                    while !update::group_drained(group) {
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    let mut output = output
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    output.settled = true;
                    if output.drained {
                        output.update_permit.take();
                    }
                }
            });
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id,
                Session {
                    master: pair.master,
                    writer,
                    child,
                    closing: false,
                    label: label.into(),
                    kind,
                    output,
                    done,
                    started,
                    ended,
                    worktree: worktree.to_string_lossy().into_owned(),
                },
            );
        Ok(id)
    }

    /// Send keystrokes.
    pub fn write(&self, id: u64, text: &str) -> Result<()> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = sessions.get_mut(&id).ok_or_else(no_such)?;
        if session.closing {
            return Err(Error::invalid(
                "this terminal is closing; input was not sent",
            ));
        }
        session
            .writer
            .write_all(text.as_bytes())
            .and_then(|()| session.writer.flush())
            .map_err(|error| Error::invalid(format!("writing to the terminal: {error}")))
    }

    /// Read from a cursor.
    pub fn read(&self, id: u64, cursor: u64) -> Result<Chunk> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = sessions.get(&id).ok_or_else(no_such)?;
        Ok(session.chunk(cursor))
    }

    /// Tell the process how big the window is.
    ///
    /// Not cosmetic: a shell that thinks it has eighty columns wraps at eighty, and the
    /// output arrives already broken in a way no amount of CSS unbreaks.
    pub fn resize(&self, id: u64, rows: u16, cols: u16) -> Result<()> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = sessions.get(&id).ok_or_else(no_such)?;
        session
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::invalid(format!("resizing the terminal: {error}")))
    }

    /// Every open session, so a reconnecting window finds the one it had.
    pub fn list(&self, worktree: Option<&Path>) -> Vec<Listed> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut listed: Vec<Listed> = sessions
            .iter()
            .filter(|(_, session)| {
                worktree.is_none_or(|path| session.worktree == path.to_string_lossy())
            })
            .map(|(id, session)| session.listed(*id))
            .collect();
        listed.sort_by_key(|entry| entry.id);
        listed
    }

    /// Signal, wait and drain before dropping the writer: portable-pty's writer Drop
    /// sends Enter/EOF, which must never execute an unfinished command on close.
    pub fn close(&self, id: u64) -> Result<()> {
        let output = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(session) = sessions.get_mut(&id) else {
                return Ok(());
            };
            update::stop(session)?;
            Arc::clone(&session.output)
        };
        update::drain(&[output])?;
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        Ok(())
    }
}

fn no_such() -> Error {
    Error::invalid("no such terminal - it may have been closed")
}

/// Copy the pty into the ring until it hangs up.
fn pump(mut reader: Box<dyn Read + Send>, output: &Arc<Mutex<Ring>>) {
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer) {
            // Retry interruptions; they neither drain nor fail the terminal.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if !pty_hangup(&error) => {
                output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .problem = Some(format!("reading the terminal: {error}"));
                return;
            }
            Ok(0) | Err(_) => {
                let mut output = output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                output.drained = true;
                if output.settled {
                    output.update_permit.take();
                }
                return;
            }
            Ok(read) => output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(&buffer[..read]),
        }
    }
}

fn pty_hangup(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error())
    }
    #[cfg(not(unix))]
    {
        error.kind() == std::io::ErrorKind::BrokenPipe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unexpected_read_error_is_not_proof_of_terminal_drain() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture read failure"))
            }
        }
        let output = Arc::new(Mutex::new(Ring::default()));
        pump(Box::new(Broken), &output);
        assert!(!output.lock().unwrap().drained);
    }

    #[test]
    fn closing_a_terminal_does_not_execute_its_pending_input() {
        let root = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let id = terminals
            .open_program(
                root.path(),
                "/bin/sh",
                &[
                    "-c".into(),
                    "printf 'ready\\n'; IFS= read -r line; printf '%s' \"$line\" > executed".into(),
                ],
            )
            .unwrap();
        let started = std::time::Instant::now();
        while !terminals.read(id, 0).unwrap().text.contains("ready") {
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        terminals.write(id, "this must remain unexecuted").unwrap();
        terminals.close(id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !root.path().join("executed").exists(),
            "closing must not send Enter to a terminal"
        );
    }

    #[test]
    fn a_cursor_reads_only_what_is_new() {
        // The same rule as ingesting eve's stream: absolute, so a reconnect asks from a
        // number it already has.
        let mut ring = Ring::default();
        ring.push(b"hello ");
        let (text, cursor, _) = ring.read_from(0);
        assert_eq!(text, "hello ");
        assert_eq!(cursor, 6);

        ring.push(b"world");
        let (text, cursor, _) = ring.read_from(cursor);
        assert_eq!(text, "world");
        assert_eq!(cursor, 11);

        // And reading again from the end returns nothing rather than repeating.
        assert_eq!(ring.read_from(cursor).0, "");
    }

    #[test]
    fn a_rewind_replays_rather_than_double_counting() {
        let mut ring = Ring::default();
        ring.push(b"abcdef");
        assert_eq!(ring.read_from(0).0, "abcdef");
        assert_eq!(ring.read_from(3).0, "def");
        assert_eq!(ring.read_from(0).1, 6, "the cursor is absolute");
    }

    #[test]
    fn scrollback_is_capped_and_the_cursor_survives_the_trim() {
        // A session outlives the window, so something has to stop an hour of output
        // taking the machine with it - and a client reading from before the trim must get
        // what is left rather than the wrong bytes.
        let mut ring = Ring::default();
        ring.push(&vec![b'x'; SCROLLBACK]);
        ring.push(b"tail");

        assert_eq!(ring.bytes.len(), SCROLLBACK);
        assert_eq!(ring.total(), SCROLLBACK as u64 + 4);

        let (text, cursor, dropped) = ring.read_from(0);
        assert_eq!(dropped, 4, "a clamped cursor says how much it missed");
        assert!(text.ends_with("tail"));
        assert_eq!(cursor, ring.total());
    }

    #[test]
    fn a_byte_that_is_not_utf8_does_not_stop_the_pane() {
        // A build printing a raw byte must not blank everything after it.
        let mut ring = Ring::default();
        ring.push(&[0xff, 0xfe]);
        ring.push(b" and then text");
        assert!(ring.read_from(0).0.contains("and then text"));
    }

    #[test]
    fn a_multibyte_character_split_across_two_writes_still_reads() {
        // A read boundary lands wherever the kernel puts it, including inside a `→`.
        let arrow = "→".as_bytes();
        let mut ring = Ring::default();
        ring.push(&arrow[..1]);
        ring.push(&arrow[1..]);
        assert_eq!(ring.read_from(0).0, "→");
    }

    #[test]
    fn asking_about_a_terminal_that_is_gone_says_so() {
        let terminals = Terminals::new();
        assert!(terminals.read(99, 0).is_err());
        assert!(terminals.write(99, "ls\n").is_err());
        assert!(terminals.resize(99, 10, 10).is_err());
    }

    // The tests below run fixed programs, never `open`'s login shell. Typing into the
    // operator's shell runs their rc files and writes every line into their history -
    // `sharehistory` appends it at once, under a lock that shells started side by side in
    // the suite then queue on until an `exit` misses its deadline. What is under test is
    // the terminal - the pty, the pump, the status - and `open` differs from
    // `open_program` only in which program it starts.

    /// Read a session until its program has ended having printed `expected`, or until a
    /// deadline. Measured in time rather than in polls: the suite runs git, Pi and
    /// language servers beside this, and a count of sleeps is only as long as the
    /// scheduler makes it.
    #[cfg(unix)]
    fn read_until_ended(terminals: &Terminals, id: u64, expected: &str) -> Chunk {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut text = String::new();
        let mut cursor = 0;
        loop {
            let chunk = terminals.read(id, cursor).unwrap();
            text.push_str(&chunk.text);
            cursor = chunk.cursor;
            if (chunk.done && text.contains(expected)) || std::time::Instant::now() > deadline {
                return Chunk { text, ..chunk };
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_command_run_in_a_terminal_streams_and_reports_its_status() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "hi").unwrap();

        let terminals = Terminals::new();
        let id = terminals
            .open_program(
                dir.path(),
                "/bin/sh",
                &[
                    "-c".to_string(),
                    r#"read line; echo "got $line"; ls; exit 3"#.to_string(),
                ],
            )
            .unwrap();
        terminals.write(id, "typed\n").unwrap();

        let ended = read_until_ended(&terminals, id, "marker.txt");
        // What was typed reached the program, what it printed came back, and so did how
        // it ended - a status that is not the default, so a zero read by mistake shows.
        assert!(ended.text.contains("got typed"), "{}", ended.text);
        assert!(ended.text.contains("marker.txt"), "{}", ended.text);
        assert!(ended.done, "{}", ended.text);
        assert_eq!(ended.status, Some(3), "{}", ended.text);
    }

    #[cfg(unix)]
    #[test]
    fn an_exact_program_argument_is_not_reinterpreted_by_a_shell() {
        // OAuth passes a config path from AI_TEAM_HOME. A quote, semicolon or space in
        // that path must remain one argv item rather than becoming shell syntax.
        let dir = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let marker = "one argument; $(not-a-command) ' with spaces".to_string();
        let id = terminals
            .open_program(
                dir.path(),
                "/usr/bin/printf",
                &["%s".to_string(), marker.clone()],
            )
            .unwrap();

        let ended = read_until_ended(&terminals, id, &marker);
        assert!(ended.text.contains(&marker), "{}", ended.text);
    }

    #[cfg(unix)]
    #[test]
    fn the_terminal_is_rooted_in_the_worktree_it_was_opened_for() {
        // The whole point of one per worktree: a gate run by hand has to run against the
        // agent's work, not against wherever the window happened to start.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        std::fs::write(real.join("only-here.txt"), "x").unwrap();

        let terminals = Terminals::new();
        let id = terminals
            .open_program(&real, "/bin/ls", &["only-here.txt".to_string()])
            .unwrap();

        let ended = read_until_ended(&terminals, id, "only-here.txt");
        assert!(ended.text.contains("only-here.txt"), "{}", ended.text);
        // `ls` of a name that is not there fails, so the status says where it ran.
        assert_eq!(ended.status, Some(0), "{}", ended.text);
    }

    #[cfg(unix)]
    #[test]
    fn sessions_are_listed_per_worktree_so_a_window_finds_its_own() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();

        // `cat` waits on the terminal until it hangs up, so both stay open until closed.
        let a = terminals.open_program(one.path(), "/bin/cat", &[]).unwrap();
        terminals.open_program(two.path(), "/bin/cat", &[]).unwrap();

        assert_eq!(terminals.list(None).len(), 2);
        let mine = terminals.list(Some(one.path()));
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, a);

        terminals.close(a).unwrap();
        assert_eq!(terminals.list(Some(one.path())).len(), 0);
    }
}
