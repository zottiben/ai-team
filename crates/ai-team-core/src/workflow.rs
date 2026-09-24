//! One prompt, all the way through, from wherever it was asked.
//!
//! The terminal and the window both start runs, and the ordering - resolve, generate,
//! build, plan or skip, dispatch - has to be the same either way. Two copies of it drift,
//! and the copy that drifts is always the one nobody demoed today.
//!
//! What differs between callers is only how progress is *shown*: the CLI prints it, the
//! window reads the database the run is already writing to. So progress is a callback,
//! and everything else lives here.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::machine::{ContextSource, ModelRegistry};
use crate::model::{NodeStatus, ProjectSource, RunStatus, RunTrigger};
use crate::neighbours::{Planner, Worktrees};
use crate::store::Store;
use crate::supervise::{Orchestration, Orchestrator};

pub const PLAN_APPROVAL_REASON: &str = "Plan ready for approval";
pub const PLAN_APPROVAL_PREPARING_REASON: &str = "Preparing plan approval";

/// What was asked for.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// Slug, id, or part of the name.
    pub project: String,
    /// Absent means "build whatever the plan already has ready".
    pub prompt: Option<String>,
    /// The checkout that owns this workflow. Absent selects the project's main checkout.
    pub workspace: Option<PathBuf>,
    pub plan: Option<String>,
    pub width: Option<usize>,
    /// Plan again even though there is ready work.
    pub replan: bool,
    /// Stop after the plan is written, leaving its ready slices available to the CLI.
    pub plan_only: bool,
    /// Stop after planning and hold its ready slices for first-class UI approval.
    pub approval_required: bool,
    /// Which branch the run works on. A fresh one unless the operator says otherwise.
    pub branching: Branching,
    /// What started it, when that was not the operator: a schedule. Kept on the run, so the
    /// window says who asked rather than assuming it was whoever is looking.
    pub trigger: Option<RunTrigger>,
}

/// Which branch a run that plans works on (PW2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Branching {
    /// A new `ai-team/run-<id>` cut from origin's default branch.
    #[default]
    Fresh,
    /// The default branch itself. Nothing lands on main/master unless the operator asks
    /// for it, and then only for the run they asked for.
    DefaultBranch,
}

/// What a caller might want to show while it happens.
#[derive(Debug, Clone)]
pub enum Progress {
    Started {
        run_id: i64,
        repo: PathBuf,
    },
    /// Which seats resolved to which provider, before any work starts.
    ///
    /// There is nothing generated any more (D20), but a denied preference falling back
    /// to another provider (D13) is still something the operator should see before the
    /// work begins rather than discover in the analytics afterwards.
    Seated {
        fallbacks: Vec<String>,
    },
    /// The board already had work, so the prompt was not planned from.
    PlanSkipped {
        ready: usize,
    },
    /// The run's checkout is on its own branch, ready to plan from.
    Branched {
        branch: String,
        from: String,
        /// Why the default branch could not be fetched first, when it could not.
        stale: Option<String>,
    },
    Planning,
    /// Something a node said or did, already classified for display.
    Note(String),
    /// Advisory, said before the work rather than after: it changes what the green at
    /// the end is worth.
    Caution(String),
    Dispatching {
        width: usize,
    },
    Routed {
        slice: String,
        role: String,
    },
}

/// Run one workflow to completion.
///
/// Everything it does is recorded in the store as it goes, which is what lets a window
/// opened halfway through show the same run the terminal is watching.
/// The planning turn: one prompt in, an ai-planner plan out.
async fn plan_it<F>(
    orchestrator: &Orchestrator,
    store: &mut Store,
    run_id: i64,
    prompt: &str,
    on_progress: &mut F,
) -> Result<()>
where
    F: FnMut(Progress) + Send,
{
    store.set_run_status(run_id, RunStatus::Planning)?;
    on_progress(Progress::Planning);
    let outcome = match orchestrator
        .plan(store, prompt, |event| {
            if let crate::PiDisposition::Record(kind, summary) = event.classify() {
                on_progress(Progress::Note(format!(
                    "{:<18} {summary}",
                    format!("{kind:?}").to_lowercase()
                )));
            }
        })
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let reason = error.to_string();
            if reason.contains("CONTEXT_UNAVAILABLE:") {
                store.block_run(run_id, &reason)?;
                notify_run(
                    store,
                    run_id,
                    "input_required",
                    "Planning needs context",
                    &reason,
                );
            } else if reason.contains("MODEL_UNAVAILABLE:") {
                let detail = reason
                    .split_once("MODEL_UNAVAILABLE:")
                    .map_or(reason.as_str(), |(_, detail)| detail.trim());
                store.block_run(run_id, detail)?;
                notify_run(
                    store,
                    run_id,
                    "input_required",
                    "Model subscription unavailable",
                    detail,
                );
            } else {
                store.set_run_status(run_id, RunStatus::Failed)?;
                notify_run(
                    store,
                    run_id,
                    "failed",
                    "Planning stopped",
                    "The planning turn did not finish.",
                );
            }
            return Err(error);
        }
    };
    if crate::outcome_status(&outcome) != NodeStatus::Done {
        store.set_run_status(run_id, RunStatus::Failed)?;
        notify_run(
            store,
            run_id,
            "failed",
            "Planning stopped",
            "The planning turn did not finish.",
        );
        return Err(Error::invalid("planning did not finish"));
    }
    Ok(())
}

/// What the database knows, read in one synchronous window.
///
/// Separate from the checks that follow because those talk to other programs, and a
/// future holding a database connection across that is both rude and - since a
/// connection is not `Sync` - impossible to spawn.
struct Resolved {
    team_id: i64,
    project_slug: String,
    project_id: i64,
    repo: PathBuf,
    required_context: Vec<ContextSource>,
    parallel_width: usize,
}

/// Everything resolved before a run row exists.
struct Prepared {
    team_id: i64,
    project_slug: String,
    project_id: i64,
    repo: PathBuf,
    required_context: Vec<ContextSource>,
    planner: Planner,
    plan_slug: Option<String>,
    worktrees: Worktrees,
    prompt: String,
    planning: bool,
    ready: usize,
    parallel_width: usize,
}

