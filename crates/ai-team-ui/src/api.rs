//! What the window reads, and how it learns something changed.
//!
//! Every route is a *view* over the store (`ai-team-core`), which is the only thing that
//! writes SQL. Nothing here keeps state of its own: a second window, the CLI, and the
//! desktop shell all read the same rows, and a cache here would be the copy that drifts.
//!
//! Live updates are polled rather than pushed, and that is not laziness. `ait run` is a
//! *different process* writing to the same SQLite file, so there is no in-process channel
//! to subscribe to; the only honest way to notice its work is to look. The poll is keyed
//! on the largest event id, so a quiet run costs one cheap query a tick.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::get;
use axum::Json as JsonBody;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt as _;

use ai_team_core::{Event, NodeRun, Project, Run, Usage};

use crate::error::Result;
use crate::state::AppState;

/// How often to look for new rows. Fast enough that a turn feels live, slow enough that
/// an idle window is not a busy loop on somebody's laptop battery.
const POLL: Duration = Duration::from_millis(400);

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(crate::health::health))
        .route("/projects", get(projects).post(register_project))
        .route("/browse", get(browse))
        .route("/worktrees", get(worktrees))
        .route("/runs", get(runs).post(start))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/events", get(run_events))
        .merge(node_routes())
        .route("/events", get(stream))
        .route("/runs/{id}/approve-plan", axum::routing::post(approve_plan))
        .route("/runs/{id}/approvals", get(approvals))
        .route("/board", get(board))
        .route(
            "/board/slices/{key}",
            get(board_slice).post(move_slice).patch(edit_board_slice),
        )
        .route(
            "/board/slices/{key}/claim",
            axum::routing::post(claim_board_slice),
        )
        .route(
            "/board/slices/{key}/release",
            axum::routing::post(release_board_slice),
        )
        .route(
            "/board/slices/{key}/notes",
            axum::routing::post(note_board_slice),
        )
        .route("/today", get(today))
        .route("/notifications", get(notifications))
        .route(
            "/notifications/{id}/read",
            axum::routing::post(read_notification),
        )
        .route("/tree", get(tree))
        .route("/map", get(repo_map))
        .route("/file", get(read_file).post(write_file))
        .route("/search", get(search))
        .route("/projects/{id}/repos", axum::routing::post(attach_repo))
        .route("/crew", get(crew))
        .route("/crew/{agent}/say", axum::routing::post(say))
        .route("/roster", get(roster))
        .route("/models", get(models))
        .route(
            "/roster/ownership/detect",
            axum::routing::post(detect_ownership),
        )
        .route("/roster/models/reset", axum::routing::post(reset_models))
        .route("/roster/delivery", axum::routing::post(edit_delivery))
        .route("/roster/{id}", axum::routing::post(edit_seat))
        .route("/settings", get(settings))
        .route("/settings/provider", axum::routing::post(set_provider))
        .route("/settings/context", axum::routing::post(set_context))
        .route("/settings/token", axum::routing::post(set_token))
        .route("/settings/sign-in", axum::routing::post(start_sign_in))
        .route(
            "/settings/context-auth",
            axum::routing::post(start_context_auth),
        )
        .route("/settings/fallback", axum::routing::post(set_fallback))
        .route("/doctor", get(doctor))
        .route("/doctor/fix", axum::routing::post(doctor_fix))
        .route("/update", get(update_check).post(update_apply))
        .route("/scm", get(scm))
        .route("/scm/stage", axum::routing::post(scm_stage))
        .route("/scm/commit", axum::routing::post(scm_commit))
        .route("/scm/branch", axum::routing::post(scm_branch))
        .route("/scm/push", axum::routing::post(scm_push))
        .route("/terminals", get(terminals).post(open_terminal))
        .route("/terminals/{id}", get(read_terminal).post(write_terminal))
        .route("/terminals/{id}/close", axum::routing::post(close_terminal))
        .route(
            "/terminals/{id}/resize",
            axum::routing::post(resize_terminal),
        )
        .route("/lsp/diagnostics", axum::routing::post(diagnostics))
        .route("/lsp/hover", axum::routing::post(hover))
        .route("/lsp/definition", axum::routing::post(definition))
        .route("/lsp/completion", axum::routing::post(completion))
        .route("/lsp/rename", axum::routing::post(rename))
        .route("/analytics", get(analytics))
        .route("/reminders", get(reminders).post(add_reminder))
        .route("/reminders/{id}", axum::routing::delete(drop_reminder))
        .route("/reviews", get(reviews))
        .route("/reviews/{id}", get(review))
        .route("/reviews/{id}/comments", axum::routing::post(add_comment))
        .route("/reviews/{id}/submit", axum::routing::post(submit))
        .route("/comments/{id}/resolve", axum::routing::post(resolve))
}

fn node_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/runs/{run}/nodes/{node}/reply",
            axum::routing::post(reply_to_node),
        )
        .route(
            "/runs/{run}/nodes/{node}/resume",
            axum::routing::post(resume_node),
        )
        .route(
            "/runs/{run}/nodes/{node}/reset-session",
            axum::routing::post(reset_node_session),
        )
        .route(
            "/runs/{run}/nodes/{node}/deliver",
            axum::routing::post(deliver_node),
        )
}

/// Which checkout the editor is looking at.
///
/// A selected workspace is an arbitrary linked worktree, including one a person opened
/// rather than a node leased. `node` remains for deep links into recorded runs; it wins
/// when present because that historical row identifies the checkout exactly.
#[derive(Debug, Deserialize)]
struct TreeQuery {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    /// A node's leased worktree, when the editor is following one.
    #[serde(default)]
    node: Option<i64>,
    #[serde(default)]
    path: String,
}

/// Resolve which directory on disk a request is about.
///
/// A path sent by the browser is never trusted merely because `/worktrees` returned it
/// earlier. `Worktrees::resolve` asks git on every request, which accepts linked
/// worktrees outside the main checkout while refusing an unrelated directory.
async fn worktree_for(
    state: &AppState,
    project: &str,
    node: Option<i64>,
    workspace: Option<&str>,
) -> Result<std::path::PathBuf> {
    let (node_path, repo) = {
        let store = state.store()?;
        let store = store.lock();
        let node_path = node
            .map(|id| store.node_run(id))
            .transpose()?
            .and_then(|node| node.worktree_path);
        let project = store.find_project(project)?;
        let repo = store
            .project_repos(project.id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| {
                crate::error::Error::Core(ai_team_core::Error::invalid(
                    "that project has no checkout to open",
                ))
            })?;
        (node_path, std::path::PathBuf::from(repo))
    };

    if let Some(path) = node_path {
        return ai_team_core::Worktrees::at(&repo)
            .resolve(std::path::Path::new(&path))
            .await
            .map_err(Into::into);
    }
    match workspace {
        Some(path) => ai_team_core::Worktrees::at(&repo)
            .resolve(std::path::Path::new(path))
            .await
            .map_err(Into::into),
        None => repo.canonicalize().map_err(|error| {
            crate::error::Error::Core(ai_team_core::Error::invalid(format!(
                "could not resolve checkout {}: {error}",
                repo.display()
            )))
        }),
    }
}

/// Resolve the checkout that owns runtime state.
///
/// Every selected workspace is narrow, including main. A team may fan out into leased
/// maker worktrees, but those nodes remain part of the run rooted where Start was pressed;
/// sibling workspaces must never share run history merely because they share a project.
async fn runtime_scope_for(
    state: &AppState,
    project: &str,
    requested: Option<&str>,
) -> Result<Option<std::path::PathBuf>> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    Ok(Some(
        worktree_for(state, project, None, Some(requested)).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct WorktreesQuery {
    project: String,
}

/// The worktrees `awt` is holding for a project.
///
/// Read from `awt` every time rather than recorded here: the pool is ai-worktree's, not
/// ai-team's (D4), and a copy of it in this database would be a second answer that is
/// wrong the moment somebody runs `awt get` in a terminal.
///
/// The checkout is resolved the same way the editor resolves one, so a project with no
/// repository attached says so rather than answering with an empty pool - which would
/// read as "no worktrees" instead of "nowhere to look".
async fn worktrees(
    State(state): State<AppState>,
    Query(query): Query<WorktreesQuery>,
) -> Result<Json<Vec<ai_team_core::PlacedWorktree>>> {
    // Resolved and the lock dropped before awaiting: `awt` and `git` are other programs,
    // and holding the database across them would block every other request.
    let repo = {
        let store = state.store()?;
        let store = store.lock();
        let project = store.find_project(&query.project)?;
        store
            .project_repos(project.id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| {
                crate::error::Error::Core(ai_team_core::Error::invalid(
                    "that project has no checkout, so it has no worktrees",
                ))
            })?
    };
    let entries = ai_team_core::Worktrees::at(&repo).pool().await?;

    // Which pull request each worktree holds, from the rows that built in it - read in one
    // window, and the lock let go before asking ai-planner anything.
    let built: Vec<(String, String, String, Option<String>)> = {
        let store = state.store()?;
        let store = store.lock();
        let mut built = Vec::new();
        for entry in entries.iter().filter(|entry| !entry.main) {
            let Some(node) = store.latest_pr_node_in(&entry.path)? else {
                continue;
            };
            let run = store.run(node.run_id)?;
            if let (Some(plan), Some(slice_key)) = (run.plan_slug, node.slice_key) {
                built.push((entry.path.clone(), plan, slice_key, run.workspace_path));
            }
        }
        built
    };

    // What each stacks on, from its plan: once per plan, and nothing when the plan cannot
    // be read - the worktree then sits under its run's checkout rather than vanishing.
    let mut plans: std::collections::HashMap<String, Vec<ai_team_core::Slice>> =
        std::collections::HashMap::new();
    for (_, plan, _, _) in &built {
        if !plans.contains_key(plan) {
            let slices = ai_team_core::Planner::at(&repo)
                .for_plan(plan.clone())
                .slices()
                .await
                .unwrap_or_default();
            plans.insert(plan.clone(), slices);
        }
    }
    let facts: Vec<ai_team_core::PrFacts> = built
        .into_iter()
        .map(|(path, plan, slice_key, workspace)| {
            let stacked_on = plans.get(&plan).and_then(|slices| {
                let slice = slices.iter().find(|slice| slice.key == slice_key)?;
                ai_team_core::stacked_on(slice, slices)?.branch.clone()
            });
            ai_team_core::PrFacts {
                path,
                plan,
                slice_key,
                workspace,
                stacked_on,
            }
        })
        .collect();
    Ok(Json(ai_team_core::place_worktrees(entries, &facts)))
}

async fn tree(
    State(state): State<AppState>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<Vec<ai_team_core::Entry>>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(ai_team_core::list_tree(&worktree, &query.path)?))
}

#[derive(Debug, Deserialize)]
struct MapQuery {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
}

/// The checkout, and which seat's zone claims each path.
///
/// The seats are read and the lock released *before* the walk: holding the store open
/// across a few hundred `read_dir` calls would stall every other request on the window
/// for as long as the disk takes.
async fn repo_map(
    State(state): State<AppState>,
    Query(query): Query<MapQuery>,
) -> Result<Json<ai_team_core::RepoMap>> {
    let worktree = worktree_for(&state, &query.project, None, query.workspace.as_deref()).await?;

    let owners = {
        let store = state.store()?;
        let store = store.lock();
        let project = store.find_project(&query.project)?;
        match project.team_id {
            // A disabled seat is not given work, so its zone does not own anything - the
            // map has to agree with dispatch or it is describing a different program.
            Some(team) => store
                .agents(team)?
                .into_iter()
                .filter(|agent| agent.enabled)
                .map(|agent| ai_team_core::Owner {
                    role: agent.role,
                    name: agent.name,
                    zone: agent.zone,
                })
                .collect(),
            None => Vec::new(),
        }
    };

    Ok(Json(ai_team_core::repo_map(&worktree, &owners)?))
}

#[derive(Debug, Serialize)]
struct FileBody {
    path: String,
    text: String,
    /// False when the bytes are not text. The editor refuses rather than rendering a
    /// screenful of replacement characters and then offering to save them back.
    editable: bool,
}

async fn read_file(
    State(state): State<AppState>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<FileBody>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    let path = ai_team_core::safe_join(&worktree, &query.path)?;
    let bytes = std::fs::read(&path).map_err(|error| {
        crate::error::Error::Core(ai_team_core::Error::invalid(format!(
            "could not read {}: {error}",
            query.path
        )))
    })?;

    // Decoded strictly, not lossily: a lossy read followed by a save would replace every
    // undecodable byte with U+FFFD and quietly corrupt the file.
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Json(FileBody {
            path: query.path,
            text,
            editable: true,
        })),
        Err(_) => Ok(Json(FileBody {
            path: query.path,
            text: String::new(),
            editable: false,
        })),
    }
}

#[derive(Debug, Deserialize)]
struct SaveRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    path: String,
    text: String,
}

async fn write_file(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<SaveRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    let path = ai_team_core::safe_join(&worktree, &request.path)?;
    std::fs::write(&path, request.text).map_err(|error| {
        crate::error::Error::Core(ai_team_core::Error::invalid(format!(
            "could not save {}: {error}",
            request.path
        )))
    })?;
    Ok(Json(serde_json::json!({ "saved": request.path })))
}

