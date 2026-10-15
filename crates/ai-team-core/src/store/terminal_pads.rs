//! Command pads a project owns, and the receipts pressing one leaves. The only place
//! their SQL lives.
//!
//! Three properties this file exists to hold.
//!
//! A pad is pressed against what the operator read: the press carries the command text,
//! the pad's revision and the checkout, and is refused if any of them has moved since,
//! because a one-press button whose command changed under you is a button that runs
//! something you did not read.
//!
//! A press happens once. The window mints a `request_id` when it opens the confirmation,
//! and that id is unique here, so a repeated POST - a lost response, a reload, a double
//! click - is answered with the receipt the first one left rather than starting the
//! command again. A request id that arrives with different bytes is a different press
//! wearing the same name, and is refused rather than answered with the wrong receipt.
//!
//! And the record comes before the process. The row is written while nothing is running
//! yet, carrying the token the spawn will use, so there is no window in which a command is
//! running that nothing has recorded. What that costs is a row that may outlive a crash
//! still saying `intent`: that row is reported as an uncertain outcome, which is the one
//! honest answer, and it is never re-run on the strength of a guess.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SettledCommand, Store};

/// How many pads a project may keep. The grid is six; the extra room is for a seventh
/// somebody adds before deleting one, not an unbounded list.
const PADS: usize = 12;
/// History page size. Durable receipts remain for idempotency even after leaving the
/// page; unsettled rows are always included.
const HISTORY: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPad {
    pub id: i64,
    pub position: i64,
    /// Bumped on every edit. The press carries the revision it was confirmed against, so
    /// an edit between reading a pad and pressing it is seen even when the new command
    /// happens to be the old one again.
    pub revision: i64,
    pub label: String,
    pub command: String,
}

/// How far a press got before anything else happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dispatch {
    /// Recorded, with no process confirmed started. One may exist.
    Intent,
    /// The pty is running, or has run, and `session_id` names it.
    Started,
    /// The spawn failed with nothing left running. Provably nothing ran.
    Refused,
}

impl Dispatch {
    fn parse(value: &str) -> Dispatch {
        match value {
            "started" => Dispatch::Started,
            "refused" => Dispatch::Refused,
            // Anything else is a row whose progress is not established, which is what
            // `intent` means. Reading an unknown value as `started` would report a
            // process nobody has evidence of.
            _ => Dispatch::Intent,
        }
    }
}

/// What a press left behind, as the window reads it back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalCommandRow {
    pub id: i64,
    pub request_id: String,
    pub token: String,
    pub pad_id: i64,
    pub revision: i64,
    pub label: String,
    pub command: String,
    pub worktree: String,
    pub dispatch: Dispatch,
    /// Why nothing ran, on a refused row.
    pub refusal: Option<String>,
    /// The pty, once one is confirmed started. `None` on a row that never got that far.
    pub session_id: Option<i64>,
    pub claimed_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub lines: Option<i64>,
    pub lines_exact: bool,
}

/// Everything a press is confirmed against. All of it is part of the request's identity,
/// so a repeat that differs in any of it is a different press.
#[derive(Debug, Clone, Copy)]
pub struct PressRequest<'a> {
    /// Minted by the window when the confirmation opened, and the same on every retry of
    /// that one press.
    pub request_id: &'a str,
    /// The checkout the command will run in, already resolved and canonical. The window
    /// sends back what it was shown and the caller compares the two, so a deck that has
    /// moved cannot run a command in a checkout nobody confirmed.
    pub worktree: &'a str,
    pub pad: i64,
    pub revision: i64,
    pub confirmed: &'a str,
}

/// A press that has been recorded and not yet started.
#[derive(Debug, Clone)]
pub struct ClaimedPress {
    pub row: i64,
    /// Minted with the record rather than by the spawn, so an outcome observed later
    /// lands on this run even if the write that was to name its pty never completed.
    pub token: String,
    pub label: String,
    pub command: String,
    pub worktree: String,
}

/// What a press turns out to be.
#[derive(Debug, Clone)]
pub enum DispatchClaim {
    /// New: the intent is recorded and nothing is running yet. Start it.
    Fresh(ClaimedPress),
    /// This exact request already has a row. Hand its receipt back, and run nothing.
    Replay(TerminalCommandRow),
}

