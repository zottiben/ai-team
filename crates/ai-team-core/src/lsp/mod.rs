//! Language servers, multiplexed.
//!
//! One server process per language per workspace root, started when a file of that
//! language is first opened there and kept alive afterwards. rust-analyzer takes the
//! better part of a minute to index a real project; starting one per request would mean
//! never getting an answer.
//!
//! What this is *not* is a language server of its own. ai-team runs the real ones and
//! passes their answers through - the same relationship it has with `aip`, `awt` and
//! `file-sql` (D4), except these speak LSP over stdio rather than a CLI.
//!
//! A missing server is a normal state, not a failure. Plenty of machines have no
//! rust-analyzer, and a repository that cannot be analysed still has to open in the
//! editor.

mod client;
mod pool;
mod wire;

pub use client::{Client, Diagnostic, Hover, Location, Position, Range};
pub use pool::Pool;

use std::path::Path;

/// Which server handles a file, and what to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// LSP's own identifier, sent in `didOpen`.
    pub id: &'static str,
    /// The binary to run.
    pub command: &'static str,
    pub args: &'static [&'static str],
}

/// rust-analyzer reads Cargo.toml; typescript-language-server reads tsconfig.json.
const LANGUAGES: &[(&[&str], Language)] = &[
    (
        &["rs"],
        Language {
            id: "rust",
            command: "rust-analyzer",
            args: &[],
        },
    ),
    (
        &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        Language {
            // One server for all of them: typescript-language-server handles JavaScript
            // too, and running a second copy for `.js` would index the same project twice.
            id: "typescript",
            command: "typescript-language-server",
            args: &["--stdio"],
        },
    ),
];

/// Which language server a path belongs to, if any.
///
/// By extension, like the editor's highlighting. A file is named before it is parsed.
pub fn language_for(path: &str) -> Option<Language> {
    let ext = path.rsplit('.').next()?.to_lowercase();
    LANGUAGES
        .iter()
        .find(|(exts, _)| exts.contains(&ext.as_str()))
        .map(|(_, language)| *language)
}

/// LSP's own language identifier for a path.
pub fn language_id(path: &str) -> &'static str {
    language_for(path).map_or("plaintext", |language| language.id)
}

/// The directory a server should treat as its workspace.
///
/// A language server wants the root of the *project*, not the repository: a `ui/` beside
/// a Cargo workspace is its own TypeScript project, and pointing tsserver at the
/// repository root makes it index Rust it will never understand.
pub fn workspace_root(worktree: &Path, path: &str, language: Language) -> std::path::PathBuf {
    let marker = match language.id {
        "rust" => "Cargo.toml",
        "typescript" => "tsconfig.json",
        _ => return worktree.to_path_buf(),
    };

    // Walk up from the file, stopping at the worktree: the nearest marker is the project.
    let mut dir = worktree.join(path);
    dir.pop();
    while dir.starts_with(worktree) {
        if dir.join(marker).is_file() {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    worktree.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_routed_by_its_extension() {
        assert_eq!(language_for("src/lib.rs").unwrap().id, "rust");
        assert_eq!(language_for("ui/src/App.tsx").unwrap().id, "typescript");
        assert_eq!(language_for("ui/src/main.js").unwrap().id, "typescript");
        assert!(language_for("README.md").is_none());
        assert!(language_for("Makefile").is_none());
    }

    #[test]
    fn javascript_and_typescript_share_one_server() {
        // Running a second copy for `.js` would index the same project twice, and
        // rust-analyzer already takes the better part of a minute on a real repo.
        assert_eq!(
            language_for("a.ts").unwrap().command,
            language_for("a.js").unwrap().command
        );
    }

    #[test]
    fn an_unknown_file_still_has_a_language_id() {
        // `didOpen` requires one, and "plaintext" is the specification's answer.
        assert_eq!(language_id("notes.txt"), "plaintext");
        assert_eq!(language_id("src/lib.rs"), "rust");
    }

    #[test]
    fn the_workspace_root_is_the_nearest_project_not_the_repository() {
        // A `ui/` beside a Cargo workspace is its own TypeScript project, and pointing
        // tsserver at the repository root makes it index Rust it cannot understand.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("ui/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/core/src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::write(root.join("crates/core/Cargo.toml"), "[package]").unwrap();
        std::fs::write(root.join("ui/tsconfig.json"), "{}").unwrap();

        let ts = language_for("ui/src/App.tsx").unwrap();
        assert_eq!(workspace_root(root, "ui/src/App.tsx", ts), root.join("ui"));

        // And the nearest Cargo.toml, not the workspace one at the top.
        let rust = language_for("crates/core/src/lib.rs").unwrap();
        assert_eq!(
            workspace_root(root, "crates/core/src/lib.rs", rust),
            root.join("crates/core")
        );
    }

    #[test]
    fn a_project_with_no_marker_falls_back_to_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let rust = language_for("src/lib.rs").unwrap();
        assert_eq!(workspace_root(dir.path(), "src/lib.rs", rust), dir.path());
    }
}
