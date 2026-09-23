//! Building one pull request, task by task, in one checkout (PW4, PW6).
//!
//! A PR's tasks are built in the order the plan gives them, each by the seat that owns
//! it, one writer at a time. They share one index, one set of lockfiles and one set of
//! gates, and a second writer would be building against a tree changing under it -
//! parallel work comes from sibling PRs, never from seats sharing a checkout. Every
//! finished turn is committed as it ends, so the next task starts from a known state and
//! each commit says which task it was.
//!
//! The PR is checked once it is whole (PW7). Halfway through, the gates can fail for no
//! reason but that the screen has not caught up with the server yet; judging each task
//! alone would reject work that is merely unfinished. A rejection goes back to the seat
//! that can fix it, in the conversation that produced the work.
//!
//! Acceptance is a fact about the PR, not about any one turn in it. So a seat's row stays
//! running until the PR is judged, and the answer settles every row at once.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::orchestrate::{
    record_quota_and_resolve, take_fresh_turn, take_replied_turn, take_turn, Dispatched, FreshTurn,
    QuotaFailover, Rig,
};
use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::model::{EventKind, NewEvent, NodeRun, NodeStatus};
use crate::neighbours::{git, Lease, Planner, Slice};
use crate::store::Store;
use crate::supervise::outcome::{outcome_status, TurnOutcome};
use crate::tasks::Task;

/// One task of a PR, and the seat that builds it.
#[derive(Debug, Clone)]
pub(super) struct Assignment {
    /// `None` for a slice with no task lines: one piece of work, built by the seat whose
    /// zone owns its paths - which is what every slice was before tasks.
    pub(super) task: Option<Task>,
    pub(super) agent_id: i64,
    pub(super) role: String,
}

impl Assignment {
    fn key(&self) -> Option<&str> {
        self.task.as_ref().map(|task| task.key.as_str())
    }

    /// `PR1 T2`, or `PR1` for a slice built whole.
    fn label(&self, slice: &Slice) -> String {
        match self.key() {
            Some(key) => format!("{} {key}", slice.key),
            None => slice.key.clone(),
        }
    }

    fn title<'a>(&'a self, slice: &'a Slice) -> &'a str {
        self.task
            .as_ref()
            .map_or(slice.title.as_str(), |task| task.title.as_str())
    }

    /// What the task touches, which is also what narrows the house rules it is given.
    fn touches(&self, slice: &Slice) -> Vec<String> {
        self.task
            .as_ref()
            .map_or_else(|| slice.touches(), |task| task.touches.clone())
    }
}

/// The seat that checks a PR, when the team has one.
#[derive(Debug, Clone, Copy)]
pub(super) struct VerifierSeat {
    pub(super) agent_id: i64,
}

/// Everything one PR needs, owned outright so it can move to its own task.
pub(super) struct PrTask {
    pub(super) db_path: PathBuf,
    pub(super) rig: Rig,
    pub(super) registry: ModelRegistry,
    pub(super) slice: Slice,
    pub(super) crew: Vec<Assignment>,
    pub(super) branch: String,
    /// The commit the PR is built on. Every task is committed as it finishes, so this -
    /// not `HEAD` - is what the whole PR is measured against.
    pub(super) base: String,
    pub(super) worktree: PathBuf,
    /// The PR's own leased worktree, or `None` when a one-PR plan builds in the run's
    /// checkout (PW3).
    pub(super) lease: Option<Lease>,
    pub(super) planner: Planner,
    pub(super) run_id: i64,
    pub(super) max_repairs: i64,
    pub(super) verifier: Option<VerifierSeat>,
    /// Where the build starts: its first task, an interrupted turn, or review comments.
    pub(super) start: Start,
}

/// Where building a PR starts.
pub(super) enum Start {
    /// Its first task.
    Fresh,
    /// The turn an interrupted process left behind, which is finished first.
    Resume(NodeRun),
    /// Review comments on a PR already built: the seat whose work they are about takes
    /// them, in its own conversation, and the PR is checked whole again.
    FollowUp { node: NodeRun, comments: String },
}

/// What building one PR needs, borrowed for as long as it takes.
struct Pr<'a> {
    rig: &'a Rig,
    registry: &'a ModelRegistry,
    run_id: i64,
    slice: &'a Slice,
    crew: &'a [Assignment],
    branch: &'a str,
    base: &'a str,
    worktree: &'a Path,
    max_repairs: i64,
    verifier: Option<&'a VerifierSeat>,
}

/// The rows a PR has opened.
#[derive(Default)]
struct Rows {
    /// Every maker row, in order, settled together when the PR is judged.
    all: Vec<i64>,
    /// Each seat's latest row: the one its conversation continues on.
    latest: HashMap<i64, NodeRun>,
}

impl Rows {
    fn opened(&mut self, agent_id: i64, node: &NodeRun) {
        self.all.push(node.id);
        self.latest.insert(agent_id, node.clone());
    }

    /// The rows an interrupted PR had already opened, from the store.
    fn recover(store: &Store, pr: &Pr<'_>) -> Result<Rows> {
        let mut rows = Rows::default();
        for node in store.node_runs(pr.run_id)? {
            if node.slice_key.as_deref() != Some(pr.slice.key.as_str()) {
                continue;
            }
            if let Some(agent_id) = node.agent_id {
                rows.opened(agent_id, &node);
            }
        }
        Ok(rows)
    }
}

/// Build one PR to an answer, on its own connection, and hand its checkout back.
///
/// Its own `Store`: SQLite is in WAL with a busy timeout, so parallel PRs write
/// concurrently, and sharing one `&mut Store` across tasks would serialise them anyway.
pub(super) async fn run_pr(task: PrTask) -> Result<Dispatched> {
    let PrTask {
        db_path,
        rig,
        registry,
        slice,
        crew,
        branch,
        base,
        worktree,
        lease,
        planner,
        run_id,
        max_repairs,
        verifier,
        start,
    } = task;
    let mut store = Store::open(&db_path)?;
    let pr = Pr {
        rig: &rig,
        registry: &registry,
        run_id,
        slice: &slice,
        crew: &crew,
        branch: &branch,
        base: &base,
        worktree: &worktree,
        max_repairs,
        verifier: verifier.as_ref(),
    };
    let resuming = matches!(start, Start::Resume(_));
    let mut rows = Rows::default();
    let attempted = match build_pr(&mut store, &pr, &mut rows, start).await {
        Ok(attempted) => attempted,
        Err(error) if resuming => {
            // Recovery must be retryable too. Keep the Pi sessions and the dirty lease;
            // returning it would make awt clean away the very work being recovered.
            let pid = i64::from(std::process::id());
            for row in &rows.all {
                let _ = store.release_node_supervision(*row, pid);
            }
            if let Some(lease) = lease {
                lease.preserve();
            }
            return Err(error);
        }
        Err(error) => {
            return Err(release_failed(&planner, &slice.key, &worktree, lease, error).await);
        }
    };
    finish_pr(
        &mut store,
        Completion {
            db_path,
            planner,
            slice,
            branch,
            worktree,
            lease,
            run_id,
        },
        &rows,
        attempted,
    )
    .await
}

/// How a PR's build ended, on the row that ended it.
struct Attempted {
    outcome: TurnOutcome,
    /// Why it was still rejected when it stopped, if it was.
    rejection: Option<String>,
    /// How many repairs it took.
    repairs: i64,
    node: NodeRun,
    /// True when it stopped because it ran out of budget or repairs rather than because
    /// it finished. Decides how the rejection reads.
    exhausted: bool,
}

impl Attempted {
    fn settled(outcome: TurnOutcome, node: &NodeRun, repairs: i64) -> Attempted {
        Attempted {
            outcome,
            rejection: None,
            repairs,
            node: node.clone(),
            exhausted: false,
        }
    }

    /// Stopped short of the check: a failed model turn, or a turn that changed nothing.
    /// Gates and the verifier are not spent on work that never happened.
    fn failed(outcome: TurnOutcome, node: &NodeRun, repairs: i64, reason: String) -> Attempted {
        Attempted {
            outcome,
            rejection: Some(reason),
            repairs,
            node: node.clone(),
            exhausted: false,
        }
    }

    /// Out of repairs, out of budget, or refused before it started.
    fn stopped(outcome: TurnOutcome, node: &NodeRun, repairs: i64, reason: String) -> Attempted {
        Attempted {
            outcome,
            rejection: Some(reason),
            repairs,
            node: node.clone(),
            exhausted: true,
        }
    }
}

