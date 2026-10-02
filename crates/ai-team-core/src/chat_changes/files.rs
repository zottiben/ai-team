//! Browse the reviewed commit's objects, never files in a returned/reused checkout.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{draft, git, DraftTarget};
use crate::{Error, Result, Store};

#[derive(Debug, Serialize)]
pub struct CommitEntry {
    pub path: String,
    pub kind: String,
    pub size: Option<u64>,
    #[serde(skip)]
    object: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitFileRequest {
    pub target: DraftTarget,
    pub path: String,
}
#[derive(Debug, Serialize)]
pub struct CommitFile {
    pub path: String,
    pub commit_sha: String,
    pub text: Option<String>,
    pub reason: Option<String>,
}

pub async fn committed_tree(
    store: &mut Store,
    chat: i64,
    target: &DraftTarget,
) -> Result<Vec<CommitEntry>> {
    let draft = draft(store, chat, target)?;
    let workspace = store.chat(chat)?.workspace_path;
    let tree = git(
        Path::new(&workspace),
        &["ls-tree", "-rlz", "--full-tree", &draft.commit_sha],
    )
    .await?;
    tree.split('\0')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (metadata, path) = line
                .split_once('\t')
                .ok_or_else(|| Error::invalid("invalid committed tree entry"))?;
            let fields: Vec<_> = metadata.split_whitespace().collect();
            if fields.len() != 4 {
                return Err(Error::invalid("invalid committed tree metadata"));
            }
            Ok(CommitEntry {
                path: path.into(),
                kind: fields[1].into(),
                object: fields[2].into(),
                size: fields[3].parse().ok(),
            })
        })
        .collect()
}
pub async fn committed_file(
    store: &mut Store,
    chat: i64,
    request: &CommitFileRequest,
) -> Result<CommitFile> {
    let draft = draft(store, chat, &request.target)?;
    let entry = committed_tree(store, chat, &request.target)
        .await?
        .into_iter()
        .find(|file| file.path == request.path)
        .ok_or_else(|| Error::invalid("that path is not in this chat's reviewed commit"))?;
    let mut result = CommitFile {
        path: entry.path,
        commit_sha: draft.commit_sha,
        text: None,
        reason: None,
    };
    if entry.kind != "blob" {
        result.reason = Some("Gitlink: inspect the pinned submodule separately.".into());
    } else if entry.size.is_none_or(|size| size > 256 * 1024) {
        result.reason = Some(
            "File exceeds the 256 KiB preview limit; inspect the commit in your editor.".into(),
        );
    } else {
        let workspace = store.chat(chat)?.workspace_path;
        let text = git(Path::new(&workspace), &["cat-file", "blob", &entry.object]).await?;
        if text.contains('\0') {
            result.reason = Some("Binary content is not shown as text.".into());
        } else {
            result.text = Some(text);
        }
    }
    Ok(result)
}
