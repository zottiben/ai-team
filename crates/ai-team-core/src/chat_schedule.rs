//! Scheduled work that belongs to one exact chat.
//!
//! The clock, the reminder row and the guarded claim are the ones that were already
//! there (`schedule`, `daemon`). What this adds is a destination recorded when the person
//! schedules the work - this chat, this checkout, this prompt, these model settings -
//! rather than resolved at two in the morning from whichever run a project last had.
//!
//! Three properties are the whole point, and each is enforced somewhere specific:
//!
//! - **At most once per occurrence.** The reminder's guarded update picks one winner and
//!   `chat_schedule_occurrence` has a unique key on the occurrence it claimed, so the
//!   window and the daemon racing is still one dispatch.
//! - **The claim is committed before anything is dispatched**, carrying the turn's
//!   request id. A scheduler killed in between leaves an unsettled claim that is resolved
//!   by looking the turn up, never by running it again.
//! - **An unattended turn is never an approval.** A team chat is planned only; building
//!   still waits for the person, which is what it does when they are there.

use std::path::Path;

use serde::Serialize;

use crate::model::{Reminder, ScheduleOutcome};
use crate::{ChatMode, Error, ModelRegistry, Provider, Reasoning, Result, Store};

/// What to schedule, said once, by the person looking at the chat.
#[derive(Debug, Clone)]
pub struct NewChatSchedule {
    pub chat_id: i64,
    pub title: String,
    pub prompt: String,
    pub due_at: String,
    pub recur: Option<crate::model::Recur>,
}

/// The immutable destination of a schedule. Changing any of it would dispatch something
/// other than what was agreed to, with nobody there to notice.
#[derive(Debug, Clone, Serialize)]
pub struct ChatSchedule {
    pub id: i64,
    pub reminder_id: i64,
    pub chat_id: i64,
    pub project_id: i64,
    pub workspace_path: String,
    pub prompt: String,
    pub provider: Provider,
    pub model: String,
    pub reasoning: Reasoning,
    pub mode: ChatMode,
    pub created_at: String,
}

/// One claimed occurrence, and what it became.
#[derive(Debug, Clone, Serialize)]
pub struct ChatScheduleOccurrence {
    pub id: i64,
    pub schedule_id: i64,
    pub occurrence_at: String,
    pub request_id: String,
    pub claimed_at: String,
    /// Occurrences the clock slept through before this one. A nightly that fired once
    /// after a week off did not run six times, and this is where that is said.
    pub skipped: i64,
    pub outcome: ScheduleOutcome,
    pub detail: Option<String>,
    pub run_id: Option<i64>,
    pub node_id: Option<i64>,
    pub settled_at: Option<String>,
}

/// A claim nobody else can have, with the context it must be dispatched against.
#[derive(Debug, Clone)]
pub struct ClaimedOccurrence {
    pub occurrence: ChatScheduleOccurrence,
    pub schedule: ChatSchedule,
    pub reminder: Reminder,
}

/// What a dispatched occurrence did, as recorded before anything was spawned.
#[derive(Debug, Clone, Serialize)]
pub struct DispatchedOccurrence {
    pub occurrence_id: i64,
    pub chat_id: i64,
    pub mode: ChatMode,
    pub outcome: ScheduleOutcome,
    pub detail: String,
    pub run_id: Option<i64>,
    pub node_id: Option<i64>,
}

impl DispatchedOccurrence {
    pub fn started(&self) -> bool {
        self.outcome == ScheduleOutcome::Started
    }
}

