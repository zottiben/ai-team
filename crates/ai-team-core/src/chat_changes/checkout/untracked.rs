//! Hash untracked bytes and build bounded previews from that same read, without staging.
use crate::{Error, FileDiff, FileStatus, Hunk, Line, LineKind, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

const EXACT_BYTES: u64 = 32 * 1024 * 1024;
const PREVIEW_BYTES: usize = 256 * 1024;
const TOTAL_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_LINES: usize = 5_000;
const TOTAL_LINES: usize = 20_000;

#[derive(Debug, Serialize)]
pub struct Preview {
    pub file: FileDiff,
    pub fingerprint: String,
    pub reason: Option<String>,
}
fn too_large() -> Error {
    Error::invalid("Untracked files exceed the exact-review limit (10000 files / 32 MiB). Ignore build output or review it with your own Git tools.")
}
fn empty(path: &str) -> Preview {
    Preview {
        file: FileDiff {
            path: path.into(),
            old_path: None,
            status: FileStatus::Added,
            binary: false,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        },
        fingerprint: String::new(),
        reason: None,
    }
}

#[cfg(unix)]
pub(super) fn inspect(repo: &Path, paths: &[String], hash: &mut Sha256) -> Result<Vec<Preview>> {
    use rustix::fs::{open, openat, readlinkat, statat, AtFlags, FileType, Mode, OFlags};
    use std::{fs::File, os::unix::fs::PermissionsExt};
    if paths.len() > 10_000 {
        return Err(too_large());
    }
    let root = File::from(
        open(
            repo,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    let mut bytes = 0;
    let mut lines = 0;
    let mut preview_bytes = 0;
    let mut previews = Vec::new();
    for path in paths {
        super::validate_path(path)?;
        let relative = Path::new(path);
        let mut parent = root.try_clone()?;
        if let Some(dirs) = relative.parent() {
            for part in dirs.components() {
                parent = File::from(
                    openat(
                        &parent,
                        part.as_os_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)?,
                );
            }
        }
        let name = relative
            .file_name()
            .ok_or_else(|| Error::invalid("choose a file"))?;
        let meta =
            statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
        let mut file_hash = Sha256::new();
        file_hash.update(path.as_bytes());
        file_hash.update([0]);
        let mut preview = empty(path);
        match FileType::from_raw_mode(meta.st_mode) {
            FileType::Symlink => {
                file_hash.update(u64::from(meta.st_mode).to_le_bytes());
                file_hash.update(
                    readlinkat(&parent, name, Vec::new())
                        .map_err(std::io::Error::from)?
                        .as_bytes(),
                );
                preview.reason = Some(
                    "Symbolic link — target is not opened. Inspect it in your own tools.".into(),
                );
            }
            FileType::RegularFile => {
                let mut file = File::from(
                    openat(
                        &parent,
                        name,
                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)?,
                );
                let opened = file.metadata()?;
                if !opened.is_file() {
                    return Err(Error::invalid(
                        "untracked file changed type; refresh before reviewing",
                    ));
                }
                file_hash.update(u64::from(opened.permissions().mode()).to_le_bytes());
                let content = read(&mut file, &mut bytes, &mut file_hash)?;
                render(&mut preview, content, &mut lines, &mut preview_bytes);
            }
            _ => {
                return Err(Error::invalid(
                    "inspect nested repositories and special files with your own Git tools",
                ))
            }
        }
        let digest = file_hash.finalize();
        hash.update(digest);
        preview.fingerprint = format!("{digest:x}");
        previews.push(preview);
    }
    Ok(previews)
}

#[cfg(not(unix))]
pub(super) fn inspect(_repo: &Path, _paths: &[String], _hash: &mut Sha256) -> Result<Vec<Preview>> {
    Err(Error::invalid(
        "contained untracked review requires a supported Unix host",
    ))
}

fn read(file: &mut std::fs::File, bytes: &mut u64, hash: &mut Sha256) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    if file.metadata()?.len() > EXACT_BYTES - *bytes {
        return Err(too_large());
    }
    let mut content = Some(Vec::new());
    let mut buffer = [0; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        *bytes += n as u64;
        if *bytes > EXACT_BYTES {
            return Err(too_large());
        }
        hash.update(&buffer[..n]);
        if let Some(text) = &mut content {
            if text.len() + n > PREVIEW_BYTES {
                content = None;
            } else {
                text.extend_from_slice(&buffer[..n]);
            }
        }
    }
    Ok(content)
}
fn render(preview: &mut Preview, content: Option<Vec<u8>>, total: &mut usize, bytes: &mut usize) {
    let Some(content) = content else {
        preview.reason = Some(
            "File exceeds the 256 KiB inline preview limit. Inspect it in Editor before staging."
                .into(),
        );
        return;
    };
    if content.len() > TOTAL_PREVIEW_BYTES - *bytes {
        preview.reason = Some(
            "The 2 MiB total inline preview limit was reached. Inspect this file in Editor.".into(),
        );
        return;
    }
    let text = std::str::from_utf8(&content)
        .ok()
        .filter(|_| !content.contains(&0));
    let Some(text) = text else {
        preview.file.binary = true;
        preview.reason = Some("Binary or non-UTF-8 file — no inline text preview.".into());
        return;
    };
    let count = text.lines().count();
    if count > PREVIEW_LINES || count > TOTAL_LINES - *total {
        preview.reason = Some(
            "Inline line limit reached (5000 per file / 20000 total). Inspect this file in Editor."
                .into(),
        );
        return;
    }
    *total += count;
    *bytes += content.len();
    if count == 0 {
        preview.reason = Some("Empty file.".into());
        return;
    }
    preview.file.additions = count;
    preview.file.hunks.push(Hunk {
        header: format!("@@ -0,0 +1,{count} @@"),
        old_start: 0,
        new_start: 1,
        lines: text
            .lines()
            .zip(1_i64..)
            .map(|(text, new)| Line {
                kind: LineKind::Added,
                old: None,
                new: Some(new),
                text: text.into(),
            })
            .collect(),
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn omitted_and_empty_previews_never_offer_line_anchors() {
        let mut lines = 0;
        let mut bytes = 0;
        let mut preview = empty("empty");
        render(&mut preview, Some(Vec::new()), &mut lines, &mut bytes);
        assert_eq!(preview.reason.as_deref(), Some("Empty file."));
        assert!(preview.file.hunks.is_empty());
        let mut preview = empty("many-lines");
        render(
            &mut preview,
            Some(vec![b'\n'; PREVIEW_LINES + 1]),
            &mut lines,
            &mut bytes,
        );
        assert!(preview.file.hunks.is_empty());
        assert!(preview.reason.unwrap().contains("line limit"));
        let mut preview = empty("total-bytes");
        bytes = TOTAL_PREVIEW_BYTES;
        render(&mut preview, Some(b"code".to_vec()), &mut lines, &mut bytes);
        assert!(preview.file.hunks.is_empty());
        assert!(preview.reason.unwrap().contains("2 MiB"));
    }

    #[test]
    fn previews_are_bounded_contained_and_use_the_hashed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("new.txt"), "one\ntwo").unwrap();
        std::fs::write(root.join("binary"), [0, 255]).unwrap();
        std::fs::write(root.join("big"), vec![b'x'; PREVIEW_BYTES + 1]).unwrap();
        std::os::unix::fs::symlink("/not-opened", root.join("link")).unwrap();
        let paths = ["new.txt", "binary", "big", "link"].map(String::from);
        let mut hash = Sha256::new();
        let previews = inspect(&root, &paths, &mut hash).unwrap();
        assert_eq!(previews[0].file.hunks[0].lines[1].text, "two");
        assert_eq!(previews[0].file.hunks[0].lines[1].new, Some(2));
        assert!(previews[1].file.binary);
        assert!(previews[2].reason.as_ref().unwrap().contains("256 KiB"));
        assert!(previews[3]
            .reason
            .as_ref()
            .unwrap()
            .contains("Symbolic link"));
        let before = hash.finalize();
        std::fs::write(root.join("big"), vec![b'y'; PREVIEW_BYTES + 1]).unwrap();
        let mut hash = Sha256::new();
        inspect(&root, &paths, &mut hash).unwrap();
        assert_ne!(
            before,
            hash.finalize(),
            "omitted text is still fingerprinted"
        );
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside"), "must not be read").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        assert!(inspect(&root, &["escape/outside".into()], &mut Sha256::new()).is_err());
        assert!(inspect(&root, &["../outside".into()], &mut Sha256::new()).is_err());
    }
}
