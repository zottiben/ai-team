//! A human-approved change of a chat's next-turn checkout, not a relaxed tool guard.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, Result, Store, Worktrees};

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRequest {
    pub id: i64,
    pub chat_id: i64,
    pub after_node_id: Option<i64>,
    pub from_path: String,
    pub to_path: String,
    pub state: String,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceChoice {
    pub path: String,
    pub name: String,
    pub branch: Option<String>,
    pub unavailable: Option<String>,
}

pub fn repository(store: &Store, chat: i64) -> Result<PathBuf> {
    project_repository(store, store.chat(chat)?.project_id)
}

pub fn project_repository(store: &Store, project: i64) -> Result<PathBuf> {
    store
        .project_repos(project)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .map(PathBuf::from)
        .ok_or_else(|| Error::invalid("this project's checkout is not registered"))
}

/// Git membership is authoritative. If awt is installed, its leases must be readable;
/// an unavailable status command is not evidence that a pooled checkout is free.
pub async fn choices(repo: &Path) -> Result<Vec<WorkspaceChoice>> {
    let git = crate::neighbours::git::worktrees_until(repo, std::future::pending()).await?;
    let has_awt = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("awt").is_file()));
    let pool = if has_awt {
        Worktrees::at(repo).pool().await?
    } else {
        Vec::new()
    };
    git.into_iter()
        .map(|(path, branch)| {
            let path = Path::new(&path)
                .canonicalize()?
                .to_string_lossy()
                .into_owned();
            let entry = pool
                .iter()
                .find(|entry| crate::same_worktree(&entry.path, &path));
            let unavailable = entry
                .filter(|entry| {
                    entry.status == "leased"
                        || entry.lease_holder.is_some()
                        || !entry.processes.is_empty()
                })
                .map(|entry| {
                    format!(
                        "This checkout is occupied in awt ({})",
                        entry.lease_holder.as_deref().unwrap_or(&entry.status)
                    )
                });
            let name = entry.map_or_else(
                || {
                    if crate::same_worktree(&path, &repo.to_string_lossy()) {
                        "main".into()
                    } else {
                        Path::new(&path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    }
                },
                |entry| entry.name.clone(),
            );
            Ok(WorkspaceChoice {
                path,
                name,
                branch,
                unavailable,
            })
        })
        .collect()
}

pub async fn validate(repo: &Path, requested: &Path) -> Result<PathBuf> {
    let target = Worktrees::at(repo).resolve(requested).await?;
    let choice = choices(repo)
        .await?
        .into_iter()
        .find(|choice| crate::same_worktree(&choice.path, &target.to_string_lossy()))
        .ok_or_else(|| Error::invalid("the requested worktree disappeared; choose it again"))?;
    if let Some(reason) = choice.unavailable {
        return Err(Error::invalid(reason));
    }
    Ok(target)
}