/// Build every task in order, then check the whole PR, repairing until it passes or the
/// run's repair budget is spent.
async fn build_pr(
    store: &mut Store,
    pr: &Pr<'_>,
    rows: &mut Rows,
    start: Start,
) -> Result<Attempted> {
    let (next, repairs, mut last) = match start {
        Start::Fresh => (0, 0, None),
        Start::Resume(node) => match pick_up(store, pr, rows, node).await? {
            PickedUp::At {
                next,
                repairs,
                last,
            } => (next, repairs, Some(*last)),
            PickedUp::Stopped(stopped) => return Ok(*stopped),
        },
        Start::FollowUp { node, comments } => {
            match follow_up(store, pr, rows, node, &comments).await? {
                Turned::Finished(finished) => (pr.crew.len(), 0, Some(*finished)),
                Turned::Stopped(stopped) => return Ok(*stopped),
            }
        }
    };

    for assignment in &pr.crew[next..] {
        let house = crate::house::read_for(pr.worktree, &assignment.touches(pr.slice));
        let instruction = task_prompt(pr.slice, pr.crew, assignment, &house);
        let subject = commit_subject(pr.slice, assignment);
        let before = git::snapshot(pr.worktree).await?;
        let (node, turn) = match take_task_turn(
            store,
            pr,
            rows,
            assignment.agent_id,
            assignment.key(),
            &assignment.label(pr.slice),
            &instruction,
        )
        .await?
        {
            Turned::Finished(finished) => *finished,
            Turned::Stopped(stopped) => return Ok(*stopped),
        };
        if keep(store, pr, &node, &before, &subject).await?.is_none() {
            // A task that finished without changing a file did not build anything.
            // Calling that done puts a green row on the board for work nobody did.
            return Ok(unchanged(turn, &node, repairs, &subject));
        }
        last = Some((node, turn));
    }

    let last =
        last.ok_or_else(|| Error::invalid(format!("{} has nothing to build", pr.slice.key)))?;
    judge(store, pr, rows, last, repairs).await
}

/// Work review comments into a PR that is already built, before it is checked again.
///
/// The seat whose work the comments are about takes them. Its last row is from the run
/// that built the PR, so it is where this seat's conversation continues from - but it is
/// that run's row, settled by that run, and never this one's to settle again.
async fn follow_up(
    store: &mut Store,
    pr: &Pr<'_>,
    rows: &mut Rows,
    node: NodeRun,
    comments: &str,
) -> Result<Turned> {
    let agent_id = node
        .agent_id
        .ok_or_else(|| Error::invalid("the seat that built this no longer exists"))?;
    let task_key = node.task_key.clone();
    rows.latest.insert(agent_id, node);
    let label = match task_key.as_deref() {
        Some(task) => format!("{} {task}", pr.slice.key),
        None => pr.slice.key.clone(),
    };
    let before = git::snapshot(pr.worktree).await?;
    let turned = take_task_turn(
        store,
        pr,
        rows,
        agent_id,
        task_key.as_deref(),
        &format!("{label} review"),
        comments,
    )
    .await?;
    if let Turned::Finished(finished) = &turned {
        // Nothing changed is still an answer worth checking: the comments may have asked
        // for nothing but an explanation, and the check says whether the PR stands.
        keep(
            store,
            pr,
            &finished.0,
            &before,
            &format!("{}: address review", pr.slice.key),
        )
        .await?;
    }
    Ok(turned)
}

enum PickedUp {
    /// Carry on from task `next`, the interrupted turn now finished and kept.
    At {
        next: usize,
        repairs: i64,
        last: Box<(NodeRun, TurnOutcome)>,
    },
    Stopped(Box<Attempted>),
}

/// Finish the turn an interrupted process left behind, and say where the PR goes next.
async fn pick_up(
    store: &mut Store,
    pr: &Pr<'_>,
    rows: &mut Rows,
    node: NodeRun,
) -> Result<PickedUp> {
    *rows = Rows::recover(store, pr)?;
    let place = resume_place(store, pr, rows, &node)?;
    let agent_id = node
        .agent_id
        .ok_or_else(|| Error::invalid("the interrupted node's seat no longer exists"))?;
    // No snapshot survives an interrupted process, so the interrupted turn keeps whatever
    // the checkout holds that is not committed.
    let before = git::Snapshot::default();
    let turn = take_replied_turn(store, pr.rig, agent_id, node.id, pr.worktree).await?;
    if outcome_status(&turn) != NodeStatus::Done {
        return Ok(PickedUp::Stopped(Box::new(failed_turn(
            turn,
            &node,
            place.repairs,
        ))));
    }
    let subject = match place.task {
        Some(index) => commit_subject(pr.slice, &pr.crew[index]),
        None => format!("{}: repair {}", pr.slice.key, place.repairs),
    };
    let kept = keep(store, pr, &node, &before, &subject).await?;
    if kept.is_none() && place.task.is_some() {
        return Ok(PickedUp::Stopped(Box::new(unchanged(
            turn,
            &node,
            place.repairs,
            &subject,
        ))));
    }
    Ok(PickedUp::At {
        next: place.task.map_or(pr.crew.len(), |index| index + 1),
        repairs: place.repairs,
        last: Box::new((node, turn)),
    })
}

/// Check the whole PR, and repair it until it passes or the repair budget is spent.
async fn judge(
    store: &mut Store,
    pr: &Pr<'_>,
    rows: &mut Rows,
    last: (NodeRun, TurnOutcome),
    mut repairs: i64,
) -> Result<Attempted> {
    let (mut node, mut outcome) = last;
    loop {
        match deliver_replies(store, pr, rows).await? {
            Replies::Stopped(stopped) => return Ok(*stopped),
            Replies::None | Replies::Delivered => {}
        }
        let (verdict, hint) = check(store, node.id, pr).await?;
        // Checks take minutes. A reply written during them belongs to the conversation,
        // and has to be acted on before the work it would change is accepted.
        match deliver_replies(store, pr, rows).await? {
            Replies::Stopped(stopped) => return Ok(*stopped),
            Replies::Delivered => continue,
            Replies::None => {}
        }
        let reason = match verdict {
            Verdict::Accepted => return Ok(Attempted::settled(outcome, &node, repairs)),
            Verdict::Unavailable(reason) => {
                return Ok(Attempted::failed(outcome, &node, repairs, reason));
            }
            Verdict::Rejected(reason) => reason,
        };
        // The rejection lands on the work that has to change: the owner's latest row, not
        // whichever seat happened to finish last. That row is the attempt that was
        // rejected, and it is what analytics holds against that seat.
        let owner = repair_owner(store, pr.crew, hint.as_ref())?;
        let faulted = rows
            .latest
            .get(&owner.agent_id)
            .cloned()
            .unwrap_or_else(|| node.clone());
        record_rejection(store, pr, &faulted, repairs, &reason)?;
        if let Some(exceeded) = crate::guardrails::node_may_continue(store, faulted.id)? {
            return Ok(Attempted::stopped(
                outcome,
                &faulted,
                repairs,
                format!("{}; last rejection: {reason}", exceeded.reason),
            ));
        }
        if repairs >= pr.max_repairs {
            return Ok(Attempted::stopped(outcome, &faulted, repairs, reason));
        }

        repairs += 1;
        let subject = format!("{}: repair {repairs}", pr.slice.key);
        let before = git::snapshot(pr.worktree).await?;
        match take_task_turn(
            store,
            pr,
            rows,
            owner.agent_id,
            owner.key(),
            &format!("{} repair", pr.slice.key),
            &repair_prompt(pr.slice, owner, &reason),
        )
        .await?
        {
            Turned::Finished(finished) => {
                let (repaired, turn) = *finished;
                // A repair that changed nothing is still checked again: the rejection
                // may have been wrong, and the check says so either way.
                keep(store, pr, &repaired, &before, &subject).await?;
                node = repaired;
                outcome = turn;
            }
            Turned::Stopped(stopped) => return Ok(*stopped),
        }
    }
}

/// Where an interrupted PR picks up.
struct Place {
    /// The index of the task the interrupted turn was building, or `None` when every
    /// task was already built and the turn was a repair.
    task: Option<usize>,
    repairs: i64,
}

