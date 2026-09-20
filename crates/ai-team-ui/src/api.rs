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
        .route("/runs", get(runs))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/events", get(run_events))
        .route("/events", get(stream))
        .route("/runs", axum::routing::post(start))
        .route("/runs/{id}/approvals", get(approvals))
        .route("/runs/{id}/approvals", axum::routing::post(answer))
        .route("/board", get(board))
        .route("/board/slices/{key}", axum::routing::post(move_slice))
        .route("/today", get(today))
        .route("/tree", get(tree))
        .route("/file", get(read_file).post(write_file))
        .route("/search", get(search))
        .route("/projects/{id}/repos", axum::routing::post(attach_repo))
        .route("/settings", get(settings))
        .route("/settings/provider", axum::routing::post(set_provider))
        .route("/settings/context", axum::routing::post(set_context))
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

/// Which checkout the editor is looking at.
///
/// A worktree when a node has one leased, otherwise the project's own checkout. Editing
/// the lease is the point - it is where the agent's work actually is, and what Review is
/// showing a diff of.
#[derive(Debug, Deserialize)]
struct TreeQuery {
    project: String,
    /// A node's leased worktree, when the editor is following one.
    #[serde(default)]
    node: Option<i64>,
    #[serde(default)]
    path: String,
}

/// Resolve which directory on disk a request is about.
fn worktree_for(state: &AppState, project: &str, node: Option<i64>) -> Result<std::path::PathBuf> {
    let store = state.store()?;
    let store = store.lock();

    // A node's lease wins when one is named: that is where the work being reviewed lives,
    // and the main checkout does not have it.
    if let Some(node) = node {
        if let Some(path) = store.node_run(node)?.worktree_path {
            return Ok(std::path::PathBuf::from(path));
        }
    }

    let project = store.find_project(project)?;
    store
        .project_repos(project.id)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid(
                "that project has no checkout to open",
            ))
        })
}

