//! The save service, against a real filesystem.
//!
//! Every test writes inside its own temporary directory. On macOS that directory is
//! several levels inside `/private/var`, reached through the `/var` symlink, which is
//! exactly the shape that has broken path checks here before (D12) - so the worktree is
//! deliberately *not* canonicalised before it is handed in.

use std::path::{Path, PathBuf};

use super::*;

#[test]
fn preview_bounds_both_proposed_and_concurrently_grown_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, "original").unwrap();
    let base = open(dir.path(), "file").unwrap().revision.unwrap();
    let large = "x".repeat(usize::try_from(MAX_EDIT_BYTES).unwrap() + 1);
    assert!(preview(dir.path(), "file", &base, &large).is_err());
    std::fs::write(path, &large).unwrap();
    assert!(preview(dir.path(), "file", &base, "proposed").is_err());
}

#[cfg(unix)]
#[test]
fn a_second_exchange_does_not_enable_cleanup_before_verification() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "original").unwrap();
    let anchor = place::Anchor::open(dir.path(), "file").unwrap();
    let mut staged = anchor.stage("proposed", 0o600).unwrap();
    staged.exchange().unwrap();
    staged.exchange().unwrap();
    let displaced = staged.path();
    std::fs::write(&displaced, "another writer").unwrap();
    drop(staged); // Read/verification can fail here; it has not authorised deletion.
    assert_eq!(
        std::fs::read_to_string(displaced).unwrap(),
        "another writer"
    );
}

fn worktree() -> (tempfile::TempDir, PathBuf) {
    let (dir, root) = bare_worktree();
    // A real checkout, because the recovery store lives inside the repository's own git
    // directory and a tree without one takes a different path through `place.rs`.
    std::fs::create_dir(root.join(".git")).unwrap();
    (dir, root)
}

/// A directory with the file in it and nothing else - not a checkout.
fn bare_worktree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("notes.md"), "first\nsecond\n").unwrap();
    (dir, root)
}

fn opened(root: &Path, path: &str) -> OpenedFile {
    open(root, path).unwrap()
}

fn base_of(root: &Path, path: &str) -> FileRevision {
    opened(root, path).revision.expect("an editable file")
}

fn confirm(root: &Path, path: &str, base: &FileRevision, text: &str) -> SaveOutcome {
    let preview = preview(root, path, base, text).unwrap();
    save(root, path, base, &preview.proposed.hash, text).unwrap()
}

fn refusal(outcome: SaveOutcome) -> Refusal {
    match outcome {
        SaveOutcome::Refused(refusal) => refusal,
        SaveOutcome::Written(saved) => panic!("expected a refusal, wrote {saved:?}"),
    }
}

fn written(outcome: SaveOutcome) -> Saved {
    match outcome {
        SaveOutcome::Written(saved) => saved,
        SaveOutcome::Refused(refusal) => panic!("expected a write, refused: {refusal:?}"),
    }
}

/// Staged files still sitting in a directory. A save that returned leaves none of its
/// own: either it was exchanged away or its guard removed it.
fn staging_left(directory: &Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(super::place::STAGING_PREFIX))
        .collect()
}

/// A save with something landing in the one gap that matters: after the last check and
/// before the replacement.
fn racing(
    root: &Path,
    path: &str,
    base: &FileRevision,
    text: &str,
    between: &dyn Fn(),
) -> SaveOutcome {
    interleaved(
        root,
        path,
        base,
        text,
        Interleave {
            before_exchange: Some(between),
            ..Interleave::default()
        },
    )
}

fn interleaved(
    root: &Path,
    path: &str,
    base: &FileRevision,
    text: &str,
    interleave: Interleave<'_>,
) -> SaveOutcome {
    save_checked(
        root,
        path,
        base,
        &hash_of(text.as_bytes()),
        text,
        &interleave,
    )
    .unwrap()
}

