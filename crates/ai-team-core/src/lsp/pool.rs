//! One server per language per workspace root, kept alive.
//!
//! rust-analyzer takes the better part of a minute to index a real project. Starting one
//! per request would mean never getting an answer, so servers live in here and are reused
//! for every file that belongs to them.
//!
//! Keyed on (command, root) rather than on language: a repository with a Cargo workspace
//! and a `ui/` beside it is two TypeScript-or-Rust projects, and one server pointed at the
//! repository root would index code it cannot understand and answer questions about the
//! wrong files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::Result;
use crate::lsp::{language_for, workspace_root, Client};

/// The servers ai-team has running.
#[derive(Debug, Default)]
pub struct Pool {
    servers: Mutex<HashMap<(String, PathBuf), Arc<Client>>>,
}

impl Pool {
    pub fn new() -> Pool {
        Pool::default()
    }

    /// The server for a file, started if this is the first one of its kind.
    ///
    /// `None` when no server handles that language, which is the ordinary answer for a
    /// README and must not be reported as a failure.
    pub async fn for_file(&self, worktree: &Path, path: &str) -> Result<Option<Arc<Client>>> {
        let Some(language) = language_for(path) else {
            return Ok(None);
        };
        let root = workspace_root(worktree, path, language);
        let key = (language.command.to_string(), root.clone());

        // Checked before starting and again after, because starting rust-analyzer takes
        // long enough that two requests for the same project routinely overlap - and the
        // second one must not leave an orphan indexing the same code.
        if let Some(existing) = self.servers.lock().await.get(&key) {
            return Ok(Some(Arc::clone(existing)));
        }

        let client = Arc::new(Client::start(language, &root).await?);

        let mut servers = self.servers.lock().await;
        Ok(Some(Arc::clone(
            servers.entry(key).or_insert_with(|| Arc::clone(&client)),
        )))
    }

    /// How many servers are up. For `ait doctor` and for tests.
    pub async fn running(&self) -> usize {
        self.servers.lock().await.len()
    }

    /// Stop everything. Dropping a client kills its process, so this is the whole of it.
    pub async fn shutdown(&self) {
        self.servers.lock().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        dir
    }

    fn have(command: &str) -> bool {
        std::process::Command::new(command)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[tokio::test]
    async fn a_file_with_no_language_server_is_not_an_error() {
        // The ordinary answer for a README, and reporting it as a failure would put a red
        // message on every text file in the tree.
        let pool = Pool::new();
        let dir = tempfile::tempdir().unwrap();
        assert!(pool
            .for_file(dir.path(), "README.md")
            .await
            .unwrap()
            .is_none());
        assert_eq!(pool.running().await, 0);
    }

    #[tokio::test]
    async fn two_files_in_one_project_share_one_server() {
        if !have("rust-analyzer") {
            eprintln!("skipped: rust-analyzer is not installed");
            return;
        }
        let dir = rust_project();
        std::fs::write(dir.path().join("src/other.rs"), "pub fn b() {}\n").unwrap();

        let pool = Pool::new();
        pool.for_file(dir.path(), "src/lib.rs")
            .await
            .unwrap()
            .unwrap();
        pool.for_file(dir.path(), "src/other.rs")
            .await
            .unwrap()
            .unwrap();

        // Two starts would mean two indexes of the same code, each taking the better part
        // of a minute.
        assert_eq!(pool.running().await, 1);
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn two_projects_in_one_repository_get_their_own_servers() {
        if !have("rust-analyzer") {
            eprintln!("skipped: rust-analyzer is not installed");
            return;
        }
        let dir = rust_project();
        let nested = dir.path().join("tools/helper");
        std::fs::create_dir_all(nested.join("src")).unwrap();
        std::fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(nested.join("src/lib.rs"), "pub fn c() {}\n").unwrap();

        let pool = Pool::new();
        pool.for_file(dir.path(), "src/lib.rs")
            .await
            .unwrap()
            .unwrap();
        pool.for_file(dir.path(), "tools/helper/src/lib.rs")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(pool.running().await, 2);
        pool.shutdown().await;
    }
}
