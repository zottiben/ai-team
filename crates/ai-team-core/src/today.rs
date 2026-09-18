//! What to work on now, across every project.
//!
//! The list is the easy part; the order is the point. One principle decides it:
//!
//! > **An agent idle waiting on a person costs more than a person idle waiting on an
//! > agent.**
//!
//! A run parked on a question has a worktree leased, a plan claimed, and nothing
//! happening in it until somebody answers. That is the most expensive kind of waiting in
//! this system, so it sorts above everything - above a reminder that has come due, above
//! work that failed and will still be failed in an hour, and well above a slice that is
//! merely under way.
//!
//! Ranking is a pure function over gathered items, so the judgement can be argued with in
//! a test rather than discovered by opening the window on a bad morning.

use serde::Serialize;

use crate::error::Result;
use crate::model::{NodeStatus, ReminderKind};
use crate::store::Store;
use crate::util::now;

/// How long after its time a reminder is still merely "due" rather than "overdue".
///
/// Without a window the distinction is dead: the store only returns reminders whose time
/// has already passed, so every one of them would be overdue by a second and the gentler
/// tier would never be reached. A quarter hour is the point where a thing that pinged
/// becomes a thing that is being ignored.
const STILL_JUST_DUE: i64 = 15 * 60;

/// Why something is on the list, and how loudly.
///
/// The order of these variants *is* the ranking - `derive(Ord)` reads them top to bottom,
/// so moving one is a deliberate change to what the window tells somebody to do first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// A run is parked on you. Agents are idle and you are the reason.
    Blocking,
    /// A reminder whose time has already passed.
    Overdue,
    /// Work that failed and will not un-fail on its own.
    Failed,
    /// Finished work waiting for somebody to look at it.
    Review,
    /// The plan is holding a question nobody has answered.
    Question,
    /// Due about now, but nothing is waiting on it.
    Due,
    /// Under way. Here so the day is legible, not because it needs doing.
    InFlight,
}

impl Urgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Urgency::Blocking => "blocking",
            Urgency::Overdue => "overdue",
            Urgency::Failed => "failed",
            Urgency::Review => "review",
            Urgency::Question => "question",
            Urgency::Due => "due",
            Urgency::InFlight => "in_flight",
        }
    }
}

/// One thing that might want doing.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub urgency: Urgency,
    /// What kind of thing it is, for the icon and the link.
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    pub project: Option<String>,
    pub run_id: Option<i64>,
    /// When it started waiting. Ties break on this, oldest first: the thing that has
    /// been ignored longest is the thing most likely to be forgotten entirely.
    pub since: Option<String>,
}