/// Resolve what the request names, and decide whether there is planning to do.
///
/// Everything here can fail without having created anything, which is the point: a run
/// row that exists because the checkout was missing is a row somebody has to explain.
fn resolve(store: &Store, request: &Request) -> Result<Resolved> {
    // Everything the database is asked for, up front and owned. The borrow ends here on
    // purpose: the checks below talk to other programs, and holding a connection open
    // across that would make this future unspawnable as well as rude.
    let (team_id, project_slug, project_id, parallel_width, repo, required_context) = {
        let project = store.find_project(&request.project)?;
        let team_id = project.team_id.ok_or_else(|| {
            Error::invalid(format!(
                "{} has no team - run `ait init` first",
                project.slug
            ))
        })?;
        let width = usize::try_from(store.team(team_id)?.guardrails.parallel_width).unwrap_or(2);

        // The repository is where the plan lives and where the worktree pool is rooted.
        // A project with no repo attached (D6 allows that) has nowhere to lease from,
        // and saying so beats leasing out of whatever directory the caller was in.
        let repo = store
            .project_repos(project.id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{} has no checkout attached, so there is nothing to lease worktrees \
                     from. Run `ait init` in the repository.",
                    project.slug
                ))
            })?;
        let repo = PathBuf::from(repo)
            .canonicalize()
            .map_err(|_| Error::invalid("the project's checkout no longer exists"))?;
        let required_context = required_context(
            project.source,
            project.source_url.as_deref(),
            &project.brief_md,
            request.prompt.as_deref().unwrap_or_default(),
        );
        (
            team_id,
            project.slug,
            project.id,
            width,
            repo,
            required_context,
        )
    };
    Ok(Resolved {
        team_id,
        project_slug,
        project_id,
        repo,
        required_context,
        parallel_width,
    })
}

/// The checks and decisions that need to talk to ai-planner and ai-worktree.
async fn prepare(resolved: Resolved, request: &Request) -> Result<Prepared> {
    let Resolved {
        team_id,
        project_slug,
        project_id,
        repo,
        required_context,
        parallel_width,
    } = resolved;

    let repo = match &request.workspace {
        Some(workspace) => Worktrees::at(&repo).resolve(workspace).await?,
        None => repo,
    };
    let planner = match &request.plan {
        Some(plan) => Planner::at(&repo).for_plan(plan.clone()),
        None => Planner::at(&repo),
    };
    let worktrees = Worktrees::at(&repo);
    // Both checked before anything is created: a run that cannot lease a worktree has
    // nothing to do, and finding that out after the plan is written wastes a turn.
    planner.check().await.map_err(Error::invalid)?;
    worktrees.check().await.map_err(Error::invalid)?;
    // Before anything is created: a planning seat reaches ai-planner through its MCP
    // server, and that server will not start without a database.
    planner.ensure().await?;

    // With no prompt, pick up work the current plan already has ready. A prompt always
    // starts a planning turn: silently dispatching an unrelated existing plan is how a
    // checkout-specific request was immediately marked blocked without ever being read.
    let ready = planner.slices().await.map_or(0, |slices| {
        slices
            .into_iter()
            .filter(crate::Slice::is_dispatchable)
            .count()
    });
    let planning = should_plan(request, ready);
    let (planner, plan_slug, ready) = if planning {
        (planner, None, ready)
    } else {
        // A no-prompt build belongs to the exact plan the checkout resolves now. Pinning
        // both the dispatcher and the run row prevents a second click from creating a
        // duplicate while the first run is still leasing its initial worktree.
        let current = planner.current().await?;
        let slug = current.plan;
        let planner = planner.for_plan(slug.clone());
        let ready = planner
            .slices()
            .await?
            .into_iter()
            .filter(crate::Slice::is_dispatchable)
            .count();
        if ready == 0 {
            return Err(Error::invalid(
                "the plan changed before the run started and has no ready slices; refresh the board",
            ));
        }
        (planner, Some(slug), ready)
    };

    let prompt = match (&request.prompt, planning) {
        (Some(prompt), _) => prompt.clone(),
        (None, true) => {
            return Err(Error::invalid(
                "nothing on the plan is ready to build, so there is nothing to pick up. \
                 Give a prompt to plan from.",
            ))
        }
        (None, false) => format!("build the {ready} ready slice(s)"),
    };

    // Something new to plan puts the checkout on a fresh branch (PW2), which would carry
    // a person's uncommitted work along into the run's pull request. Refused here, before
    // there is a run row to explain.
    if planning {
        crate::workspace::ensure_clean(&repo).await?;
    }

    Ok(Prepared {
        team_id,
        project_slug,
        project_id,
        repo,
        required_context,
        planner,
        plan_slug,
        worktrees,
        prompt,
        planning,
        ready,
        parallel_width: request.width.unwrap_or(parallel_width),
    })
}

fn required_context(
    source: Option<ProjectSource>,
    source_url: Option<&str>,
    brief: &str,
    prompt: &str,
) -> Vec<ContextSource> {
    let mut required = Vec::new();
    match source {
        Some(ProjectSource::ClickUp) => required.push(ContextSource::ClickUp),
        Some(ProjectSource::Figma) => required.push(ContextSource::Figma),
        Some(ProjectSource::Manual) | None => {}
    }
    let prose = format!("{}\n{brief}\n{prompt}", source_url.unwrap_or_default()).to_lowercase();
    for (needle, source) in [
        ("clickup.com", ContextSource::ClickUp),
        ("figma.com", ContextSource::Figma),
    ] {
        if prose.contains(needle) && !required.contains(&source) {
            required.push(source);
        }
    }
    required
}

fn context_problem(required: &[ContextSource], registry: &ModelRegistry) -> Option<String> {
    let allowed = registry.context_sources();
    for source in required {
        if !allowed.contains(source) {
            return Some(format!(
                "Planning needs {source} context, but it is denied in this machine's context policy. Enable and connect {source} in Settings."
            ));
        }
        if !crate::secrets::has_oauth(*source) && !crate::secrets::has_token(*source) {
            return Some(format!(
                "Planning needs {source} context, but it is not connected. Connect {source} in Settings and continue this run."
            ));
        }
    }
    None
}

/// Stop a run that needs a context source this machine cannot reach, before it plans.
fn block_without_context(
    store: &mut Store,
    run_id: i64,
    required: &[ContextSource],
    registry: &ModelRegistry,
) -> Result<()> {
    let Some(reason) = context_problem(required, registry) else {
        return Ok(());
    };
    store.block_run(run_id, &reason)?;
    notify_run(
        store,
        run_id,
        "input_required",
        "Planning needs context",
        &reason,
    );
    Err(Error::invalid(reason))
}

/// Put the run's checkout on the branch it will plan from (PW2), and say so.
async fn branch_workspace<F>(
    store: &mut Store,
    run_id: i64,
    repo: &Path,
    request: &Request,
    on_progress: &mut F,
) -> Result<()>
where
    F: FnMut(Progress) + Send,
{
    let branched = if request.branching == Branching::DefaultBranch {
        crate::workspace::stay_on_default_branch(repo).await?
    } else {
        crate::workspace::branch_for_run(repo, run_id).await?
    };
    let short = &branched.sha[..branched.sha.len().min(7)];
    store.append_event(
        run_id,
        crate::NewEvent::new(
            crate::EventKind::Note,
            format!(
                "{} is on {}, from {} ({short})",
                repo.display(),
                branched.branch,
                branched.from
            ),
        )
        .by("ai-team"),
    )?;
    if let Some(stale) = &branched.stale {
        store.append_event(
            run_id,
            crate::NewEvent::new(
                crate::EventKind::Note,
                format!(
                    "could not bring {} up to date first, so this starts from the last \
                     fetched copy: {stale}",
                    branched.from
                ),
            )
            .by("ai-team"),
        )?;
    }
    on_progress(Progress::Branched {
        branch: branched.branch,
        from: branched.from,
        stale: branched.stale,
    });
    Ok(())
}