async fn tree(
    State(state): State<AppState>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<Vec<ai_team_core::Entry>>> {
    let worktree = worktree_for(&state, &query.project, query.node)?;
    Ok(Json(ai_team_core::list_tree(&worktree, &query.path)?))
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
    let worktree = worktree_for(&state, &query.project, query.node)?;
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
    node: Option<i64>,
    path: String,
    text: String,
}

async fn write_file(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<SaveRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(&state, &request.project, request.node)?;
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
struct RegisterRequest {
    /// A directory on this machine. Absolute, because a relative path would be relative to
    /// wherever the server happens to have been started - which is not where the person
    /// clicking is looking.
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
    /// Enabled without a token is worth saying: the connection is generated and its seats
    /// fail at their first call.
    token_set: bool,
    token_env: String,
}

fn how_authenticated(provider: ai_team_core::Provider) -> &'static str {
    match provider {
        ai_team_core::Provider::Claude => {
            "your Claude subscription, through the Claude Code CLI - run `claude` once to sign in"
        }
        ai_team_core::Provider::OpenAi => {
            "your ChatGPT subscription, through eve - run `npx eve login` once"
        }
        ai_team_core::Provider::ZAi => "a GLM Coding Plan - flat rate, no metering",
        ai_team_core::Provider::Local => {
            "the ailocal gateway on 127.0.0.1:8081 - free, and never leaves the machine"
        }
    }
}

async fn settings() -> Result<Json<Settings>> {
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
            let env = format!("AI_TEAM_{}_TOKEN", source.as_str().to_uppercase());
            ContextSetting {
                source: source.as_str().to_string(),
                allowed: registry
                    .as_ref()
                    .is_ok_and(|registry| registry.context_sources().contains(source)),
                token_set: std::env::var(&env).is_ok_and(|value| !value.trim().is_empty()),
                token_env: env,
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
    Json(ai_team_core::readiness_report(known.as_ref()).await)
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
    let _ = &state;
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

    ai_team_core::apply_update(&latest, &binary, |_| {}).await?;
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
    let worktree = worktree_for(&state, &query.project, query.node)?;
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
    let worktree = worktree_for(&state, &request.project, request.node)?;

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
    node: Option<i64>,
    message: String,
}

async fn scm_commit(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<CommitRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(&state, &request.project, request.node)?;
    let sha = ai_team_core::commit_staged(&worktree, &request.message).await?;
    Ok(Json(serde_json::json!({ "sha": sha })))
}

#[derive(Debug, Deserialize)]
struct BranchRequest {
    project: String,
    #[serde(default)]
    node: Option<i64>,
    branch: String,
}

async fn scm_branch(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<BranchRequest>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(&state, &request.project, request.node)?;
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
    let worktree = worktree_for(&state, &query.project, query.node)?;
    let output = ai_team_core::push(&worktree).await?;
    Ok(Json(serde_json::json!({ "pushed": output.trim() })))
}

#[derive(Debug, Deserialize)]
struct TerminalQuery {
    project: String,
    #[serde(default)]
    node: Option<i64>,
}

async fn terminals(
    State(state): State<AppState>,
    Query(query): Query<TerminalQuery>,
) -> Result<Json<Vec<ai_team_core::Listed>>> {
    let worktree = worktree_for(&state, &query.project, query.node)?;
    Ok(Json(state.terminals().list(Some(&worktree))))
}

async fn open_terminal(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<TerminalQuery>,
) -> Result<Json<serde_json::Value>> {
    let worktree = worktree_for(&state, &request.project, request.node)?;
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
    let worktree = worktree_for(state, &request.project, request.node)?;
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
    let worktree = worktree_for(&state, &query.project, query.node)?;
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
    let store = state.store()?;
    let store = store.lock();
    let project = match &query.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    Ok(Json(
        ai_team_core::rollup(&store, query.by, project)?
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
    open_only: bool,
}

async fn reviews(
    State(state): State<AppState>,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<Vec<ai_team_core::Review>>> {
    let store = state.store()?;
    let store = store.lock();
    let project = match &query.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    Ok(Json(store.reviews(project, query.open_only)?))
}

/// A review, its diff, and everything anybody has said about it.
#[derive(Debug, Serialize)]
struct ReviewDetail {
    #[serde(flatten)]
    review: ai_team_core::Review,
    files: Vec<ai_team_core::FileDiff>,
    comments: Vec<ai_team_core::ReviewComment>,
    /// Whether the node that wrote this is still up. The surface says which of the two
    /// things submitting will do, rather than letting it be a surprise.
    steerable: bool,
}

async fn review(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<ReviewDetail>> {
    let (review, comments, repo) = {
        let store = state.store()?;
        let store = store.lock();
        let review = store.review(id)?;
        let comments = store.comments(id)?;
        let repo = store
            .project_repos(review.project_id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| {
                crate::error::Error::Core(ai_team_core::Error::invalid(
                    "that project has no checkout, so there is no diff to show",
                ))
            })?;
        (review, comments, repo)
    };

    // Read the node in a synchronous window, then let the lock go: what follows talks to
    // git and to the agent, and a future holding a `Store` is neither `Send` nor
    // spawnable.
    let node = {
        let store = state.store()?;
        let store = store.lock();
        ai_team_core::responsible(&store, &review)
    };

    let files = ai_team_core::diff_for(&review, std::path::Path::new(&repo)).await?;
    let steerable = ai_team_core::steerable(node).await;

    Ok(Json(ReviewDetail {
        review,
        files,
        comments,
        steerable,
    }))
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
}

/// Submit a review, which either steers the node that wrote the code or puts the work on
/// the plan for whoever picks it up next.
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(request): JsonBody<SubmitRequest>,
) -> Result<Json<ai_team_core::Submitted>> {
    // Everything the database knows, read before any await and released after.
    let (pending, node, repo) = {
        let store = state.store()?;
        let store = store.lock();
        let pending = ai_team_core::pending_review(&store, id)?;
        let node = ai_team_core::responsible(&store, &pending.review);
        let repo = store
            .project_repos(pending.review.project_id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| {
                crate::error::Error::Core(ai_team_core::Error::invalid(
                    "that project has no checkout, so feedback has nowhere to go",
                ))
            })?;
        (pending, node, repo)
    };

    // Both paths write to ai-planner through its own CLI (D4).
    let for_plan = repo.clone();
    let outcome = ai_team_core::deliver_review(
        &pending,
        node,
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
    // Two passes over the store, with the lock released in between, because reading the
    // questions means running `aip` once per checkout and no request should hold a
    // database connection across that.
    let (mut items, checkouts) = {
        let store = state.store()?;
        let store = store.lock();
        let items = ai_team_core::today_from_store(&store)?;
        let checkouts: Vec<(String, String)> = store
            .projects()?
            .into_iter()
            .filter_map(|project| {
                let repo = store
                    .project_repos(project.id)
                    .ok()?
                    .into_iter()
                    .find_map(|repo| repo.main_path)?;
                Some((project.slug, repo))
            })
            .collect();
        (items, checkouts)
    };

    for (slug, repo) in checkouts {
        // Best effort per project: a checkout that has been deleted, or one with no plan
        // yet, must not empty the whole list for every other project.
        let planner = ai_team_core::Planner::at(repo);
        let Ok(questions) = planner.open_questions().await else {
            continue;
        };
        for question in questions {
            items.push(ai_team_core::from_question(
                &slug,
                &question.body,
                question.asked_at,
            ));
        }
        // Work an agent finished and left `in_review` is the commonest thing waiting
        // after a run, and it lives on the plan rather than in ai-team's own tables.
        for slice in planner.slices().await.unwrap_or_default() {
            if let Some(item) =
                ai_team_core::from_slice(&slug, &slice.key, &slice.title, &slice.status)
            {
                items.push(item);
            }
        }
    }

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
    /// is the same rule dispatch follows.
    owner: Option<String>,
    /// The paths it declared, so a card can say why it routed where it did.
    touches: Vec<String>,
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
}

/// Resolve the project's checkout, which is where its plan lives.
fn planner_for(state: &AppState, project: &str) -> Result<(ai_team_core::Planner, i64)> {
    let store = state.store()?;
    let store = store.lock();
    let project = store.find_project(project)?;
    let team_id = project.team_id.ok_or_else(|| {
        crate::error::Error::Core(ai_team_core::Error::invalid("that project has no team"))
    })?;
    let repo = store
        .project_repos(project.id)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .ok_or_else(|| {
            crate::error::Error::Core(ai_team_core::Error::invalid(
                "that project has no checkout, so it has no plan to show",
            ))
        })?;
    Ok((ai_team_core::Planner::at(repo), team_id))
}

async fn board(
    State(state): State<AppState>,
    Query(query): Query<BoardQuery>,
) -> Result<Json<Board>> {
    // The planner is resolved, and the store lock released, before any of the awaits
    // below: `aip` is another program, and holding a database connection across it
    // would block every other request for the duration.
    let (planner, team_id) = planner_for(&state, &query.project)?;

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

    let store = state.store()?;
    let store = store.lock();
    let slices = slices
        .into_iter()
        .map(|slice| {
            let touches = slice.touches();
            let owner = touches
                .iter()
                .find_map(|path| store.agent_for_path(team_id, path).transpose())
                .transpose()?
                .map(|agent| agent.role);
            Ok(BoardSlice {
                slice,
                owner,
                touches,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(Board {
        plan: Some(plan),
        slices,
        next_step: None,
    }))
}

#[derive(Debug, Deserialize)]
struct MoveRequest {
    project: String,
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
    let (planner, _) = planner_for(&state, &request.project)?;
    planner
        .set_status(&key, &request.status, request.reason.as_deref())
        .await?;
    Ok(Json(serde_json::json!({ "moved": key })))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    project: String,
    prompt: Option<String>,
    plan: Option<String>,
    #[serde(default)]
    replan: bool,
    #[serde(default)]
    plan_only: bool,
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
    // Checked here so an obvious mistake answers immediately rather than failing inside
    // a task nobody is waiting on.
    {
        let store = state.store()?;
        let store = store.lock();
        store.find_project(&request.project)?;
    }

    let spec = ai_team_core::Request {
        project: request.project,
        prompt: request.prompt,
        plan: request.plan,
        width: None,
        replan: request.replan,
        plan_only: request.plan_only,
    };
    tokio::spawn(async move {
        // Its own store: `run_workflow` opens one, and the handler's is held by a mutex
        // the request will drop long before this finishes.
        if let Err(error) = ai_team_core::run_workflow(&spec, |_| {}).await {
            // Nothing to return to - the window reads the run's own rows, and a
            // workflow that failed before creating one has nowhere to write. stderr is
            // where `ait ui` is already being watched from.
            eprintln!("ai-team: run failed: {error}");
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

/// What a run is parked on, if anything.
async fn approvals(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Vec<Event>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.pending_approvals(id)?))
}

#[derive(Debug, Deserialize)]
struct AnswerRequest {
    /// Which node asked. The question belongs to a turn, not to the run as a whole.
    node: i64,
    request: String,
    /// The option the human picked, by the id eve offered it under.
    chose: String,
}

/// Answer a question an agent asked.
///
/// The node records where its eve is listening and the secret it checks, so this reaches
/// the agent over loopback whichever process started it - the window can answer what the
/// terminal's run is parked on.
async fn answer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(request): JsonBody<AnswerRequest>,
) -> Result<Json<serde_json::Value>> {
    let (port, token, session) = {
        let store = state.store()?;
        let store = store.lock();
        let node = store.node_run(request.node)?;
        if node.run_id != id {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that node belongs to a different run",
            )));
        }
        (
            node.eve_port,
            node.eve_token.clone(),
            node.session_id.clone(),
        )
    };

    let (Some(port), Some(token), Some(session)) = (port, token, session) else {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that node has no agent listening - it has already finished or its process \
             has gone",
        )));
    };

    let client = ai_team_core::EveClient::new(u16::try_from(port).unwrap_or(0), &token);
    client
        .respond(&session, &request.request, &request.chose)
        .await?;
    Ok(Json(serde_json::json!({ "answered": true })))
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
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

async fn runs(
    State(state): State<AppState>,
    Query(query): Query<RunQuery>,
) -> Result<Json<Vec<Run>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.runs(query.project, query.limit)?))
}

/// One run, with the nodes and the spend that make it readable at a glance.
#[derive(Debug, Serialize)]
struct RunDetail {
    #[serde(flatten)]
    run: Run,
    nodes: Vec<NodeRun>,
    usage: Usage,
}

async fn run(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<RunDetail>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(RunDetail {
        run: store.run(id)?,
        nodes: store.node_runs(id)?,
        usage: store.run_usage(id)?,
    }))
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    /// Events after this id. The window keeps the last one it saw and asks for the rest,
    /// so a reconnect never replays what is already on screen.
    after: Option<i64>,
    #[serde(default = "default_event_limit")]
    limit: i64,
}

fn default_event_limit() -> i64 {
    500
}

async fn run_events(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<Event>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.events(id, query.after, query.limit)?))
}

/// What changed, as far as a window needs to care.
///
/// Deliberately thin: a tick says *something* happened and hands over the cursor, and the
/// window re-reads whichever view it is showing. Streaming whole rows here would mean
/// this endpoint knowing what every surface renders.
#[derive(Debug, Serialize)]
struct Tick {
    latest_event: i64,
    open_runs: usize,
}

async fn stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(POLL))
        .map(move |_| {
            let tick = state.tick().unwrap_or(Tick {
                latest_event: -1,
                open_runs: 0,
            });
            Ok(SseEvent::default().json_data(tick).unwrap_or_default())
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
        Ok(Tick {
            latest_event,
            open_runs,
        })
    }
}
