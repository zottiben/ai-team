//! Bounded presentation captured with an approval. The operation snapshot remains
//! the authority; this evidence is never reconstructed from a later checkout.
mod commits;
#[cfg(all(test, unix))]
mod commits_tests;
#[cfg(test)]
mod tests;

use commits::{commit, pull_request};

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use super::checkout::{Action, Remote, State};
use super::{DeliveryAction, DeliverySnapshot};
use crate::{FileDiff, Result};

pub(crate) const MAX_BYTES: usize = 512 * 1024;
const MAX_FILES: usize = 200;
const MAX_LINES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewEvidence {
    pub basis: String,
    pub files_total: Option<usize>,
    pub omitted_files: usize,
    pub files: Vec<PreviewFile>,
    pub note: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewFile {
    pub area: String,
    pub file: FileDiff,
    pub revision: Option<String>,
    pub reason: Option<String>,
    pub hunks_omitted: bool,
}
#[derive(Serialize)]
struct BorrowedFile<'a> {
    area: &'a str,
    file: &'a FileDiff,
    revision: Option<&'a str>,
    reason: Option<&'a str>,
    hunks_omitted: bool,
}

struct Budget {
    written: usize,
    limit: usize,
    exceeded: bool,
}
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.written) {
            self.exceeded = true;
            return Err(io::Error::other("preview evidence byte limit"));
        }
        self.written += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn size(value: &impl Serialize, limit: usize) -> Result<Option<usize>> {
    let mut budget = Budget {
        written: 0,
        limit,
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut budget, value);
    if budget.exceeded {
        return Ok(None);
    }
    result?;
    Ok(Some(budget.written))
}
pub(crate) fn encode(value: &PreviewEvidence) -> Result<String> {
    if size(value, MAX_BYTES)?.is_none() {
        return Err(crate::Error::invalid(
            "approval evidence exceeds its storage limit",
        ));
    }
    Ok(serde_json::to_string(value)?)
}
impl PreviewEvidence {
    pub(crate) fn unavailable(basis: &str, note: &str) -> Self {
        Self {
            basis: basis.into(),
            files_total: None,
            omitted_files: 0,
            files: Vec::new(),
            note: Some(note.into()),
        }
    }
}

pub(crate) async fn delivery(
    snapshot: &DeliverySnapshot,
    draft_base: &str,
) -> Result<PreviewEvidence> {
    let repo = std::path::Path::new(&snapshot.workspace_path);
    match snapshot.action {
        DeliveryAction::Integrate => {
            commit(repo, Some(&snapshot.checkout_head), &snapshot.commit_sha).await
        }
        DeliveryAction::Push => commit(repo, Some(draft_base), &snapshot.commit_sha).await,
        DeliveryAction::PullRequest => match snapshot.base_sha.as_deref() {
            Some(base) => pull_request(repo, base, &snapshot.commit_sha).await,
            None => Ok(PreviewEvidence::unavailable(
                "PR comparison",
                "No exact PR base was recorded.",
            )),
        },
    }
}

pub(crate) async fn checkout(
    state: &State,
    action: &Action,
    remote: Option<&Remote>,
) -> Result<PreviewEvidence> {
    match action {
        Action::Stage { .. } | Action::Unstage { .. } | Action::Commit { .. } => {
            working_checkout(state, action)
        }
        Action::PushBranch { commit: target, .. } => {
            commit(std::path::Path::new(&state.workspace), None, target).await
        }
        Action::PullRequest => match (
            state.head.as_deref(),
            remote.and_then(|remote| remote.base_sha.as_deref()),
        ) {
            (Some(target), Some(base)) => {
                pull_request(std::path::Path::new(&state.workspace), base, target).await
            }
            _ => Ok(PreviewEvidence::unavailable(
                "PR comparison",
                "No exact PR target/base was recorded.",
            )),
        },
        Action::Push => match state.head.as_deref() {
            Some(target) => commit(std::path::Path::new(&state.workspace), None, target).await,
            None => Ok(PreviewEvidence::unavailable(
                "Checkout commit",
                "No commit identity was recorded.",
            )),
        },
    }
}