fn resume_place(store: &Store, pr: &Pr<'_>, rows: &Rows, node: &NodeRun) -> Result<Place> {
    let mut built = HashSet::new();
    let mut repairs = 0;
    for row in &rows.all {
        if *row == node.id {
            continue;
        }
        for event in store.node_events_marked(*row, "built")? {
            built.insert(
                event
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("task"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            );
        }
        repairs +=
            i64::try_from(store.node_events_marked(*row, "rejection")?.len()).unwrap_or_default();
    }
    let every_task_built = pr
        .crew
        .iter()
        .all(|assignment| built.contains(&assignment.key().map(str::to_string)));
    let task = if every_task_built {
        None
    } else {
        let index = pr
            .crew
            .iter()
            .position(|assignment| assignment.key() == node.task_key.as_deref())
            .ok_or_else(|| {
                Error::invalid("the interrupted turn is not one of this PR's tasks any more")
            })?;
        Some(index)
    };
    Ok(Place { task, repairs })
}

enum Turned {
    Finished(Box<(NodeRun, TurnOutcome)>),
    Stopped(Box<Attempted>),
}

/// One seat's turn at a task or a repair, on a row of its own.
///
/// Moves across subscription accounts only when Pi reports a recognised quota boundary,
/// and delivers anything said to the seat while it worked before calling the turn over.
async fn take_task_turn(
    store: &mut Store,
    pr: &Pr<'_>,
    rows: &mut Rows,
    agent_id: i64,
    task_key: Option<&str>,
    subject: &str,
    instruction: &str,
) -> Result<Turned> {
    let mut exhausted_providers = Vec::new();
    let mut resolution: Option<crate::ModelResolution> = None;
    loop {
        let node = open_row(store, pr, rows, agent_id, task_key, resolution.as_ref())?;
        if let Some(exceeded) = crate::guardrails::run_may_continue(store, pr.run_id)? {
            return Ok(Turned::Stopped(Box::new(Attempted::stopped(
                TurnOutcome::default(),
                &node,
                0,
                exceeded.reason,
            ))));
        }
        store.set_node_status(node.id, NodeStatus::Running)?;
        let mut turn =
            match take_turn(store, pr.rig, agent_id, node.id, pr.worktree, instruction).await {
                Ok(turn) => turn,
                Err(error) => {
                    store.set_node_status(node.id, NodeStatus::Failed)?;
                    return Err(error);
                }
            };
        if let Some(quota) = crate::supervise::quota_exhaustion(&turn) {
            resolution = record_quota_and_resolve(
                store,
                &mut exhausted_providers,
                QuotaFailover::new(pr.registry, pr.run_id, agent_id, subject, &node, &quota),
            )?;
            if resolution.is_some() {
                // Repeat the instruction on the next account, without spending a repair.
                continue;
            }
            let reason = format!(
                "{subject} cannot continue: every reachable allowed model subscription is \
                 unavailable; last response: {quota}"
            );
            return Ok(Turned::Stopped(Box::new(Attempted::failed(
                turn, &node, 0, reason,
            ))));
        }
        if outcome_status(&turn) != NodeStatus::Done {
            return Ok(Turned::Stopped(Box::new(failed_turn(turn, &node, 0))));
        }
        while store.waiting_for_node(agent_id, node.id)? > 0 {
            turn = take_replied_turn(store, pr.rig, agent_id, node.id, pr.worktree).await?;
            if outcome_status(&turn) != NodeStatus::Done {
                return Ok(Turned::Stopped(Box::new(failed_turn(turn, &node, 0))));
            }
        }
        return Ok(Turned::Finished(Box::new((node, turn))));
    }
}

/// Open a row for one turn, continuing the seat's conversation in this PR.
///
/// A fresh row per turn, never an edit (D2). The seat's session comes with it, and so do
/// replies still waiting on its previous row - both belong to the conversation, and the
/// conversation has moved. Not across providers: another account's model starts fresh,
/// and the prompt is written to stand on its own for exactly that case.
fn open_row(
    store: &mut Store,
    pr: &Pr<'_>,
    rows: &mut Rows,
    agent_id: i64,
    task_key: Option<&str>,
    resolution: Option<&crate::ModelResolution>,
) -> Result<NodeRun> {
    let node = match resolution {
        Some(resolution) => store.dispatch_with_resolution(
            pr.run_id,
            agent_id,
            Some(&pr.slice.key),
            task_key,
            resolution,
        )?,
        None => store.dispatch_task(pr.run_id, agent_id, &pr.slice.key, task_key, pr.registry)?,
    };
    store.attach_worktree(
        node.id,
        &pr.worktree.to_string_lossy(),
        Some(pr.branch),
        None,
    )?;
    if let Some(previous) = rows.latest.get(&agent_id) {
        if previous.provider == node.provider && previous.model == node.model {
            store.continue_session(node.id, previous.id)?;
        }
        store.retarget_pending(agent_id, previous.id, node.id)?;
    }
    let node = store.node_run(node.id)?;
    rows.opened(agent_id, &node);
    Ok(node)
}

fn failed_turn(turn: TurnOutcome, node: &NodeRun, repairs: i64) -> Attempted {
    let reason = crate::supervise::outcome::provider_diagnostic(&turn)
        .unwrap_or_else(|| "the model turn failed before completing".to_string());
    Attempted::failed(turn, node, repairs, reason)
}

fn unchanged(turn: TurnOutcome, node: &NodeRun, repairs: i64, subject: &str) -> Attempted {
    let what = subject.split(':').next().unwrap_or(subject);
    Attempted::failed(
        turn,
        node,
        repairs,
        format!("{what} finished without changing a file"),
    )
}

enum Replies {
    None,
    Delivered,
    Stopped(Box<Attempted>),
}

/// Deliver what anyone said to one of this PR's seats while it waited.
///
/// Before the PR is judged, never after: a reply written while the verifier read the
/// diff still changes what the verifier should have read.
async fn deliver_replies(store: &mut Store, pr: &Pr<'_>, rows: &mut Rows) -> Result<Replies> {
    let mut delivered = false;
    let seats: Vec<(i64, NodeRun)> = rows
        .latest
        .iter()
        .map(|(agent, node)| (*agent, node.clone()))
        .collect();
    for (agent_id, node) in seats {
        if store.node_run(node.id)?.status != NodeStatus::Running
            || store.waiting_for_node(agent_id, node.id)? == 0
        {
            continue;
        }
        let before = git::snapshot(pr.worktree).await?;
        let turn = take_replied_turn(store, pr.rig, agent_id, node.id, pr.worktree).await?;
        if outcome_status(&turn) != NodeStatus::Done {
            return Ok(Replies::Stopped(Box::new(failed_turn(turn, &node, 0))));
        }
        let label = match node.task_key.as_deref() {
            Some(task) => format!("{} {task}", pr.slice.key),
            None => pr.slice.key.clone(),
        };
        keep(
            store,
            pr,
            &node,
            &before,
            &format!("{label}: follow up a reply"),
        )
        .await?;
        delivered = true;
    }
    Ok(if delivered {
        Replies::Delivered
    } else {
        Replies::None
    })
}

/// How long a commit subject may run before it is clipped: a one-line log stays readable.
const SUBJECT_LIMIT: usize = 72;

/// `PR1 T2: Show the range picker`, or `PR1: <slice title>` for a slice built whole.
///
/// A planner writes task titles for a person reading the plan, and some run long. Git's
/// subject line is clipped at a word, and the whole title goes in the body - so the log
/// reads cleanly and nothing the planner wrote is lost.
fn commit_subject(slice: &Slice, assignment: &Assignment) -> String {
    let label = assignment.label(slice);
    let title = assignment.title(slice).trim();
    let whole = format!("{label}: {title}");
    if whole.chars().count() <= SUBJECT_LIMIT {
        return whole;
    }
    let clipped: String = whole.chars().take(SUBJECT_LIMIT - 1).collect();
    let cut = clipped
        .rfind(' ')
        .filter(|at| *at > label.len() + 1)
        .unwrap_or(clipped.len());
    format!("{}…\n\n{title}", clipped[..cut].trim_end())
}

/// Commit what one turn changed, as that turn, and say so. `None` when it changed
/// nothing.
///
/// Committing is the control plane's job, not the seat's (rule 7): a model that forgets,
/// or commits half, loses work silently. Measured against the snapshot taken before the
/// turn rather than against `HEAD`, so what the gates left behind is not the seat's work.
async fn keep(
    store: &mut Store,
    pr: &Pr<'_>,
    node: &NodeRun,
    before: &git::Snapshot,
    subject: &str,
) -> Result<Option<String>> {
    let changes = git::changed_since(pr.worktree, before).await?;
    let sha = match git::commit_paths(pr.worktree, pr.branch, subject, &changes.paths).await? {
        Some(sha) => sha,
        // Told not to, but committed its work itself: still built, and still this turn's.
        None if changes.committed => git::rev_parse(pr.worktree, "HEAD").await?,
        None => return Ok(None),
    };
    let short = &sha[..sha.len().min(7)];
    let what = subject.split(':').next().unwrap_or(subject);
    store.append_event(
        pr.run_id,
        NewEvent::new(EventKind::Note, format!("built {what} as {short}"))
            .on_node(node.id)
            .by(&node.role)
            // Structured, so recovery can tell which tasks are already built without
            // reading English back out of a summary.
            .with(serde_json::json!({
                "built": sha,
                "task": node.task_key,
                "paths": changes.paths,
            })),
    )?;
    Ok(Some(sha))
}

/// Keep what a rejected attempt was told, on its own row.
///
/// Each rejected attempt carries its own verdict rather than the last one overwriting what
/// the earlier ones found.
fn record_rejection(
    store: &mut Store,
    pr: &Pr<'_>,
    node: &NodeRun,
    repairs: i64,
    reason: &str,
) -> Result<()> {
    store.append_event(
        pr.run_id,
        NewEvent::new(
            EventKind::Note,
            format!("{} rejected on attempt {}", pr.slice.key, repairs + 1),
        )
        .on_node(node.id)
        .by(&node.role)
        .with(serde_json::json!({ "reason": reason, "rejection": true })),
    )?;
    store.block_node(node.id, &truncate_reason(reason))?;
    store.set_node_status(node.id, NodeStatus::Failed)?;
    Ok(())
}

/// A rejection can be a whole build log; `blocked_reason` is a column somebody reads in
/// a table. The full evidence is on the event beside it.
pub(super) fn truncate_reason(reason: &str) -> String {
    let first = reason
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(reason);
    if first.chars().count() <= 200 {
        return first.to_string();
    }
    first.chars().take(199).collect::<String>() + "…"
}

/// What a rejection says about who has to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RepairHint {
    /// The manifest of the first gate that failed: the seat whose zone owns it.
    Manifest(String),
    /// The seat the verifier named.
    Role(String),
}

