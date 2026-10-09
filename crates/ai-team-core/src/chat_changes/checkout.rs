//! Explicit operator review/Git commands over the persistent checkout. Not verified drafts.
mod untracked;
use super::{
    delivery::{origin, remote_head, string},
    git,
};
use crate::{
    chat::team::{children, ownership::Ownership},
    DeliveryPolicy, Error, FileDiff, Result, Store,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path},
    sync::Arc,
};
pub use untracked::Preview as UntrackedPreview;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Stage {
        path: String,
    },
    Unstage {
        path: String,
    },
    Commit {
        message: String,
    },
    Push,
    PullRequest,
    /// The person's own explicit instruction in chat, publishing the branch being worked
    /// on rather than this panel's synthetic checkout-SHA ref. Older `Push` receipts keep
    /// the destination they were approved against; nothing retargets them.
    PushBranch {
        branch: String,
        commit: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub workspace: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub fingerprint: String,
    pub action: Action,
    pub remote: Option<Remote>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Remote {
    pub url: String,
    pub branch: String,
    pub repository: Option<String>,
    pub base: Option<String>,
    pub base_sha: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Operation {
    pub id: i64,
    pub chat_id: i64,
    pub snapshot: Snapshot,
    pub state: String,
    pub result: Option<String>,
    pub rev: i64,
}
#[derive(Debug, Serialize)]
pub struct State {
    pub workspace: String,
    pub workspace_epoch: i64,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub fingerprint: String,
    pub staged: Vec<FileDiff>,
    pub unstaged: Vec<FileDiff>,
    pub untracked: Vec<String>,
    pub untracked_files: Vec<UntrackedPreview>,
    pub findings: Vec<Finding>,
    pub operations: Vec<Operation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub id: i64,
    pub fingerprint: String,
    pub head: Option<String>,
    pub area: String,
    pub path: String,
    pub side: String,
    pub line: i64,
    pub body: String,
    #[serde(default)]
    pub created_at: String,
}
#[derive(Debug, Serialize)]
pub struct Inspection {
    pub operation: Operation,
    pub checkout: State,
}

pub async fn state(store: &mut Store, chat: i64) -> Result<State> {
    let current = store.chat(chat)?;
    let workspace = current.workspace_path;
    let repo = Path::new(&workspace);
    let head = checkout_head(repo).await?;
    let branch = crate::current_branch(repo).await;
    let staged = git(
        repo,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--binary",
            "--full-index",
            "--",
        ],
    )
    .await?;
    let unstaged = git(
        repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--binary",
            "--full-index",
            "--",
        ],
    )
    .await?;
    let index = git(repo, &["ls-files", "--stage", "-z"]).await?;
    let status = git(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    let unknown = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    let untracked: Vec<String> = unknown
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    let mut hash = Sha256::new();
    for value in [
        workspace.as_str(),
        head.as_deref().unwrap_or(""),
        branch.as_deref().unwrap_or(""),
        &staged,
        &unstaged,
        &index,
        &status,
    ] {
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    let untracked_files = untracked::inspect(repo, &untracked, &mut hash)?;
    Ok(State {
        workspace,
        workspace_epoch: current.workspace_epoch,
        head,
        branch,
        fingerprint: format!("{:x}", hash.finalize()),
        staged: crate::parse_diff(&staged),
        unstaged: crate::parse_diff(&unstaged),
        untracked,
        untracked_files,
        findings: store.checkout_findings(chat)?,
        operations: store.checkout_operations(chat)?,
    })
}

async fn checkout_head(repo: &Path) -> Result<Option<String>> {
    match git(repo, &["rev-parse", "--verify", "HEAD"]).await {
        Ok(head) => Ok(Some(head.trim().into())),
        Err(error) => {
            // A missing symbolic branch is unborn, even if other branches exist.
            let reference = git(repo, &["symbolic-ref", "HEAD"]).await?;
            let found = git(
                repo,
                &["for-each-ref", "--format=%(refname)", reference.trim()],
            )
            .await?;
            if !reference.starts_with("refs/heads/") || !found.is_empty() {
                return Err(error);
            }
            Ok(None)
        }
    }
}

fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.split('/').any(|c| c.eq_ignore_ascii_case(".git"))
    {
        return Err(Error::invalid(
            "choose an exact repository-relative file path",
        ));
    }
    Ok(())
}
fn policy(store: &Store, chat: i64, action: &Action) -> Result<()> {
    let c = store.chat(chat)?;
    if let Some(team) = store.project(c.project_id)?.team_id {
        let d = store.team(team)?.delivery;
        if matches!(action, Action::Push | Action::PushBranch { .. })
            && d.push == DeliveryPolicy::Manual
            || matches!(action, Action::PullRequest) && d.pr == DeliveryPolicy::Manual
        {
            return Err(Error::invalid(
                "delivery policy is manual; use your own Git/GitHub tools",
            ));
        }
    }
    Ok(())
}
pub async fn preview(
    store: &mut Store,
    chat: i64,
    fingerprint: &str,
    action: Action,
) -> Result<Operation> {
    if matches!(action, Action::PushBranch { .. }) {
        return Err(Error::invalid(
            "a working-branch push is authorised by your own message in this chat, not by this panel",
        ));
    }
    store.checkout_available(chat)?;
    policy(store, chat, &action)?;
    let s = state(store, chat).await?;
    if s.fingerprint != fingerprint {
        return Err(Error::invalid("checkout changed; refresh and review again"));
    }
    match &action {
        Action::Stage { path } => {
            validate_path(path)?;
            if !s.untracked.contains(path) && !s.unstaged.iter().any(|f| &f.path == path) {
                return Err(Error::invalid("this path has no unstaged changes"));
            }
        }
        Action::Unstage { path } => {
            validate_path(path)?;
            if !s.staged.iter().any(|f| &f.path == path) {
                return Err(Error::invalid("this path has no staged changes"));
            }
        }
        Action::Commit { message } => {
            if message.trim().is_empty() || message.len() > 16000 || s.staged.is_empty() {
                return Err(Error::invalid(
                    "a manual commit needs staged changes and a message of 1–16000 bytes",
                ));
            }
        }
        _ => {
            if s.head.is_none() || s.branch.is_none() {
                return Err(Error::invalid(
                    "publication needs a committed checkout branch",
                ));
            }
        }
    }
    let remote = publication(Path::new(&s.workspace), chat, s.head.as_deref(), &action).await?;
    store.record_checkout_preview(
        chat,
        &Snapshot {
            workspace: s.workspace,
            head: s.head,
            branch: s.branch,
            fingerprint: s.fingerprint,
            action,
            remote,
        },
    )
}
/// Build the operation one explicit human push instruction authorised (`chat_push`).
///
/// Everything the grant claimed is resolved again here, against the real checkout, and
/// `checkout_available` refuses while anything is still working in it - which is why the
/// solo path waits for its turn's process group to drain before calling this.
pub(crate) async fn authorized_push(
    store: &mut Store,
    grant: &crate::chat_push::PushGrant,
) -> Result<Operation> {
    let chat = grant.chat_id;
    let branch = grant.publication_branch();
    let commit = grant
        .commit_sha
        .as_deref()
        .ok_or_else(|| Error::invalid("the push has no pinned commit"))?;
    let origin_url = &grant.origin_url;
    let action = Action::PushBranch {
        branch: branch.to_string(),
        commit: commit.to_string(),
    };
    store.checkout_available(chat)?;
    policy(store, chat, &action)?;
    let s = state(store, chat).await?;
    if s.workspace != grant.workspace_path
        || s.workspace_epoch != grant.workspace_epoch
        || (grant.mode == crate::ChatMode::Single
            && (s.branch.as_deref() != Some(branch) || s.head.as_deref() != Some(commit)))
    {
        return Err(Error::invalid(
            "the approved working checkout or branch changed; ask again",
        ));
    }
    let remote = publication(Path::new(&s.workspace), chat, s.head.as_deref(), &action).await?;
    if remote.as_ref().is_none_or(|r| &r.url != origin_url) {
        return Err(Error::invalid(
            "origin changed since you asked for this push; ask again",
        ));
    }
    store.record_checkout_preview(
        chat,
        &Snapshot {
            workspace: s.workspace,
            head: s.head,
            branch: s.branch,
            fingerprint: s.fingerprint,
            action,
            remote,
        },
    )
}

/// Whether this chat's team allows ai-team to push at all. Manual is a veto, and the
/// person hears it when they ask rather than after a turn has been spent on it.
pub(crate) fn push_allowed(store: &Store, chat: i64) -> Result<()> {
    policy(store, chat, &Action::Push)
}

/// Branch names an explicit chat push may never resolve to.
const PROTECTED: &[&str] = &[
    "main",
    "master",
    "trunk",
    "develop",
    "development",
    "production",
    "prod",
    "release",
    "stable",
];

/// Refuse a default or protected branch rather than deciding what the person meant.
///
/// A remote whose default branch cannot be read is refused too: "probably not the default"
/// is not a thing to publish on.
pub(crate) async fn publishable_branch(repo: &Path, url: &str, branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.contains("..")
        || branch.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::invalid(
            "this checkout's branch name cannot be published safely; use your own Git tools",
        ));
    }
    if PROTECTED
        .iter()
        .any(|name| branch.eq_ignore_ascii_case(name))
    {
        return Err(Error::invalid(format!(
            "'{branch}' is a protected branch name; an explicit chat push publishes a working branch only"
        )));
    }
    let text = git(repo, &["ls-remote", "--symref", url, "HEAD"]).await?;
    let default = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("ref: refs/heads/"))
        .and_then(|rest| rest.split('\t').next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            Error::invalid(
                "the remote does not report a default branch, so this push cannot be shown to be safe; publish it with your own Git tools",
            )
        })?;
    if branch == default {
        return Err(Error::invalid(format!(
            "'{branch}' is this repository's default branch; an explicit chat push publishes a working branch only"
        )));
    }
    Ok(())
}