/// Put the list in the order somebody should actually work through it.
///
/// Stable within a tier on `since`, oldest first. An item with no timestamp sorts after
/// ones that have it, because "unknown age" is weaker evidence than "waiting since 9am".
pub fn rank(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by(|a, b| {
        a.urgency
            .cmp(&b.urgency)
            .then_with(|| match (&a.since, &b.since) {
                (Some(left), Some(right)) => left.cmp(right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
    });
    items
}

/// Everything ai-team's own database knows about.
///
/// The plan's open questions live in ai-planner and are added by the caller, which has a
/// `Planner` per checkout - this stays synchronous so it holds no connection across an
/// await.
pub fn from_store(store: &Store) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    let at = now();

    for project in store.projects()? {
        let slug = project.slug.clone();

        for run in store.runs(Some(project.id), 50)? {
            // A parked run is the expensive case: everything it leased is sitting idle.
            for approval in store.pending_approvals(run.id)? {
                items.push(Item {
                    urgency: Urgency::Blocking,
                    kind: "approval".into(),
                    title: approval.summary.clone(),
                    detail: Some(format!("run #{} is waiting on you", run.id)),
                    project: Some(slug.clone()),
                    run_id: Some(run.id),
                    since: Some(approval.at.clone()),
                });
            }

            for node in store.node_runs(run.id)? {
                match node.status {
                    NodeStatus::Failed | NodeStatus::Blocked => items.push(Item {
                        urgency: Urgency::Failed,
                        kind: "node".into(),
                        title: format!(
                            "{} failed{}",
                            node.role,
                            node.slice_key
                                .as_ref()
                                .map(|key| format!(" on {key}"))
                                .unwrap_or_default()
                        ),
                        detail: node.blocked_reason.clone(),
                        project: Some(slug.clone()),
                        run_id: Some(run.id),
                        since: node.ended_at.clone().or(node.started_at.clone()),
                    }),
                    NodeStatus::Running => items.push(Item {
                        urgency: Urgency::InFlight,
                        kind: "node".into(),
                        title: format!(
                            "{} is building{}",
                            node.role,
                            node.slice_key
                                .as_ref()
                                .map(|key| format!(" {key}"))
                                .unwrap_or_default()
                        ),
                        detail: None,
                        project: Some(slug.clone()),
                        run_id: Some(run.id),
                        since: node.started_at.clone(),
                    }),
                    _ => {}
                }
            }
        }

        for review in store.reviews(Some(project.id), true)? {
            items.push(Item {
                urgency: Urgency::Review,
                kind: "review".into(),
                title: review.title.clone(),
                detail: Some(format!(
                    "{} unresolved comment(s)",
                    store.unresolved_count(review.id)?
                )),
                project: Some(slug.clone()),
                run_id: review.run_id,
                since: Some(review.created_at.clone()),
            });
        }
    }

    // Reminders are not per project - an idea jotted down at midnight belongs to whoever
    // is reading this, not to a checkout.
    for reminder in store.due_reminders(&at)? {
        let overdue = reminder
            .due_at
            .as_ref()
            .is_some_and(|due| slipped(due, &at) > STILL_JUST_DUE);
        items.push(Item {
            urgency: if overdue {
                Urgency::Overdue
            } else {
                Urgency::Due
            },
            kind: match reminder.kind {
                ReminderKind::ScheduledRun => "scheduled_run".into(),
                ReminderKind::Idea => "idea".into(),
                ReminderKind::Reminder => "reminder".into(),
            },
            title: reminder.title.clone(),
            detail: (!reminder.body.trim().is_empty()).then(|| reminder.body.clone()),
            project: None,
            run_id: None,
            since: reminder.due_at.clone(),
        });
    }

    Ok(items)
}

/// Seconds between two RFC3339 timestamps, or 0 if either cannot be read.
///
/// Unparseable is deliberately the gentler answer: a clock ai-team cannot read is not
/// grounds for telling somebody their morning is on fire.
fn slipped(due: &str, at: &str) -> i64 {
    let parse = |value: &str| {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
    };
    match (parse(due), parse(at)) {
        (Some(due), Some(at)) => (at - due).whole_seconds(),
        _ => 0,
    }
}

/// An open question ai-planner is holding, as an item.
pub fn from_question(project: &str, question: &str, asked: Option<String>) -> Item {
    Item {
        urgency: Urgency::Question,
        kind: "question".into(),
        title: question.to_string(),
        detail: Some("the plan is waiting on an answer".into()),
        project: Some(project.to_string()),
        run_id: None,
        since: asked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(urgency: Urgency, title: &str, since: Option<&str>) -> Item {
        Item {
            urgency,
            kind: "test".into(),
            title: title.into(),
            detail: None,
            project: None,
            run_id: None,
            since: since.map(ToString::to_string),
        }
    }

    #[test]
    fn a_parked_run_outranks_everything_else() {
        // The judgement this whole view rests on: an agent idle waiting on a person costs
        // more than a person idle waiting on an agent. A parked run has a worktree leased
        // and a slice claimed, and none of it moves until somebody answers.
        let ranked = rank(vec![
            item(Urgency::InFlight, "backend is building", None),
            item(Urgency::Due, "stand-up", None),
            item(Urgency::Review, "PR1", None),
            item(Urgency::Blocking, "may I commit?", None),
            item(Urgency::Failed, "verifier rejected it", None),
        ]);
        assert_eq!(ranked[0].title, "may I commit?");
        assert_eq!(
            ranked.iter().map(|i| i.urgency).collect::<Vec<_>>(),
            [
                Urgency::Blocking,
                Urgency::Failed,
                Urgency::Review,
                Urgency::Due,
                Urgency::InFlight
            ]
        );
    }

    #[test]
    fn an_overdue_reminder_beats_work_that_failed_and_will_stay_failed() {
        let ranked = rank(vec![
            item(Urgency::Failed, "gates rejected it", None),
            item(Urgency::Overdue, "ship the release", None),
        ]);
        assert_eq!(ranked[0].title, "ship the release");
    }

    #[test]
    fn within_a_tier_the_thing_waiting_longest_comes_first() {
        // The one most likely to be forgotten entirely.
        let ranked = rank(vec![
            item(Urgency::Blocking, "newer", Some("2026-09-18T10:00:00Z")),
            item(Urgency::Blocking, "oldest", Some("2026-09-18T08:00:00Z")),
            item(Urgency::Blocking, "middle", Some("2026-09-18T09:00:00Z")),
        ]);
        assert_eq!(
            ranked.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["oldest", "middle", "newer"]
        );
    }

    #[test]
    fn something_with_no_timestamp_sorts_after_something_with_one() {
        // "Unknown age" is weaker evidence than "waiting since 8am".
        let ranked = rank(vec![
            item(Urgency::Review, "undated", None),
            item(Urgency::Review, "dated", Some("2026-09-18T08:00:00Z")),
        ]);
        assert_eq!(ranked[0].title, "dated");
    }

    #[test]
    fn in_flight_work_is_listed_but_never_asked_for() {
        // It is on the list so the day is legible, not because it needs doing. Anything
        // that actually wants a human must outrank it.
        let ranked = rank(vec![
            item(Urgency::InFlight, "running", Some("2026-09-18T07:00:00Z")),
            item(
                Urgency::Question,
                "which database?",
                Some("2026-09-18T11:00:00Z"),
            ),
        ]);
        assert_eq!(ranked[0].title, "which database?");
    }

    #[test]
    fn a_reminder_that_just_pinged_is_due_and_one_from_this_morning_is_overdue() {
        // The store only hands back reminders whose time has passed, so without a window
        // every one of them is overdue by a second and the gentler tier is dead code.
        assert!(slipped("2026-09-18T03:59:00Z", "2026-09-18T04:00:00Z") <= STILL_JUST_DUE);
        assert!(slipped("2026-09-18T01:00:00Z", "2026-09-18T04:00:00Z") > STILL_JUST_DUE);
    }

    #[test]
    fn a_clock_that_cannot_be_read_does_not_raise_the_alarm() {
        assert_eq!(slipped("whenever", "2026-09-18T04:00:00Z"), 0);
    }

    #[test]
    fn ranking_an_empty_day_is_an_empty_day() {
        assert!(rank(Vec::new()).is_empty());
    }
}
