//! Reminders, the idea inbox, and scheduled run triggers.
//!
//! One table, because they are one question - what should happen later - and three
//! tables would mean three places to look when the answer is wrong.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::model::{NewReminder, Recur, Reminder, ReminderKind, ReminderStatus};
use crate::store::{non_empty, Store};
use crate::util::now;

impl Store {
    pub fn add_reminder(&mut self, new: NewReminder) -> Result<Reminder> {
        let title = new.title.trim().to_string();
        if title.is_empty() {
            return Err(Error::invalid("a reminder needs a title"));
        }
        let kind = new.kind.unwrap_or(ReminderKind::Reminder);
        // A scheduled run needs somewhere to run, but not something to say. An empty
        // prompt means "build whatever the plan already has ready" everywhere else in
        // ai-team since Q14, and that is precisely what a nightly wants - the alternative
        // is writing out a prompt that describes the plan you already wrote.
        if kind == ReminderKind::ScheduledRun && new.project_id.is_none() {
            return Err(Error::invalid(
                "a scheduled run needs a project - there would be nowhere to run",
            ));
        }
        // A recurrence with no first occurrence never fires.
        if new.recur.is_some() && new.due_at.is_none() {
            return Err(Error::invalid(
                "a recurring reminder needs a due time to recur from",
            ));
        }
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO reminder
                   (project_id, team_id, kind, title, body, prompt, due_at, recur,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    new.project_id,
                    new.team_id,
                    kind,
                    title,
                    new.body,
                    new.prompt,
                    new.due_at,
                    new.recur,
                    at
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.reminder(id)
    }

    pub fn reminder(&self, id: i64) -> Result<Reminder> {
        self.db()
            .conn()
            .query_row(
                &format!("{REMINDER_SELECT} WHERE id = ?1"),
                params![id],
                reminder_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid(format!("no reminder {id}")))
    }

    /// Everything pending and due at or before `at`. The scheduler's only query.
    ///
    /// `<=` rather than `<`, and ordered oldest first, so a machine that was asleep for
    /// a day fires what it missed in the order it was meant to happen.
    pub fn due_reminders(&self, at: &str) -> Result<Vec<Reminder>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{REMINDER_SELECT} WHERE status = 'pending' AND due_at IS NOT NULL AND due_at <= ?1
              ORDER BY due_at, id"
        ))?;
        let rows = stmt
            .query_map(params![at], reminder_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn reminders(
        &self,
        project_id: Option<i64>,
        kind: Option<ReminderKind>,
    ) -> Result<Vec<Reminder>> {
        let mut clauses = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(id) = project_id {
            clauses.push("project_id = ?");
            args.push(Box::new(id));
        }
        if let Some(k) = kind {
            clauses.push("kind = ?");
            args.push(Box::new(k));
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        let mut stmt = self.db().conn().prepare(&format!(
            "{REMINDER_SELECT}{where_sql} ORDER BY COALESCE(due_at, '9999'), id"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), reminder_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Mark a reminder as having fired.
    ///
    /// A recurring one is rolled forward to its next occurrence and stays pending; a
    /// one-off is done. Rolling forward from the *due* time rather than from now is what
    /// stops a daily 09:00 reminder drifting later every day it fires late.
    /// Take a due reminder, if nobody else has.
    ///
    /// Two schedulers is the normal case, not a mistake: the window runs one so having
    /// it open is enough, and the daemon runs one so closing it does not stop the clock.
    /// What makes that safe is here rather than in a rule about who may run - the update
    /// is guarded on the `rev` the caller read, so exactly one claim can win and the
    /// loser is told it lost. Without it a scheduled run fires twice, which means two
    /// unattended agents in one repository.
    ///
    /// Returns the reminder as it now stands, or `None` if somebody else took it.
    pub fn claim_reminder(&mut self, reminder: &Reminder) -> Result<Option<Reminder>> {
        let at = now();
        let next = reminder
            .recur
            .and_then(|r| {
                reminder
                    .due_at
                    .as_deref()
                    .map(|due| next_occurrence(due, r))
            })
            .transpose()?;

        let id = reminder.id;
        let rev = reminder.rev;
        let won = self.db_mut().write(|tx| {
            let changed = match &next {
                Some(next_due) => tx.execute(
                    "UPDATE reminder SET due_at = ?2, last_fired_at = ?3, status = 'pending',
                                         rev = rev + 1, updated_at = ?3
                      WHERE id = ?1 AND rev = ?4 AND status = 'pending'",
                    params![id, next_due, at, rev],
                )?,
                None => tx.execute(
                    "UPDATE reminder SET status = 'fired', last_fired_at = ?2, rev = rev + 1,
                                         updated_at = ?2
                      WHERE id = ?1 AND rev = ?3 AND status = 'pending'",
                    params![id, at, rev],
                )?,
            };
            Ok(changed == 1)
        })?;

        if won {
            Ok(Some(self.reminder(id)?))
        } else {
            Ok(None)
        }
    }

    pub fn fire_reminder(&mut self, id: i64) -> Result<Reminder> {
        let reminder = self.reminder(id)?;
        let at = now();
        let next = reminder
            .recur
            .and_then(|r| {
                reminder
                    .due_at
                    .as_deref()
                    .map(|due| next_occurrence(due, r))
            })
            .transpose()?;

        self.db_mut().write(|tx| {
            match next {
                Some(next_due) => tx.execute(
                    "UPDATE reminder SET due_at = ?2, last_fired_at = ?3, status = 'pending',
                                         rev = rev + 1, updated_at = ?3
                      WHERE id = ?1",
                    params![id, next_due, at],
                )?,
                None => tx.execute(
                    "UPDATE reminder SET status = 'fired', last_fired_at = ?2, rev = rev + 1,
                                         updated_at = ?2
                      WHERE id = ?1",
                    params![id, at],
                )?,
            };
            Ok(())
        })?;

        self.reminder(id)
    }

    pub fn set_reminder_status(&mut self, id: i64, status: ReminderStatus) -> Result<Reminder> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE reminder SET status = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![id, status, at],
            )?;
            if changed == 0 {
                return Err(Error::invalid(format!("no reminder {id}")));
            }
            Ok(())
        })?;
        self.reminder(id)
    }
}