/// One provider, as the settings page needs it.
#[derive(Debug, Deserialize)]
struct CrewQuery {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SayRequest {
    message: String,
    #[serde(default)]
    workspace: Option<String>,
}

/// Say something to one seat.
///
/// Two different acts, and the response says which happened rather than leaving it to be
/// inferred. A seat mid-turn takes the message now - that is an interruption and it returns
/// when delivered. An idle seat needs a turn started, which means generating the project,
/// leasing a worktree and driving it: minutes. So that is spawned and this returns as soon
/// as it is under way, the same as starting a run (M3-S12).
async fn say(
    State(state): State<AppState>,
    Path(agent): Path<i64>,
    JsonBody(request): JsonBody<SayRequest>,
) -> Result<Json<ai_team_core::Reached>> {
    if request.message.trim().is_empty() {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "say something",
        )));
    }

    // Read enough to identify the project, then validate the selected checkout without
    // holding the database across git.
    let project = {
        let store = state.store()?;
        let store = store.lock();
        ai_team_core::speak_target(&store, agent)?.project_slug
    };
    let worktree = runtime_scope_for(&state, &project, request.workspace.as_deref()).await?;
    let target = {
        let store = state.store()?;
        let store = store.lock();
        ai_team_core::speak_target_in(&store, agent, worktree.as_deref())?
    };

    match target.would() {
        ai_team_core::Would::Nothing => Ok(Json(ai_team_core::Reached::Refused {
            because: format!(
                "{} is switched off, so it would never be given the message",
                target.agent.role
            ),
        })),
        ai_team_core::Would::Queue => {
            // Synchronous: there is nothing to reach out to, so this is a row written and
            // the answer is known before the response is built.
            let store = state.store()?;
            let mut store = store.lock();
            Ok(Json(ai_team_core::queue_message(
                &mut store,
                &target,
                &request.message,
            )?))
        }
        ai_team_core::Would::ApprovePlan => {
            let run_id = target.approval_run_id.ok_or_else(|| {
                crate::error::Error::Core(ai_team_core::Error::invalid(
                    "the approval-held run changed; refresh the crew and try again",
                ))
            })?;
            let db = {
                let store = state.store()?;
                let mut store = store.lock();
                ai_team_core::claim_plan_approval_with_direction(
                    &mut store,
                    run_id,
                    &request.message,
                )?;
                store.path().to_path_buf()
            };
            tokio::spawn(async move {
                if let Err(error) = ai_team_core::continue_approved_run_at(&db, run_id).await {
                    eprintln!("ai-team: approved run {run_id} stopped: {error}");
                }
            });
            Ok(Json(ai_team_core::Reached::Continued { run_id }))
        }
        ai_team_core::Would::Coordinate => {
            // Talking to the orchestrator is the graph's control surface, never a
            // planless read-only Pi turn. It runs the same workflow as Start and stops at
            // approval so the person can inspect what the planner produced.
            let spec = ai_team_core::Request {
                project: target.project_slug,
                prompt: Some(request.message),
                workspace: worktree,
                approval_required: true,
                ..ai_team_core::Request::default()
            };
            tokio::spawn(async move {
                if let Err(error) = ai_team_core::run_workflow(&spec, |_| {}).await {
                    eprintln!("ai-team: orchestrator direction failed: {error}");
                }
            });
            Ok(Json(ai_team_core::Reached::Coordinating))
        }
        ai_team_core::Would::StartWork => {
            // Detached: a maker turn takes minutes and the run records itself in the
            // database the window is already watching.
            let message = request.message.clone();
            tokio::spawn(async move {
                let started = match worktree {
                    Some(worktree) => ai_team_core::start_turn_in(agent, message, worktree).await,
                    None => ai_team_core::start_turn(agent, message).await,
                };
                if let Err(error) = started {
                    eprintln!("starting a turn for seat {agent}: {error}");
                }
            });
            Ok(Json(ai_team_core::Reached::Started))
        }
    }
}

/// What every seat is doing, in one read.
///
/// One request because the window polls: six would be five too many, and a seat arriving a
/// tick after its neighbour makes the whole panel look unstable.
async fn crew(
    State(state): State<AppState>,
    Query(query): Query<CrewQuery>,
) -> Result<Json<Vec<ai_team_core::Member>>> {
    let worktree = runtime_scope_for(&state, &query.project, query.workspace.as_deref()).await?;
    let store = state.store()?;
    let store = store.lock();
    let project = store.find_project(&query.project)?;
    Ok(Json(ai_team_core::crew_of_workspace(
        &store,
        project.id,
        worktree.as_deref(),
    )?))
}

