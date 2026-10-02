//! Chat-owned review and human delivery. A verified draft is a commit, not a lease path.

mod delivery;
mod files;
mod recovery;
mod retained;
pub use delivery::{approve, preview, Delivery, DeliveryAction, DeliverySnapshot};
pub use files::{committed_file, committed_tree, CommitEntry, CommitFile, CommitFileRequest};
pub use recovery::{
    acknowledge, inspect, CheckoutEvidence, DeliveryAcknowledgement, DeliveryInspection,
};
pub use retained::{
    inspect_retained, retained_file, KeepRetained, RetainedFile, RetainedFileRequest,
    RetainedInspection,
};

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{ChatBuildSlice, ChatTeamRun, Error, FileDiff, Result, Store};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DraftTarget {
    pub run_id: i64,
    pub slice_key: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Draft {
    pub target: DraftTarget,
    pub base_sha: String,
    pub commit_sha: String,
    pub branch: String,
    pub lease_state: String,
    pub worktree_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Changes {
    pub workspace_path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub issues: Vec<String>,
    pub staged: Vec<FileDiff>,
    pub unstaged: Vec<FileDiff>,
    pub untracked: Vec<String>,
    pub drafts: Vec<Draft>,
    pub deliveries: Vec<Delivery>,
}

#[derive(Debug, Serialize)]
pub struct DraftReview {
    pub draft: Draft,
    pub files: Vec<FileDiff>,
    pub findings: Vec<crate::Event>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub target: DraftTarget,
    pub body: String,
}

/// The working checkout may be shared by sequential chats. These are its real changes,
/// not a claim that everything dirty was written by this chat or by its last agent.
pub async fn changes(store: &mut Store, chat_id: i64) -> Result<Changes> {
    let chat = store.chat(chat_id)?;
    let repo = Path::new(&chat.workspace_path);
    let mut issues = Vec::new();
    let head = metadata(
        git(repo, &["rev-parse", "--verify", "HEAD"]).await,
        &mut issues,
    )
    .map(|head| head.trim().to_owned());
    let staged = git(
        repo,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--unified=3",
            "--",
        ],
    )
    .await;
    let staged = metadata(staged, &mut issues).unwrap_or_default();
    let unstaged = git(
        repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--unified=3",
            "--",
        ],
    )
    .await;
    let unstaged = metadata(unstaged, &mut issues).unwrap_or_default();
    let untracked = metadata(
        git(repo, &["ls-files", "--others", "--exclude-standard", "-z"]).await,
        &mut issues,
    )
    .unwrap_or_default();
    let mut drafts = Vec::new();
    for run in store.chat_draft_runs(chat_id)? {
        for slice in store.chat_build_slices(run)? {
            if slice.build_status == "verified" && slice.commit_sha.is_some() {
                let target = DraftTarget {
                    run_id: run,
                    slice_key: slice.slice_key,
                    revision: slice.rev,
                };
                match draft(store, chat_id, &target) {
                    Ok(draft) => drafts.push(draft),
                    Err(error) => issues.push(format!("Run {run} · {}: {error}", target.slice_key)),
                }
            }
        }
    }
    Ok(Changes {
        workspace_path: chat.workspace_path.clone(),
        head,
        issues,
        branch: crate::current_branch(repo).await,
        staged: crate::parse_diff(&staged),
        unstaged: crate::parse_diff(&unstaged),
        untracked: untracked
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
        drafts,
        deliveries: store.chat_deliveries(chat_id)?,
    })
}

pub async fn review(store: &mut Store, chat_id: i64, target: &DraftTarget) -> Result<DraftReview> {
    let draft = draft(store, chat_id, target)?;
    let chat = store.chat(chat_id)?;
    let raw = git(
        Path::new(&chat.workspace_path),
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--unified=3",
            &draft.base_sha,
            &draft.commit_sha,
            "--",
        ],
    )
    .await?;
    let findings = store
        .chat_draft_findings(chat_id, target)?
        .into_iter()
        .filter(|event| {
            event.payload.as_ref().is_some_and(|p| {
                p.get("chat_draft_review")
                    .and_then(serde_json::Value::as_str)
                    == Some(draft.commit_sha.as_str())
                    && p.get("slice_key").and_then(serde_json::Value::as_str)
                        == Some(target.slice_key.as_str())
            })
        })
        .collect();
    Ok(DraftReview {
        draft,
        files: crate::parse_diff(&raw),
        findings,
    })
}

pub(crate) fn owned_slice(
    store: &Store,
    chat: i64,
    target: &DraftTarget,
) -> Result<(ChatTeamRun, ChatBuildSlice)> {
    store.chat(chat)?;
    let execution = store
        .chat_team_run(target.run_id)?
        .filter(|run| run.chat_id == chat)
        .ok_or_else(|| Error::invalid("that build does not belong to this chat"))?;
    let slice = store
        .chat_build_slices(target.run_id)?
        .into_iter()
        .find(|slice| slice.slice_key == target.slice_key && slice.rev == target.revision)
        .ok_or_else(|| {
            Error::invalid("this draft changed; refresh before reviewing or delivering")
        })?;
    Ok((execution, slice))
}

pub(crate) fn draft(store: &Store, chat: i64, target: &DraftTarget) -> Result<Draft> {
    let (execution, slice) = owned_slice(store, chat, target)?;
    if slice.build_status != "verified" {
        return Err(Error::invalid(
            "only verified draft commits can be reviewed for delivery",
        ));
    }
    let base_sha = execution
        .base_sha
        .ok_or_else(|| Error::invalid("this draft has no approved base"))?;
    let commit_sha = slice
        .commit_sha
        .ok_or_else(|| Error::invalid("this draft has no verified commit"))?;
    for sha in [&base_sha, &commit_sha] {
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::invalid("invalid recorded draft commit"));
        }
    }
    Ok(Draft {
        target: target.clone(),
        base_sha,
        commit_sha,
        branch: slice
            .branch
            .ok_or_else(|| Error::invalid("this draft has no branch"))?,
        lease_state: slice.lease_state,
        worktree_path: slice.worktree_path,
    })
}

fn metadata(value: Result<String>, issues: &mut Vec<String>) -> Option<String> {
    match value {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(error.to_string());
            None
        }
    }
}

pub(crate) async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let mut safe = vec![
        "-c",
        "core.fsmonitor=false",
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "protocol.ssh.allow=always",
        "-c",
        "protocol.git.allow=always",
        "-c",
        "protocol.file.allow=always",
    ];
    safe.extend_from_slice(args);
    crate::neighbours::git::git(repo, &safe).await
}