/// The next occurrence after `due`, in the same ISO-8601 shape.
///
/// Time-of-day is preserved deliberately: a 09:00 reminder stays a 09:00 reminder, which
/// is the whole point of "daily".
fn next_occurrence(due: &str, recur: Recur) -> Result<String> {
    use time::format_description::well_known::Rfc3339;
    use time::{Duration, OffsetDateTime, Weekday};

    let start = OffsetDateTime::parse(due, &Rfc3339)
        .map_err(|e| Error::invalid(format!("{due:?} is not a usable due time: {e}")))?;

    let next = match recur {
        Recur::Daily => start + Duration::days(1),
        Recur::Weekdays => {
            let mut candidate = start + Duration::days(1);
            while matches!(candidate.weekday(), Weekday::Saturday | Weekday::Sunday) {
                candidate += Duration::days(1);
            }
            candidate
        }
        Recur::Weekly => start + Duration::weeks(1),
        // Calendar months vary, so this steps by 4 weeks rather than pretending
        // otherwise. Named `monthly` because that is what a person means by it; if the
        // drift ever matters, this is the one function to fix.
        Recur::Monthly => start + Duration::weeks(4),
    };

    next.format(&Rfc3339)
        .map_err(|e| Error::invalid(format!("could not format the next occurrence: {e}")))
}

const REMINDER_SELECT: &str = "SELECT id, project_id, team_id, kind, title, body, prompt, due_at, \
     recur, status, last_fired_at, rev, created_at, updated_at FROM reminder";

