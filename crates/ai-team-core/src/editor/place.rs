//! Putting new bytes where the old ones were, without unlinking anybody else's work.
//!
//! A save used to end in `rename(temporary, path)`. That has two holes, and both are the
//! kind that only ever bite on a machine somebody is actually working on:
//!
//! - **`rename` unlinks what it replaces.** A write another process made after the last
//!   check, or one it makes afterwards through a descriptor it is still holding, lands in
//!   an inode with no name left on it and is gone the moment that descriptor closes.
//! - **Both the check and the rename resolve the path again,** from the root, following
//!   every symbolic link on the way. Swap a directory component for a link between them
//!   and the file that was checked is not the file that is written.
//!
//! What replaces it:
//!
//! - **Anchored handles.** Every component below the checkout is opened with
//!   `O_NOFOLLOW | O_DIRECTORY`, so a link anywhere along the way is refused rather than
//!   followed, and the handle that comes out is pinned to that directory's inode. Every
//!   later check, the staging and the swap itself go through that handle with `*at`
//!   calls, so there is no second path resolution left to race.
//! - **An exchange, not a replacement.** `renameat2(RENAME_EXCHANGE)` on Linux and
//!   `renameatx_np(RENAME_SWAP)` on macOS swap two directory entries in one step. The
//!   file that was at the path is *not* unlinked - it comes out alive at the staging
//!   name, with its inode, so a descriptor another process still holds keeps writing into
//!   a file that still has a name, and this module gives it a durable one.
//!
//! **This is not compare-and-swap on content, and nothing here pretends it is.** POSIX
//! offers no such operation and no filesystem here provides one. What the exchange buys
//! is that the losing side is *kept* rather than destroyed: a write that lands in the gap
//! is detected after the swap, the save is rolled back and refused, and the other
//! writer's bytes are still readable. The guarantee is no loss, not no race.
//!
//! On a filesystem with no atomic exchange this refuses. A plain rename would be the old
//! hole with a newer name on it.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{
    openat, renameat_with, statat, unlinkat, AtFlags, FileType, Mode, OFlags, RenameFlags,
};
use rustix::io::Errno;

use super::MAX_EDIT_BYTES;

/// Opening a directory to act in, never to read from.
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);

/// The prefix every staged file carries, so one left by a crash is recognisable.
pub(super) const STAGING_PREFIX: &str = ".ai-team-save-";

/// The prefix a displaced original gets when it has to stay in the checkout.
pub(super) const BESIDE_PREFIX: &str = ".ai-team-replaced-";

/// Where displaced originals live inside the repository's own git directory.
pub(super) const RECOVERY_DIRECTORY: &str = "ai-team-editor-recovery";

/// Names are tried in order until one is free; a collision needs a different clock tick.
const ATTEMPTS: u32 = 64;

