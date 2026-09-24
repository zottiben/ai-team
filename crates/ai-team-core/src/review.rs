//! Reviewing an agent's work, and what submitting one actually does.
//!
//! A review is a diff plus the things a human said about it. The interesting question is
//! what happens when they press submit, because there are two very different worlds:
//!
//! - the node that wrote the code is **still alive** - its eve process is up and its
//!   session is parked - in which case the comments go straight to it, and it fixes the
//!   work in the worktree it already has;
//! - the node is **gone**, which is the normal case an hour later, and then the comments
//!   have to become a new slice on the plan for whoever picks it up next.
//!
//! Getting that distinction wrong in the quiet direction is the bad failure: a review
//! submitted into a dead process disappears, and the human believes it landed. So the
//! liveness check is a real request to the agent, not an inference from a database
//! column, and anything short of a healthy answer falls through to writing a slice.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{CommentStatus, Review, ReviewComment};
use crate::neighbours::git;
use crate::store::Store;
use crate::FileDiff;

/// What submitting a review did, so the surface can say so rather than guess.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Submitted {
    /// The node that wrote the code took the comments and is working on them.
    Steered {
        node_run_id: i64,
        comments: usize,
        /// Whether the orchestrator was told directly as well.
        ///
        /// It always learns through the amended slice, which it reads on its next plan
        /// read - but that is the next time it looks, and if it is mid-turn deciding what
        /// the work is right now, it is deciding from the old text. So it is told when it
        /// has a session to tell.
        told_orchestrator: bool,
    },
    /// The seat that wrote the code had finished, but its pull request is still open in
    /// the worktree it was built in - so a follow-up run has picked the comments up there,
    /// on that branch, in that seat's own conversation.
    FollowedUp {
        slice_key: String,
        run_id: i64,
        comments: usize,
    },
    /// Nobody was listening, so the work is on the plan for the next run.
    Planned { slice_key: String, comments: usize },
    /// Approved with nothing to act on.
    Accepted,
}

/// The diff a review is about.
///
/// Measured from the point the branch forked, not from the tip of the base: diffing
/// against the tip attributes everything anybody else landed in the meantime to this
/// agent, which is how a review of four lines turns into a review of four hundred.
pub async fn diff_for(review: &Review, repo: &Path) -> Result<Vec<FileDiff>> {
    diff_against(review, repo, None).await
}

/// The diff a review is about, measured from the branch its pull request is based on.
///
/// A PR stacked on another is reviewed against that one's branch, as GitHub will show it:
/// measured from the default branch, the second PR of a stack shows the first one's work
/// as well, and a reviewer comments on code that is not this PR's to change. Asked of the
/// branch each time rather than pinned, because a stack is rebased when its parent moves.
pub async fn diff_against(
    review: &Review,
    repo: &Path,
    base_branch: Option<&str>,
) -> Result<Vec<FileDiff>> {
    let (base, head) = range_for(review, repo, base_branch).await?;

    // If this checkout is sitting on the branch under review, diff against what is on
    // disk rather than against its last commit. A hosted review cannot do this - there is
    // no working tree on a server - but a local one should: somebody who opens a file in
    // the editor and fixes it by hand expects to see that, not to be told to commit first
    // so the tool can notice.
    //
    // Only when the review is not pinned to a commit range. A pinned review keeps showing
    // the same diff on purpose, because that is what a comment on line 42 needs to stay
    // true.
    let pinned = review.base_sha.is_some() && review.head_sha.is_some();
    let on_branch = match (&review.branch, git::current_branch(repo).await) {
        (Some(branch), Some(here)) => branch == &here,
        _ => false,
    };

    let raw = if !pinned && on_branch {
        git::diff_worktree(repo, &base).await?
    } else {
        git::diff(repo, &base, &head).await?
    };
    Ok(crate::parse_diff(&raw))
}