/// **The save this module used to make**, kept so the race it loses can be demonstrated
/// rather than asserted in a comment.
///
/// Same checks, old replacement: re-read the path, then `rename` a staged sibling onto it
/// unconditionally. Both halves resolve the path again from the root, and the rename
/// unlinks whatever it lands on.
fn legacy_save(
    root: &Path,
    relative: &str,
    base: &FileRevision,
    text: &str,
    between: &dyn Fn(),
) -> SaveOutcome {
    use std::io::Write as _;

    let (path, _) = resolve(root, relative).unwrap();
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    let current = on_disk(&path).unwrap().expect("a file to replace");
    assert_eq!(
        current.0.hash, base.hash,
        "the legacy path checked this too"
    );

    let directory = path.parent().unwrap();
    let mut temporary = tempfile::Builder::new()
        .prefix(".ai-team-save-")
        .tempfile_in(directory)
        .unwrap();
    temporary.as_file_mut().write_all(text.as_bytes()).unwrap();
    temporary.as_file_mut().sync_all().unwrap();
    temporary
        .as_file()
        .set_permissions(metadata.permissions())
        .unwrap();

    let again = on_disk(&path).unwrap().expect("still there");
    if again.0.hash != current.0.hash || !same_file(&current.0, &again.0) {
        return refuse(RefusalReason::LostARace, LOST_A_RACE, Some(again));
    }

    between();

    temporary.persist(&path).unwrap();
    let written = std::fs::symlink_metadata(&path).unwrap();
    SaveOutcome::Written(Saved {
        path: relative.to_string(),
        revision: revision(&written, text.as_bytes()),
        replaced: current.0,
        preserved: None,
        preserve_note: None,
    })
}

#[test]
fn a_save_writes_exactly_the_previewed_bytes_and_rebases_the_buffer() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let saved = written(confirm(&root, "notes.md", &base, "first\nsecond\nthird\n"));

    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "first\nsecond\nthird\n"
    );
    assert_eq!(saved.replaced.hash, base.hash);
    assert_ne!(saved.revision.hash, base.hash);
    // The window saves again from the returned revision; if that did not describe the
    // file now, the second save of a session would always refuse.
    let again = confirm(&root, "notes.md", &saved.revision, "first\n");
    assert_eq!(written(again).replaced.hash, saved.revision.hash);
}

#[test]
fn a_file_that_changed_on_disk_is_refused_with_both_versions_intact() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    std::fs::write(root.join("notes.md"), "somebody else\n").unwrap();

    let refusal = refusal(confirm(&root, "notes.md", &base, "mine\n"));
    assert_eq!(refusal.reason, RefusalReason::ChangedOnDisk);
    assert_eq!(refusal.message, CHANGED_ON_DISK);
    assert_eq!(refusal.disk_text.unwrap().text, "somebody else\n");
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "somebody else\n",
        "the other writer's file is what is still there"
    );
}

#[test]
fn the_preview_says_a_file_moved_before_the_operator_confirms_anything() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let fresh = preview(&root, "notes.md", &base, "mine\n").unwrap();
    assert!(!fresh.stale);

    std::fs::write(root.join("notes.md"), "somebody else\n").unwrap();
    let moved = preview(&root, "notes.md", &base, "mine\n").unwrap();
    assert!(moved.stale);
    assert_eq!(moved.disk_text.unwrap().text, "somebody else\n");
}

#[test]
fn the_preview_diffs_the_proposal_against_the_bytes_it_would_replace() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let preview = preview(&root, "notes.md", &base, "first\nsecond\nthird\n").unwrap();

    assert_eq!(preview.change.added, 1);
    assert_eq!(preview.change.removed, 0);
    assert_eq!(
        preview.proposed.bytes,
        "first\nsecond\nthird\n".len() as u64
    );
    assert_eq!(preview.path, "notes.md");
    // Everything the confirmation is about, in the preview itself: no hidden server state.
    assert_eq!(preview.base, base);
    assert_eq!(preview.disk.unwrap().hash, base.hash);
}

