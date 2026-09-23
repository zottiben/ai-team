//! `ait run` without `--worktree`: plan, then build the ready slices in parallel.
//!
//! The ordering lives in core's `run_workflow`, because the window starts runs too and
//! two copies of that sequence would drift. This is the terminal's view of it: turn each
//! step into a line, and report what came back in the terms a human thinks in - slices,
//! seats and branches.

use anyhow::{Context, Result};

use ai_team_core as core;
use ai_team_core::{NodeStatus, Store};

use crate::cli::RunArgs;

pub(crate) async fn run(args: RunArgs) -> Result<()> {
    let request = core::Request {
        project: args.project.clone(),
        prompt: args.prompt.clone(),
        workspace: None,
        plan: args.plan.clone(),
        width: args.width,
        replan: args.replan,
        plan_only: args.plan_only,
        approval_required: false,
    };

    let done = core::run_workflow(&request, report)
        .await
        .context("running the workflow")?;

    if args.plan_only {
        println!("\nplan written. `aip show` reads it.");
        return Ok(());
    }

    let db = core::default_db_path()?;
    let mut store = Store::open(&db)?;
    summarise(&mut store, &done)
}

fn report(progress: core::Progress) {
    match progress {
        core::Progress::Started { run_id, repo } => {
            println!("run {run_id} on {}", repo.display());
        }
        core::Progress::Planning => {
            println!("\nplanning");
        }
        core::Progress::Seated { fallbacks } => {
            for notice in fallbacks {
                println!("  fallback           {notice}");
            }
        }
        core::Progress::PlanSkipped { ready } => {
            println!("\n{ready} slice(s) already ready, so planning was skipped.");
        }
        core::Progress::Note(line) => println!("  {}", truncate(&line, 108)),
        core::Progress::Caution(note) => println!("\nnote: {note}"),
        core::Progress::Dispatching { width } => println!("\ndispatching (width {width})"),
        core::Progress::Routed { slice, role } => println!("  {slice} -> {role}"),
    }
}

fn summarise(store: &mut Store, done: &core::Orchestration) -> Result<()> {
    for (key, reason) in &done.unrouted {
        println!("  {key} was not dispatched: {reason}");
    }
    if done.dispatched.is_empty() {
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

    if let Some(node) = done.dispatched.first() {
        let usage = store.run_usage(store.node_run(node.node_run_id)?.run_id)?;
        println!(
            "\ntokens  in {} out {} cache-read {} cache-write {}  (billable {})",
            usage.tokens_in,
            usage.tokens_out,
            usage.cache_read,
            usage.cache_write,
            usage.billable()
        );
    }

    let failed = done
        .dispatched
        .iter()
        .filter(|node| node.status != NodeStatus::Done)
        .count();
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
