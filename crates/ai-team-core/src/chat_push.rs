//! A direct human message is the only thing that can authorise a push.
//!
//! Nothing here ever publishes on its own initiative. One authenticated human send, whose
//! text includes a direct push instruction (`grammar`), mints one grant: one chat,
//! one request id, one workspace generation, one branch, one origin, one supervising
//! process. A solo turn may then pin the commit it just made through the scoped
//! `request_publication` tool, and the host publishes it after the turn's process has
//! drained - the journalled checkout executor refuses to act while work is live, which is
//! exactly the property wanted. A team message targets one exact owned draft instead, and
//! starts no planning.
//!
//! Push authority is not pull-request, merge, tag or force authority, and it is never
//! authority over a default or protected branch. Everything a grant claims is rechecked
//! immediately before the push, and the push itself is fast-forward only.

mod branch;
mod grammar;
pub use branch::NewBranchScope;

use std::path::Path;

pub(crate) use grammar::request as request_scope;
pub use grammar::{classify, Intent};

use serde::Serialize;

use crate::chat_changes::{checkout, DeliveryAction, DraftTarget};
use crate::{ChatMode, Error, Result, Store};

/// The one scoped tool a solo turn gets, and only while its grant is live.
pub const PUBLICATION_TOOL: &str = "request_publication";

#[derive(Debug, Clone, Serialize)]
pub struct PushGrant {
    pub id: i64,
    pub chat_id: i64,
    pub request_id: String,
    pub node_id: Option<i64>,
    pub draft: Option<DraftTarget>,
    pub mode: ChatMode,
    pub workspace_path: String,
    pub workspace_epoch: i64,
    pub branch: String,
    pub new_branch: Option<NewBranchScope>,
    pub pinned_branch: Option<String>,
    pub origin_url: String,
    pub head_sha: String,
    pub allow_commit: bool,
    #[serde(skip)]
    pub(crate) supervisor_pid: i64,
    #[serde(skip)]
    pub(crate) supervisor_identity: String,
    pub commit_sha: Option<String>,
    pub operation_id: Option<i64>,
    pub state: String,
    pub result: Option<String>,
}

impl PushGrant {
    pub(crate) fn publication_branch(&self) -> &str {
        self.pinned_branch.as_deref().unwrap_or(&self.branch)
    }

    /// The process the person was talking to is still that process. A restart leaves the
    /// rows behind; it does not leave the permission behind, because the pid is either
    /// gone or belongs to something that started later.
    ///
    /// Liveness rather than "I am it": the scoped MCP server runs as a child of the turn
    /// and has to be able to see the grant it may pin. Only the supervisor executes.
    pub(crate) fn supervised(&self) -> bool {
        crate::chat::process_matches(Some(self.supervisor_pid), Some(&self.supervisor_identity))
    }

