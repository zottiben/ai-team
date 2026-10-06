//! Explicit operator review/Git commands over the persistent checkout. Not verified drafts.
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Stage { path: String },
    Unstage { path: String },
    Commit { message: String },
    Push,
    PullRequest,
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
    pub head: Option<String>,
    pub branch: Option<String>,
    pub fingerprint: String,
    pub staged: Vec<FileDiff>,
    pub unstaged: Vec<FileDiff>,
    pub untracked: Vec<String>,
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
    let workspace = store.chat(chat)?.workspace_path;
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
    hash_untracked(repo, &untracked, &mut hash)?;
    Ok(State {
        workspace,
        head,
        branch,
        fingerprint: format!("{:x}", hash.finalize()),
        staged: crate::parse_diff(&staged),
        unstaged: crate::parse_diff(&unstaged),
        untracked,
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

fn hash_untracked(repo: &Path, paths: &[String], hash: &mut Sha256) -> Result<()> {
    const LIMIT: u64 = 32 * 1024 * 1024;
    let too_large = || {
        Error::invalid("Untracked files exceed the exact-review limit (10000 files / 32 MiB). Ignore build output or review it with your own Git tools.")
    };
    if paths.len() > 10_000 {
        return Err(too_large());
    }
    let mut bytes = 0;
    for path in paths {
        validate_path(path)?;
        let full = repo.join(path);
        let meta = std::fs::symlink_metadata(&full)?;
        hash.update(path.as_bytes());
        hash.update([0]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            hash.update(meta.permissions().mode().to_le_bytes());
        }
        if meta.file_type().is_symlink() {
            hash.update(std::fs::read_link(&full)?.as_os_str().as_encoded_bytes());
        } else if meta.is_file() {
            use std::io::Read;
            if meta.len() > LIMIT - bytes {
                return Err(too_large());
            }
            let mut file = std::fs::File::open(&full)?.take(LIMIT - bytes + 1);
            let mut buffer = [0; 8192];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                if bytes > LIMIT {
                    return Err(too_large());
                }
                hash.update(&buffer[..n]);
            }
        } else {
            return Err(Error::invalid(
                "inspect nested repositories and special files with your own Git tools",
            ));
        }
        hash.update([0]);
    }
    Ok(())
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
        if matches!(action, Action::Push) && d.push == DeliveryPolicy::Manual
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
async fn publication(
    repo: &Path,
    chat: i64,
    head: Option<&str>,
    action: &Action,
) -> Result<Option<Remote>> {
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
        let result=execute(&op.snapshot).await;
        match result {
            Ok(result) if owner.quiescent()=>store.settle_checkout_operation(chat,id,"done",&result),
            Ok(result)=>store.settle_checkout_operation(chat,id,"inspection",&format!("{result}; command groups may still be alive. Drain and inspect; no retry.")),
            Err(e)=>store.settle_checkout_operation(chat,id,"inspection",&format!("{e}. The action may have happened. Drain and inspect; no automatic retry, reset or cleanup.")),
        }
    }).await
}
async fn execute(s: &Snapshot) -> Result<String> {
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
        Action::Push | Action::PullRequest => publish(s).await,
    }
}
async fn publish(s: &Snapshot) -> Result<String> {
    let repo = Path::new(&s.workspace);
    let r = s
        .remote
        .as_ref()
        .ok_or_else(|| Error::invalid("missing approved destination"))?;
    let head = s
        .head
        .as_deref()
        .ok_or_else(|| Error::invalid("missing approved commit"))?;
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
    store.checkout_available(chat)?;
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
        _ => return Err(Error::invalid("choose staged or unstaged diff")),
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
    store.record_checkout_finding(chat, input)
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