#[test]
fn a_confirmation_cannot_be_replayed_against_different_bytes() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let preview = preview(&root, "notes.md", &base, "previewed\n").unwrap();

    let refusal = refusal(
        save(
            &root,
            "notes.md",
            &base,
            &preview.proposed.hash,
            "something else\n",
        )
        .unwrap(),
    );
    assert_eq!(refusal.reason, RefusalReason::NotThePreview);
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn a_file_replaced_by_an_identical_one_is_still_a_different_file() {
    // Same bytes, new inode: somebody rewrote it from elsewhere. The content check alone
    // would wave this through and the editor would be writing over a file it never read.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    std::fs::write(root.join("replacement"), "first\nsecond\n").unwrap();
    std::fs::rename(root.join("replacement"), root.join("notes.md")).unwrap();

    let outcome = confirm(&root, "notes.md", &base, "mine\n");
    if cfg!(unix) {
        assert_eq!(refusal(outcome).reason, RefusalReason::ReplacedOnDisk);
    } else {
        // No identity to compare: the hash is the whole check, and it matches.
        written(outcome);
    }
}

#[test]
fn a_deleted_file_is_refused_rather_than_recreated() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    std::fs::remove_file(root.join("notes.md")).unwrap();

    assert_eq!(
        refusal(confirm(&root, "notes.md", &base, "mine\n")).reason,
        RefusalReason::Missing
    );
    assert!(!root.join("notes.md").exists(), "a refusal creates nothing");
}

#[cfg(unix)]
#[test]
fn a_symlink_is_neither_opened_for_editing_nor_written_through() {
    let (_dir, root) = worktree();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.txt");
    std::fs::write(&target, "not yours\n").unwrap();
    std::os::unix::fs::symlink(&target, root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(root.join("notes.md"), root.join("inside.txt")).unwrap();

    // Out of the worktree: refused by the path check before anything is read.
    assert!(open(&root, "link.txt").is_err() || !open(&root, "link.txt").unwrap().editable);
    // Inside the worktree, so the path check passes - and it is still refused, because
    // showing one file and saving into another is the bug, not the escape.
    let inside = opened(&root, "inside.txt");
    assert!(!inside.editable);
    assert_eq!(
        inside.reason.unwrap(),
        "a symbolic link - open the file it points at instead"
    );

    let base = base_of(&root, "notes.md");
    let refusal = refusal(
        save(
            &root,
            "inside.txt",
            &base,
            &super::hash_of(b"mine\n"),
            "mine\n",
        )
        .unwrap(),
    );
    assert_eq!(refusal.reason, RefusalReason::NotAPlainFile);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "not yours\n");
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn a_path_that_leaves_the_checkout_is_refused_before_the_filesystem_is_touched() {
    let (_dir, root) = worktree();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.txt");
    std::fs::write(&target, "not yours\n").unwrap();

    let escape = format!(
        "../{}/secret.txt",
        outside.path().file_name().unwrap().to_string_lossy()
    );
    assert!(open(&root, &escape).is_err());
    assert!(save(
        &root,
        &escape,
        &base_of(&root, "notes.md"),
        &super::hash_of(b"x"),
        "x"
    )
    .is_err());
    assert!(save(
        &root,
        "/etc/hosts",
        &base_of(&root, "notes.md"),
        &super::hash_of(b"x"),
        "x"
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "not yours\n");
}

#[test]
fn a_checkout_reached_through_a_symlinked_parent_still_works() {
    // macOS hands out `/var/folders/...`, and `/var` is a link into `/private`. A check
    // that compared the raw path against a canonicalised root would refuse every file.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    assert!(opened(&root, "notes.md").editable);
    written(confirm(&root, "notes.md", &base, "still fine\n"));
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "still fine\n"
    );
}

#[test]
fn a_name_differing_only_in_case_never_reports_a_write_it_did_not_make() {
    // APFS folds case and ext4 does not, so the same request is two different files
    // depending on the machine. Either answer is correct; writing "saved" while the file
    // the operator is looking at is untouched is not.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    match confirm(&root, "NOTES.md", &base, "cased\n") {
        SaveOutcome::Written(_) => assert_eq!(
            std::fs::read_to_string(root.join("notes.md")).unwrap(),
            "cased\n",
            "a case-insensitive filesystem wrote the file that was open"
        ),
        SaveOutcome::Refused(refusal) => {
            assert_eq!(refusal.reason, RefusalReason::Missing);
            assert_eq!(
                std::fs::read_to_string(root.join("notes.md")).unwrap(),
                "first\nsecond\n"
            );
        }
    }
}

