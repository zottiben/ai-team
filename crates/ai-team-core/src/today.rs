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
    /// The review, for a review waiting on somebody: following it opens the review itself
    /// rather than the run that built it.
    pub review_id: Option<i64>,
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

/// The newest accepted maker turn on each PR, by plan and slice.
///
/// A rejected attempt is evidence, not work waiting on anyone, once a later turn on the
/// same PR - in its own run or a review follow-up - was accepted. The verifier's rows name
/// the PR too, but a verifier's turn finishing is not the PR being accepted.
fn newest_accepted(
    store: &Store,
    runs: &[crate::model::Run],
) -> Result<std::collections::HashMap<(Option<String>, String), i64>> {
    let mut accepted = std::collections::HashMap::new();
    for run in runs {
        for node in store.node_runs(run.id)? {
            if node.role == crate::VERIFIER_ROLE {
                continue;
            }
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
            review_id: None,
            since: node.ended_at.clone().or(node.started_at.clone()),
        }),
        NodeStatus::Running => Some(Item {
            urgency: Urgency::InFlight,
            kind: "node".into(),
            title: format!(
                "{} is {}{}",
                node.role,
                if node.role == crate::VERIFIER_ROLE {
                    "checking"
                } else {
                    "building"
                },
                on(" ")
            ),
            detail: None,
            project: Some(slug.to_string()),
            run_id: Some(run_id),
            review_id: None,
            since: node.started_at.clone(),
        }),
        _ => None,
    }
}