/// One seat, as configured and as it will actually resolve.
#[derive(Debug, Serialize)]
struct Seat {
    id: i64,
    role: String,
    name: String,
    purpose: String,
    /// What the roster says.
    provider: String,
    model: String,
    /// What dispatch will actually use. Different when the machine denies the configured
    /// provider and the ranking picks another (D13) - a roster that hides that lies about
    /// which account the work lands on.
    effective_provider: String,
    effective_model: String,
    /// Why it differs, when it does.
    fallback_reason: Option<String>,
    reasoning: String,
    zone: String,
    /// Read-only seats never get the editing tools generated, so this is a real capability
    /// rather than a label.
    read_only: bool,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct Roster {
    /// `None` when asking about the defaults rather than a project.
    project: Option<String>,
    team: Option<String>,
    delivery: ai_team_core::DeliverySettings,
    seats: Vec<Seat>,
    /// The providers a seat may be set to on this machine, so the page cannot offer one
    /// that would fail at dispatch.
    available: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ModelCatalog {
    models: Vec<ai_team_core::ModelChoice>,
    /// A catalogue failure does not blank the roster: configured seats remain useful,
    /// and the page says why it cannot offer alternatives.
    error: Option<String>,
}

async fn models() -> Json<ModelCatalog> {
    match tokio::task::spawn_blocking(|| {
        ai_team_core::ModelRegistry::load().and_then(|registry| registry.models())
    })
    .await
    {
        Ok(Ok(models)) => Json(ModelCatalog {
            models,
            error: None,
        }),
        Ok(Err(error)) => Json(ModelCatalog {
            models: Vec::new(),
            error: Some(error.to_string()),
        }),
        Err(error) => Json(ModelCatalog {
            models: Vec::new(),
            error: Some(format!("could not read Pi's models: {error}")),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct RosterQuery {
    /// Omitted asks what a new project would get.
    #[serde(default)]
    project: Option<String>,
}

async fn roster(
    State(state): State<AppState>,
    Query(query): Query<RosterQuery>,
) -> Result<Json<Roster>> {
    let registry = ai_team_core::ModelRegistry::load().ok();
    let available = registry.as_ref().map_or_else(Vec::new, |registry| {
        registry
            .statuses()
            .into_iter()
            .filter(|status| status.state == ai_team_core::ProviderState::Allowed)
            .map(|status| status.provider.as_str().to_string())
            .collect()
    });

    // No project named: describe what a new one would get, which is the question somebody
    // asks before creating one rather than after.
    let Some(slug) = query.project else {
        let local = || {
            ai_team_core::DEFAULT_ROSTER
                .iter()
                .map(|preset| ai_team_core::RoleModelDefault {
                    role: preset.role.to_string(),
                    provider: ai_team_core::Provider::Local,
                    model: "auto".to_string(),
                })
                .collect()
        };
        let defaults = registry
            .as_ref()
            .and_then(|registry| registry.role_defaults().ok())
            .unwrap_or_else(local);
        return Ok(Json(Roster {
            project: None,
            team: None,
            delivery: ai_team_core::DeliverySettings::default(),
            seats: (0i64..)
                .zip(ai_team_core::DEFAULT_ROSTER)
                .zip(defaults)
                .map(|((ord, preset), choice)| {
                    let new = preset.to_new_agent_on(choice.provider, &choice.model, ord);
                    Seat {
                        // Not a row yet, so it has no id. Negative rather than zero, which
                        // a surface could mistake for a real one.
                        id: -1,
                        role: new.role,
                        name: new.name,
                        purpose: new.purpose,
                        provider: choice.provider.as_str().to_string(),
                        model: choice.model.clone(),
                        effective_provider: choice.provider.as_str().to_string(),
                        effective_model: choice.model,
                        fallback_reason: None,
                        reasoning: new.reasoning.as_str().to_string(),
                        zone: new.zone,
                        read_only: new.read_only,
                        enabled: true,
                    }
                })
                .collect(),
            available,
        }));
    };

    let store = state.store()?;
    let store = store.lock();
    let project = store.find_project(&slug)?;
    let team_id = project.team_id.ok_or_else(|| {
        crate::error::Error::Core(ai_team_core::Error::invalid("that project has no team yet"))
    })?;
    let team = store.team(team_id)?;

    let seats = store
        .agents(team_id)?
        .into_iter()
        .map(|agent| {
            // Resolved the way dispatch will resolve it, so the page shows the account the
            // work actually lands on.
            let resolved = registry
                .as_ref()
                .and_then(|registry| registry.resolve(&agent).ok());
            Seat {
                id: agent.id,
                role: agent.role,
                name: agent.name,
                purpose: agent.purpose,
                provider: agent.provider.as_str().to_string(),
                model: agent.model.clone(),
                effective_provider: resolved.as_ref().map_or_else(
                    || agent.provider.as_str().to_string(),
                    |r| r.provider.as_str().to_string(),
                ),
                effective_model: resolved
                    .as_ref()
                    .map_or_else(|| agent.model.clone(), |r| r.model.clone()),
                fallback_reason: resolved.and_then(|r| r.fallback_reason),
                reasoning: agent.reasoning.as_str().to_string(),
                zone: agent.zone,
                read_only: agent.read_only,
                enabled: agent.enabled,
            }
        })
        .collect();

    Ok(Json(Roster {
        project: Some(project.slug),
        team: Some(team.name),
        delivery: team.delivery,
        seats,
        available,
    }))
}

#[derive(Debug, Deserialize)]
struct TeamAction {
    project: String,
}

#[derive(Debug, Deserialize)]
struct DeliveryChange {
    project: String,
    push: ai_team_core::DeliveryPolicy,
    pr: ai_team_core::DeliveryPolicy,
    merge: ai_team_core::DeliveryPolicy,
}

async fn edit_delivery(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<DeliveryChange>,
) -> Result<Json<ai_team_core::DeliverySettings>> {
    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&request.project)?;
    let team_id = project
        .team_id
        .ok_or_else(|| ai_team_core::Error::invalid("that project has no team to configure"))?;
    let delivery = ai_team_core::DeliverySettings {
        push: request.push,
        pr: request.pr,
        merge: request.merge,
    };
    store.update_delivery(team_id, delivery)?;
    Ok(Json(delivery))
}

/// Re-run the conservative repository detector only when the operator asks.
///
/// Zones are ordinary editable team data after registration. Silently re-detecting them
/// on every page load would overwrite deliberate ownership, so this mutation is explicit.
async fn detect_ownership(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<TeamAction>,
) -> Result<Json<serde_json::Value>> {
    let root = worktree_for(&state, &request.project, None, None).await?;
    let (backend, frontend) =
        tokio::task::spawn_blocking(move || ai_team_core::suggested_zones(&root))
            .await
            .map_err(|error| {
                ai_team_core::Error::invalid(format!("ownership detection failed: {error}"))
            })?;

    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&request.project)?;
    let team_id = project.team_id.ok_or_else(|| {
        crate::error::Error::Core(ai_team_core::Error::invalid("that project has no team yet"))
    })?;
    let mut changed = 0;
    for agent in store.agents(team_id)? {
        let zone = match agent.role.as_str() {
            "backend" => Some(backend.clone()),
            "frontend" => Some(frontend.clone()),
            _ => None,
        };
        if let Some(zone) = zone {
            let mut update = ai_team_core::NewAgent::from(&agent);
            update.zone = zone;
            store.update_agent(agent.id, update)?;
            changed += 1;
        }
    }

    Ok(Json(serde_json::json!({ "changed": changed })))
}

/// Apply today's role defaults to an existing team only after an explicit operator action.
/// Existing rosters otherwise remain stable even as Pi's catalogue evolves.
async fn reset_models(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<TeamAction>,
) -> Result<Json<serde_json::Value>> {
    let defaults = tokio::task::spawn_blocking(|| {
        ai_team_core::ModelRegistry::load().and_then(|registry| registry.role_defaults())
    })
    .await
    .map_err(|error| ai_team_core::Error::invalid(format!("model detection failed: {error}")))??;

    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&request.project)?;
    let team_id = project.team_id.ok_or_else(|| {
        crate::error::Error::Core(ai_team_core::Error::invalid("that project has no team yet"))
    })?;
    let mut changed = 0;
    for agent in store.agents(team_id)? {
        let Some(choice) = defaults.iter().find(|choice| choice.role == agent.role) else {
            continue;
        };
        let mut update = ai_team_core::NewAgent::from(&agent);
        update.provider = choice.provider;
        update.model.clone_from(&choice.model);
        update.context_window = None;
        store.update_agent(agent.id, update)?;
        changed += 1;
    }

    Ok(Json(serde_json::json!({ "changed": changed })))
}

#[derive(Debug, Deserialize)]
struct SeatChange {
    #[serde(default)]
    provider: Option<ai_team_core::Provider>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    zone: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// Change one seat - `ait agents edit`, from the window.
///
/// Leaves the generated eve project stale in exactly the same deliberate way: regenerating
/// is `ait agents generate`, and `ait run` does it before a turn, so an edit never
/// half-applies to a project that is mid-run.
async fn edit_seat(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(change): JsonBody<SeatChange>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let mut store = store.lock();
    let agent = store.agent(id)?;

    let mut update = ai_team_core::NewAgent::from(&agent);
    if let Some(provider) = change.provider {
        update.provider = provider;
        // A provider change without a model would leave the old provider's model name
        // behind, which fails at the model call rather than here. Defaulted rather than
        // refused, because the window has a picker and not a second field.
        update.model = change
            .model
            .clone()
            .unwrap_or_else(|| ai_team_core::ModelRegistry::default_model(provider).to_string());
        update.context_window = None;
    } else if let Some(model) = change.model.clone() {
        update.model = model;
    }
    if let Some(zone) = change.zone {
        update.zone = zone;
    }
    if let Some(enabled) = change.enabled {
        update.enabled = enabled;
    }

    let saved = store.update_agent(id, update)?;
    Ok(Json(serde_json::json!({
        "role": saved.role,
        "provider": saved.provider.as_str(),
        "model": saved.model,
    })))
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    /// A directory on this machine. Absolute, because a relative path would be relative to
    /// wherever the server happens to have been started - which is not where the person
    /// clicking is looking. A leading `~` is expanded, because it is what somebody types
    /// and a window has no shell in front of it to do the expanding.
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<ai_team_core::ProjectKind>,
}

/// Register a directory as a project - what `ait init` does, from the window.
///
/// The database is created first if it is absent, so this works as the first thing
/// somebody does rather than failing with "no database" on a machine that has just been
/// installed.
async fn register_project(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<RegisterRequest>,
) -> Result<Json<ai_team_core::Registered>> {
    if state.store().is_err() {
        ai_team_core::apply_fix(ai_team_core::Action::CreateDatabase)?;
    }
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(ai_team_core::register_project(
        &mut store,
        std::path::Path::new(&request.path),
        request.name.as_deref(),
        request.kind,
    )?))
}

#[derive(Debug, Default, Deserialize)]
struct BrowseQuery {
    /// Where to look. Empty means home, which is where a person's checkouts are.
    #[serde(default)]
    path: String,
}

/// The directories inside one, so a repository can be picked rather than typed (D24).
///
/// One of the routes that has to work *before* there is a database, like `/doctor`: the
/// first thing somebody does on a new machine is find the repository, and a picker that
/// needed a project already registered would be useless exactly then.
async fn browse(Query(query): Query<BrowseQuery>) -> Result<Json<ai_team_core::Listing>> {
    Ok(Json(ai_team_core::browse_dir(&query.path)?))
}

#[derive(Debug, Deserialize)]
struct AttachRequest {
    path: String,
}

async fn attach_repo(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(request): JsonBody<AttachRequest>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let mut store = store.lock();
    let path = ai_team_core::attach_repo_at(&mut store, id, std::path::Path::new(&request.path))?;
    Ok(Json(serde_json::json!({ "attached": path })))
}

#[derive(Debug, Serialize)]
struct ProviderSetting {
    provider: String,
    label: String,
    /// What the machine profile says.
    allowed: bool,
    /// Whether it actually answers. Kept apart from `allowed`, because a provider that is
    /// ticked and not signed into fails at dispatch - minutes later and somewhere else -
    /// and a page that shows one boolean cannot warn about that.
    reachable: bool,
    detail: String,
    /// How this provider is authenticated, so the page can say what to do about it rather
    /// than leaving somebody to guess which account it means.
    how: String,
    /// The command that signs it in, when there is one. Shown in full and run only when
    /// somebody presses the button beside it (D25).
    sign_in: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct Settings {
    profile_path: String,
    providers: Vec<ProviderSetting>,
    /// A total ranking. A denied preference resolves to the first allowed one (D13).
    fallback: Vec<String>,
    context: Vec<ContextSetting>,
}

#[derive(Debug, Serialize)]
struct ContextSetting {
    source: String,
    allowed: bool,
    /// Whether Pi already holds OAuth for this server name. This is the normal path and
    /// is intentionally separate from a manually supplied token, which is only fallback.
    oauth_connected: bool,
    /// Whether the optional manual-token fallback exists.
    token_set: bool,
    /// Where the token in force came from, so the page can say whether it is editable
    /// here. A variable exported into this process cannot be unset from the window.
    held: ai_team_core::Held,
    /// Still reported, because an operator who exports it wants to see the name they
    /// exported. Never the value - nothing on this route produces one (D23).
    token_env: String,
}

fn how_authenticated(provider: ai_team_core::Provider) -> &'static str {
    match provider {
        ai_team_core::Provider::Claude => {
            "your Claude subscription, through the Claude Code CLI - never a metered API key"
        }
        // Codex, not eve: eve stopped being the runtime at D20, and the sentence that was
        // here told somebody to sign in to a program they do not have.
        ai_team_core::Provider::OpenAi => {
            "your ChatGPT subscription, through the Codex CLI that Pi's `openai-codex` uses"
        }
        ai_team_core::Provider::ZAi => {
            "a GLM Coding Plan - flat rate, no metering. Set AI_TEAM_ZAI_KEY to its key"
        }
        ai_team_core::Provider::Local => {
            "the ailocal gateway on 127.0.0.1:8081 - free, and never leaves the machine"
        }
    }
}

async fn settings(State(state): State<AppState>) -> Result<Json<Settings>> {
    let path = ai_team_core::machine_profile_path()?;
    let registry = ai_team_core::ModelRegistry::load();

    let providers = match &registry {
        Ok(registry) => registry
            .statuses()
            .into_iter()
            .map(|status| ProviderSetting {
                provider: status.provider.as_str().to_string(),
                label: status.provider.to_string(),
                allowed: status.state != ai_team_core::ProviderState::Denied,
                reachable: status.state == ai_team_core::ProviderState::Allowed,
                detail: status.detail,
                how: how_authenticated(status.provider).into(),
                sign_in: status.sign_in,
            })
            .collect(),
        // No profile yet: every provider is denied, which is the truth rather than an
        // error, and the page can still show what each one would be.
        Err(_) => ai_team_core::Provider::ALL
            .iter()
            .map(|provider| ProviderSetting {
                provider: provider.as_str().to_string(),
                label: provider.to_string(),
                allowed: false,
                reachable: false,
                detail: "no machine profile yet".into(),
                how: how_authenticated(*provider).into(),
                sign_in: ai_team_core::sign_in_command(*provider),
            })
            .collect(),
    };

    let fallback = registry.as_ref().map_or_else(
        |_| {
            ai_team_core::Provider::ALL
                .iter()
                .map(|provider| provider.as_str().to_string())
                .collect()
        },
        |registry| {
            registry
                .profile()
                .fallback()
                .iter()
                .map(|provider| provider.as_str().to_string())
                .collect()
        },
    );

    let context = ai_team_core::ContextSource::ALL
        .iter()
        .map(|source| {
            let held = state.credentials().held(*source);
            ContextSetting {
                source: source.as_str().to_string(),
                allowed: registry
                    .as_ref()
                    .is_ok_and(|registry| registry.context_sources().contains(source)),
                oauth_connected: state.credentials().has_oauth(*source),
                token_set: held != ai_team_core::Held::Absent,
                held,
                token_env: ai_team_core::token_env(*source),
            }
        })
        .collect();

    Ok(Json(Settings {
        profile_path: path.display().to_string(),
        providers,
        fallback,
        context,
    }))
}

#[derive(Debug, Deserialize)]
struct ProviderChange {
    provider: ai_team_core::Provider,
    allowed: bool,
}

/// Allow or deny a provider, by editing the file rather than replacing it.
///
/// The profile is created first if it is absent, so ticking a provider on a fresh machine
/// does the obvious thing instead of failing with "no such file".
async fn set_provider(
    JsonBody(change): JsonBody<ProviderChange>,
) -> Result<Json<serde_json::Value>> {
    let (path, _) = ai_team_core::ensure_machine_profile()?;
    ai_team_core::set_provider(&path, change.provider, change.allowed)?;
    Ok(Json(serde_json::json!({
        "provider": change.provider.as_str(),
        "allowed": change.allowed,
    })))
}

#[derive(Debug, Deserialize)]
struct ContextChange {
    source: ai_team_core::ContextSource,
    allowed: bool,
}

async fn set_context(JsonBody(change): JsonBody<ContextChange>) -> Result<Json<serde_json::Value>> {
    let (path, _) = ai_team_core::ensure_machine_profile()?;
    ai_team_core::set_context(&path, change.source, change.allowed)?;
    Ok(Json(serde_json::json!({
        "source": change.source.as_str(),
        "allowed": change.allowed,
    })))
}

/// `deny_unknown_fields` is the guarantee, not tidiness.
///
/// Without it a request carrying `"command": "..."` deserialises happily, the field is
/// ignored, and the route does the right thing for a reason nobody can see in the type. A
/// refusal is a property a test can assert on.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignIn {
    provider: ai_team_core::Provider,
}

/// Start a provider's sign-in in a terminal, and hand back the pane to watch (D25).
///
/// The command is **not** taken from the request. It is looked up from the provider, so
/// the set of things this route can run is the set in `sign_in_command` and nothing else -
/// the same shape as `/doctor/fix`, and for the same reason: a route that ran what it was
/// sent would be a shell on loopback.
///
/// Home is the working directory, because a credential flow has no worktree and there is
/// nowhere else sensible for it to be.
async fn start_sign_in(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<SignIn>,
) -> Result<Json<serde_json::Value>> {
    let command = ai_team_core::sign_in_command(request.provider).ok_or_else(|| {
        ai_team_core::Error::invalid(format!(
            "{} is not signed in with a command - see what the settings page says about it",
            request.provider
        ))
    })?;
    let home = ai_team_core::home_dir()?;
    let id = state.terminals().open_running(&home, Some(command))?;
    Ok(Json(serde_json::json!({ "id": id, "command": command })))
}

/// The request names a known source and nothing executable.
///
/// `deny_unknown_fields` makes that boundary assertable: a body carrying `command` is
/// refused rather than silently ignored, exactly like provider sign-in above.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextAuth {
    source: ai_team_core::ContextSource,
}

/// Start Pi's own browser OAuth flow in ai-team's terminal pane.
///
/// ai-team does not implement ClickUp or Figma OAuth. It writes the MCP definition a seat
/// uses, starts interactive Pi, and submits the adapter's public `/mcp-auth <server>`
/// command. Pi opens the browser, receives the callback, and keeps the credential in its
/// own OS-backed store. The source enum fixes both the server and slash command; no shell
/// line or URL comes from the request.
async fn start_context_auth(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<ContextAuth>,
) -> Result<Json<serde_json::Value>> {
    let config = ai_team_core::write_context_oauth_config(request.source)?;
    let home = ai_team_core::home_dir()?;
    let args = vec![
        "--no-session".to_string(),
        "--no-context-files".to_string(),
        "--no-skills".to_string(),
        "--no-builtin-tools".to_string(),
        "--no-approve".to_string(),
        "--mcp-config".to_string(),
        config.to_string_lossy().into_owned(),
    ];
    let id = state.terminals().open_program(&home, "pi", &args)?;
    // The pty queues this line even while Pi is still starting. It is delivered to Pi's
    // editor once it begins reading input, and because `mcp-auth` is an extension command
    // no model turn or model credential is involved.
    state
        .terminals()
        .write(id, &format!("/mcp-auth {}\r", request.source.as_str()))?;
    state.credentials().forget_oauth(request.source);

    Ok(Json(serde_json::json!({
        "id": id,
        "command": format!("/mcp-auth {}", request.source.as_str()),
    })))
}

#[derive(Debug, Deserialize)]
struct TokenChange {
    source: ai_team_core::ContextSource,
    /// The token to keep. Empty clears it, which is what emptying the field means.
    token: String,
}

/// Keep or clear a context source's token (D23).
///
/// The one route that receives a credential, and it never hands one back: the answer says
/// whether there is now a token, not what it is. A route that echoed what it had just been
/// given would put the value in a response body, a browser cache and any log in between.
async fn set_token(
    State(state): State<AppState>,
    JsonBody(change): JsonBody<TokenChange>,
) -> Result<Json<serde_json::Value>> {
    state
        .credentials()
        .set_token(change.source, &change.token)?;
    Ok(Json(serde_json::json!({
        "source": change.source.as_str(),
        "token_set": state.credentials().has_token(change.source),
    })))
}

#[derive(Debug, Deserialize)]
struct FallbackChange {
    order: Vec<ai_team_core::Provider>,
}

async fn set_fallback(
    JsonBody(change): JsonBody<FallbackChange>,
) -> Result<Json<serde_json::Value>> {
    let (path, _) = ai_team_core::ensure_machine_profile()?;
    ai_team_core::set_fallback(&path, &change.order)?;
    Ok(Json(serde_json::json!({ "order": change.order.len() })))
}

/// What this machine is, structured.
///
/// One of the two routes that works *without* a database, along with `/update`. Every
/// other route 503s and says `ait init`, which is right for them and exactly wrong here:
/// a report about a machine with no database is the report somebody most needs.
async fn doctor(State(state): State<AppState>) -> Json<ai_team_core::Report> {
    // Read in one synchronous window and the lock dropped before the awaits inside the
    // report: a `rusqlite::Connection` is `Send` but not `Sync`, so a future holding one
    // cannot be a handler.
    let known = state
        .store()
        .ok()
        .map(|store| ai_team_core::Known::of(&store.lock()));
    Json(ai_team_core::readiness_report_with(known.as_ref(), state.credentials()).await)
}

#[derive(Debug, Deserialize)]
struct FixRequest {
    /// Exactly one repair, named. There is deliberately no "fix everything": that is a
    /// button somebody presses without reading.
    action: ai_team_core::Action,
}

/// Apply one of the repairs ai-team owns (D17).
///
/// The set of things this can cause is the set of `Action` variants and nothing else -
/// anything needing a command or a person is not representable here, rather than being
/// refused at the end of a function that looked like it might do it.
async fn doctor_fix(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<FixRequest>,
) -> Result<Json<serde_json::Value>> {
    let _ = &state;
    let outcome = ai_team_core::apply_fix(request.action)?;
    Ok(Json(serde_json::json!({ "done": outcome })))
}

async fn update_check() -> Result<Json<ai_team_core::Available>> {
    Ok(Json(ai_team_core::check_update().await))
}

/// What an update did, reported at the end rather than streamed.
///
/// The steps are bounded and short - download, verify, replace - and an SSE channel for
/// four of them would be more moving parts than the thing it reports on. The window shows
/// an indeterminate bar while this request is in flight, which is honest: nobody knows
/// how long a download takes.
#[derive(Debug, Serialize)]
struct Updated {
    version: String,
    /// Always true when this returns Ok. The window restarts on it rather than inferring
    /// success from the absence of an error.
    restart_required: bool,
}

async fn update_apply(State(state): State<AppState>) -> Result<Json<Updated>> {
    let available = ai_team_core::check_update().await;

    if let Some(blocked) = available.blocked {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            blocked,
        )));
    }
    let Some(latest) = available.latest.filter(|_| available.can_update) else {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "already up to date",
        )));
    };

    if available.method == ai_team_core::Method::Source {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "this copy was built from source - update it with `cargo install --git \
             https://github.com/zottiben/ai-team ai-team --locked`",
        )));
    }

    // The binary actually running, not whichever `ait` is first on PATH: the window is
    // served by this process and it is this process that has to be replaced.
    let binary = std::env::current_exe().map_err(|error| {
        crate::error::Error::Core(ai_team_core::Error::invalid(format!("where am I? {error}")))
    })?;

    // Which program that is comes from whoever started the server. This route is reached
    // from `ait ui` and from the desktop app alike, and a release ships both - so an
    // updater left to guess installs `ait` over the app, which then stops opening.
    ai_team_core::apply_update(&latest, &binary, state.host(), |_| {}).await?;
    Ok(Json(Updated {
        version: latest,
        restart_required: true,
    }))
}