#[test]
fn crlf_survives_a_round_trip_and_mixed_endings_are_reported_not_fixed() {
    let (_dir, root) = worktree();
    std::fs::write(root.join("dos.txt"), "one\r\ntwo\r\n").unwrap();
    let dos = opened(&root, "dos.txt");
    assert_eq!(dos.eol, Eol::Crlf);
    assert_eq!(dos.line_separator, Some("\r\n"));

    let base = dos.revision.unwrap();
    written(confirm(&root, "dos.txt", &base, "one\r\ntwo\r\nthree\r\n"));
    assert_eq!(
        std::fs::read(root.join("dos.txt")).unwrap(),
        b"one\r\ntwo\r\nthree\r\n",
        "the bytes that were previewed are the bytes on disk"
    );

    std::fs::write(root.join("mixed.txt"), "one\r\ntwo\n").unwrap();
    let mixed = opened(&root, "mixed.txt");
    assert_eq!(mixed.eol, Eol::Mixed);
    assert_eq!(mixed.line_separator, None);
    let preview = preview(&root, "mixed.txt", &mixed.revision.unwrap(), "one\ntwo\n").unwrap();
    assert!(
        preview
            .notices
            .iter()
            .any(|notice| notice.contains("mixed line endings")),
        "{:?}",
        preview.notices
    );
}

#[test]
fn a_write_that_drops_the_final_newline_says_so() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let preview = preview(&root, "notes.md", &base, "first\nsecond").unwrap();
    assert!(preview
        .notices
        .iter()
        .any(|notice| notice == "No newline at end of file."));
}

#[test]
fn bytes_that_are_not_text_are_not_opened_for_editing() {
    let (_dir, root) = worktree();
    std::fs::write(root.join("logo.png"), [0x89, b'P', b'N', b'G', 0xff, 0xfe]).unwrap();
    let file = opened(&root, "logo.png");
    assert!(!file.editable);
    assert_eq!(file.reason.unwrap(), "not UTF-8 text");
    assert!(
        file.revision.is_none(),
        "a buffer cannot claim a base it never read"
    );
    assert!(file.text.is_empty());
}

#[test]
fn a_file_past_the_size_limit_is_refused_with_its_size() {
    let (_dir, root) = worktree();
    let big = vec![b'a'; usize::try_from(MAX_EDIT_BYTES).unwrap() + 1];
    std::fs::write(root.join("big.log"), &big).unwrap();
    let file = opened(&root, "big.log");
    assert!(!file.editable);
    assert!(file.reason.unwrap().contains("MiB"));

    let text = "x".repeat(usize::try_from(MAX_EDIT_BYTES).unwrap() + 1);
    let base = base_of(&root, "notes.md");
    assert_eq!(
        refusal(
            save(
                &root,
                "notes.md",
                &base,
                &super::hash_of(text.as_bytes()),
                &text
            )
            .unwrap()
        )
        .reason,
        RefusalReason::TooLarge
    );
}

