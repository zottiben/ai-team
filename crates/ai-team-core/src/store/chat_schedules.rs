//! Chat-linked schedules: one exact chat, checkout, prompt and model, claimed once.
//!
//! The reminder table stays the clock. What this adds is the answer to "where does it
//! go", recorded when the person schedules it rather than resolved at two in the
//! morning from whatever the project most recently ran.

use rusqlite::{params, OptionalExtension, Row};

use super::Store;
use crate::chat_schedule::{
    ChatSchedule, ChatScheduleOccurrence, ClaimedOccurrence, NewChatSchedule,
};
use crate::error::{Error, Result};
use crate::model::{Reminder, ScheduleOutcome};
use crate::util::now;

const SCHEDULE_SELECT: &str = "SELECT id, reminder_id, chat_id, project_id, workspace_path,
    prompt, provider, model, reasoning, mode, created_at FROM chat_schedule";

const OCCURRENCE_SELECT: &str = "SELECT id, schedule_id, occurrence_at, request_id, claimed_at,
    skipped, outcome, detail, run_id, node_id, settled_at FROM chat_schedule_occurrence";

/// The turn's idempotency key, decided when the occurrence is claimed rather than when it
/// is dispatched. A scheduler that died in between is resolved by looking this up.
fn request_id(schedule: i64, occurrence_at: &str) -> String {
    format!("schedule/{schedule}/{occurrence_at}")
}

impl Store {
    /// Record a schedule and its reminder in one write.
    ///
    /// One transaction because the halves are dangerous apart: a reminder of kind
    /// `scheduled_run` with no schedule beside it is a legacy unattended workflow run in
    /// somebody's repository, which is precisely what this is not.
    pub fn add_chat_schedule(&mut self, new: NewChatSchedule) -> Result<ChatSchedule> {
        let chat = self.chat(new.chat_id)?;
        if chat.archived {
            return Err(Error::invalid(
                "this chat is archived; restore it before scheduling work in it",
            ));
        }
        let seat = if chat.mode == crate::ChatMode::Team {
            self.chat_team_coordinator(self.project(chat.project_id)?.team_id)?
        } else {
            chat.agent()
        };
        let prompt = new.prompt.trim().to_string();
        if prompt.is_empty() || prompt.len() > 100_000 {
            return Err(Error::invalid(
                "a scheduled chat prompt must contain 1–100000 bytes - there is nobody there \
                 to say what it meant",
            ));
        }
        let due_at = new.due_at.trim().to_string();
        if due_at.is_empty() {
            return Err(Error::invalid("a schedule needs a first occurrence"));
        }
        super::reminders::check_due_time(&due_at)?;
        let title = match new.title.trim() {
            "" => prompt.chars().take(80).collect::<String>(),
            given => given.chars().take(80).collect::<String>(),
        };
        let at = now();
        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO reminder
                   (project_id, kind, title, body, prompt, due_at, recur, created_at, updated_at)
                 VALUES (?1, 'scheduled_run', ?2, '', ?3, ?4, ?5, ?6, ?6)",
                params![chat.project_id, title, prompt, due_at, new.recur, at],
            )?;
            let reminder = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO chat_schedule (reminder_id, chat_id, project_id, workspace_path,
                    prompt, provider, model, reasoning, mode, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    reminder,
                    chat.id,
                    chat.project_id,
                    chat.workspace_path,
                    prompt,
                    seat.provider,
                    seat.model,
                    seat.reasoning,
                    chat.mode,
                    at
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;
        self.chat_schedule(id)
    }

    pub fn chat_schedule(&self, id: i64) -> Result<ChatSchedule> {
        self.db()
            .conn()
            .query_row(
                &format!("{SCHEDULE_SELECT} WHERE id = ?1"),
                params![id],
                schedule_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid(format!("no chat schedule {id}")))
    }

    pub(crate) fn scheduled_reasoning(&self, node: i64) -> Result<Option<crate::Reasoning>> {
        Ok(self.db().conn().query_row(
            "SELECT s.reasoning FROM chat_schedule s JOIN chat_schedule_occurrence o ON o.schedule_id=s.id
             JOIN chat_turn t ON t.chat_id=s.chat_id AND t.request_id=o.request_id WHERE t.node_id=?1",
            [node], |row| row.get(0),
        ).optional()?)
    }

    pub fn chat_schedule_for_reminder(&self, reminder_id: i64) -> Result<Option<ChatSchedule>> {
        Ok(self
            .db()
            .conn()
            .query_row(
                &format!("{SCHEDULE_SELECT} WHERE reminder_id = ?1"),
                params![reminder_id],
                schedule_from_row,
            )
            .optional()?)
    }

