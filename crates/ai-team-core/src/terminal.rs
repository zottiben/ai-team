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

use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use serde::Serialize;

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
}

/// One pty and the process in it.
struct Session {
    /// Kept so the pane can be resized; dropping it closes the terminal.
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    /// Everything the process has written, capped.
    output: Arc<Mutex<Ring>>,
    done: Arc<Mutex<Option<i32>>>,
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

    /// Everything from `cursor` on, and the new cursor.
    fn read_from(&self, cursor: u64) -> (String, u64) {
        // A cursor behind what the ring still holds is clamped forward. The alternative is
        // an error the surface cannot act on - the bytes are gone either way.
        let start = cursor.max(self.dropped);
        // The difference cannot exceed the ring's own length, which is a `usize` already.
        let offset = usize::try_from(start - self.dropped).unwrap_or(usize::MAX);
        let slice = self.bytes.get(offset..).unwrap_or_default();
        (String::from_utf8_lossy(slice).into_owned(), self.total())
    }
}

/// Every terminal this process has open.
#[derive(Debug, Default)]
pub struct Terminals {
    sessions: Mutex<HashMap<u64, Session>>,
    next_id: AtomicU64,
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
}

impl Terminals {
    pub fn new() -> Terminals {
        Terminals::default()
    }

    /// Open a shell in a worktree.
    ///
    /// The operator's own login shell, because this is their terminal and it should
    /// behave the way theirs does - aliases, prompt and all. On macOS that is zsh (D12),
    /// which is why the shell is read from the environment rather than assumed.
    pub fn open(&self, worktree: &Path) -> Result<u64> {
        let system = NativePtySystem::default();
        let pair = system
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::invalid(format!("could not open a terminal: {error}")))?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.cwd(worktree);
        // Told what it is, or programs guess "dumb" and stop colouring anything.
        command.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| Error::invalid(format!("could not start a shell: {error}")))?;
        // Dropped immediately: while this end is open the terminal never reports EOF, so
        // a pane whose shell has exited would sit there looking alive.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| Error::invalid(format!("could not read the terminal: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::invalid(format!("could not write to the terminal: {error}")))?;

        let output = Arc::new(Mutex::new(Ring::default()));
        let done = Arc::new(Mutex::new(None));

        // A blocking thread rather than a task: portable-pty's reader is synchronous, and
        // a blocking read on an async runtime stalls every other request on that worker.
        {
            let output = Arc::clone(&output);
            std::thread::spawn(move || pump(reader, &output));
        }
        {
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let status = child
                    .wait()
                    .ok()
                    .map(|status| status.exit_code().cast_signed());
                *done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(status.unwrap_or(-1));
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
                    output,
                    done,
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
        let (text, cursor) = session
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read_from(cursor);
        let status = *session
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(Chunk {
            text,
            cursor,
            done: status.is_some(),
            status,
        })
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
            .map(|(id, session)| {
                let status = *session
                    .done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Listed {
                    id: *id,
                    worktree: session.worktree.clone(),
                    done: status.is_some(),
                    status,
                }
            })
            .collect();
        listed.sort_by_key(|entry| entry.id);
        listed
    }

    /// Close one. Dropping the master hangs up the terminal, which ends the shell.
    pub fn close(&self, id: u64) {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
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
            Ok(0) | Err(_) => return,
            Ok(read) => output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(&buffer[..read]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_reads_only_what_is_new() {
        // The same rule as ingesting eve's stream: absolute, so a reconnect asks from a
        // number it already has.
        let mut ring = Ring::default();
        ring.push(b"hello ");
        let (text, cursor) = ring.read_from(0);
        assert_eq!(text, "hello ");
        assert_eq!(cursor, 6);

        ring.push(b"world");
        let (text, cursor) = ring.read_from(cursor);
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

        let (text, cursor) = ring.read_from(0);
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

    #[cfg(unix)]
    #[test]
    fn a_command_run_in_a_terminal_streams_and_reports_its_status() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "hi").unwrap();

        let terminals = Terminals::new();
        let id = terminals.open(dir.path()).unwrap();
        terminals.write(id, "ls\nexit\n").unwrap();

        // Polled: a shell takes a moment to start, echo and exit.
        let mut text = String::new();
        let mut cursor = 0;
        for _ in 0..100 {
            let chunk = terminals.read(id, cursor).unwrap();
            text.push_str(&chunk.text);
            cursor = chunk.cursor;
            if chunk.done && text.contains("marker.txt") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert!(text.contains("marker.txt"), "{text}");
        assert!(terminals.read(id, cursor).unwrap().done);
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
        let id = terminals.open(&real).unwrap();
        terminals.write(id, "ls only-here.txt\nexit\n").unwrap();

        let mut text = String::new();
        let mut cursor = 0;
        for _ in 0..100 {
            let chunk = terminals.read(id, cursor).unwrap();
            text.push_str(&chunk.text);
            cursor = chunk.cursor;
            if chunk.done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(text.contains("only-here.txt"), "{text}");
        assert!(!text.contains("No such file"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn sessions_are_listed_per_worktree_so_a_window_finds_its_own() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();

        let a = terminals.open(one.path()).unwrap();
        terminals.open(two.path()).unwrap();

        assert_eq!(terminals.list(None).len(), 2);
        let mine = terminals.list(Some(one.path()));
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, a);

        terminals.close(a);
        assert_eq!(terminals.list(Some(one.path())).len(), 0);
    }
}
