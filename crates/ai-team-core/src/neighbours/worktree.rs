//! ai-worktree, over the `awt` CLI.
//!
//! Isolation is the whole point: one leased worktree per node, one eve process bound to
//! it (D3, D10). `awt` owns the pool, the cleaning and the reuse; ai-team only borrows
//! and returns (D4).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct PoolEntry {
    pub name: String,
    pub path: String,
    pub status: String,
    #[serde(rename = "leaseHolder", default)]
    pub lease_holder: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Pool {
    #[serde(default)]
    worktrees: Vec<PoolEntry>,
}

/// A worktree held for one node, returned to the pool when it is dropped.
///
/// `awt` will not hand a leased tree to a later `get` and will not prune it, so a lease
/// that is never returned leaks a worktree out of the pool. Returning is therefore tied
/// to the value's lifetime rather than to remembering.
#[derive(Debug)]
pub struct Lease {
    path: PathBuf,
    repo: PathBuf,
    returned: bool,
}

impl Lease {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Give the worktree back, cleaned. Returns nothing useful on purpose: a failure to
    /// return is reported, but it must not fail the work that was already done in it.
    pub async fn release(mut self) -> Result<()> {
        self.returned = true;
        Worktrees::at(&self.repo).release(&self.path).await
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.returned {
            return;
        }
        // A synchronous best-effort return on the unhappy path. The async `release` is
        // the intended route; this is what stops a panic or an early `?` from quietly
        // shrinking the pool.
        let _ = std::process::Command::new("awt")
            .arg("return")
            .arg(&self.path)
            .arg("--force")
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The `awt` pool for one repository.
#[derive(Debug, Clone)]
pub struct Worktrees {
    repo: PathBuf,
}

impl Worktrees {
    pub fn at(repo: impl Into<PathBuf>) -> Worktrees {
        Worktrees { repo: repo.into() }
    }

    pub async fn check(&self) -> std::result::Result<String, String> {
        match self.output(&["--version"]).await {
            Ok(version) => Ok(version.trim().to_string()),
            Err(error) => Err(format!("ai-worktree is not usable: {error}")),
        }
    }

    /// Lease a worktree. `--lease` prints only the path on stdout and puts its banners
    /// on stderr, which is the contract this depends on.
    pub async fn lease(&self, holder: &str) -> Result<Lease> {
        let path = self
            .output(&["get", "--lease", "--lease-holder", holder])
            .await?;
        let path = PathBuf::from(path.trim());
        if !path.is_dir() {
            return Err(Error::invalid(format!(
                "`awt get --lease` printed {} which is not a directory",
                path.display()
            )));
        }
        Ok(Lease {
            path,
            repo: self.repo.clone(),
            returned: false,
        })
    }

    pub async fn release(&self, path: &Path) -> Result<()> {
        self.output(&["return", &path.to_string_lossy(), "--force"])
            .await
            .map(drop)
    }

    pub async fn pool(&self) -> Result<Vec<PoolEntry>> {
        let json = self.output(&["status", "--json"]).await?;
        let pool: Pool = serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!("could not read `awt status --json`: {error}"))
        })?;
        Ok(pool.worktrees)
    }

    async fn output(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("awt")
            .args(args)
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| {
                Error::invalid(format!(
                    "could not run `awt`: {error}. ai-worktree is a separate tool; install \
                     it from its own repo."
                ))
            })?;
        if !output.status.success() {
            return Err(Error::invalid(format!(
                "`awt {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_json_shape_is_what_awt_actually_prints() {
        // Captured from `awt status --json` v0.2.0 rather than written from the docs.
        let pool: Pool = serde_json::from_str(
            r#"{"poolDir":"/home/me/.awt/repo-d0e174","worktrees":[
                 {"name":"1","path":"/home/me/.awt/repo-d0e174/1/repo","status":"leased",
                  "leaseHolder":"ai-team","processes":[]},
                 {"name":"2","path":"/home/me/.awt/repo-d0e174/2/repo","status":"available",
                  "processes":[]}]}"#,
        )
        .unwrap();
        assert_eq!(pool.worktrees.len(), 2);
        assert_eq!(pool.worktrees[0].status, "leased");
        assert_eq!(pool.worktrees[0].lease_holder.as_deref(), Some("ai-team"));
        // An available tree carries no holder, and that must not fail to parse.
        assert_eq!(pool.worktrees[1].lease_holder, None);
    }
}