/// Why this occurrence must not be dispatched, if there is a reason.
///
/// Every one of these is the context the person agreed to having changed since. Rather
/// than dispatching the nearest thing still possible, the occurrence is spent and says
/// what happened: unattended work that quietly adapts is work nobody authorised.
fn blocked(chat: &crate::Chat, schedule: &ChatSchedule) -> Option<(ScheduleOutcome, String)> {
    if chat.archived {
        return Some((
            ScheduleOutcome::Refused,
            "This chat is archived. Nothing was started; restore the chat or cancel the schedule."
                .into(),
        ));
    }
    if !crate::same_worktree(&chat.workspace_path, &schedule.workspace_path) {
        return Some((
            ScheduleOutcome::Refused,
            format!(
                "This chat now works in {}, not the checkout this schedule was made for ({}). \
                 Nothing was started.",
                chat.workspace_path, schedule.workspace_path
            ),
        ));
    }
    if chat.mode != schedule.mode {
        return Some((
            ScheduleOutcome::Refused,
            format!(
                "This chat's execution mode changed from {} to {} after this schedule was made. \
                 Nothing was started; schedule it again from the chat as it is now.",
                schedule.mode, chat.mode
            ),
        ));
    }
    // Deliberately not queued behind the turn that is running: an instruction written
    // yesterday for 09:00 is not an instruction for whenever today's work finishes.
    if chat.active_node_id.is_some() {
        return Some((
            ScheduleOutcome::Busy,
            "The chat was already working when this was due, so it was skipped. It was not \
             queued or retried; the next occurrence stands."
                .into(),
        ));
    }
    None
}

/// Decide and record what one claimed occurrence becomes, without taking a model turn.
///
/// Everything slow happens after this returns: the clock must not be held open for the
/// minutes an agent takes, and the occurrence's result has to be durable before a child
/// process exists that could outlive the writer.
pub async fn dispatch(db: &Path, claimed: &ClaimedOccurrence) -> Result<DispatchedOccurrence> {
    dispatch_with_registry(db, claimed, ModelRegistry::load())
}

fn dispatch_with_registry(
    db: &Path,
    claimed: &ClaimedOccurrence,
    registry: Result<ModelRegistry>,
) -> Result<DispatchedOccurrence> {
    let mut store = Store::open(db)?;
    let schedule = &claimed.schedule;
    let id = claimed.occurrence.id;

    let settle = |store: &mut Store,
                  outcome: ScheduleOutcome,
                  detail: String|
     -> Result<DispatchedOccurrence> {
        store.settle_chat_schedule_occurrence(id, outcome, &detail, None, None)?;
        Ok(DispatchedOccurrence {
            occurrence_id: id,
            chat_id: schedule.chat_id,
            mode: schedule.mode,
            outcome,
            detail,
            run_id: None,
            node_id: None,
        })
    };

    let chat = match store.chat(schedule.chat_id) {
        Ok(chat) => chat,
        Err(error) => {
            return settle(
                &mut store,
                ScheduleOutcome::Refused,
                format!("This schedule's chat could not be read: {error}"),
            )
        }
    };
    if let Some((outcome, detail)) = blocked(&chat, schedule) {
        return settle(&mut store, outcome, detail);
    }

    let registry = match registry {
        Ok(registry) => registry,
        Err(error) => {
            return settle(
                &mut store,
                ScheduleOutcome::Refused,
                format!("This machine's model policy could not be read: {error}"),
            )
        }
    };
    // The schedule's own model settings, put through policy now rather than trusted from
    // when it was written: a provider denied since then must not be reached unattended.
    let mut requested = chat.agent();
    requested.provider = schedule.provider;
    requested.model.clone_from(&schedule.model);
    requested.reasoning = schedule.reasoning;
    if let Err(error) = registry.resolve(&requested) {
        return settle(
            &mut store,
            ScheduleOutcome::Refused,
            format!("{error}. Nothing was started."),
        );
    }

    let settings = (schedule.mode == ChatMode::Single).then(|| {
        (
            schedule.provider,
            schedule.model.clone(),
            schedule.reasoning,
        )
    });
    let receipt = match store.begin_scheduled_chat_turn(
        schedule.chat_id,
        &schedule.prompt,
        &claimed.occurrence.request_id,
        &registry,
        settings,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            return settle(
                &mut store,
                ScheduleOutcome::Refused,
                format!("{error}. Nothing was started."),
            )
        }
    };

    let detail = match schedule.mode {
        ChatMode::Single => "Started in this chat.".to_string(),
        // Planning only, and said so where the result is read: an unattended run must not
        // be able to approve its own build (D5, rule 7).
        ChatMode::Team => "Started planning in this chat. The team stops for your approval \
             before any build, so nothing was built unattended."
            .to_string(),
    };
    store.settle_chat_schedule_occurrence(
        id,
        ScheduleOutcome::Started,
        &detail,
        Some(receipt.run_id),
        Some(receipt.node_id),
    )?;
    Ok(DispatchedOccurrence {
        occurrence_id: id,
        chat_id: schedule.chat_id,
        mode: schedule.mode,
        outcome: ScheduleOutcome::Started,
        detail,
        run_id: Some(receipt.run_id),
        node_id: Some(receipt.node_id),
    })
}