/// Refuse to continue a run into a checkout another live run is still working in.
///
/// Asked before the approval is claimed, so a refusal leaves the plan waiting for
/// approval rather than turning it into a failed run.
fn refuse_busy_workspace(store: &Store, run_id: i64) -> Result<()> {
    let run = store.run(run_id)?;
    let Some(workspace) = run.workspace_path.as_deref() else {
        return Ok(());
    };
    match store.busy_run_in_workspace(
        run.project_id,
        Path::new(workspace),
        crate::util::process_is_alive,
    )? {
        Some(busy) if busy.id != run_id => Err(Error::invalid(crate::store::workspace_busy(
            busy.id,
            busy.status,
            busy.supervisor_pid.unwrap_or_default(),
        ))),
        _ => Ok(()),
    }
}

fn should_plan(request: &Request, ready: usize) -> bool {
    request.replan || request.prompt.is_some() || ready == 0
}

fn record_workflow_failure(store: &mut Store, run_id: i64, error: &Error) -> Result<()> {
    let run = store.run(run_id)?;
    if matches!(
        run.status,
        RunStatus::Blocked | RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
    ) {
        return Ok(());
    }
    let reason = error.to_string();
    store.fail_run(run_id, &reason)?;
    store.append_event(
        run_id,
        crate::NewEvent::new(crate::EventKind::Failed, &reason).by("ai-team"),
    )?;
    notify_run(store, run_id, "failed", "Team run stopped", &reason);
    Ok(())
}

fn finish_run(store: &mut Store, run_id: i64, done: Orchestration) -> Result<Orchestration> {
    let unfinished = done
        .dispatched
        .iter()
        .filter(|node| node.status != NodeStatus::Done)
        .count();
    let status = if unfinished > 0 || !done.unrouted.is_empty() || done.dispatched.is_empty() {
        RunStatus::Blocked
    } else {
        RunStatus::Done
    };
    match status {
        RunStatus::Blocked => {
            let mut reasons: Vec<String> = done
                .dispatched
                .iter()
                .filter(|node| node.status != NodeStatus::Done)
                .map(|node| {
                    format!(
                        "{} ({}) {}",
                        node.slice_key,
                        node.role,
                        node.status.as_str()
                    )
                })
                .collect();
            reasons.extend(
                done.unrouted
                    .iter()
                    .map(|(slice, reason)| format!("{slice} unrouted: {reason}")),
            );
            if done.dispatched.is_empty() && done.unrouted.is_empty() {
                reasons.push("no slices were dispatched".into());
            }
            store.block_run(run_id, &reasons.join("; "))?;
        }
        _ => {
            store.set_run_status(run_id, status)?;
        }
    }
    match status {
        RunStatus::Done => notify_run(
            store,
            run_id,
            "completed",
            "Team run complete",
            "Every dispatched slice finished and passed its checks.",
        ),
        RunStatus::Blocked => notify_run(
            store,
            run_id,
            "follow_up",
            "Team run needs follow-up",
            "The run stopped with work still blocked or unrouted.",
        ),
        _ => {}
    }
    Ok(done)
}

async fn stop_before_build(
    request: &Request,
    orchestrator: &Orchestrator,
    store: &mut Store,
    run_id: i64,
) -> Result<bool> {
    if request.approval_required {
        // Claim plan ownership in the run before changing the external board. A sibling
        // checkout can resolve to the same ai-planner plan; without this visible blocked
        // state it could release the holds in the gap and start a replacement run.
        store.begin_plan_hold(run_id, PLAN_APPROVAL_PREPARING_REASON, PLAN_APPROVAL_REASON)?;
        let held = match orchestrator
            .planner
            .hold_ready_for_approval(|| {
                if store.transition_blocked_run(
                    run_id,
                    PLAN_APPROVAL_PREPARING_REASON,
                    PLAN_APPROVAL_PREPARING_REASON,
                )? {
                    Ok(())
                } else {
                    Err(Error::invalid(
                        "plan approval preparation was continued by another process",
                    ))
                }
            })
            .await
        {
            Ok(held) => held,
            Err(error) => {
                let reason = format!("The plan could not be held for approval: {error}");
                if store.transition_blocked_run(run_id, PLAN_APPROVAL_PREPARING_REASON, &reason)? {
                    notify_run(
                        store,
                        run_id,
                        "failed",
                        "Plan approval setup failed",
                        &reason,
                    );
                    return Err(error);
                }
                let _ = store.append_event(
                    run_id,
                    crate::NewEvent::new(
                        crate::EventKind::Note,
                        format!("plan approval hold stopped after ownership moved: {error}"),
                    )
                    .by("ai-team"),
                );
                return Ok(true);
            }
        };
        if held == 0 {
            let reason = "The planner produced no ready slices to approve";
            if store.transition_blocked_run(run_id, PLAN_APPROVAL_PREPARING_REASON, reason)? {
                notify_run(store, run_id, "follow_up", "Plan needs follow-up", reason);
            }
            return Ok(true);
        }
        let findings = record_crew_findings(orchestrator, store, run_id).await;
        if store.transition_blocked_run(
            run_id,
            PLAN_APPROVAL_PREPARING_REASON,
            PLAN_APPROVAL_REASON,
        )? {
            let mut body = String::from(
                "Review the plan, then approve it to continue this same run into the build.",
            );
            if !findings.is_empty() {
                let _ = write!(body, "\n\nWorth a look first: {}", findings.join("; "));
            }
            notify_run(store, run_id, "plan_ready", "Plan ready", &body);
        }
        return Ok(true);
    }
    if request.plan_only {
        store.set_run_status(run_id, RunStatus::Done)?;
        notify_run(
            store,
            run_id,
            "plan_ready",
            "Plan ready",
            "The plan is ready on the board.",
        );
        return Ok(true);
    }
    Ok(false)
}

/// Check each PR's tasks against the team before a person approves the plan (PW4), and
/// record what is worth knowing on the run. Best-effort: a board that could not be read
/// back is not a reason to lose the approval, and the dispatcher checks again anyway.
async fn record_crew_findings(
    orchestrator: &Orchestrator,
    store: &mut Store,
    run_id: i64,
) -> Vec<String> {
    let Ok(slices) = orchestrator.offered().await else {
        return Vec::new();
    };
    let Ok(findings) = orchestrator.crew_findings(store, &slices) else {
        return Vec::new();
    };
    let mut summary = Vec::new();
    for (slice, finding, blocking) in findings {
        let line = if blocking {
            format!("{slice} would not be built: {finding}")
        } else {
            format!("{slice}: {finding}")
        };
        let _ = store.append_event(
            run_id,
            crate::NewEvent::new(
                if blocking {
                    crate::EventKind::Failed
                } else {
                    crate::EventKind::Note
                },
                &line,
            )
            .by("orchestrator")
            .with(serde_json::json!({ "slice": slice, "blocking": blocking })),
        );
        summary.push(line);
    }
    summary
}

