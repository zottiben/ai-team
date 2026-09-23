//! The clock.
//!
//! A runtime may have schedules of its own, but they are static - declared in a project it
//! builds, which ai-team generates per team. A reminder somebody adds at four in the
//! afternoon has to work without regenerating and rebuilding anything, so the clock is
//! ours and it lives in Rust.
//!
//! Two processes run this loop: `ait ui`, so having the window open is enough, and
//! `ait daemon`, so closing the window does not stop the clock. That is deliberate, and
//! it is why claiming a reminder is a guarded update rather than a read followed by a
//! write. A scheduled run that fires twice is two unattended agents in one repository.
//!
//! Nothing here decides *what* a run does. A fired `scheduled_run` hands its prompt to
//! the same `run_workflow` the terminal and the window call, because a third way to start
//! a run is a third thing to keep in step.

use std::time::Duration;

use crate::error::Result;
use crate::model::{Reminder, ReminderKind};
use crate::store::Store;
use crate::util::now;

/// How often the clock looks. A minute is the resolution a human schedules at, and a
/// tighter loop would only find the same nothing more often.
pub const TICK: Duration = Duration::from_secs(20);

/// What one tick did.
#[derive(Debug, Clone)]
pub struct Fired {
    pub reminder: Reminder,
    /// Whether a run was started. Separate from `run_id` because a caller that starts
    /// the workflow detached genuinely does not have an id yet, and printing a made-up
    /// one is worse than printing none.
    pub started: bool,
    /// The run this started, when the caller waited long enough to know which.
    pub run_id: Option<i64>,
    /// Why nothing started, when it was a scheduled run that did not.
    pub problem: Option<String>,
}

/// Claim everything that is due, and say what was claimed.
///
/// Claiming is separate from acting on it: this returns quickly and holds no lock while
/// a workflow runs, which matters because a scheduled run takes minutes and the next tick
/// is twenty seconds away.
pub fn claim_due(store: &mut Store, at: &str) -> Result<Vec<Reminder>> {
    let due = store.due_reminders(at)?;
    let mut claimed = Vec::new();
    for reminder in due {
        // A loser here is the ordinary outcome when both the window and the daemon are
        // up, not an error worth reporting.
        if let Some(taken) = store.claim_reminder(&reminder)? {
            claimed.push(taken);
        }
    }
    Ok(claimed)
}

/// Tell the desktop something happened.
///
/// Shelling out rather than taking a notification crate: this is two one-line commands
/// that ship with the operating system, and a dependency here would be a build-time cost
/// on both platforms for something `osascript` already does.
pub async fn notify(title: &str, body: &str) -> bool {
    // Neither platform's notifier is worth failing a run over, so every error here is
    // swallowed on purpose - a missing notification must not stop the work it was
    // announcing.
    #[cfg(target_os = "macos")]
    let command = {
        // Quotes inside an AppleScript string end it, so they are stripped rather than
        // escaped - a reminder title is not worth an injection surface.
        let clean = |text: &str| text.replace(['"', '\\'], "");
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            clean(body),
            clean(title)
        );
        let mut command = tokio::process::Command::new("osascript");
        command.arg("-e").arg(script);
        command
    };

    #[cfg(not(target_os = "macos"))]
    let command = {
        let mut command = tokio::process::Command::new("notify-send");
        command.arg(title).arg(body);
        command
    };

    let mut command = command;
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// What a fired reminder should say.
pub fn announcement(reminder: &Reminder) -> (String, String) {
    let title = match reminder.kind {
        ReminderKind::ScheduledRun => "ai-team is starting a run",
        ReminderKind::Idea => "Idea",
        ReminderKind::Reminder => "Reminder",
    };
    (title.to_string(), reminder.title.clone())
}

/// Whether a reminder should start work, and what to say if it cannot.
///
/// A scheduled run needs a project and a prompt. Missing either is a configuration
/// mistake, and the useful thing to do is say so rather than fire silently every day
/// from now until somebody notices.
pub fn runnable(reminder: &Reminder) -> std::result::Result<(i64, String), String> {
    if reminder.kind != ReminderKind::ScheduledRun {
        return Err("not a scheduled run".into());
    }
    let project = reminder
        .project_id
        .ok_or("a scheduled run needs a project to run in")?;
    let prompt = reminder
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        // An empty prompt means "build what is ready" everywhere else in ai-team, and it
        // means the same here.
        .unwrap_or_default()
        .to_string();
    Ok((project, prompt))
}

