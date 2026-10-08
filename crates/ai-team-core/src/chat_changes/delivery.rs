use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{draft, git, DraftTarget};
use crate::neighbours::github::gh;
use crate::{ChatTeamPhase, DeliveryPolicy, Error, Result, Store};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAction {
    Integrate,
    Push,
    PullRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverySnapshot {
    pub target: DraftTarget,
    pub action: DeliveryAction,
    pub commit_sha: String,
    pub workspace_path: String,
    pub checkout_head: String,
    pub checkout_branch: String,
    pub remote_url: Option<String>,
    pub delivery_branch: String,
    pub github_repo: Option<String>,
    pub base_branch: Option<String>,
    pub base_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub id: i64,
    pub chat_id: i64,
    pub snapshot: DeliverySnapshot,
    pub state: String,
    pub result: Option<String>,
    pub rev: i64,
}

pub async fn preview(
    store: &mut Store,
    chat: i64,
    target: &DraftTarget,
    action: DeliveryAction,
) -> Result<Delivery> {
    available(store, chat, target, action)?;
    let draft = draft(store, chat, target)?;
    let workspace = store.chat(chat)?.workspace_path;
    let repo = Path::new(&workspace);
    let mut snapshot = DeliverySnapshot {
        target: target.clone(),
        action,
        commit_sha: draft.commit_sha.clone(),
        checkout_head: git(repo, &["rev-parse", "--verify", "HEAD"])
            .await?
            .trim()
            .into(),
        checkout_branch: crate::current_branch(repo)
            .await
            .ok_or_else(|| Error::invalid("select a checkout branch before delivery"))?,
        workspace_path: workspace.clone(),
        remote_url: None,
        // Different verified commits never share a mutable publication ref.
        delivery_branch: format!(
            "ai-team/chat-{chat}/run-{}/{}",
            target.run_id, draft.commit_sha
        ),
        github_repo: None,
        base_branch: None,
        base_sha: None,
    };
    if action == DeliveryAction::Integrate {
        clean(repo).await?;
        git(repo, &["merge-base", "--is-ancestor", &snapshot.checkout_head, &snapshot.commit_sha]).await
            .map_err(|_| Error::invalid("this draft cannot fast-forward the checkout; use a PR or integrate divergent history manually"))?;
    } else {
        let url = origin(repo).await?;
        snapshot.remote_url = Some(url.clone());
        let published = remote_head(repo, &url, &snapshot.delivery_branch).await?;
        if published
            .as_deref()
            .is_some_and(|sha| sha != snapshot.commit_sha)
        {
            return Err(Error::invalid(
                "the delivery ref already contains different work; it will not be overwritten",
            ));
        }
        if action == DeliveryAction::PullRequest {
            if published.as_deref() != Some(snapshot.commit_sha.as_str()) {
                return Err(Error::invalid(
                    "push this exact draft before approving a pull request",
                ));
            }
            let json: serde_json::Value = serde_json::from_str(
                &gh(
                    repo,
                    &["repo", "view", &url, "--json", "url,defaultBranchRef"],
                )
                .await?,
            )?;
            let repository = string(&json, "url")?;
            let base = json
                .get("defaultBranchRef")
                .and_then(|v| v.get("name"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::invalid("GitHub did not report a default branch"))?
                .to_owned();
            snapshot.base_sha = Some(
                remote_head(repo, &url, &base)
                    .await?
                    .ok_or_else(|| Error::invalid("the PR base branch is missing"))?,
            );
            snapshot.github_repo = Some(repository);
            snapshot.base_branch = Some(base);
        }
    }
    store.record_chat_delivery_preview(chat, &snapshot)
}

/// Kept inside a watched task by HTTP callers. An interrupted command keeps its durable
/// claim; a reopened window must not guess whether a checkout or remote was changed.
pub async fn approve(store: &mut Store, chat: i64, id: i64, revision: i64) -> Result<Delivery> {
    let delivery = store.chat_delivery(chat, id)?;
    if delivery.state == "done" {
        return Ok(delivery);
    }
    available(
        store,
        chat,
        &delivery.snapshot.target,
        delivery.snapshot.action,
    )?;
    let owner = store.claim_chat_delivery(chat, id, revision)?;
    owner.track(async {
        let snapshot = &delivery.snapshot;
        if let Err(error) = recheck(store, chat, snapshot).await {
            return store.settle_chat_delivery(chat, id, if owner.quiescent() { "refused" } else { "inspection" }, &error.to_string());
        }
        store.attempt_chat_delivery(chat, id)?;
        let result = match execute(snapshot).await {
            Ok(result) => ("done", result),
            Err(error) => {
                let observed = if owner.quiescent() { observe(snapshot).await } else { Ok(None) };
                match observed {
                    Ok(Some((state, evidence))) => (state, format!("{error}; {evidence}")),
                    Ok(None) => ("inspection", format!("{error}. Delivery may have started; drain and inspect its outcome. No automatic retry or cleanup.")),
                    Err(probe) => ("inspection", format!("{error}; inspecting the result also failed: {probe}")),
                }
            }
        };
        let (state, result) = if owner.quiescent() { result } else { ("inspection", format!("{}; a delivery child may still be alive; drain and inspect before continuing.", result.1)) };
        store.settle_chat_delivery(chat, id, state, &result)
    }).await
}

pub(crate) fn available(
    store: &Store,
    chat: i64,
    target: &DraftTarget,
    action: DeliveryAction,
) -> Result<()> {
    let current = store.chat(chat)?;
    let (run, _) = super::owned_slice(store, chat, target)?;
    draft(store, chat, target)?;
    if current.archived
        || current.active_node_id.is_some()
        || run.phase != ChatTeamPhase::Finished
        || !run.quiescent
        || run.supervisor_alive()
    {
        return Err(Error::invalid(
            "finish or close the team build and wait for this chat to be idle before delivery",
        ));
    }
    if store
        .chat_children(target.run_id)?
        .iter()
        .any(|child| child.state != "drained")
    {
        return Err(Error::invalid(
            "this build has undrained process evidence; recover it before delivery",
        ));
    }
    let team = store.run(target.run_id)?.team_id.ok_or_else(|| {
        Error::invalid("this build's team is unavailable; use your own git/gh tools")
    })?;
    let settings = store.team(team)?.delivery;
    let policy = match action {
        DeliveryAction::Push => settings.push,
        DeliveryAction::PullRequest => settings.pr,
        DeliveryAction::Integrate => settings.merge,
    };
    if policy == DeliveryPolicy::Manual {
        return Err(Error::invalid(
            "this team's delivery policy is manual; use your own git/gh tools",
        ));
    }
    Ok(())
}

async fn recheck(store: &mut Store, chat: i64, expected: &DeliverySnapshot) -> Result<()> {
    available(store, chat, &expected.target, expected.action)?;
    let actual = draft(store, chat, &expected.target)?;
    let workspace = store.chat(chat)?.workspace_path;
    let repo = Path::new(&workspace);
    if workspace != expected.workspace_path
        || actual.commit_sha != expected.commit_sha
        || git(repo, &["rev-parse", "--verify", "HEAD"]).await?.trim() != expected.checkout_head
        || crate::current_branch(repo).await.as_deref() != Some(expected.checkout_branch.as_str())
    {
        return Err(Error::invalid(
            "the reviewed checkout or draft changed; request a fresh preview",
        ));
    }
    if expected.action == DeliveryAction::Integrate {
        clean(repo).await?;
        git(
            repo,
            &[
                "merge-base",
                "--is-ancestor",
                &expected.checkout_head,
                &expected.commit_sha,
            ],
        )
        .await?;
    } else {
        let url = origin(repo).await?;
        if Some(&url) != expected.remote_url.as_ref() {
            return Err(Error::invalid(
                "origin changed since approval; request a fresh preview",
            ));
        }
        let published = remote_head(repo, &url, &expected.delivery_branch).await?;
        if published
            .as_deref()
            .is_some_and(|sha| sha != expected.commit_sha)
        {
            return Err(Error::invalid(
                "the remote delivery ref changed; refusing to overwrite it",
            ));
        }
        if expected.action == DeliveryAction::PullRequest {
            if published.as_deref() != Some(expected.commit_sha.as_str()) {
                return Err(Error::invalid("the approved draft is no longer published"));
            }
            if remote_head(repo, &url, required(expected.base_branch.as_deref())?).await?
                != expected.base_sha
            {
                return Err(Error::invalid("the PR base moved; request a fresh preview"));
            }
        }
    }
    Ok(())
}

async fn integrate(s: &DeliverySnapshot) -> Result<String> {
    let repo = Path::new(&s.workspace_path);
    // No hooks, recursive submodule checkout, implicit commit, rebase or reset.
    git(
        repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
            "merge",
            "--ff-only",
            "--no-edit",
            "--no-autostash",
            "--no-overwrite-ignore",
            &s.commit_sha,
        ],
    )
    .await?;
    if git(repo, &["rev-parse", "HEAD"]).await?.trim() != s.commit_sha {
        return Err(Error::invalid(
            "integration did not reach the approved commit",
        ));
    }
    Ok(format!(
        "Integrated {} into {}",
        s.commit_sha, s.checkout_branch
    ))
}

async fn execute(s: &DeliverySnapshot) -> Result<String> {
    let repo = Path::new(&s.workspace_path);
    match s.action {
        DeliveryAction::Integrate => integrate(s).await,
        DeliveryAction::Push => {
            let url = required(s.remote_url.as_deref())?;
            if remote_head(repo, url, &s.delivery_branch).await?.as_deref()
                != Some(s.commit_sha.as_str())
            {
                // An absent ref is the only thing we may create. Even a concurrent
                // same-branch publisher cannot turn this into an unapproved overwrite.
                git(
                    repo,
                    &[
                        "-c",
                        "core.hooksPath=/dev/null",
                        "push",
                        "--no-verify",
                        "--no-follow-tags",
                        "--recurse-submodules=no",
                        &format!("--force-with-lease=refs/heads/{}:", s.delivery_branch),
                        url,
                        &format!("{}:refs/heads/{}", s.commit_sha, s.delivery_branch),
                    ],
                )
                .await?;
            }
            if remote_head(repo, url, &s.delivery_branch).await?.as_deref()
                != Some(s.commit_sha.as_str())
            {
                return Err(Error::invalid("could not confirm the published draft"));
            }
            Ok(format!(
                "Published {} to {url} · {}",
                s.commit_sha, s.delivery_branch
            ))
        }
        DeliveryAction::PullRequest => {
            let repository = required(s.github_repo.as_deref())?;
            let base = required(s.base_branch.as_deref())?;
            let json: serde_json::Value = serde_json::from_str(
                &gh(
                    repo,
                    &[
                        "pr",
                        "list",
                        "--repo",
                        repository,
                        "--head",
                        &s.delivery_branch,
                        "--state",
                        "all",
                        "--json",
                        "url,state,baseRefName,headRefOid",
                    ],
                )
                .await?,
            )?;
            let found = json
                .as_array()
                .ok_or_else(|| Error::invalid("GitHub returned an invalid PR list"))?;
            if !found.is_empty() {
                if found.len() != 1
                    || string(&found[0], "state")? != "OPEN"
                    || string(&found[0], "baseRefName")? != base
                    || string(&found[0], "headRefOid")? != s.commit_sha
                {
                    return Err(Error::invalid("this delivery ref has a different or closed PR; it will not be adopted or retargeted"));
                }
                return string(&found[0], "url");
            }
            gh(repo, &["pr", "create", "--draft", "--repo", repository, "--head", &s.delivery_branch, "--base", base, "--title", &format!("{} · chat build {}", s.target.slice_key, s.target.run_id), "--body", &format!("Human-approved ai-team draft `{}`. Verification belongs to this commit; integration and merge remain separate decisions.", s.commit_sha)]).await?;
            match observe(s).await? {
                Some(("done", url)) => Ok(url),
                _ => Err(Error::invalid("GitHub did not yet confirm the exact created PR; keep the outcome for inspection")),
            }
        }
    }
}

/// Called only while holding the delivery receipt, after every attempted command has
/// drained. Remote absence is never proof of no effect: a submitted request may still
/// complete remotely after the local command ends.
pub(super) async fn observe(s: &DeliverySnapshot) -> Result<Option<(&'static str, String)>> {
    let repo = Path::new(&s.workspace_path);
    match s.action {
        DeliveryAction::Integrate => {
            let head = git(repo, &["rev-parse", "HEAD"]).await?;
            if crate::current_branch(repo).await.as_deref() != Some(s.checkout_branch.as_str()) {
                return Ok(None);
            }
            if head.trim() == s.commit_sha && clean(repo).await.is_ok() {
                return Ok(Some((
                    "done",
                    format!(
                        "Confirmed integration of {} into {}",
                        s.commit_sha, s.checkout_branch
                    ),
                )));
            }
            if head.trim() == s.checkout_head && clean(repo).await.is_ok() {
                return Ok(Some((
                    "refused",
                    "Integration left the checkout at its reviewed base; no work was integrated."
                        .into(),
                )));
            }
        }
        DeliveryAction::Push => {
            match remote_head(repo, required(s.remote_url.as_deref())?, &s.delivery_branch).await? {
                Some(sha) if sha == s.commit_sha => {
                    return Ok(Some((
                        "done",
                        format!(
                            "Confirmed publication of {} to {}",
                            s.commit_sha, s.delivery_branch
                        ),
                    )))
                }
                _ => {}
            }
        }
        DeliveryAction::PullRequest => {
            let found: serde_json::Value = serde_json::from_str(
                &gh(
                    repo,
                    &[
                        "pr",
                        "list",
                        "--repo",
                        required(s.github_repo.as_deref())?,
                        "--head",
                        &s.delivery_branch,
                        "--state",
                        "all",
                        "--json",
                        "url,state,baseRefName,headRefOid",
                    ],
                )
                .await?,
            )?;
            let found = found
                .as_array()
                .ok_or_else(|| Error::invalid("invalid PR list"))?;
            if found.len() == 1
                && string(&found[0], "state")? == "OPEN"
                && string(&found[0], "headRefOid")? == s.commit_sha
                && string(&found[0], "baseRefName")? == required(s.base_branch.as_deref())?
            {
                return Ok(Some(("done", string(&found[0], "url")?)));
            }
        }
    }
    Ok(None)
}

async fn clean(repo: &Path) -> Result<()> {
    if !git(repo, &["status", "--porcelain=v1", "--untracked-files=all"])
        .await?
        .is_empty()
    {
        return Err(Error::invalid(
            "the solo checkout is dirty; commit or stash it yourself before integration",
        ));
    }
    Ok(())
}
pub(crate) async fn origin(repo: &Path) -> Result<String> {
    let text = git(repo, &["remote", "get-url", "--push", "--all", "origin"]).await?;
    let urls: Vec<_> = text.lines().filter(|s| !s.is_empty()).collect();
    if urls.len() != 1
        || urls[0].starts_with('-')
        || urls[0].contains("::")
        || urls[0].chars().any(char::is_control)
    {
        return Err(Error::invalid(
            "delivery needs exactly one ordinary origin push URL, not a remote helper",
        ));
    }
    let url = urls[0];
    let ordinary = ["https://", "ssh://", "git://", "file://"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
        || Path::new(url).is_absolute()
        || (url.contains('@') && url.contains(':') && !url.contains("://"));
    if !ordinary {
        return Err(Error::invalid(
            "origin must use HTTPS, SSH, Git, or an explicit local repository path",
        ));
    }
    Ok(urls[0].to_owned())
}
pub(super) async fn remote_head(repo: &Path, url: &str, branch: &str) -> Result<Option<String>> {
    let text = git(
        repo,
        &["ls-remote", "--heads", url, &format!("refs/heads/{branch}")],
    )
    .await?;
    let mut lines = text.lines();
    let head = lines
        .next()
        .map(|line| {
            line.split_once('\t')
                .map(|(sha, _)| sha.to_owned())
                .ok_or_else(|| Error::invalid("invalid remote ref"))
        })
        .transpose()?;
    if lines.next().is_some() {
        return Err(Error::invalid("ambiguous remote ref"));
    }
    Ok(head)
}
pub(super) fn string(value: &serde_json::Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::invalid(format!("GitHub did not report {field}")))
}
fn required(value: Option<&str>) -> Result<&str> {
    value.ok_or_else(|| Error::invalid("incomplete delivery preview"))
}