/// Supervise a turn the clock started, in the process that started it.
///
/// The same drivers a typed message uses, including follow-up advancement: an
/// instruction somebody queued for this chat is theirs, and it should not wait for them
/// to come back and press something because the turn before it was scheduled.
pub async fn drive_scheduled(db: &Path, chat_id: i64, node_id: i64, mode: ChatMode) {
    if mode == ChatMode::Team {
        if let Err(error) = crate::drive_chat_team_planning(db, chat_id, node_id).await {
            eprintln!("chat {chat_id} scheduled planning: {error}");
            // Never through the solo failure path: team ownership is settled only by
            // discovery, which keeps live workers and parks provably abandoned ones.
            if let Err(recovery) = crate::recover_abandoned_chat_team(db, chat_id).await {
                eprintln!("chat {chat_id} team recovery: {recovery}");
            }
        }
        return;
    }

    let mut node_id = node_id;
    loop {
        if let Err(error) = crate::drive_chat(db, chat_id, node_id, false).await {
            eprintln!("chat {chat_id} scheduled turn {node_id}: {error}");
            let recorded = Store::open(db)
                .and_then(|mut store| store.fail_chat_worker(chat_id, node_id, &error.to_string()));
            if let Err(recording) = recorded {
                eprintln!("chat {chat_id}: could not reconcile the scheduled worker: {recording}");
            }
            return;
        }
        let next = (|| -> Result<Option<crate::ChatSubmission>> {
            let registry = ModelRegistry::load()?;
            Store::open(db)?.advance_chat_followup(chat_id, node_id, &registry)
        })();
        match next {
            Ok(Some(receipt)) if receipt.started => node_id = receipt.node_id,
            Ok(_) => return,
            Err(error) => {
                eprintln!("chat {chat_id}: could not advance follow-up: {error}");
                let recorded = Store::open(db).and_then(|mut store| {
                    store.record_chat_followup_problem(chat_id, node_id, &error.to_string())
                });
                if let Err(recording) = recorded {
                    eprintln!("chat {chat_id}: could not record the held follow-up: {recording}");
                }
                return;
            }
        }
    }
}

