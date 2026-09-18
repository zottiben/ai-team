//! `ait run` without `--worktree`: plan, then build the ready slices in parallel.
//!
//! The ordering lives in core's `Orchestrator`. This is the surface: resolve what the
//! run needs, build the generated project once, and report what happened in the terms a
//! human thinks in - slices, seats and worktrees.

use anyhow::{Context, Result};

use ai_team_core as core;
use ai_team_core::{NodeStatus, Planner, RunStatus, RunTrigger, Store, Worktrees};

use crate::cli::RunArgs;

pub(crate) async fn run(args: RunArgs) -> Result<()> {
    let db = core::default_db_path()?;
    let mut store = Store::open(&db).with_context(|| format!("opening {}", db.display()))?;
    let registry = core::ModelRegistry::load().context("loading the machine profile")?;

    let project = store.find_project(&args.project)?;
    let team_id = project
        .team_id
        .with_context(|| format!("{} has no team - run `ait init` first", project.slug))?;
    let team = store.team(team_id)?;

    // The repository is where the plan lives and where the worktree pool is rooted. A
    // project with no repo attached (D6 allows that) has nowhere to lease from, and
    // saying so beats leasing out of whatever directory the shell happened to be in.
    let (repo, planner, worktrees) = neighbours(&store, &project, args.plan.as_deref()).await?;

    // Does the board already have work? A second `ait run` on a plan that is mid-flight
    // should pick that up rather than plan it again: planning twice gives somebody the
    // same slice twice, and the duplicate is only noticed once two agents are building
    // it. `--replan` is how you ask for a new plan anyway.
    let ready = planner.slices().await.map_or(0, |slices| {
        slices
            .into_iter()
            .filter(ai_team_core::Slice::is_dispatchable)
            .count()
    });
    let planning = args.replan || ready == 0;

    let prompt = match (&args.prompt, planning) {
        (Some(prompt), _) => prompt.clone(),
        // Nothing to plan from and nothing ready: there is no work to do either way.
        (None, true) => anyhow::bail!(
            "nothing on the plan is ready to build, so there is nothing to pick up. \
             Give a prompt to plan from."
        ),
        (None, false) => format!("build the {ready} ready slice(s)"),
    };

    let run = store.create_run(project.id, &prompt, RunTrigger::Manual)?;
    println!("run {} on {}", run.id, repo.display());

    // Always regenerate: the team rows are the source of truth (D2).
    let project_dir = store.agents_dir(&project.slug)?;
    let generated = store.generate_project_for_machine(team_id, &project_dir, &registry)?;
    generated.write()?;
    println!("generated {} files", generated.files.len());
    for resolution in &generated.resolutions {
        if let Some(notice) = resolution.notice() {
            println!("  fallback           {notice}");
        }
    }

    // Built once, then started once per lease: the project is the team, and every node
    // in this run is a seat on it.
    let width = args
        .width
        .unwrap_or(usize::try_from(team.guardrails.parallel_width).unwrap_or(2));
    let orchestrator = core::Orchestrator {
        db_path: db.clone(),
        project_dir: project_dir.clone(),
        repo: repo.clone(),
        run_id: run.id,
        team_id,
        registry: registry.clone(),
        required_env: generated.required_env.clone(),
        parallel_width: width,
        planner,
        worktrees,
    };

    let supervisor = orchestrator.builder()?;
    crate::cmd::run::build(&supervisor, &mut store, run.id).await?;

    // --- plan -----------------------------------------------------------------------
    if planning {
        plan(&orchestrator, &mut store, run.id, &prompt).await?;
    } else {
        println!(
            "\n{ready} slice(s) already ready, so planning was skipped.{}",
            if args.prompt.is_some() {
                " Your prompt was not planned from - pass --replan for that."
            } else {
                ""
            }
        );
    }
    if args.plan_only {
        store.set_run_status(run.id, RunStatus::Done)?;
        if planning {
            println!("\nplan written. `aip show` reads it.");
        }
        return Ok(());
    }

    // --- build ----------------------------------------------------------------------
    store.set_run_status(run.id, RunStatus::Running)?;
    if let Some(note) = orchestrator.verifier_note(&store)? {
        // Said before the work rather than after: it changes how much the green at the
        // end is worth, and that is worth knowing up front.
        println!("\nnote: {note}");
        store.append_event(
            run.id,
            core::NewEvent::new(core::EventKind::Note, note).by("orchestrator"),
        )?;
    }
    println!("\ndispatching (width {width})");
    let done = orchestrator
        .build_slices(&mut store, |line| println!("  {line}"))
        .await?;

    report(&mut store, run.id, &done)
}

