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
        .with_context(|| "the project's checkout no longer exists")?;

    let planner = match &args.plan {
        Some(plan) => Planner::at(&repo).for_plan(plan.clone()),
        None => Planner::at(&repo),
    };
    let worktrees = Worktrees::at(&repo);
    if let Err(reason) = planner.check().await {
        anyhow::bail!("{reason}");
    }
    if let Err(reason) = worktrees.check().await {
        anyhow::bail!("{reason}");
    }

    let run = store.create_run(project.id, &args.prompt, RunTrigger::Manual)?;
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
    store.set_run_status(run.id, RunStatus::Planning)?;

    // --- plan -----------------------------------------------------------------------
    println!("\nplanning");
    let outcome = orchestrator
        .plan(&mut store, &args.prompt, |event| {
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
        store.set_run_status(run.id, RunStatus::Failed)?;
        anyhow::bail!("planning did not finish - see `ait db open`");
    }
    if args.plan_only {
        store.set_run_status(run.id, RunStatus::Done)?;
        println!("\nplan written. `aip show` reads it.");
        return Ok(());
    }

    // --- build ----------------------------------------------------------------------
    store.set_run_status(run.id, RunStatus::Running)?;
    println!("\ndispatching (width {width})");
    let done = orchestrator
        .build_slices(&mut store, |line| println!("  {line}"))
        .await?;

    report(&mut store, run.id, &done)
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
