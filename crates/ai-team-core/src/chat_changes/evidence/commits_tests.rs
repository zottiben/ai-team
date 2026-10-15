use super::{commit, pull_request};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn forbidden_driver(repo: &Path) -> (PathBuf, PathBuf) {
    let driver = repo.join(".git/forbidden-driver");
    let marker = repo.join(".git/driver-invoked");
    let quoted = marker.to_string_lossy().replace('\'', "'\"'\"'");
    std::fs::write(
        &driver,
        format!("#!/bin/sh\nprintf called > '{quoted}'\nexit 97\n"),
    )
    .unwrap();
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
    (driver, marker)
}
async fn linked_evidence(repo: &Path, base: &str, target: &str) {
    let tree = git(repo, &["rev-parse", &format!("{base}^{{tree}}")]);
    let divergent = git(
        repo,
        &["commit-tree", &tree, "-p", base, "-m", "divergent base"],
    );
    let comparison = pull_request(repo, &divergent, target).await.unwrap();
    assert_eq!(comparison.files_total, Some(2));
    assert!(comparison.basis.contains(&format!("merge-base {base}")));
    assert!(comparison
        .basis
        .contains(&format!("captured base {divergent}")));
    let linked = tempfile::tempdir().unwrap();
    git(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            linked.path().to_str().unwrap(),
            target,
        ],
    );
    std::fs::write(linked.path().join("side.txt"), "different linked bytes\n").unwrap();
    let evidence = commit(linked.path(), Some(base), target).await.unwrap();
    assert_eq!(evidence.files_total, Some(2), "{evidence:?}");
    assert!(evidence
        .files
        .iter()
        .flat_map(|entry| &entry.file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .any(|line| line.text == "side version"));
    git(
        repo,
        &[
            "worktree",
            "remove",
            "--force",
            linked.path().to_str().unwrap(),
        ],
    );
}
#[tokio::test]
async fn missing_promisor_objects_cannot_start_a_fetch_transport() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("source.txt"), "local object\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "source"]);
    let head = git(repo, &["rev-parse", "HEAD"]);
    let blob = git(repo, &["rev-parse", "HEAD:source.txt"]);
    let (driver, marker) = forbidden_driver(repo);
    git(repo, &["config", "core.repositoryFormatVersion", "1"]);
    git(repo, &["config", "extensions.partialClone", "origin"]);
    git(repo, &["config", "remote.origin.promisor", "true"]);
    git(
        repo,
        &[
            "config",
            "remote.origin.url",
            &format!("ext::{}", driver.display()),
        ],
    );
    git(repo, &["config", "protocol.ext.allow", "always"]);
    std::fs::remove_file(repo.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    let evidence = commit(repo, None, &head).await.unwrap();
    assert_eq!(evidence.files_total, None, "{evidence:?}");
    assert!(evidence.note.unwrap().contains("unavailable"));
    assert!(
        !marker.exists(),
        "a read-only preview must not launch a missing-object transport"
    );
}
#[tokio::test]
async fn captures_root_ranges_and_first_parent_merges_without_reading_current_files() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("root.txt"), "root version\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "root"]);
    let root = git(repo, &["rev-parse", "HEAD"]);
    let (driver, marker) = forbidden_driver(repo);
    git(repo, &["config", "diff.external", driver.to_str().unwrap()]);
    git(
        repo,
        &[
            "config",
            "diff.forbidden.textconv",
            driver.to_str().unwrap(),
        ],
    );
    std::fs::write(repo.join(".git/info/attributes"), "*.txt diff=forbidden\n").unwrap();
    let index = std::fs::read(repo.join(".git/index")).unwrap();
    let evidence = commit(repo, None, &root).await.unwrap();
    assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);
    assert!(
        !marker.exists(),
        "pinned previews must not invoke external diff/textconv drivers"
    );
    assert_eq!(evidence.files_total, Some(1), "{evidence:?}");
    assert_eq!(
        evidence.files[0].file.hunks[0].lines[0].text,
        "root version"
    );
    git(repo, &["checkout", "-qb", "side"]);
    std::fs::write(repo.join("side.txt"), "side version\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "side"]);
    git(repo, &["checkout", "main"]);
    std::fs::write(repo.join("main.txt"), "main version\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "main"]);
    git(
        repo,
        &["merge", "--no-ff", "--no-gpg-sign", "-m", "merge", "side"],
    );
    let merged = git(repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("side.txt"), "uncommitted replacement\n").unwrap();
    let evidence = commit(repo, None, &merged).await.unwrap();
    assert_eq!(evidence.files_total, Some(1));
    assert!(evidence.basis.contains("first parent"));
    assert_eq!(evidence.files[0].file.path, "side.txt");
    assert_eq!(
        evidence.files[0].file.hunks[0].lines[0].text,
        "side version"
    );
    let range = commit(repo, Some(&root), &merged).await.unwrap();
    assert_eq!(range.files_total, Some(2));
    linked_evidence(repo, &root, &merged).await;
    assert_eq!(
        commit(repo, Some(&merged), &merged)
            .await
            .unwrap()
            .files_total,
        Some(0)
    );
    let missing = commit(repo, None, &"f".repeat(40)).await.unwrap();
    assert_eq!(missing.files_total, None);
    assert!(missing.note.unwrap().contains("unavailable"));
    std::fs::write(repo.join("large.txt"), "x".repeat(2 * 1024 * 1024 + 1)).unwrap();
    git(repo, &["add", "large.txt"]);
    git(repo, &["commit", "-qm", "large"]);
    let large = git(repo, &["rev-parse", "HEAD"]);
    let bounded = commit(repo, None, &large).await.unwrap();
    assert_eq!(bounded.files_total, None);
    assert!(bounded
        .note
        .unwrap()
        .contains("no partial patch was parsed"));
}
