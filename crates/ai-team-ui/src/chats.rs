//! Chat-scoped commands. Never infer an execution target from the project's latest run.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::ActivityEvent;
use crate::error::{Error, Result};
use crate::state::AppState;
use ai_team_core::{Chat, ChatTurn, ModelRegistry, NewChat, NodeStatus, Provider, Reasoning};

// Runs inside auth. A database may have appeared since bind, but an anonymous GET
// must not attach it or initiate recovery. This never creates a missing database.
pub(crate) async fn recover_on_attach(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !crate::updates::is_update(&request) {
        state.recover_chats().await;
    }
    next.run(request).await
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/chat-today", get(today))
        .route("/chats", get(list).post(create))
        .route("/chats/{id}", get(detail).patch(edit))
        .route("/chats/{id}/events", get(events))
        .route("/chats/{id}/messages", post(send))
        .route("/chats/{id}/followups", post(queue_followup))
        .route("/chats/{id}/followups/cancel", post(cancel_followup))
        .route("/chats/{id}/followups/send", post(send_followup))
        .route("/chats/{id}/stop", post(stop))
        .route("/chats/{id}/resume", post(resume))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectQuery {
    project: String,
    #[serde(default)]
    include_archived: bool,
}

async fn today(State(state): State<AppState>) -> Result<Json<ai_team_core::chat_today::ChatToday>> {
    Ok(Json(state.store()?.lock().chat_today()?))
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<Chat>>> {
    let store = state.store()?;
    let store = store.lock();
    let project = store.find_project(&query.project)?;
    Ok(Json(store.chats_including_archived(
        project.id,
        query.include_archived,
    )?))
}

#[derive(Deserialize)]
struct Create {
    project: String,
    workspace: Option<String>,
    provider: Provider,
    model: String,
    reasoning: Reasoning,
    #[serde(default)]
    mode: ai_team_core::ChatMode,
}

async fn create(State(state): State<AppState>, Json(input): Json<Create>) -> Result<Json<Chat>> {
    let workspace =
        crate::api::worktree_for(&state, &input.project, None, input.workspace.as_deref()).await?;
    if input.workspace.is_some() {
        let (db, root) = {
            let store = state.store()?;
            let store = store.lock();
            (
                store.path().to_path_buf(),
                ai_team_core::chat_workspaces::project_repository(
                    &store,
                    store.find_project(&input.project)?.id,
                )?,
            )
        };
        ai_team_core::chat_workspaces::validate_owned(&db, &root, &workspace).await?;
    }
    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&input.project)?;
    Ok(Json(store.create_chat_in_mode(
        NewChat {
            project_id: project.id,
            workspace,
            provider: input.provider,
            model: input.model,
            reasoning: input.reasoning,
        },
        input.mode,
    )?))
}

#[derive(Serialize)]
struct Detail {
    #[serde(flatten)]
    chat: Chat,
    turns: Vec<ChatTurn>,
    followups: Vec<ai_team_core::ChatFollowup>,
    team_builds: Vec<crate::chat_teams::Build>,
    state: &'static str,
    can_resume: bool,
    orphan_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    team_recovery: Option<ai_team_core::ChatRecoveryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_error: Option<String>,
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Detail>> {
    let store = state.store()?;
    let store = store.lock();
    let chat = store.chat(id)?;
    let team_recovery = store.chat_team_recovery_scan(Some(id))?.into_iter().next();
    let turns = store.chat_turns(id)?;
    let active = chat
        .active_node_id
        .map(|node| store.node_run(node))
        .transpose()?;
    let supervised = active
        .as_ref()
        .is_some_and(|node| chat.supervisor_alive(node));
    let orphan_running = !supervised && active.as_ref().is_some_and(|node| chat.pi_alive(node));
    let team = turns
        .iter()
        .filter_map(|turn| turn.team.as_ref())
        .find(|team| Some(team.control_node_id) == chat.active_node_id);
    let team_builds = turns
        .iter()
        .filter_map(|turn| turn.team.clone())
        .map(|execution| {
            Ok(crate::chat_teams::Build {
                slices: store.chat_build_slices(execution.run_id)?,
                closure: store.chat_build_closure(execution.run_id)?,
                execution,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let status = if let Some(team) = team {
        use ai_team_core::{ChatRecoveryState, ChatTeamPhase};
        if chat.stop_requested && team.supervisor_alive() {
            "stopping"
        } else if team_recovery.as_ref().is_some_and(|entry| {
            matches!(
                entry.state,
                ChatRecoveryState::Recoverable | ChatRecoveryState::NeedsInspection
            )
        }) {
            "team_interrupted"
        } else {
            match team.phase {
                ChatTeamPhase::AwaitingApproval => "awaiting_approval",
                ChatTeamPhase::Blocked => "team_blocked",
                ChatTeamPhase::Grounding | ChatTeamPhase::Planning | ChatTeamPhase::Building => {
                    "running"
                }
                ChatTeamPhase::Finished => "idle",
            }
        }
    } else {
        match active {
            Some(_) if !supervised => "interrupted",
            Some(_) if chat.stop_requested => "stopping",
            Some(_) => "running",
            None => match turns.last().map(|turn| turn.node.status) {
                None => "empty",
                Some(NodeStatus::Failed) => "failed",
                Some(NodeStatus::Cancelled) => "stopped",
                _ => "idle",
            },
        }
    };
    Ok(Json(Detail {
        followups: store.chat_followups(id)?,
        chat,
        turns,
        team_builds,
        state: status,
        can_resume: team_recovery.is_none() && status == "interrupted" && !orphan_running,
        orphan_running,
        team_recovery,
        recovery_error: state.chat_recovery_error(),
    }))
}

#[derive(Deserialize)]
struct Edit {
    title: Option<String>,
    archived: Option<bool>,
}

async fn edit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Edit>,
) -> Result<Json<Chat>> {
    let store = state.store()?;
    let mut store = store.lock();
    if input.title.is_some() == input.archived.is_some() {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "change either the title or archived state",
        )));
    }
    if let Some(title) = input.title {
        return Ok(Json(store.rename_chat(id, &title)?));
    }
    Ok(Json(
        store.archive_chat(id, input.archived.unwrap_or(false))?,
    ))
}

#[derive(Deserialize)]
struct Events {
    #[serde(default)]
    after: i64,
}

async fn events(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<Events>,
) -> Result<Json<Vec<ActivityEvent>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(
        store
            .chat_events(id, query.after, 500)?
            .into_iter()
            .map(ActivityEvent::from)
            .collect(),
    ))
}