/// What a `*at` read, following no links, found at a name.
#[derive(Debug)]
pub(super) enum AtName {
    Missing,
    /// There, but not something this writes. The text is the whole operator message.
    NotAPlainFile(&'static str),
    TooLarge(u64),
    File(DiskFile),
}

/// A plain file, read through a pinned directory handle.
#[derive(Debug, Clone)]
pub(super) struct DiskFile {
    pub(super) device: u64,
    pub(super) inode: u64,
    /// Permission bits, to put back on the replacement.
    pub(super) mode: u32,
    pub(super) modified_ms: Option<i64>,
    pub(super) bytes: Vec<u8>,
}

impl DiskFile {
    pub(super) fn is(&self, other: &DiskFile) -> bool {
        (self.device, self.inode) == (other.device, other.inode) && self.bytes == other.bytes
    }
}

/// Why a path could not be anchored to real directories inside the checkout.
#[derive(Debug)]
pub(super) enum NoAnchor {
    /// A component below the checkout is a symbolic link. Following it is exactly how a
    /// write ends up somewhere the path check never approved.
    ThroughALink(String),
    /// A component is missing, or is not a directory.
    NoDirectory(String),
    Io(std::io::Error),
}

/// A directory this will act in, pinned by its inode, and the name to act on in it.
///
/// Pinned is the point. Once the handle is open, a rename of any ancestor - including an
/// ancestor swapped for a symbolic link pointing anywhere - cannot redirect a `*at` call
/// made through it. The directory can still be moved, and if somebody moves it out of the
/// checkout the handle follows the directory rather than the path; that is a real limit
/// and `editor.rs` says so.
#[derive(Debug)]
pub(super) struct Anchor {
    directory: OwnedFd,
    directory_path: PathBuf,
    name: OsString,
}

impl Anchor {
    /// Walk the relative path one component at a time, following no link below the root.
    ///
    /// The checkout root itself *is* followed: the operator named it, and on macOS it is
    /// routinely reached through `/tmp` or `/var`, which are links into `/private` (D12).
    pub(super) fn open(worktree: &Path, relative: &str) -> Result<Anchor, NoAnchor> {
        let mut parts: Vec<&OsStr> = Vec::new();
        for component in Path::new(relative).components() {
            match component {
                std::path::Component::Normal(part) => parts.push(part),
                std::path::Component::CurDir => {}
                _ => return Err(NoAnchor::NoDirectory(relative.to_string())),
            }
        }
        let Some((name, directories)) = parts.split_last() else {
            return Err(NoAnchor::NoDirectory(relative.to_string()));
        };

        let mut directory = rustix::fs::open(worktree, DIRECTORY, Mode::empty())
            .map_err(|error| NoAnchor::Io(error.into()))?;
        let mut walked = worktree.to_path_buf();
        for part in directories {
            match openat(
                &directory,
                *part,
                DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(next) => directory = next,
                Err(error) => {
                    // `O_NOFOLLOW | O_DIRECTORY` on a link answers ELOOP on one kernel
                    // and ENOTDIR on another, so the reason comes from a stat rather
                    // than from the errno.
                    let at = walked.join(part);
                    let link = matches!(
                        statat(&directory, *part, AtFlags::SYMLINK_NOFOLLOW),
                        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink
                    );
                    return Err(if link {
                        NoAnchor::ThroughALink(at.display().to_string())
                    } else if error == Errno::NOENT || error == Errno::NOTDIR {
                        NoAnchor::NoDirectory(at.display().to_string())
                    } else {
                        NoAnchor::Io(error.into())
                    });
                }
            }
            walked.push(part);
        }
        Ok(Anchor {
            directory,
            directory_path: walked,
            name: name.to_os_string(),
        })
    }

    pub(super) fn read(&self) -> std::io::Result<AtName> {
        self.read_at(&self.name)
    }

    /// Make the directory's own change durable.
    ///
    /// Best effort and deliberately ignored on failure: the exchange is already atomic,
    /// and this only decides whether a power cut a moment later remembers it.
    pub(super) fn sync(&self) {
        let _ = rustix::fs::fsync(&self.directory);
    }

    fn read_at(&self, name: &OsStr) -> std::io::Result<AtName> {
        let stat = match statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(Errno::NOENT) => return Ok(AtName::Missing),
            Err(error) => return Err(error.into()),
        };
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile => {}
            FileType::Symlink => {
                return Ok(AtName::NotAPlainFile(
                    "That path is a symbolic link - the editor will not write through it",
                ))
            }
            _ => {
                return Ok(AtName::NotAPlainFile(
                    "That path is not a plain file - the editor will not write to it",
                ))
            }
        }

        // The stat said what it is; this is what makes it true. `NOFOLLOW` refuses a
        // link that appeared since, and `NONBLOCK` means a fifo or a device that slipped
        // in cannot park the server inside an open.
        let opened = match openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(opened) => opened,
            Err(Errno::NOENT) => return Ok(AtName::Missing),
            Err(Errno::LOOP) => {
                return Ok(AtName::NotAPlainFile(
                    "That path is a symbolic link - the editor will not write through it",
                ))
            }
            Err(error) => return Err(error.into()),
        };
        let mut file = File::from(opened);
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Ok(AtName::NotAPlainFile(
                "That path is not a plain file - the editor will not write to it",
            ));
        }
        if metadata.len() > MAX_EDIT_BYTES {
            return Ok(AtName::TooLarge(metadata.len()));
        }
        // Bounded by the limit rather than by the size just read: the file can grow
        // between the two, and an unbounded read is how a log file becomes the heap.
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(MAX_EDIT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_EDIT_BYTES {
            return Ok(AtName::TooLarge(bytes.len() as u64));
        }
        Ok(AtName::File(DiskFile {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode() & 0o7777,
            modified_ms: super::modified_ms(&metadata),
            bytes,
        }))
    }

    /// The new bytes, in a sibling of the target, durable and wearing its permissions.
    ///
    /// A sibling so the exchange that follows is an exchange rather than a copy across
    /// filesystems, and so a partly written file never sits at the real path.
    pub(super) fn stage(&self, text: &str, mode: u32) -> std::io::Result<Staged<'_>> {
        for attempt in 0..ATTEMPTS {
            let name = OsString::from(unique(STAGING_PREFIX, attempt, ""));
            let opened = match openat(
                &self.directory,
                name.as_os_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(opened) => opened,
                Err(Errno::EXIST) => continue,
                Err(error) => return Err(error.into()),
            };
            let mut staged = Staged {
                anchor: self,
                name,
                holds: Holds::Ours,
                device: 0,
                inode: 0,
                modified_ms: None,
            };
            let mut file = File::from(opened);
            file.write_all(text.as_bytes())?;
            file.flush()?;
            // Durable before it is visible: a crash between the exchange and the flush
            // would otherwise leave an empty file where the work was.
            file.sync_all()?;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
            let metadata = file.metadata()?;
            staged.device = metadata.dev();
            staged.inode = metadata.ino();
            staged.modified_ms = super::modified_ms(&metadata);
            return Ok(staged);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "no free staging name in this directory",
        ))
    }
}

