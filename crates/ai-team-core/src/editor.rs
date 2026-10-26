//! Exact, checked saves for the operator editor (SPEC §11.4).
//!
//! The editor writes into the live checkout an agent may be working in, so a save is not
//! "put these bytes there". It is: these exact bytes, into this exact path inside this
//! exact checkout, only while the file is still the one that was opened - and if it is
//! not, refuse and keep both versions.
//!
//! The replacement is an **atomic exchange of two directory entries**, through a
//! directory handle opened one no-follow component at a time. `place.rs` says how and
//! why. What that buys, and what it does not:
//!
//! - **No write is destroyed by a save.** The exchange does not unlink the file it
//!   displaces: it comes out alive, with its inode, and this module gives it a durable
//!   name in the checkout's recovery store. A write another process made in the gap, and
//!   a write it makes afterwards through a descriptor it still holds, land in a file that
//!   still exists and is named in the response.
//! - **A conflict is refused and rolled back, not patched over.** When the displaced file
//!   is not the one that was checked, the exchange is run again to put it back and the
//!   save is refused with both versions readable. Nothing on a refusal path deletes
//!   another writer's bytes - not even to tidy up after itself.
//! - **There is no compare-and-swap on content here, and none is claimed.** POSIX does
//!   not offer one. The conflict is detected *after* the swap, not prevented before it.
//!   The guarantee is that the losing side survives, not that there is no race.
//! - **It never writes through a symbolic link,** at the leaf or at any directory
//!   component below the checkout. Components are opened with `O_NOFOLLOW`, so an
//!   ancestor swapped for a link cannot redirect the write, and the handle that results
//!   is pinned to the directory's inode rather than to its path.
//! - **A filesystem with no atomic exchange gets a refusal,** not a rename. Falling back
//!   would be the old hole under a newer name.
//! - **The recovery store grows and nothing here prunes it.** Every save leaves the
//!   version it replaced in `<git dir>/ai-team-editor-recovery`, because no read can tell
//!   whether another process is still writing into it. [`preserved`] lists them and
//!   [`discard_preserved`] removes one, and both are the operator's to call.
//! - **Nothing here is a security boundary.** It is the same operator tool the toolbar
//!   says it is; it protects against the accident, not the adversary. The limits it
//!   cannot close are listed on [`save`].

mod linediff;
#[cfg(unix)]
mod place;
#[cfg(test)]
mod tests;

use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub use linediff::{Change as FileChange, Hunk as ChangeHunk, Line as ChangeLine, LineKind};

/// The largest file the editor will open or write.
///
/// Not a judgement about the file: a buffer this size is already past what the window can
/// show usefully, and reading it is a copy in the server, a copy in the JSON and a copy in
/// the browser. Refused with the size, rather than opened and quietly truncated.
pub const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;

/// How much of a refused file's own text comes back for comparison.
const DISK_TEXT_BYTES: usize = 256 * 1024;

/// Hunks in one preview. Enough to read a hand edit; a wholesale rewrite says it was cut.
const MAX_HUNKS: usize = 200;

/// What a file was when the editor opened it.
///
/// The hash is the authority - identity is a cheap pre-check that catches a replaced file
/// whose content happens to match. Both are echoed back by the window at save time rather
/// than remembered here: the server holds no per-window session, and a revision the client
/// cannot produce is a revision that stops working the moment the window is reloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRevision {
    /// SHA-256 of the exact bytes, hex.
    pub hash: String,
    pub bytes: u64,
    /// Filesystem identity, 0 where the platform does not report it.
    pub device: u64,
    pub inode: u64,
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eol {
    /// No line ending at all: one line, or an empty file.
    None,
    Lf,
    Crlf,
    /// Both, which no editor can round-trip without choosing. The preview says so.
    Mixed,
}

impl Eol {
    /// The separator a buffer must be joined with to reproduce this file's bytes, or
    /// `None` when the file does not agree with itself and there is nothing to preserve.
    pub fn separator(self) -> Option<&'static str> {
        match self {
            Eol::Lf | Eol::None => Some("\n"),
            Eol::Crlf => Some("\r\n"),
            Eol::Mixed => None,
        }
    }

    fn of(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        match (crlf, lf) {
            (0, 0) => Eol::None,
            (0, _) => Eol::Lf,
            (_, 0) => Eol::Crlf,
            _ => Eol::Mixed,
        }
    }
}

