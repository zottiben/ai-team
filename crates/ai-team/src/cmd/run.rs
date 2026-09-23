//! `ait run` - one prompt, through a supervised Pi process, into the database.
//!
//! This is the single-node path: generate if needed, install, build, start, drive one
//! turn, and report what it spent. It is what `--worktree` selects; without it, `run`
//! hands over to `orchestrate`, which plans and then dispatches a node per slice.

use std::path::PathBuf;

use anyhow::{Context, Result};

use ai_team_core as core;
use ai_team_core::{EventKind, NodeStatus, RunStatus, RunTrigger, Store};

use crate::cli::RunArgs;

pub(crate) async fn run(args: RunArgs) -> Result<()> {
    // Two shapes, and the flag says which: name a worktree and one seat takes one turn
    // in it; name none and the orchestrator plans, then every ready slice is built in a
    // worktree of its own.
    match args.worktree.clone() {
        Some(worktree) => single_node(args, PathBuf::from(worktree)).await,
        None => crate::cmd::orchestrate::run(args).await,
    }
}

async fn single_node(args: RunArgs, worktree: PathBuf) -> Result<()> {
    // One seat, one turn, one directory: there is no plan to pick work up from, so the
    // prompt is the whole instruction and it has to be there.
    let prompt = args
        .prompt
        .clone()
        .context("say what to do: `ait run -p <project> --worktree <dir> \"…\"`")?;
    let db = core::default_db_path()?;
    let mut store = Store::open(&db).with_context(|| format!("opening {}", db.display()))?;
    let registry = core::ModelRegistry::load().context("loading the machine profile")?;

    let project = store.find_project(&args.project)?;
    let team_id = project
        .team_id
        .with_context(|| format!("{} has no team - run `ait init` first", project.slug))?;

    let worktree = worktree
        .canonicalize()
        .with_context(|| format!("{} does not exist", worktree.display()))?;

    let run = store.create_run_in(project.id, &prompt, RunTrigger::Manual, Some(&worktree))?;
    println!("run {} in {}", run.id, worktree.display());

    // Nothing to generate and nothing to build (D20). The seat is a set of flags, and
    // the only thing written is the guard and this seat's MCP config - outside the lease,
    // because a guard a node can edit is not a guard.
    let support = store.support_dir(&project.slug)?;

    store.set_run_status(run.id, RunStatus::Running)?;
    let orchestrator = store
        .agents(team_id)?
        .into_iter()
        .find(|a| a.role == core::ROOT_ROLE)
        .context("the team has no orchestrator")?;
    let team = store.team(team_id)?;
    let roster = store.agents(team_id)?;
    let (effective, resolutions) = registry.resolve_agents(std::slice::from_ref(&orchestrator))?;
    for resolution in &resolutions {
        if let Some(notice) = resolution.notice() {
            println!("  fallback           {notice}");
        }
    }
    let resolved = effective.first().unwrap_or(&orchestrator);

    let node = store.dispatch(run.id, orchestrator.id, None, &registry)?;
    store.attach_worktree(node.id, &worktree.to_string_lossy(), None, None)?;
    store.set_node_status(node.id, NodeStatus::Running)?;

    // The repository's own rules reach this turn too. A direct turn has no slice, so
    // nothing narrows which directory it might edit and every nested file applies -
    // and it is the one path where a seat is doing exactly what a person just asked,
    // which is when being held to the repo's standards matters most.
    let house = core::read_house_rules_for(&worktree, &[]);
    let prompt = format!("{prompt}{}", core::house_section(&house));

    let turn = core::PiSeat {
        agent: &orchestrator,
        provider: resolved.provider,
        model: &resolved.model,
        worktree: &worktree,
        support: &support,
        sources: &registry.context_sources(),
        // No plan behind a single-node run, so the planning tools stay absent rather than
        // pointing at whichever plan the working directory resolves to.
        plan: None,
        team: &team,
        roster: &roster,
    }
    .turn(prompt)?;

    let (session, outcome) = core::run_pi_turn(&mut store, node.id, &turn, |event| {
        if let ai_team_core::PiDisposition::Record(kind, summary) = event.classify() {
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
    let status = core::outcome_status(outcome);
    store.set_node_status(node_run_id, status)?;
    store.set_run_status(
        run_id,
        match status {
            NodeStatus::Done => RunStatus::Done,
            NodeStatus::Parked => RunStatus::Blocked,
            NodeStatus::Cancelled => RunStatus::Cancelled,
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

fn truncate(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max.saturating_sub(1)).collect::<String>() + "\u{2026}"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_model_failure_cannot_be_reported_as_done() {
        let failed = core::TurnOutcome {
            terminal: Some(core::TerminalState::Failed),
            ..Default::default()
        };
        assert_eq!(core::outcome_status(&failed), NodeStatus::Failed);

        let completed = core::TurnOutcome {
            terminal: Some(core::TerminalState::Completed),
            ..Default::default()
        };
        assert_eq!(core::outcome_status(&completed), NodeStatus::Done);

        let cancelled = core::TurnOutcome {
            terminal: Some(core::TerminalState::Cancelled),
            ..Default::default()
        };
        assert_eq!(core::outcome_status(&cancelled), NodeStatus::Cancelled);
    }
}