#[derive(Debug, Serialize)]
struct Scm {
    branch: Option<String>,
    branches: Vec<String>,
    /// Worktree against index - what pressing stage would add.
    unstaged: Vec<ai_team_core::FileDiff>,
    /// Index against HEAD - what pressing commit would record. Kept apart, because their
    /// sum cannot tell you either one.
    staged: Vec<ai_team_core::FileDiff>,
    /// Files git has never seen. They have no diff at all, so a view built only from
    /// `git diff` leaves an agent's new file out of the commit entirely.
    untracked: Vec<String>,
}

async fn scm(State(state): State<AppState>, Query(query): Query<TreeQuery>) -> Result<Json<Scm>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(Scm {
        branch: ai_team_core::current_branch(&worktree).await,
        branches: ai_team_core::branches(&worktree).await?,
        unstaged: ai_team_core::parse_diff(&ai_team_core::worktree_diff(&worktree).await?),
        staged: ai_team_core::parse_diff(&ai_team_core::staged_diff(&worktree).await?),
        untracked: ai_team_core::untracked(&worktree).await?,
    }))
}

#[derive(Debug, Deserialize)]
struct StageRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    path: String,
    /// Which hunk, when staging one. Absent stages the whole file.
    #[serde(default)]
    hunk: Option<usize>,
    /// True to take it back out of the index.
    #[serde(default)]
    unstage: bool,
}

async fn scm_stage(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<StageRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;

    if request.unstage {
        ai_team_core::unstage(&worktree, &request.path).await?;
        return Ok(Json(serde_json::json!({ "unstaged": request.path })));
    }

    let Some(index) = request.hunk else {
        ai_team_core::stage(&worktree, &request.path).await?;
        return Ok(Json(serde_json::json!({ "staged": request.path })));
    };

    // Re-read rather than trusting what the window last saw: it polls, so the diff it is
    // showing may be a second old, and a hunk index means nothing against a stale diff.
    let files = ai_team_core::parse_diff(&ai_team_core::worktree_diff(&worktree).await?);
    let file = files
        .iter()
        .find(|file| file.path == request.path)
        .ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid(
                "that file has no unstaged changes any more",
            ))
        })?;
    let hunk = file.hunks.get(index).ok_or_else(|| {
        crate::error::Error::Core(ai_team_core::Error::invalid(
            "that hunk is gone - the file has changed since the view was drawn",
        ))
    })?;

    ai_team_core::apply_patch_cached(&worktree, &ai_team_core::patch_for(file, hunk)).await?;
    Ok(Json(
        serde_json::json!({ "staged": request.path, "hunk": index }),
    ))
}

#[derive(Debug, Deserialize)]
struct CommitRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    message: String,
}

async fn scm_commit(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<CommitRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    let sha = ai_team_core::commit_staged(&worktree, &request.message).await?;
    Ok(Json(serde_json::json!({ "sha": sha })))
}

#[derive(Debug, Deserialize)]
struct BranchRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    branch: String,
}

async fn scm_branch(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<BranchRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    ai_team_core::checkout(&worktree, &request.branch).await?;
    Ok(Json(serde_json::json!({ "branch": request.branch })))
}

/// Push the current branch.
///
/// Publishing is not a node's call - `irreversible.ts` refuses it from a generated agent
/// - but it is squarely the operator's, and this is their surface.
async fn scm_push(
    State(state): State<AppState>,
    JsonBody(query): JsonBody<TreeQuery>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    let output = ai_team_core::push(&worktree).await?;
    Ok(Json(serde_json::json!({ "pushed": output.trim() })))
}

#[derive(Debug, Deserialize)]
struct TerminalQuery {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
}

async fn terminals(
    State(state): State<AppState>,
    Query(query): Query<TerminalQuery>,
) -> Result<Json<Vec<ai_team_core::Listed>>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(state.terminals().list(Some(&worktree))))
}

async fn open_terminal(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<TerminalQuery>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    let id = state.terminals().open(&worktree)?;
    Ok(Json(serde_json::json!({ "id": id })))
}

/// Where the reader has got to. Absolute, so a reconnecting window asks from a number it
/// already has rather than starting the output again.
#[derive(Debug, Deserialize)]
struct CursorQuery {
    #[serde(default)]
    cursor: u64,
}

async fn read_terminal(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<CursorQuery>,
) -> Result<Json<ai_team_core::Chunk>> {
    Ok(Json(state.terminals().read(id, query.cursor)?))
}

#[derive(Debug, Deserialize)]
struct Keys {
    text: String,
}

async fn write_terminal(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    JsonBody(keys): JsonBody<Keys>,
) -> Result<Json<serde_json::Value>> {
    state.terminals().write(id, &keys.text)?;
    Ok(Json(serde_json::json!({ "sent": keys.text.len() })))
}

#[derive(Debug, Deserialize)]
struct Size {
    rows: u16,
    cols: u16,
}

/// Not cosmetic: a shell that thinks it has eighty columns wraps at eighty, and the
/// output arrives already broken in a way no styling undoes.
async fn resize_terminal(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    JsonBody(size): JsonBody<Size>,
) -> Result<Json<serde_json::Value>> {
    state.terminals().resize(id, size.rows, size.cols)?;
    Ok(Json(serde_json::json!({ "resized": id })))
}

async fn close_terminal(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>> {
    state.terminals().close(id);
    Ok(Json(serde_json::json!({ "closed": id })))
}

/// What every language request needs: which checkout, which file, and its current text.
///
/// The text comes from the editor rather than from disk, because the whole point is to
/// see a mistake before saving it. A server told about the file on disk would report
/// diagnostics for code the human is no longer looking at.
#[derive(Debug, Deserialize)]
struct LspRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    path: String,
    text: String,
    #[serde(default)]
    line: i64,
    #[serde(default)]
    character: i64,
    #[serde(default)]
    new_name: Option<String>,
}

impl LspRequest {
    fn at(&self) -> ai_team_core::Position {
        ai_team_core::Position {
            line: self.line,
            character: self.character,
        }
    }
}

/// Resolve the server for a request and tell it what the buffer currently says.
///
/// `None` when no server handles this language, which is the ordinary answer for a README
/// and is not an error.
async fn attach(
    state: &AppState,
    request: &LspRequest,
) -> Result<Option<std::sync::Arc<ai_team_core::Client>>> {
    let worktree = worktree_for(
        state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    let Some(client) = state.lsp().for_file(&worktree, &request.path).await? else {
        return Ok(None);
    };
    client.sync(&request.path, &request.text).await?;
    Ok(Some(client))
}

#[derive(Debug, Serialize)]
struct Diagnostics {
    /// False when no server handles this language. The gutter draws nothing rather than
    /// implying the file is clean.
    analysed: bool,
    /// None until the server has published for this file at all - which is not the same
    /// as an empty list, and drawing them the same way says a file is clean when nobody
    /// has looked at it yet.
    diagnostics: Option<Vec<ai_team_core::Diagnostic>>,
}

async fn diagnostics(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<LspRequest>,
) -> Result<Json<Diagnostics>> {
    let Some(client) = attach(&state, &request).await? else {
        return Ok(Json(Diagnostics {
            analysed: false,
            diagnostics: None,
        }));
    };
    Ok(Json(Diagnostics {
        analysed: true,
        diagnostics: client.diagnostics(&request.path).await,
    }))
}

async fn hover(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<LspRequest>,
) -> Result<Json<Option<ai_team_core::Hover>>> {
    let Some(client) = attach(&state, &request).await? else {
        return Ok(Json(None));
    };
    Ok(Json(client.hover(&request.path, request.at()).await?))
}

async fn definition(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<LspRequest>,
) -> Result<Json<Vec<ai_team_core::Location>>> {
    let Some(client) = attach(&state, &request).await? else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(client.definition(&request.path, request.at()).await?))
}

async fn completion(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<LspRequest>,
) -> Result<Json<Vec<String>>> {
    let Some(client) = attach(&state, &request).await? else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(client.completion(&request.path, request.at()).await?))
}

#[derive(Debug, Serialize)]
struct RenameEdit {
    path: String,
    range: ai_team_core::Range,
    new_text: String,
}

