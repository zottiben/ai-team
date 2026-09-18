//! `file-sql`, over its own CLI.
//!
//! file-sql speaks MCP over stdio, and nothing here can hold a stdio MCP session open -
//! so it is reached the same way `aip` and `awt` are: by running the neighbour's own
//! binary and reading its JSON (D4).
//!
//! Search is file-sql's job and ai-team does not have a second opinion about it. A repo
//! with no index gets told how to make one rather than being handed a grep: a naive
//! substring scan over a checkout is slower, worse ranked, and would quietly become the
//! thing everybody uses.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{Error, Result};

/// One ranked hit.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Hit {
    pub path: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    pub score: f64,
    pub start_line: i64,
    pub end_line: i64,
    pub snippet: String,
}

/// file-sql, rooted at one checkout.
#[derive(Debug, Clone)]
pub struct FileSql {
    root: PathBuf,
}

impl FileSql {
    pub fn at(root: impl Into<PathBuf>) -> FileSql {
        FileSql { root: root.into() }
    }

    /// Where this checkout's index configuration lives.
    fn config(&self) -> PathBuf {
        self.root.join(".file-sql").join("config.toml")
    }

    /// Whether this checkout has an index to search.
    ///
    /// Checked rather than assumed, so the surface can say "run `file-sql index` here"
    /// instead of returning nothing and looking broken.
    pub fn indexed(&self) -> bool {
        self.config().is_file()
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        if !self.indexed() {
            return Err(Error::invalid(
                "this checkout has no file-sql index - run `file-sql index` in it",
            ));
        }
        let limit = limit.to_string();
        let output = Command::new("file-sql")
            .args(["search", query, "--limit", &limit])
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| {
                Error::invalid(format!(
                    "could not run file-sql: {error} - see zottiben.github.io/file-sql"
                ))
            })?;

        if !output.status.success() {
            return Err(Error::invalid(format!(
                "file-sql search failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| Error::invalid(format!("could not read file-sql's hits: {error}")))
    }
}

/// Whether file-sql is installed at all.
pub async fn available() -> bool {
    Command::new("file-sql")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// A file in the tree, as the window needs it.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub dir: bool,
}

/// What ai-team refuses to walk into.
///
/// Build output, not source. A tree that lists `target/` is a tree nobody scrolls, and on
/// this project that single directory holds more files than the rest of the repo by two
/// orders of magnitude.
const SKIP: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    ".output",
    ".eve",
    ".file-sql",
];

/// One level of the tree, directories first then files, each alphabetical.
///
/// One level rather than the whole thing: a recursive walk of a real repository is
/// thousands of entries the window will not draw, and the cost is paid on every poll.
pub fn list(worktree: &Path, relative: &str) -> Result<Vec<Entry>> {
    let dir = safe_join(worktree, relative)?;
    let mut entries: Vec<Entry> = std::fs::read_dir(&dir)
        .map_err(|error| Error::invalid(format!("could not read {}: {error}", dir.display())))?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKIP.contains(&name.as_str()) {
                return None;
            }
            let dir = entry.file_type().ok()?.is_dir();
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", relative.trim_end_matches('/'))
            };
            Some(Entry { path, name, dir })
        })
        .collect();

    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// Resolve a path inside a worktree, refusing anything that leaves it.
///
/// The window sends paths a human clicked, but the route that accepts them is an HTTP
/// endpoint on loopback and `../../../../etc/passwd` is one curl away. Canonicalised
/// because on macOS `/tmp` is a symlink into `/private` and a string comparison against
/// an uncanonicalised root rejects perfectly legitimate paths (D12).
pub fn safe_join(worktree: &Path, relative: &str) -> Result<PathBuf> {
    let root = worktree
        .canonicalize()
        .map_err(|error| Error::invalid(format!("no such worktree: {error}")))?;
    // Joined as given. Stripping a leading slash first would quietly turn
    // `/etc/passwd` into `<worktree>/etc/passwd` - safe, but it rewrites the caller's
    // input instead of refusing it. `Path::join` replaces the whole path when the
    // argument is absolute, which is exactly what makes the guard below catch it.
    let joined = root.join(relative);

    // Canonicalise the deepest part that exists, so a path to a file being created still
    // gets checked rather than skipping the guard.
    let resolved = joined.canonicalize().unwrap_or_else(|_| {
        joined
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .map_or_else(
                || joined.clone(),
                |parent| parent.join(joined.file_name().unwrap_or_default()),
            )
    });

    if !resolved.starts_with(&root) {
        return Err(Error::invalid("that path is outside the worktree"));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn a() {}").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi").unwrap();
        std::fs::write(dir.path().join("target/debug/huge"), "junk").unwrap();
        dir
    }

    #[test]
    fn build_output_is_not_part_of_the_tree() {
        // On this project `target/` alone holds more files than the rest of the repo by
        // two orders of magnitude, and a tree that lists it is a tree nobody scrolls.
        let dir = repo();
        let entries = list(dir.path(), "").unwrap();
        assert!(entries.iter().all(|entry| entry.name != "target"));
    }

    #[test]
    fn directories_come_first_then_files_each_alphabetical() {
        let dir = repo();
        let entries = list(dir.path(), "").unwrap();
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["src", "README.md"]);
    }

    #[test]
    fn a_child_path_is_relative_to_the_worktree_not_absolute() {
        // The window sends these back to open a file, so they have to mean something on
        // their own.
        let dir = repo();
        let entries = list(dir.path(), "src").unwrap();
        assert_eq!(entries[0].path, "src/lib.rs");
    }

    #[test]
    fn a_path_that_climbs_out_of_the_worktree_is_refused() {
        // The route that accepts these is an HTTP endpoint, and `../../etc/passwd` is one
        // curl away.
        let dir = repo();
        assert!(safe_join(dir.path(), "../../etc/passwd").is_err());
        assert!(safe_join(dir.path(), "/etc/passwd").is_err());
        assert!(safe_join(dir.path(), "src/../../..").is_err());
    }

    #[test]
    fn an_ordinary_path_resolves() {
        let dir = repo();
        let path = safe_join(dir.path(), "src/lib.rs").unwrap();
        assert!(path.ends_with("src/lib.rs"));
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_still_checked() {
        // Saving a new file must not be the way past the guard.
        let dir = repo();
        assert!(safe_join(dir.path(), "src/new.rs").is_ok());
        assert!(safe_join(dir.path(), "src/../../escape.rs").is_err());
    }

    #[test]
    fn a_checkout_with_no_index_says_so_rather_than_searching_badly() {
        let dir = repo();
        assert!(!FileSql::at(dir.path()).indexed());
    }
}