/// Which side of the exchange the staging name is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holds {
    /// The bytes this save wants to write. Ours to remove.
    Ours,
    /// The file that was at the path before the exchange. Never ours to remove.
    Displaced,
    /// The name is free - whatever was there has been moved somewhere durable.
    Nothing,
}

/// The staged replacement, and after an exchange the file it displaced.
///
/// The drop guard removes the entry **only** while it holds this save's own bytes. Once
/// an exchange has put another writer's file there, nothing in this type deletes it: a
/// failure path that tidied up would be the loss this module exists to prevent.
#[derive(Debug)]
pub(super) struct Staged<'a> {
    anchor: &'a Anchor,
    name: OsString,
    holds: Holds,
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) modified_ms: Option<i64>,
}

/// What the kernel did with the exchange request.
#[derive(Debug)]
pub(super) enum Exchanged {
    /// Swapped. The staging name now holds whatever was at the path.
    Done,
    /// This filesystem has no atomic exchange. Nothing moved.
    Unsupported,
    /// The path was gone before the swap. Nothing moved.
    Vanished,
}

/// Errors that say the kernel or the filesystem will not do this, rather than that this
/// attempt failed. `EPERM` is in the list because some filesystems answer renames they do
/// not implement that way, and treating it as unsupported refuses the save either way.
fn unsupported(error: Errno) -> bool {
    error == Errno::NOSYS
        || error == Errno::INVAL
        || error == Errno::NOTSUP
        || error == Errno::OPNOTSUPP
        || error == Errno::PERM
}

impl Staged<'_> {
    /// The absolute path of the staging entry. For a message the operator has to act on.
    pub(super) fn path(&self) -> PathBuf {
        self.anchor.directory_path.join(&self.name)
    }

    /// Swap the two directory entries. Called a second time, it undoes the first.
    pub(super) fn exchange(&mut self) -> std::io::Result<Exchanged> {
        match renameat_with(
            &self.anchor.directory,
            self.anchor.name.as_os_str(),
            &self.anchor.directory,
            self.name.as_os_str(),
            RenameFlags::EXCHANGE,
        ) {
            Ok(()) => {
                // A second exchange is not proof that our inode came back. In
                // particular, an I/O error during rollback verification must not
                // turn Drop into an unverified deletion.
                self.holds = Holds::Displaced;
                Ok(Exchanged::Done)
            }
            Err(Errno::NOENT) => Ok(Exchanged::Vanished),
            Err(error) if unsupported(error) => Ok(Exchanged::Unsupported),
            Err(error) => Err(error.into()),
        }
    }

    /// What is at the staging name now - after an exchange, the displaced original.
    pub(super) fn read(&self) -> std::io::Result<AtName> {
        self.anchor.read_at(&self.name)
    }

    pub(super) fn discard_verified_proposal(&mut self) {
        self.holds = Holds::Ours;
    }

    /// Stop the drop guard removing this, whatever it is holding.
    ///
    /// For the one state nothing can reason about: an exchange that happened and a
    /// rollback that could not be verified. Two files are on disk and this does not know
    /// which is which, so it keeps both and the refusal names them.
    pub(super) fn keep(&mut self) {
        self.holds = Holds::Displaced;
    }

    /// Move what is at the staging name somewhere durable, keeping its inode.
    ///
    /// A rename, never a copy: the inode is the whole point. A descriptor another process
    /// is still holding writes into *that* file, so the file has to survive with its
    /// identity rather than be reproduced byte by byte at a new one.
    pub(super) fn preserve(&mut self, worktree: &Path, relative: &str) -> Kept {
        let flat = relative.replace(['/', '\\'], "~");
        if let Some(store) = Store::open_beside(worktree, &self.anchor.directory_path) {
            for attempt in 0..ATTEMPTS {
                let name = unique("", attempt, &flat);
                match renameat_with(
                    &self.anchor.directory,
                    self.name.as_os_str(),
                    &store.directory,
                    name.as_str(),
                    RenameFlags::NOREPLACE,
                ) {
                    Ok(()) => {
                        self.holds = Holds::Nothing;
                        return Kept::At {
                            path: store.path.join(&name),
                            in_checkout: false,
                        };
                    }
                    // A name this tick already used; the next attempt picks another.
                    Err(Errno::EXIST) => {}
                    // No `NOREPLACE`, another filesystem, or a store that went away:
                    // fall back to a sibling rather than lose the file to a tidy-up.
                    Err(_) => break,
                }
            }
        }

        // No store, or the store is not on this filesystem. The original stays beside the
        // file it came from, under a name that says what it is.
        for attempt in 0..ATTEMPTS {
            let name = unique(BESIDE_PREFIX, attempt, &flat);
            match renameat_with(
                &self.anchor.directory,
                self.name.as_os_str(),
                &self.anchor.directory,
                name.as_str(),
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => {
                    self.holds = Holds::Nothing;
                    return Kept::At {
                        path: self.anchor.directory_path.join(&name),
                        in_checkout: true,
                    };
                }
                Err(Errno::EXIST) => {}
                Err(_) => break,
            }
        }

        // Nothing could be moved anywhere. It is still at the staging name and it still
        // has every byte: say where, and never remove it.
        self.keep();
        Kept::StillStaged { path: self.path() }
    }
}

