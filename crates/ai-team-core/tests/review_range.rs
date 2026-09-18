//! What a review is measuring, against real git.
//!
//! Two behaviours that are easy to get subtly wrong and impossible to notice from the
//! code: which commit a branch is measured *from*, and whether uncommitted work counts.
//! Both produce a diff that looks perfectly plausible when they are wrong - an empty one,
//! or a stale one - so both are checked here rather than reasoned about.

use std::process::Command;

use ai_team_core::{NewProject, Review, Store};

fn git(dir: &std::path::Path, args: &[&str]) -> String {
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

/// A repo with `main`, and an `ai-team/s1` branch carrying one change.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main", "."]);
    git(path, &["config", "user.email", "t@t"]);
    git(path, &["config", "user.name", "t"]);

    std::fs::write(path.join("lib.rs"), "fn add() {}\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    git(path, &["checkout", "-q", "-b", "ai-team/s1"]);
    std::fs::write(path.join("lib.rs"), "fn add() {}\nfn sub() {}\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "add sub"]);
    dir
}

/// A review of that branch, unpinned.
fn review(store: &mut Store, branch: &str) -> Review {
    let project = store
        .create_project(NewProject {
            name: "Demo".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .open_review(project.id, "PR1", None, None, Some(branch))
        .unwrap()
}

#[tokio::test]
async fn a_worktree_sitting_on_the_branch_still_shows_its_work() {
    // The bug this exists to catch: measuring from HEAD means measuring from the branch
    // itself whenever the checkout is on it - which is the normal state of a leased
    // worktree. The diff came back empty, which reads as "the agent changed nothing"
    // rather than as a broken tool.
    let dir = repo();
    assert_eq!(
        git(dir.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "ai-team/s1"
    );

    let mut store = Store::memory().unwrap();
    let review = review(&mut store, "ai-team/s1");

    let files = ai_team_core::diff_for(&review, dir.path()).await.unwrap();
    assert_eq!(files.len(), 1, "expected the branch's one changed file");
    assert_eq!(files[0].additions, 1);
    assert!(files[0].hunks[0]
        .lines
        .iter()
        .any(|line| line.text.contains("fn sub")));
}

#[tokio::test]
async fn an_uncommitted_hand_edit_shows_up_without_committing_first() {
    // A hosted review cannot do this - there is no working tree on a server. A local one
    // should: somebody who opens a file in the editor and fixes it expects to see that,
    // not to be told to commit so the tool can notice.
    let dir = repo();
    let mut store = Store::memory().unwrap();
    let review = review(&mut store, "ai-team/s1");

    std::fs::write(dir.path().join("lib.rs"), "fn add() {}\nfn subtract() {}\n").unwrap();

    let files = ai_team_core::diff_for(&review, dir.path()).await.unwrap();
    let added: Vec<_> = files[0].hunks[0]
        .lines
        .iter()
        .filter(|line| line.kind == ai_team_core::LineKind::Added)
        .map(|line| line.text.as_str())
        .collect();
    assert!(
        added.iter().any(|text| text.contains("subtract")),
        "{added:?}"
    );
    assert!(
        !added.iter().any(|text| text.contains("fn sub()")),
        "{added:?}"
    );
}

#[tokio::test]
async fn a_pinned_review_keeps_showing_the_diff_it_was_opened_on() {
    // Deliberately the opposite behaviour. A comment anchored to line 42 stays true only
    // if the diff under it does not move, so a review with a recorded range ignores both
    // later commits and the working tree.
    let dir = repo();
    let mut store = Store::memory().unwrap();
    let review = review(&mut store, "ai-team/s1");

    let base = git(dir.path(), &["rev-parse", "HEAD~1"]).trim().to_string();
    let head = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
    let pinned = store.set_review_range(review.id, &base, &head).unwrap();

    // The world moves on, in both the ways it can.
    std::fs::write(
        dir.path().join("lib.rs"),
        "fn add() {}\nfn totally_different() {}\n",
    )
    .unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-qm", "later work"]);
    std::fs::write(
        dir.path().join("lib.rs"),
        "fn add() {}\nfn and_again() {}\n",
    )
    .unwrap();

    let files = ai_team_core::diff_for(&pinned, dir.path()).await.unwrap();
    let text: String = files[0].hunks[0]
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    assert!(text.contains("fn sub"), "{text}");
    assert!(!text.contains("totally_different"), "{text}");
    assert!(!text.contains("and_again"), "{text}");
}

#[tokio::test]
async fn a_branch_reviewed_from_another_checkout_is_measured_from_main() {
    // The other normal shape: the window is on main and the agent's work is on a branch
    // it is not standing on. Nothing uncommitted should leak in from main's working tree.
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "main"]);
    std::fs::write(dir.path().join("scratch.txt"), "not part of the review\n").unwrap();

    let mut store = Store::memory().unwrap();
    let review = review(&mut store, "ai-team/s1");

    let files = ai_team_core::diff_for(&review, dir.path()).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "lib.rs");
}