struct StartedExecution<'a> {
    request: &'a Request,
    orchestrator: &'a mut Orchestrator,
    run_id: i64,
    prompt: &'a str,
    planning: bool,
    ready: usize,
    width: usize,
}

impl StartedExecution<'_> {
    async fn execute<F>(self, store: &mut Store, on_progress: &mut F) -> Result<Orchestration>
    where
        F: FnMut(Progress) + Send,
    {
        if self.planning {
            plan_it(
                self.orchestrator,
                store,
                self.run_id,
                self.prompt,
                on_progress,
            )
            .await?;
            // Hold and build the plan this run made, by name, rather than whichever plan
            // the checkout happens to resolve to next.
            if let Some(slug) = store.run(self.run_id)?.plan_slug {
                self.orchestrator.planner = self.orchestrator.planner.clone().for_plan(slug);
            }
        } else {
            on_progress(Progress::PlanSkipped { ready: self.ready });
        }

        if stop_before_build(self.request, self.orchestrator, store, self.run_id).await? {
            return Ok(Orchestration::default());
        }

        store.set_run_status(self.run_id, RunStatus::Running)?;
        if let Some(note) = self.orchestrator.verifier_note(store)? {
            on_progress(Progress::Caution(note.clone()));
            store.append_event(
                self.run_id,
                crate::NewEvent::new(crate::EventKind::Note, note).by("orchestrator"),
            )?;
        }
        on_progress(Progress::Dispatching { width: self.width });

        let done = self
            .orchestrator
            .build_slices(store, |line| {
                if let Some((slice, role)) = line.split_once(" -> ") {
                    on_progress(Progress::Routed {
                        slice: slice.to_string(),
                        role: role.to_string(),
                    });
                }
            })
            .await?;

        finish_run(store, self.run_id, done)
    }
}

/// The PR a worktree was built as, when the run that built it started somewhere else:
/// its key, its branch, and the checkout above it.
type HeldPr = (Option<String>, Option<String>, Option<String>);

/// Read in one synchronous window, so no borrowed store is held across the git call after.
fn pr_held_in(store: &Store, repo: &Path) -> Result<Option<HeldPr>> {
    let path = repo.to_string_lossy();
    let Some(node) = store.latest_pr_node_in(&path)? else {
        return Ok(None);
    };
    let run = store.run(node.run_id)?;
    let started_here = run
        .workspace_path
        .as_deref()
        .is_some_and(|workspace| crate::neighbours::same_worktree(workspace, &path));
    Ok((!started_here).then_some((node.slice_key, node.branch, run.workspace_path)))
}

/// Refuse to start a run in a pull request's worktree (PW1).
///
/// A PR's worktree is a leaf of the run that planned it: its crew builds it and review
/// comments come back to it, but the orchestrator coordinates from the checkout above. A
/// run started here would plan inside a PR and switch its branch out from under it. Only
/// while it still holds that PR, though - a slot `awt` has taken back is nobody's.
async fn refuse_pr_worktree(held: Option<HeldPr>, repo: &Path) -> Result<()> {
    let Some((slice, branch, above)) = held else {
        return Ok(());
    };
    if branch.is_none() || crate::neighbours::current_branch(repo).await != branch {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "{} is {}'s worktree. A team run starts from the checkout above it{}.",
        repo.display(),
        slice.as_deref().unwrap_or("a pull request"),
        above.map_or_else(String::new, |above| format!(" ({above})"))
    )))
}

/// Create the run's row and take its checkout.
///
/// One live run per checkout (PW12). Asked before the row exists, so the usual refusal
/// leaves nothing behind; `supervise_run` then closes the race between two windows.
fn open_run(
    store: &mut Store,
    request: &Request,
    project_id: i64,
    repo: &Path,
    prompt: &str,
    plan_slug: Option<&str>,
) -> Result<crate::model::Run> {
    if let Some(busy) =
        store.busy_run_in_workspace(project_id, repo, crate::util::process_is_alive)?
    {
        return Err(Error::invalid(crate::store::workspace_busy(
            busy.id,
            busy.status,
            busy.supervisor_pid.unwrap_or_default(),
        )));
    }

    let trigger = request.trigger.unwrap_or(RunTrigger::Manual);
    let run = store.create_run_in(project_id, prompt, trigger, Some(repo))?;
    let opened = (|| {
        if request.branching == Branching::DefaultBranch {
            store.set_run_on_default_branch(run.id)?;
        }
        store.supervise_run(
            run.id,
            i64::from(std::process::id()),
            crate::util::process_is_alive,
        )?;
        if let Some(plan_slug) = plan_slug {
            store.claim_run_plan(run.id, plan_slug)?;
        }
        Ok::<_, Error>(())
    })();
    if let Err(error) = opened {
        record_workflow_failure(store, run.id, &error)?;
        return Err(error);
    }
    Ok(run)
}

pub async fn run<F>(request: &Request, mut on_progress: F) -> Result<Orchestration>
where
    F: FnMut(Progress) + Send,
{
    let db = crate::default_db_path()?;
    let mut store = Store::open(&db)?;
    let registry = ModelRegistry::load()?;

    let Prepared {
        team_id,
        project_slug,
        project_id,
        repo,
        required_context,
        planner,
        plan_slug,
        worktrees,
        prompt,
        planning,
        ready,
        parallel_width: width,
    } = prepare(resolve(&store, request)?, request).await?;

    let held = pr_held_in(&store, &repo)?;
    refuse_pr_worktree(held, &repo).await?;
    let run = open_run(
        &mut store,
        request,
        project_id,
        &repo,
        &prompt,
        plan_slug.as_deref(),
    )?;
    on_progress(Progress::Started {
        run_id: run.id,
        repo: repo.clone(),
    });
    if planning {
        block_without_context(&mut store, run.id, &required_context, &registry)?;
        if let Err(error) =
            branch_workspace(&mut store, run.id, &repo, request, &mut on_progress).await
        {
            record_workflow_failure(&mut store, run.id, &error)?;
            return Err(error);
        }
    }

    // There is no project to generate any more (D20). A seat is a Pi invocation, so the
    // only thing written for a run is the guard and the per-seat MCP configs - which is
    // also why the install and build phases below are gone, and with them the twenty
    // seconds every run used to spend before a model saw the prompt.
    //
    // The resolutions are still reported: a denied preference falling back to another
    // provider (D13) is something the operator should see before the work starts, not
    // discover in the analytics afterwards.
    let initialization = (|| {
        let project_dir = store.support_dir(&project_slug)?;
        let agents: Vec<crate::Agent> = store
            .agents(team_id)?
            .into_iter()
            .filter(|agent| agent.enabled)
            .collect();
        let (_, resolutions) = registry.resolve_agents(&agents)?;
        Ok::<_, Error>((project_dir, resolutions))
    })();
    let (project_dir, resolutions) = match initialization {
        Ok(initialized) => initialized,
        Err(error) => {
            record_workflow_failure(&mut store, run.id, &error)?;
            return Err(error);
        }
    };
    on_progress(Progress::Seated {
        fallbacks: resolutions
            .iter()
            .filter_map(crate::ModelResolution::notice)
            .collect(),
    });

    let mut orchestrator = Orchestrator {
        db_path: db.clone(),
        project_dir,
        repo,
        run_id: run.id,
        team_id,
        registry,
        parallel_width: width,
        planner,
        worktrees,
    };

    let result = StartedExecution {
        request,
        orchestrator: &mut orchestrator,
        run_id: run.id,
        prompt: &prompt,
        planning,
        ready,
        width,
    }
    .execute(&mut store, &mut on_progress)
    .await;
    if let Err(error) = &result {
        record_workflow_failure(&mut store, run.id, error)?;
    }
    result
}

