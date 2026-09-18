//! The diff parser, checked against the git that is actually installed.
//!
//! Every other test in `diff.rs` feeds the parser a fixture, which proves it against an
//! idea of what git emits. This one runs git. It matters on both CI legs because macOS
//! ships a different git from the Linux runner, and the parser's whole job - putting the
//! right number on the right line - is exactly the thing a format change would break
//! silently.

use std::process::Command;

use ai_team_core::{FileStatus, LineKind};

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git should be installed");
    assert!(out.status.success(), "git {args:?} failed");
    // git's output is bytes, not text: a latin-1 source file, or a small binary git
    // decides to treat as text, both produce output that is not valid UTF-8. Production
    // decodes the same way, so a review renders rather than erroring out.
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_installed_git_produces_what_the_parser_expects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main", "."]);
    git(path, &["config", "user.email", "t@t"]);
    git(path, &["config", "user.name", "t"]);

    std::fs::write(
        path.join("lib.rs"),
        "fn one() {}\nfn two() {}\nfn four() {}\n",
    )
    .unwrap();
    std::fs::write(path.join("gone.rs"), "removed\n").unwrap();
    std::fs::write(
        path.join("before.rs"),
        "some content here\nand more of it\n",
    )
    .unwrap();
    std::fs::write(path.join("blank.rs"), "a\n\nb\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);

    std::fs::write(
        path.join("lib.rs"),
        "fn one() {}\nfn two(x: i32) {}\nfn three() {}\nfn four() {}\n",
    )
    .unwrap();
    std::fs::remove_file(path.join("gone.rs")).unwrap();
    git(path, &["mv", "before.rs", "after.rs"]);
    std::fs::write(path.join("blank.rs"), "a\n\nB\n").unwrap();
    std::fs::write(path.join("added.rs"), "brand new\nsecond line\n").unwrap();
    // No trailing newline: git emits `\ No newline at end of file`, which is not a line
    // and must not be numbered.
    std::fs::write(path.join("nonl.rs"), "no trailing newline").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "change"]);

    let files = ai_team_core::parse_diff(&git(path, &["diff", "HEAD~1", "HEAD"]));
    let by = |name: &str| {
        files
            .iter()
            .find(|f| f.path == name)
            .unwrap_or_else(|| panic!("expected {name} in {:?}", files.iter().map(|f| &f.path)))
    };

    // The numbers, which is the entire reason this module exists.
    let lib = by("lib.rs");
    assert_eq!(lib.status, FileStatus::Modified);
    assert_eq!((lib.additions, lib.deletions), (2, 1));
    let numbered: Vec<_> = lib.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.old, l.new))
        .collect();
    assert_eq!(
        numbered,
        [
            (LineKind::Context, Some(1), Some(1)),
            (LineKind::Removed, Some(2), None),
            (LineKind::Added, None, Some(2)),
            (LineKind::Added, None, Some(3)),
            (LineKind::Context, Some(3), Some(4)),
        ]
    );

    assert_eq!(by("added.rs").status, FileStatus::Added);
    assert_eq!(by("added.rs").hunks[0].lines[0].new, Some(1));
    assert_eq!(by("gone.rs").status, FileStatus::Removed);

    // A rename git reports as a rename - which it only does above its similarity
    // threshold, hence the deliberately substantial content.
    let after = by("after.rs");
    assert_eq!(after.status, FileStatus::Renamed);
    assert_eq!(after.old_path.as_deref(), Some("before.rs"));

    // git drops the leading space on a blank context line, and skipping it would
    // desynchronise every number below.
    let blank = by("blank.rs");
    assert_eq!(blank.hunks[0].lines[1].kind, LineKind::Context);
    assert_eq!(blank.hunks[0].lines[1].new, Some(2));
    assert_eq!(blank.hunks[0].lines[3].new, Some(3));

    let nonl = by("nonl.rs");
    assert_eq!(nonl.hunks[0].lines.len(), 1);
    assert_eq!(nonl.hunks[0].lines[0].new, Some(1));
}

#[test]
fn a_file_git_cannot_read_as_text_does_not_break_the_review() {
    // A NUL in the first 8000 bytes is how git decides something is binary; without one
    // it emits the raw bytes as if they were text. Either way the parser must produce a
    // file entry rather than losing the whole diff.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main", "."]);
    git(path, &["config", "user.email", "t@t"]);
    git(path, &["config", "user.name", "t"]);

    std::fs::write(path.join("blob.bin"), [0u8, 1, 2, 3, 4]).unwrap();
    std::fs::write(path.join("latin.txt"), b"caf\xe9\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "base"]);
    std::fs::write(path.join("blob.bin"), [0u8, 9, 9, 9, 9]).unwrap();
    std::fs::write(path.join("latin.txt"), b"caf\xe9 changed\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "change"]);

    let files = ai_team_core::parse_diff(&git(path, &["diff", "HEAD~1", "HEAD"]));
    let blob = files.iter().find(|f| f.path == "blob.bin").unwrap();
    assert!(blob.binary, "a NUL byte makes git call it binary");
    assert!(blob.hunks.is_empty());

    // Not valid UTF-8, but still a text file as far as git is concerned. It renders,
    // with the undecodable bytes replaced, and the line numbers are still right.
    let latin = files.iter().find(|f| f.path == "latin.txt").unwrap();
    assert!(!latin.binary);
    assert_eq!(latin.hunks[0].lines[0].old, Some(1));
}