#[cfg(unix)]
#[test]
fn a_saved_file_keeps_the_mode_it_had() {
    // The replacement is a new file renamed into place, so without this an executable
    // script silently stops being executable the first time it is edited.
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root) = worktree();
    let script = root.join("run.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let base = base_of(&root, "run.sh");
    written(confirm(&root, "run.sh", &base, "#!/bin/sh\necho there\n"));
    let mode = std::fs::metadata(&script).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn a_refused_save_leaves_no_temporary_file_behind() {
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    std::fs::write(root.join("notes.md"), "somebody else\n").unwrap();
    refusal(confirm(&root, "notes.md", &base, "mine\n"));
    written(confirm(
        &root,
        "notes.md",
        &base_of(&root, "notes.md"),
        "mine\n",
    ));

    let left: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".ai-team-save-"))
        .collect();
    assert_eq!(left, Vec::<String>::new());
}

#[test]
fn a_directory_is_not_a_file_to_edit() {
    let (_dir, root) = worktree();
    std::fs::create_dir(root.join("src")).unwrap();
    assert!(open(&root, "src").is_err());
}

#[test]
fn an_empty_file_opens_and_saves_without_pretending_to_have_line_endings() {
    let (_dir, root) = worktree();
    std::fs::write(root.join("empty.txt"), "").unwrap();
    let file = opened(&root, "empty.txt");
    assert!(file.editable);
    assert_eq!(file.eol, Eol::None);
    assert_eq!(file.bytes, 0);

    written(confirm(
        &root,
        "empty.txt",
        &file.revision.unwrap(),
        "now has text\n",
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("empty.txt")).unwrap(),
        "now has text\n"
    );
}

// ---------------------------------------------------------------------------------
// The gap between the last check and the replacement.
//
// Every test below puts a writer in exactly that gap. The first one shows what the old
// check-then-rename did with it; the rest show that nothing is lost now. They are not
// timing tests: the hook runs inside the save, so the write lands in the gap every run
// rather than once in a thousand.
// ---------------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn the_old_check_then_rename_destroys_a_write_that_lands_in_the_gap() {
    // This is the regression. It fails for the reported reason against the
    // implementation that was here: the rename unlinks the inode another process just
    // wrote, and with no name left on it those bytes are unreachable.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let file = root.join("notes.md");

    let saved = written(legacy_save(&root, "notes.md", &base, "mine\n", &|| {
        std::fs::write(&file, "theirs\n").unwrap();
    }));

    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "mine\n",
        "the rename landed on top of them"
    );
    assert!(
        saved.preserved.is_none(),
        "and there was nowhere their bytes could have gone"
    );
    let anywhere = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| std::fs::read_to_string(entry.unwrap().path()).ok())
        .any(|text| text == "theirs\n");
    assert!(!anywhere, "their write is gone from the checkout entirely");
}