/// Resolve a review's two commits, preferring what was recorded when it was opened.
///
/// A review pinned to shas keeps showing the same diff after the branch moves on, which
/// is what a comment on line 42 needs to stay true.
async fn range_for(
    review: &Review,
    repo: &Path,
    base_branch: Option<&str>,
) -> Result<(String, String)> {
    if let (Some(base), Some(head)) = (&review.base_sha, &review.head_sha) {
        return Ok((base.clone(), head.clone()));
    }
    let branch = review
        .branch
        .as_deref()
        .ok_or_else(|| Error::invalid("that review has no branch and no commit range"))?;
    let head = git::rev_parse(repo, branch).await?;

    // Measured from the branch it is based on - the default branch unless it stacks -
    // not from HEAD. A leased worktree is normally *sitting on* the branch under review,
    // so `merge-base(HEAD, branch)` is the branch tip itself and the diff comes back
    // empty - which reads as "the agent changed nothing" rather than as a bug in the tool.
    let base_ref = match base_branch {
        Some(base) => base.to_string(),
        None => git::default_branch(repo).await.ok_or_else(|| {
            Error::invalid("this repository has no main branch to measure against")
        })?,
    };
    let base = git::merge_base(repo, &base_ref, branch).await?;
    Ok((base, head))
}

/// The review, written as an addition to the slice's scope.
///
/// Phrased as an override rather than a note, because it is read alongside the original
/// text and the two will contradict each other - that is the whole point of a review.
pub fn amendment(comments: &[ReviewComment]) -> String {
    let mut out = String::from(
        "## Review feedback\n\nA human reviewed this work. Where the following \
         contradicts the scope above, the following wins.\n\n",
    );
    for comment in comments {
        match (&comment.file_path, comment.line_start) {
            (Some(path), Some(line)) => {
                let _ = writeln!(out, "- `{path}:{line}` - {}", comment.body.trim());
            }
            (Some(path), None) => {
                let _ = writeln!(out, "- `{path}` - {}", comment.body.trim());
            }
            _ => {
                let _ = writeln!(out, "- {}", comment.body.trim());
            }
        }
    }
    out
}

/// The same comments, addressed to the orchestrator.
///
/// It is not being asked to fix the code - somebody else is already doing that. It is being
/// told that what the work *is* has changed, which is its business, and that the slice it
/// reads has been amended to match.
pub fn for_orchestrator(comments: &[ReviewComment], review: &Review) -> String {
    let mut out = format!(
        "A human reviewed {} and asked for changes. The seat that wrote it is acting on \
         them, and the slice has been amended so the plan matches. You do not need to make \
         these edits.\n\nWhat they said:\n\n",
        review.title
    );
    for comment in comments {
        match (&comment.file_path, comment.line_start) {
            (Some(path), Some(line)) => {
                let _ = writeln!(out, "- `{path}:{line}` - {}", comment.body.trim());
            }
            (Some(path), None) => {
                let _ = writeln!(out, "- `{path}` - {}", comment.body.trim());
            }
            _ => {
                let _ = writeln!(out, "- {}", comment.body.trim());
            }
        }
    }
    out.push_str(
        "\nTake it into account in anything you plan next. If it changes what remains to be \
         done, say so on the plan.",
    );
    out
}

/// Who is going to read the comments, which changes what they need to be told.
///
/// The agent that wrote the code still has its worktree and its context. An agent
/// picking the work up off the plan tomorrow has neither, and telling it to use "the
/// worktree you already have" is an instruction it cannot follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// The node that wrote the code, still running.
    TheSameNode,
    /// Whoever picks the slice up next.
    Whoever { branch: Option<&'static str> },
}

/// Format the open comments as the message the responsible agent receives.
///
/// Deliberately plain. These are the operator's own words about their own code, so they
/// are an instruction rather than untrusted input - but they still say *where* before
/// *what*, because an agent that cannot find the line cannot act on the note.
pub fn as_instructions(comments: &[ReviewComment]) -> String {
    instructions_for(comments, Audience::TheSameNode, None)
}

/// The same comments, addressed to whoever is going to act on them.
pub fn instructions_for(
    comments: &[ReviewComment],
    audience: Audience,
    branch: Option<&str>,
) -> String {
    let mut out = match audience {
        Audience::TheSameNode => String::from(
            "A human reviewed your work and left comments. Address every one of them in \
             the worktree you already have. Do not commit: your changes are committed for \
             you, as before, when you finish.\n\n",
        ),
        Audience::Whoever { .. } => {
            let mut opening = String::from(
                "A human reviewed work that has already been committed and left the \
                 comments below. Address every one of them",
            );
            // Without the branch an agent cannot find the code the comments are about,
            // and would reimplement it from the description instead.
            if let Some(branch) = branch {
                let _ = write!(opening, ", starting from the branch `{branch}`");
            }
            opening.push_str(".\n\n");
            opening
        }
    };
    for comment in comments {
        match (&comment.file_path, comment.line_start, comment.line_end) {
            (Some(path), Some(start), Some(end)) if end > start => {
                let _ = writeln!(out, "{path}:{start}-{end}");
            }
            (Some(path), Some(start), _) => {
                let _ = writeln!(out, "{path}:{start}");
            }
            (Some(path), None, _) => {
                let _ = writeln!(out, "{path}");
            }
            _ => out.push_str("On the change as a whole\n"),
        }
        let _ = writeln!(out, "  {}\n", comment.body.trim());
    }
    out
}