/// Announce what was claimed, and start what should start.
///
/// Takes no `Store`, which is not tidiness: a `rusqlite::Connection` is `Send` but not
/// `Sync`, so a future holding one cannot be spawned - and this loop lives inside a
/// spawned task in both processes that run it. The caller claims synchronously, drops
/// its lock, and awaits this.
///
/// `start` is passed in because core must not decide how a run is supervised.
pub async fn act<F, Fut>(claimed: Vec<Reminder>, start: F) -> Vec<Fired>
where
    F: Fn(i64, String) -> Fut,
    Fut: std::future::Future<Output = Result<Option<i64>>>,
{
    let mut fired = Vec::new();

    for reminder in claimed {
        let (title, body) = announcement(&reminder);
        let _ = notify(&title, &body).await;

        let (started, run_id, problem) = match runnable(&reminder) {
            Err(_) if reminder.kind != ReminderKind::ScheduledRun => (false, None, None),
            Err(problem) => (false, None, Some(problem)),
            Ok((project, prompt)) => match start(project, prompt).await {
                Ok(run_id) => (true, run_id, None),
                // A failed start is reported, not retried: the reminder has already been
                // advanced to its next occurrence, and retrying inside the tick would
                // hold the clock for however long the failure takes.
                Err(error) => (false, None, Some(error.to_string())),
            },
        };

        fired.push(Fired {
            reminder,
            started,
            run_id,
            problem,
        });
    }
    fired
}

