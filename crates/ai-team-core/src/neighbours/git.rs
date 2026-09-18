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
pub(crate) async fn commit_paths(
    worktree: &Path,
    branch: &str,
    message: &str,
    paths: &[String],
) -> Result<Option<String>> {
    if paths.is_empty() {
        return Ok(None);
    }

    // A fresh branch per slice, from wherever the lease was handed over. `-B` rather
    // than `-b` so a retry of the same slice reuses the name instead of failing on it.
    git(worktree, &["checkout", "-B", branch]).await?;
    let mut add = vec!["add", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(worktree, &add).await?;

    // A path the agent deleted and the gates rebuilt, or one already matching HEAD, can
    // leave nothing staged. That is the same "nothing to commit" outcome.
    if git(worktree, &["diff", "--cached", "--name-only"])
        .await?
        .trim()
        .is_empty()
    {
        return Ok(None);
    }
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

/// The paths a turn changed.
///
/// Taken immediately after a turn and before the gates run, because running the gates
/// changes the worktree too: `cargo test` writes `target/` and `npm test` writes wherever
/// it likes. A repo without a `.gitignore` covering them would otherwise get a review
/// branch carrying a hundred files of build output. What ai-team commits is what the
/// agent did, not what checking it produced.
pub(crate) async fn changed_paths(worktree: &Path) -> Result<Vec<String>> {
    Ok(porcelain(worktree)
        .await?
        .lines()
        .filter_map(|line| {
            // `XY path`, and for a rename `XY old -> new`; the new name is what to add.
            let path = line.get(3..)?.trim();
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            let path = path.trim_matches('"');
            (!path.is_empty()).then(|| path.to_string())
        })
        .collect())
}

/// What changed, in `git status --porcelain` form. Empty means a clean tree.
///
/// Only the trailing newline is trimmed. A porcelain line is `XY path`, and an unstaged
/// modification's X is a space - so trimming the front eats it, and every path then
/// starts one character late.
pub(crate) async fn porcelain(worktree: &Path) -> Result<String> {
    Ok(git(worktree, &["status", "--porcelain"])
        .await?
        .trim_end()
        .to_string())
}

/// Keep the gates' own output out of git's view, for this lease only.
///
/// `.git/info/exclude` is git's per-checkout ignore file: it is not tracked, so this
/// does not edit the repository, and the worktree is returned to the pool afterwards
/// anyway. A repo whose own .gitignore already covers these loses nothing.
pub(crate) async fn ignore_build_output(worktree: &Path) -> Result<()> {
    let info = git(worktree, &["rev-parse", "--git-path", "info/exclude"]).await?;
    let path = worktree.join(info.trim());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains("# ai-team") {
        return Ok(());
    }
    let added = format!(
        "{existing}\n# ai-team: what running this project's checks leaves behind.\n\
         target/\nnode_modules/\ndist/\n.output/\n.eve/\n"
    );
    std::fs::write(&path, added)
        .map_err(|error| Error::invalid(format!("could not write {}: {error}", path.display())))?;
    Ok(())
}

/// The commit a branch forked from, which is what a review should be measured against.
///
/// Diffing a branch against the *tip* of main shows every change anybody else landed in
/// the meantime as though the agent had made it. The fork point shows only its work.
pub(crate) async fn merge_base(worktree: &Path, base: &str, head: &str) -> Result<String> {
    Ok(git(worktree, &["merge-base", base, head])
        .await?
        .trim()
        .to_string())
}

pub(crate) async fn rev_parse(worktree: &Path, rev: &str) -> Result<String> {
    Ok(git(worktree, &["rev-parse", rev]).await?.trim().to_string())
}

/// The branch work is cut from, as this repository actually names it.
///
/// Tried in order rather than assumed: `origin/HEAD` is what the remote says, and the two
/// fallbacks cover a repo with no remote. Returning `None` is a real answer - a checkout
/// with none of them has nothing to measure a branch against.
pub(crate) async fn default_branch(worktree: &Path) -> Option<String> {
    if let Ok(name) = git(
        worktree,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        let name = name.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    for candidate in ["main", "master"] {
        if git(worktree, &["rev-parse", "--verify", "--quiet", candidate])
            .await
            .is_ok_and(|sha| !sha.trim().is_empty())
        {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Which branch this checkout is on, or `None` on a detached head.
pub(crate) async fn current_branch(worktree: &Path) -> Option<String> {
    let name = git(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()?
        .trim()
        .to_string();
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// The diff from a commit to what is on disk right now, uncommitted edits included.
///
/// A local review can show work in progress, which a hosted one cannot - there is no
/// working tree on a server. Used when the checkout is sitting on the branch under
/// review, so a human who edits a file by hand sees the effect without committing first.
pub(crate) async fn diff_worktree(worktree: &Path, base: &str) -> Result<String> {
    git(
        worktree,
        &["diff", "--no-color", "--find-renames", "--unified=3", base],
    )
    .await
}

/// The unified diff between two commits.
///
/// `--no-color` because a configured `color.ui = always` would otherwise wrap every line
/// in escape codes and the parser would read them as content. `--find-renames` so a moved
/// file reads as a move rather than as a whole file deleted and another one written.
pub(crate) async fn diff(worktree: &Path, base: &str, head: &str) -> Result<String> {
    git(
        worktree,
        &[
            "diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            base,
            head,
        ],
    )
    .await
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

        let paths = changed_paths(dir.path()).await.unwrap();
        assert_eq!(paths, ["new.txt"]);
        let sha = commit_paths(dir.path(), "ai-team/PR1", "PR1: do the thing", &paths)
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
            commit_paths(dir.path(), "ai-team/PR2", "PR2: nothing", &[])
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn an_unstaged_modification_keeps_the_first_letter_of_its_path() {
        // `git status --porcelain` writes ` M path` for an unstaged edit. Trimming the
        // whole output eats that leading space, and every path then starts one character
        // late - which reaches git as `add -- rates/widget/src/lib.rs`.
        let dir = repo().await;
        std::fs::create_dir_all(dir.path().join("crates/widget/src")).unwrap();
        std::fs::write(dir.path().join("crates/widget/src/lib.rs"), "one\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "add lib"])
            .await
            .unwrap();

        std::fs::write(dir.path().join("crates/widget/src/lib.rs"), "two\n").unwrap();
        assert_eq!(
            changed_paths(dir.path()).await.unwrap(),
            ["crates/widget/src/lib.rs"]
        );
    }

    #[tokio::test]
    async fn a_renamed_file_is_added_under_the_name_it_now_has() {
        let dir = repo().await;
        std::fs::write(dir.path().join("old.txt"), "content\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "add old"])
            .await
            .unwrap();
        git(dir.path(), &["mv", "old.txt", "new.txt"])
            .await
            .unwrap();

        let paths = changed_paths(dir.path()).await.unwrap();
        assert!(paths.contains(&"new.txt".to_string()), "{paths:?}");
    }

    #[tokio::test]
    async fn the_gates_own_output_is_excluded_for_this_lease_only() {
        let dir = repo().await;
        ignore_build_output(dir.path()).await.unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/out.bin"), "junk").unwrap();
        std::fs::write(dir.path().join("real.rs"), "work\n").unwrap();

        assert_eq!(changed_paths(dir.path()).await.unwrap(), ["real.rs"]);
        // Written to .git/info/exclude, so the repository's own files are untouched.
        assert!(!dir.path().join(".gitignore").exists());

        // Running it twice must not keep appending.
        ignore_build_output(dir.path()).await.unwrap();
        let exclude =
            std::fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap_or_default();
        assert_eq!(exclude.matches("# ai-team").count(), 1);
    }

    #[tokio::test]
    async fn build_output_from_checking_the_work_is_not_committed_with_it() {
        // The gates run in the worktree, so `cargo test` leaves `target/` behind. A repo
        // whose .gitignore does not cover it would otherwise get a review branch with a
        // hundred files of build output on it, which is ai-team's mess, not the agent's.
        let dir = repo().await;
        std::fs::write(dir.path().join("src.rs"), "the agent's work\n").unwrap();

        // Captured after the turn, before the gates run.
        let paths = changed_paths(dir.path()).await.unwrap();

        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/out.bin"), "junk").unwrap();

        commit_paths(dir.path(), "ai-team/PR4", "PR4: work", &paths)
            .await
            .unwrap()
            .unwrap();

        let files = git(
            dir.path(),
            &["show", "--name-only", "--pretty=", "ai-team/PR4"],
        )
        .await
        .unwrap();
        assert!(files.contains("src.rs"), "{files}");
        assert!(
            !files.contains("target/"),
            "build output leaked in: {files}"
        );
    }

    #[tokio::test]
    async fn re_running_a_slice_reuses_its_branch_instead_of_failing_on_it() {
        let dir = repo().await;
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let paths = changed_paths(dir.path()).await.unwrap();
        commit_paths(dir.path(), "ai-team/PR3", "first", &paths)
            .await
            .unwrap();

        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let paths = changed_paths(dir.path()).await.unwrap();
        let second = commit_paths(dir.path(), "ai-team/PR3", "second", &paths)
            .await
            .unwrap();
        assert!(second.is_some(), "a retry must not fail on its own branch");
    }
}