#[cfg(unix)]
#[test]
fn an_exchange_keeps_a_write_that_lands_in_the_gap_and_refuses() {
    // The same writer, in the same gap, against the exchange. The save does not happen,
    // their bytes are back at the path, and this save's bytes are only in the buffer the
    // operator still has.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let file = root.join("notes.md");

    let refused = refusal(racing(&root, "notes.md", &base, "mine\n", &|| {
        std::fs::write(&file, "theirs\n").unwrap();
    }));

    assert_eq!(refused.reason, RefusalReason::LostARace);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "theirs\n",
        "their write survived the save and is where they put it"
    );
    assert_eq!(refused.disk_text.unwrap().text, "theirs\n");
    assert!(
        refused.preserved.is_empty(),
        "a rollback that verified leaves nothing for the operator to clear up"
    );
    assert_eq!(staging_left(&root), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_path_replaced_in_the_gap_keeps_the_replacement_rather_than_this_save() {
    // Not a write into the file: a different file moved onto the name. The exchange
    // displaces theirs, the identity check sees it is not what was checked, and the swap
    // is undone.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let file = root.join("notes.md");
    let other = root.join("other");

    let refused = refusal(racing(&root, "notes.md", &base, "mine\n", &|| {
        std::fs::write(&other, "a whole new file\n").unwrap();
        std::fs::rename(&other, &file).unwrap();
    }));

    assert_eq!(refused.reason, RefusalReason::LostARace);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "a whole new file\n",
        "the file they put there is the file that is there"
    );
    assert_eq!(staging_left(&root), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_descriptor_held_across_the_save_writes_into_a_file_that_still_has_a_name() {
    // The case a read cannot catch: another process opened the file before the save and
    // writes after it. The exchange does not unlink what it displaces, and the displaced
    // inode is kept, so those bytes land somewhere the response named.
    use std::io::{Seek as _, Write as _};
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let mut theirs = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("notes.md"))
        .unwrap();

    let saved = written(confirm(&root, "notes.md", &base, "mine\n"));
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "mine\n"
    );

    // Long after the swap, through the descriptor they still hold.
    theirs.rewind().unwrap();
    theirs.write_all(b"late!").unwrap();
    theirs.flush().unwrap();
    drop(theirs);

    let kept = saved.preserved.expect("the displaced original is kept");
    assert!(
        !kept.foreign,
        "nothing had written to it when it was checked"
    );
    assert!(
        !kept.in_checkout,
        "and it is out of the checkout's way: {}",
        kept.path
    );
    assert_eq!(
        std::fs::read_to_string(&kept.path).unwrap(),
        "late!\nsecond\n",
        "their late write is in the file the save reported, not nowhere"
    );

    // And the store lists it under the name a discard takes.
    let listed = preserved(&root).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, kept.name);
    assert_eq!(listed[0].origin, "notes.md");
    assert_eq!(
        read_preserved(&root, &kept.name).unwrap().unwrap().text,
        "late!\nsecond\n"
    );
    assert!(discard_preserved(&root, &kept.name).unwrap());
    assert!(preserved(&root).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlink_swapped_onto_the_path_in_the_gap_is_refused_and_left_where_it_was() {
    // Not a write and not a plain replacement: a link dropped onto the name. The
    // exchange displaces the link rather than following it, the rollback puts it back,
    // and the file it pointed at is never opened, let alone written.
    let (_dir, root) = worktree();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.txt");
    std::fs::write(&target, "not yours\n").unwrap();

    let base = base_of(&root, "notes.md");
    let refused = refusal(racing(&root, "notes.md", &base, "mine\n", &|| {
        std::os::unix::fs::symlink(&target, root.join("link")).unwrap();
        std::fs::rename(root.join("link"), root.join("notes.md")).unwrap();
    }));

    assert_eq!(refused.reason, RefusalReason::LostARace);
    assert!(
        std::fs::symlink_metadata(root.join("notes.md"))
            .unwrap()
            .is_symlink(),
        "their link is back at the path"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "not yours\n");
    assert_eq!(staging_left(&root), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_displaced_original_somebody_else_removes_is_reported_rather_than_invented() {
    // The exchange put this save's bytes at the path and something then unlinked the
    // version it displaced, before it could be kept. The save did happen; claiming a
    // recoverable copy that is not there would be the lie.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");

    let saved = written(interleaved(
        &root,
        "notes.md",
        &base,
        "mine\n",
        Interleave {
            before_exchange: None,
            after_exchange: Some(&|| {
                for entry in std::fs::read_dir(&root).unwrap() {
                    let entry = entry.unwrap();
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(super::place::STAGING_PREFIX)
                    {
                        std::fs::remove_file(entry.path()).unwrap();
                    }
                }
            }),
        },
    ));

    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "mine\n"
    );
    assert!(saved.preserved.is_none());
    assert!(saved
        .preserve_note
        .unwrap()
        .contains("Something removed the version this replaced"),);
    assert!(preserved(&root).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_rollback_that_cannot_be_verified_deletes_nothing_and_names_what_it_left() {
    // Two writers, one in each gap: the first makes the exchange a conflict, the second
    // replaces the path again while the rollback is being decided. The rollback then
    // leaves a third party's file at the staging name, which is exactly the state a
    // tidy-up would destroy. It is kept, and the refusal says where.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    let file = root.join("notes.md");

    let refused = refusal(interleaved(
        &root,
        "notes.md",
        &base,
        "mine\n",
        Interleave {
            before_exchange: Some(&|| std::fs::write(&file, "theirs\n").unwrap()),
            after_exchange: Some(&|| {
                let third = root.join("third");
                std::fs::write(&third, "a third writer\n").unwrap();
                std::fs::rename(&third, &file).unwrap();
            }),
        },
    ));

    assert_eq!(refused.reason, RefusalReason::NeedsRecovery);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "theirs\n",
        "the rollback did put their file back"
    );
    let kept = refused.preserved;
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].foreign);
    assert_eq!(
        std::fs::read_to_string(&kept[0].path).unwrap(),
        "a third writer\n",
        "the third writer's bytes were not swept up with this save's"
    );
    assert_eq!(
        staging_left(&root),
        Vec::<String>::new(),
        "and nothing is left under a name only this module understands"
    );
}

#[cfg(unix)]
#[test]
fn a_kept_original_cannot_be_used_to_name_a_file_in_the_checkout() {
    let (_dir, root) = worktree();
    assert!(discard_preserved(&root, "../../notes.md").is_err());
    assert!(discard_preserved(&root, "a/b").is_err());
    assert!(read_preserved(&root, "..").is_err());
    assert!(
        !discard_preserved(&root, "nothing-by-that-name").unwrap(),
        "a name in the store that is not there is not an error"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "first\nsecond\n"
    );
}

#[cfg(unix)]
#[test]
fn an_ancestor_swapped_for_a_link_in_the_gap_cannot_redirect_the_write() {
    // The other half of the old hole: the check and the rename each resolved the path
    // again, so a directory component swapped for a link between them sent the write
    // somewhere else. The handle is pinned to the real directory's inode, so the swap
    // changes nothing about where these bytes go.
    let (_dir, root) = worktree();
    let real = root.join("src");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("main.rs"), "fn main() {}\n").unwrap();

    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("main.rs"), "not yours\n").unwrap();

    let base = base_of(&root, "src/main.rs");
    let swap = || {
        std::fs::rename(&real, root.join("src.real")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), &real).unwrap();
    };
    let saved = written(racing(
        &root,
        "src/main.rs",
        &base,
        "fn main() { ok(); }\n",
        &swap,
    ));

    assert_eq!(
        std::fs::read_to_string(elsewhere.path().join("main.rs")).unwrap(),
        "not yours\n",
        "nothing outside the checkout was touched"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src.real/main.rs")).unwrap(),
        "fn main() { ok(); }\n",
        "the bytes went into the directory that was checked"
    );
    assert_eq!(saved.path, "src/main.rs");
}

