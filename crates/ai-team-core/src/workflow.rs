//! One prompt, all the way through, from wherever it was asked.
//!
//! The terminal and the window both start runs, and the ordering - resolve, generate,
//! build, plan or skip, dispatch - has to be the same either way. Two copies of it drift,
//! and the copy that drifts is always the one nobody demoed today.
//!
//! What differs between callers is only how progress is *shown*: the CLI prints it, the
//! window reads the database the run is already writing to. So progress is a callback,
//! and everything else lives here.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::model::{NodeStatus, RunStatus, RunTrigger};
use crate::neighbours::{Planner, Worktrees};
use crate::store::Store;
use crate::supervise::{BuildProgress, Orchestration, Orchestrator};

/// What was asked for.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// Slug, id, or part of the name.
    pub project: String,
    /// Absent means "build whatever the plan already has ready".
    pub prompt: Option<String>,
    pub plan: Option<String>,
    pub width: Option<usize>,
    /// Plan again even though there is ready work.
    pub replan: bool,
    /// Stop after the plan is written.
    pub plan_only: bool,
}

/// What a caller might want to show while it happens.
#[derive(Debug, Clone)]
pub enum Progress {
    Started {
        run_id: i64,
        repo: PathBuf,
    },
    Generated {
        files: usize,
        fallbacks: Vec<String>,
    },
    Building(BuildProgress),
    Built {
        lines: usize,
    },
    /// The board already had work, so the prompt was not planned from.
    PlanSkipped {
        ready: usize,
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
    let outcome = orchestrator
        .plan(store, prompt, |event| {
            if let crate::Disposition::Record(kind, summary) = event.classify() {
                on_progress(Progress::Note(format!(
                    "{:<18} {summary}",
                    format!("{kind:?}").to_lowercase()
                )));
            }
        })
        .await?;
    if crate::outcome_status(&outcome) != NodeStatus::Done {
        store.set_run_status(run_id, RunStatus::Failed)?;
        return Err(Error::invalid("planning did not finish"));
    }
    Ok(())
}

/// Install and build the generated project, once, before any node starts.
async fn build_once<F>(
    orchestrator: &Orchestrator,
    store: &mut Store,
    run_id: i64,
    on_progress: &mut F,
) -> Result<()>
where
    F: FnMut(Progress) + Send,
{
    let supervisor = orchestrator.builder()?;
    let mut lines = 0usize;
    let mut build_events = Vec::new();
    supervisor
        .install_and_build(|progress| {
            lines += 1;
            build_events.push(progress.clone());
            on_progress(Progress::Building(progress));
        })
        .await?;
    // Recorded after the fact: the callback cannot hold the store, and a build is not
    // interesting enough to interleave transactions with.
    for progress in &build_events {
        crate::record_build_progress(store, run_id, progress)?;
    }
    on_progress(Progress::Built { lines });
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
    parallel_width: usize,
}

/// Everything resolved before a run row exists.
struct Prepared {
    team_id: i64,
    project_slug: String,
    project_id: i64,
    repo: PathBuf,
    planner: Planner,
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
    let (team_id, project_slug, project_id, parallel_width, repo) = {
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
        (team_id, project.slug, project.id, width, repo)
    };
    Ok(Resolved {
        team_id,
        project_slug,
        project_id,
        repo,
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
        parallel_width,
    } = resolved;

    let planner = match &request.plan {
        Some(plan) => Planner::at(&repo).for_plan(plan.clone()),
        None => Planner::at(&repo),
    };
    let worktrees = Worktrees::at(&repo);
    // Both checked before anything is created: a run that cannot lease a worktree has
    // nothing to do, and finding that out after the plan is written wastes a turn.
    planner.check().await.map_err(Error::invalid)?;
    worktrees.check().await.map_err(Error::invalid)?;

    // Does the board already have work? A second run on a plan that is mid-flight picks
    // that up rather than planning it again - planning twice gives somebody the same
    // slice twice, and the duplicate only shows up once two agents are building it.
    let ready = planner.slices().await.map_or(0, |slices| {
        slices
            .into_iter()
            .filter(crate::Slice::is_dispatchable)
            .count()
    });
    let planning = request.replan || ready == 0;

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

    Ok(Prepared {
        team_id,
        project_slug,
        project_id,
        repo,
        planner,
        worktrees,
        prompt,
        planning,
        ready,
        parallel_width: request.width.unwrap_or(parallel_width),
    })
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
        planner,
        worktrees,
        prompt,
        planning,
        ready,
        parallel_width: width,
    } = prepare(resolve(&store, request)?, request).await?;

    let run = store.create_run(project_id, &prompt, RunTrigger::Manual)?;
    on_progress(Progress::Started {
        run_id: run.id,
        repo: repo.clone(),
    });

    // Always regenerate: the team rows are the source of truth (D2).
    let project_dir = store.agents_dir(&project_slug)?;
    let generated = store.generate_project_for_machine(team_id, &project_dir, &registry)?;
    generated.write()?;
    on_progress(Progress::Generated {
        files: generated.files.len(),
        fallbacks: generated
            .resolutions
            .iter()
            .filter_map(crate::ModelResolution::notice)
            .collect(),
    });

    let orchestrator = Orchestrator {
        db_path: db.clone(),
        project_dir,
        repo,
        run_id: run.id,
        team_id,
        registry,
        required_env: generated.required_env.clone(),
        parallel_width: width,
        planner,
        worktrees,
    };

    build_once(&orchestrator, &mut store, run.id, &mut on_progress).await?;

    if planning {
        plan_it(&orchestrator, &mut store, run.id, &prompt, &mut on_progress).await?;
    } else {
        on_progress(Progress::PlanSkipped { ready });
    }

    if request.plan_only {
        store.set_run_status(run.id, RunStatus::Done)?;
        return Ok(Orchestration::default());
    }

    store.set_run_status(run.id, RunStatus::Running)?;
    if let Some(note) = orchestrator.verifier_note(&store)? {
        on_progress(Progress::Caution(note.clone()));
        store.append_event(
            run.id,
            crate::NewEvent::new(crate::EventKind::Note, note).by("orchestrator"),
        )?;
    }
    on_progress(Progress::Dispatching { width });

    let done = orchestrator
        .build_slices(&mut store, |line| {
            if let Some((slice, role)) = line.split_once(" -> ") {
                on_progress(Progress::Routed {
                    slice: slice.to_string(),
                    role: role.to_string(),
                });
            }
        })
        .await?;

    // A run is only done when everything it dispatched is. Anything else leaves the
    // board and the database disagreeing about whether there is work left.
    let unfinished = done
        .dispatched
        .iter()
        .filter(|node| node.status != NodeStatus::Done)
        .count();
    store.set_run_status(
        run.id,
        if unfinished > 0 || !done.unrouted.is_empty() || done.dispatched.is_empty() {
            RunStatus::Blocked
        } else {
            RunStatus::Done
        },
    )?;
    Ok(done)
}