fn working_checkout(state: &State, action: &Action) -> Result<PreviewEvidence> {
    let mut candidates = Vec::new();
    let selected = match action {
        Action::Stage { path } | Action::Unstage { path } => Some(path.as_str()),
        Action::Commit { .. } => None,
        _ => return Ok(PreviewEvidence::unavailable("Pinned checkout commit", "Commit patch evidence was not captured. The exact target hash and destination remain in the operation snapshot.")),
    };
    for file in &state.staged {
        if selected.is_none_or(|path| path == file.path) {
            candidates.push(BorrowedFile {
                area: "staged",
                file,
                revision: state.staged_revisions.get(&file.path).map(String::as_str),
                reason: None,
                hunks_omitted: false,
            });
        }
    }
    if matches!(action, Action::Stage { .. }) {
        for file in &state.unstaged {
            if selected == Some(file.path.as_str()) {
                candidates.push(BorrowedFile {
                    area: "unstaged",
                    file,
                    revision: state.unstaged_revisions.get(&file.path).map(String::as_str),
                    reason: None,
                    hunks_omitted: false,
                });
            }
        }
        for entry in &state.untracked_files {
            if selected == Some(entry.file.path.as_str()) {
                candidates.push(BorrowedFile {
                    area: "untracked",
                    file: &entry.file,
                    revision: Some(&entry.fingerprint),
                    reason: entry.reason.as_deref(),
                    hunks_omitted: false,
                });
            }
        }
    }
    capture(
        "Captured index/worktree differences, bound to the checkout fingerprint",
        &candidates,
    )
}

fn capture(basis: &str, candidates: &[BorrowedFile<'_>]) -> Result<PreviewEvidence> {
    let mut result = PreviewEvidence {
        basis: basis.into(),
        files_total: Some(candidates.len()),
        omitted_files: 0,
        files: Vec::new(),
        note: None,
    };
    // Reserve the envelope and omission notice before admitting any file. Measure JSON,
    // not source length: control characters and quoting can multiply stored bytes.
    let mut remaining = MAX_BYTES - 2048;
    let mut lines = 0;
    for candidate in candidates {
        let file = candidate.file;
        if result.files.len() == MAX_FILES
            || file.path.len() > 4096
            || file.old_path.as_ref().is_some_and(|path| path.len() > 4096)
        {
            result.omitted_files += 1;
            continue;
        }
        let count = file
            .hunks
            .iter()
            .map(|hunk| hunk.lines.len())
            .sum::<usize>();
        let length = if count <= MAX_LINES - lines {
            size(candidate, remaining)?
        } else {
            None
        };
        let (entry, length) = if let Some(length) = length {
            lines += count;
            (
                PreviewFile {
                    area: candidate.area.into(),
                    file: file.clone(),
                    revision: candidate.revision.map(str::to_owned),
                    reason: candidate.reason.map(str::to_owned),
                    hunks_omitted: false,
                },
                length,
            )
        } else {
            let metadata = FileDiff {
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                status: file.status,
                binary: file.binary,
                hunks: Vec::new(),
                additions: file.additions,
                deletions: file.deletions,
            };
            let entry = PreviewFile { area: candidate.area.into(), file: metadata, revision: candidate.revision.map(str::to_owned),
                reason: Some(format!("{}Pinned hunks omitted by the 512 KiB / 10000-line preview limit. Counts describe the captured diff; inspect its exact bytes separately.", candidate.reason.map_or(String::new(), |reason| format!("{reason} ")))), hunks_omitted: true };
            let Some(length) = size(&entry, remaining)? else {
                result.omitted_files += 1;
                continue;
            };
            (entry, length)
        };
        remaining = remaining.saturating_sub(length + 1);
        result.files.push(entry);
    }
    if result.omitted_files > 0 {
        result.note = Some("Some files could not fit in the 200-file / 512 KiB pinned preview. No omitted filename or line anchor was invented.".into());
    }
    Ok(result)
}