#[cfg(unix)]
#[test]
fn the_old_rename_could_not_finish_the_save_it_had_already_validated() {
    // The same swap against the old replacement, so the test above is measured against
    // something rather than asserted on its own. The old code named the staged file and
    // the target by *path*, and both of those paths mean a different directory once an
    // ancestor is swapped: the rename cannot find what it staged, and the save it had
    // already validated does not land anywhere.
    let (_dir, root) = worktree();
    let real = root.join("src");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("main.rs"), "fn main() {}\n").unwrap();

    let elsewhere = tempfile::tempdir().unwrap();
    let base = base_of(&root, "src/main.rs");
    let legacy = std::panic::catch_unwind(|| {
        legacy_save(
            &root,
            "src/main.rs",
            &base,
            "fn main() { ok(); }\n",
            &|| {
                std::fs::rename(&real, root.join("src.real")).unwrap();
                std::os::unix::fs::symlink(elsewhere.path(), &real).unwrap();
            },
        )
    });
    assert!(legacy.is_err(), "the rename could not complete");
    assert_eq!(
        std::fs::read_to_string(root.join("src.real/main.rs")).unwrap(),
        "fn main() {}\n",
        "and the file it validated never got the bytes"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_component_that_is_a_link_is_refused_before_anything_is_staged() {
    // The same shape, but the link is already there when the save starts - and it points
    // *inside* the checkout, so the path check has nothing to say about it. Components
    // are opened with `O_NOFOLLOW`, so it is a refusal naming the component, not a write
    // into a file the operator was not looking at.
    let (_dir, root) = worktree();
    let real = root.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("main.rs"), "fn main() {}\n").unwrap();
    std::os::unix::fs::symlink(&real, root.join("src")).unwrap();

    let base = base_of(&root, "notes.md");
    let refused = refusal(
        save(
            &root,
            "src/main.rs",
            &base,
            &super::hash_of(b"mine\n"),
            "mine\n",
        )
        .unwrap(),
    );
    assert_eq!(refused.reason, RefusalReason::NotAPlainFile);
    assert!(
        refused.message.contains("src") && refused.message.contains("symbolic link"),
        "{}",
        refused.message
    );
    assert_eq!(
        std::fs::read_to_string(real.join("main.rs")).unwrap(),
        "fn main() {}\n"
    );

    // And the editor does not offer it for editing either, so the window never shows a
    // file it would refuse to write.
    let through = opened(&root, "src/main.rs");
    assert!(!through.editable);
    assert!(
        through.reason.unwrap().contains("symbolic link"),
        "a file it will not save is not a file it opens"
    );
    // The real path is editable, because there is nothing wrong with the file.
    assert!(opened(&root, "real/main.rs").editable);
}