const ROW_COLUMNS: &str = "id, request_id, token, pad_id, revision, label, command, worktree,
     dispatch, refusal, session_id, claimed_at, started_at, finished_at, exit_code,
     duration_ms, lines, lines_exact";

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerminalCommandRow> {
    Ok(TerminalCommandRow {
        id: row.get(0)?,
        request_id: row.get(1)?,
        token: row.get(2)?,
        pad_id: row.get(3)?,
        revision: row.get(4)?,
        label: row.get(5)?,
        command: row.get(6)?,
        worktree: row.get(7)?,
        dispatch: Dispatch::parse(&row.get::<_, String>(8)?),
        refusal: row.get(9)?,
        session_id: row.get(10)?,
        claimed_at: row.get(11)?,
        started_at: row.get(12)?,
        finished_at: row.get(13)?,
        exit_code: row.get(14)?,
        duration_ms: row.get(15)?,
        lines: row.get(16)?,
        lines_exact: row.get::<_, i64>(17)? == 1,
    })
}

fn trimmed(field: &str, value: &str, max: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::invalid(format!("a pad needs {field}")));
    }
    if value.chars().count() > max {
        return Err(Error::invalid(format!(
            "that pad {field} is longer than {max} characters"
        )));
    }
    // A control character in a command is a keystroke nobody typed - a newline would make
    // one pad two commands, and the press confirmation would show only the first.
    if value.chars().any(char::is_control) {
        return Err(Error::invalid(format!(
            "a pad {field} cannot contain control characters or line breaks"
        )));
    }
    Ok(value.to_owned())
}

/// What a request id says it is a press of.
///
/// Length-prefixed, so no field can be made to look like the end of the one before it: a
/// command ending in the separator must not be able to pass itself off as a press of a
/// different pad in a different checkout. The command is trimmed here for the same reason
/// it is trimmed before the pad is compared - one press must not have two identities
/// because a space came along for the ride.
fn fingerprint(project: i64, request: &PressRequest<'_>) -> String {
    let mut parts = String::new();
    for field in [
        project.to_string(),
        request.worktree.to_owned(),
        request.pad.to_string(),
        request.revision.to_string(),
        request.confirmed.trim().to_owned(),
    ] {
        parts.push_str(&field.len().to_string());
        parts.push(':');
        parts.push_str(&field);
        parts.push('\u{1f}');
    }
    crate::update::sha256(parts.as_bytes())
}

/// A client-minted identity, held to a shape a receipt can be looked up by. Short enough
/// to be an accident, or carrying anything that is not plainly an identifier, and the
/// press is refused before a row exists rather than recorded under a name that will not
/// come back.
fn request_id(value: &str) -> Result<String> {
    let value = value.trim();
    let shaped = value.len() >= 16
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !shaped {
        return Err(Error::invalid(
            "that press carried no usable request id; nothing was run",
        ));
    }
    Ok(value.to_owned())
}