async fn publication(
    repo: &Path,
    chat: i64,
    head: Option<&str>,
    action: &Action,
) -> Result<Option<Remote>> {
    if let Action::PushBranch { branch, commit } = action {
        let url = origin(repo).await?;
        publishable_branch(repo, &url, branch).await?;
        // The commit may have been made in a lease of this repository. It still has to be
        // an object this checkout can see, and still be on the branch it was approved
        // for: a branch reset after the approval is not the work that was approved.
        git(repo, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).await?;
        let tip = git(
            repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )
        .await?;
        if tip.trim() != commit {
            return Err(Error::invalid(
                "the approved commit is no longer the exact branch tip; ask again",
            ));
        }
        if let Some(found) = remote_head(repo, &url, branch).await? {
            if &found != commit {
                git(repo, &["merge-base", "--is-ancestor", &found, commit])
                    .await
                    .map_err(|_| {
                        Error::invalid(
                            "the remote branch holds work this commit does not contain; it will not be overwritten",
                        )
                    })?;
            }
        }
        return Ok(Some(Remote {
            url,
            branch: branch.clone(),
            repository: None,
            base: None,
            base_sha: None,
        }));
    }
    if !matches!(action, Action::Push | Action::PullRequest) {
        return Ok(None);
    }
    let head = head.ok_or_else(|| Error::invalid("no commit to publish"))?;
    let url = origin(repo).await?;
    let branch = format!("ai-team/chat-{chat}/checkout-{head}");
    let found = remote_head(repo, &url, &branch).await?;
    if found.as_deref().is_some_and(|sha| sha != head) {
        return Err(Error::invalid(
            "remote destination contains other work; it will not be overwritten",
        ));
    }
    let mut r = Remote {
        url,
        branch,
        repository: None,
        base: None,
        base_sha: None,
    };
    if matches!(action, Action::PullRequest) {
        if found.as_deref() != Some(head) {
            return Err(Error::invalid(
                "push this exact commit before previewing a draft PR",
            ));
        }
        let value: serde_json::Value = serde_json::from_str(
            &crate::neighbours::github::gh(
                repo,
                &["repo", "view", &r.url, "--json", "url,defaultBranchRef"],
            )
            .await?,
        )?;
        let base = value
            .pointer("/defaultBranchRef/name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::invalid("GitHub did not report a default branch"))?;
        r.repository = Some(string(&value, "url")?);
        r.base = Some(base.into());
        r.base_sha = Some(
            remote_head(repo, &r.url, base)
                .await?
                .ok_or_else(|| Error::invalid("PR base is missing"))?,
        );
    }
    Ok(Some(r))
}
pub async fn approve(store: &mut Store, chat: i64, id: i64, rev: i64) -> Result<Operation> {
    let op = store.checkout_operation(chat, id)?;
    if op.state == "done" {
        return Ok(op);
    }
    let creation_only = matches!(op.snapshot.action, Action::PushBranch { .. })
        && store.chat_push_creates_branch(chat, id)?;
    let owner = store.claim_checkout_operation(chat, id, rev, false)?;
    owner.track(async {
        let validation=async {
            policy(store,chat,&op.snapshot.action)?;
            let current=state(store,chat).await?;
            if current.fingerprint!=op.snapshot.fingerprint {return Err(Error::invalid("checkout changed after preview; no action approved for these new bytes"));}
            if publication(Path::new(&current.workspace),chat,current.head.as_deref(),&op.snapshot.action).await? != op.snapshot.remote {return Err(Error::invalid("publication destination or PR base changed; preview again"));} Ok(())
        }.await;
        if let Err(e)=validation {return store.settle_checkout_operation(chat,id,if owner.quiescent(){"refused"}else{"inspection"},&e.to_string());}
        store.attempt_checkout_operation(chat,id)?;
        let result=execute(&op.snapshot, creation_only).await;
        match result {
            Ok(result) if owner.quiescent()=>store.settle_checkout_operation(chat,id,"done",&result),
            Ok(result)=>store.settle_checkout_operation(chat,id,"inspection",&format!("{result}; command groups may still be alive. Drain and inspect; no retry.")),
            Err(e)=>store.settle_checkout_operation(chat,id,"inspection",&format!("{e}. The action may have happened. Drain and inspect; no automatic retry, reset or cleanup.")),
        }
    }).await
}
async fn execute(s: &Snapshot, creation_only: bool) -> Result<String> {
    let repo = Path::new(&s.workspace);
    match &s.action {
        Action::Stage { path } => {
            git(repo, &["--literal-pathspecs", "add", "--", path]).await?;
            Ok(format!("Staged {path}"))
        }
        Action::Unstage { path } => {
            if s.head.is_some() {
                git(
                    repo,
                    &["--literal-pathspecs", "restore", "--staged", "--", path],
                )
                .await?;
            } else {
                git(repo, &["--literal-pathspecs", "rm", "--cached", "--", path]).await?;
            }
            Ok(format!("Unstaged {path}; working file kept"))
        }
        Action::Commit { message } => {
            git(
                repo,
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgSign=false",
                    "commit",
                    "--no-verify",
                    "--no-gpg-sign",
                    "-m",
                    message,
                ],
            )
            .await?;
            Ok(format!(
                "Manual commit {} (not a verification verdict)",
                git(repo, &["rev-parse", "HEAD"]).await?.trim()
            ))
        }
        Action::Push | Action::PullRequest | Action::PushBranch { .. } => {
            publish(s, creation_only).await
        }
    }
}
async fn publish(s: &Snapshot, creation_only: bool) -> Result<String> {
    let repo = Path::new(&s.workspace);
    let r = s
        .remote
        .as_ref()
        .ok_or_else(|| Error::invalid("missing approved destination"))?;
    let head = match &s.action {
        // The pinned commit, which for a team draft is not this checkout's HEAD.
        Action::PushBranch { commit, .. } => commit.as_str(),
        _ => s
            .head
            .as_deref()
            .ok_or_else(|| Error::invalid("missing approved commit"))?,
    };
    if let Action::PushBranch { branch, .. } = &s.action {
        let remote = remote_head(repo, &r.url, branch).await?;
        if creation_only && remote.is_some() {
            return Err(Error::invalid(
                "the requested new branch now exists on origin; it will not be overwritten",
            ));
        }
        if remote.as_deref() != Some(head) {
            let destination = format!("{head}:refs/heads/{branch}");
            let absent = format!("--force-with-lease=refs/heads/{branch}:");
            let mut args = vec![
                "-c",
                "core.hooksPath=/dev/null",
                "push",
                "--no-verify",
                "--no-follow-tags",
                "--recurse-submodules=no",
            ];
            // Existing branches remain fast-forward only. Explicit creation requires
            // absence atomically at the remote; the empty lease cannot overwrite a ref.
            if creation_only {
                args.push(&absent);
            }
            args.extend([r.url.as_str(), destination.as_str()]);
            git(repo, &args).await?;
        }
        if remote_head(repo, &r.url, branch).await?.as_deref() != Some(head) {
            return Err(Error::invalid("publication could not be confirmed"));
        }
        return Ok(format!(
            "Pushed {head} to {} · {branch}, as you asked in this chat. No pull request was opened and nothing was merged.",
            r.url
        ));
    }
    if matches!(s.action, Action::Push) {
        if remote_head(repo, &r.url, &r.branch).await?.as_deref() != Some(head) {
            git(
                repo,
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "push",
                    "--no-verify",
                    "--no-follow-tags",
                    "--recurse-submodules=no",
                    &format!("--force-with-lease=refs/heads/{}:", r.branch),
                    &r.url,
                    &format!("{head}:refs/heads/{}", r.branch),
                ],
            )
            .await?;
        }
        if remote_head(repo, &r.url, &r.branch).await?.as_deref() != Some(head) {
            return Err(Error::invalid("publication could not be confirmed"));
        }
        return Ok(format!(
            "Published {head} to {} · {}. No PR was opened.",
            r.url, r.branch
        ));
    }
    let repository = r
        .repository
        .as_deref()
        .ok_or_else(|| Error::invalid("missing GitHub repository"))?;
    let base = r
        .base
        .as_deref()
        .ok_or_else(|| Error::invalid("missing PR base"))?;
    if let Some(url) = existing_pr(repo, r, head).await? {
        return Ok(url);
    }
    crate::neighbours::github::gh(repo,&["pr","create","--draft","--repo",repository,"--head",&r.branch,"--base",base,"--title",&format!("Checkout changes {}",&head[..12]),"--body",&format!("Human-approved checkout commit `{head}`. This is NOT a verified team draft. Review and merge remain separate decisions.")]).await?;
    existing_pr(repo, r, head)
        .await?
        .ok_or_else(|| Error::invalid("GitHub has not confirmed the exact draft PR"))
}
async fn existing_pr(repo: &Path, r: &Remote, head: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(
        &crate::neighbours::github::gh(
            repo,
            &[
                "pr",
                "list",
                "--repo",
                r.repository
                    .as_deref()
                    .ok_or_else(|| Error::invalid("missing repository"))?,
                "--head",
                &r.branch,
                "--state",
                "all",
                "--json",
                "url,state,baseRefName,headRefOid,isDraft",
            ],
        )
        .await?,
    )?;
    let rows = value
        .as_array()
        .ok_or_else(|| Error::invalid("invalid GitHub PR list"))?;
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1
        || rows[0]["state"] != "OPEN"
        || rows[0]["isDraft"] != true
        || rows[0]["headRefOid"] != head
        || rows[0]["baseRefName"].as_str() != r.base.as_deref()
    {
        return Err(Error::invalid(
            "destination has another or closed/non-draft PR; it will not be adopted",
        ));
    }
    Ok(Some(string(&rows[0], "url")?))
}
pub async fn finding(store: &mut Store, chat: i64, input: &Finding) -> Result<()> {
    validate_finding(store, chat, input).await?;
    store.record_checkout_finding(chat, input)
}