/// Continue a plan-only run after the human has approved its board.
///
/// The run id, team id, guardrails, workspace, plan slug and orchestrator node all come
/// from the existing rows. Creating a new run here would lose the session and make the
/// approval apply to a different snapshot from the one the human reviewed.
fn claimable_approval_reason(store: &Store, run_id: i64) -> Result<(&'static str, Option<String>)> {
    match store.run(run_id)?.blocked_reason.as_deref() {
        Some(PLAN_APPROVAL_REASON) => Ok((PLAN_APPROVAL_REASON, None)),
        Some(PLAN_APPROVAL_PREPARING_REASON) => Ok((
            PLAN_APPROVAL_PREPARING_REASON,
            Some(crate::util::rfc3339_in(-5 * 60)),
        )),
        _ => Err(Error::invalid(
            "that run is not waiting for plan approval; refresh its current state",
        )),
    }
}

pub fn claim_plan_approval(store: &mut Store, run_id: i64) -> Result<()> {
    let (reason, cutoff) = claimable_approval_reason(store, run_id)?;
    refuse_busy_workspace(store, run_id)?;
    match store.begin_plan_approval(run_id, reason, cutoff.as_deref()) {
        Err(Error::Invalid(_)) if cutoff.is_some() => Err(Error::invalid(
            "plan approval is still being prepared; retry in a few minutes if it does not finish",
        )),
        result => result.map(|_| ()),
    }
}

/// Approve through the orchestrator control channel and keep the operator's direction in
/// the conversation that will coordinate dispatch. The run is claimed first, so two
/// windows cannot both continue it; the node is validated before that claim so an invalid
/// legacy run is left untouched rather than becoming an unresumable running run.
pub fn claim_plan_approval_with_direction(
    store: &mut Store,
    run_id: i64,
    direction: &str,
) -> Result<()> {
    let coordinator = store
        .node_runs(run_id)?
        .into_iter()
        .rev()
        .find(|node| node.role == crate::ROOT_ROLE && node.session_id.is_some())
        .ok_or_else(|| Error::invalid("the run has no orchestrator session to continue"))?;
    let agent_id = coordinator
        .agent_id
        .ok_or_else(|| Error::invalid("the run's orchestrator seat no longer exists"))?;
    let (reason, cutoff) = claimable_approval_reason(store, run_id)?;
    refuse_busy_workspace(store, run_id)?;
    let result = store.begin_plan_approval_with_direction(
        run_id,
        reason,
        coordinator.id,
        agent_id,
        direction,
        cutoff.as_deref(),
    );
    if matches!(&result, Err(Error::Invalid(_))) && cutoff.is_some() {
        return Err(Error::invalid(
            "plan approval is still being prepared; retry in a few minutes if it does not finish",
        ));
    }
    result?;
    Ok(())
}

pub async fn approve_at(db: &Path, run_id: i64) -> Result<Orchestration> {
    let mut store = Store::open(db)?;
    claim_plan_approval(&mut store, run_id)?;
    drop(store);
    continue_approved_at(db, run_id).await
}

async fn release_approved_slices(store: &mut Store, planner: &Planner, run_id: i64) -> Result<()> {
    let released = match planner.approve_held().await {
        Ok(released) => released,
        Err(error) => {
            let reason = format!("the approved plan could not be advanced: {error}");
            store.block_run(run_id, &reason)?;
            notify_run(
                store,
                run_id,
                "failed",
                "Approved plan could not continue",
                &reason,
            );
            return Err(error);
        }
    };
    if released == 0
        && !planner
            .slices()
            .await?
            .iter()
            .any(crate::Slice::is_dispatchable)
    {
        let reason = "the approved plan has no held or ready slices to build";
        store.block_run(run_id, reason)?;
        notify_run(
            store,
            run_id,
            "follow_up",
            "Approved plan needs follow-up",
            reason,
        );
        return Err(Error::invalid(reason));
    }
    Ok(())
}

pub async fn continue_approved_at(db: &Path, run_id: i64) -> Result<Orchestration> {
    let mut store = Store::open(db)?;
    let run = store.run(run_id)?;
    if run.status != RunStatus::Running || run.blocked_reason.is_some() {
        return Err(Error::invalid(
            "that run is not an approved continuation; refresh its current state",
        ));
    }
    if let Err(error) = store.supervise_run(
        run_id,
        i64::from(std::process::id()),
        crate::util::process_is_alive,
    ) {
        record_workflow_failure(&mut store, run_id, &error)?;
        return Err(error);
    }
    let initialization = (|| {
        let team_id = run
            .team_id
            .ok_or_else(|| Error::invalid("the approved run has no team snapshot"))?;
        let plan_slug = run
            .plan_slug
            .clone()
            .ok_or_else(|| Error::invalid("the approved run has no plan"))?;
        let repo = run
            .workspace_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| Error::invalid("the approved run has no workspace"))?
            .canonicalize()
            .map_err(|_| Error::invalid("the approved run's workspace no longer exists"))?;
        let project = store.project(run.project_id)?;
        let registry = ModelRegistry::load()?;
        let planner = Planner::at(&repo).for_plan(plan_slug);
        let orchestrator = Orchestrator {
            db_path: db.to_path_buf(),
            project_dir: store.support_dir(&project.slug)?,
            repo: repo.clone(),
            run_id,
            team_id,
            registry,
            parallel_width: usize::try_from(run.parallel_width).unwrap_or(1).max(1),
            planner: planner.clone(),
            worktrees: Worktrees::at(&repo),
        };
        Ok::<_, Error>((planner, orchestrator))
    })();
    let (planner, orchestrator) = match initialization {
        Ok(initialized) => initialized,
        Err(error) => {
            record_workflow_failure(&mut store, run_id, &error)?;
            return Err(error);
        }
    };

    match orchestrator.acknowledge_plan_approval(&mut store).await {
        Ok(outcome) if crate::outcome_status(&outcome) == NodeStatus::Done => {}
        Ok(_) => {
            let reason = "the orchestrator could not resume after plan approval";
            store.block_run(run_id, reason)?;
            notify_run(
                &mut store,
                run_id,
                "failed",
                "Approval continuation stopped",
                reason,
            );
            return Err(Error::invalid(reason));
        }
        Err(error) => {
            let reason = format!("the orchestrator could not resume after plan approval: {error}");
            store.block_run(run_id, &reason)?;
            notify_run(
                &mut store,
                run_id,
                "failed",
                "Approval continuation stopped",
                &reason,
            );
            return Err(error);
        }
    }

    let result = async {
        release_approved_slices(&mut store, &planner, run_id).await?;
        store.set_run_status(run_id, RunStatus::Running)?;
        if let Some(note) = orchestrator.verifier_note(&store)? {
            store.append_event(
                run_id,
                crate::NewEvent::new(crate::EventKind::Note, note).by("orchestrator"),
            )?;
        }
        let done = orchestrator.build_slices(&mut store, |_| {}).await?;
        finish_run(&mut store, run_id, done)
    }
    .await;
    if let Err(error) = &result {
        record_workflow_failure(&mut store, run_id, error)?;
    }
    result
}