/// What a rename would change, without changing it.
///
/// Returned rather than applied: a rename can touch files the human has open with unsaved
/// work in them, and writing over that from under the editor is the kind of thing nobody
/// forgives. The window shows the edits and applies them through the buffers it owns.
async fn rename(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<LspRequest>,
) -> Result<Json<Vec<RenameEdit>>> {
    let Some(new_name) = request.new_name.clone() else {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "a rename needs a new name",
        )));
    };
    let Some(client) = attach(&state, &request).await? else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(
        client
            .rename(&request.path, request.at(), &new_name)
            .await?
            .into_iter()
            .map(|(path, range, new_text)| RenameEdit {
                path,
                range,
                new_text,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    q: String,
    #[serde(default = "ten")]
    limit: usize,
}

fn ten() -> usize {
    10
}

/// Search, which is file-sql's job and not ai-team's.
///
/// A checkout with no index is told to make one rather than handed a substring scan:
/// ai-team having its own worse search is how the good one stops being used (D4).
async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<ai_team_core::Hit>>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(
        ai_team_core::FileSql::at(worktree)
            .search(&query.q, query.limit)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct RemindersQuery {
    project: Option<String>,
    kind: Option<ai_team_core::ReminderKind>,
}

async fn reminders(
    State(state): State<AppState>,
    Query(query): Query<RemindersQuery>,
) -> Result<Json<Vec<ai_team_core::Reminder>>> {
    let store = state.store()?;
    let store = store.lock();
    let project = match &query.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    Ok(Json(store.reminders(project, query.kind)?))
}

#[derive(Debug, Deserialize)]
struct NewReminderRequest {
    title: String,
    #[serde(default)]
    kind: Option<ai_team_core::ReminderKind>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    recur: Option<ai_team_core::Recur>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

async fn add_reminder(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<NewReminderRequest>,
) -> Result<Json<ai_team_core::Reminder>> {
    let store = state.store()?;
    let mut store = store.lock();
    let project = match &request.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    Ok(Json(store.add_reminder(ai_team_core::NewReminder {
        project_id: project,
        team_id: None,
        kind: request.kind,
        title: request.title,
        body: request.body.unwrap_or_default(),
        prompt: request.prompt,
        due_at: request.due_at,
        recur: request.recur,
    })?))
}

/// Cancelled rather than deleted: what was scheduled and then called off is worth being
/// able to see, and `event` is not the only place history matters.
async fn drop_reminder(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ai_team_core::Reminder>> {
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(store.set_reminder_status(
        id,
        ai_team_core::ReminderStatus::Cancelled,
    )?))
}

#[derive(Debug, Deserialize)]
struct AnalyticsQuery {
    #[serde(default = "by_agent")]
    by: ai_team_core::By,
    project: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
}

fn by_agent() -> ai_team_core::By {
    ai_team_core::By::Agent
}

/// One row per group, with the ratios computed where they mean something.
///
/// The ratios are `Option` all the way to the client: a pairing with no history has not
/// earned a bad score, and a 0% rendered for a model nobody has tried is how a good model
/// gets retired.
#[derive(Debug, Serialize)]
struct AnalyticsRow {
    #[serde(flatten)]
    row: ai_team_core::Row,
    accepted_rate: Option<f64>,
    rework: Option<f64>,
    total_input: i64,
    input_per_accepted: Option<f64>,
    cache_hit_rate: Option<f64>,
    yield_per_k: Option<f64>,
    gate_pass_rate: Option<f64>,
    cycle_time: Option<f64>,
}

async fn analytics(
    State(state): State<AppState>,
    Query(query): Query<AnalyticsQuery>,
) -> Result<Json<Vec<AnalyticsRow>>> {
    let worktree = match (&query.project, query.workspace.as_deref()) {
        (Some(project), requested) => runtime_scope_for(&state, project, requested).await?,
        (None, Some(_)) => {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "workspace analytics also need a project",
            )))
        }
        (None, None) => None,
    };
    let store = state.store()?;
    let store = store.lock();
    let project = match &query.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    let worktree = worktree.as_ref().map(|path| path.to_string_lossy());
    Ok(Json(
        ai_team_core::rollup_workspace(&store, query.by, project, worktree.as_deref())?
            .into_iter()
            .map(|row| AnalyticsRow {
                accepted_rate: row.accepted_rate(),
                rework: row.rework(),
                total_input: row.total_input(),
                input_per_accepted: row.input_per_accepted(),
                cache_hit_rate: row.cache_hit_rate(),
                yield_per_k: row.yield_per_k(),
                gate_pass_rate: row.gate_pass_rate(),
                cycle_time: row.cycle_time(),
                row,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct ReviewQuery {
    project: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    open_only: bool,
}

async fn reviews(
    State(state): State<AppState>,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<Vec<ai_team_core::Review>>> {
    let worktree = match (&query.project, query.workspace.as_deref()) {
        (Some(project), requested) => runtime_scope_for(&state, project, requested).await?,
        (None, Some(_)) => {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "workspace reviews also need a project",
            )))
        }
        (None, None) => None,
    };
    let store = state.store()?;
    let store = store.lock();
    let project = match &query.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    let reviews = store.reviews(project, query.open_only)?;
    let reviews = match worktree {
        Some(worktree) => reviews
            .into_iter()
            .map(|review| Ok((review_in_workspace(&store, &review, &worktree)?, review)))
            .collect::<ai_team_core::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(belongs, review)| belongs.then_some(review))
            .collect(),
        None => reviews,
    };
    Ok(Json(reviews))
}

fn review_in_workspace(
    store: &ai_team_core::Store,
    review: &ai_team_core::Review,
    worktree: &std::path::Path,
) -> ai_team_core::Result<bool> {
    let run_id = match (review.run_id, review.node_run_id) {
        (Some(run), _) => Some(run),
        (None, Some(node)) => Some(store.node_run(node)?.run_id),
        (None, None) => None,
    };
    let Some(run_id) = run_id else {
        return Ok(false);
    };
    run_in_workspace(store, run_id, worktree)
}

/// A review, its diff, and everything anybody has said about it.
#[derive(Debug, Serialize)]
struct ReviewDetail {
    #[serde(flatten)]
    review: ai_team_core::Review,
    files: Vec<ai_team_core::FileDiff>,
    comments: Vec<ai_team_core::ReviewComment>,
    /// Whether the node that wrote this is still up. The surface says which of the
    /// things submitting will do, rather than letting it be a surprise.
    steerable: bool,
    /// Whether, its seat finished, the pull request is still open where it was built - so
    /// the comments would go back into that worktree rather than onto the plan.
    follows_up: bool,
}

async fn review(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<ReviewDetail>> {
    let (review, comments, slug) = {
        let store = state.store()?;
        let store = store.lock();
        let review = store.review(id)?;
        let comments = store.comments(id)?;
        let slug = store.project(review.project_id)?.slug;
        (review, comments, slug)
    };
    let repo = worktree_for(&state, &slug, None, query.workspace.as_deref()).await?;
    let scope = runtime_scope_for(&state, &slug, query.workspace.as_deref()).await?;

    // Read the node in a synchronous window, then let the lock go: what follows talks to
    // git and to the agent, and a future holding a `Store` is neither `Send` nor
    // spawnable.
    let (node, target, pr) = {
        let store = state.store()?;
        let store = store.lock();
        if let Some(scope) = &scope {
            if !review_in_workspace(&store, &review, scope)? {
                return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                    "that review does not belong to the selected workspace",
                )));
            }
        }
        let node = ai_team_core::responsible(&store, &review);
        // Which plan and slice this review's work is, to find what it stacks on.
        let pr = review
            .run_id
            .and_then(|run| store.run(run).ok()?.plan_slug)
            .zip(node.as_ref().and_then(|node| node.slice_key.clone()));
        (node, ai_team_core::follow_up_target(&store, &review), pr)
    };

    // A stacked PR is reviewed against the one it stacks on, read from the plan now
    // rather than pinned, because a stack is rebased when its parent moves.
    let stacked_on = match &pr {
        Some((plan, slice_key)) => stacked_base(&repo, plan, slice_key).await,
        None => None,
    };
    let files = ai_team_core::diff_against(&review, &repo, stacked_on.as_deref()).await?;
    let steerable = ai_team_core::steerable(node);
    let follows_up = match &target {
        Some(target) => !steerable && ai_team_core::follow_up_ready(target).await,
        None => false,
    };

    Ok(Json(ReviewDetail {
        review,
        files,
        comments,
        steerable,
        follows_up,
    }))
}

/// The branch a PR stacks on, when it stacks on another PR of its plan. `None` for a PR
/// based on the default branch, and whenever the plan cannot be read: the review then
/// measures from the default branch, as it always has.
async fn stacked_base(repo: &std::path::Path, plan: &str, slice_key: &str) -> Option<String> {
    let slices = ai_team_core::Planner::at(repo)
        .for_plan(plan)
        .slices()
        .await
        .ok()?;
    let slice = slices.iter().find(|slice| slice.key == slice_key)?;
    ai_team_core::stacked_on(slice, &slices).map(|parent| parent.branch.clone().unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct CommentRequest {
    #[serde(default)]
    parent_id: Option<i64>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    side: Option<ai_team_core::DiffSide>,
    #[serde(default)]
    line_start: Option<i64>,
    #[serde(default)]
    line_end: Option<i64>,
    body: String,
}

async fn add_comment(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(request): JsonBody<CommentRequest>,
) -> Result<Json<ai_team_core::ReviewComment>> {
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(store.comment(
        id,
        ai_team_core::NewComment {
            parent_id: request.parent_id,
            file_path: request.file_path,
            side: request.side,
            line_start: request.line_start,
            line_end: request.line_end,
            // The window is the human's surface; anything an agent writes goes in
            // through core, not through here.
            author: "human".into(),
            body: request.body,
        },
    )?))
}

async fn resolve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ai_team_core::ReviewComment>> {
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(store.resolve_comment(
        id,
        ai_team_core::CommentStatus::Resolved,
    )?))
}

#[derive(Debug, Deserialize)]
struct SubmitRequest {
    status: ai_team_core::ReviewStatus,
    #[serde(default)]
    workspace: Option<String>,
}

/// Submit a review, which either steers the node that wrote the code or puts the work on
/// the plan for whoever picks it up next.
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(request): JsonBody<SubmitRequest>,
) -> Result<Json<ai_team_core::Submitted>> {
    // Everything the database knows, read before any await and released after.
    let (pending, node, orchestrator, slug, follow_up) = {
        let store = state.store()?;
        let store = store.lock();
        let pending = ai_team_core::pending_review(&store, id)?;
        let node = ai_team_core::responsible(&store, &pending.review);
        // The orchestrator too: it decides what the work is, and a correction that only
        // reaches the author leaves the plan still saying the old thing.
        let orchestrator = ai_team_core::conductor(&store, &pending.review);
        let slug = store.project(pending.review.project_id)?.slug;
        let follow_up = ai_team_core::follow_up_target(&store, &pending.review);
        (pending, node, orchestrator, slug, follow_up)
    };
    let repo = worktree_for(&state, &slug, None, request.workspace.as_deref()).await?;
    let scope = runtime_scope_for(&state, &slug, request.workspace.as_deref()).await?;
    if let Some(scope) = &scope {
        let store = state.store()?;
        let store = store.lock();
        if !review_in_workspace(&store, &pending.review, scope)? {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that review does not belong to the selected workspace",
            )));
        }
    }

    // A finished seat whose pull request is still open where it was built: the comments go
    // back into that worktree, as a follow-up run, rather than onto the plan as new work
    // somebody would start without the code they are about (PW10).
    if let Some(target) = follow_up {
        if !pending.open.is_empty() && ai_team_core::follow_up_ready(&target).await {
            // The slice first, as when steering: the verifier checks the PR against it.
            ai_team_core::Planner::at(&repo)
                .for_plan(target.plan.clone())
                .amend_scope(&target.slice_key, &ai_team_core::amendment(&pending.open))
                .await?;
            let (db, run_id) = {
                let store = state.store()?;
                let mut store = store.lock();
                let run = ai_team_core::open_follow_up(&mut store, &target)?;
                store.submit_review(id, request.status)?;
                (store.path().to_path_buf(), run.id)
            };
            let comments = pending.message.clone();
            let slice_key = target.slice_key.clone();
            // Detached: the follow-up takes minutes, and records itself in the database
            // the window is already watching.
            tokio::spawn(async move {
                if let Err(error) = ai_team_core::follow_up_at(&db, run_id, target, comments).await
                {
                    eprintln!("ai-team: review follow-up run {run_id} stopped: {error}");
                }
            });
            return Ok(Json(ai_team_core::Submitted::FollowedUp {
                slice_key,
                run_id,
                comments: pending.open.len(),
            }));
        }
    }

    // Both paths write to ai-planner through its own CLI (D4).
    let for_plan = repo.clone();
    let queue_state = state.clone();
    let outcome = ai_team_core::deliver_review(
        &pending,
        node,
        orchestrator,
        |title, scope| async move {
            ai_team_core::Planner::at(for_plan)
                .add_slice(
                    "RV",
                    &title,
                    &scope,
                    "the comments are addressed and the gates pass",
                    // The feedback could name anywhere in the diff, so the slice is
                    // offered to whichever seat owns what it mentions rather than pinned
                    // to a zone chosen here.
                    &["**"],
                )
                .await
        },
        |key, addition| async move {
            ai_team_core::Planner::at(repo)
                .amend_scope(&key, &addition)
                .await
        },
        // The lock is taken and dropped inside the callback, so it is never held across
        // one of the awaits above.
        |node_run_id, agent_id, message| {
            let store = queue_state
                .store()
                .map_err(|_| ai_team_core::Error::invalid("the database is not open"))?;
            let mut store = store.lock();
            store.queue_node_message(node_run_id, agent_id, message)?;
            Ok(())
        },
    )
    .await?;

    // Recorded only once delivery succeeded. A review marked submitted after a refused
    // hand-off is the quiet failure this path exists to avoid.
    {
        let store = state.store()?;
        let mut store = store.lock();
        store.submit_review(id, request.status)?;
    }
    Ok(Json(outcome))
}

/// What to work on now, across every project.
///
/// The ranking lives in core and is tested there; this gathers the two halves - what
/// ai-team's own database knows, and the questions ai-planner is holding - and hands
/// them over in one order.
async fn today(State(state): State<AppState>) -> Result<Json<Vec<ai_team_core::Item>>> {
    // Two passes, with the lock released in between, because reading the plans means
    // running `aip` once per checkout and no request should hold a database connection
    // across that.
    let (mut items, checkouts) = {
        let store = state.store()?;
        let store = store.lock();
        (
            ai_team_core::today_from_store(&store)?,
            ai_team_core::today_checkouts(&store)?,
        )
    };
    items.extend(ai_team_core::today_from_plans(checkouts).await);

    Ok(Json(ai_team_core::rank_today(items)))
}

/// A slice, plus the one thing ai-team knows about it that ai-planner does not: which
/// seat would build it.
#[derive(Debug, Serialize)]
struct BoardSlice {
    #[serde(flatten)]
    slice: ai_team_core::Slice,
    /// The seat whose zone owns the paths this slice touches, if any owns them. `None`
    /// is a real answer - a slice nobody owns is reported rather than guessed at, which
    /// is the same rule dispatch follows. For a PR built as tasks, its first task's owner.
    owner: Option<String>,
    /// Every seat that builds it, in the order each first builds (PW4): its tasks'
    /// owners, or the zone owner of a slice built whole. Empty when nobody can.
    crew: Vec<String>,
    /// The paths it declared, so a card can say why it routed where it did.
    touches: Vec<String>,
    /// Blocked only for explicit ai-team plan approval, never a substantive failure.
    approval_held: bool,
    /// The accepted node that owns this slice's exact local/remote delivery state.
    delivery: Option<SliceDelivery>,
}