/// The seat a rejection goes back to.
///
/// A gate that failed names its own manifest, and the seat whose zone owns that manifest
/// is the one that can fix it - `ui/package.json` is the frontend's, `Cargo.toml` the
/// backend's. A verifier names a seat. Either way it must be one of this PR's seats, and
/// when nothing narrows it the last task's owner has the freshest view of the whole.
fn repair_owner<'a>(
    store: &Store,
    crew: &'a [Assignment],
    hint: Option<&RepairHint>,
) -> Result<&'a Assignment> {
    let last = crew
        .last()
        .ok_or_else(|| Error::invalid("a PR with no tasks has nobody to repair it"))?;
    let chosen = match hint {
        Some(RepairHint::Role(role)) => crew
            .iter()
            .rev()
            .find(|assignment| assignment.role.eq_ignore_ascii_case(role)),
        Some(RepairHint::Manifest(manifest)) => {
            let mut best: Option<(usize, &Assignment)> = None;
            for assignment in crew.iter().rev() {
                let zone = store.agent(assignment.agent_id)?.zone;
                if let Some(rank) = crate::util::zone_specificity(&zone, manifest) {
                    if best.is_none_or(|(seen, _)| rank > seen) {
                        best = Some((rank, assignment));
                    }
                }
            }
            best.map(|(_, assignment)| assignment)
        }
        None => None,
    };
    Ok(chosen.unwrap_or(last))
}

/// The answer to "is this PR actually done?".
enum Verdict {
    Accepted,
    Rejected(String),
    Unavailable(String),
}

/// Run the project's own gates over the whole PR, then let the verifier read it.
///
/// Gates first, because they are cheap, objective, and their output is the evidence the
/// verifier reasons from. A model asked to judge without them is guessing.
async fn check(
    store: &mut Store,
    node_run_id: i64,
    pr: &Pr<'_>,
) -> Result<(Verdict, Option<RepairHint>)> {
    let gates = crate::gates::discover_gates(pr.worktree);
    if gates.is_empty() {
        // No manifest, so nothing to run. Say so rather than treating silence as a pass:
        // the verifier still reads the diff, and the human should know what was skipped.
        store.append_event(
            pr.run_id,
            NewEvent::new(
                EventKind::Note,
                "no gates found in this worktree - nothing automatic was run",
            )
            .on_node(node_run_id),
        )?;
    }

    let results = crate::gates::run_gates(pr.worktree, &gates).await?;
    let evidence = crate::gates::evidence(&results);
    for result in &results {
        store.append_event(
            pr.run_id,
            NewEvent::new(
                if result.passed {
                    EventKind::Note
                } else {
                    EventKind::Failed
                },
                format!(
                    "gate {} `{}` {}",
                    result.gate.kind.as_str(),
                    result.gate.command(),
                    if result.passed { "passed" } else { "failed" }
                ),
            )
            .on_node(node_run_id)
            // Structured, not only prose. Analytics reads `passed` and `gate` from
            // here; a rate computed by matching English in `summary` breaks the first
            // time somebody improves the wording (rule 8).
            .with(serde_json::json!({
                "gate": result.gate.kind.as_str(),
                "command": result.gate.command(),
                "passed": result.passed,
                "output": result.output,
            })),
        )?;
    }

    if let Some(failed) = results.iter().find(|result| !result.passed) {
        // A failing gate is already a rejection with evidence attached. Asking a model to
        // confirm it would spend a turn to reach the same answer.
        return Ok((
            Verdict::Rejected(evidence),
            Some(RepairHint::Manifest(gate_manifest(&failed.gate))),
        ));
    }

    let Some(verifier) = pr.verifier else {
        // No verifier seat on this team. The gates are the whole check, and they passed.
        return Ok((Verdict::Accepted, None));
    };

    // Captured off the stream rather than read back out of the event table. A node's
    // Note events also carry ai-team's own bookkeeping - the model-fallback notice, for
    // one - and parsing that as the verifier's answer rejects work it never looked at.
    let mut said = String::new();
    let answered = take_fresh_turn(
        store,
        pr.rig,
        pr.run_id,
        verifier.agent_id,
        None,
        pr.worktree,
        &verify_prompt(pr.slice, pr.crew, pr.base, &evidence),
        |event| {
            if let Some(message) = event.assistant_message() {
                said = message;
            }
        },
    )
    .await;
    Ok(match answered {
        Ok(FreshTurn::Finished(_)) => {
            let hint = named_owner(&said).map(RepairHint::Role);
            (read_verdict(&said), hint)
        }
        Ok(FreshTurn::Unavailable { reason }) => (
            Verdict::Unavailable(format!("the verifier could not run because {reason}")),
            None,
        ),
        // A verifier that could not run has not approved anything.
        Err(error) => (
            Verdict::Rejected(format!("the verifier could not run: {error}")),
            None,
        ),
    })
}

/// The file a gate answers for: the manifest it was discovered from.
fn gate_manifest(gate: &crate::gates::Gate) -> String {
    let manifest = if gate.program == "cargo" {
        "Cargo.toml"
    } else {
        "package.json"
    };
    if gate.dir == "." {
        manifest.to_string()
    } else {
        format!("{}/{manifest}", gate.dir.trim_end_matches('/'))
    }
}

/// Read the verifier's answer.
///
/// Fails closed: anything that is not an explicit pass is a rejection. A verifier whose
/// reply could not be read has not approved the work, and defaulting the other way is
/// how an unchecked branch reaches somebody who trusted that this ran.
fn read_verdict(said: &str) -> Verdict {
    let lower = said.to_lowercase();
    if let Some(at) = lower.rfind("verdict:") {
        let tail = &lower[at + "verdict:".len()..];
        if tail.trim_start().starts_with("pass") {
            return Verdict::Accepted;
        }
    }
    Verdict::Rejected(if said.trim().is_empty() {
        "the verifier said nothing".to_string()
    } else {
        said.trim().to_string()
    })
}

/// The seat a rejecting verifier said has to change its work: the last `OWNER:` line.
fn named_owner(said: &str) -> Option<String> {
    // Markdown a model wraps a label in, and the space between it and the word.
    let markup = |c: char| c.is_whitespace() || "*`[]._".contains(c);
    said.lines().rev().find_map(|line| {
        let (label, role) = line.split_once(':')?;
        if !label.trim_matches(markup).eq_ignore_ascii_case("owner") {
            return None;
        }
        let role = role.trim_matches(markup).to_lowercase();
        (!role.is_empty() && !role.contains(char::is_whitespace)).then_some(role)
    })
}

fn verify_prompt(slice: &Slice, crew: &[Assignment], base: &str, evidence: &str) -> String {
    use std::fmt::Write as _;

    let mut prompt = format!(
        "Check the pull request in this worktree: {} - {}. `git diff {base}` shows \
         everything it changes, and `git log --oneline {base}..HEAD` the commit each \
         step made.\n",
        slice.key, slice.title
    );
    // What it was asked to do, whole: the story and its acceptance criteria are what
    // "done" is judged against, and review feedback is appended to the same scope. A
    // verifier shown less judged a reviewer's own request as scope creep, and the repair
    // it sent took the requested change back out.
    if let Some(scope) = slice
        .scope_md
        .as_deref()
        .filter(|scope| !scope.trim().is_empty())
    {
        let _ = write!(
            prompt,
            "\nWhat it was asked to do:\n{}\n\nWhere review feedback in it contradicts the \
             text before it, the review feedback is what was asked for.\n",
            scope.trim()
        );
    }
    if let Some(demo) = slice
        .demo_md
        .as_deref()
        .filter(|demo| !demo.trim().is_empty())
    {
        let _ = write!(prompt, "\nIt is done when this is true:\n{}\n", demo.trim());
    }
    if crew.iter().any(|assignment| assignment.task.is_some()) {
        prompt.push_str("\nIt was built as these tasks, in order:\n");
        for assignment in crew {
            let _ = writeln!(
                prompt,
                "- {} ({}): {}",
                assignment.key().unwrap_or(&slice.key),
                assignment.role,
                assignment.title(slice)
            );
        }
    }
    let _ = write!(
        prompt,
        "\nThe project's own gates have already been run:\n\n{evidence}\n\n\
         Decide whether this is actually done, and answer with `VERDICT: pass` or \
         `VERDICT: reject` on its own line. If you reject it, add `OWNER: <role>` on the \
         line after, naming the seat whose work has to change."
    );
    prompt
}