#[derive(Deserialize)]
struct SendRequest {
    message: String,
    request_id: String,
    #[serde(default)]
    workspace_epoch: i64,
}

async fn send(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<SendRequest>,
) -> Result<Json<ai_team_core::ChatSubmission>> {
    let db = state.database_path()?;
    ai_team_core::recover_abandoned_chat_team(&db, id).await?;
    // The only place push authority is ever minted. A schedule, a queued instruction, a
    // recovered prompt or an agent's own text is also a `run.prompt`, and none of them is
    // a person sending a message here.
    if let Some(receipt) =
        ai_team_core::Store::open(&db)?.replay_chat_push(id, &input.request_id, &input.message)?
    {
        return Ok(Json(receipt));
    }
    let authority = push_authority(&db, id, &input).await?;
    if let ai_team_core::chat_push::Authority::Team(target, _) = authority {
        return Ok(Json(publish_team(db, id, input, target).await?));
    }
    let registry = ModelRegistry::load()?;
    let receipt = {
        let store = state.store()?;
        let mut store = store.lock();
        if let ai_team_core::chat_push::Authority::Solo(target) = &authority {
            store.begin_chat_turn_with_push(
                id,
                &input.message,
                &input.request_id,
                &registry,
                input.workspace_epoch,
                target.clone(),
            )?
        } else {
            store.begin_chat_turn_at_epoch(
                id,
                &input.message,
                &input.request_id,
                &registry,
                input.workspace_epoch,
            )?
        }
    };
    if receipt.started {
        // Before the worker: the seat is offered the publication tool only if this is
        // written down first. A replay already has its own row and must not mint another.
        record_authority(&db, id, receipt.node_id, &authority)?;
        if state
            .store()?
            .lock()
            .chat_team_run(receipt.run_id)?
            .is_some()
        {
            let node = receipt.node_id;
            crate::chat_teams::watch(db.clone(), id, receipt.run_id, async move {
                ai_team_core::drive_chat_team_planning(&db, id, node).await
            });
        } else {
            spawn(state.clone(), db, id, receipt.node_id, false);
        }
    }
    Ok(Json(receipt))
}

