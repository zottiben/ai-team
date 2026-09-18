//! `ait remind` - one-off and recurring reminders, the idea inbox, and scheduled runs.
//!
//! Three kinds share one table because they share one clock and one due date. What
//! differs is what firing means: a reminder and an idea announce themselves, a scheduled
//! run starts work.

use anyhow::{bail, Context, Result};

use ai_team_core::{NewReminder, Recur, ReminderKind, ReminderStatus, Store};

use crate::cli::{RemindArgs, RemindCommand};

pub(crate) fn run(command: RemindCommand) -> Result<()> {
    match command {
        RemindCommand::Ls => ls(),
        RemindCommand::Add(args) => add(args, ReminderKind::Reminder),
        RemindCommand::Idea(args) => add(args, ReminderKind::Idea),
        RemindCommand::Run(args) => add(args, ReminderKind::ScheduledRun),
        RemindCommand::Rm { id } => {
            let mut store = Store::open_default()?;
            store.set_reminder_status(id, ReminderStatus::Cancelled)?;
            println!("cancelled #{id}");
            Ok(())
        }
    }
}

fn ls() -> Result<()> {
    let store = Store::open_default()?;
    let rows = store.reminders(None, None)?;
    if rows.is_empty() {
        println!("Nothing scheduled. `ait remind add \"…\" --in 2h` or `ait remind run …`.");
        return Ok(());
    }

    println!(
        "{:>4}  {:<14} {:<10} {:<22} TITLE",
        "ID", "KIND", "STATUS", "DUE"
    );
    for reminder in rows {
        println!(
            "{:>4}  {:<14} {:<10} {:<22} {}",
            reminder.id,
            reminder.kind.as_str(),
            reminder.status.as_str(),
            reminder.due_at.as_deref().unwrap_or("-"),
            reminder.title
        );
    }
    Ok(())
}

fn add(args: RemindArgs, kind: ReminderKind) -> Result<()> {
    let mut store = Store::open_default()?;

    // A scheduled run needs somewhere to run. Resolving it now means a mistake is caught
    // while somebody is looking, rather than at two in the morning when it fires.
    let project = match (&args.project, kind) {
        (Some(slug), _) => Some(store.find_project(slug)?.id),
        (None, ReminderKind::ScheduledRun) => Some(
            store
                .project_at(&std::env::current_dir()?)?
                .context("a scheduled run needs a project - pass --project, or stand in one")?
                .id,
        ),
        (None, _) => None,
    };

    let due_at = match (&args.at, &args.r#in) {
        (Some(_), Some(_)) => bail!("pass --at or --in, not both"),
        (Some(at), None) => Some(at.clone()),
        (None, Some(delta)) => Some(in_from_now(delta)?),
        // An idea has no due date until somebody gives it one; that is what makes it an
        // inbox rather than another queue with a deadline.
        (None, None) if kind == ReminderKind::Idea => None,
        (None, None) => bail!("when? pass --at <rfc3339> or --in <30m|2h|1d>"),
    };

    let reminder = store.add_reminder(NewReminder {
        project_id: project,
        team_id: None,
        kind: Some(kind),
        title: args.title.clone(),
        body: args.note.clone().unwrap_or_default(),
        prompt: args.prompt.clone(),
        due_at,
        recur: args.every.as_deref().map(parse_recur).transpose()?,
    })?;

    println!(
        "#{} {} {}",
        reminder.id,
        reminder.kind.as_str(),
        reminder.due_at.as_deref().unwrap_or("(no date)")
    );
    if kind == ReminderKind::ScheduledRun {
        // Worth saying: the window does not have to be open, but something does.
        println!("the clock runs in `ait ui` or `ait daemon` - one of them needs to be up");
    }
    Ok(())
}

/// `90m`, `2h`, `1d` from now, as RFC3339.
fn in_from_now(delta: &str) -> Result<String> {
    let (value, unit) = delta.split_at(
        delta
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(delta.len()),
    );
    let value: i64 = value
        .parse()
        .with_context(|| format!("could not read `{delta}` - try 30m, 2h or 1d"))?;
    let seconds = match unit {
        "s" => value,
        "m" | "" => value * 60,
        "h" => value * 3_600,
        "d" => value * 86_400,
        other => bail!("unknown unit `{other}` - try s, m, h or d"),
    };
    Ok(ai_team_core::rfc3339_in(seconds))
}

fn parse_recur(value: &str) -> Result<Recur> {
    match value {
        "daily" => Ok(Recur::Daily),
        "weekdays" => Ok(Recur::Weekdays),
        "weekly" => Ok(Recur::Weekly),
        "monthly" => Ok(Recur::Monthly),
        other => bail!("unknown recurrence `{other}` - try daily, weekdays, weekly or monthly"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_number_is_minutes_because_that_is_what_people_mean() {
        // `--in 5` is five minutes, not five seconds. Getting this backwards would fire
        // a scheduled run almost immediately, which looks like a bug in the scheduler.
        assert!(in_from_now("5").is_ok());
        assert!(in_from_now("90m").is_ok());
        assert!(in_from_now("2h").is_ok());
        assert!(in_from_now("1d").is_ok());
    }

    #[test]
    fn a_delta_that_cannot_be_read_says_so_rather_than_guessing() {
        assert!(in_from_now("soon").is_err());
        assert!(in_from_now("2w").is_err());
    }
}
