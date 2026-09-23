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

/// The newest accepted turn on each PR, by plan and slice.
///
/// A rejected attempt is evidence, not work waiting on anyone, once a later turn on the
/// same PR - in its own run or a review follow-up - was accepted.
fn newest_accepted(
    store: &Store,
    runs: &[crate::model::Run],
) -> Result<std::collections::HashMap<(Option<String>, String), i64>> {
    let mut accepted = std::collections::HashMap::new();
    for run in runs {
        for node in store.node_runs(run.id)? {
            if let (NodeStatus::Done, Some(slice)) = (node.status, node.slice_key) {
                let newest = accepted
                    .entry((run.plan_slug.clone(), slice))
                    .or_insert(node.id);
                *newest = (*newest).max(node.id);
            }
        }
    }
    Ok(accepted)
}

/// What one node run puts on the list: failed work, or work in flight.
fn node_item(node: &crate::model::NodeRun, slug: &str, run_id: i64) -> Option<Item> {
    let on = |joiner: &str| {
        node.slice_key
            .as_ref()
            .map(|key| format!("{joiner}{key}"))
            .unwrap_or_default()
    };
    match node.status {
        NodeStatus::Failed | NodeStatus::Blocked => Some(Item {
            urgency: Urgency::Failed,
            kind: "node".into(),
            title: format!("{} failed{}", node.role, on(" on ")),
            detail: node.blocked_reason.clone(),
            project: Some(slug.to_string()),
            run_id: Some(run_id),
            since: node.ended_at.clone().or(node.started_at.clone()),
        }),
        NodeStatus::Running => Some(Item {
            urgency: Urgency::InFlight,
            kind: "node".into(),
            title: format!("{} is building{}", node.role, on(" ")),
            detail: None,
            project: Some(slug.to_string()),
            run_id: Some(run_id),
            since: node.started_at.clone(),
        }),
        _ => None,
    }
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

        let runs = store.runs(Some(project.id), 50)?;
        let accepted = newest_accepted(store, &runs)?;

        for run in runs {
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

            let superseded = |failed: &crate::model::NodeRun| {
                failed.slice_key.as_ref().is_some_and(|slice| {
                    accepted
                        .get(&(run.plan_slug.clone(), slice.clone()))
                        .is_some_and(|newest| *newest > failed.id)
                })
            };
            for node in store.node_runs(run.id)? {
                if matches!(node.status, NodeStatus::Failed | NodeStatus::Blocked)
                    && superseded(&node)
                {
                    continue;
                }
                if let Some(item) = node_item(&node, &slug, run.id) {
                    items.push(item);
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

/// A slice the plan says is waiting on a person, as an item.
///
/// Work an agent finished and left `in_review` is the commonest thing waiting after a
/// run, and it lives in ai-planner rather than in ai-team's own `review` table - so
/// without this, the most likely answer to "I just ran something, what now" was
/// "nothing".
pub fn from_slice(project: &str, key: &str, title: &str, status: &str) -> Option<Item> {
    let (urgency, detail) = match status {
        "in_review" => (Urgency::Review, "finished, waiting for you to look at it"),
        // Blocked work will not unblock itself, and the reason is on the slice.
        "blocked" => (Urgency::Failed, "blocked - the plan says why"),
        _ => return None,
    };
    Some(Item {
        urgency,
        kind: "slice".into(),
        title: format!("{key} - {title}"),
        detail: Some(detail.into()),
        project: Some(project.to_string()),
        run_id: None,
        since: None,
    })
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
    fn a_rejected_attempt_whose_pr_was_then_accepted_is_not_failed_work() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let seat = |store: &Store, role: &str| {
            store
                .agents(team.id)
                .unwrap()
                .into_iter()
                .find(|agent| agent.role == role)
                .unwrap()
                .id
        };
        let registry = crate::machine::ModelRegistry::local_only();
        let run = store
            .create_run(project.id, "build", crate::RunTrigger::Manual)
            .unwrap();
        let frontend = seat(&store, "frontend");

        // Rejected on its first go, repaired on its second: an accepted PR.
        let rejected = store
            .dispatch_task(run.id, frontend, "PR1", Some("T1"), &registry)
            .unwrap();
        store.block_node(rejected.id, "gates failed").unwrap();
        store
            .set_node_status(rejected.id, NodeStatus::Failed)
            .unwrap();
        let repaired = store
            .dispatch_task(run.id, frontend, "PR1", Some("T1"), &registry)
            .unwrap();
        store
            .set_node_status(repaired.id, NodeStatus::Done)
            .unwrap();

        // Rejected in the run that built it, accepted by a follow-up run on the same PR.
        store.set_run_plan(run.id, "csv").unwrap();
        let backend = seat(&store, "backend");
        let first = store
            .dispatch_task(run.id, backend, "PR3", Some("T1"), &registry)
            .unwrap();
        store.set_node_status(first.id, NodeStatus::Failed).unwrap();
        let follow = store
            .create_run(project.id, "address review", crate::RunTrigger::Review)
            .unwrap();
        store.set_run_plan(follow.id, "csv").unwrap();
        let later = store
            .dispatch_task(follow.id, backend, "PR3", Some("T1"), &registry)
            .unwrap();
        store.set_node_status(later.id, NodeStatus::Done).unwrap();

        // Rejected and never repaired: still somebody's problem.
        let stuck = store
            .dispatch_task(run.id, backend, "PR2", None, &registry)
            .unwrap();
        store.block_node(stuck.id, "out of repairs").unwrap();
        store.set_node_status(stuck.id, NodeStatus::Failed).unwrap();

        let failed: Vec<String> = from_store(&store)
            .unwrap()
            .into_iter()
            .filter(|item| item.urgency == Urgency::Failed)
            .map(|item| item.title)
            .collect();
        assert_eq!(failed, ["backend failed on PR2"]);
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
    fn a_slice_left_in_review_is_the_answer_to_what_now() {
        // The commonest thing waiting after a run, and it lives in ai-planner rather than
        // in ai-team's own review table - so without it Today was empty the moment a run
        // succeeded, which is exactly when somebody looks.
        let item = from_slice("widget", "S1", "Add subtract", "in_review").unwrap();
        assert_eq!(item.urgency, Urgency::Review);
        assert!(item.title.contains("S1"));

        // Blocked work will not unblock itself.
        assert_eq!(
            from_slice("widget", "S2", "x", "blocked").unwrap().urgency,
            Urgency::Failed
        );

        // Everything else is work in progress or work done, and neither wants a human.
        for status in ["ready", "active", "done", "draft", "deferred"] {
            assert!(
                from_slice("widget", "S3", "x", status).is_none(),
                "{status}"
            );
        }
    }

    #[test]
    fn ranking_an_empty_day_is_an_empty_day() {
        assert!(rank(Vec::new()).is_empty());
    }
}
