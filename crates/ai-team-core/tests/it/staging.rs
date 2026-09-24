//! Per-hunk staging, against real git.
//!
//! Parsing a reconstructed patch back proves it is well formed; only `git apply --cached`
//! proves it is *right*. The failure worth catching is the quiet one - a patch git accepts
//! and applies to the wrong place, or one that drags a neighbouring hunk along with it,
//! which is exactly what per-hunk staging exists to prevent.

use std::process::Command;

fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git should be installed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A file with two well-separated changes, so hunks cannot merge into one.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    run_git(path, &["init", "-q", "-b", "main", "."]);
    run_git(path, &["config", "user.email", "t@t"]);
    run_git(path, &["config", "user.name", "t"]);

    let mut lines: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
    std::fs::write(path.join("file.txt"), format!("{}\n", lines.join("\n"))).unwrap();
    run_git(path, &["add", "-A"]);
    run_git(path, &["commit", "-qm", "base"]);

    // Two edits twenty lines apart: far enough that git emits two hunks.
    lines[2] = "line 3 CHANGED NEAR THE TOP".into();
    lines[32] = "line 33 CHANGED NEAR THE BOTTOM".into();
    std::fs::write(path.join("file.txt"), format!("{}\n", lines.join("\n"))).unwrap();
    dir
}

#[tokio::test]
async fn staging_one_hunk_stages_only_that_hunk() {
    let dir = repo();
    let path = dir.path();

    let files = ai_team_core::parse_diff(&run_git(path, &["diff", "--no-color", "--unified=3"]));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks.len(), 2, "the fixture needs two hunks");

    // The second one only.
    let one_hunk = ai_team_core::patch_for(&files[0], &files[0].hunks[1]);
    ai_team_core::apply_patch_cached(path, &one_hunk)
        .await
        .unwrap();

    let staged = run_git(path, &["diff", "--cached", "--no-color"]);
    assert!(staged.contains("CHANGED NEAR THE BOTTOM"), "{staged}");
    assert!(
        !staged.contains("CHANGED NEAR THE TOP"),
        "the other hunk came along: {staged}"
    );

    // And the first is still waiting in the worktree rather than lost.
    let unstaged = run_git(path, &["diff", "--no-color"]);
    assert!(unstaged.contains("CHANGED NEAR THE TOP"), "{unstaged}");
}

#[tokio::test]
async fn staging_both_hunks_one_at_a_time_ends_with_a_clean_worktree() {
    let dir = repo();
    let path = dir.path();

    // Re-read between applications: staging the first changes the index, so the second
    // hunk's patch has to be taken from a diff that already reflects it.
    for _ in 0..2 {
        let files =
            ai_team_core::parse_diff(&run_git(path, &["diff", "--no-color", "--unified=3"]));
        let Some(file) = files.first() else { break };
        let Some(hunk) = file.hunks.first() else {
            break;
        };
        let one_hunk = ai_team_core::patch_for(file, hunk);
        ai_team_core::apply_patch_cached(path, &one_hunk)
            .await
            .unwrap();
    }

    assert!(
        run_git(path, &["diff", "--no-color"]).trim().is_empty(),
        "everything should be staged"
    );
    let staged = run_git(path, &["diff", "--cached", "--no-color"]);
    assert!(staged.contains("CHANGED NEAR THE TOP"), "{staged}");
    assert!(staged.contains("CHANGED NEAR THE BOTTOM"), "{staged}");
}

#[tokio::test]
async fn a_hunk_that_no_longer_applies_to_the_index_is_refused() {
    // The window polls, so a stale patch is a matter of time: two clicks on stage, or one
    // click against a view that has not caught up. Applying it twice would record a change
    // on top of itself, so git's context check must be allowed to refuse - and the message
    // has to say what happened.
    //
    // Note this is about the *index*, not the worktree: `--cached` applies there, so
    // editing the file afterwards does not invalidate a patch. That is `git add -p`'s
    // behaviour too, and matching it is deliberate.
    let dir = repo();
    let path = dir.path();

    let files = ai_team_core::parse_diff(&run_git(path, &["diff", "--no-color", "--unified=3"]));
    let one_hunk = ai_team_core::patch_for(&files[0], &files[0].hunks[0]);

    ai_team_core::apply_patch_cached(path, &one_hunk)
        .await
        .unwrap();
    let error = ai_team_core::apply_patch_cached(path, &one_hunk)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("already staged"), "{error}");
}

#[tokio::test]
async fn a_new_file_can_be_staged_from_its_own_patch() {
    // An untracked file has no diff at all, so it is staged whole - but once `git add -N`
    // has recorded its existence it produces one, and that patch has to be valid.
    let dir = repo();
    let path = dir.path();
    std::fs::write(path.join("new.txt"), "brand new\nsecond line\n").unwrap();
    run_git(path, &["add", "-N", "new.txt"]);

    let files = ai_team_core::parse_diff(&run_git(path, &["diff", "--no-color", "--unified=3"]));
    let new_file = files.iter().find(|file| file.path == "new.txt").unwrap();
    let one_hunk = ai_team_core::patch_for(new_file, &new_file.hunks[0]);
    ai_team_core::apply_patch_cached(path, &one_hunk)
        .await
        .unwrap();

    let staged = run_git(path, &["diff", "--cached", "--no-color"]);
    assert!(staged.contains("brand new"), "{staged}");
}

#[tokio::test]
async fn committing_records_only_what_was_staged() {
    let dir = repo();
    let path = dir.path();

    let files = ai_team_core::parse_diff(&run_git(path, &["diff", "--no-color", "--unified=3"]));
    let one_hunk = ai_team_core::patch_for(&files[0], &files[0].hunks[1]);
    ai_team_core::apply_patch_cached(path, &one_hunk)
        .await
        .unwrap();

    let sha = ai_team_core::commit_staged(
        path,
        "fix: correct the bottom\n\nThe top is still in progress.",
    )
    .await
    .unwrap();
    assert_eq!(sha.len(), 40, "a commit sha: {sha}");

    let shown = run_git(path, &["show", "--no-color", &sha]);
    assert!(shown.contains("fix: correct the bottom"), "{shown}");
    // The body reached the message, on its own paragraph.
    assert!(shown.contains("The top is still in progress."), "{shown}");
    assert!(shown.contains("CHANGED NEAR THE BOTTOM"), "{shown}");
    assert!(!shown.contains("CHANGED NEAR THE TOP"), "{shown}");
}
