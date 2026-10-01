//! The first real team phase: sequential read-only Pi seats, then a durable human pause.
//! No standalone planner, lease, build, or implicit approval is reachable from this driver.

use std::path::Path;
use std::time::Duration;

use super::TeamControl;
use crate::planning::PlanActor;
use crate::{ChatTeamPhase, Error, ModelRegistry, NodeStatus, Result, RunStatus, Store};

/// Drive a newly reserved team turn through grounding and embedded planning. The chat
/// remains owned while awaiting approval; neither settled Pi node is kept Running.
/// Recovery and the approved-build driver are separate operations, not a replay here.
pub async fn drive_chat_team_planning(db: &Path, chat_id: i64, node_id: i64) -> Result<()> {
    let mut store = Store::open(db)?;
    let (mut control, ownership) = store.claim_chat_team_planning(chat_id, node_id)?;
    let result = ownership.track(plan(&mut store, &mut control)).await;
    let (phase, reason) = match &result {
        Ok(()) => (ChatTeamPhase::AwaitingApproval, "Review this chat's plan and questions in Overview. No build has been approved or started.".to_string()),
        Err(error) => (ChatTeamPhase::Blocked, error.to_string()),
    };
    let parked = store
        .park_chat_team_planning(&mut control, phase, &reason)
        .map_err(|cleanup| {
            Error::invalid(format!("{reason}; pausing the team also failed: {cleanup}"))
        })?;
    if parked == ChatTeamPhase::Finished {
        Ok(())
    } else {
        result
    }
}

async fn plan(store: &mut Store, control: &mut TeamControl) -> Result<()> {
    let chat = store.chat(control.chat_id)?;
    let project = store.project(chat.project_id)?;
    let repo = store
        .project_repos(project.id)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .ok_or_else(|| Error::invalid("this chat's project has no checkout"))?;
    let workspace = crate::Worktrees::at(repo)
        .resolve_until(
            Path::new(&chat.workspace_path),
            until_stopped(Store::open(store.path())?, *control, control.node_id),
        )
        .await?;
    if !crate::same_worktree(&workspace.to_string_lossy(), &chat.workspace_path) {
        return Err(Error::invalid("this chat's checkout changed"));
    }
    let run = store.run(control.run_id)?;
    let context = store.chat_turn_context(control.chat_id, control.node_id)?;
    let prompt = format!("{context}Ground this operator request for the planner. Produce a delegation brief, not source changes.\n\nOperator request:\n{}", run.prompt);
    let brief = take_turn(store, control, control.node_id, prompt).await?;
    if brief.trim().is_empty() {
        return Err(Error::invalid(
            "the coordinator produced no delegation brief",
        ));
    }
    store.start_chat_team_planner(control)?;
    if let Some(limit) = crate::run_may_continue(store, run.id)? {
        return Err(Error::invalid(limit.reason));
    }
    let planner = store
        .agents(
            run.team_id
                .ok_or_else(|| Error::invalid("this execution's team no longer exists"))?,
        )?
        .into_iter()
        .find(|agent| agent.role == "planner" && agent.enabled && agent.read_only)
        .ok_or_else(|| Error::invalid("this team needs an enabled read-only planner"))?;
    let node = store.dispatch(run.id, planner.id, None, &ModelRegistry::load()?)?;
    store.attach_worktree(node.id, &chat.workspace_path, None, None)?;
    store.set_node_status(node.id, NodeStatus::Running)?;
    let prompt = format!("Use only this chat's embedded plan. Read it first, preserving previous work; create it if absent. Turn the grounded brief into scoped slices for human review. Do not build, lease, publish or dispatch.\n\nOperator request:\n{}\n\nCoordinator brief:\n{brief}", run.prompt);
    take_turn(store, control, node.id, prompt).await?;
    store.check_chat_team_control(control)?;
    let plan = store.chat_plan(chat.id, PlanActor::Human)?;
    if plan.bundle.is_none_or(|bundle| bundle.slices.is_empty()) {
        return Err(Error::invalid(
            "the planner produced no reviewable slices in this chat's plan",
        ));
    }
    Ok(())
}

async fn take_turn(
    store: &mut Store,
    control: &TeamControl,
    node: i64,
    prompt: String,
) -> Result<String> {
    if let Some(reason) = stop_reason(store, control, node)? {
        return Err(Error::invalid(reason));
    }
    let turn = crate::pi::chat_team_planning_turn(
        store,
        control.chat_id,
        node,
        &ModelRegistry::load()?,
        prompt,
    )?;
    let stop = until_stopped(Store::open(store.path())?, *control, node);
    let mut said = String::new();
    let mut context_failure = None;
    let result = crate::pi::run_until(
        store,
        node,
        &turn,
        |event| {
            if let Some(message) = event.assistant_message() {
                said = message;
            }
            if let Some(failure) = event.context_tool_failure() {
                context_failure = Some(failure);
            }
        },
        stop,
    )
    .await;
    let outcome = match result {
        Ok((_, outcome)) => outcome,
        Err(error) => {
            settle_attempt(
                store,
                control,
                node,
                NodeStatus::Failed,
                Some(&error.to_string()),
            )?;
            return Err(error);
        }
    };
    let mut status = crate::outcome_status(&outcome);
    let reason = context_failure
        .or_else(|| said.contains("CONTEXT_UNAVAILABLE:").then(|| said.clone()))
        .or_else(|| {
            (status != NodeStatus::Done).then(|| {
                outcome
                    .provider_message
                    .unwrap_or_else(|| "Pi ended without completing this planning turn".into())
            })
        });
    if reason.is_some() && status == NodeStatus::Done {
        status = NodeStatus::Failed;
    }
    settle_attempt(store, control, node, status, reason.as_deref())?;
    if let Some(reason) = reason {
        return Err(Error::invalid(reason));
    }
    Ok(said)
}

fn settle_attempt(
    store: &mut Store,
    control: &TeamControl,
    node: i64,
    status: NodeStatus,
    reason: Option<&str>,
) -> Result<()> {
    store
        .settle_chat_team_member(control, node, status, reason)
        .map_err(|cleanup| {
            Error::invalid(format!(
                "{}; settling the planning attempt also failed: {cleanup}",
                reason.unwrap_or("The planning turn finished")
            ))
        })
}

async fn until_stopped(observer: Store, control: TeamControl, node: i64) -> String {
    loop {
        match stop_reason(&observer, &control, node) {
            Ok(Some(reason)) => return reason,
            Err(error) => return format!("Stopped because team supervision failed: {error}"),
            Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

fn stop_reason(store: &Store, control: &TeamControl, node: i64) -> Result<Option<String>> {
    store.check_chat_team_control(control)?;
    if store.chat(control.chat_id)?.stop_requested {
        return Ok(Some(
            "Stopped by you. The chat, plan and working files are kept.".into(),
        ));
    }
    if store.run(control.run_id)?.status != RunStatus::Running {
        return Ok(Some("This team run is no longer running.".into()));
    }
    Ok(crate::node_may_continue(store, node)?.map(|limit| limit.reason))
}