/// Claim and act, for a caller that has a `Store` to hand and nothing to keep it from.
pub async fn tick<F, Fut>(store: &mut Store, start: F) -> Result<Vec<Fired>>
where
    F: Fn(i64, String) -> Fut,
    Fut: std::future::Future<Output = Result<Option<i64>>>,
{
    let claimed = claim_due(store, &now())?;
    Ok(act(claimed, start).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, NewReminder, Recur, ReminderStatus};
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    fn store_with(reminder: NewReminder) -> (Store, Reminder) {
        let mut store = Store::memory().unwrap();
        store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let reminder = store.add_reminder(reminder).unwrap();
        (store, reminder)
    }

    #[test]
    fn a_missed_daily_fires_once_not_once_per_missed_day() {
        // Three days of downtime, then the clock comes back up. A person expects one
        // run, not one per day they were away.
        // Whole seconds, because `now()` stores whole seconds and these timestamps are
        // compared as strings - a fixture carrying nanoseconds sorts before the very
        // second it names.
        let due_three_days_ago = (OffsetDateTime::now_utc() - time::Duration::days(3))
            .replace_nanosecond(0)
            .unwrap();
        let (mut store, _) = store_with(NewReminder {
            project_id: Some(1),
            kind: Some(ReminderKind::ScheduledRun),
            title: "nightly sweep".into(),
            prompt: Some("tidy the imports".into()),
            due_at: Some(due_three_days_ago.format(&Rfc3339).unwrap()),
            recur: Some(Recur::Daily),
            ..Default::default()
        });

        // Tick until the clock settles, exactly as the daemon does every 20 seconds.
        let mut fired = 0;
        for _ in 0..10 {
            let claimed = claim_due(&mut store, &now()).unwrap();
            if claimed.is_empty() {
                break;
            }
            fired += claimed.len();
        }

        assert_eq!(
            fired, 1,
            "a daily reminder missed for 3 days fired {fired} times - each one \
             notifies and starts an unattended agent"
        );
    }

    fn due_now(kind: ReminderKind) -> NewReminder {
        NewReminder {
            project_id: Some(1),
            kind: Some(kind),
            title: "nightly sweep".into(),
            body: String::new(),
            prompt: Some("tidy the imports".into()),
            due_at: Some("2020-01-01T00:00:00Z".into()),
            recur: None,
            ..Default::default()
        }
    }

    #[test]
    fn only_one_claim_can_win() {
        // The whole reason claiming is a guarded update. Both the window and the daemon
        // run this loop, and a scheduled run that fires twice is two unattended agents
        // in one repository.
        let (mut store, reminder) = store_with(due_now(ReminderKind::ScheduledRun));

        let first = store.claim_reminder(&reminder).unwrap();
        assert!(first.is_some());

        // The second caller read the same row a moment earlier, as a concurrent
        // scheduler would have.
        let second = store.claim_reminder(&reminder).unwrap();
        assert!(second.is_none(), "a stale claim must lose");
    }

    #[test]
    fn a_recurring_reminder_comes_back_rather_than_finishing() {
        let (mut store, reminder) = store_with(NewReminder {
            recur: Some(Recur::Daily),
            ..due_now(ReminderKind::Reminder)
        });

        let fired = store.claim_reminder(&reminder).unwrap().unwrap();
        assert_eq!(fired.status, ReminderStatus::Pending);
        assert_ne!(fired.due_at, reminder.due_at, "it should have moved on");
        assert!(fired.last_fired_at.is_some());
    }

    #[test]
    fn a_one_off_is_done_once_it_has_fired() {
        let (mut store, reminder) = store_with(due_now(ReminderKind::Reminder));
        let fired = store.claim_reminder(&reminder).unwrap().unwrap();
        assert_eq!(fired.status, ReminderStatus::Fired);

        // And it is no longer due, so the next tick will not see it.
        assert!(store.due_reminders(&now()).unwrap().is_empty());
    }

    #[test]
    fn claiming_takes_what_is_due_and_leaves_what_is_not() {
        let (mut store, _) = store_with(due_now(ReminderKind::Reminder));
        store
            .add_reminder(NewReminder {
                title: "next week".into(),
                due_at: Some("2099-01-01T00:00:00Z".into()),
                ..due_now(ReminderKind::Reminder)
            })
            .unwrap();

        let claimed = claim_due(&mut store, &now()).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].title, "nightly sweep");
    }

    #[test]
    fn a_scheduled_run_without_a_project_says_so_instead_of_firing_silently() {
        // Otherwise it fires every night from now until somebody notices nothing runs.
        let mut reminder = store_with(due_now(ReminderKind::ScheduledRun)).1;
        reminder.project_id = None;
        assert!(runnable(&reminder).is_err());
    }

    #[test]
    fn a_scheduled_run_with_no_prompt_means_build_what_is_ready() {
        // Which is what an empty prompt means everywhere else in ai-team, and two
        // surfaces disagreeing about that is worse than either rule alone.
        let mut reminder = store_with(due_now(ReminderKind::ScheduledRun)).1;
        reminder.prompt = None;
        assert_eq!(runnable(&reminder).unwrap(), (1, String::new()));

        reminder.prompt = Some("   ".into());
        assert_eq!(runnable(&reminder).unwrap().1, "");
    }

    #[tokio::test]
    async fn a_tick_starts_the_run_a_scheduled_reminder_asked_for() {
        let (mut store, _) = store_with(due_now(ReminderKind::ScheduledRun));
        let fired = tick(&mut store, |project, prompt| async move {
            assert_eq!(project, 1);
            assert_eq!(prompt, "tidy the imports");
            Ok(Some(42))
        })
        .await
        .unwrap();

        assert_eq!(fired.len(), 1);
        assert!(fired[0].started);
        assert_eq!(fired[0].run_id, Some(42));
        assert!(fired[0].problem.is_none());
    }

    #[tokio::test]
    async fn a_plain_reminder_announces_itself_without_starting_anything() {
        let (mut store, _) = store_with(due_now(ReminderKind::Reminder));
        let fired = tick(&mut store, |_, _| async {
            panic!("a reminder is not a run");
        })
        .await
        .unwrap();

        assert_eq!(fired.len(), 1);
        assert!(!fired[0].started);
        assert!(fired[0].run_id.is_none());
        assert!(fired[0].problem.is_none());
    }

    #[tokio::test]
    async fn a_run_that_will_not_start_is_reported_rather_than_retried() {
        // The reminder has already advanced, and retrying inside the tick would hold the
        // clock for as long as the failure takes.
        let (mut store, _) = store_with(due_now(ReminderKind::ScheduledRun));
        let fired = tick(&mut store, |_, _| async {
            Err(crate::error::Error::invalid("no checkout for that project"))
        })
        .await
        .unwrap();

        assert!(!fired[0].started);
        assert_eq!(fired[0].run_id, None);
        assert!(fired[0].problem.as_deref().unwrap().contains("no checkout"));
    }

    #[tokio::test]
    async fn a_detached_start_reports_started_without_inventing_a_run_number() {
        // The daemon spawns the workflow and returns, so it genuinely does not know
        // which run it will be. Printing `#0` reads as a real id and is not one.
        let (mut store, _) = store_with(due_now(ReminderKind::ScheduledRun));
        let fired = tick(&mut store, |_, _| async { Ok(None) }).await.unwrap();
        assert!(fired[0].started);
        assert_eq!(fired[0].run_id, None);
        assert!(fired[0].problem.is_none());
    }

    #[tokio::test]
    async fn a_tick_with_nothing_due_does_nothing_at_all() {
        let mut store = Store::memory().unwrap();
        let fired = tick(&mut store, |_, _| async { panic!("nothing was due") })
            .await
            .unwrap();
        assert!(fired.is_empty());
    }
}