#[cfg(unix)]
#[test]
fn the_file_a_save_replaced_is_kept_in_the_repositorys_own_git_directory() {
    // Kept, not deleted, and out of `git status`: the store is inside `.git`, which git
    // neither reports nor walks, so keeping every replaced version costs the operator no
    // untracked files and nothing has to be written into `.git/info/exclude`.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");

    let saved = written(confirm(&root, "notes.md", &base, "mine\n"));
    let kept = saved.preserved.expect("kept");
    assert!(!kept.in_checkout);
    assert!(
        kept.path
            .contains(&format!(".git/{}", super::place::RECOVERY_DIRECTORY)),
        "{}",
        kept.path
    );
    assert_eq!(
        std::fs::read_to_string(&kept.path).unwrap(),
        "first\nsecond\n"
    );
    assert_eq!(kept.hash, base.hash);
}

#[cfg(unix)]
#[test]
fn with_nowhere_to_keep_it_the_original_stays_beside_the_file_and_says_so() {
    // No `.git`, so no store. The displaced file is still never deleted - it is kept as
    // a sibling under a name that says what it is, and the response admits it is in the
    // checkout where `git status` will show it.
    let (_dir, root) = bare_worktree();
    let base = base_of(&root, "notes.md");

    let saved = written(confirm(&root, "notes.md", &base, "mine\n"));
    let kept = saved.preserved.expect("kept");
    assert!(kept.in_checkout, "{}", kept.path);
    assert!(
        kept.path.contains(super::place::BESIDE_PREFIX),
        "{}",
        kept.path
    );
    assert_eq!(
        std::fs::read_to_string(&kept.path).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn a_preview_of_a_file_that_is_gone_says_that_rather_than_blaming_its_encoding() {
    // Missing and unreadable are different states; one message for both sent the
    // operator looking at the file's encoding for a file that was not there.
    let (_dir, root) = worktree();
    let base = base_of(&root, "notes.md");
    std::fs::remove_file(root.join("notes.md")).unwrap();

    let gone = preview(&root, "notes.md", &base, "mine\n").unwrap();
    assert!(gone.stale);
    assert!(gone.disk.is_none());
    assert!(
        gone.notices
            .iter()
            .any(|notice| notice.contains("not on disk any more")),
        "{:?}",
        gone.notices
    );

    std::fs::write(root.join("notes.md"), [0xff, 0xfe]).unwrap();
    let binary = preview(&root, "notes.md", &base, "mine\n").unwrap();
    assert!(binary
        .notices
        .iter()
        .any(|notice| notice.contains("not UTF-8")));
    assert!(binary.disk_text.is_none());
}