/// What a submit needs to know before it can talk to anybody.
///
/// Gathered in one synchronous window so the caller can drop its database lock before the
/// awaits below. A future holding a `Store` is neither `Send` nor spawnable, which rules
/// it out of both an axum handler and a background task.
#[derive(Debug)]
pub struct Pending {
    pub review: Review,
    pub open: Vec<ReviewComment>,
    pub message: String,
}

/// Read what submitting this review would act on, without acting on it.
pub fn pending(store: &Store, review_id: i64) -> Result<Pending> {
    let review = store.review(review_id)?;
    if review.submitted_at.is_some() {
        return Err(Error::invalid("that review has already been submitted"));
    }
    let open: Vec<ReviewComment> = store
        .comments(review_id)?
        .into_iter()
        .filter(|comment| comment.status == CommentStatus::Open)
        .collect();
    let message = as_instructions(&open);
    Ok(Pending {
        review,
        open,
        message,
    })
}

/// The seat that wrote this code, if it is still at work.
///
/// No round trip any more, and none needed. On eve this had to ask a live process, because
/// a message that missed one vanished. A correction is queued now (D21), so the question
/// is only whether that seat has a next turn to be given it - and a finished node has
/// none, which is when a review becomes a slice instead.
fn still_working(node: Option<crate::model::NodeRun>) -> Option<(i64, i64)> {
    let node = node?;
    if !matches!(
        node.status,
        crate::model::NodeStatus::Running | crate::model::NodeStatus::Parked
    ) {
        return None;
    }
    node.session_id.as_ref()?;
    Some((node.id, node.agent_id?))
}

/// The node a review would steer, read in one synchronous window.
pub fn responsible(store: &Store, review: &Review) -> Option<crate::model::NodeRun> {
    store.node_run(review.node_run_id?).ok()
}

/// A pull request whose comments can be worked on where it was built.
#[derive(Debug, Clone)]
pub struct FollowUp {
    /// The run that built it. A follow-up is a run of its own, rooted where this one was.
    pub run_id: i64,
    /// The seat's last turn on it: whose work the comments are about, and whose
    /// conversation the follow-up continues.
    pub node: crate::model::NodeRun,
    pub slice_key: String,
    /// The plan the slice is on, from the run that built it - never re-resolved from a
    /// checkout, which may have moved on to another plan since.
    pub plan: String,
    pub worktree: std::path::PathBuf,
    pub branch: String,
}

/// Where a review's comments could be worked on in place, when its seat has finished.
///
/// Read from the database in one synchronous window; whether the worktree is still on that
/// branch is the caller's to check before acting (`follow_up_ready`), because it asks git.
pub fn follow_up_target(store: &Store, review: &Review) -> Option<FollowUp> {
    let node = responsible(store, review)?;
    if still_working(Some(node.clone())).is_some() || node.status != crate::NodeStatus::Done {
        return None;
    }
    Some(FollowUp {
        run_id: node.run_id,
        plan: store.run(node.run_id).ok()?.plan_slug?,
        slice_key: node.slice_key.clone()?,
        worktree: node
            .worktree_path
            .as_deref()
            .map(std::path::PathBuf::from)?,
        branch: node.branch.clone()?,
        node,
    })
}

/// Whether a follow-up can work in that worktree now: it still exists, and it is still on
/// the pull request's branch. A worktree that has moved on - a one-PR plan's checkout
/// started on the next plan, a lease returned after merge - is not the PR's any more.
pub async fn follow_up_ready(target: &FollowUp) -> bool {
    target.worktree.is_dir()
        && git::current_branch(&target.worktree).await.as_deref() == Some(target.branch.as_str())
}