/// Where a displaced original ended up.
#[derive(Debug)]
pub(super) enum Kept {
    At {
        path: PathBuf,
        in_checkout: bool,
    },
    /// It could not be moved, so it is where the exchange left it.
    StillStaged {
        path: PathBuf,
    },
}

impl Kept {
    pub(super) fn path(&self) -> &Path {
        match self {
            Kept::At { path, .. } | Kept::StillStaged { path } => path,
        }
    }

    pub(super) fn in_checkout(&self) -> bool {
        match self {
            Kept::At { in_checkout, .. } => *in_checkout,
            Kept::StillStaged { .. } => true,
        }
    }
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if self.holds == Holds::Ours {
            let _ = unlinkat(
                &self.anchor.directory,
                self.name.as_os_str(),
                AtFlags::empty(),
            );
        }
    }
}

/// The recovery store: where displaced originals are kept for one checkout.
///
/// Inside the repository's own git directory, which git neither reports nor walks, so
/// keeping every replaced version does not turn `git status` into a list of them and
/// nothing has to be written into `.git/info/exclude` to hide it.
#[derive(Debug)]
pub(super) struct Store {
    directory: OwnedFd,
    pub(super) path: PathBuf,
}

impl Store {
    /// The store for this checkout, created if it is not there yet.
    pub(super) fn open(worktree: &Path) -> Option<Store> {
        let path = directory_for(worktree)?;
        std::fs::create_dir_all(&path).ok()?;
        let directory = rustix::fs::open(&path, DIRECTORY, Mode::empty()).ok()?;
        Some(Store { directory, path })
    }

    /// The store, but only when a rename can reach it from `beside`.
    ///
    /// A rename cannot cross a filesystem, and copying instead would give the preserved
    /// file a new inode - which is precisely the thing a descriptor another process holds
    /// would not follow.
    fn open_beside(worktree: &Path, beside: &Path) -> Option<Store> {
        let store = Store::open(worktree)?;
        let here = std::fs::metadata(beside).ok()?.dev();
        let there = std::fs::metadata(&store.path).ok()?.dev();
        (here == there).then_some(store)
    }
}

/// Where this checkout's store belongs: inside whatever `.git` resolves to.
///
/// A linked worktree carries a `.git` *file* naming the directory the repository keeps
/// for it, so a lease's kept originals go with the lease rather than into the main
/// checkout. `None` for a directory that is not a checkout at all.
fn directory_for(worktree: &Path) -> Option<PathBuf> {
    let marker = worktree.join(".git");
    let metadata = std::fs::symlink_metadata(&marker).ok()?;
    if metadata.is_dir() {
        return Some(marker.join(RECOVERY_DIRECTORY));
    }
    if !metadata.is_file() {
        return None;
    }
    let pointer = std::fs::read_to_string(&marker).ok()?;
    let target = pointer
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?;
    let target = Path::new(target.trim());
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        worktree.join(target)
    };
    resolved.is_dir().then(|| resolved.join(RECOVERY_DIRECTORY))
}