#[derive(Debug, Clone, Serialize)]
struct SliceDelivery {
    run_id: i64,
    node_run_id: i64,
    branch: String,
    pushed_at: Option<String>,
    pr_url: Option<String>,
    merge_requested_at: Option<String>,
    delivery_claim: Option<String>,
    delivery_claimed_at: Option<String>,
    delivery_error: Option<String>,
    policy: ai_team_core::DeliverySettings,
    remote: Option<ai_team_core::RemoteDeliveryStatus>,
}

#[derive(Debug, Serialize)]
struct Board {
    /// `None` when this checkout has no plan yet, which is where every new project
    /// starts. A repository nobody has run `aip new` in is not a broken one, and
    /// answering with ai-planner's own "not registered" error makes a first look at the
    /// Board a red message about a tool the reader may not have met.
    plan: Option<ai_team_core::PlanSummary>,
    slices: Vec<BoardSlice>,
    /// What to do about it, when there is nothing to show.
    next_step: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BoardQuery {
    /// Which project's plan. The board belongs to a checkout, and a project is how the
    /// window names one.
    project: String,
    #[serde(default)]
    workspace: Option<String>,
}

/// Resolve the selected checkout, which is where its plan lives.
async fn planner_for(
    state: &AppState,
    project: &str,
    workspace: Option<&str>,
) -> Result<(ai_team_core::Planner, i64)> {
    let team_id = {
        let store = state.store()?;
        let store = store.lock();
        store.find_project(project)?.team_id.ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid("that project has no team"))
        })?
    };
    let worktree = worktree_for(state, project, None, workspace).await?;
    Ok((ai_team_core::Planner::at(worktree), team_id))
}

async fn board(
    State(state): State<AppState>,
    Query(query): Query<BoardQuery>,
) -> Result<Json<Board>> {
    // The planner is resolved, and the store lock released, before any of the awaits
    // below: `aip` is another program, and holding a database connection across it
    // would block every other request for the duration.
    let (planner, team_id) =
        planner_for(&state, &query.project, query.workspace.as_deref()).await?;

    // A checkout with no plan is a normal, early state rather than a failure, so it is
    // reported as an empty board with an instruction instead of ai-planner's own error.
    let Ok(plan) = planner.current().await else {
        return Ok(Json(Board {
            plan: None,
            slices: Vec::new(),
            next_step: Some(
                "This checkout has no plan yet. Run `aip new \"<what you are building>\"` \
                 in it, or start a run and the orchestrator will write one."
                    .into(),
            ),
        }));
    };
    let slices = planner.slices().await?;

    let (db_path, mut slices) = {
        let store = state.store()?;
        let store = store.lock();
        let project_id = store.find_project(&query.project)?.id;
        let delivery = slice_deliveries(
            &store,
            project_id,
            planner.root(),
            &plan.plan,
            store.team(team_id)?.delivery,
        )?;
        let db_path = store.path().to_path_buf();
        let slices = slices
            .into_iter()
            .map(|slice| {
                let delivered = delivery.get(&slice.key).cloned();
                decorate_slice(&store, team_id, slice, delivered)
            })
            .collect::<Result<Vec<_>>>()?;
        (db_path, slices)
    };
    for slice in &mut slices {
        if let Some(delivery) = slice
            .delivery
            .as_mut()
            .filter(|delivery| delivery.pr_url.is_some())
        {
            delivery.remote = Some(
                ai_team_core::delivery_status(&db_path, delivery.node_run_id)
                    .await
                    .unwrap_or(ai_team_core::RemoteDeliveryStatus {
                        pr_state: "unknown".into(),
                        checks: "unavailable".into(),
                    }),
            );
        }
    }
    Ok(Json(Board {
        plan: Some(plan),
        slices,
        next_step: None,
    }))
}

fn slice_deliveries(
    store: &ai_team_core::Store,
    project_id: i64,
    workspace: &std::path::Path,
    plan_slug: &str,
    policy: ai_team_core::DeliverySettings,
) -> Result<std::collections::HashMap<String, SliceDelivery>> {
    let mut found = std::collections::HashMap::new();
    for run in store
        .runs_in_workspace(project_id, workspace, 200)?
        .into_iter()
        .filter(|run| run.plan_slug.as_deref() == Some(plan_slug))
    {
        for node in store.node_runs(run.id)? {
            let (Some(slice), Some(branch)) = (node.slice_key.clone(), node.branch.clone()) else {
                continue;
            };
            if node.status != ai_team_core::NodeStatus::Done {
                continue;
            }
            let replace = found
                .get(&slice)
                .is_none_or(|delivery: &SliceDelivery| node.id > delivery.node_run_id);
            if replace {
                found.insert(
                    slice,
                    SliceDelivery {
                        run_id: run.id,
                        node_run_id: node.id,
                        branch,
                        pushed_at: node.pushed_at,
                        pr_url: node.pr_url,
                        merge_requested_at: node.merge_requested_at,
                        delivery_claim: node.delivery_claim,
                        delivery_claimed_at: node.delivery_claimed_at,
                        delivery_error: node.delivery_error,
                        policy,
                        remote: None,
                    },
                );
            }
        }
    }
    Ok(found)
}

fn decorate_slice(
    store: &ai_team_core::Store,
    team_id: i64,
    slice: ai_team_core::Slice,
    delivery: Option<SliceDelivery>,
) -> Result<BoardSlice> {
    let touches = slice.touches();
    let listed = slice.tasks();
    // Read the way dispatch reads it: task lines name their owners, and a slice with none
    // is one piece of work for the seat whose zone owns its paths.
    let crew: Vec<String> = if !listed.problems.is_empty() {
        Vec::new()
    } else if listed.tasks.is_empty() {
        touches
            .iter()
            .find_map(|path| store.agent_for_path(team_id, path).transpose())
            .transpose()?
            .map(|agent| agent.role)
            .into_iter()
            .collect()
    } else {
        let mut roles: Vec<String> = Vec::new();
        for task in listed.tasks {
            if !roles.contains(&task.owner) {
                roles.push(task.owner);
            }
        }
        roles
    };
    let owner = crew.first().cloned();
    let approval_held = slice.is_approval_held();
    Ok(BoardSlice {
        slice,
        owner,
        crew,
        touches,
        approval_held,
        delivery,
    })
}

#[derive(Debug, Serialize)]
struct BoardSliceDetail {
    slice: BoardSlice,
    log: Vec<ai_team_core::PlanLogEntry>,
}

/// One card with the progress recorded against it.
///
/// Both calls go through ai-planner and can run together. ai-team adds only the owner,
/// resolved from its team zones after the neighbour has answered.
async fn board_slice(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(query): Query<BoardQuery>,
) -> Result<Json<BoardSliceDetail>> {
    let (planner, team_id) =
        planner_for(&state, &query.project, query.workspace.as_deref()).await?;
    let (slice, log) = tokio::try_join!(planner.slice(&key), planner.logs(&key))?;
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(BoardSliceDetail {
        slice: decorate_slice(&store, team_id, slice, None)?,
        log,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditSliceRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    pr_url: String,
}

async fn edit_board_slice(
    State(state): State<AppState>,
    Path(key): Path<String>,
    JsonBody(request): JsonBody<EditSliceRequest>,
) -> Result<Json<serde_json::Value>> {
    let (planner, _) = planner_for(&state, &request.project, request.workspace.as_deref()).await?;
    planner.set_pr(&key, request.pr_url.trim()).await?;
    Ok(Json(serde_json::json!({ "edited": key })))
}

async fn claim_board_slice(
    State(state): State<AppState>,
    Path(key): Path<String>,
    JsonBody(request): JsonBody<ProjectRequest>,
) -> Result<Json<serde_json::Value>> {
    let (planner, _) = planner_for(&state, &request.project, request.workspace.as_deref()).await?;
    let root = planner.root().to_path_buf();
    if !planner.claim(&key, &root).await? {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that slice is already claimed",
        )));
    }
    Ok(Json(serde_json::json!({ "claimed": key })))
}

async fn release_board_slice(
    State(state): State<AppState>,
    Path(key): Path<String>,
    JsonBody(request): JsonBody<ProjectRequest>,
) -> Result<Json<serde_json::Value>> {
    let (planner, _) = planner_for(&state, &request.project, request.workspace.as_deref()).await?;
    let root = planner.root().to_path_buf();
    planner.release(&key, &root).await?;
    Ok(Json(serde_json::json!({ "released": key })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    body: String,
}

async fn note_board_slice(
    State(state): State<AppState>,
    Path(key): Path<String>,
    JsonBody(request): JsonBody<NoteRequest>,
) -> Result<Json<serde_json::Value>> {
    let body = request.body.trim();
    if body.is_empty() {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "a progress note cannot be empty",
        )));
    }
    let (planner, _) = planner_for(&state, &request.project, request.workspace.as_deref()).await?;
    planner.log(body, Some(&key)).await?;
    Ok(Json(serde_json::json!({ "noted": key })))
}

#[derive(Debug, Deserialize)]
struct MoveRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    status: String,
    /// Required by ai-planner when blocking, and worth having anyway.
    reason: Option<String>,
}

/// Move a card.
///
/// Writes through ai-planner rather than into ai-team's own database, so `aip status` in
/// a terminal and this board are the same state rather than two that agree until they
/// do not.
async fn move_slice(
    State(state): State<AppState>,
    Path(key): Path<String>,
    JsonBody(request): JsonBody<MoveRequest>,
) -> Result<Json<serde_json::Value>> {
    let (planner, _) = planner_for(&state, &request.project, request.workspace.as_deref()).await?;
    planner
        .set_status(&key, &request.status, request.reason.as_deref())
        .await?;
    Ok(Json(serde_json::json!({ "moved": key })))
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StartAction {
    #[default]
    Start,
    ApproveCurrent,
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    prompt: Option<String>,
    plan: Option<String>,
    #[serde(default)]
    replan: bool,
    #[serde(default)]
    plan_only: bool,
    #[serde(default)]
    approval_required: bool,
    /// Which branch the run works on: a fresh one unless the operator asks, for this run,
    /// for the default branch itself.
    #[serde(default)]
    branching: ai_team_core::Branching,
    /// Continue a current approval board, adopting the originating legacy orchestrator
    /// run first when it predates run/session-owned approval.
    #[serde(default)]
    action: StartAction,
}

async fn continue_current_approval(
    state: &AppState,
    request: &StartRequest,
    workspace: &std::path::Path,
    has_prompt: bool,
) -> Result<Json<serde_json::Value>> {
    if has_prompt
        || request.plan.is_some()
        || request.replan
        || request.plan_only
        || request.approval_required
        || request.branching != ai_team_core::Branching::Fresh
    {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "approving the current plan cannot also change how a run starts",
        )));
    }
    let current = ai_team_core::Planner::at(workspace).current().await?;
    let planner = ai_team_core::Planner::at(workspace).for_plan(current.plan.clone());
    let slices = planner.slices().await?;
    let has_legacy_hold = slices.iter().any(ai_team_core::Slice::is_approval_held);
    let (run_id, db) = {
        let store = state.store()?;
        let mut store = store.lock();
        let project = store.find_project(&request.project)?;
        let team_id = project.team_id.ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid("that project has no team"))
        })?;
        let existing = store.blocked_run_for_plan(
            project.id,
            team_id,
            &current.plan,
            [
                ai_team_core::PLAN_APPROVAL_REASON,
                ai_team_core::PLAN_APPROVAL_PREPARING_REASON,
            ],
        )?;
        if existing.is_none() && !has_legacy_hold {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "the current plan has no slices held only for approval",
            )));
        }
        let run = match existing {
            Some(run) => run,
            None => store.adopt_legacy_plan_approval(
                project.id,
                team_id,
                workspace,
                &current.plan,
                ai_team_core::PLAN_APPROVAL_PREPARING_REASON,
                ai_team_core::PLAN_APPROVAL_REASON,
            )?,
        };
        ai_team_core::claim_plan_approval(&mut store, run.id)?;
        (run.id, store.path().to_path_buf())
    };
    tokio::spawn(async move {
        if let Err(error) = ai_team_core::continue_approved_run_at(&db, run_id).await {
            eprintln!("ai-team: approved run {run_id} stopped: {error}");
        }
    });
    Ok(Json(serde_json::json!({
        "started": true,
        "continued": true,
        "run_id": run_id
    })))
}