/// Resolve what this exact human message authorises, before any row is written.
///
/// The prompt the person typed is read once, here, in the authenticated send path. The
/// checkout generation they were looking at is checked too, so a delayed request cannot
/// authorise a push against a checkout they never saw.
async fn push_authority(
    db: &std::path::Path,
    id: i64,
    input: &SendRequest,
) -> Result<ai_team_core::chat_push::Authority> {
    let mut store = ai_team_core::Store::open(db)?;
    let chat = store.chat(id)?;
    if chat.workspace_epoch != input.workspace_epoch {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "this chat's checkout changed since this message was composed; refresh before sending",
        )));
    }
    let authority = ai_team_core::chat_push::prepare(&mut store, id, &input.message).await?;
    if chat.mode == ai_team_core::ChatMode::Team {
        if let ai_team_core::chat_push::Authority::Ask(reason) = &authority {
            return Err(Error::Core(ai_team_core::Error::invalid(format!(
                "{reason} No team planning was started."
            ))));
        }
    }
    Ok(authority)
}

/// Publish the exact draft a team message named, and answer with the turn the receipt was
/// recorded on. No planning run is started, and nothing new is dispatched.
///
/// Kept inside a watched task: dropping the HTTP request does not cancel an approved
/// publication, and a reopened window must not have to guess whether it happened.
async fn publish_team(
    db: std::path::PathBuf,
    id: i64,
    input: SendRequest,
    target: ai_team_core::chat_push::Target,
) -> Result<ai_team_core::ChatSubmission> {
    let task = tokio::spawn(async move {
        let mut store = ai_team_core::Store::open(&db)?;
        ai_team_core::chat_push::publish_team(
            &mut store,
            id,
            &input.request_id,
            &input.message,
            &target,
        )
        .await?;
        store
            .replay_chat_push(id, &input.request_id, &input.message)?
            .ok_or_else(|| ai_team_core::Error::invalid("this push has no request receipt"))
    });
    Ok(task.await.map_err(|error| {
        ai_team_core::Error::invalid(format!(
            "the push was interrupted; inspect its receipt in Changes: {error}"
        ))
    })??)
}

/// Attach the authority to the turn that may use it, or say plainly that none was issued.
fn record_authority(
    db: &std::path::Path,
    id: i64,
    node_id: i64,
    authority: &ai_team_core::chat_push::Authority,
) -> Result<()> {
    use ai_team_core::chat_push::Authority;
    let mut store = ai_team_core::Store::open(db)?;
    if let Authority::Ask(reason) = authority {
        store.note_chat_push_refusal(id, node_id, reason)?;
    }
    Ok(())
}