fn repair_prompt(slice: &Slice, owner: &Assignment, reason: &str) -> String {
    use std::fmt::Write as _;

    let mut prompt = format!(
        "Your work on {} was rejected when {} was checked as a whole. Fix it in this \
         worktree.\n\n{reason}\n\n\
         Address the specific problem above rather than starting over, and run the \
         project's own checks yourself before you finish. Do not commit: your changes are \
         committed for you when you are done.\n",
        owner.label(slice),
        slice.key
    );
    // For a seat on another account's model, which starts without the conversation.
    let _ = write!(
        prompt,
        "\nFor reference, the pull request is {} - {}",
        slice.key, slice.title
    );
    if let Some(scope) = slice
        .scope_md
        .as_deref()
        .filter(|scope| !scope.trim().is_empty())
    {
        let _ = write!(prompt, ":\n\n{}", scope.trim());
    }
    prompt.push('\n');
    prompt
}

/// What a seat is actually asked to do with its task.
fn task_prompt(
    slice: &Slice,
    crew: &[Assignment],
    assignment: &Assignment,
    house: &[crate::house::Rules],
) -> String {
    use std::fmt::Write as _;

    let mut prompt = match &assignment.task {
        Some(task) => format!(
            "Build {} of this pull request: {} - {}\n\nYour task is {}: {}\n",
            task.key, slice.key, slice.title, task.key, task.title
        ),
        None => format!("Build this pull request: {} - {}\n", slice.key, slice.title),
    };
    if let Some(scope) = &slice.scope_md {
        let _ = write!(prompt, "\nThe pull request:\n{}\n", scope.trim());
    }
    if let Some(demo) = &slice.demo_md {
        let _ = write!(prompt, "\nIt is done when this is true:\n{}\n", demo.trim());
    }
    if assignment.task.is_some() {
        prompt.push_str(
            "\nIt is built as these tasks, in order, each by its own seat, one at a time, \
             in this same checkout:\n",
        );
        let mine = crew
            .iter()
            .position(|other| other.key() == assignment.key())
            .unwrap_or_default();
        for (index, other) in crew.iter().enumerate() {
            let state = match index.cmp(&mine) {
                std::cmp::Ordering::Less => "built, and committed here",
                std::cmp::Ordering::Equal => "yours",
                std::cmp::Ordering::Greater => "later",
            };
            let _ = writeln!(
                prompt,
                "- {} [{}] {} - {state}",
                other.key().unwrap_or_default(),
                other.role,
                other.title(slice)
            );
        }
        prompt.push_str(
            "\nBuild only your task: the ones before it are already in this checkout, and \
             the ones after it belong to the seats named. A check that fails only because a \
             later task has not been built yet is expected - say which, and leave it.\n",
        );
    }
    prompt.push_str(
        "\nYou are in a worktree leased for this pull request. Work only inside it, run the \
         project's own checks before you call it done, and say so plainly if they do not \
         pass. Do not commit: your changes are committed for you, as this task, when you \
         finish.\n",
    );
    // Last, and after the worktree rules, so the closest thing to the model's final
    // attention is the standard its work will be judged against.
    prompt.push_str(&crate::house::section(house));
    prompt
}

struct Completion {
    db_path: PathBuf,
    planner: Planner,
    slice: Slice,
    branch: String,
    worktree: PathBuf,
    lease: Option<Lease>,
    run_id: i64,
}

/// Settle every row of the PR on its answer, tell the board and the operator, and give
/// the checkout back.
async fn finish_pr(
    store: &mut Store,
    completion: Completion,
    rows: &Rows,
    attempted: Attempted,
) -> Result<Dispatched> {
    let Completion {
        db_path,
        planner,
        slice,
        branch,
        worktree,
        lease,
        run_id,
    } = completion;
    let (status, fallout) = settle(store, run_id, &slice.key, rows, &attempted)?;
    let Attempted {
        outcome,
        rejection,
        node,
        ..
    } = attempted;

    let landed = if status == NodeStatus::Done {
        land(store, &planner, &slice, &node, &branch, &worktree).await?
    } else {
        None
    };

    update_board(
        &planner,
        &slice.key,
        &worktree,
        status,
        fallout,
        rejection.as_deref(),
    )
    .await;
    // Delivery and notification are deliberately best-effort. The node row and event
    // stream are the truth; neither remote system may turn accepted local work into
    // failed work.
    let _ = notify_node(store, node.id, &slice.key, &node.role, status);

    let dispatched = Dispatched {
        slice_key: slice.key,
        role: node.role,
        worktree,
        node_run_id: node.id,
        status,
        outcome,
        branch: landed,
    };
    release_and_deliver(lease, &db_path, &dispatched).await?;
    Ok(dispatched)
}

/// Give every row of the PR its answer.
///
/// The row that ended the build gets the team's failure policy applied; every other row
/// still waiting on the verdict gets the same status, because it is the same verdict. Done
/// before the lease goes back: a row left running lets the window accept a message after
/// the last delivery, when no supervisor remains to act on it.
fn settle(
    store: &mut Store,
    run_id: i64,
    slice_key: &str,
    rows: &Rows,
    attempted: &Attempted,
) -> Result<(NodeStatus, Option<crate::Fallout>)> {
    let (status, fallout) = settle_status(
        store,
        run_id,
        &attempted.node,
        slice_key,
        &attempted.outcome,
        attempted.rejection.as_deref(),
        attempted.repairs,
        attempted.exhausted,
    )?;
    for row in &rows.all {
        if *row != attempted.node.id && store.node_run(*row)?.status == NodeStatus::Running {
            if status != NodeStatus::Done {
                store.block_node(*row, &format!("{slice_key} stopped before it was accepted"))?;
            }
            store.set_node_status(*row, status)?;
        }
    }
    store.set_node_status(attempted.node.id, status)?;
    Ok((status, fallout))
}

/// Make an accepted PR reviewable.
///
/// Its commits are already on the branch - every turn was kept as it ended - so this is
/// the part that tells people: a review to comment on, and the board pointed at the
/// branch. A branch in the shared object store outlives the lease, which `awt return`
/// is about to clean.
async fn land(
    store: &mut Store,
    planner: &Planner,
    slice: &Slice,
    node: &NodeRun,
    branch: &str,
    worktree: &Path,
) -> Result<Option<String>> {
    let head = git::rev_parse(worktree, "HEAD").await?;
    let subject = format!("{}: {}", slice.key, slice.title.trim());
    // A review, so the work is *reviewable*. Opened here rather than when somebody asks,
    // because the point is to see a diff as the team works rather than to remember to ask
    // for one afterwards.
    let project_id = store.run(node.run_id)?.project_id;
    // One PR, one review: built again after review, it reopens the one it has.
    let reviewed = match store.reopen_review(project_id, branch, Some(node.run_id), Some(node.id)) {
        Ok(Some(review)) => Ok(review),
        Ok(None) => store.open_review(
            project_id,
            &subject,
            Some(node.run_id),
            Some(node.id),
            Some(branch),
        ),
        Err(error) => Err(error),
    };
    if let Err(error) = reviewed {
        // Not fatal: the work is committed and on a branch. A review that could not be
        // opened costs a surface, not the PR.
        store.append_event(
            node.run_id,
            NewEvent::new(
                EventKind::Note,
                format!("could not open a review for {}: {error}", slice.key),
            )
            .on_node(node.id),
        )?;
    }
    let _ = planner.set_branch(&slice.key, branch).await;
    let _ = planner
        .log(
            &format!(
                "ai-team built {} on {branch} ({})",
                slice.key,
                &head[..head.len().min(12)]
            ),
            Some(&slice.key),
        )
        .await;
    Ok(Some(branch.to_string()))
}

/// Move the slice to where this PR left it.
///
/// Best-effort throughout: the work is already committed by the time this runs, and a
/// board that could not be updated must not turn a finished PR into a failed one.
async fn update_board(
    planner: &Planner,
    slice_key: &str,
    worktree: &Path,
    status: NodeStatus,
    fallout: Option<crate::Fallout>,
    rejection: Option<&str>,
) {
    match status {
        // The claim stays with the PR's worktree while it is in review: it is where the
        // work is, and what the board and the window point at.
        NodeStatus::Done => {
            let _ = planner
                .set_status(slice_key, "in_review", Some("built by ai-team"))
                .await;
        }
        // Escalated work is still somebody's, so the claim stays and the board says why
        // it is waiting rather than quietly offering it to the next run.
        NodeStatus::Parked => {
            if fallout == Some(crate::Fallout::Escalate) {
                let _ = planner.set_status(slice_key, "blocked", rejection).await;
            }
        }
        // A failed slice goes back to `blocked` carrying its reason rather than staying
        // `ready`: ready would hand the next run the same slice with no memory of why it
        // did not work, and it must not look buildable when it is not.
        _ => {
            let _ = planner.set_status(slice_key, "blocked", rejection).await;
            let _ = planner.release(slice_key, worktree).await;
        }
    }
}