/// The run a review follow-up is, created before anything is spawned so the caller can
/// say which run took the comments.
///
/// A run of its own, rooted in the checkout the building run was: it is new work, with its
/// own spend and its own verdict, and the run that built the PR has already been judged.
pub fn open_follow_up(
    store: &mut Store,
    target: &crate::review::FollowUp,
) -> Result<crate::model::Run> {
    let built_by = store.run(target.run_id)?;
    let plan = built_by
        .plan_slug
        .clone()
        .ok_or_else(|| Error::invalid("the run that built this pull request had no plan"))?;
    let workspace = built_by
        .workspace_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| Error::invalid("the run that built this pull request has no checkout"))?;
    let run = store.create_run_in(
        built_by.project_id,
        &format!("Address review comments on {}", target.slice_key),
        RunTrigger::Review,
        Some(&workspace),
    )?;
    store.set_run_plan(run.id, &plan)
}

/// Work a submitted review's comments into the pull request where it was built.
///
/// It does not hold the checkout the way a planning run does - it works in the PR's own
/// worktree - so it runs even while another run is building above it, which is when a
/// person reviewing the first PR is most likely to want it (PW12).
pub async fn follow_up_at(
    db: &Path,
    run_id: i64,
    target: crate::review::FollowUp,
    comments: String,
) -> Result<Orchestration> {
    let mut store = Store::open(db)?;
    let initialization = (|| {
        store.set_run_supervisor(run_id, i64::from(std::process::id()))?;
        let run = store.set_run_status(run_id, RunStatus::Running)?;
        let plan = run
            .plan_slug
            .clone()
            .ok_or_else(|| Error::invalid("the follow-up run has no plan"))?;
        let repo = run
            .workspace_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| Error::invalid("the follow-up run has no checkout"))?
            .canonicalize()
            .map_err(|_| Error::invalid("the checkout that built this no longer exists"))?;
        let team_id = run
            .team_id
            .ok_or_else(|| Error::invalid("the project has no team"))?;
        let project = store.project(run.project_id)?;
        Ok::<_, Error>(Orchestrator {
            db_path: db.to_path_buf(),
            project_dir: store.support_dir(&project.slug)?,
            planner: Planner::at(&repo).for_plan(plan),
            worktrees: Worktrees::at(&repo),
            repo,
            run_id,
            team_id,
            registry: ModelRegistry::load()?,
            parallel_width: 1,
        })
    })();
    let orchestrator = match initialization {
        Ok(orchestrator) => orchestrator,
        Err(error) => {
            record_workflow_failure(&mut store, run_id, &error)?;
            return Err(error);
        }
    };
    let result = match orchestrator.follow_up(&mut store, &target, comments).await {
        Ok(dispatched) => finish_run(
            &mut store,
            run_id,
            Orchestration {
                unrouted: Vec::new(),
                dispatched: vec![dispatched],
            },
        ),
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        record_workflow_failure(&mut store, run_id, error)?;
    }
    result
}

/// Restack one stacked pull request, on the run and node the watch opened for it (PW11).
///
/// Like a follow-up it works in the PR's own worktree and holds nothing above it, so it
/// goes ahead while another run builds in the checkout it was started from.
pub(crate) async fn restack_at(
    db: &Path,
    run_id: i64,
    node_id: i64,
    restack: crate::restack::Restack,
) -> Result<Orchestration> {
    let mut store = Store::open(db)?;
    let initialization = (|| {
        store.set_run_supervisor(run_id, i64::from(std::process::id()))?;
        let run = store.set_run_status(run_id, RunStatus::Running)?;
        let repo = run
            .workspace_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| Error::invalid("the restack has no checkout"))?
            .canonicalize()
            .map_err(|_| Error::invalid("the checkout that built this no longer exists"))?;
        let team_id = run
            .team_id
            .ok_or_else(|| Error::invalid("the project has no team"))?;
        let project = store.project(run.project_id)?;
        Ok::<_, Error>(Orchestrator {
            db_path: db.to_path_buf(),
            project_dir: store.support_dir(&project.slug)?,
            planner: Planner::at(&repo).for_plan(restack.plan.clone()),
            worktrees: Worktrees::at(&repo),
            repo,
            run_id,
            team_id,
            registry: ModelRegistry::load()?,
            parallel_width: 1,
        })
    })();
    let result = match initialization {
        Ok(orchestrator) => match orchestrator.restack(&mut store, &restack, node_id).await {
            Ok(dispatched) => finish_run(
                &mut store,
                run_id,
                Orchestration {
                    unrouted: Vec::new(),
                    dispatched: vec![dispatched],
                },
            ),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        // Not left queued: a restack that never ran would hold the PR's worktree as busy
        // until the process that opened it exited.
        let node = store.node_run(node_id)?;
        if matches!(node.status, NodeStatus::Queued | NodeStatus::Running) {
            store.block_node(node_id, &error.to_string())?;
            store.set_node_status(node_id, NodeStatus::Failed)?;
        }
        record_workflow_failure(&mut store, run_id, error)?;
    }
    result
}