/// The orchestrator's current node on this review's run, if it has one.
///
/// Scoped to the run rather than to the project: an orchestrator mid-turn on *this* work is
/// the one whose decisions the correction changes.
pub fn conductor(store: &Store, review: &Review) -> Option<crate::model::NodeRun> {
    let run_id = review.run_id?;
    store
        .node_runs(run_id)
        .ok()?
        .into_iter()
        .rev()
        .find(|node| node.role == crate::ROOT_ROLE)
}

/// Whether submitting would reach the agent or write a slice.
///
/// Worth asking before the human decides what to say, because the two are different
/// kinds of feedback: one lands in a worktree that already exists, the other is a note
/// for somebody starting fresh.
pub fn steerable(node: Option<crate::model::NodeRun>) -> bool {
    still_working(node).is_some()
}

/// Deliver a submitted review.
///
/// Takes no `Store`: the caller reads with [`pending`] and [`responsible`], calls this,
/// then records the result. That split is what lets an HTTP handler hold its lock only
/// while it is actually using it.
///
/// Getting the quiet direction wrong is the bad failure - a review delivered into a dead
/// process disappears while the human believes it landed - so the liveness check is a
/// real request, and anything short of a healthy answer falls through to the plan.
pub async fn deliver<F, Fut, A, AFut, Q>(
    pending: &Pending,
    node: Option<crate::model::NodeRun>,
    // The orchestrator's current node, when it has one. A correction reaches it too: it
    // decides what the work *is*, and feedback that only reaches the author leaves the plan
    // still saying the old thing.
    orchestrator: Option<crate::model::NodeRun>,
    plan_slice: F,
    amend_slice: A,
    // Queueing a message for a seat, as a callback for the same reason the other two are:
    // this function awaits, and a `rusqlite::Connection` is not `Sync`, so a caller that
    // handed over its store would be holding a lock across an await.
    mut queue: Q,
) -> Result<Submitted>
where
    F: FnOnce(String, String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
    A: FnOnce(String, String) -> AFut,
    AFut: std::future::Future<Output = Result<()>>,
    Q: FnMut(i64, i64, &str) -> Result<()>,
{
    // Approving with nothing outstanding is the one case with nobody to tell.
    if pending.open.is_empty() {
        return Ok(Submitted::Accepted);
    }

    let node_run_id = node.as_ref().map(|node| node.id);
    let slice_key = node.as_ref().and_then(|node| node.slice_key.clone());
    if let Some((node_id, agent_id)) = still_working(node) {
        // The slice first. A review is a change to *what the work is*, and the verifier
        // checks the commit against the slice spec - so feedback that only reaches the
        // agent produces work that is correct and then rejected for not matching a spec
        // nobody updated. Amending before messaging also means a crash between the two
        // leaves the plan right and the agent merely uninformed, which is the recoverable
        // order.
        if let Some(key) = slice_key {
            amend_slice(key, amendment(&pending.open)).await?;
        }
        queue(node_id, agent_id, &pending.message)?;

        // And the orchestrator, when it is still working. Best effort on purpose: it
        // always learns through the amended slice, so failing to reach it costs immediacy
        // rather than the correction, and an unreachable orchestrator must not make a
        // delivered review look failed.
        let told_orchestrator = match still_working(orchestrator) {
            Some((orchestrator_node, orchestrator_agent)) => queue(
                orchestrator_node,
                orchestrator_agent,
                &for_orchestrator(&pending.open, &pending.review),
            )
            .is_ok(),
            None => false,
        };

        return Ok(Submitted::Steered {
            node_run_id: node_run_id.unwrap_or(node_id),
            comments: pending.open.len(),
            told_orchestrator,
        });
    }

    let title = format!("Review feedback: {}", pending.review.title);
    // Re-addressed: the message gathered for the live node tells it to use a worktree
    // that whoever picks this slice up will not have.
    let scope = instructions_for(
        &pending.open,
        Audience::Whoever { branch: None },
        pending.review.branch.as_deref(),
    );
    let slice_key = plan_slice(title, scope).await?;
    Ok(Submitted::Planned {
        slice_key,
        comments: pending.open.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CommentStatus, DiffSide};

    fn comment(
        path: Option<&str>,
        start: Option<i64>,
        end: Option<i64>,
        body: &str,
    ) -> ReviewComment {
        ReviewComment {
            id: 1,
            review_id: 1,
            parent_id: None,
            file_path: path.map(ToString::to_string),
            side: Some(DiffSide::New),
            line_start: start,
            line_end: end,
            author: "human".into(),
            body: body.into(),
            status: CommentStatus::Open,
            created_at: String::new(),
            resolved_at: None,
        }
    }

    #[test]
    fn a_comment_says_where_before_it_says_what() {
        // An agent that cannot find the line cannot act on the note.
        let text = as_instructions(&[comment(Some("src/lib.rs"), Some(42), Some(42), "name this")]);
        let where_at = text.find("src/lib.rs:42").unwrap();
        let what_at = text.find("name this").unwrap();
        assert!(where_at < what_at);
    }

    #[test]
    fn a_range_comment_names_the_range_not_just_its_first_line() {
        let text = as_instructions(&[comment(Some("a.rs"), Some(10), Some(14), "extract this")]);
        assert!(text.contains("a.rs:10-14"), "{text}");
    }

    #[test]
    fn a_single_line_range_is_written_as_one_line() {
        let text = as_instructions(&[comment(Some("a.rs"), Some(10), Some(10), "x")]);
        assert!(text.contains("a.rs:10\n"), "{text}");
        assert!(!text.contains("10-10"));
    }

    #[test]
    fn a_comment_on_no_particular_line_still_reaches_the_agent() {
        // A review of the change as a whole is the most common kind, and dropping it
        // because it has no anchor would lose the most important note.
        let text = as_instructions(&[comment(None, None, None, "this needs a test")]);
        assert!(text.contains("On the change as a whole"));
        assert!(text.contains("this needs a test"));
    }

    #[test]
    fn a_fresh_agent_is_not_told_to_use_a_worktree_it_does_not_have() {
        // The node that wrote the code still has its worktree. Whoever picks the slice
        // up tomorrow has neither that nor the context, and "the worktree you already
        // have" is an instruction it cannot follow.
        let comments = [comment(Some("a.rs"), Some(1), Some(1), "rename this")];
        let live = instructions_for(&comments, Audience::TheSameNode, None);
        assert!(live.contains("worktree you already have"));

        let fresh = instructions_for(
            &comments,
            Audience::Whoever { branch: None },
            Some("ai-team/s1"),
        );
        assert!(!fresh.contains("worktree you already have"), "{fresh}");
        // And it is told where the code is, or it will reimplement it from the comments.
        assert!(fresh.contains("ai-team/s1"), "{fresh}");
    }

    #[test]
    fn every_comment_makes_it_into_the_message() {
        let text = as_instructions(&[
            comment(Some("a.rs"), Some(1), Some(1), "first"),
            comment(Some("b.rs"), Some(2), Some(2), "second"),
            comment(None, None, None, "third"),
        ]);
        for body in ["first", "second", "third"] {
            assert!(text.contains(body), "{body} missing from {text}");
        }
    }

    #[test]
    fn a_finished_seats_review_follows_up_where_its_pr_was_built() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "build PR1", crate::RunTrigger::Manual)
            .unwrap();
        store.set_run_plan(run.id, "widget-plan").unwrap();
        let backend = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap();
        let node = store
            .dispatch_task(
                run.id,
                backend.id,
                "PR1",
                Some("T2"),
                &crate::machine::ModelRegistry::local_only(),
            )
            .unwrap();
        store
            .attach_worktree(node.id, "/work/pr1", Some("widget-plan/pr1"), None)
            .unwrap();
        let review = store
            .open_review(
                project.id,
                "PR1: a range",
                Some(run.id),
                Some(node.id),
                Some("widget-plan/pr1"),
            )
            .unwrap();

        // Still working: its session takes the comments directly, not a follow-up.
        store
            .set_node_status(node.id, crate::NodeStatus::Running)
            .unwrap();
        assert!(follow_up_target(&store, &review).is_none());

        store
            .set_node_status(node.id, crate::NodeStatus::Done)
            .unwrap();
        let target = follow_up_target(&store, &review).unwrap();
        assert_eq!(target.slice_key, "PR1");
        assert_eq!(target.plan, "widget-plan");
        assert_eq!(target.branch, "widget-plan/pr1");
        assert_eq!(target.worktree, std::path::Path::new("/work/pr1"));
        assert_eq!(target.node.task_key.as_deref(), Some("T2"));
    }
}