/// A turn whose run's process stopped under it, as something to resume.
fn interrupted_item(node: &crate::model::NodeRun, slug: &str, run_id: i64) -> Item {
    let work = [node.slice_key.as_deref(), node.task_key.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    Item {
        urgency: Urgency::Blocking,
        kind: "node".into(),
        title: format!("{} was interrupted on {work}", node.role),
        detail: Some(format!(
            "The ai-team process running run #{run_id} stopped part-way through this turn. \
             Resume it from Work to carry on in the same session and worktree."
        )),
        project: Some(slug.to_string()),
        run_id: Some(run_id),
        review_id: None,
        since: node.started_at.clone(),
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
                    review_id: None,
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
            let nodes = store.node_runs(run.id)?;
            // A PR's rows stay running until its verdict, so the turns that built it are
            // still open while it is checked. Its one seat actually at work is its newest
            // running row; the others are waiting on the answer.
            let waiting = |node: &crate::model::NodeRun| {
                node.status == NodeStatus::Running
                    && node.slice_key.is_some()
                    && nodes.iter().any(|later| {
                        later.id > node.id
                            && later.status == NodeStatus::Running
                            && later.slice_key == node.slice_key
                    })
            };
            // Its process stopped with this turn part-way: nobody is building it, and it
            // carries on only when somebody resumes it.
            let interrupted = run.status == crate::model::RunStatus::Blocked
                && run.blocked_reason.as_deref() == Some(crate::store::INTERRUPTED_REASON);
            for node in &nodes {
                if matches!(node.status, NodeStatus::Failed | NodeStatus::Blocked)
                    && superseded(node)
                {
                    continue;
                }
                if waiting(node) {
                    continue;
                }
                if interrupted && node.status == NodeStatus::Running {
                    items.push(interrupted_item(node, &slug, run.id));
                    continue;
                }
                if let Some(item) = node_item(node, &slug, run.id) {
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
                review_id: Some(review.id),
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
            review_id: None,
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
///
/// `reviewed` says the slice's branch already has an open review. That review is the same
/// finished PR, and already listed; the slice is only listed where there is none.
pub fn from_slice(
    project: &str,
    key: &str,
    title: &str,
    status: &str,
    reviewed: bool,
) -> Option<Item> {
    let (urgency, detail) = match status {
        "in_review" if reviewed => return None,
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
        review_id: None,
        since: None,
    })
}

/// A project's checkout, and what ai-team already lists for it, read before asking its
/// plan anything - so no database connection is held across the `aip` calls.
#[derive(Debug, Clone)]
pub struct Checkout {
    slug: String,
    repo: String,
    /// Branches whose PR already has an open review, listed as that review.
    reviewed: std::collections::HashSet<String>,
    /// The plans to ask, named: never whichever ai-planner would infer for the checkout.
    plans: Vec<String>,
}

/// Every project with a checkout, as [`from_plans`] needs it.
pub fn checkouts(store: &Store) -> Result<Vec<Checkout>> {
    let mut found = Vec::new();
    for project in store.projects()? {
        let Some(repo) = store
            .project_repos(project.id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
        else {
            continue;
        };
        let reviewed = store
            .reviews(Some(project.id), true)?
            .into_iter()
            .filter_map(|review| review.branch)
            .collect();
        found.push(Checkout {
            plans: store.plans_in_use(project.id)?,
            slug: project.slug,
            repo,
            reviewed,
        });
    }
    Ok(found)
}

/// What the plans each project's work is in are holding: open questions, and finished or
/// blocked slices.
///
/// Best effort per plan: a checkout that has moved, or a plan that cannot be read, must not
/// empty the list for every other one.
pub async fn from_plans(checkouts: Vec<Checkout>) -> Vec<Item> {
    let mut items = Vec::new();
    for Checkout {
        slug,
        repo,
        reviewed,
        plans,
    } in checkouts
    {
        for plan in plans {
            let planner = crate::neighbours::Planner::at(&repo).for_plan(plan);
            let Ok(questions) = planner.open_questions().await else {
                continue;
            };
            for question in questions {
                items.push(from_question(&slug, &question.body, question.asked_at));
            }
            // Work an agent finished and left `in_review` is the commonest thing waiting
            // after a run, and it lives on the plan rather than in ai-team's own tables.
            for slice in planner.slices().await.unwrap_or_default() {
                let has_review = slice
                    .branch
                    .as_ref()
                    .is_some_and(|branch| reviewed.contains(branch));
                if let Some(item) =
                    from_slice(&slug, &slice.key, &slice.title, &slice.status, has_review)
                {
                    items.push(item);
                }
            }
        }
    }
    items
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
        review_id: None,
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
            review_id: None,
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
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
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

        // Rejected, and then only checked: a verifier finishing is not the PR accepted.
        let unrepaired = store
            .dispatch_task(run.id, backend, "PR4", Some("T1"), &registry)
            .unwrap();
        store
            .set_node_status(unrepaired.id, NodeStatus::Failed)
            .unwrap();
        let checked = store
            .dispatch_task(run.id, seat(&store, "verifier"), "PR4", None, &registry)
            .unwrap();
        store.set_node_status(checked.id, NodeStatus::Done).unwrap();

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
        assert_eq!(failed, ["backend failed on PR4", "backend failed on PR2"]);
    }

    #[test]
    fn a_pr_in_flight_is_one_item_naming_the_seat_taking_its_turn() {
        // A PR's rows stay running until its verdict, so while the verifier checks it the
        // turns that built it are open too. That is one PR being checked, not three seats
        // at work.
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
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
        let backend = seat(&store, "backend");
        let running = |store: &mut Store, agent: i64, slice: &str, task: Option<&str>| {
            let node = store
                .dispatch_task(run.id, agent, slice, task, &registry)
                .unwrap();
            store.set_node_status(node.id, NodeStatus::Running).unwrap();
        };
        let (verifier, frontend) = (seat(&store, "verifier"), seat(&store, "frontend"));
        running(&mut store, backend, "PR1", Some("T1"));
        running(&mut store, backend, "PR1", Some("T2"));
        running(&mut store, verifier, "PR1", None);
        running(&mut store, frontend, "PR2", Some("T1"));

        let flying: Vec<String> = from_store(&store)
            .unwrap()
            .into_iter()
            .filter(|item| item.urgency == Urgency::InFlight)
            .map(|item| item.title)
            .collect();
        assert_eq!(
            flying,
            ["verifier is checking PR1", "frontend is building PR2"]
        );
    }

    #[test]
    fn a_turn_whose_process_stopped_is_something_to_resume_not_the_team_working() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let backend = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap()
            .id;
        let run = store
            .create_run(project.id, "build", crate::RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch_task(
                run.id,
                backend,
                "PR1",
                Some("T2"),
                &crate::machine::ModelRegistry::local_only(),
            )
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Running).unwrap();
        store
            .block_run(run.id, crate::store::INTERRUPTED_REASON)
            .unwrap();

        let items = from_store(&store).unwrap();

        assert!(
            items.iter().all(|item| item.urgency != Urgency::InFlight),
            "{items:?}"
        );
        let resume = items
            .iter()
            .find(|item| item.run_id == Some(run.id))
            .unwrap();
        assert_eq!(resume.urgency, Urgency::Blocking);
        assert_eq!(resume.title, "backend was interrupted on PR1 T2");
        assert!(resume
            .detail
            .as_deref()
            .unwrap()
            .contains("Resume it from Work"));
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
    fn a_projects_plans_are_the_ones_its_runs_used_never_one_inferred() {
        // Asked with no plan named, ai-planner answers with whichever plan the checkout has
        // resolved to most - on the author's machine a finished plan from weeks before - so
        // Today listed that plan's questions and slices, and never the ones the team's
        // runs were working through (rule 7).
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        store
            .attach_repo(
                project.id,
                crate::model::NewRepo {
                    main_path: Some(dir.path().to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .unwrap();

        // A project ai-team has planned nothing in has no plan to ask.
        assert!(checkouts(&store).unwrap()[0].plans.is_empty());

        let side = dir.path().join("side");
        for (prompt, plan, workspace) in [
            ("first", "csv", dir.path()),
            ("then", "json", dir.path()),
            ("aside", "xml", side.as_path()),
        ] {
            let run = store
                .create_run_in(
                    project.id,
                    prompt,
                    crate::RunTrigger::Manual,
                    Some(workspace),
                )
                .unwrap();
            store.set_run_plan(run.id, plan).unwrap();
        }
        // The newest plan in each checkout ai-team has run in, newest first; the plan a
        // checkout has moved on from is not asked.
        assert_eq!(checkouts(&store).unwrap()[0].plans, ["xml", "json"]);
    }

    #[test]
    fn a_review_waiting_on_you_says_which_review_it_is() {
        // Today listed the review, and following it opened the run that built it - a page
        // of agent activity - so reviewing meant finding it again under Review.
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let run = store
            .create_run(project.id, "build it", crate::RunTrigger::Manual)
            .unwrap();
        let review = store
            .open_review(
                project.id,
                "PR1: subtract",
                Some(run.id),
                None,
                Some("w/pr1"),
            )
            .unwrap();

        let items = from_store(&store).unwrap();
        let waiting = items.iter().find(|item| item.kind == "review").unwrap();
        assert_eq!(waiting.review_id, Some(review.id));
        assert_eq!(waiting.run_id, Some(run.id));
        // Everything else is not a review, and says so by having none.
        assert!(items
            .iter()
            .filter(|item| item.kind != "review")
            .all(|item| item.review_id.is_none()));
    }

    #[test]
    fn a_slice_left_in_review_is_the_answer_to_what_now() {
        // The commonest thing waiting after a run, and it lives in ai-planner rather than
        // in ai-team's own review table - so without it Today was empty the moment a run
        // succeeded, which is exactly when somebody looks.
        let item = from_slice("widget", "S1", "Add subtract", "in_review", false).unwrap();
        assert_eq!(item.urgency, Urgency::Review);
        assert!(item.title.contains("S1"));

        // Blocked work will not unblock itself.
        assert_eq!(
            from_slice("widget", "S2", "x", "blocked", false)
                .unwrap()
                .urgency,
            Urgency::Failed
        );

        // Everything else is work in progress or work done, and neither wants a human.
        for status in ["ready", "active", "done", "draft", "deferred"] {
            assert!(
                from_slice("widget", "S3", "x", status, false).is_none(),
                "{status}"
            );
        }
    }

    #[test]
    fn a_slice_with_its_own_review_open_is_that_review_not_a_second_item() {
        // ai-team opens a review for every PR it builds, and the plan marks the same PR
        // `in_review`: listed both ways, two finished PRs read as four things to look at.
        assert!(from_slice("widget", "PR1", "Shout", "in_review", true).is_none());
        // Without a review - built by hand, say - the plan is the only place it shows.
        assert!(from_slice("widget", "PR1", "Shout", "in_review", false).is_some());
        // Blocked says something a review does not.
        assert!(from_slice("widget", "PR1", "Shout", "blocked", true).is_some());
    }

    #[test]
    fn ranking_an_empty_day_is_an_empty_day() {
        assert!(rank(Vec::new()).is_empty());
    }
}