/// Reattach an interrupted maker to the same run, Pi session and awt lease.
///
/// The caller claims `node_run.supervisor_pid` before spawning this future. Failure leaves
/// the node running but unsupervised so the same recovery can be retried without losing
/// either its transcript or uncommitted checkout.
pub async fn resume_interrupted_node_at(
    db: &Path,
    run_id: i64,
    node_id: i64,
) -> Result<Orchestration> {
    let mut store = Store::open(db)?;
    let run = store.run(run_id)?;
    if run.status != RunStatus::Running || run.blocked_reason.is_some() {
        return Err(Error::invalid("that run is no longer available to resume"));
    }
    let node = store.node_run(node_id)?;
    if node.run_id != run_id || node.status != NodeStatus::Running {
        return Err(Error::invalid("that node is no longer available to resume"));
    }
    let team_id = run
        .team_id
        .ok_or_else(|| Error::invalid("the interrupted run has no team snapshot"))?;
    let plan_slug = run
        .plan_slug
        .clone()
        .ok_or_else(|| Error::invalid("the interrupted run has no plan"))?;
    let repo = run
        .workspace_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| Error::invalid("the interrupted run has no workspace"))?
        .canonicalize()
        .map_err(|_| Error::invalid("the interrupted run's workspace no longer exists"))?;
    let project = store.project(run.project_id)?;
    let orchestrator = Orchestrator {
        db_path: db.to_path_buf(),
        project_dir: store.support_dir(&project.slug)?,
        repo: repo.clone(),
        run_id,
        team_id,
        registry: ModelRegistry::load()?,
        parallel_width: usize::try_from(run.parallel_width).unwrap_or(1).max(1),
        planner: Planner::at(&repo).for_plan(plan_slug),
        worktrees: Worktrees::at(&repo),
    };

    match orchestrator.resume_node(&mut store, node_id).await {
        Ok(dispatched) => finish_run(
            &mut store,
            run_id,
            Orchestration {
                unrouted: Vec::new(),
                dispatched: vec![dispatched],
            },
        ),
        Err(error) => {
            let _ = store.release_node_supervision(node_id, i64::from(std::process::id()));
            let _ = store.append_event(
                run_id,
                crate::NewEvent::new(
                    crate::EventKind::Failed,
                    format!("interrupted turn could not resume: {error}"),
                )
                .on_node(node_id)
                .by("ai-team"),
            );
            Err(error)
        }
    }
}

/// Atomically reserve one idle session reset. Orchestrators must have a plan because the
/// reset is forbidden until that plan has a durable ai-planner handoff.
pub fn claim_session_reset(store: &mut Store, run_id: i64, node_id: i64) -> Result<()> {
    let run = store.run(run_id)?;
    let node = store.node_run(node_id)?;
    if node.run_id != run.id {
        return Err(Error::invalid("that node does not belong to that run"));
    }
    if node.role == crate::ROOT_ROLE && run.plan_slug.is_none() {
        return Err(Error::invalid(
            "the orchestrator session has no plan to hand off",
        ));
    }
    store.claim_node_session_reset(node_id)?;
    Ok(())
}

/// Complete a previously claimed reset. Non-orchestrators retire immediately; the
/// orchestrator resumes the exact Pi session and must successfully call write_handoff.
pub async fn reset_claimed_session_at(db: &Path, run_id: i64, node_id: i64) -> Result<()> {
    let mut store = Store::open(db)?;
    let node = store.node_run(node_id)?;
    if node.run_id != run_id || node.session_resetting_at.is_none() {
        return Err(Error::invalid("that session reset is not pending"));
    }
    if node.role != crate::ROOT_ROLE {
        store.retire_node_session(node_id)?;
        let _ = store.append_event(
            run_id,
            crate::NewEvent::new(
                crate::EventKind::Note,
                "session retired; the next turn starts fresh",
            )
            .on_node(node_id)
            .by("ai-team"),
        );
        return Ok(());
    }

    let run = store.run(run_id)?;
    let repo = run
        .workspace_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| Error::invalid("the run has no workspace for its handoff"))?
        .canonicalize()
        .map_err(|_| Error::invalid("the run's workspace no longer exists"))?;
    let team_id = run
        .team_id
        .ok_or_else(|| Error::invalid("the run has no team snapshot"))?;
    let project = store.project(run.project_id)?;
    let plan = run
        .plan_slug
        .clone()
        .ok_or_else(|| Error::invalid("the orchestrator session has no plan to hand off"))?;
    let orchestrator = Orchestrator {
        db_path: db.to_path_buf(),
        project_dir: store.support_dir(&project.slug)?,
        repo: repo.clone(),
        run_id,
        team_id,
        registry: ModelRegistry::load()?,
        parallel_width: usize::try_from(run.parallel_width).unwrap_or(1).max(1),
        planner: Planner::at(&repo).for_plan(plan),
        worktrees: Worktrees::at(&repo),
    };
    if let Err(error) = orchestrator
        .handoff_before_session_reset(&mut store, node_id)
        .await
    {
        store.cancel_node_session_reset(node_id)?;
        let reason = error.to_string();
        store.append_event(
            run_id,
            crate::NewEvent::new(
                crate::EventKind::Note,
                format!("orchestrator session reset refused: {reason}"),
            )
            .on_node(node_id)
            .by("ai-team"),
        )?;
        notify_run(
            &mut store,
            run_id,
            "input_required",
            "Orchestrator reset needs a handoff",
            &reason,
        );
        return Err(error);
    }
    let _ = store.append_event(
        run_id,
        crate::NewEvent::new(
            crate::EventKind::Note,
            "orchestrator handoff saved; the next turn starts fresh",
        )
        .on_node(node_id)
        .by("ai-team"),
    );
    Ok(())
}