impl Store {
    /// This project's pads, in the order they are shown. Never seeded: an empty project
    /// has no pads, because a command nobody chose is not a default.
    pub fn terminal_pads(&self, project: i64) -> Result<Vec<TerminalPad>> {
        let mut query = self.db().conn().prepare(
            "SELECT id, position, revision, label, command FROM terminal_pad
             WHERE project_id=?1 ORDER BY position, id",
        )?;
        let pads = query
            .query_map([project], |row| {
                Ok(TerminalPad {
                    id: row.get(0)?,
                    position: row.get(1)?,
                    revision: row.get(2)?,
                    label: row.get(3)?,
                    command: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(pads)
    }

    pub fn add_terminal_pad(
        &mut self,
        project: i64,
        label: &str,
        command: &str,
    ) -> Result<TerminalPad> {
        let label = trimmed("label", label, 40)?;
        let command = trimmed("command", command, 2_000)?;
        let at = crate::now();
        self.db_mut().write(|tx| {
            let count: usize = tx.query_row(
                "SELECT COUNT(*) FROM terminal_pad WHERE project_id=?1",
                [project],
                |row| row.get(0),
            )?;
            if count >= PADS {
                return Err(Error::invalid(format!(
                    "this project already has {PADS} pads; edit or remove one first"
                )));
            }
            let position: i64 = tx.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM terminal_pad WHERE project_id=?1",
                [project],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO terminal_pad(project_id, position, revision, label, command, created_at, updated_at)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, ?5)",
                params![project, position, label, command, at],
            )?;
            Ok(TerminalPad {
                id: tx.last_insert_rowid(),
                position,
                revision: 1,
                label,
                command,
            })
        })
    }

    /// Edit a pad, and say so by bumping its revision. A press confirmed against the pad
    /// as it was is refused after this, whatever the new text happens to be.
    pub fn edit_terminal_pad(
        &mut self,
        project: i64,
        id: i64,
        label: &str,
        command: &str,
    ) -> Result<TerminalPad> {
        let label = trimmed("label", label, 40)?;
        let command = trimmed("command", command, 2_000)?;
        let at = crate::now();
        self.db_mut().write(|tx| {
            let found: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT position, revision FROM terminal_pad WHERE id=?1 AND project_id=?2",
                    params![id, project],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (position, revision) = found.ok_or_else(|| Error::invalid("no such pad"))?;
            let revision = revision + 1;
            tx.execute(
                "UPDATE terminal_pad SET label=?1, command=?2, revision=?3, updated_at=?4
                 WHERE id=?5",
                params![label, command, revision, at, id],
            )?;
            Ok(TerminalPad {
                id,
                position,
                revision,
                label,
                command,
            })
        })
    }

    pub fn remove_terminal_pad(&mut self, project: i64, id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            let removed = tx.execute(
                "DELETE FROM terminal_pad WHERE id=?1 AND project_id=?2",
                params![id, project],
            )?;
            if removed == 0 {
                return Err(Error::invalid("no such pad"));
            }
            Ok(())
        })
    }

    /// Record a press before anything runs, or recognise one that has already been made.
    ///
    /// Everything that decides whether a command may start happens here, in one write: the
    /// request is either known - in which case its receipt is the answer and nothing is
    /// started - or it is new, checked against the pad as the operator read it, and
    /// written down. The caller spawns only against a `Fresh` claim, so there is no moment
    /// at which a process is running and nothing has recorded it.
    pub fn claim_terminal_dispatch(
        &mut self,
        project: i64,
        request: &PressRequest<'_>,
    ) -> Result<DispatchClaim> {
        let id = request_id(request.request_id)?;
        let fingerprint = fingerprint(project, request);
        let confirmed = request.confirmed.trim().to_owned();
        let at = crate::now();
        let token = crate::mint_token();
        // Db::write takes an IMMEDIATE transaction, serializing the lookup and insert.
        self.db_mut().write(|tx| {
            if let Some((seen, recorded)) = tx
                .query_row(
                    &format!(
                        "SELECT {ROW_COLUMNS}, fingerprint FROM terminal_command WHERE request_id=?1"
                    ),
                    params![id],
                    |row| Ok((read_row(row)?, row.get::<_, String>(18)?)),
                )
                .optional()?
            {
                if recorded != fingerprint {
                    return Err(Error::invalid(
                        "that request id was already used for a different press; nothing was run",
                    ));
                }
                return Ok(DispatchClaim::Replay(seen));
            }

            let pad: Option<(i64, String, String)> = tx
                .query_row(
                    "SELECT revision, label, command FROM terminal_pad
                     WHERE id=?1 AND project_id=?2",
                    params![request.pad, project],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let (revision, label, command) =
                pad.ok_or_else(|| Error::invalid("that pad no longer exists; nothing was run"))?;
            if command != confirmed {
                return Err(Error::invalid(format!(
                    "this pad now runs `{command}`, not what you confirmed; nothing was run"
                )));
            }
            if revision != request.revision {
                return Err(Error::invalid(
                    "this pad was edited since you read it; nothing was run",
                ));
            }

            tx.execute(
                "INSERT INTO terminal_command(project_id, request_id, fingerprint, token, worktree,
                        pad_id, revision, label, command, dispatch, claimed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'intent', ?10)",
                params![
                    project,
                    id,
                    fingerprint,
                    token,
                    request.worktree,
                    request.pad,
                    revision,
                    label,
                    command,
                    at
                ],
            )?;
            let row = tx.last_insert_rowid();
            Ok(DispatchClaim::Fresh(ClaimedPress {
                row,
                token: token.clone(),
                label,
                command,
                worktree: request.worktree.to_owned(),
            }))
        })
    }