/// Where the work happens, and the two tools that make it possible.
///
/// Both are checked before anything is created: a run that cannot lease a worktree has
/// nothing to do, and finding that out after the plan is written wastes a turn.
async fn neighbours(
    store: &Store,
    project: &ai_team_core::Project,
    plan: Option<&str>,
) -> Result<(std::path::PathBuf, Planner, Worktrees)> {
    let repo = store
        .project_repos(project.id)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .with_context(|| {
            format!(
                "{} has no checkout attached, so there is nothing to lease worktrees from. \
                 Run `ait init` in the repository, or pass --worktree to run a single turn.",
                project.slug
            )
        })?;
    let repo = std::path::PathBuf::from(repo)
        .canonicalize()
        .context("the project's checkout no longer exists")?;

    let planner = match plan {
        Some(plan) => Planner::at(&repo).for_plan(plan.to_string()),
        None => Planner::at(&repo),
    };
    let worktrees = Worktrees::at(&repo);
    if let Err(reason) = planner.check().await {
        anyhow::bail!("{reason}");
    }
    if let Err(reason) = worktrees.check().await {
        anyhow::bail!("{reason}");
    }
    Ok((repo, planner, worktrees))
}

/// The planning turn: one prompt in, an ai-planner plan out.
async fn plan(
    orchestrator: &core::Orchestrator,
    store: &mut Store,
    run_id: i64,
    prompt: &str,
) -> Result<()> {
    store.set_run_status(run_id, RunStatus::Planning)?;
    println!("\nplanning");
    let outcome = orchestrator
        .plan(store, prompt, |event| {
            if let core::Disposition::Record(kind, summary) = event.classify() {
                println!(
                    "  {:<18} {}",
                    format!("{kind:?}").to_lowercase(),
                    truncate(&summary, 90)
                );
            }
        })
        .await
        .context("the orchestrator could not write a plan")?;

    if core::outcome_status(&outcome) != NodeStatus::Done {
        store.set_run_status(run_id, RunStatus::Failed)?;
        anyhow::bail!("planning did not finish - see `ait db open`");
    }
    Ok(())
}

fn report(store: &mut Store, run_id: i64, done: &core::Orchestration) -> Result<()> {
    for (key, reason) in &done.unrouted {
        println!("  {key} was not dispatched: {reason}");
    }

    if done.dispatched.is_empty() {
        store.set_run_status(run_id, RunStatus::Blocked)?;
        println!("\nNothing was dispatched. `aip slice ls` shows what the plan says.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = done
        .dispatched
        .iter()
        .map(|node| {
            vec![
                node.slice_key.clone(),
                node.role.clone(),
                format!("{:?}", node.status).to_lowercase(),
                // The branch is where the work actually is: the worktree it was built in
                // has already gone back to the pool and been reset.
                node.branch.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    println!();
    crate::cmd::table(&["SLICE", "SEAT", "RESULT", "BRANCH"], &rows);

    let usage = store.run_usage(run_id)?;
    println!(
        "\ntokens  in {} out {} cache-read {} cache-write {}  (billable {})",
        usage.tokens_in,
        usage.tokens_out,
        usage.cache_read,
        usage.cache_write,
        usage.billable()
    );

    // A run is only done when every node it dispatched is. Anything else leaves the
    // board and the database disagreeing about whether there is work left.
    let failed = done
        .dispatched
        .iter()
        .filter(|node| node.status != NodeStatus::Done)
        .count();
    let status = if failed > 0 || !done.unrouted.is_empty() {
        RunStatus::Blocked
    } else {
        RunStatus::Done
    };
    store.set_run_status(run_id, status)?;
    if failed > 0 {
        println!("{failed} node(s) did not finish - see `ait db open`");
    }
    Ok(())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}