/// Refuse to treat a chat schedule as a legacy project run.
///
/// Stated here as well as in the routing because the two paths are not interchangeable:
/// the legacy workflow leases worktrees, plans and builds against a project, and a chat
/// schedule is a message in somebody's conversation.
pub(crate) fn never_legacy(reminder: &Reminder) -> Result<()> {
    if reminder.chat_id.is_some() {
        return Err(Error::invalid(
            "this is a chat schedule; it is dispatched into its chat, never as a legacy run",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, Recur};
    use crate::{NewChat, NodeStatus};

    fn seed() -> (tempfile::TempDir, Store, crate::Chat) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::init(&dir.path().join("test.db")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let chat = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: dir.path().into(),
                provider: Provider::Local,
                model: "test-model".into(),
                reasoning: Reasoning::High,
            })
            .unwrap();
        (dir, store, chat)
    }

    fn schedule(store: &mut Store, chat: i64, due_at: &str) -> ChatSchedule {
        store
            .add_chat_schedule(NewChatSchedule {
                chat_id: chat,
                title: "nightly sweep".into(),
                prompt: "tidy the flaky tests".into(),
                due_at: due_at.into(),
                recur: None,
            })
            .unwrap()
    }

    /// Claim and dispatch without the clock's notifications or its spawned children.
    fn fire(db: &Path, store: &mut Store) -> Vec<DispatchedOccurrence> {
        let mut done = Vec::new();
        for claim in crate::schedule::claim_due(store, &crate::now()).unwrap() {
            let chat = claim.chat.expect("these fixtures schedule chats");
            done.push(dispatch_with_registry(db, &chat, Ok(ModelRegistry::local_only())).unwrap());
        }
        done
    }

    #[test]
    fn scheduled_admission_rechecks_recorded_mode_and_chat_at_the_write() {
        let (_dir, mut store, chat) = seed();
        store
            .seed_default_team(chat.project_id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        let claim = store
            .claim_chat_schedule_occurrence(&due[0])
            .unwrap()
            .unwrap();
        store
            .set_chat_mode(chat.id, ChatMode::Team, store.chat(chat.id).unwrap().rev)
            .unwrap();
        let result = store.begin_scheduled_chat_turn(
            chat.id,
            &made.prompt,
            &claim.occurrence.request_id,
            &ModelRegistry::local_only(),
            Some((made.provider, made.model, made.reasoning)),
        );
        assert!(
            result.is_err(),
            "a single-mode occurrence started a team execution"
        );
        assert!(store.chat_turns(chat.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_denied_schedule_never_reroutes_or_creates_a_turn() {
        let (dir, mut store, chat) = seed();
        schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        let claim = store
            .claim_chat_schedule_occurrence(&due[0])
            .unwrap()
            .unwrap();
        let registry = ModelRegistry::new(
            crate::MachineProfile::parse(crate::DEFAULT_MACHINE_PROFILE).unwrap(),
        );
        let result =
            dispatch_with_registry(&dir.path().join("test.db"), &claim, Ok(registry)).unwrap();
        assert_eq!(result.outcome, ScheduleOutcome::Refused);
        assert!(store.chat_turns(chat.id).unwrap().is_empty());
        assert!(!result.started());
    }

    #[tokio::test]
    async fn a_scheduled_prompt_reaches_the_exact_chat_it_was_made_in() {
        // Not the project's latest run, and not the legacy workflow: this is a message in
        // one conversation, with the checkout that conversation owns.
        let (dir, mut store, chat) = seed();
        let db = dir.path().join("test.db");
        schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");

        let fired = fire(&db, &mut store);
        assert_eq!(fired.len(), 1);
        assert!(fired[0].started(), "{:?}", fired[0].detail);

        let turns = store.chat_turns(chat.id).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].run.prompt, "tidy the flaky tests");
        assert_eq!(
            turns[0].node.worktree_path.as_deref(),
            Some(chat.workspace_path.as_str())
        );
        assert_eq!(
            store.chat(chat.id).unwrap().active_node_id,
            Some(turns[0].node.id)
        );
    }

    #[tokio::test]
    async fn a_chat_schedule_never_starts_a_legacy_workflow_run() {
        let (dir, mut store, chat) = seed();
        let db = dir.path().join("test.db");
        schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");

        let claimed = crate::schedule::claim_due(&mut store, &crate::now()).unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(claimed[0].chat.is_some(), "it must route to its chat");
        assert!(
            crate::schedule::runnable(&claimed[0].reminder).is_err(),
            "the legacy path must refuse it even if something asks"
        );
        let _ = dispatch_with_registry(
            &db,
            claimed[0].chat.as_ref().unwrap(),
            Ok(ModelRegistry::local_only()),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn one_occurrence_is_claimed_once_however_many_clocks_are_running() {
        // The window and the daemon both run this loop. Two dispatches of one occurrence
        // is two unattended agents in one conversation.
        let (_dir, mut store, chat) = seed();
        let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        assert_eq!(due.len(), 1);

        let first = store.claim_chat_schedule_occurrence(&due[0]).unwrap();
        assert!(first.is_some());
        // The second clock read the same row a moment earlier, as a concurrent one would.
        let second = store.claim_chat_schedule_occurrence(&due[0]).unwrap();
        assert!(second.is_none(), "a stale claim must lose");
        assert_eq!(
            store.chat_schedule_occurrences(made.id, 10).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn a_recurring_schedule_records_what_the_clock_slept_through() {
        // A machine off for three days owes one run, and owes the person the fact that
        // the other occurrences were skipped rather than silently never existing.
        let (dir, mut store, chat) = seed();
        let db = dir.path().join("test.db");
        let due = time::OffsetDateTime::now_utc() - time::Duration::days(3);
        let made = store
            .add_chat_schedule(NewChatSchedule {
                chat_id: chat.id,
                title: "nightly sweep".into(),
                prompt: "tidy the flaky tests".into(),
                due_at: due
                    .replace_nanosecond(0)
                    .unwrap()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
                recur: Some(Recur::Daily),
            })
            .unwrap();

        let fired = fire(&db, &mut store);
        assert_eq!(fired.len(), 1, "one run, not one per day away");
        let history = store.chat_schedule_occurrences(made.id, 10).unwrap();
        assert_eq!(history.len(), 1);
        // The occurrence that fired is the one three days old; the three between it and
        // the next are gone, and that is the number the person is owed.
        assert_eq!(history[0].skipped, 3);
        assert!(store.reminder(made.reminder_id).unwrap().due_at.unwrap() > crate::now());
    }

    #[tokio::test]
    async fn a_busy_chat_is_skipped_rather_than_queued_behind_the_turn_it_found() {
        let (dir, mut store, chat) = seed();
        let db = dir.path().join("test.db");
        let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        store
            .begin_chat_turn(
                chat.id,
                "working on it",
                "typed",
                &ModelRegistry::local_only(),
            )
            .unwrap();

        let fired = fire(&db, &mut store);
        assert_eq!(fired[0].outcome, ScheduleOutcome::Busy);
        assert_eq!(
            store.chat_turns(chat.id).unwrap().len(),
            1,
            "no second turn"
        );
        let history = store.chat_schedule_occurrences(made.id, 10).unwrap();
        assert_eq!(history[0].outcome, ScheduleOutcome::Busy);
        assert!(history[0].detail.as_deref().unwrap().contains("not"));
        // Visible where the chat is, because nothing in the conversation would show it.
        let notice = store.notifications(10).unwrap();
        assert_eq!(
            serde_json::to_value(&notice[0]).unwrap()["chat_id"],
            chat.id,
            "{notice:?}"
        );
    }

    #[tokio::test]
    async fn a_changed_chat_refuses_rather_than_dispatching_something_else() {
        for change in ["mode", "workspace", "archived"] {
            let (dir, mut store, chat) = seed();
            let db = dir.path().join("test.db");
            let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
            match change {
                "mode" => {
                    store
                        .set_chat_mode(chat.id, ChatMode::Team, store.chat(chat.id).unwrap().rev)
                        .unwrap();
                }
                "archived" => {
                    store.archive_chat(chat.id, true).unwrap();
                }
                _ => {
                    let moved = dir.path().join("elsewhere");
                    std::fs::create_dir_all(&moved).unwrap();
                    let turn = store
                        .begin_chat_turn(chat.id, "move", "move", &ModelRegistry::local_only())
                        .unwrap();
                    store
                        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
                        .unwrap();
                    store
                        .db_mut()
                        .write(|tx| {
                            tx.execute(
                                "UPDATE chat SET workspace_path = ?2 WHERE id = ?1",
                                rusqlite::params![
                                    chat.id,
                                    moved.canonicalize().unwrap().to_string_lossy()
                                ],
                            )?;
                            Ok(())
                        })
                        .unwrap();
                }
            }

            let fired = fire(&db, &mut store);
            assert_eq!(fired[0].outcome, ScheduleOutcome::Refused, "{change}");
            let history = store.chat_schedule_occurrences(made.id, 10).unwrap();
            assert_eq!(history[0].outcome, ScheduleOutcome::Refused, "{change}");
            assert!(history[0].settled_at.is_some());
        }
    }

    #[tokio::test]
    async fn a_team_chat_is_scheduled_to_plan_and_still_waits_for_approval_to_build() {
        // The one thing an unattended run must never be is an approval. A team schedule
        // produces a plan to look at, and the build still waits for the person (D5).
        let (dir, mut store, _) = seed();
        let db = dir.path().join("test.db");
        store
            .seed_default_team(1, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let chat = store
            .create_chat_in_mode(
                NewChat {
                    project_id: 1,
                    workspace: dir.path().into(),
                    provider: Provider::Local,
                    model: "test-model".into(),
                    reasoning: Reasoning::High,
                },
                ChatMode::Team,
            )
            .unwrap();
        let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");

        let fired = fire(&db, &mut store);
        assert!(fired[0].started(), "{:?}", fired[0].detail);
        assert_eq!(fired[0].mode, ChatMode::Team);
        assert!(fired[0].detail.contains("approval"), "{}", fired[0].detail);

        let execution = store
            .chat_team_run(fired[0].run_id.unwrap())
            .unwrap()
            .expect("a team turn has a controller");
        assert_eq!(execution.phase, crate::ChatTeamPhase::Grounding);
        assert_eq!(execution.approved_revision, None, "nothing was approved");
        assert!(store
            .chat_build_slices(execution.run_id)
            .unwrap()
            .is_empty());
        assert!(store.chat_schedule_occurrences(made.id, 10).unwrap()[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("approval"));
    }

    #[tokio::test]
    async fn a_scheduled_turn_uses_the_model_the_schedule_was_made_with() {
        // The schedule's own settings, not whatever the chat was last switched to: the
        // person agreed to this prompt on this model, unattended.
        let (dir, mut store, chat) = seed();
        let db = dir.path().join("test.db");
        schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE chat SET model = 'switched-since', reasoning = 'low' WHERE id = ?1",
                    rusqlite::params![chat.id],
                )?;
                Ok(())
            })
            .unwrap();

        let fired = fire(&db, &mut store);
        assert!(fired[0].started(), "{:?}", fired[0].detail);
        let turns = store.chat_turns(chat.id).unwrap();
        assert_eq!(turns[0].node.model, "test-model");
        assert_eq!(
            store.scheduled_reasoning(turns[0].node.id).unwrap(),
            Some(Reasoning::High)
        );
    }

    #[tokio::test]
    async fn a_scheduler_that_died_after_starting_a_turn_adopts_it_rather_than_repeating_it() {
        let (_dir, mut store, chat) = seed();
        let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        let claim = store
            .claim_chat_schedule_occurrence(&due[0])
            .unwrap()
            .unwrap();
        // The turn started, and the scheduler died before it could record that it had.
        let receipt = store
            .begin_scheduled_chat_turn(
                chat.id,
                &made.prompt,
                &claim.occurrence.request_id,
                &ModelRegistry::local_only(),
                None,
            )
            .unwrap();
        dead_claimer(&mut store, claim.occurrence.id);

        let settled = store.sweep_chat_schedule_claims().unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].outcome, ScheduleOutcome::Started);
        assert_eq!(settled[0].node_id, Some(receipt.node_id));
        assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1, "never twice");
        assert!(
            store.sweep_chat_schedule_claims().unwrap().is_empty(),
            "a settled occurrence is spent"
        );
    }

    #[tokio::test]
    async fn a_scheduler_that_died_before_starting_says_so_and_does_not_run_it_later() {
        let (_dir, mut store, chat) = seed();
        let made = schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        let claim = store
            .claim_chat_schedule_occurrence(&due[0])
            .unwrap()
            .unwrap();
        dead_claimer(&mut store, claim.occurrence.id);

        let settled = store.sweep_chat_schedule_claims().unwrap();
        assert_eq!(settled[0].outcome, ScheduleOutcome::Interrupted);
        assert!(store.chat_turns(chat.id).unwrap().is_empty());
        // And the clock does not come back for it: the occurrence is spent, the reminder
        // has moved on, and re-running what it is unsure about is how one becomes two.
        assert!(store.due_reminders(&crate::now()).unwrap().is_empty());
        assert_eq!(
            store.chat_schedule_occurrences(made.id, 10).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn a_live_claim_is_left_alone_by_the_other_clock() {
        let (_dir, mut store, chat) = seed();
        schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        store.claim_chat_schedule_occurrence(&due[0]).unwrap();
        assert!(
            store.sweep_chat_schedule_claims().unwrap().is_empty(),
            "this process is alive and mid-dispatch"
        );
    }

    #[tokio::test]
    async fn a_settled_occurrence_is_never_rewritten() {
        let (_dir, mut store, chat) = seed();
        schedule(&mut store, chat.id, "2020-01-01T00:00:00Z");
        let due = store.due_reminders(&crate::now()).unwrap();
        let claim = store
            .claim_chat_schedule_occurrence(&due[0])
            .unwrap()
            .unwrap();
        store
            .settle_chat_schedule_occurrence(
                claim.occurrence.id,
                ScheduleOutcome::Busy,
                "skipped",
                None,
                None,
            )
            .unwrap();
        assert!(store
            .settle_chat_schedule_occurrence(
                claim.occurrence.id,
                ScheduleOutcome::Started,
                "second thoughts",
                None,
                None,
            )
            .is_err());
    }

    #[tokio::test]
    async fn a_schedule_needs_a_prompt_of_its_own() {
        // An empty prompt means "build what is ready" to the legacy runner. In a chat it
        // would mean sending nothing to a model, and the person would never learn why.
        let (_dir, mut store, chat) = seed();
        assert!(store
            .add_chat_schedule(NewChatSchedule {
                chat_id: chat.id,
                title: "nightly".into(),
                prompt: "   ".into(),
                due_at: "2026-09-18T09:00:00Z".into(),
                recur: None,
            })
            .is_err());
    }

    #[tokio::test]
    async fn a_schedule_is_immutable_once_made() {
        let (_dir, mut store, chat) = seed();
        let made = schedule(&mut store, chat.id, "2026-09-18T09:00:00Z");
        assert!(store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE chat_schedule SET prompt = 'something else' WHERE id = ?1",
                    rusqlite::params![made.id],
                )?;
                Ok(())
            })
            .is_err());
    }

    #[tokio::test]
    async fn a_legacy_project_reminder_still_runs_the_legacy_workflow() {
        // The CLI's `ait remind run` is unchanged, and must stay that way.
        let (_dir, mut store, _chat) = seed();
        store
            .add_reminder(crate::model::NewReminder {
                project_id: Some(1),
                kind: Some(crate::model::ReminderKind::ScheduledRun),
                title: "nightly build".into(),
                prompt: Some("tidy the imports".into()),
                due_at: Some("2020-01-01T00:00:00Z".into()),
                ..Default::default()
            })
            .unwrap();

        let claimed = crate::schedule::claim_due(&mut store, &crate::now()).unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(claimed[0].chat.is_none());
        assert_eq!(
            crate::schedule::runnable(&claimed[0].reminder).unwrap(),
            (1, "tidy the imports".to_string())
        );
    }

    /// Make the claim look like one left by a scheduler that is no longer running. PID 0
    /// is not a process `kill(0)` reports as alive, which is what the sweep asks.
    fn dead_claimer(store: &mut Store, occurrence: i64) {
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE chat_schedule_occurrence SET claimed_pid = NULL,
                        claimed_identity = NULL WHERE id = ?1",
                    rusqlite::params![occurrence],
                )?;
                Ok(())
            })
            .unwrap();
    }
}
