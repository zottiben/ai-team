//! `ait run` - one prompt, through a supervised eve process, into the database.
//!
//! This is the single-node path: generate if needed, install, build, start, drive one
//! turn, and report what it spent. The orchestrator that plans and dispatches *several*
//! nodes across worktrees is M2-S8; everything it will need is already here.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};

use ai_team_core as core;
use ai_team_core::{EveEnv, EventKind, NodeStatus, RunStatus, RunTrigger, Store, Supervisor};

use crate::cli::RunArgs;

pub(crate) async fn run(args: RunArgs) -> Result<()> {
    let db = core::default_db_path()?;
    let mut store = Store::open(&db).with_context(|| format!("opening {}", db.display()))?;

    let project = store.find_project(&args.project)?;
    let team_id = project
        .team_id
        .with_context(|| format!("{} has no team - run `ait init` first", project.slug))?;

    // The worktree. M2-S8 leases one per slice with `awt get --lease`; a single-node run
    // works in a directory the caller names, defaulting to where they are standing.
    let worktree = match &args.worktree {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().context("reading the current directory")?,
    };
    let worktree = worktree
        .canonicalize()
        .with_context(|| format!("{} does not exist", worktree.display()))?;

    let run = store.create_run(project.id, &args.prompt, RunTrigger::Manual)?;
    println!("run {} in {}", run.id, worktree.display());

    // Always regenerate: the team rows are the source of truth, and a stale project is
    // how an agent ends up running the model you changed an hour ago (D2).
    let project_dir = store.agents_dir(&project.slug)?;
    let generated = store.generate_project(team_id, &project_dir)?;
    generated.write()?;
    println!("generated {} files", generated.files.len());

    let env = EveEnv {
        worktree: worktree.clone(),
        token: core::mint_token(),
        provider_keys: provider_keys(&generated.required_env),
    };
    let supervisor = Supervisor::new(&project_dir, env);

    build(&supervisor, &mut store, run.id).await?;

    // --- start ----------------------------------------------------------------------
    let mut supervisor = supervisor;
    let client = supervisor.start().await.context("starting the agent")?;
    let port = supervisor.port().unwrap_or(0);
    println!("  serving on 127.0.0.1:{port}");

    let info = client.info().await?;
    if info.discovery_errors > 0 {
        anyhow::bail!("eve reported {} discovery error(s)", info.discovery_errors);
    }
    println!("  tools {:?}", info.tools);
    println!("  subagents {:?}", info.subagents);

    // --- drive ----------------------------------------------------------------------
    store.set_run_status(run.id, RunStatus::Running)?;
    let orchestrator = store
        .agents(team_id)?
        .into_iter()
        .find(|a| a.role == core::ROOT_ROLE)
        .context("the team has no orchestrator")?;
    let node = store.dispatch(run.id, orchestrator.id, None)?;
    store.attach_worktree(node.id, &worktree.to_string_lossy(), None, None)?;
    store.set_node_status(node.id, NodeStatus::Running)?;

    let (session, outcome) = core::run_turn(&mut store, node.id, &client, &args.prompt, |event| {
        if let ai_team_core::Disposition::Record(kind, summary) = event.classify() {
            // Only the events that become rows are printed, so the terminal shows what
            // the database will hold rather than the transport underneath it.
            println!(
                "  {:<18} {}",
                format!("{kind:?}").to_lowercase(),
                truncate(&summary, 90)
            );
        }
    })
    .await
    .context("driving the turn")?;

    settle(&mut store, run.id, node.id, &session, &outcome)?;

    supervisor.stop().await?;
    Ok(())
}

