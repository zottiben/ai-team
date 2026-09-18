//! Just enough git to make a node's work survive its worktree.
//!
//! A leased worktree is borrowed: `awt return` cleans and resets it, so anything left
//! uncommitted is gone the moment the lease ends. The worktrees `awt` hands out are git
//! worktrees of the same repository, sharing one object store, so a commit made on a
//! branch inside one is still there afterwards - and a branch is what a human reviews.
//!
//! Committing is the control plane's job rather than the agent's. A model that forgets,
//! or commits half of it, loses work silently; this always runs and always reports what
//! it did.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::error::{Error, Result};

/// Commit everything in a worktree onto a branch.
///
/// `None` means there was nothing to commit, which is a real outcome worth reporting:
/// a node that finished without changing a file did not build its slice.
pub(crate) async fn commit_all(
    worktree: &Path,
    branch: &str,
    message: &str,
) -> Result<Option<String>> {
    if porcelain(worktree).await?.is_empty() {
        return Ok(None);
    }

    // A fresh branch per slice, from wherever the lease was handed over. `-B` rather
    // than `-b` so a retry of the same slice reuses the name instead of failing on it.
    git(worktree, &["checkout", "-B", branch]).await?;
    git(worktree, &["add", "-A"]).await?;
    // The committer is ai-team, not whoever's name is in the repo's config: a human
    // reading `git log` should be able to tell which commits they did not write.
    git(
        worktree,
        &[
            "-c",
            "user.name=ai-team",
            "-c",
            "user.email=ai-team@localhost",
            "commit",
            "--no-verify",
            "-m",
            message,
        ],
    )
    .await?;
    let sha = git(worktree, &["rev-parse", "HEAD"]).await?;
    Ok(Some(sha.trim().to_string()))
}

/// What changed, in `git status --porcelain` form. Empty means a clean tree.
pub(crate) async fn porcelain(worktree: &Path) -> Result<String> {
    Ok(git(worktree, &["status", "--porcelain"])
        .await?
        .trim()
        .to_string())
}

async fn git(worktree: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| Error::invalid(format!("could not run git: {error}")))?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "."],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git(dir.path(), &args).await.unwrap();
        }
        std::fs::write(dir.path().join("README.md"), "start\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "init"]).await.unwrap();
        dir
    }

    #[tokio::test]
    async fn work_lands_on_a_branch_that_outlives_the_worktree() {
        let dir = repo().await;
        std::fs::write(dir.path().join("new.txt"), "work\n").unwrap();

        let sha = commit_all(dir.path(), "ai-team/PR1", "PR1: do the thing")
            .await
            .unwrap()
            .expect("something changed, so something was committed");
        assert_eq!(sha.len(), 40, "a full sha: {sha}");

        // The branch exists and carries the message, which is what review reads.
        let log = git(
            dir.path(),
            &["log", "-1", "--pretty=%s%n%an", "ai-team/PR1"],
        )
        .await
        .unwrap();
        assert!(log.contains("PR1: do the thing"), "{log}");
        assert!(
            log.contains("ai-team"),
            "the author says who wrote it: {log}"
        );
        assert!(porcelain(dir.path()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_node_that_changed_nothing_says_so_rather_than_committing_air() {
        // An empty commit would put a green branch on the board for work nobody did.
        let dir = repo().await;
        assert_eq!(
            commit_all(dir.path(), "ai-team/PR2", "PR2: nothing")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn re_running_a_slice_reuses_its_branch_instead_of_failing_on_it() {
        let dir = repo().await;
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        commit_all(dir.path(), "ai-team/PR3", "first")
            .await
            .unwrap();

        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let second = commit_all(dir.path(), "ai-team/PR3", "second")
            .await
            .unwrap();
        assert!(second.is_some(), "a retry must not fail on its own branch");
    }
}