/// Start a run from the window.
///
/// Returns as soon as the work is under way rather than when it finishes: a run takes
/// minutes, and an HTTP request that waits for one is a request that times out. The run
/// records itself in the database as it goes, which is what the window is already
/// watching - so "started" is genuinely all the caller needs.
async fn start(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<StartRequest>,
) -> Result<Json<serde_json::Value>> {
    // The board-owned approval fallback is a read-then-write operation in ai-planner.
    // Serialising starts inside this server closes the two-window double-click race; the
    // coherent run path has its own database claim as well.
    let _run_start = state.lock_run_start().await;

    // Resolve through Git on every request. Besides rejecting arbitrary paths, the
    // canonical checkout is the root used by the planner and every surface that follows
    // this run; a child-worktree request must never fall back to the project's main plan.
    let workspace =
        worktree_for(&state, &request.project, None, request.workspace.as_deref()).await?;
    let has_prompt = request
        .prompt
        .as_ref()
        .is_some_and(|prompt| !prompt.trim().is_empty());
    if request.action == StartAction::ApproveCurrent {
        return continue_current_approval(&state, &request, &workspace, has_prompt).await;
    }
    let prompt = request.prompt.filter(|prompt| !prompt.trim().is_empty());
    if prompt.is_none() {
        // "Build what is ready" must not step around a run that already owns this plan.
        // The normal UI continues that run directly; this server check closes stale-tab
        // and non-UI callers as well.
        if let Ok(current) = ai_team_core::Planner::at(&workspace).current().await {
            let coherent_run = {
                let store = state.store()?;
                let store = store.lock();
                let project = store.find_project(&request.project)?;
                match project.team_id {
                    Some(team_id) => store
                        .active_run_for_plan(project.id, team_id, &current.plan)?
                        .or(store.blocked_run_for_plan(
                            project.id,
                            team_id,
                            &current.plan,
                            [
                                ai_team_core::PLAN_APPROVAL_REASON,
                                ai_team_core::PLAN_APPROVAL_PREPARING_REASON,
                            ],
                        )?),
                    None => None,
                }
            };
            if let Some(run) = coherent_run {
                return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                    format!(
                        "run #{} owns this plan; continue that run instead of starting another",
                        run.id
                    ),
                )));
            }
        }
    }
    let spec = ai_team_core::Request {
        project: request.project,
        prompt,
        workspace: Some(workspace),
        plan: request.plan,
        width: None,
        replan: request.replan,
        plan_only: request.plan_only,
        approval_required: request.approval_required,
        branching: request.branching,
    };
    let (signal, started) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        // Its own store: `run_workflow` opens one, and the handler's is held by a mutex
        // the request will drop long before this finishes.
        let mut signal = Some(signal);
        let result = ai_team_core::run_workflow(&spec, |progress| {
            if let ai_team_core::Progress::Started { run_id, .. } = progress {
                if let Some(signal) = signal.take() {
                    let _ = signal.send(Ok(run_id));
                }
            }
        })
        .await;
        if let Err(error) = result {
            if let Some(signal) = signal.take() {
                let _ = signal.send(Err(error.to_string()));
            }
            eprintln!("ai-team: run failed: {error}");
        }
    });

    match started.await {
        Ok(Ok(run_id)) => Ok(Json(serde_json::json!({
            "started": true,
            "run_id": run_id
        }))),
        Ok(Err(error)) => Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            error,
        ))),
        Err(_) => Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "the run stopped before it could start",
        ))),
    }
}

/// Claim plan approval synchronously, then resume the existing run in the background.
/// The atomic claim makes a double-click or second open window an honest conflict rather
/// than two orchestrator continuations dispatching the same board.
async fn approve_plan(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>> {
    let db = {
        let store = state.store()?;
        let mut store = store.lock();
        ai_team_core::claim_plan_approval(&mut store, id)?;
        store.path().to_path_buf()
    };
    tokio::spawn(async move {
        if let Err(error) = ai_team_core::continue_approved_run_at(&db, id).await {
            eprintln!("ai-team: approved run {id} stopped: {error}");
        }
    });
    Ok(Json(serde_json::json!({ "continued": true, "run_id": id })))
}

/// What a run is parked on, if anything.
async fn approvals(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Vec<Event>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.pending_approvals(id)?))
}

#[derive(Debug, Serialize)]
struct ProjectView {
    #[serde(flatten)]
    project: Project,
    open_runs: usize,
}

async fn projects(State(state): State<AppState>) -> Result<Json<Vec<ProjectView>>> {
    let store = state.store()?;
    let store = store.lock();
    let mut out = Vec::new();
    for project in store.projects()? {
        let open_runs = store
            .runs(Some(project.id), 100)?
            .into_iter()
            .filter(|run| {
                !matches!(
                    run.status,
                    ai_team_core::RunStatus::Done
                        | ai_team_core::RunStatus::Failed
                        | ai_team_core::RunStatus::Cancelled
                )
            })
            .count();
        out.push(ProjectView { project, open_runs });
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct RunQuery {
    project: Option<i64>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

/// Whether a run belongs to the selected checkout: the same rule its run list follows, so
/// what the window lists there is what it may open, reply to, resume or deliver.
fn run_in_workspace(
    store: &ai_team_core::Store,
    run_id: i64,
    workspace: &std::path::Path,
) -> ai_team_core::Result<bool> {
    store.run_in_workspace(run_id, workspace)
}

async fn runs(
    State(state): State<AppState>,
    Query(query): Query<RunQuery>,
) -> Result<Json<Vec<Run>>> {
    let Some(requested) = query.workspace.as_deref() else {
        let store = state.store()?;
        let store = store.lock();
        return Ok(Json(store.runs(query.project, query.limit)?));
    };
    let project_id = query.project.ok_or_else(|| {
        crate::error::Error::Core(ai_team_core::Error::invalid(
            "a workspace run list also needs a project",
        ))
    })?;
    let slug = {
        let store = state.store()?;
        let store = store.lock();
        store.project(project_id)?.slug
    };
    let Some(worktree) = runtime_scope_for(&state, &slug, Some(requested)).await? else {
        let store = state.store()?;
        let store = store.lock();
        return Ok(Json(store.runs(Some(project_id), query.limit)?));
    };
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.runs_in_workspace(
        project_id,
        &worktree,
        query.limit,
    )?))
}

/// One run, with the nodes and the spend that make it readable at a glance.
#[derive(Debug, Serialize)]
struct RunDetail {
    #[serde(flatten)]
    run: Run,
    nodes: Vec<RunNode>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct RunNode {
    #[serde(flatten)]
    node: NodeRun,
    /// Read-only verifier seats are visible evidence, not conversations to steer.
    replyable: bool,
    /// The row says running but the local process that owned it is gone. Its session and
    /// dirty awt lease can be reattached instead of presenting permanent fake activity.
    recoverable: bool,
}

#[derive(Debug, Deserialize)]
struct RunDetailQuery {
    #[serde(default)]
    workspace: Option<String>,
}

async fn run(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<RunDetailQuery>,
) -> Result<Json<RunDetail>> {
    let (run, slug) = {
        let store = state.store()?;
        let store = store.lock();
        let run = store.run(id)?;
        let slug = store.project(run.project_id)?.slug;
        (run, slug)
    };
    let scope = runtime_scope_for(&state, &slug, query.workspace.as_deref()).await?;
    let store = state.store()?;
    let store = store.lock();
    if let Some(scope) = scope.as_deref() {
        if !run_in_workspace(&store, run.id, scope)? {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that run does not belong to the selected workspace",
            )));
        }
    }
    // From a pull request's worktree, only the turns that built or checked that PR.
    let nodes = match scope.as_deref() {
        Some(scope) => store.nodes_in_workspace(id, scope)?,
        None => store.node_runs(id)?,
    };
    let usage = nodes
        .iter()
        .fold(ai_team_core::Usage::default(), |mut total, node| {
            total += node.usage;
            total
        });
    let nodes = nodes
        .into_iter()
        .map(|node| {
            let writable = node
                .agent_id
                .and_then(|id| store.agent(id).ok())
                .is_some_and(|agent| !agent.read_only);
            let replyable = node.status == ai_team_core::NodeStatus::Running
                && node.session_id.is_some()
                && node.session_retired_at.is_none()
                && node.session_resetting_at.is_none()
                && writable;
            let recoverable = replyable
                && node.slice_key.is_some()
                && node.worktree_path.is_some()
                && !node
                    .supervisor_pid
                    .is_some_and(ai_team_core::process_is_alive);
            RunNode {
                node,
                replyable,
                recoverable,
            }
        })
        .collect();
    Ok(Json(RunDetail { run, nodes, usage }))
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    /// Events after this id. The window keeps the last one it saw and asks for the rest,
    /// so a reconnect never replays what is already on screen.
    after: Option<i64>,
    #[serde(default = "default_event_limit")]
    limit: i64,
    #[serde(default)]
    workspace: Option<String>,
}

fn default_event_limit() -> i64 {
    500
}

/// The part of an append-only event a person can use as a conversation.
///
/// The raw payload remains in SQLite as evidence. It does not cross this API: provider
/// payloads can carry encrypted reasoning signatures and enormous tool results, neither of
/// which belong in a browser rendering what the team is doing.
#[derive(Debug, Serialize)]
struct ActivityEvent {
    id: i64,
    node_run_id: Option<i64>,
    kind: ai_team_core::EventKind,
    actor: Option<String>,
    summary: String,
    message: Option<String>,
    thinking: Vec<String>,
    at: String,
}

impl From<Event> for ActivityEvent {
    fn from(event: Event) -> Self {
        let message = event_message(&event);
        let thinking = event_thinking(&event);
        let summary = event_summary(&event);
        ActivityEvent {
            id: event.id,
            node_run_id: event.node_run_id,
            kind: event.kind,
            actor: event.actor,
            summary,
            message,
            thinking,
            at: event.at,
        }
    }
}

fn event_summary(event: &Event) -> String {
    // Historical Pi streams used "completed" for `agent_settled`, even after a provider
    // error. Settled means the stream stopped retrying; it does not mean the work passed.
    if event.kind == ai_team_core::EventKind::Done && event.summary == "turn completed" {
        return "turn settled".into();
    }
    if event.kind != ai_team_core::EventKind::ToolCall {
        return event.summary.clone();
    }
    let Some(args) = event.payload.as_ref().and_then(|value| value.get("args")) else {
        return event.summary.clone();
    };
    // The useful, bounded part of common tools: where it is reading/writing, what command
    // it is running, or what it searched for. Never return the whole payload or results.
    let detail = ["path", "filePath", "command", "query", "pattern", "url"]
        .into_iter()
        .find_map(|key| args.get(key).and_then(serde_json::Value::as_str));
    let Some(detail) = detail else {
        return event.summary.clone();
    };
    let detail: String = detail
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect();
    format!("{} · {detail}", event.summary)
}

fn message_content(event: &Event) -> Option<&Vec<serde_json::Value>> {
    event
        .payload
        .as_ref()?
        .get("message")?
        .get("content")?
        .as_array()
}

fn event_message(event: &Event) -> Option<String> {
    if event.actor.as_deref() == Some("human") {
        return event
            .payload
            .as_ref()?
            .get("body")?
            .as_str()
            .map(str::to_string);
    }
    if event.kind != ai_team_core::EventKind::Note {
        return None;
    }
    let mut parts = Vec::new();
    for text in message_content(event)?
        .iter()
        .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        if !parts.contains(&text) {
            parts.push(text);
        }
    }
    let text = parts.join("\n");
    let lower = text.to_lowercase();
    if lower.contains("not logged in") && lower.contains("/login") {
        return Some(
            "The selected provider is not signed in. Open Settings and sign in before retrying this slice."
                .into(),
        );
    }
    (!text.is_empty()).then_some(text)
}

fn event_thinking(event: &Event) -> Vec<String> {
    // Pi repeats the final message in `message_end` and `turn_end`. Reading reasoning from
    // cost/turn_end only shows each provider summary once.
    if event.kind != ai_team_core::EventKind::Cost {
        return Vec::new();
    }
    if let Some(thinking) = event
        .payload
        .as_ref()
        .and_then(|value| value.pointer("/assistantMessageEvent"))
        .filter(|value| {
            value.get("type").and_then(serde_json::Value::as_str) == Some("thinking_end")
        })
        .and_then(|value| value.get("content"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return vec![thinking.to_string()];
    }
    message_content(event)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("thinking").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .collect()
}

async fn run_events(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<ActivityEvent>>> {
    let slug = {
        let store = state.store()?;
        let store = store.lock();
        let run = store.run(id)?;
        store.project(run.project_id)?.slug
    };
    let scope = runtime_scope_for(&state, &slug, query.workspace.as_deref()).await?;
    let store = state.store()?;
    let store = store.lock();
    let run = store.run(id)?;
    if let Some(scope) = scope.as_deref() {
        if !run_in_workspace(&store, run.id, scope)? {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that run does not belong to the selected workspace",
            )));
        }
    }
    let events = match scope.as_deref() {
        Some(scope) => store.events_in_workspace(id, scope, query.after, query.limit)?,
        None => store.events(id, query.after, query.limit)?,
    };
    Ok(Json(events.into_iter().map(ActivityEvent::from).collect()))
}

#[derive(Debug, Deserialize)]
struct ReplyRequest {
    message: String,
    workspace: String,
}