/// Attention delivery must not be able to fail the run it describes.
fn notify_run(store: &mut Store, run_id: i64, kind: &str, title: &str, body: &str) {
    let Ok(run) = store.run(run_id) else { return };
    let Ok(project) = store.project(run.project_id) else {
        return;
    };
    let _ = store.notify_once(crate::model::NewNotification {
        dedupe_key: format!("run:{run_id}:{kind}"),
        project_id: run.project_id,
        workspace_path: run.workspace_path,
        run_id: Some(run_id),
        node_run_id: None,
        kind: kind.into(),
        title: format!("{} · {title}", project.name),
        body: body.into(),
        action_path: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_the_schedule_starts_is_the_schedules_not_the_operators() {
        let checkout = tempfile::tempdir().unwrap();
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        let fired = Request {
            project: "widget".into(),
            trigger: Some(RunTrigger::Scheduled),
            ..Request::default()
        };

        let run = open_run(
            &mut store,
            &fired,
            project.id,
            checkout.path(),
            "nightly",
            None,
        )
        .unwrap();
        assert_eq!(run.trigger, RunTrigger::Scheduled);
        assert_eq!(
            store.run_origin(&run).unwrap(),
            crate::model::RunOrigin::Schedule
        );

        store.set_run_status(run.id, RunStatus::Done).unwrap();
        let asked = Request {
            project: "widget".into(),
            ..Request::default()
        };
        let run = open_run(
            &mut store,
            &asked,
            project.id,
            checkout.path(),
            "build it",
            None,
        )
        .unwrap();
        assert_eq!(run.trigger, RunTrigger::Manual);
    }

    #[test]
    fn a_review_follow_up_is_a_run_of_its_own_triggered_by_the_review() {
        let checkout = tempfile::tempdir().unwrap();
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let built = store
            .create_run_in(
                project.id,
                "build PR1",
                RunTrigger::Manual,
                Some(checkout.path()),
            )
            .unwrap();
        let built = store.set_run_plan(built.id, "widget-plan").unwrap();
        let backend = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap();
        let node = store
            .dispatch_task(
                built.id,
                backend.id,
                "PR1",
                Some("T1"),
                &ModelRegistry::local_only(),
            )
            .unwrap();
        let node = store.set_node_status(node.id, NodeStatus::Done).unwrap();
        let target = crate::review::FollowUp {
            run_id: built.id,
            node,
            slice_key: "PR1".into(),
            plan: "widget-plan".into(),
            worktree: PathBuf::from("/work/pr1"),
            branch: "widget-plan/pr1".into(),
        };

        let follow = open_follow_up(&mut store, &target).unwrap();

        // Its own spend and verdict, rooted and planned where the PR was built - and on
        // the record as a review's, which is what started it.
        assert_ne!(follow.id, built.id);
        assert_eq!(follow.trigger, RunTrigger::Review);
        assert_eq!(follow.workspace_path, built.workspace_path);
        assert_eq!(follow.plan_slug.as_deref(), Some("widget-plan"));
        assert_eq!(follow.prompt, "Address review comments on PR1");
    }

    #[test]
    fn an_explicit_prompt_is_planned_even_when_an_existing_plan_has_ready_work() {
        let request = Request {
            project: "widget".into(),
            prompt: Some("a separate task".into()),
            ..Request::default()
        };

        assert!(should_plan(&request, 3));
    }

    #[test]
    fn ticket_and_design_links_make_their_context_mandatory() {
        assert_eq!(
            required_context(
                Some(ProjectSource::ClickUp),
                Some("https://app.clickup.com/t/86abc"),
                "Design: https://www.figma.com/design/key/name?node-id=1-2",
                "build it",
            ),
            vec![ContextSource::ClickUp, ContextSource::Figma]
        );
        assert!(required_context(None, None, "ordinary brief", "ordinary prompt").is_empty());
    }

    #[test]
    fn no_prompt_picks_up_ready_work_without_replanning() {
        let request = Request {
            project: "widget".into(),
            ..Request::default()
        };

        assert!(!should_plan(&request, 3));
        assert!(should_plan(&request, 0));
    }

    #[test]
    fn a_finished_workflow_says_which_slice_stopped_instead_of_blocking_blankly() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::model::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "build it", RunTrigger::Manual)
            .unwrap();
        let done = crate::Orchestration {
            dispatched: vec![crate::Dispatched {
                slice_key: "S1".into(),
                role: "frontend".into(),
                worktree: "/tmp/widget".into(),
                node_run_id: 99,
                status: NodeStatus::Failed,
                outcome: crate::TurnOutcome::default(),
                branch: None,
            }],
            unrouted: Vec::new(),
        };

        finish_run(&mut store, run.id, done).unwrap();

        let blocked = store.run(run.id).unwrap();
        assert_eq!(blocked.status, RunStatus::Blocked);
        assert_eq!(
            blocked.blocked_reason.as_deref(),
            Some("S1 (frontend) failed")
        );
    }

    #[test]
    fn a_dispatch_error_settles_the_run_with_visible_evidence() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::model::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "build it", RunTrigger::Manual)
            .unwrap();
        store.set_run_status(run.id, RunStatus::Running).unwrap();

        record_workflow_failure(
            &mut store,
            run.id,
            &Error::invalid("git fetch origin: Permission denied (publickey)"),
        )
        .unwrap();

        let failed = store.run(run.id).unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(
            failed.blocked_reason.as_deref(),
            Some("git fetch origin: Permission denied (publickey)")
        );
        assert_eq!(
            store.events(run.id, None, 10).unwrap()[0].kind,
            crate::EventKind::Failed
        );
        let notifications = store.notifications(10).unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].kind, "failed");
        assert!(notifications[0].body.contains("Permission denied"));
    }

    #[test]
    fn approval_claim_rolls_back_when_direction_cannot_be_queued() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::model::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let orchestrator = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == crate::ROOT_ROLE)
            .unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        store.set_run_plan(run.id, "widget-plan").unwrap();
        store.block_run(run.id, PLAN_APPROVAL_REASON).unwrap();
        let node = store
            .dispatch(run.id, orchestrator.id, None, &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "session-1").unwrap();
        store
            .db_mut()
            .write(|tx| {
                tx.execute_batch(
                    "CREATE TRIGGER reject_pending BEFORE INSERT ON pending_message
                     BEGIN SELECT RAISE(ABORT, 'test queue failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();

        assert!(claim_plan_approval_with_direction(&mut store, run.id, "Build it").is_err());
        let unchanged = store.run(run.id).unwrap();
        assert_eq!(unchanged.status, RunStatus::Blocked);
        assert_eq!(
            unchanged.blocked_reason.as_deref(),
            Some(PLAN_APPROVAL_REASON)
        );
    }

    #[test]
    fn a_preparing_hold_is_recoverable_only_after_its_lease_is_stale() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::model::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        store.set_run_plan(run.id, "widget-plan").unwrap();
        store
            .block_run(run.id, PLAN_APPROVAL_PREPARING_REASON)
            .unwrap();

        assert!(claim_plan_approval(&mut store, run.id).is_err());
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE run SET updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![run.id, crate::util::rfc3339_in(-10 * 60)],
                )?;
                Ok(())
            })
            .unwrap();
        claim_plan_approval(&mut store, run.id).unwrap();
        assert_eq!(store.run(run.id).unwrap().status, RunStatus::Running);
    }

    #[test]
    fn orchestrator_approval_direction_stays_with_the_existing_run_and_session() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::model::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let orchestrator = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == crate::ROOT_ROLE)
            .unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        store.set_run_plan(run.id, "widget-plan").unwrap();
        store.block_run(run.id, PLAN_APPROVAL_REASON).unwrap();
        let node = store
            .dispatch(run.id, orchestrator.id, None, &ModelRegistry::local_only())
            .unwrap();
        store.set_node_session(node.id, "session-1").unwrap();
        store.set_node_status(node.id, NodeStatus::Done).unwrap();

        claim_plan_approval_with_direction(
            &mut store,
            run.id,
            "The plan looks good. Start building.",
        )
        .unwrap();

        let claimed = store.run(run.id).unwrap();
        assert_eq!(claimed.status, RunStatus::Running);
        assert!(claimed.blocked_reason.is_none());
        assert_eq!(store.waiting_for(orchestrator.id).unwrap(), 1);
        let event = store.events(run.id, None, 20).unwrap().pop().unwrap();
        assert_eq!(event.actor.as_deref(), Some("human"));
        assert_eq!(event.node_run_id, Some(node.id));
    }
}