/// A name nothing else in the directory has: the clock, this process, and a counter.
fn unique(prefix: &str, attempt: u32, suffix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let tail = if suffix.is_empty() {
        String::new()
    } else {
        format!("-{suffix}")
    };
    format!(
        "{prefix}{millis:013}-{}-{attempt}{tail}",
        std::process::id()
    )
}

use super::PreservedEntry;

/// Everything kept for this checkout, newest first.
pub(super) fn listed(worktree: &Path) -> std::io::Result<Vec<PreservedEntry>> {
    let Some(path) = directory_for(worktree) else {
        return Ok(Vec::new());
    };
    let reading = match std::fs::read_dir(&path) {
        Ok(reading) => reading,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut entries = Vec::new();
    for entry in reading {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(PreservedEntry {
            origin: origin_of(&name),
            path: path.join(&name).display().to_string(),
            bytes: metadata.len(),
            kept_at: super::modified_ms(&metadata).map(crate::util::rfc3339_from_ms),
            name,
        });
    }
    // Newest first, and the name carries the clock, so the name is the order.
    entries.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(entries)
}

/// The path a kept original came from, read back out of its name.
fn origin_of(name: &str) -> String {
    name.splitn(4, '-')
        .nth(3)
        .map_or_else(|| name.to_string(), |flat| flat.replace('~', "/"))
}

/// A single plain name, or nothing.
///
/// `..`, a separator or an absolute path would reach out of the store, and the name comes
/// in over an HTTP route. Kept separate from finding the store so that a bad name and a
/// checkout with nothing kept in it are different answers.
pub(super) fn plain_name(name: &str) -> Option<OsString> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(part)), None) => Some(part.to_os_string()),
        _ => None,
    }
}

/// Where this checkout keeps displaced originals, if it is a checkout at all.
pub(super) fn store_path(worktree: &Path) -> Option<PathBuf> {
    directory_for(worktree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_filesystem_exchanges_two_directory_entries_atomically() {
        // Not a test of the kernel: a test that the mechanism `editor.rs` fails closed
        // without is actually here. On a leg where it is not, every save refuses, and
        // the operator should learn that from a red test rather than from the window.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "A").unwrap();
        std::fs::write(dir.path().join("b"), "B").unwrap();

        let anchor = Anchor::open(dir.path(), "a").unwrap();
        renameat_with(
            &anchor.directory,
            "a",
            &anchor.directory,
            "b",
            RenameFlags::EXCHANGE,
        )
        .expect("RENAME_EXCHANGE / RENAME_SWAP");

        assert_eq!(std::fs::read_to_string(dir.path().join("a")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(dir.path().join("b")).unwrap(), "A");
    }

    #[test]
    fn the_guard_removes_this_saves_bytes_and_never_the_file_it_displaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "theirs").unwrap();
        let anchor = Anchor::open(dir.path(), "f").unwrap();

        // Dropped before any exchange: these are our bytes, so they go.
        let path = {
            let staged = anchor.stage("mine", 0o644).unwrap();
            staged.path()
        };
        assert!(!path.exists(), "a staged file nothing used is removed");

        // Dropped after an exchange: the staging name is holding their file now.
        let path = {
            let mut staged = anchor.stage("mine", 0o644).unwrap();
            assert!(matches!(staged.exchange().unwrap(), Exchanged::Done));
            staged.path()
        };
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theirs",
            "the displaced file survived the guard"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn a_name_that_could_reach_out_of_the_store_is_not_a_name() {
        assert!(plain_name("kept-file").is_some());
        assert!(plain_name("..").is_none());
        assert!(plain_name("a/b").is_none());
        assert!(plain_name("/etc/hosts").is_none());
        assert!(plain_name("").is_none());
    }

    #[test]
    fn a_kept_files_name_carries_the_path_it_came_from() {
        assert_eq!(
            origin_of("1760000000000-42-0-ui~src~App.tsx"),
            "ui/src/App.tsx"
        );
        assert_eq!(origin_of("1760000000000-42-0-notes.md"), "notes.md");
        // Anything that is not one of this module's names is reported as itself rather
        // than parsed into a path that was never there.
        assert_eq!(origin_of("stray"), "stray");
    }
}
