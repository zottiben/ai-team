use super::*;
use crate::diff::{FileStatus, Hunk, Line, LineKind};

fn file(text: String) -> FileDiff {
    FileDiff {
        path: "source.rs".into(),
        old_path: None,
        status: FileStatus::Added,
        binary: false,
        additions: 1,
        deletions: 0,
        hunks: vec![Hunk {
            header: "@@ -0,0 +1 @@".into(),
            old_start: 0,
            new_start: 1,
            lines: vec![Line {
                kind: LineKind::Added,
                old: None,
                new: Some(1),
                text,
            }],
        }],
    }
}
fn candidate(file: &FileDiff) -> BorrowedFile<'_> {
    BorrowedFile {
        area: "staged",
        file,
        revision: Some("raw-file-identity"),
        reason: None,
        hunks_omitted: false,
    }
}
#[test]
fn bounds_encoded_bytes_without_cloning_or_truncating_source_lines() {
    let source = file("\0".repeat(MAX_BYTES / 2));
    let captured = capture("index", &[candidate(&source)]).unwrap();
    assert_eq!(captured.files_total, Some(1));
    assert_eq!(captured.omitted_files, 0);
    assert!(captured.files[0].hunks_omitted);
    assert!(captured.files[0].file.hunks.is_empty());
    assert_eq!(captured.files[0].file.additions, 1);
    assert_eq!(
        captured.files[0].revision.as_deref(),
        Some("raw-file-identity")
    );
    assert!(encode(&captured).unwrap().len() < MAX_BYTES);
    assert_eq!(source.hunks[0].lines[0].text.len(), MAX_BYTES / 2);
}
#[test]
fn line_and_file_limits_are_explicit_and_unknown_is_not_zero() {
    let mut source = file("short".into());
    source.hunks[0].lines = vec![source.hunks[0].lines[0].clone(); MAX_LINES + 1];
    assert!(capture("index", &[candidate(&source)]).unwrap().files[0].hunks_omitted);
    let small = file("short".into());
    let files = (0..=MAX_FILES)
        .map(|_| candidate(&small))
        .collect::<Vec<_>>();
    let captured = capture("index", &files).unwrap();
    assert_eq!(captured.files_total, Some(MAX_FILES + 1));
    assert_eq!(captured.files.len(), MAX_FILES);
    assert_eq!(captured.omitted_files, 1);
    assert!(captured.note.is_some());
    assert_eq!(capture("empty index", &[]).unwrap().files_total, Some(0));
    assert_eq!(
        PreviewEvidence::unavailable("commit", "not available").files_total,
        None
    );
}
#[test]
fn source_paths_are_never_truncated_into_a_different_identity() {
    let mut source = file("short".into());
    source.path = "x".repeat(4097);
    let captured = capture("index", &[candidate(&source)]).unwrap();
    assert!(captured.files.is_empty());
    assert_eq!(captured.omitted_files, 1);
    assert_eq!(captured.files_total, Some(1));
}
