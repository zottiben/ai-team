//! Read retained files only when the existing lease still has conclusive ownership.
use std::{io::Read, path::Path};

use serde::{Deserialize, Serialize};

use super::{git, owned_slice, DraftTarget};
use crate::{chat::team::ownership::Ownership, Error, FileDiff, Result, Store, Worktrees};

#[derive(Debug, Serialize)]
pub struct RetainedInspection {
    pub chat_id: i64,
    pub target: DraftTarget,
    pub path: String,
    pub branch: String,
    pub head: String,
    pub staged: Vec<FileDiff>,
    pub unstaged: Vec<FileDiff>,
    pub untracked: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeepRetained {
    pub target: DraftTarget,
    pub reason: String,
}

pub async fn inspect_retained(
    store: &mut Store,
    chat: i64,
    target: &DraftTarget,
) -> Result<RetainedInspection> {
    owned_slice(store, chat, target)?;
    let _receipt = Ownership::acquire(store.path(), target.run_id)?;
    snapshot(store, chat, target).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedFileRequest {
    pub target: DraftTarget,
    pub path: String,
}
#[derive(Debug, Serialize)]
pub struct RetainedFile {
    pub path: String,
    pub text: Option<String>,
    pub reason: Option<String>,
}

pub async fn retained_file(
    store: &mut Store,
    chat: i64,
    request: &RetainedFileRequest,
) -> Result<RetainedFile> {
    owned_slice(store, chat, &request.target)?;
    let _receipt = Ownership::acquire(store.path(), request.target.run_id)?;
    let inspected = snapshot(store, chat, &request.target).await?;
    if !inspected.untracked.contains(&request.path)
        && !inspected
            .staged
            .iter()
            .chain(&inspected.unstaged)
            .any(|file| file.path == request.path)
    {
        return Err(Error::invalid(
            "that path is not among the inspected retained changes",
        ));
    }
    let path = crate::safe_join(Path::new(&inspected.path), &request.path)?;
    let metadata = std::fs::metadata(&path)?;
    let mut result = RetainedFile {
        path: request.path.clone(),
        text: None,
        reason: None,
    };
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        result.reason = Some("Preview requires a regular file no larger than 256 KiB.".into());
    } else {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 256 * 1024 || bytes.contains(&0) {
            result.reason =
                Some("Large or binary file; inspect it in the retained checkout.".into());
        } else {
            match String::from_utf8(bytes) {
                Ok(text) => result.text = Some(text),
                Err(_) => result.reason = Some("Non-UTF-8 content is not shown as text.".into()),
            }
        }
    }
    owned_slice(store, chat, &request.target)?;
    Ok(result)
}

async fn snapshot(
    store: &mut Store,
    chat: i64,
    target: &DraftTarget,
) -> Result<RetainedInspection> {
    let (execution, slice) = owned_slice(store, chat, target)?;
    if !execution.quiescent
        || execution.supervisor_alive()
        || store
            .chat_children(target.run_id)?
            .iter()
            .any(|child| child.state != "drained")
        || slice.release_started
        || !matches!(slice.lease_state.as_str(), "retained" | "leased")
    {
        return Err(Error::invalid("process or return ownership is uncertain; leave this path protected and inspect it manually"));
    }
    let current = store.chat(chat)?;
    let pool = Worktrees::at(&current.workspace_path);
    let path = slice
        .worktree_path
        .as_deref()
        .ok_or_else(|| Error::invalid("this responsibility has no proven checkout address"))?;
    let resolved = pool.resolve(Path::new(path)).await?;
    let entries = pool.pool().await?;
    let entry = entries
        .iter()
        .find(|entry| crate::same_worktree(&entry.path, &resolved.to_string_lossy()))
        .ok_or_else(|| Error::invalid("the retained path is not in this repository's pool"))?;
    if entry.main
        || entry.status != "leased"
        || entry.orphaned()
        || !entry.processes.is_empty()
        || slice.lease_holder.is_none()
        || entry.lease_holder != slice.lease_holder
    {
        return Err(Error::invalid(
            "this lease has another/unknown holder or live processes; leave its files alone",
        ));
    }
    let branch = slice
        .branch
        .ok_or_else(|| Error::invalid("this retained work has no recorded branch"))?;
    if crate::current_branch(&resolved).await.as_deref() != Some(branch.as_str()) {
        return Err(Error::invalid("this checkout is no longer on the recorded draft branch; do not read another owner's work"));
    }
    let head = git(&resolved, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_owned();
    let staged = git(
        &resolved,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--",
        ],
    )
    .await?;
    let unstaged = git(
        &resolved,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--",
        ],
    )
    .await?;
    let untracked = git(
        &resolved,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await?;
    owned_slice(store, chat, target)?;
    Ok(RetainedInspection {
        chat_id: chat,
        target: target.clone(),
        path: resolved.to_string_lossy().into_owned(),
        branch,
        head,
        staged: crate::parse_diff(&staged),
        unstaged: crate::parse_diff(&unstaged),
        untracked: untracked
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}