    /// Name the pty a claimed press is running in.
    ///
    /// Idempotent: a row that already names one keeps it, because the process it names is
    /// the one that is running, and a second write cannot improve on that.
    pub fn record_terminal_start(
        &mut self,
        token: &str,
        session: u64,
    ) -> Result<TerminalCommandRow> {
        let session = i64::try_from(session).unwrap_or(i64::MAX);
        let at = crate::now();
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE terminal_command SET dispatch='started', session_id=?1, started_at=?2
                 WHERE token=?3 AND dispatch='intent'",
                params![session, at, token],
            )?;
            tx.query_row(
                &format!("SELECT {ROW_COLUMNS} FROM terminal_command WHERE token=?1"),
                params![token],
                read_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("that press is no longer recorded"))
        })
    }

    /// Record that a claimed press started nothing.
    ///
    /// Only for a spawn that failed leaving no process behind. A press whose outcome is
    /// merely unobserved stays `intent` and is reported as uncertain - calling that a
    /// refusal would be telling the operator nothing ran when something might have.
    pub fn refuse_terminal_dispatch(
        &mut self,
        token: &str,
        reason: &str,
    ) -> Result<TerminalCommandRow> {
        let reason = reason.to_owned();
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE terminal_command SET dispatch='refused', refusal=?1
                 WHERE token=?2 AND dispatch='intent'",
                params![reason, token],
            )?;
            tx.query_row(
                &format!("SELECT {ROW_COLUMNS} FROM terminal_command WHERE token=?1"),
                params![token],
                read_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("that press is no longer recorded"))
        })
    }

    /// Record an outcome, once. A row that already has one keeps it: the deck hands an
    /// outcome out a single time, and this is the write that proves it.
    ///
    /// A row still saying `intent` takes its outcome too. That row is a press whose start
    /// could not be written down; the exit status was still observed, and recording it is
    /// what turns an uncertain row back into a finished one.
    pub fn settle_terminal_command(&mut self, settled: &SettledCommand) -> Result<bool> {
        let at = crate::now();
        let duration = settled
            .exit_code
            .map(|_| i64::try_from(settled.duration_ms).unwrap_or(i64::MAX));
        let lines = i64::try_from(settled.lines).unwrap_or(i64::MAX);
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE terminal_command
                 SET finished_at=?1, exit_code=?2, duration_ms=?3, lines=?4, lines_exact=?5
                 WHERE token=?6 AND finished_at IS NULL AND dispatch<>'refused'",
                params![
                    at,
                    settled.exit_code,
                    duration,
                    lines,
                    i64::from(settled.lines_exact),
                    settled.token
                ],
            )?;
            Ok(changed == 1)
        })
    }

    /// One press, by the id the window minted for it.
    pub fn terminal_command_by_request(&self, request: &str) -> Result<Option<TerminalCommandRow>> {
        Ok(self
            .db()
            .conn()
            .query_row(
                &format!("SELECT {ROW_COLUMNS} FROM terminal_command WHERE request_id=?1"),
                params![request],
                read_row,
            )
            .optional()?)
    }

    /// What has been run in this checkout, newest first.
    ///
    /// The newest few, *plus* every row that has not settled, however old. A press whose
    /// process may still be alive is the one row that must not scroll out of the window:
    /// it is where an uncertain outcome is read, and the deck is the only place anybody
    /// looks for it.
    pub fn terminal_command_history(
        &self,
        project: i64,
        worktree: &str,
    ) -> Result<Vec<TerminalCommandRow>> {
        let mut query = self.db().conn().prepare(&format!(
            "SELECT {ROW_COLUMNS} FROM terminal_command
             WHERE project_id=?1 AND worktree=?2
               AND ((finished_at IS NULL AND dispatch<>'refused')
                    OR id IN (SELECT id FROM terminal_command
                              WHERE project_id=?1 AND worktree=?2
                              ORDER BY id DESC LIMIT ?3))
             ORDER BY id DESC"
        ))?;
        let rows = query
            .query_map(params![project, worktree, HISTORY], read_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{Dispatch, DispatchClaim, PressRequest, TerminalCommandRow};
    use crate::{NewProject, SettledCommand, Store};

    fn store() -> (Store, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        (store, project.id)
    }

    fn press<'a>(id: &'a str, pad: i64, revision: i64, confirmed: &'a str) -> PressRequest<'a> {
        PressRequest {
            request_id: id,
            worktree: "/w",
            pad,
            revision,
            confirmed,
        }
    }

    fn fresh(claim: DispatchClaim) -> super::ClaimedPress {
        match claim {
            DispatchClaim::Fresh(claimed) => claimed,
            DispatchClaim::Replay(row) => panic!("expected a fresh claim, got {row:?}"),
        }
    }

    fn replay(claim: DispatchClaim) -> TerminalCommandRow {
        match claim {
            DispatchClaim::Replay(row) => row,
            DispatchClaim::Fresh(claimed) => panic!("expected a replay, got {claimed:?}"),
        }
    }

    #[test]
    fn a_project_starts_with_no_pads_and_keeps_the_ones_it_is_given() {
        let (mut store, project) = store();
        assert_eq!(store.terminal_pads(project).unwrap(), vec![]);

        let first = store
            .add_terminal_pad(project, " Test ", " cargo test ")
            .unwrap();
        let second = store
            .add_terminal_pad(project, "Lint", "cargo clippy")
            .unwrap();
        assert_eq!(
            (first.label.as_str(), first.command.as_str(), first.revision),
            ("Test", "cargo test", 1)
        );
        assert_eq!(second.position, 1);
        assert_eq!(store.terminal_pads(project).unwrap().len(), 2);

        let edited = store
            .edit_terminal_pad(project, first.id, "Test", "cargo test --lib")
            .unwrap();
        assert_eq!(
            (edited.command.as_str(), edited.revision),
            ("cargo test --lib", 2),
            "an edit is counted, so a press confirmed against the old pad can be seen"
        );
        assert_eq!(store.terminal_pads(project).unwrap()[0], edited);
        store.remove_terminal_pad(project, first.id).unwrap();
        assert_eq!(store.terminal_pads(project).unwrap(), vec![second]);
        assert!(store.remove_terminal_pad(project, first.id).is_err());
    }

    #[test]
    fn a_pad_cannot_hide_a_second_command_in_a_line_break() {
        let (mut store, project) = store();
        assert!(store
            .add_terminal_pad(project, "Two", "cargo test\nrm -rf /")
            .is_err());
        assert!(store.add_terminal_pad(project, "Empty", "   ").is_err());
        assert!(store
            .add_terminal_pad(project, &"x".repeat(41), "cargo test")
            .is_err());
        assert_eq!(store.terminal_pads(project).unwrap(), vec![]);
    }

    #[test]
    fn a_press_is_refused_when_the_pad_changed_since_it_was_read() {
        // The confirmation showed one command; running a different one because somebody
        // edited the pad in another window is exactly the surprise this prevents.
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();

        store
            .edit_terminal_pad(project, pad.id, "Test", "cargo test --release")
            .unwrap();
        let refused = store
            .claim_terminal_dispatch(
                project,
                &press("req-changed-00000", pad.id, 1, "cargo test"),
            )
            .unwrap_err();
        assert!(
            format!("{refused}").contains("nothing was run"),
            "{refused}"
        );

        // Edited back to the same text is still an edit: the pad the operator read is not
        // the pad on screen now, and the revision is what says so.
        store
            .edit_terminal_pad(project, pad.id, "Test", "cargo test")
            .unwrap();
        let stale = store
            .claim_terminal_dispatch(
                project,
                &press("req-reverted-0000", pad.id, 1, "cargo test"),
            )
            .unwrap_err();
        assert!(
            format!("{stale}").contains("edited since you read it"),
            "{stale}"
        );
        assert!(
            store
                .terminal_command_history(project, "/w")
                .unwrap()
                .is_empty(),
            "a refusal that happens before the record leaves no row"
        );

        store.remove_terminal_pad(project, pad.id).unwrap();
        assert!(store
            .claim_terminal_dispatch(
                project,
                &press("req-missing-00000", pad.id, 3, "cargo test")
            )
            .is_err());
    }

    #[test]
    fn a_repeated_press_is_answered_with_the_receipt_the_first_one_left() {
        // A lost response, a reload or a double click arrives as the same request id. It
        // must come back with the first press's row rather than starting a second run.
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let request = press("req-repeat-000000", pad.id, 1, "cargo test");

        let claimed = fresh(store.claim_terminal_dispatch(project, &request).unwrap());
        let started = store.record_terminal_start(&claimed.token, 7).unwrap();
        assert_eq!(started.dispatch, Dispatch::Started);
        assert_eq!(started.session_id, Some(7));

        let again = replay(store.claim_terminal_dispatch(project, &request).unwrap());
        assert_eq!(again, started);
        assert_eq!(
            store.terminal_command_history(project, "/w").unwrap().len(),
            1,
            "one press, one row, however many times it was asked for"
        );
    }

    #[test]
    fn a_request_id_reused_for_different_bytes_is_refused_rather_than_answered() {
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let other = store
            .add_terminal_pad(project, "Lint", "cargo clippy")
            .unwrap();
        let claimed = fresh(
            store
                .claim_terminal_dispatch(
                    project,
                    &press("req-collide-00000", pad.id, 1, "cargo test"),
                )
                .unwrap(),
        );
        store.record_terminal_start(&claimed.token, 1).unwrap();

        for differing in [
            press("req-collide-00000", other.id, 1, "cargo clippy"),
            PressRequest {
                worktree: "/elsewhere",
                ..press("req-collide-00000", pad.id, 1, "cargo test")
            },
        ] {
            let refused = store
                .claim_terminal_dispatch(project, &differing)
                .unwrap_err();
            assert!(
                format!("{refused}").contains("different press"),
                "a request id is not a free pass to run something else: {refused}"
            );
        }
        assert_eq!(
            store.terminal_command_history(project, "/w").unwrap().len(),
            1
        );
        assert!(store
            .terminal_command_history(project, "/elsewhere")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_press_that_cannot_be_recorded_is_never_started() {
        // The record comes first precisely so this is the order a failure happens in: no
        // row, no claim, and the caller has nothing to spawn against.
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let missing = store.claim_terminal_dispatch(
            99_999,
            &press("req-no-project-000", pad.id, 1, "cargo test"),
        );
        assert!(missing.is_err(), "{missing:?}");
        assert!(store
            .terminal_command_by_request("req-no-project-000")
            .unwrap()
            .is_none());

        assert!(
            store
                .claim_terminal_dispatch(project, &press("short", pad.id, 1, "cargo test"))
                .is_err(),
            "a press with no usable request id is one no receipt could be found for"
        );
        assert!(store
            .terminal_command_history(project, "/w")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_press_whose_start_was_never_written_down_stays_uncertain_and_still_takes_its_outcome() {
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let request = press("req-uncertain-000", pad.id, 1, "cargo test");
        let claimed = fresh(store.claim_terminal_dispatch(project, &request).unwrap());

        let row = replay(store.claim_terminal_dispatch(project, &request).unwrap());
        assert_eq!(row.dispatch, Dispatch::Intent);
        assert_eq!(row.session_id, None);
        assert_eq!(
            row.token, claimed.token,
            "the token is minted with the record"
        );

        // The process ran and ended; only the write that was to name its pty was lost.
        // The outcome is observed, so it is recorded, on the run its token names.
        assert!(store
            .settle_terminal_command(&SettledCommand {
                token: claimed.token.clone(),
                exit_code: Some(2),
                duration_ms: 40,
                lines: 3,
                lines_exact: true,
            })
            .unwrap());
        let settled = &store.terminal_command_history(project, "/w").unwrap()[0];
        assert_eq!(settled.exit_code, Some(2));
        assert_eq!(settled.dispatch, Dispatch::Intent);
    }

    #[test]
    fn a_spawn_that_started_nothing_is_recorded_as_refused_and_never_settles() {
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let request = press("req-refused-00000", pad.id, 1, "cargo test");
        let claimed = fresh(store.claim_terminal_dispatch(project, &request).unwrap());

        let refused = store
            .refuse_terminal_dispatch(&claimed.token, "could not open a terminal")
            .unwrap();
        assert_eq!(refused.dispatch, Dispatch::Refused);
        assert_eq!(
            refused.refusal.as_deref(),
            Some("could not open a terminal")
        );
        assert!(
            !store
                .settle_terminal_command(&SettledCommand {
                    token: claimed.token.clone(),
                    exit_code: Some(0),
                    duration_ms: 1,
                    lines: 0,
                    lines_exact: true,
                })
                .unwrap(),
            "nothing ran, so there is no outcome for it to take"
        );

        let again = replay(store.claim_terminal_dispatch(project, &request).unwrap());
        assert_eq!(again.dispatch, Dispatch::Refused);
        assert!(again.finished_at.is_none());
    }

    #[test]
    fn an_outcome_lands_once_on_the_run_its_token_names() {
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let first = fresh(
            store
                .claim_terminal_dispatch(
                    project,
                    &press("req-first-0000000", pad.id, 1, "cargo test"),
                )
                .unwrap(),
        );
        store.record_terminal_start(&first.token, 3).unwrap();
        let second = fresh(
            store
                .claim_terminal_dispatch(
                    project,
                    &press("req-second-000000", pad.id, 1, "cargo test"),
                )
                .unwrap(),
        );
        let second = store.record_terminal_start(&second.token, 4).unwrap();
        assert!(
            second.finished_at.is_none(),
            "a press is recorded while it runs"
        );

        let settled = SettledCommand {
            token: first.token.clone(),
            exit_code: Some(3),
            duration_ms: 1_200,
            lines: 42,
            lines_exact: false,
        };
        assert!(store.settle_terminal_command(&settled).unwrap());
        assert!(
            !store.settle_terminal_command(&settled).unwrap(),
            "a settled run keeps the outcome it was given"
        );

        let history = store.terminal_command_history(project, "/w").unwrap();
        assert_eq!(history[0].id, second.id, "newest first");
        assert_eq!(history[1].exit_code, Some(3));
        assert_eq!(history[1].duration_ms, Some(1_200));
        assert_eq!(history[1].lines, Some(42));
        assert!(!history[1].lines_exact);
        assert!(history[0].exit_code.is_none());
        assert!(store
            .terminal_command_history(project, "/elsewhere")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_unknown_outcome_is_recorded_as_unknown_rather_than_as_a_pass() {
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let claimed = fresh(
            store
                .claim_terminal_dispatch(
                    project,
                    &press("req-unknown-00000", pad.id, 1, "cargo test"),
                )
                .unwrap(),
        );
        store.record_terminal_start(&claimed.token, 9).unwrap();
        assert!(store
            .settle_terminal_command(&SettledCommand {
                token: claimed.token,
                exit_code: None,
                duration_ms: 0,
                lines: 0,
                lines_exact: true,
            })
            .unwrap());
        let row = &store.terminal_command_history(project, "/w").unwrap()[0];
        assert!(row.finished_at.is_some());
        assert_eq!(row.exit_code, None);
    }

    #[test]
    fn history_is_bounded_over_what_is_settled_and_keeps_what_may_still_be_running() {
        let (mut store, project) = store();
        let pad = store
            .add_terminal_pad(project, "Test", "cargo test")
            .unwrap();
        let unsettled = fresh(
            store
                .claim_terminal_dispatch(
                    project,
                    &press("req-unsettled-000", pad.id, 1, "cargo test"),
                )
                .unwrap(),
        );
        store.record_terminal_start(&unsettled.token, 1).unwrap();
        for index in 0..110 {
            let id = format!("req-bounded-{index:05}");
            let claimed = fresh(
                store
                    .claim_terminal_dispatch(project, &press(&id, pad.id, 1, "cargo test"))
                    .unwrap(),
            );
            store.record_terminal_start(&claimed.token, 2).unwrap();
            store
                .settle_terminal_command(&SettledCommand {
                    token: claimed.token,
                    exit_code: Some(0),
                    duration_ms: 1,
                    lines: 1,
                    lines_exact: true,
                })
                .unwrap();
        }
        let history = store.terminal_command_history(project, "/w").unwrap();
        assert_eq!(history[0].request_id, "req-bounded-00109");
        assert_eq!(
            history.len(),
            101,
            "a hundred settled, and the one that has not: a row that may still be running \
             is never what makes room, and never scrolls out of the window either"
        );
        assert_eq!(history.last().unwrap().token, unsettled.token);
        assert!(
            matches!(
                store
                    .claim_terminal_dispatch(
                        project,
                        &press("req-bounded-00000", pad.id, 1, "cargo test")
                    )
                    .unwrap(),
                DispatchClaim::Replay(_)
            ),
            "paging history must not forget execution identity"
        );
    }
}