/// Turn a stopped PR into a status, applying the team's failure policy.
///
/// All three policies leave the siblings alone - that is the property this exists to
/// guarantee. They differ only in what happens to *this* branch.
#[allow(clippy::too_many_arguments)]
fn settle_status(
    store: &mut Store,
    run_id: i64,
    node: &NodeRun,
    slice_key: &str,
    outcome: &TurnOutcome,
    rejection: Option<&str>,
    repairs: i64,
    exhausted: bool,
) -> Result<(NodeStatus, Option<crate::Fallout>)> {
    let status = outcome_status(outcome);
    let Some(reason) = rejection else {
        return Ok((status, None));
    };

    // It stopped without being accepted. Recording it as done would put a branch on the
    // board that nothing checked.
    let policy = crate::Fallout::of(store.run(run_id)?.on_failure);
    let status = match policy {
        // Escalate parks it: somebody asked to be asked, so the work stays claimed and
        // its branch is still there for them to look at.
        crate::Fallout::Escalate => NodeStatus::Parked,
        crate::Fallout::AbortBranch => NodeStatus::Failed,
    };
    let detail = if exhausted {
        format!("stopped after {repairs} repair(s): {reason}")
    } else {
        reason.to_string()
    };
    store.block_node(node.id, &truncate_reason(&detail))?;
    store.append_event(
        run_id,
        NewEvent::new(
            EventKind::Note,
            format!("{slice_key}: {} - {detail}", policy.as_str()),
        )
        .on_node(node.id)
        .by(&node.role),
    )?;
    Ok((status, Some(policy)))
}

async fn release_failed(
    planner: &Planner,
    slice_key: &str,
    worktree: &Path,
    lease: Option<Lease>,
    error: Error,
) -> Error {
    let _ = planner.release(slice_key, worktree).await;
    let _ = planner
        .log(
            &format!("ai-team: {slice_key} failed - {error}"),
            Some(slice_key),
        )
        .await;
    if let Some(lease) = lease {
        let _ = lease.release().await;
    }
    error
}

/// Whether a PR keeps its worktree after this answer (PW10).
///
/// A built PR does: it is where review comments are worked on, and what a PR stacked on
/// it is built beside. So does one parked for a person, whose work is still somebody's.
/// One that stopped for good gives its worktree back - its branch keeps the commits.
fn keeps_worktree(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::Done | NodeStatus::Parked)
}

async fn release_and_deliver(
    lease: Option<Lease>,
    db_path: &Path,
    dispatched: &Dispatched,
) -> Result<()> {
    if let Some(lease) = lease {
        if keeps_worktree(dispatched.status) {
            // Held until the PR is merged or abandoned, not until this build ends.
            lease.preserve();
        } else {
            // Returned explicitly so a failure to return is reported rather than
            // swallowed by the Drop fallback.
            lease.release().await?;
        }
    }
    if dispatched.status == NodeStatus::Done && dispatched.branch.is_some() {
        crate::delivery::automatic_delivery(db_path, dispatched.node_run_id).await;
    }
    Ok(())
}

fn notify_node(
    store: &mut Store,
    node_run_id: i64,
    slice_key: &str,
    role: &str,
    status: NodeStatus,
) -> Result<()> {
    let node = store.node_run(node_run_id)?;
    let run = store.run(node.run_id)?;
    let (kind, title, body) = match status {
        NodeStatus::Done => (
            "completed",
            format!("{slice_key} built"),
            format!("{slice_key} is built and ready to review."),
        ),
        NodeStatus::Parked => (
            "input_required",
            format!("{role} needs your input"),
            node.blocked_reason
                .unwrap_or_else(|| format!("{slice_key} is waiting for your reply.")),
        ),
        NodeStatus::Failed | NodeStatus::Blocked => (
            "failed",
            format!("{slice_key} stopped"),
            node.blocked_reason
                .unwrap_or_else(|| format!("{slice_key} did not finish.")),
        ),
        NodeStatus::Cancelled => (
            "follow_up",
            format!("{slice_key} was cancelled"),
            format!("{slice_key} was cancelled before it finished."),
        ),
        NodeStatus::Queued | NodeStatus::Running => return Ok(()),
    };
    let project = store.project(run.project_id)?;
    store.notify_once(crate::model::NewNotification {
        dedupe_key: format!("node:{node_run_id}:{}", status.as_str()),
        project_id: run.project_id,
        workspace_path: run.workspace_path,
        run_id: Some(run.id),
        node_run_id: Some(node_run_id),
        kind: kind.into(),
        title: format!("{} · {title}", project.name),
        body,
        action_path: None,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for Pi: answers in Pi's `--mode json` stream and acts on its prompt the
    /// way a seat would, so a whole PR is built for real - commits, gates, verdict - with
    /// no model behind it. Sessions it opens are numbered; a resumed one keeps its id.
    const FAKE_PI: &str = r#"#!/bin/sh
here=$(cd "$(dirname "$0")" && pwd)
session=""
previous=""
prompt=""
for arg in "$@"; do
  if [ "$previous" = "--session-id" ]; then session="$arg"; fi
  previous="$arg"
  prompt="$arg"
done
if [ -z "$session" ]; then
  count=$(cat "$here/sessions" 2>/dev/null || echo 0)
  count=$((count + 1))
  echo "$count" > "$here/sessions"
  session="fake-$count"
fi
settle() {
  printf '{"type":"session","id":"%s"}\n' "$session"
  printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\n' "$1"
  printf '{"type":"agent_settled"}\n'
}
task=$(printf '%s\n' "$prompt" | sed -n 's/^Your task is \(T[0-9]*\): .*/\1/p' | head -n 1)
case "$prompt" in
  *"Check the pull request"*) settle "VERDICT: pass" ;;
  *"was rejected"*)
    fixing=$(printf '%s\n' "$prompt" | sed -n 's/^Your work on [A-Z0-9]* \(T[0-9]*\) was rejected.*/\1/p' | head -n 1)
    echo fixed > "repair-$fixing.txt"
    settle "fixed" ;;
  *"Your task is T"*": Fail"*)
    printf '{"type":"session","id":"%s"}\n' "$session"
    printf '{"type":"agent_error","error":"the model could not be reached"}\n'
    printf '{"type":"agent_settled"}\n' ;;
  *"Your task is T"*": Think"*) settle "nothing to change" ;;
  *"Your task is T"*": Commit"*)
    echo built > "$task.txt"
    git add "$task.txt" && git -c user.email=seat@test -c user.name=seat -c commit.gpgsign=false commit -qm "my own commit"
    settle "built and committed $task" ;;
  *"A human reviewed your work"*) echo addressed > review.txt; settle "addressed the review" ;;
  *"Your task is T"*) echo built > "$task.txt"; settle "built $task" ;;
  *) settle "nothing to do" ;;