pub(crate) fn spawn(
    state: AppState,
    db: std::path::PathBuf,
    chat_id: i64,
    mut node_id: i64,
    mut recovering: bool,
) {
    tokio::spawn(async move {
        loop {
            // Keep a watcher for each exact attempt, including automatically queued ones.
            let worker_db = db.clone();
            let worker = tokio::spawn(async move {
                ai_team_core::drive_chat(&worker_db, chat_id, node_id, recovering).await
            });
            let failure = match worker.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(error) => Some(format!("The chat worker stopped unexpectedly: {error}")),
            };
            // Before anything else is started for this chat: the turn's process group has
            // drained, so the journalled executor will act, and a queued instruction has
            // not yet taken the checkout. A turn that asked for nothing settles to nothing.
            if let Err(error) =
                ai_team_core::chat_push::settle(&db, chat_id, node_id, failure.is_some()).await
            {
                eprintln!("chat {chat_id} turn {node_id}: authorised push: {error}");
            }
            if let Some(error) = failure {
                eprintln!("chat {chat_id} turn {node_id}: {error}");
                let recorded = (|| -> Result<()> {
                    state
                        .store()?
                        .lock()
                        .fail_chat_worker(chat_id, node_id, &error)?;
                    Ok(())
                })();
                if let Err(recording) = recorded {
                    eprintln!("chat {chat_id} turn {node_id}: could not reconcile worker failure: {recording}");
                }
                break;
            }
            let next = (|| -> Result<Option<ai_team_core::ChatSubmission>> {
                let registry = ModelRegistry::load()?;
                Ok(state
                    .store()?
                    .lock()
                    .advance_chat_followup(chat_id, node_id, &registry)?)
            })();
            match next {
                Ok(Some(receipt)) if receipt.started => {
                    node_id = receipt.node_id;
                    recovering = false;
                }
                Ok(_) => break,
                Err(error) => {
                    eprintln!("chat {chat_id}: could not advance follow-up: {error}");
                    let recorded = (|| -> Result<()> {
                        state.store()?.lock().record_chat_followup_problem(
                            chat_id,
                            node_id,
                            &error.to_string(),
                        )?;
                        Ok(())
                    })();
                    if let Err(recording) = recorded {
                        eprintln!("chat {chat_id}: could not record held follow-up: {recording}");
                    }
                    break;
                }
            }
        }
    });
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Followup {
    node_id: i64,
    message: String,
    request_id: String,
    kind: ai_team_core::FollowupKind,
}
async fn queue_followup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Followup>,
) -> Result<Json<ai_team_core::ChatFollowup>> {
    Ok(Json(state.store()?.lock().queue_chat_followup(
        id,
        input.node_id,
        &input.message,
        &input.request_id,
        input.kind,
    )?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueuedTarget {
    followup_id: i64,
}
async fn cancel_followup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<QueuedTarget>,
) -> Result<Json<serde_json::Value>> {
    state
        .store()?
        .lock()
        .cancel_chat_followup(id, input.followup_id)?;
    Ok(Json(serde_json::json!({"cancelled":true})))
}
async fn send_followup(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<QueuedTarget>,
) -> Result<Json<ai_team_core::ChatSubmission>> {
    let registry = ModelRegistry::load()?;
    let db = state.database_path()?;
    let receipt = state
        .store()?
        .lock()
        .send_chat_followup(id, input.followup_id, &registry)?;
    if receipt.started {
        spawn(state, db, id, receipt.node_id, false);
    }
    Ok(Json(receipt))
}

#[derive(Deserialize)]
struct Target {
    node_id: i64,
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<Target>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let mut store = store.lock();
    let chat = store.chat(id)?;
    if chat.active_node_id != Some(target.node_id) {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "that turn is no longer active in this chat",
        )));
    }
    let node = store.node_run(target.node_id)?;
    if store.chat_team_run(node.run_id)?.is_some() {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "use this team's exact controller controls, not solo Stop",
        )));
    }
    if chat.supervisor_alive(&node) {
        store.request_chat_stop(id, target.node_id)?;
    } else {
        if chat.pi_alive(&node) {
            return Err(Error::Core(ai_team_core::Error::invalid(
                "the orphaned Pi process is still running; its checkout cannot be released safely yet",
            )));
        }
        store.request_chat_stop(id, node.id)?;
        store.claim_chat_resume(id, node.id)?;
        store.finish_chat_turn(
            id,
            node.id,
            NodeStatus::Cancelled,
            Some("Interrupted turn stopped. Working files are kept."),
        )?;
    }
    Ok(Json(serde_json::json!({"stopping": true})))
}

async fn resume(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<Target>,
) -> Result<Json<serde_json::Value>> {
    let db = state.database_path()?;
    let node_id = {
        let store = state.store()?;
        let mut store = store.lock();
        if store.chat(id)?.active_node_id != Some(target.node_id) {
            return Err(Error::Core(ai_team_core::Error::invalid(
                "that turn is no longer active in this chat",
            )));
        }
        store.claim_chat_resume(id, target.node_id)?
    };
    spawn(state.clone(), db, id, node_id, true);
    Ok(Json(serde_json::json!({"resumed": true})))
}