    pub(crate) fn ours(&self) -> bool {
        self.supervisor_pid == i64::from(std::process::id()) && self.supervised()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NewPushGrant {
    pub chat_id: i64,
    pub request_id: String,
    pub node_id: Option<i64>,
    pub draft: Option<DraftTarget>,
    pub mode: ChatMode,
    pub message: String,
    pub workspace_path: String,
    pub workspace_epoch: i64,
    pub branch: String,
    pub origin_url: String,
    pub head_sha: String,
    pub allow_commit: bool,
    pub supervisor_pid: i64,
    pub supervisor_identity: String,
    pub commit_sha: Option<String>,
    pub new_branch: Option<NewBranchScope>,
}

/// The exact destination an instruction resolved to, before anything is written down.
#[derive(Debug, Clone)]
pub struct Target {
    pub draft: Option<DraftTarget>,
    pub new_branch: Option<NewBranchScope>,
    pub workspace_path: String,
    pub workspace_epoch: i64,
    pub branch: String,
    pub origin_url: String,
    pub head_sha: String,
    pub allow_commit: bool,
}

/// What the send path should do with this message.
#[derive(Debug)]
pub enum Authority {
    /// Nothing to do. The chat behaves as it always has.
    None,
    /// Publishing was mentioned but not instructed, or this request was already answered.
    /// The turn starts with no authority and the chat is told so, in its own conversation.
    Ask(String),
    /// Start the turn, and let it pin one commit on this branch for the host to publish.
    Solo(Target),
    /// Publish this exact owned draft now. No planning turn is started for it.
    Team(Target, DraftTarget),
}

/// Resolve what one direct human message authorises, without writing anything.
///
/// Called only from the authenticated direct send path. A schedule's prompt, a queued
/// instruction, a recovered turn's prompt and an agent's own text never reach it: they are
/// `run.prompt` too, and `run.prompt` is not a person speaking.
pub async fn prepare(store: &mut Store, chat_id: i64, message: &str) -> Result<Authority> {
    let request = grammar::request(message);
    let commit =
        match request.intent {
            Intent::None => return Ok(Authority::None),
            Intent::Ambiguous => return Ok(Authority::Ask(
                "This message mentions publishing but does not read as an explicit instruction, \
                 so no push was authorised. Clarify the branch you want pushed, or use Changes."
                    .into(),
            )),
            Intent::Authorize { commit } => commit,
        };
    store.expire_stale_chat_push_grants(Some(chat_id))?;
    let chat = store.chat(chat_id)?;
    if chat.archived {
        return Err(Error::invalid(
            "restore this chat before publishing from it",
        ));
    }
    checkout::push_allowed(store, chat_id)?;
    let repo = Path::new(&chat.workspace_path);
    let branch = crate::current_branch(repo)
        .await
        .ok_or_else(|| Error::invalid("this checkout has no branch to publish"))?;
    let origin_url = crate::chat_changes::origin(repo).await?;
    let target = Target {
        draft: None,
        new_branch: None,
        workspace_path: chat.workspace_path.clone(),
        workspace_epoch: chat.workspace_epoch,
        branch,
        origin_url,
        head_sha: String::new(),
        allow_commit: commit,
    };
    match chat.mode {
        ChatMode::Single => {
            let (head, new_branch) = branch::prepare(repo, &target.origin_url, &request).await?;
            if new_branch.is_none() {
                if request
                    .branch
                    .as_deref()
                    .is_some_and(|name| name != target.branch)
                {
                    return Err(Error::invalid("the named branch is not this chat's working branch; select its checkout before asking to push"));
                }
                checkout::publishable_branch(repo, &target.origin_url, &target.branch).await?;
            }
            Ok(Authority::Solo(Target {
                head_sha: head,
                new_branch,
                ..target
            }))
        }
        ChatMode::Team => {
            if request.work {
                return Err(Error::invalid("this message requests new work as well as a push; approve and finish that team work first, then ask to push its exact draft. No existing draft was pushed and no planning was started"));
            }
            let draft = sole_draft(store, chat_id, request.branch.as_deref())?;
            let found = crate::chat_changes::draft(store, chat_id, &draft)?;
            crate::chat_changes::delivery_available(store, chat_id, &draft, DeliveryAction::Push)?;
            checkout::publishable_branch(repo, &target.origin_url, &found.branch).await?;
            Ok(Authority::Team(
                Target {
                    draft: Some(draft.clone()),
                    branch: found.branch,
                    head_sha: found.commit_sha,
                    ..target
                },
                draft,
            ))
        }
    }
}

/// The one verified draft this chat owns. Never the project's latest, never a choice
/// between several: a push message names work the person has in mind, and guessing which
/// one is how the wrong commit reaches a shared branch.
fn sole_draft(store: &Store, chat_id: i64, branch: Option<&str>) -> Result<DraftTarget> {
    let mut found = Vec::new();
    for run in store.chat_draft_runs(chat_id)? {
        for slice in store.chat_build_slices(run)? {
            if store.chat_draft_superseded(chat_id, run, &slice.slice_key)? {
                continue;
            }
            if slice.build_status == "verified"
                && slice.commit_sha.is_some()
                && branch.is_none_or(|name| slice.branch.as_deref() == Some(name))
            {
                found.push(DraftTarget {
                    run_id: run,
                    slice_key: slice.slice_key,
                    revision: slice.rev,
                });
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(Error::invalid(
            "this team chat has no verified draft to push; no planning was started for this message",
        )),
        n => Err(Error::invalid(format!(
            "this team chat has {n} verified drafts; approve the exact one in Changes rather than letting a message choose"
        ))),
    }
}

/// Write the authority down against the solo turn that may use it.
pub fn mint(
    store: &mut Store,
    chat_id: i64,
    request_id: &str,
    message: &str,
    node_id: Option<i64>,
    target: &Target,
    commit_sha: Option<&str>,
) -> Result<PushGrant> {
    let pid = i64::from(std::process::id());
    let supervisor_identity = crate::chat::process_identity(pid)
        .ok_or_else(|| Error::invalid("could not identify the chat supervisor process"))?;
    let mode = store.chat(chat_id)?.mode;
    store.mint_chat_push_grant(&NewPushGrant {
        chat_id,
        request_id: request_id.to_string(),
        node_id,
        draft: target.draft.clone(),
        mode,
        message: message.to_string(),
        workspace_path: target.workspace_path.clone(),
        workspace_epoch: target.workspace_epoch,
        branch: target.branch.clone(),
        origin_url: target.origin_url.clone(),
        head_sha: target.head_sha.clone(),
        allow_commit: target.allow_commit,
        supervisor_pid: pid,
        supervisor_identity,
        commit_sha: commit_sha.map(str::to_string),
        new_branch: target.new_branch.clone(),
    })
}

/// The scoped tool a solo agent calls after committing. It pins; it does not publish.
///
/// Deferring the push until the turn's process group has drained is not politeness: the
/// journalled executor refuses to act on a checkout something is still working in, which
/// is the property that keeps a half-written index out of a published commit.
pub async fn request_publication(
    db: &Path,
    chat_id: i64,
    node: i64,
    commit: &str,
) -> Result<String> {
    let mut store = Store::open(db)?;
    let grant = store
        .chat_push_grant_for_node(chat_id, node)?
        .ok_or_else(|| {
            Error::invalid(
                "this turn has no push authority: the person in this chat has not asked for a push",
            )
        })?;
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::invalid("supply the exact commit SHA"));
    }
    let commit = commit.to_ascii_lowercase();
    let repo = Path::new(&grant.workspace_path);
    let branch = crate::current_branch(repo)
        .await
        .ok_or_else(|| Error::invalid("this checkout has no branch to publish"))?;
    branch::validate(&grant, repo, &branch).await?;
    let head = crate::chat_changes::git(repo, &["rev-parse", "--verify", "HEAD"])
        .await?
        .trim()
        .to_ascii_lowercase();
    if head != commit {
        return Err(Error::invalid(
            "that commit is not this branch's tip; commit the work first, then pin its own SHA",
        ));
    }
    if !grant.allow_commit && commit != grant.head_sha.to_ascii_lowercase() {
        return Err(Error::invalid(
            "the person authorised publishing what was already committed, not a new commit",
        ));
    }
    crate::chat_changes::git(
        repo,
        &["merge-base", "--is-ancestor", &grant.head_sha, &commit],
    )
    .await
    .map_err(|_| {
        Error::invalid(
            "the authorised starting commit is not an ancestor; rewritten history was not approved",
        )
    })?;
    let grant = store.arm_chat_push_grant(chat_id, node, &branch, &commit)?;
    Ok(format!(
        "Publication requested for {} at {commit}. ai-team pushes it after this turn ends and \
         reports the real result in this conversation. Do not run git push, and do not tell the \
         person it is published: you do not know yet.",
        grant.publication_branch()
    ))
}

/// Run after a solo turn's worker has finished, before anything else is started for it.
pub async fn settle(db: &Path, chat_id: i64, node: i64, failed: bool) -> Result<()> {
    let mut store = Store::open(db)?;
    let Some(grant) = store.chat_push_grant_for_node(chat_id, node)? else {
        return Ok(());
    };
    // Only the process the person asked publishes. Another one's worker loop is not this
    // conversation's supervisor, whatever its rows can see.
    if !grant.ours() {
        return Ok(());
    }
    if failed
        || store.node_run(node)?.status != crate::NodeStatus::Done
        || grant.commit_sha.is_none()
    {
        let reason = if failed {
            "The turn did not finish, so the authorised push was not performed. Nothing was published."
        } else {
            "The turn ended without asking for publication, so nothing was pushed."
        };
        store.settle_chat_push_grant(grant.id, "expired", reason)?;
        return store.record_chat_push_note(chat_id, Some(node), "Push not performed", reason);
    }
    publish(&mut store, &grant).await
}

/// Publish one authority through the journalled checkout executor, exactly once.
pub(crate) async fn publish(store: &mut Store, grant: &PushGrant) -> Result<()> {
    let chat_id = grant.chat_id;
    // A grant that already recorded an attempt is never retried. Its receipt is the only
    // thing that knows whether the remote took it.
    if let Some(operation) = grant.operation_id {
        let found = store.checkout_operation(chat_id, operation)?;
        let note = format!(
            "A push was already attempted for this message ({}). Inspect receipt {operation} in Changes; it was not repeated.",
            found.state
        );
        store.settle_chat_push_grant(grant.id, "spent", &note)?;
        return store.record_chat_push_note(
            chat_id,
            grant.node_id,
            "Push already attempted",
            &note,
        );
    }
    let prepared = match checkout::authorized_push(store, grant).await {
        Ok(prepared) => prepared,
        Err(error) => {
            let note = format!("The authorised push was refused before it ran: {error}");
            store.settle_chat_push_grant(grant.id, "expired", &note)?;
            return store.record_chat_push_note(chat_id, grant.node_id, "Push refused", &note);
        }
    };
    store.bind_chat_push_operation(grant.id, prepared.id)?;
    let done = match checkout::approve(store, chat_id, prepared.id, prepared.rev).await {
        Ok(done) => done,
        Err(error) => {
            let operation = store.checkout_operation(chat_id, prepared.id)?;
            let note = format!("Push did not complete: {error}. Inspect checkout receipt {} ({}); no automatic retry.", operation.id, operation.state);
            if store.chat_push_grant(chat_id, grant.id)?.state == "armed" {
                store.settle_chat_push_grant(grant.id, "spent", &note)?;
            }
            return store.record_chat_push_note(
                chat_id,
                grant.node_id,
                "Push needs attention",
                &note,
            );
        }
    };
    let result = done
        .result
        .clone()
        .unwrap_or_else(|| "The push recorded no result.".into());
    let summary = match done.state.as_str() {
        "done" => "Pushed",
        "refused" => "Push refused",
        _ => "Push needs inspection",
    };
    store.settle_chat_push_grant(grant.id, "spent", &result)?;
    store.record_chat_push_note(chat_id, grant.node_id, summary, &result)
}

/// A team message publishes the exact draft it named, now, with no turn behind it.
pub async fn publish_team(
    store: &mut Store,
    chat_id: i64,
    request_id: &str,
    message: &str,
    target: &Target,
) -> Result<PushGrant> {
    if store
        .replay_chat_push(chat_id, request_id, message)?
        .is_some()
    {
        return store
            .chat_push_grant_for_request(chat_id, request_id)?
            .ok_or_else(|| Error::invalid("push receipt disappeared"));
    }
    let grant = mint(
        store,
        chat_id,
        request_id,
        message,
        None,
        target,
        Some(&target.head_sha),
    )?;
    store.record_chat_push_note(
        chat_id,
        None,
        "Push requested",
        &format!("You asked for this in chat: {message}"),
    )?;
    if grant.state == "armed" {
        publish(store, &grant).await?;
    }
    store.chat_push_grant(chat_id, grant.id)
}