fn reminder_from_row(r: &Row<'_>) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: r.get(0)?,
        project_id: r.get(1)?,
        team_id: r.get(2)?,
        kind: r.get(3)?,
        title: r.get(4)?,
        body: r.get(5)?,
        prompt: non_empty(r.get(6)?),
        due_at: non_empty(r.get(7)?),
        recur: r.get(8)?,
        status: r.get(9)?,
        last_fired_at: non_empty(r.get(10)?),
        rev: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::memory().unwrap()
    }

    #[test]
    fn a_scheduled_run_without_a_project_is_refused() {
        // Nowhere to run is a mistake worth catching while somebody is looking, not at
        // two in the morning when it fires.
        let mut s = store();
        let bad = s.add_reminder(NewReminder {
            kind: Some(ReminderKind::ScheduledRun),
            title: "nightly tidy".into(),
            prompt: Some("tidy the flaky tests".into()),
            due_at: Some("2026-09-18T09:00:00Z".into()),
            ..Default::default()
        });
        assert!(bad.is_err(), "it would fire and have nowhere to go");
    }

    #[test]
    fn a_scheduled_run_needs_no_prompt_because_empty_means_build_what_is_ready() {
        // Which is what `ait run` with no prompt has meant since Q14, and is precisely
        // what a nightly wants - the alternative is writing a prompt that describes the
        // plan you already wrote.
        let mut s = store();
        let project = s
            .create_project(crate::model::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let added = s.add_reminder(NewReminder {
            project_id: Some(project.id),
            kind: Some(ReminderKind::ScheduledRun),
            title: "nightly build".into(),
            due_at: Some("2026-09-18T09:00:00Z".into()),
            ..Default::default()
        });
        assert!(added.is_ok(), "{:?}", added.err());
    }

    #[test]
    fn an_idea_needs_no_due_time_and_is_never_due() {
        let mut s = store();
        s.add_reminder(NewReminder {
            kind: Some(ReminderKind::Idea),
            title: "what if runs could fork".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(s.due_reminders("2099-01-01T00:00:00Z").unwrap().is_empty());
        assert_eq!(
            s.reminders(None, Some(ReminderKind::Idea)).unwrap().len(),
            1
        );
    }

    #[test]
    fn a_recurrence_without_a_first_occurrence_is_refused() {
        let mut s = store();
        assert!(s
            .add_reminder(NewReminder {
                title: "stand-up".into(),
                recur: Some(Recur::Weekdays),
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn everything_missed_fires_oldest_first() {
        let mut s = store();
        for due in [
            "2026-09-18T09:00:00Z",
            "2026-09-16T09:00:00Z",
            "2026-09-17T09:00:00Z",
        ] {
            s.add_reminder(NewReminder {
                title: due.into(),
                due_at: Some(due.into()),
                ..Default::default()
            })
            .unwrap();
        }
        // A machine asleep for two days must replay in the order things were meant to
        // happen, not in insertion order.
        let due = s.due_reminders("2026-09-17T12:00:00Z").unwrap();
        assert_eq!(
            due.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["2026-09-16T09:00:00Z", "2026-09-17T09:00:00Z"]
        );
    }

    #[test]
    fn a_one_off_is_done_once_it_fires() {
        let mut s = store();
        let r = s
            .add_reminder(NewReminder {
                title: "ship the release".into(),
                due_at: Some("2026-09-18T09:00:00Z".into()),
                ..Default::default()
            })
            .unwrap();

        let fired = s.fire_reminder(r.id).unwrap();
        assert_eq!(fired.status, ReminderStatus::Fired);
        assert!(fired.last_fired_at.is_some());
        assert!(s.due_reminders("2099-01-01T00:00:00Z").unwrap().is_empty());
    }

    #[test]
    fn a_daily_reminder_rolls_forward_and_keeps_its_time_of_day() {
        let mut s = store();
        let r = s
            .add_reminder(NewReminder {
                title: "morning sweep".into(),
                due_at: Some("2026-09-18T09:00:00Z".into()),
                recur: Some(Recur::Daily),
                ..Default::default()
            })
            .unwrap();

        let fired = s.fire_reminder(r.id).unwrap();
        assert_eq!(fired.status, ReminderStatus::Pending, "it recurs");
        // 09:00 stays 09:00. Rolling forward from `now` instead would make it drift
        // later every day it fired late.
        assert_eq!(fired.due_at.as_deref(), Some("2026-09-19T09:00:00Z"));
    }

    #[test]
    fn weekdays_skips_the_weekend() {
        // 2026-09-18 is a Friday.
        assert_eq!(
            next_occurrence("2026-09-18T09:00:00Z", Recur::Weekdays).unwrap(),
            "2026-09-21T09:00:00Z"
        );
        assert_eq!(
            next_occurrence("2026-09-17T09:00:00Z", Recur::Weekdays).unwrap(),
            "2026-09-18T09:00:00Z"
        );
        assert_eq!(
            next_occurrence("2026-09-18T09:00:00Z", Recur::Weekly).unwrap(),
            "2026-09-25T09:00:00Z"
        );
    }

    #[test]
    fn cancelling_takes_it_out_of_the_due_list() {
        let mut s = store();
        let r = s
            .add_reminder(NewReminder {
                title: "nope".into(),
                due_at: Some("2026-09-18T09:00:00Z".into()),
                recur: Some(Recur::Daily),
                ..Default::default()
            })
            .unwrap();
        s.set_reminder_status(r.id, ReminderStatus::Cancelled)
            .unwrap();
        assert!(s.due_reminders("2099-01-01T00:00:00Z").unwrap().is_empty());
    }
}