    /// Every schedule, newest first. Scoped by project when the caller has one.
    pub fn chat_schedules(&self, project_id: Option<i64>) -> Result<Vec<ChatSchedule>> {
        let mut statement = self.db().conn().prepare(&format!(
            "{SCHEDULE_SELECT} WHERE (?1 IS NULL OR project_id = ?1) ORDER BY id DESC"
        ))?;
        let rows = statement
            .query_map(params![project_id], schedule_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// What this schedule has actually done, newest first.
    pub fn chat_schedule_occurrences(
        &self,
        schedule_id: i64,
        limit: i64,
    ) -> Result<Vec<ChatScheduleOccurrence>> {
        let mut statement = self.db().conn().prepare(&format!(
            "{OCCURRENCE_SELECT} WHERE schedule_id = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = statement
            .query_map(
                params![schedule_id, limit.clamp(1, 200)],
                occurrence_from_row,
            )?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Take one occurrence of a chat schedule, if nobody else has.
    ///
    /// The guarded reminder update decides the winner, exactly as it does for any other
    /// reminder; the occurrence row's unique key is the second lock, so even a reminder
    /// advanced by some other path cannot produce two runs of one occurrence. The claim
    /// is committed before anything is dispatched, which is what makes a scheduler that
    /// dies mid-tick resolvable rather than repeatable.
    pub fn claim_chat_schedule_occurrence(
        &mut self,
        reminder: &Reminder,
    ) -> Result<Option<ClaimedOccurrence>> {
        let Some(schedule) = self.chat_schedule_for_reminder(reminder.id)? else {
            return Ok(None);
        };
        let occurrence_at = reminder
            .due_at
            .clone()
            .ok_or_else(|| Error::invalid("a chat schedule occurrence needs a due time"))?;
        let at = now();
        let advanced = reminder
            .recur
            .map(|recur| super::reminders::advance(&occurrence_at, recur, &at))
            .transpose()?;
        let identity = crate::chat::process_identity(i64::from(std::process::id()));
        let request = request_id(schedule.id, &occurrence_at);
        let pid = i64::from(std::process::id());

        let claimed = self.db_mut().write(|tx| {
            let won = match &advanced {
                Some((next_due, _)) => tx.execute(
                    "UPDATE reminder SET due_at = ?2, last_fired_at = ?3, status = 'pending',
                                         rev = rev + 1, updated_at = ?3
                      WHERE id = ?1 AND rev = ?4 AND status = 'pending'",
                    params![reminder.id, next_due, at, reminder.rev],
                )?,
                None => tx.execute(
                    "UPDATE reminder SET status = 'fired', last_fired_at = ?2, rev = rev + 1,
                                         updated_at = ?2
                      WHERE id = ?1 AND rev = ?3 AND status = 'pending'",
                    params![reminder.id, at, reminder.rev],
                )?,
            };
            if won != 1 {
                return Ok(None);
            }
            tx.execute(
                "INSERT INTO chat_schedule_occurrence
                   (schedule_id, occurrence_at, request_id, claimed_at, claimed_pid,
                    claimed_identity, skipped)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    schedule.id,
                    occurrence_at,
                    request,
                    at,
                    pid,
                    identity,
                    advanced.as_ref().map_or(0, |(_, skipped)| *skipped)
                ],
            )?;
            Ok(Some(tx.last_insert_rowid()))
        })?;

        let Some(id) = claimed else {
            return Ok(None);
        };
        Ok(Some(ClaimedOccurrence {
            occurrence: self.chat_schedule_occurrence(id)?,
            schedule,
            reminder: self.reminder(reminder.id)?,
        }))
    }

    pub fn chat_schedule_occurrence(&self, id: i64) -> Result<ChatScheduleOccurrence> {
        self.db()
            .conn()
            .query_row(
                &format!("{OCCURRENCE_SELECT} WHERE id = ?1"),
                params![id],
                occurrence_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid(format!("no schedule occurrence {id}")))
    }

    /// Record what one claimed occurrence became. Settling is once, by trigger.
    ///
    /// Anything other than a started turn also becomes attention on the chat it was
    /// scheduled in: there is no run to carry it, so without this a nightly that refused
    /// itself for a fortnight is visible only to whoever opens the Schedule page.
    pub fn settle_chat_schedule_occurrence(
        &mut self,
        id: i64,
        outcome: ScheduleOutcome,
        detail: &str,
        run_id: Option<i64>,
        node_id: Option<i64>,
    ) -> Result<ChatScheduleOccurrence> {
        if outcome == ScheduleOutcome::Claimed {
            return Err(Error::invalid("an occurrence cannot settle as unclaimed"));
        }
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE chat_schedule_occurrence
                    SET outcome = ?2, detail = ?3, run_id = ?4, node_id = ?5, settled_at = ?6
                  WHERE id = ?1 AND outcome = 'claimed'",
                params![id, outcome, detail, run_id, node_id, at],
            )?;
            if changed != 1 {
                return Err(Error::invalid(
                    "that occurrence has already been settled; its result stands",
                ));
            }
            if outcome != ScheduleOutcome::Started {
                tx.execute(
                    "INSERT OR IGNORE INTO notification
                       (dedupe_key, project_id, workspace_path, chat_id, kind, title, body,
                        created_at)
                     SELECT 'chat-schedule-occurrence:' || ?1, s.project_id, s.workspace_path,
                            s.chat_id, 'follow_up', c.title || ' · scheduled work did not start',
                            ?2, ?3
                       FROM chat_schedule_occurrence o
                       JOIN chat_schedule s ON s.id = o.schedule_id
                       JOIN chat c ON c.id = s.chat_id
                      WHERE o.id = ?1",
                    params![id, detail, at],
                )?;
            }
            Ok(())
        })?;
        self.chat_schedule_occurrence(id)
    }

    /// Resolve claims whose scheduler is gone, without running anything again.
    ///
    /// The claim is committed before dispatch and carries the turn's request id, so this
    /// is a lookup rather than a guess: a chat turn under that id means the occurrence
    /// did start, and no turn means it never did. Neither is retried - the occurrence is
    /// spent either way, because a clock that re-runs what it is unsure about is a clock
    /// that runs an unattended agent twice.
    pub fn sweep_chat_schedule_claims(&mut self) -> Result<Vec<ChatScheduleOccurrence>> {
        let mut statement = self.db().conn().prepare(
            "SELECT o.id, o.claimed_pid, o.claimed_identity, t.run_id, t.node_id
               FROM chat_schedule_occurrence o
               JOIN chat_schedule s ON s.id = o.schedule_id
               LEFT JOIN chat_turn t ON t.request_id = o.request_id AND t.chat_id = s.chat_id
              WHERE o.outcome = 'claimed'",
        )?;
        let pending = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let mut settled = Vec::new();
        for (id, pid, identity, run_id, node_id) in pending {
            if crate::chat::process_matches(pid, identity.as_deref()) {
                continue;
            }
            let (outcome, detail) = match node_id {
                Some(_) => (
                    ScheduleOutcome::Started,
                    "The scheduler stopped before recording this occurrence. Its turn did \
                     start, and is in the chat."
                        .to_string(),
                ),
                None => (
                    ScheduleOutcome::Interrupted,
                    "The scheduler stopped between claiming this occurrence and starting it. \
                     Nothing ran, and it was not started again."
                        .to_string(),
                ),
            };
            settled
                .push(self.settle_chat_schedule_occurrence(id, outcome, &detail, run_id, node_id)?);
        }
        Ok(settled)
    }
}

pub(super) fn check_admission(
    conn: &rusqlite::Connection,
    chat: &crate::Chat,
    message: &str,
    request: &str,
    agent: &crate::Agent,
) -> Result<()> {
    if !request.starts_with("schedule/") {
        return Ok(());
    }
    let owns: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_schedule s JOIN chat_schedule_occurrence o ON o.schedule_id=s.id
         WHERE o.request_id=?1 AND o.outcome='claimed' AND o.claimed_pid=?2
         AND s.chat_id=?3 AND s.project_id=?4 AND s.workspace_path=?5 AND s.mode=?6 AND s.prompt=?7
         AND s.provider=?8 AND s.model=?9 AND s.reasoning=?10)",
        params![request, i64::from(std::process::id()), chat.id, chat.project_id, chat.workspace_path, chat.mode, message, agent.provider, agent.model, agent.reasoning], |row| row.get(0),
    )?;
    if !owns {
        return Err(Error::invalid("the claimed schedule's chat, mode, checkout, prompt or model changed; nothing was started"));
    }
    if chat.mode == crate::ChatMode::Team {
        let current: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent a JOIN project p ON p.team_id=a.team_id WHERE p.id=?1 AND a.id=?2 AND a.enabled=1 AND a.read_only=1 AND a.provider=?3 AND a.model=?4 AND a.reasoning=?5)",
            params![chat.project_id, agent.id, agent.provider, agent.model, agent.reasoning], |row| row.get(0),
        )?;
        if !current {
            return Err(Error::invalid(
                "the scheduled coordinator changed before dispatch; nothing was started",
            ));
        }
    }
    Ok(())
}

fn schedule_from_row(row: &Row<'_>) -> rusqlite::Result<ChatSchedule> {
    Ok(ChatSchedule {
        id: row.get(0)?,
        reminder_id: row.get(1)?,
        chat_id: row.get(2)?,
        project_id: row.get(3)?,
        workspace_path: row.get(4)?,
        prompt: row.get(5)?,
        provider: row.get(6)?,
        model: row.get(7)?,
        reasoning: row.get(8)?,
        mode: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn occurrence_from_row(row: &Row<'_>) -> rusqlite::Result<ChatScheduleOccurrence> {
    Ok(ChatScheduleOccurrence {
        id: row.get(0)?,
        schedule_id: row.get(1)?,
        occurrence_at: row.get(2)?,
        request_id: row.get(3)?,
        claimed_at: row.get(4)?,
        skipped: row.get(5)?,
        outcome: row.get(6)?,
        detail: super::non_empty(row.get(7)?),
        run_id: row.get(8)?,
        node_id: row.get(9)?,
        settled_at: super::non_empty(row.get(10)?),
    })
}