/// A file, as the editor opened it.
#[derive(Debug, Clone, Serialize)]
pub struct OpenedFile {
    pub path: String,
    pub text: String,
    /// False when the editor refuses to edit it. `reason` always says why.
    pub editable: bool,
    pub reason: Option<String>,
    /// Absent exactly when the file was not read, so a buffer can never claim a base it
    /// does not have.
    pub revision: Option<FileRevision>,
    pub eol: Eol,
    /// The separator the buffer has to be rejoined with, or null for a mixed file.
    pub line_separator: Option<&'static str>,
    pub encoding: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundedText {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposed {
    pub hash: String,
    pub bytes: u64,
    pub eol: Eol,
}

/// Exactly what a save would write, and what it would replace.
#[derive(Debug, Clone, Serialize)]
pub struct SavePreview {
    pub checkout: String,
    pub path: String,
    /// The revision the window opened, echoed so the confirmation is about one state.
    pub base: FileRevision,
    /// What is on disk as the preview is taken, absent when the path is gone.
    pub disk: Option<FileRevision>,
    /// True when the disk is no longer the file that was opened. A confirm will refuse.
    pub stale: bool,
    pub proposed: Proposed,
    /// Against the bytes on disk now - which is what "will be written over" means.
    pub change: FileChange,
    /// The disk's own text when it has moved and is still readable, for comparison.
    pub disk_text: Option<BoundedText>,
    /// Plain statements about the write: line endings, a missing final newline. Never a
    /// silent fix-up.
    pub notices: Vec<String>,
}

/// One kept original, as the recovery store lists it.
#[derive(Debug, Clone, Serialize)]
pub struct PreservedEntry {
    /// The store-relative name. This is what a discard takes, so that nothing outside the
    /// store can be named.
    pub name: String,
    /// Exactly where it is, for an operator who wants to open it outside the window.
    pub path: String,
    /// The checkout-relative path it was displaced from, as the name records it.
    pub origin: String,
    pub bytes: u64,
    /// When it was displaced, from the store entry's own timestamp.
    pub kept_at: Option<String>,
}

/// A file this displaced, kept where it can be read back.
///
/// Kept rather than deleted, always: a descriptor another process is still holding writes
/// into *this* file, and the only way those bytes stay findable is for the inode to keep
/// a name. The operator discards it, through [`discard_preserved`]; nothing here does.
#[derive(Debug, Clone, Serialize)]
pub struct PreservedOriginal {
    /// The store-relative name, which [`discard_preserved`] takes.
    pub name: String,
    /// Exactly where it is. Absolute, because the operator may want it outside the
    /// window.
    pub path: String,
    pub bytes: u64,
    /// Of the bytes as they were when it was displaced. A descriptor another process
    /// holds can write into it afterwards, and this does not follow that.
    pub hash: String,
    /// True when these are not the bytes the editor had open - another writer's work.
    pub foreign: bool,
    /// True when it had to be kept inside the checkout, where `git status` will show it.
    pub in_checkout: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Saved {
    pub path: String,
    /// What the file is now. The window rebases its buffer on this.
    pub revision: FileRevision,
    /// What it was immediately before the exchange.
    pub replaced: FileRevision,
    /// Where the file this replaced is kept. `None` only when it could not be kept, and
    /// then `preserve_note` says why.
    pub preserved: Option<PreservedOriginal>,
    pub preserve_note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The content moved since the file was opened.
    ChangedOnDisk,
    /// Same content, different file: something replaced the path.
    ReplacedOnDisk,
    /// It is not there any more.
    Missing,
    /// It is a symlink, a directory or a device - or it is reached through one.
    NotAPlainFile,
    /// The bytes do not hash to the preview that was confirmed.
    NotThePreview,
    TooLarge,
    /// Something wrote or replaced the file between the last check and the exchange. The
    /// exchange has been undone and their bytes are back at the path.
    LostARace,
    /// This filesystem has no atomic exchange of directory entries. Refused rather than
    /// written with a plain rename, which would be able to destroy another writer's work.
    NoAtomicExchange,
    /// The exchange happened and the rollback could not be verified. Nothing was deleted;
    /// `preserved` names every file this left on disk.
    NeedsRecovery,
}

#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    pub reason: RefusalReason,
    pub message: String,
    pub disk: Option<FileRevision>,
    pub disk_text: Option<BoundedText>,
    /// Files this refusal left on disk for the operator to deal with. Empty for an
    /// ordinary refusal, which changes nothing.
    pub preserved: Vec<PreservedOriginal>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SaveOutcome {
    Written(Saved),
    Refused(Refusal),
}

/// The refusal §11.4 specifies, with the spec's em dash written as the plain dash this
/// repository uses everywhere else.
pub const CHANGED_ON_DISK: &str = "Changed on disk since you opened it - reload or compare";

fn hash_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn bounded(text: &str) -> BoundedText {
    let mut end = text.len().min(DISK_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    BoundedText {
        text: text[..end].to_string(),
        truncated: end < text.len(),
    }
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn identity(_metadata: &std::fs::Metadata) -> (u64, u64) {
    // No portable identity here; the content hash is the whole check.
    (0, 0)
}

fn modified_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    let since = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    i64::try_from(since.as_millis()).ok()
}

fn revision(metadata: &std::fs::Metadata, bytes: &[u8]) -> FileRevision {
    let (device, inode) = identity(metadata);
    FileRevision {
        hash: hash_of(bytes),
        bytes: bytes.len() as u64,
        device,
        inode,
        modified_ms: modified_ms(metadata),
    }
}

/// Is this the same file, by identity, as the one that was opened?
///
/// A revision carrying no identity (a client that opened the file on another platform, or
/// a filesystem that reports none) is judged on its hash alone rather than refused.
fn same_file(base: &FileRevision, now: &FileRevision) -> bool {
    (base.device, base.inode) == (0, 0)
        || (now.device, now.inode) == (0, 0)
        || (base.device, base.inode) == (now.device, now.inode)
}

/// The path to act on, and the path exactly as the window named it.
///
/// `safe_join` answers the escape question, and to do it it canonicalises - which
/// *follows* a symlink, so the path it returns can no longer be asked whether it was
/// one. The literal join can be, and it is only formed after `safe_join` has refused
/// `..` and absolute paths, so it is never a path the guard has not already seen.
fn resolve(worktree: &Path, relative: &str) -> Result<(PathBuf, PathBuf)> {
    let resolved = crate::safe_join(worktree, relative)?;
    Ok((resolved, worktree.join(relative)))
}

/// Is the path, as named, a symbolic link? Missing is not a link.
fn is_link(literal: &Path) -> bool {
    std::fs::symlink_metadata(literal).is_ok_and(|metadata| metadata.is_symlink())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_EDIT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_EDIT_BYTES {
        return Err(Error::invalid("File exceeds the editor size limit"));
    }
    Ok(bytes)
}

/// What is at `path` right now: its revision, and its text when it is readable.
fn on_disk(path: &Path) -> Result<Option<(FileRevision, Option<String>)>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let bytes = read_bounded(path)?;
    let text = String::from_utf8(bytes.clone()).ok();
    Ok(Some((revision(&metadata, &bytes), text)))
}

fn not_editable(path: &str, reason: &str) -> OpenedFile {
    OpenedFile {
        path: path.to_string(),
        text: String::new(),
        editable: false,
        reason: Some(reason.to_string()),
        revision: None,
        eol: Eol::None,
        line_separator: None,
        encoding: "unknown",
        bytes: 0,
    }
}

/// Read a file for editing, with the fingerprint a later save is checked against.
///
/// Decoded strictly, not lossily: a lossy read followed by a save would replace every
/// undecodable byte with U+FFFD and quietly corrupt the file.
pub fn open(worktree: &Path, relative: &str) -> Result<OpenedFile> {
    let (path, literal) = resolve(worktree, relative)?;
    if is_link(&literal) {
        return Ok(not_editable(
            relative,
            "a symbolic link - open the file it points at instead",
        ));
    }
    // A link on the way is refused here as well as at save time, so the window never
    // shows a file it would then refuse to write - which is the same bug as showing one
    // file and saving into another, one directory up.
    #[cfg(unix)]
    if let Err(place::NoAnchor::ThroughALink(at)) = place::Anchor::open(worktree, relative) {
        return Ok(not_editable(
            relative,
            &format!("reached through a symbolic link ({at}) - open the real path instead"),
        ));
    }
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| Error::invalid(format!("could not open {relative}: {}", error.kind())))?;
    if metadata.is_dir() {
        return Err(Error::invalid(format!("{relative} is a directory")));
    }
    if !metadata.is_file() {
        return Ok(not_editable(relative, "not a plain file"));
    }
    if metadata.len() > MAX_EDIT_BYTES {
        return Ok(not_editable(
            relative,
            &format!(
                "{} MiB, past the {} MiB the editor will open",
                metadata.len() / (1024 * 1024),
                MAX_EDIT_BYTES / (1024 * 1024)
            ),
        ));
    }

    let bytes = read_bounded(&path)?;
    let revision = revision(&metadata, &bytes);
    let size = revision.bytes;
    match String::from_utf8(bytes) {
        Ok(text) => {
            let eol = Eol::of(&text);
            Ok(OpenedFile {
                path: relative.to_string(),
                text,
                editable: true,
                reason: None,
                revision: Some(revision),
                eol,
                line_separator: eol.separator(),
                encoding: "utf-8",
                bytes: size,
            })
        }
        Err(_) => Ok(not_editable(relative, "not UTF-8 text")),
    }
}

/// What `present` means: the path is there. `readable` is `Some` when it is UTF-8.
fn notices(proposed: &str, present: bool, readable: Option<&str>, base_eol: Eol) -> Vec<String> {
    let mut notices = Vec::new();
    let eol = Eol::of(proposed);
    if base_eol == Eol::Mixed {
        notices.push(
            "This file has mixed line endings. The lines below are exactly what will be written."
                .into(),
        );
    } else if eol != base_eol && !matches!((base_eol, eol), (Eol::None, _) | (_, Eol::None)) {
        notices.push(format!(
            "Line endings change from {base_eol:?} to {eol:?} across the whole file."
        ));
    }
    if !proposed.is_empty() && !proposed.ends_with('\n') {
        notices.push("No newline at end of file.".into());
    }
    if !present {
        notices.push("That path is not on disk any more; this save will be refused.".into());
    } else if readable.is_none() {
        notices.push(
            "The bytes on disk are not UTF-8, so there is nothing to compare them with.".into(),
        );
    }
    notices
}

/// Exactly what confirming this save would write, and over what.
pub fn preview(
    worktree: &Path,
    relative: &str,
    base: &FileRevision,
    text: &str,
) -> Result<SavePreview> {
    if text.len() as u64 > MAX_EDIT_BYTES {
        return Err(Error::invalid(
            "Proposed content exceeds the editor size limit",
        ));
    }
    let (path, _) = resolve(worktree, relative)?;
    let current = on_disk(&path)?;
    let disk_revision = current.as_ref().map(|(revision, _)| revision.clone());
    let disk_text = current.as_ref().and_then(|(_, text)| text.clone());
    let stale = match &disk_revision {
        None => true,
        Some(now) => now.hash != base.hash || !same_file(base, now),
    };

    // Against what is there now, because that is what "will be written over" means. With
    // nothing readable there, the base is the only honest left-hand side.
    let left = disk_text.as_deref().unwrap_or("");
    let change = linediff::diff(left, text, MAX_HUNKS);
    let notices = notices(
        text,
        disk_revision.is_some(),
        disk_text.as_deref(),
        Eol::of(left),
    );

    Ok(SavePreview {
        checkout: worktree.to_string_lossy().into_owned(),
        path: relative.to_string(),
        base: base.clone(),
        disk: disk_revision,
        stale,
        proposed: Proposed {
            hash: hash_of(text.as_bytes()),
            bytes: text.len() as u64,
            eol: Eol::of(text),
        },
        change,
        disk_text: disk_text.as_deref().map(bounded),
        notices,
    })
}

fn refuse(
    reason: RefusalReason,
    message: &str,
    disk: Option<(FileRevision, Option<String>)>,
) -> SaveOutcome {
    let (revision, text) = match disk {
        Some((revision, text)) => (Some(revision), text),
        None => (None, None),
    };
    SaveOutcome::Refused(Refusal {
        reason,
        message: message.to_string(),
        disk: revision,
        disk_text: text.as_deref().map(bounded),
        preserved: Vec::new(),
    })
}

/// Is this request even a save? Size, and the bytes matching their own preview.
fn refuse_request(proposed_hash: &str, text: &str) -> Option<SaveOutcome> {
    if text.len() as u64 > MAX_EDIT_BYTES {
        return Some(refuse(
            RefusalReason::TooLarge,
            &format!(
                "{} MiB is past the {} MiB the editor will write",
                text.len() / (1024 * 1024),
                MAX_EDIT_BYTES / (1024 * 1024)
            ),
            None,
        ));
    }
    (hash_of(text.as_bytes()) != proposed_hash).then(|| {
        refuse(
            RefusalReason::NotThePreview,
            "These are not the bytes that were previewed - preview the save again",
            None,
        )
    })
}

/// Said when the filesystem will not exchange two directory entries atomically.
const NO_EXCHANGE: &str = "This filesystem cannot swap two files in one step, so a save here could destroy another program's work - refused rather than written";

const MISSING: &str = "That file is no longer on disk - save it somewhere else or reopen it";

const LOST_A_RACE: &str =
    "Something wrote that file while this save was being prepared - reload or compare";

/// Write `text`, but only over the file that was opened.
///
/// `proposed_hash` is the preview the operator confirmed. It is checked against the bytes
/// in this request so that a confirmation cannot be replayed against a different body -
/// the preview and the write are one decision, not two.
///
/// # What this still cannot do
///
/// Stated here rather than implied, because an unstated limit is the one somebody relies
/// on:
///
/// - **It does not prevent a concurrent write, it survives one.** The conflict is found
///   after the exchange, by reading the file the exchange displaced. A write that lands
///   in the gap makes the save refuse and roll back; it is never silently absorbed and
///   never deleted, but it is also not stopped. POSIX has no compare-and-swap on a file's
///   content, and an advisory lock binds only processes that ask for one - `vim`,
///   `rustfmt` and an agent's `write` tool do not.
/// - **It reads the displaced file once.** A write that lands after that read goes into
///   the preserved original, which keeps its name and its inode, so it is readable at the
///   path the response names. It is not in the saved file.
/// - **A directory handle follows its directory.** A component swapped for a symbolic
///   link after the walk cannot redirect anything, because the handle is pinned to the
///   inode. A component *moved* out of the checkout takes the handle with it; the
///   directory is still the one the path check approved, but it is no longer where it
///   was.
/// - **Nothing here is a security boundary** (rule 4). A process that wants to corrupt
///   the checkout can, through its own `bash`.
pub fn save(
    worktree: &Path,
    relative: &str,
    base: &FileRevision,
    proposed_hash: &str,
    text: &str,
) -> Result<SaveOutcome> {
    save_checked(
        worktree,
        relative,
        base,
        proposed_hash,
        text,
        &Interleave::default(),
    )
}

/// Where another writer gets to act, for the tests that put one there.
///
/// A race nobody can schedule is a race nobody can test, and the two gaps below are the
/// whole subject of this module. `save` passes neither.
#[derive(Default)]
struct Interleave<'a> {
    /// After the last check, before the exchange.
    before_exchange: Option<&'a dyn Fn()>,
    /// After the exchange, before the file it displaced is read back.
    after_exchange: Option<&'a dyn Fn()>,
}

fn save_checked(
    worktree: &Path,
    relative: &str,
    base: &FileRevision,
    proposed_hash: &str,
    text: &str,
    interleave: &Interleave<'_>,
) -> Result<SaveOutcome> {
    // The escape check first and on its own: `..` and absolute paths are refused before
    // any handle is opened on anything.
    resolve(worktree, relative)?;

    if let Some(refusal) = refuse_request(proposed_hash, text) {
        return Ok(refusal);
    }

    exchange_save(worktree, relative, base, text, interleave)
}

#[cfg(not(unix))]
fn exchange_save(
    _worktree: &Path,
    _relative: &str,
    _base: &FileRevision,
    _text: &str,
    _interleave: &Interleave<'_>,
) -> Result<SaveOutcome> {
    // No `*at` calls and no atomic exchange to reach them with. Refusing is the whole
    // policy: a plain rename here would be able to destroy another writer's bytes.
    Ok(refuse(RefusalReason::NoAtomicExchange, NO_EXCHANGE, None))
}

#[cfg(unix)]
fn anchor_refusal(error: place::NoAnchor) -> Result<SaveOutcome> {
    match error {
        place::NoAnchor::ThroughALink(at) => Ok(refuse(
            RefusalReason::NotAPlainFile,
            &format!("{at} is a symbolic link - the editor will not write through it"),
            None,
        )),
        place::NoAnchor::NoDirectory(at) => Ok(refuse(
            RefusalReason::Missing,
            &format!("{at} is not a directory in this checkout any more"),
            None,
        )),
        place::NoAnchor::Io(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn exchange_save(
    worktree: &Path,
    relative: &str,
    base: &FileRevision,
    text: &str,
    interleave: &Interleave<'_>,
) -> Result<SaveOutcome> {
    use place::{AtName, Exchanged};

    let anchor = match place::Anchor::open(worktree, relative) {
        Ok(anchor) => anchor,
        Err(error) => return anchor_refusal(error),
    };

    let current = match read_entry(anchor.read()?) {
        Entry::File(file) => file,
        Entry::Refused(refusal) => return Ok(refusal),
    };
    let seen = disk_revision(&current);
    if seen.hash != base.hash {
        return Ok(refuse(
            RefusalReason::ChangedOnDisk,
            CHANGED_ON_DISK,
            Some(as_disk(&current)),
        ));
    }
    if !same_file(base, &seen) {
        return Ok(refuse(
            RefusalReason::ReplacedOnDisk,
            "Replaced on disk since you opened it - reload or compare",
            Some(as_disk(&current)),
        ));
    }

    let mut staged = anchor.stage(text, current.mode)?;

    // The last look before the swap. It does not make the swap conditional - nothing can
    // - it is what tells the code after the swap whether the file it displaced is the
    // file that was checked.
    let before = match read_entry(anchor.read()?) {
        Entry::File(file) => file,
        Entry::Refused(refusal) => return Ok(refusal),
    };
    if !before.is(&current) {
        return Ok(refuse(
            RefusalReason::LostARace,
            LOST_A_RACE,
            Some(as_disk(&before)),
        ));
    }

    if let Some(between) = interleave.before_exchange {
        between();
    }

    match staged.exchange()? {
        Exchanged::Done => {}
        Exchanged::Unsupported => {
            return Ok(refuse(
                RefusalReason::NoAtomicExchange,
                NO_EXCHANGE,
                Some(as_disk(&before)),
            ))
        }
        Exchanged::Vanished => {
            return Ok(refuse(
                RefusalReason::Missing,
                "That file was removed while the save was being prepared",
                None,
            ))
        }
    }

    if let Some(after) = interleave.after_exchange {
        after();
    }

    // The swap has happened. The new bytes are at the path and the file they displaced is
    // alive at the staging name - that is the property the exchange was chosen for, and
    // from here nothing may throw that file away.
    let displaced = staged.read()?;
    // Written is the shape of the file the exchange put at the path: the staged inode,
    // carrying these bytes. Read back from the path it would be a guess, because the
    // path is a name anybody can move.
    let revision = FileRevision {
        hash: hash_of(text.as_bytes()),
        bytes: text.len() as u64,
        device: staged.device,
        inode: staged.inode,
        modified_ms: staged.modified_ms,
    };

    match &displaced {
        // Nobody wrote in the gap, as far as a read taken straight after the swap can
        // tell. Keep the displaced file anyway: the one thing that read cannot see is a
        // descriptor another process is still holding, and a descriptor keeps writing
        // into the inode, not into the name.
        AtName::File(file) if file.is(&before) => {
            anchor.sync();
            let kept = keep(&mut staged, worktree, relative, Some(&before), false);
            Ok(SaveOutcome::Written(Saved {
                path: relative.to_string(),
                revision,
                replaced: seen,
                preserved: kept.0,
                preserve_note: kept.1,
            }))
        }
        // The exchange succeeded and then something removed the file it displaced. This
        // save did not destroy it and cannot produce it; saying so is all that is left.
        AtName::Missing => {
            anchor.sync();
            Ok(SaveOutcome::Written(Saved {
                path: relative.to_string(),
                revision,
                replaced: seen,
                preserved: None,
                preserve_note: Some(
                    "Something removed the version this replaced before it could be kept, so \
                     there is nothing to recover it from."
                        .into(),
                ),
            }))
        }
        _ => rolled_back(&mut staged, &anchor, worktree, relative, displaced),
    }
}

/// Undo an exchange that displaced the wrong file, and refuse.
///
/// Both entries exist throughout, so nothing is destroyed by swapping them back; what the
/// second exchange cannot promise is the *arrangement*, because a third writer can act
/// between the two. So the result is verified, and when it cannot be, this keeps every
/// file it can see and names them rather than guessing which to remove.
#[cfg(unix)]
fn rolled_back(
    staged: &mut place::Staged<'_>,
    anchor: &place::Anchor,
    worktree: &Path,
    relative: &str,
    displaced: place::AtName,
) -> Result<SaveOutcome> {
    use place::{AtName, Exchanged};

    let theirs = match &displaced {
        AtName::File(file) => Some(file.clone()),
        _ => None,
    };
    let ours = (staged.device, staged.inode);

    let undone = matches!(staged.exchange()?, Exchanged::Done);
    // Two things have to be true before this is reportable as a rollback: the staging
    // name is holding *this save's* bytes again, and the path is not. The first is what
    // makes the drop guard safe - a third writer acting between the two exchanges can
    // leave somebody else's file at the staging name, and removing that would be the
    // loss this whole module is about.
    let restored = undone
        && is(&staged.read()?, Some(ours))
        && match theirs.as_ref() {
            Some(file) => is(&anchor.read()?, Some((file.device, file.inode))),
            // Whatever the exchange displaced was not a plain file this could read - a
            // link swapped onto the path, say. There is no identity to match, so the
            // test is only that this save's bytes are no longer the thing at the path.
            None => !is(&anchor.read()?, Some(ours)),
        };

    if restored {
        staged.discard_verified_proposal();
        // Their bytes are back where they were; the bytes this save wanted are at the
        // staging name and in the operator's buffer, so the guard may remove those.
        return Ok(refuse(
            RefusalReason::LostARace,
            LOST_A_RACE,
            theirs.as_ref().map(as_disk),
        ));
    }

    // Fail closed. This cannot say which file is which any more, so it removes none of
    // them: whatever is at the staging name is kept and the refusal names it.
    staged.keep();
    let (kept, _) = keep(staged, worktree, relative, None, true);
    Ok(SaveOutcome::Refused(Refusal {
        reason: RefusalReason::NeedsRecovery,
        message: format!(
            "{relative} was swapped and the swap could not be undone, so this save was not \
             completed and nothing was deleted. Every version is still on disk - compare \
             them before you edit that file again"
        ),
        disk: theirs.as_ref().map(disk_revision),
        disk_text: theirs
            .as_ref()
            .and_then(|file| String::from_utf8(file.bytes.clone()).ok())
            .as_deref()
            .map(bounded),
        preserved: kept.into_iter().collect(),
    }))
}

/// Is this entry the file with that identity? Anything this could not read is not.
#[cfg(unix)]
fn is(found: &place::AtName, identity: Option<(u64, u64)>) -> bool {
    match (found, identity) {
        (place::AtName::File(file), Some(identity)) => (file.device, file.inode) == identity,
        _ => false,
    }
}

/// Move whatever is at the staging name somewhere durable and describe where it went.
///
/// `known` is the entry when it has already been read, so a successful save does not read
/// the displaced file a second time. With `None` this reads it, and reports nothing it
/// could not read rather than guessing.
#[cfg(unix)]
fn keep(
    staged: &mut place::Staged<'_>,
    worktree: &Path,
    relative: &str,
    known: Option<&place::DiskFile>,
    foreign: bool,
) -> (Option<PreservedOriginal>, Option<String>) {
    let read;
    let mut file = known;
    if file.is_none() {
        read = staged.read().ok();
        if let Some(place::AtName::File(found)) = &read {
            file = Some(found);
        }
    }
    let (bytes, hash) = file.map_or((0, String::new()), |file| {
        (file.bytes.len() as u64, hash_of(&file.bytes))
    });

    let kept = staged.preserve(worktree, relative);
    let path = kept.path().to_path_buf();
    let note = matches!(kept, place::Kept::StillStaged { .. }).then(|| {
        format!(
            "The replaced version could not be moved into the recovery store; it is at {} \
             and nothing will remove it.",
            path.display()
        )
    });
    (
        Some(PreservedOriginal {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.display().to_string(),
            bytes,
            hash,
            foreign,
            in_checkout: kept.in_checkout(),
        }),
        note,
    )
}

/// Turn what the anchored read found into the file, or into the refusal that says why
/// there is not one.
#[cfg(unix)]
fn read_entry(found: place::AtName) -> Entry {
    match found {
        place::AtName::File(file) => Entry::File(file),
        place::AtName::Missing => Entry::Refused(refuse(RefusalReason::Missing, MISSING, None)),
        place::AtName::NotAPlainFile(why) => {
            Entry::Refused(refuse(RefusalReason::NotAPlainFile, why, None))
        }
        place::AtName::TooLarge(bytes) => Entry::Refused(refuse(
            RefusalReason::TooLarge,
            &format!(
                "That file is {} MiB on disk, past the {} MiB the editor will handle",
                bytes / (1024 * 1024),
                MAX_EDIT_BYTES / (1024 * 1024)
            ),
            None,
        )),
    }
}

/// The file, or the refusal that says why there is not one.
#[cfg(unix)]
#[derive(Debug)]
enum Entry {
    File(place::DiskFile),
    Refused(SaveOutcome),
}

#[cfg(unix)]
fn disk_revision(file: &place::DiskFile) -> FileRevision {
    FileRevision {
        hash: hash_of(&file.bytes),
        bytes: file.bytes.len() as u64,
        device: file.device,
        inode: file.inode,
        modified_ms: file.modified_ms,
    }
}

/// The pair a refusal carries: what is there, and its text when it is text.
#[cfg(unix)]
fn as_disk(file: &place::DiskFile) -> (FileRevision, Option<String>) {
    (
        disk_revision(file),
        String::from_utf8(file.bytes.clone()).ok(),
    )
}

/// Every version this checkout's editor has displaced, newest first.
///
/// Nothing prunes this. A kept original is the only copy of whatever another process
/// wrote into that inode, and deciding that nobody wants it is the operator's call.
pub fn preserved(worktree: &Path) -> Result<Vec<PreservedEntry>> {
    #[cfg(unix)]
    {
        Ok(place::listed(worktree)?)
    }
    #[cfg(not(unix))]
    {
        let _ = worktree;
        Ok(Vec::new())
    }
}

/// Remove one kept original, by the name the listing gave it.
///
/// `Ok(false)` when there is no such entry. The name has to be a single plain component,
/// so this route cannot be pointed at a file in the checkout.
pub fn discard_preserved(worktree: &Path, name: &str) -> Result<bool> {
    #[cfg(unix)]
    {
        let Some(path) = kept_path(worktree, name)? else {
            return Ok(false);
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (worktree, name);
        Ok(false)
    }
}

/// One kept original's path. An error for a name that is not one; `None` for a checkout
/// with no store - which is nothing kept, not a bad request.
#[cfg(unix)]
fn kept_path(worktree: &Path, name: &str) -> Result<Option<PathBuf>> {
    let Some(name) = place::plain_name(name) else {
        return Err(Error::invalid("that is not a kept file's name"));
    };
    Ok(place::store_path(worktree).map(|store| store.join(name)))
}

/// The text of one kept original, bounded like every other disk comparison here.
pub fn read_preserved(worktree: &Path, name: &str) -> Result<Option<BoundedText>> {
    #[cfg(unix)]
    {
        let Some(path) = kept_path(worktree, name)? else {
            return Ok(None);
        };
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(String::from_utf8(bytes).map_or_else(
                |_| BoundedText {
                    text: String::new(),
                    truncated: true,
                },
                |text| bounded(&text),
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (worktree, name);
        Ok(None)
    }
}

/// The committed version of a file, for the change markers in the gutter.
///
/// `None` when the path is not in HEAD at all - a new file is not a failure, and the
/// window draws every line as added rather than drawing nothing.
pub async fn head_text(worktree: &Path, relative: &str) -> Result<Option<String>> {
    // Refuse the path before handing it to git, so a `..` never reaches a command line.
    crate::safe_join(worktree, relative)?;
    crate::neighbours::git::head_blob(worktree, relative).await
}