/// Reply inside one node's existing conversation rather than talking to its seat in the
/// abstract. The row is written before returning; the active supervisor sees the queue
/// and resumes the same Pi session before it verifies or returns the checkout.
async fn reply_to_node(
    State(state): State<AppState>,
    Path((run_id, node_id)): Path<(i64, i64)>,
    JsonBody(request): JsonBody<ReplyRequest>,
) -> Result<Json<serde_json::Value>> {
    if request.message.trim().is_empty() {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "say something",
        )));
    }

    let (slug, agent_id) = {
        let store = state.store()?;
        let store = store.lock();
        let run = store.run(run_id)?;
        let node = store.node_run(node_id)?;
        if node.run_id != run_id {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that node does not belong to that run",
            )));
        }
        if node.status != ai_team_core::NodeStatus::Running {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that turn is no longer active; replies stay with a live conversation",
            )));
        }
        let agent_id = node.agent_id.ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid(
                "that historical agent no longer exists",
            ))
        })?;
        if store.agent(agent_id)?.read_only {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that verifier is read-only; its turn is evidence, not a conversation to steer",
            )));
        }
        if node.session_id.is_none()
            || node.session_retired_at.is_some()
            || node.session_resetting_at.is_some()
        {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that turn has no active Pi session to continue",
            )));
        }
        (store.project(run.project_id)?.slug, agent_id)
    };

    let selected = worktree_for(&state, &slug, None, Some(&request.workspace)).await?;
    let belongs = {
        let store = state.store()?;
        let store = store.lock();
        run_in_workspace(&store, run_id, &selected)?
    };
    if !belongs {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that node does not belong to the selected workspace",
        )));
    }

    let waiting = {
        let store = state.store()?;
        let mut store = store.lock();
        store.queue_conversation(node_id, agent_id, &request.message)?
    };

    Ok(Json(serde_json::json!({
        "reached": "queued",
        "waiting": waiting,
    })))
}

#[derive(Debug, Deserialize)]
struct ResumeNodeRequest {
    workspace: String,
}

/// Claim and restart a maker whose supervising desktop/CLI process disappeared.
///
/// This is deliberately a continuation, not a new run or retry: the existing node row,
/// Pi session, planner claim and dirty awt lease are the evidence needed to pick up where
/// the interrupted process stopped.
async fn resume_node(
    State(state): State<AppState>,
    Path((run_id, node_id)): Path<(i64, i64)>,
    JsonBody(request): JsonBody<ResumeNodeRequest>,
) -> Result<Json<serde_json::Value>> {
    let (slug, previous_pid) = {
        let store = state.store()?;
        let store = store.lock();
        let run = store.run(run_id)?;
        let node = store.node_run(node_id)?;
        if node.run_id != run_id || node.status != ai_team_core::NodeStatus::Running {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that turn is no longer an interrupted running turn",
            )));
        }
        let agent_id = node.agent_id.ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid(
                "that historical agent no longer exists",
            ))
        })?;
        if store.agent(agent_id)?.read_only {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that verifier is read-only and cannot be resumed",
            )));
        }
        if node.session_id.is_none()
            || node.session_retired_at.is_some()
            || node.session_resetting_at.is_some()
            || node.slice_key.is_none()
            || node.worktree_path.is_none()
        {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that turn has no recoverable Pi session and worktree",
            )));
        }
        if node
            .supervisor_pid
            .is_some_and(ai_team_core::process_is_alive)
        {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that turn is still supervised; refresh its live activity",
            )));
        }
        (store.project(run.project_id)?.slug, node.supervisor_pid)
    };

    let selected = worktree_for(&state, &slug, None, Some(&request.workspace)).await?;
    let belongs = {
        let store = state.store()?;
        let store = store.lock();
        run_in_workspace(&store, run_id, &selected)?
    };
    if !belongs {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that node does not belong to the selected workspace",
        )));
    }

    let pid = i64::from(std::process::id());
    {
        let store = state.store()?;
        let mut store = store.lock();
        store.claim_node_supervision(node_id, pid, previous_pid)?;
    }
    let db = state.database_path()?;
    tokio::spawn(async move {
        let _ = ai_team_core::resume_interrupted_node(&db, run_id, node_id).await;
    });

    Ok(Json(serde_json::json!({
        "resumed": true,
        "run_id": run_id,
        "node_id": node_id,
    })))
}

#[derive(Debug, Deserialize)]
struct DeliveryRequest {
    action: ai_team_core::DeliveryAction,
    project: String,
    workspace: String,
}

async fn deliver_node(
    State(state): State<AppState>,
    Path((run_id, node_id)): Path<(i64, i64)>,
    JsonBody(request): JsonBody<DeliveryRequest>,
) -> Result<Json<NodeRun>> {
    let db = {
        let store = state.store()?;
        let store = store.lock();
        let node = store.node_run(node_id)?;
        if node.run_id != run_id {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that delivery node does not belong to this run",
            )));
        }
        let run = store.run(run_id)?;
        let project = store.project(run.project_id)?;
        if project.slug != request.project {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that delivery node does not belong to the selected project",
            )));
        }
        store.path().to_path_buf()
    };
    let selected = worktree_for(
        &state,
        &request.project,
        None,
        Some(request.workspace.as_str()),
    )
    .await?;
    let belongs = {
        let store = state.store()?;
        let store = store.lock();
        run_in_workspace(&store, run_id, &selected)?
    };
    if !belongs {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that delivery node does not belong to the selected workspace",
        )));
    }
    Ok(Json(
        ai_team_core::approve_delivery(&db, node_id, request.action).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ResetSessionRequest {
    workspace: String,
}

async fn reset_node_session(
    State(state): State<AppState>,
    Path((run_id, node_id)): Path<(i64, i64)>,
    JsonBody(request): JsonBody<ResetSessionRequest>,
) -> Result<Json<serde_json::Value>> {
    let slug = {
        let store = state.store()?;
        let store = store.lock();
        let run = store.run(run_id)?;
        store.project(run.project_id)?.slug
    };
    let selected = worktree_for(&state, &slug, None, Some(&request.workspace)).await?;
    let belongs = {
        let store = state.store()?;
        let store = store.lock();
        run_in_workspace(&store, run_id, &selected)?
    };
    if !belongs {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that node does not belong to the selected workspace",
        )));
    }

    let db = {
        let store = state.store()?;
        let mut store = store.lock();
        ai_team_core::claim_session_reset(&mut store, run_id, node_id)?;
        store.path().to_path_buf()
    };
    tokio::spawn(async move {
        if let Err(error) = ai_team_core::reset_claimed_session(&db, run_id, node_id).await {
            eprintln!("ai-team: session reset for node {node_id} stopped: {error}");
        }
    });
    Ok(Json(serde_json::json!({
        "resetting": true,
        "run_id": run_id,
        "node_run_id": node_id,
    })))
}

#[derive(Debug, Deserialize)]
struct NotificationQuery {
    #[serde(default = "default_notification_limit")]
    limit: i64,
}

fn default_notification_limit() -> i64 {
    60
}

async fn notifications(
    State(state): State<AppState>,
    Query(query): Query<NotificationQuery>,
) -> Result<Json<Vec<ai_team_core::Notification>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.notifications(query.limit)?))
}

async fn read_notification(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ai_team_core::Notification>> {
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(store.read_notification(id)?))
}

/// What changed, as far as a window needs to care.
///
/// Deliberately thin: a tick says *something* happened and hands over the cursor, and the
/// window re-reads whichever view it is showing. Streaming whole rows here would mean
/// this endpoint knowing what every surface renders.
#[derive(Debug, Serialize)]
struct Tick {
    latest_event: i64,
    latest_notification: i64,
    open_runs: usize,
}

async fn stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(POLL))
        .then(move |_| {
            let state = state.clone();
            async move {
                state.deliver_notifications().await;
                let tick = state.tick().unwrap_or(Tick {
                    latest_event: -1,
                    latest_notification: -1,
                    open_runs: 0,
                });
                Ok(SseEvent::default().json_data(tick).unwrap_or_default())
            }
        })
        // Only send when something moved. A window left open overnight should not
        // accumulate a night's worth of identical frames.
        .filter(dedupe());

    // The keep-alive is what stops an idle connection being reaped by anything between
    // here and the browser, even though both ends are on loopback.
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Suppress ticks that say the same thing as the one before.
fn dedupe() -> impl FnMut(&std::result::Result<SseEvent, Infallible>) -> bool {
    let mut last: Option<String> = None;
    move |event| {
        let rendered = format!("{event:?}");
        if last.as_deref() == Some(rendered.as_str()) {
            return false;
        }
        last = Some(rendered);
        true
    }
}

impl AppState {
    /// Best-effort native delivery while a window is open. `ait daemon` performs the
    /// same atomic claim, so whichever process sees an item first is its sole deliverer.
    async fn deliver_notifications(&self) {
        let Ok(pending) = self.store().and_then(|store| {
            let mut store = store.lock();
            store.claim_notification_delivery(25).map_err(Into::into)
        }) else {
            return;
        };

        for notification in pending {
            let delivered = ai_team_core::notify(&notification.title, &notification.body).await;
            if let Ok(store) = self.store() {
                let _ = store
                    .lock()
                    .complete_notification_delivery(notification.id, delivered);
            }
        }
    }

    /// The cheap "has anything moved?" query behind the stream.
    fn tick(&self) -> Result<Tick> {
        let store = self.store()?;
        let store = store.lock();
        let runs = store.runs(None, 200)?;
        let open_runs = runs
            .iter()
            .filter(|run| {
                !matches!(
                    run.status,
                    ai_team_core::RunStatus::Done
                        | ai_team_core::RunStatus::Failed
                        | ai_team_core::RunStatus::Cancelled
                )
            })
            .count();
        // The largest event id across every run is one indexed lookup, and it moves
        // whenever any node in any run does anything at all.
        let latest_event = store.latest_event_id()?;
        let latest_notification = store.latest_notification_id()?;
        Ok(Tick {
            latest_event,
            latest_notification,
            open_runs,
        })
    }
}

#[cfg(test)]
mod activity_event_tests {
    use super::*;

    fn event(kind: ai_team_core::EventKind, actor: &str, payload: serde_json::Value) -> Event {
        Event {
            id: 1,
            run_id: 1,
            node_run_id: Some(2),
            at: "now".into(),
            kind,
            actor: Some(actor.into()),
            summary: "clipped…".into(),
            payload: Some(payload),
        }
    }

    #[test]
    fn a_settled_turn_is_not_presented_as_successful_work() {
        let mut settled = event(
            ai_team_core::EventKind::Done,
            "frontend",
            serde_json::json!({}),
        );
        settled.summary = "turn completed".into();
        let visible = ActivityEvent::from(settled);
        assert_eq!(visible.summary, "turn settled");
    }

    #[test]
    fn tool_activity_names_the_target_without_returning_the_raw_payload() {
        let visible = ActivityEvent::from(event(
            ai_team_core::EventKind::ToolCall,
            "backend",
            serde_json::json!({
                "toolName": "read",
                "args": { "path": "crates/core/src/lib.rs" },
                "result": { "content": "must stay in SQLite" }
            }),
        ));
        assert_eq!(visible.summary, "clipped… · crates/core/src/lib.rs");
        let json = serde_json::to_string(&visible).unwrap();
        assert!(!json.contains("must stay in SQLite"), "{json}");
    }

    #[test]
    fn a_full_answer_is_returned_without_provider_signatures() {
        let visible = ActivityEvent::from(event(
            ai_team_core::EventKind::Note,
            "orchestrator",
            serde_json::json!({
                "message": { "content": [
                    { "type": "thinking", "thinking": "checking", "thinkingSignature": "secret" },
                    { "type": "text", "text": "The whole answer." }
                ]}
            }),
        ));
        assert_eq!(visible.message.as_deref(), Some("The whole answer."));
        assert!(visible.thinking.is_empty());
        let json = serde_json::to_string(&visible).unwrap();
        assert!(!json.contains("thinkingSignature"), "{json}");
        assert!(!json.contains("secret"), "{json}");
    }

    #[test]
    fn a_provider_login_error_is_actionable_and_not_repeated() {
        let visible = ActivityEvent::from(event(
            ai_team_core::EventKind::Note,
            "frontend",
            serde_json::json!({
                "message": { "content": [
                    { "type": "text", "text": "Not logged in · Please run /login" },
                    { "type": "text", "text": "Not logged in · Please run /login" }
                ]}
            }),
        ));
        assert_eq!(
            visible.message.as_deref(),
            Some(
                "The selected provider is not signed in. Open Settings and sign in before retrying this slice."
            )
        );
    }

    #[test]
    fn a_completed_thinking_block_is_available_before_the_tool_runs() {
        let visible = ActivityEvent::from(event(
            ai_team_core::EventKind::Cost,
            "backend",
            serde_json::json!({
                "assistantMessageEvent": {
                    "type": "thinking_end",
                    "content": "Inspecting the workspace"
                }
            }),
        ));
        assert_eq!(visible.thinking, ["Inspecting the workspace"]);
    }

    #[test]
    fn provider_thinking_is_read_once_from_the_step_end() {
        let visible = ActivityEvent::from(event(
            ai_team_core::EventKind::Cost,
            "backend",
            serde_json::json!({
                "message": { "content": [
                    { "type": "thinking", "thinking": "Checking the failing test" }
                ]}
            }),
        ));
        assert_eq!(visible.thinking, ["Checking the failing test"]);
        assert!(visible.message.is_none());
    }

    #[test]
    fn a_human_reply_is_returned_verbatim() {
        let visible = ActivityEvent::from(event(
            ai_team_core::EventKind::Note,
            "human",
            serde_json::json!({ "body": "Yes.\nContinue." }),
        ));
        assert_eq!(visible.message.as_deref(), Some("Yes.\nContinue."));
    }
}