pub(crate) async fn validate_finding(store: &mut Store, chat: i64, input: &Finding) -> Result<()> {
    store.checkout_available(chat)?;
    validate_finding_snapshot(store, chat, input).await
}

pub(crate) async fn validate_finding_snapshot(
    store: &mut Store,
    chat: i64,
    input: &Finding,
) -> Result<()> {
    let s = state(store, chat).await?;
    if s.fingerprint != input.fingerprint || s.head != input.head {
        return Err(Error::invalid(
            "diff changed; review the new lines before commenting",
        ));
    }
    if input.body.trim().is_empty() || input.body.len() > 32000 {
        return Err(Error::invalid("a finding needs 1–32000 bytes"));
    }
    let files = match input.area.as_str() {
        "staged" => s.staged,
        "unstaged" => s.unstaged,
        "untracked" => s
            .untracked_files
            .into_iter()
            .map(|preview| preview.file)
            .collect(),
        _ => return Err(Error::invalid("choose staged, unstaged or untracked diff")),
    };
    let valid = files
        .iter()
        .filter(|f| f.path == input.path)
        .flat_map(|f| &f.hunks)
        .flat_map(|h| &h.lines)
        .any(|l| match input.side.as_str() {
            "old" => l.old == Some(input.line),
            "new" => l.new == Some(input.line),
            _ => false,
        });
    if !valid {
        return Err(Error::invalid("that diff line no longer exists"));
    }
    Ok(())
}
async fn drain(store: &mut Store, owner: &Arc<Ownership>) -> Result<()> {
    let id = owner
        .checkout
        .ok_or_else(|| Error::invalid("not a checkout owner"))?;
    for record in store
        .checkout_children(id)?
        .into_iter()
        .filter(|child| child.state != "drained")
    {
        children::drain(&record).await?;
        store.finish_chat_child(owner, record.id)?;
    }
    Ok(())
}
pub async fn inspect(store: &mut Store, chat: i64, id: i64, rev: i64) -> Result<Inspection> {
    let owner = store.claim_checkout_operation(chat, id, rev, true)?;
    owner.track(async {
        if let Err(e)=drain(store,&owner).await {store.settle_checkout_operation(chat,id,"inspection",&e.to_string())?;return Err(e);}
        let result=state(store,chat).await;
        let operation=store.settle_checkout_operation(chat,id,"inspection","Local command groups drained. Inspect current files/index/history and remote effects before acknowledging. No action was retried.")?;
        Ok(Inspection {operation,checkout:result?})
    }).await
}
pub async fn acknowledge(
    store: &mut Store,
    chat: i64,
    id: i64,
    rev: i64,
    fingerprint: &str,
    reason: &str,
) -> Result<Operation> {
    if reason.trim().is_empty() || reason.len() > 4000 {
        return Err(Error::invalid(
            "acknowledgement needs a reason of 1–4000 bytes",
        ));
    }
    let owner = store.claim_checkout_operation(chat, id, rev, true)?;
    owner.track(async {
        let check = async {
            drain(store, &owner).await?;
            let current = state(store, chat).await?;
            if current.fingerprint != fingerprint {
                return Err(Error::invalid("inspected checkout changed; inspect again"));
            }
            if !owner.quiescent() {
                return Err(Error::invalid("inspection children did not drain"));
            }
            Ok(())
        }.await;
        match check {Ok(())=>store.settle_checkout_operation(chat,id,"acknowledged",&format!("Kept inspected state: {reason}. No success certified or remote effects ruled out; nothing retried.")),Err(e)=>{store.settle_checkout_operation(chat,id,"inspection",&e.to_string())?;Err(e)}}
    }).await
}