esac
"#;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn slice(scope: &str) -> Slice {
        serde_json::from_value(serde_json::json!({
            "key": "PR1",
            "title": "Pick a range",
            "status": "ready",
            "ord": 10,
            "scope_md": scope,
            "demo_md": "the range is kept",
        }))
        .unwrap()
    }

    /// A team, a run, a checkout on the PR's branch, and a fake Pi to build it with.
    struct Fixture {
        store: Store,
        run_id: i64,
        verifier: VerifierSeat,
        rig: Rig,
        slice: Slice,
        crew: Vec<Assignment>,
        base: String,
        repo: tempfile::TempDir,
        _support: tempfile::TempDir,
    }

    impl Fixture {
        /// `ui_test` is the frontend's `npm test`, run from `ui/`: the PR's only gate.
        fn new(scope: &str, ui_test: &str) -> Fixture {
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
            let roster = store.agents(team.id).unwrap();
            let verifier = VerifierSeat {
                agent_id: roster
                    .iter()
                    .find(|agent| agent.role == crate::VERIFIER_ROLE)
                    .unwrap()
                    .id,
            };

            let slice = slice(scope);
            let crew = slice
                .tasks()
                .tasks
                .into_iter()
                .map(|task| {
                    let owner = roster
                        .iter()
                        .find(|agent| agent.role == task.owner)
                        .unwrap();
                    Assignment {
                        agent_id: owner.id,
                        role: owner.role.clone(),
                        task: Some(task),
                    }
                })
                .collect();

            let repo = tempfile::tempdir().unwrap();
            git(repo.path(), &["init", "-q", "-b", "main"]);
            for setting in [
                ["user.email", "test@example.invalid"],
                ["user.name", "ai-team test"],
                ["commit.gpgsign", "false"],
            ] {
                git(repo.path(), &["config", setting[0], setting[1]]);
            }
            std::fs::create_dir(repo.path().join("ui")).unwrap();
            std::fs::write(
                repo.path().join("ui/package.json"),
                serde_json::json!({ "scripts": { "test": ui_test } }).to_string(),
            )
            .unwrap();
            git(repo.path(), &["add", "-A"]);
            git(repo.path(), &["commit", "-qm", "fixture"]);
            git(repo.path(), &["checkout", "-qb", "ai-team/pr1"]);
            let base = git(repo.path(), &["rev-parse", "HEAD"]);

            let support = tempfile::tempdir().unwrap();
            let pi = support.path().join("pi");
            std::fs::write(&pi, FAKE_PI).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let rig = Rig {
                support: support.path().into(),
                plan_root: repo.path().into(),
                plan: None,
                sources: Vec::new(),
                registry: ModelRegistry::local_only(),
                pi: Some(pi),
            };
            Fixture {
                store,
                run_id: run.id,
                verifier,
                rig,
                slice,
                crew,
                base,
                repo,
                _support: support,
            }
        }

        async fn build(&mut self, max_repairs: i64) -> (Attempted, Rows) {
            let pr = Pr {
                rig: &self.rig,
                registry: &self.rig.registry,
                run_id: self.run_id,
                slice: &self.slice,
                crew: &self.crew,
                branch: "ai-team/pr1",
                base: &self.base,
                worktree: self.repo.path(),
                max_repairs,
                verifier: Some(&self.verifier),
            };
            let mut rows = Rows::default();
            let attempted = build_pr(&mut self.store, &pr, &mut rows, Start::Fresh)
                .await
                .unwrap();
            (attempted, rows)
        }

        /// Maker rows for this PR, oldest first.
        fn makers(&self) -> Vec<NodeRun> {
            self.store
                .node_runs(self.run_id)
                .unwrap()
                .into_iter()
                .filter(|node| node.slice_key.as_deref() == Some("PR1"))
                .collect()
        }

        fn commits(&self) -> Vec<String> {
            git(
                self.repo.path(),
                &[
                    "log",
                    "--reverse",
                    "--format=%s",
                    &format!("{}..HEAD", self.base),
                ],
            )
            .lines()
            .map(str::to_string)
            .collect()
        }
    }

    const TWO_SEATS: &str = "As an operator, I want a range.\n\n## Tasks\n\
        - T1 [frontend] Pick the range - Touches: ui/**\n\
        - T2 [backend] Keep the range - Touches: crates/**\n";

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pr_is_built_task_by_task_checked_whole_and_repaired_by_the_seat_that_broke_it() {
        // The frontend's gate fails until the frontend has repaired its task. The backend
        // built last, so a rejection sent to "whoever finished last" goes to the wrong seat.
        let mut pr = Fixture::new(TWO_SEATS, "test -f ../repair-T1.txt");

        let (attempted, rows) = pr.build(2).await;

        assert_eq!(attempted.rejection, None, "{:?}", attempted.rejection);
        assert_eq!(attempted.repairs, 1);
        // One commit per task, in task order, then the repair - each saying what it was.
        assert_eq!(
            pr.commits(),
            [
                "PR1 T1: Pick the range",
                "PR1 T2: Keep the range",
                "PR1: repair 1"
            ]
        );
        let makers = pr.makers();
        let summary: Vec<(&str, Option<&str>, i64)> = makers
            .iter()
            .map(|node| (node.role.as_str(), node.task_key.as_deref(), node.attempt))
            .collect();
        assert_eq!(
            summary,
            [
                ("frontend", Some("T1"), 1),
                ("backend", Some("T2"), 1),
                ("frontend", Some("T1"), 2),
            ]
        );
        // The repair continued the conversation that built T1, not a new one - and the
        // backend had its own.
        assert_eq!(makers[2].session_id, makers[0].session_id);
        assert_ne!(makers[1].session_id, makers[0].session_id);

        // Judged once the PR was whole: the gate ran twice, not after every task.
        let gates = pr
            .store
            .events(pr.run_id, None, 1000)
            .unwrap()
            .into_iter()
            .filter(|event| {
                event
                    .payload
                    .as_ref()
                    .is_some_and(|payload| payload.get("gate").is_some())
            })
            .count();
        assert_eq!(gates, 2);

        let (status, _) = settle(&mut pr.store, pr.run_id, "PR1", &rows, &attempted).unwrap();
        assert_eq!(status, NodeStatus::Done);
        let settled: Vec<NodeStatus> = pr.makers().iter().map(|node| node.status).collect();
        // The rejected attempt was the frontend's T1; the backend's T2 was never at fault.
        assert_eq!(
            settled,
            [NodeStatus::Failed, NodeStatus::Done, NodeStatus::Done]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_model_turn_stops_the_pr_before_anything_is_checked() {
        let mut pr = Fixture::new(
            "## Tasks\n- T1 [frontend] Fail loudly - Touches: ui/**\n\
             - T2 [backend] Never reached - Touches: crates/**\n",
            "touch ../gate-ran",
        );

        let (attempted, rows) = pr.build(2).await;

        assert!(attempted.rejection.is_some());
        assert!(
            !attempted.exhausted,
            "a failed turn is not an exhausted budget"
        );
        assert!(!pr.repo.path().join("gate-ran").exists(), "a gate ran");
        // No T2, no verifier, no repair: only the turn that failed.
        assert_eq!(pr.store.node_runs(pr.run_id).unwrap().len(), 1);
        assert!(pr.commits().is_empty());
        let (status, _) = settle(&mut pr.store, pr.run_id, "PR1", &rows, &attempted).unwrap();
        assert_ne!(status, NodeStatus::Done);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_task_that_changes_nothing_did_not_build_anything() {
        let mut pr = Fixture::new(
            "## Tasks\n- T1 [frontend] Think about it - Touches: ui/**\n",
            "true",
        );

        let (attempted, _) = pr.build(2).await;

        assert_eq!(
            attempted.rejection.as_deref(),
            Some("PR1 T1 finished without changing a file")
        );
        assert!(pr.commits().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn review_comments_go_back_into_the_pr_in_the_seats_own_conversation() {
        let mut pr = Fixture::new(TWO_SEATS, "true");
        let (built, rows) = pr.build(2).await;
        settle(&mut pr.store, pr.run_id, "PR1", &rows, &built).unwrap();
        let t1 = pr.makers()[0].clone();
        assert_eq!(t1.task_key.as_deref(), Some("T1"));

        // A follow-up is a run of its own, starting from the seat's last row.
        let project = pr.store.run(pr.run_id).unwrap().project_id;
        let follow = pr
            .store
            .create_run(project, "address review", crate::RunTrigger::Manual)
            .unwrap();
        let comments = "A human reviewed your work and left comments. Name it clearly.";
        let pr_view = Pr {
            rig: &pr.rig,
            registry: &pr.rig.registry,
            run_id: follow.id,
            slice: &pr.slice,
            crew: &pr.crew,
            branch: "ai-team/pr1",
            base: &pr.base,
            worktree: pr.repo.path(),
            max_repairs: 2,
            verifier: Some(&pr.verifier),
        };
        let mut rows = Rows::default();
        let answered = build_pr(
            &mut pr.store,
            &pr_view,
            &mut rows,
            Start::FollowUp {
                node: t1.clone(),
                comments: comments.into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(answered.rejection, None);
        assert_eq!(
            pr.commits().last().map(String::as_str),
            Some("PR1: address review")
        );
        // The frontend took them, carrying on its own conversation, on this run's row -
        // and the run that built the PR is not settled again.
        assert_eq!(rows.all.len(), 1);
        let row = pr.store.node_run(rows.all[0]).unwrap();
        assert_eq!(row.run_id, follow.id);
        assert_eq!(row.role, "frontend");
        assert_eq!(row.session_id, t1.session_id);
        assert_eq!(pr.store.node_run(t1.id).unwrap().status, NodeStatus::Done);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_seat_that_commits_its_own_work_still_built_it() {
        let mut pr = Fixture::new(
            "## Tasks\n- T1 [frontend] Commit it myself - Touches: ui/**\n",
            "true",
        );

        let (attempted, _) = pr.build(2).await;

        assert_eq!(attempted.rejection, None, "{:?}", attempted.rejection);
        assert_eq!(pr.commits(), ["my own commit"]);
    }

    #[test]
    fn a_resumed_pr_picks_up_at_the_interrupted_task_or_at_its_repairs() {
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
        let roster = store.agents(team.id).unwrap();
        let seat = |role: &str| roster.iter().find(|agent| agent.role == role).unwrap().id;
        let registry = ModelRegistry::local_only();
        let slice = slice(TWO_SEATS);
        let crew: Vec<Assignment> = slice
            .tasks()
            .tasks
            .into_iter()
            .map(|task| Assignment {
                agent_id: seat(&task.owner),
                role: task.owner.clone(),
                task: Some(task),
            })
            .collect();
        let rig = Rig {
            support: PathBuf::new(),
            plan_root: PathBuf::new(),
            plan: None,
            sources: Vec::new(),
            registry: registry.clone(),
            pi: None,
        };
        let pr = Pr {
            rig: &rig,
            registry: &registry,
            run_id: run.id,
            slice: &slice,
            crew: &crew,
            branch: "ai-team/pr1",
            base: "base",
            worktree: Path::new("."),
            max_repairs: 2,
            verifier: None,
        };
        let built = |store: &mut Store, node: &NodeRun| {
            store
                .append_event(
                    run.id,
                    NewEvent::new(EventKind::Note, "built")
                        .on_node(node.id)
                        .with(serde_json::json!({ "built": "abc", "task": node.task_key })),
                )
                .unwrap();
        };

        let t1 = store
            .dispatch_task(run.id, seat("frontend"), "PR1", Some("T1"), &registry)
            .unwrap();
        built(&mut store, &t1);
        let t2 = store
            .dispatch_task(run.id, seat("backend"), "PR1", Some("T2"), &registry)
            .unwrap();
        let rows = Rows::recover(&store, &pr).unwrap();
        let place = resume_place(&store, &pr, &rows, &t2).unwrap();
        assert_eq!((place.task, place.repairs), (Some(1), 0));

        // Both built, one rejection recorded: an interrupted turn now is a repair.
        built(&mut store, &t2);
        store
            .append_event(
                run.id,
                NewEvent::new(EventKind::Note, "rejected")
                    .on_node(t1.id)
                    .with(serde_json::json!({ "reason": "no", "rejection": true })),
            )
            .unwrap();
        let repair = store
            .dispatch_task(run.id, seat("frontend"), "PR1", Some("T1"), &registry)
            .unwrap();
        let rows = Rows::recover(&store, &pr).unwrap();
        let place = resume_place(&store, &pr, &rows, &repair).unwrap();
        assert_eq!((place.task, place.repairs), (None, 1));
    }

    #[test]
    fn a_rejection_goes_to_the_seat_that_owns_what_failed() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let roster = store.agents(team.id).unwrap();
        let crew: Vec<Assignment> = ["frontend", "backend"]
            .iter()
            .map(|role| Assignment {
                task: None,
                agent_id: roster.iter().find(|agent| agent.role == *role).unwrap().id,
                role: (*role).to_string(),
            })
            .collect();
        let owner = |hint: Option<RepairHint>| {
            repair_owner(&store, &crew, hint.as_ref())
                .unwrap()
                .role
                .clone()
        };

        assert_eq!(
            owner(Some(RepairHint::Manifest("ui/package.json".into()))),
            "frontend"
        );
        assert_eq!(
            owner(Some(RepairHint::Manifest("Cargo.toml".into()))),
            "backend"
        );
        assert_eq!(owner(Some(RepairHint::Role("frontend".into()))), "frontend");
        // Nothing narrows it, or it names a seat not on this PR: the last task's owner.
        assert_eq!(
            owner(Some(RepairHint::Manifest("package.json".into()))),
            "backend"
        );
        assert_eq!(owner(Some(RepairHint::Role("reviewer".into()))), "backend");
        assert_eq!(owner(None), "backend");
    }

    #[test]
    fn a_gate_answers_for_the_manifest_it_came_from() {
        let gate = |program: &str, dir: &str| crate::gates::Gate {
            kind: crate::gates::GateKind::Test,
            dir: dir.into(),
            program: program.into(),
            args: Vec::new(),
        };
        assert_eq!(gate_manifest(&gate("cargo", ".")), "Cargo.toml");
        assert_eq!(gate_manifest(&gate("npm", "ui")), "ui/package.json");
        assert_eq!(gate_manifest(&gate("npm", ".")), "package.json");
    }

    #[test]
    fn a_verdict_that_is_not_an_explicit_pass_is_a_rejection() {
        // Fails closed on purpose. A verifier whose answer could not be read has not
        // approved anything, and defaulting the other way is how an unchecked branch
        // reaches the person who trusted that this ran.
        assert!(matches!(
            read_verdict("Looks right to me.\nVERDICT: pass"),
            Verdict::Accepted
        ));
        assert!(matches!(read_verdict("verdict:   PASS"), Verdict::Accepted));

        for said in [
            "VERDICT: reject - subtract() returns 0",
            "This all looks good and complete!",
            "I could not find the files.",
            "",
        ] {
            assert!(
                matches!(read_verdict(said), Verdict::Rejected(_)),
                "{said:?} must not read as approval"
            );
        }

        // The last verdict wins: a model that reconsiders mid-answer means the later one.
        assert!(matches!(
            read_verdict("VERDICT: pass\n...actually no.\nVERDICT: reject"),
            Verdict::Rejected(_)
        ));
    }

    #[test]
    fn a_verifier_names_the_seat_whose_work_has_to_change() {
        assert_eq!(
            named_owner("VERDICT: reject\nOWNER: frontend").as_deref(),
            Some("frontend")
        );
        assert_eq!(
            named_owner("VERDICT: reject\n**Owner:** `Backend`.").as_deref(),
            Some("backend")
        );
        assert_eq!(named_owner("VERDICT: reject"), None);
        assert_eq!(named_owner("owner: the whole team"), None);
    }

    #[test]
    fn a_rejection_carries_what_the_seat_needs_to_act_on() {
        let Verdict::Rejected(reason) = read_verdict("VERDICT: reject - subtract() returns 0")
        else {
            panic!("expected a rejection");
        };
        assert!(reason.contains("subtract() returns 0"));

        let slice = slice(TWO_SEATS);
        let owner = Assignment {
            task: slice.tasks().tasks.into_iter().next(),
            agent_id: 1,
            role: "frontend".into(),
        };
        // That reason is what reaches the seat, not a bare "it failed" - and the PR it
        // belongs to, for a seat that starts without the conversation.
        let prompt = repair_prompt(&slice, &owner, &reason);
        assert!(
            prompt.contains("Your work on PR1 T1 was rejected"),
            "{prompt}"
        );
        assert!(prompt.contains("subtract() returns 0"));
        assert!(prompt.contains("rather than starting over"));
        assert!(prompt.contains("T2 [backend] Keep the range"), "{prompt}");
    }

    #[test]
    fn the_prompt_carries_the_pr_the_task_and_where_it_sits() {
        let slice = slice(TWO_SEATS);
        let crew: Vec<Assignment> = slice
            .tasks()
            .tasks
            .into_iter()
            .enumerate()
            .map(|(id, task)| Assignment {
                agent_id: i64::try_from(id).unwrap(),
                role: task.owner.clone(),
                task: Some(task),
            })
            .collect();

        let prompt = task_prompt(&slice, &crew, &crew[1], &[]);
        assert!(prompt.contains("PR1 - Pick a range"), "{prompt}");
        assert!(
            prompt.contains("Your task is T2: Keep the range"),
            "{prompt}"
        );
        assert!(
            prompt.contains("the range is kept"),
            "the demo is what done means"
        );
        assert!(prompt.contains("- T1 [frontend] Pick the range - built, and committed here"));
        assert!(prompt.contains("- T2 [backend] Keep the range - yours"));
        assert!(prompt.contains("Do not commit"));

        // A slice with no task lines is one piece of work, as it always was.
        let whole = Assignment {
            task: None,
            agent_id: 1,
            role: "backend".into(),
        };
        let plain = slice_with_no_tasks();
        let prompt = task_prompt(&plain, std::slice::from_ref(&whole), &whole, &[]);
        assert!(prompt.contains("Build this pull request: PR1 - Pick a range"));
        assert!(!prompt.contains("Your task is"));
        // A repo with no house rules gets no section at all: most have none, and an
        // empty heading spends context telling a model nothing.
        assert!(!prompt.contains("house rules"));

        let with_house = task_prompt(
            &plain,
            std::slice::from_ref(&whole),
            &whole,
            &[crate::house::Rules {
                path: "AGENTS.md".into(),
                body: "Never use an em dash.".into(),
                truncated: false,
            }],
        );
        // After the worktree rules, so the last thing read is the standard the work is
        // judged against.
        assert!(
            with_house.find("leased for this pull request").unwrap()
                < with_house.find("Never use an em dash").unwrap(),
            "{with_house}"
        );
    }

    #[test]
    fn the_verifier_judges_against_the_whole_scope_review_feedback_included() {
        let mut slice = slice(TWO_SEATS);
        slice.scope_md = Some(format!(
            "{}\n## Review feedback\n\n- web/index.html:7 - Add a hint under the command.",
            slice.scope_md.unwrap()
        ));
        let prompt = verify_prompt(&slice, &[], "abc123", "(gates passed)");
        assert!(
            prompt.contains("As an operator, I want a range."),
            "{prompt}"
        );
        assert!(prompt.contains("Add a hint under the command."), "{prompt}");
        assert!(
            prompt.contains("the review feedback is what was asked for"),
            "{prompt}"
        );
        assert!(prompt.contains("git diff abc123"), "{prompt}");
    }

    #[test]
    fn a_long_task_title_is_clipped_in_the_subject_and_kept_whole_in_the_body() {
        let slice = slice(TWO_SEATS);
        let long = Assignment {
            task: Some(Task {
                key: "T3".into(),
                owner: "frontend".into(),
                title: "Show `sh src/greet.sh --shout Ada` on the page beside the existing \
                        command, leaving the original paragraph untouched"
                    .into(),
                touches: vec!["web/index.html".into()],
            }),
            agent_id: 1,
            role: "frontend".into(),
        };
        let message = commit_subject(&slice, &long);
        let (subject, body) = message.split_once("\n\n").unwrap();
        assert!(subject.chars().count() <= SUBJECT_LIMIT, "{subject}");
        assert!(subject.starts_with("PR1 T3: Show"), "{subject}");
        assert!(
            subject.ends_with("beside the…"),
            "clipped at a word: {subject}"
        );
        assert!(body.ends_with("original paragraph untouched"), "{body}");

        let short = Assignment {
            task: slice.tasks().tasks.into_iter().next(),
            agent_id: 1,
            role: "frontend".into(),
        };
        assert_eq!(commit_subject(&slice, &short), "PR1 T1: Pick the range");
    }

    fn slice_with_no_tasks() -> Slice {
        slice("Add the export.\n\nTouches: crates/**")
    }
}