/// Record how the turn ended, and tell the human what it cost and what it is waiting on.
fn settle(
    store: &mut Store,
    run_id: i64,
    node_run_id: i64,
    session: &str,
    outcome: &core::TurnOutcome,
) -> Result<()> {
    // Parked outranks finished: a turn that asked a question and then hit a terminal
    // event is still waiting on a person, and calling it done would strand it.
    let status = if outcome.approvals.is_empty() {
        if outcome.finished {
            NodeStatus::Done
        } else {
            NodeStatus::Failed
        }
    } else {
        NodeStatus::Parked
    };
    store.set_node_status(node_run_id, status)?;
    store.set_run_status(
        run_id,
        match status {
            NodeStatus::Done => RunStatus::Done,
            NodeStatus::Parked => RunStatus::Blocked,
            _ => RunStatus::Failed,
        },
    )?;

    let node = store.node_run(node_run_id)?;
    println!("\nsession {session}");
    println!(
        "  {} events, {} steps, cursor {}",
        outcome.recorded, outcome.steps, node.stream_cursor
    );
    println!(
        "  tokens  in {} out {} cache-read {} cache-write {}  (billable {})",
        node.usage.tokens_in,
        node.usage.tokens_out,
        node.usage.cache_read,
        node.usage.cache_write,
        node.usage.billable()
    );

    if !outcome.approvals.is_empty() {
        println!("\nparked, waiting on you:");
        for approval in &outcome.approvals {
            let options: Vec<&str> = approval.options.iter().map(|o| o.id.as_str()).collect();
            println!(
                "  [{}] {} {options:?}",
                approval.request_id, approval.prompt
            );
        }
    }

    let failures = store
        .node_events(node_run_id, 500)?
        .into_iter()
        .filter(|e| e.kind == EventKind::Failed)
        .count();
    if failures > 0 {
        println!("\n{failures} failure event(s) - see `ait db open`");
    }
    Ok(())
}

/// Install, build, and report every line as it happens.
///
/// The bar is a spinner with a line count rather than a percentage: neither npm nor eve
/// offers a total, and inventing one would be a progress bar that lies.
async fn build(supervisor: &Supervisor, store: &mut Store, run_id: i64) -> Result<()> {
    let started = Instant::now();
    let mut lines = 0usize;
    let mut build_events = Vec::new();
    // A carriage return only redraws on a terminal. Piped to a file or a CI log it runs
    // every line together into one unreadable smear, so the fallback is periodic.
    let redraw = std::io::stdout().is_terminal();

    supervisor
        .install_and_build(|progress| {
            lines += 1;
            build_events.push(progress.clone());
            if redraw {
                print!(
                    "\r  {} {:>5} lines  {:.0}s  {:<50}",
                    progress.phase.as_str(),
                    lines,
                    started.elapsed().as_secs_f32(),
                    truncate(&progress.line, 50)
                );
                let _ = std::io::stdout().flush();
            } else if lines.is_multiple_of(25) {
                println!("  {} {lines} lines", progress.phase.as_str());
            }
        })
        .await
        .context("building the agent project")?;
    if redraw {
        print!("\r");
    }
    println!(
        "  built in {:.0}s ({lines} lines){:30}",
        started.elapsed().as_secs_f32(),
        ""
    );

    // Recorded after the fact rather than inside the callback: the callback cannot hold
    // the store, and a build is not interesting enough to interleave transactions with.
    for progress in &build_events {
        core::record_build_progress(store, run_id, progress)?;
    }
    Ok(())
}

/// Read the provider keys a generated project asked for out of the environment.
///
/// Missing ones are passed through as empty rather than refused here: the machine
/// profile is what decides whether a provider is allowed (D8, M1-S5), and failing at
/// this layer would pre-empt it with a worse error message.
fn provider_keys(required: &[&'static str]) -> Vec<(String, String)> {
    required
        .iter()
        .filter(|key| key.ends_with("_KEY"))
        .map(|key| {
            let value = std::env::var(key).ok().unwrap_or_else(|| ailocal_key(key));
            ((*key).to_string(), value)
        })
        .collect()
}

/// ai-local writes its gateway key to a known path, so the common case needs no
/// environment variable at all. M1-S5 generalises this into the model registry.
fn ailocal_key(key: &str) -> String {
    if key != "AI_TEAM_AILOCAL_KEY" {
        return String::new();
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/ailocal/gateway.key"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|key| key.trim().to_string())
        .unwrap_or_default()
}

fn truncate(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max.saturating_sub(1)).collect::<String>() + "\u{2026}"
}
