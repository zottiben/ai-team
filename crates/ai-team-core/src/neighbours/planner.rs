//! ai-planner, over the `aip` CLI.
//!
//! The work graph belongs to ai-planner (D4). Nothing here copies a plan or a slice into
//! ai-team's database: a slice is referenced by its key, read when it is needed, and
//! written back through the same CLI a human uses. If that means a process per call,
//! that is the price of not owning a second copy of the truth.
//!
//! `aip serve` speaks MCP over stdio, not HTTP, so eve's `defineMcpClientConnection`
//! cannot reach it - which is why this is a CLI wrapper and not a generated connection.

use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

use crate::error::{Error, Result};

/// One slice as ai-planner reports it. Only the fields dispatch actually reads: the plan
/// document is the human's, and mirroring all of it here would be the copy D4 forbids.
#[derive(Debug, Clone, Deserialize)]
pub struct Slice {
    pub key: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub ord: i64,
    #[serde(default)]
    pub scope_md: Option<String>,
    #[serde(default)]
    pub demo_md: Option<String>,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
}

impl Slice {
    /// A slice nobody is building and nothing is holding.
    pub fn is_dispatchable(&self) -> bool {
        self.status == "ready" && self.claimed_by.is_none()
    }

    /// The paths this slice touches, as the orchestrator declared them.
    ///
    /// Written as a `Touches:` trailer on the scope, which is deliberately something a
    /// human reads on the board too rather than a private side-channel. Routing needs a
    /// path because zones are paths: the seat that owns a path is the seat that changes
    /// it, and that is the only thing keeping two agents out of one file.
    pub fn touches(&self) -> Vec<String> {
        let Some(scope) = &self.scope_md else {
            return Vec::new();
        };
        scope
            .lines()
            .rev()
            .find_map(|line| line.trim().strip_prefix("Touches:").map(str::trim))
            .map(|paths| {
                paths
                    .split(',')
                    .map(|path| path.trim().trim_matches('`').to_string())
                    .filter(|path| !path.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The `aip` CLI, rooted at one checkout.
///
/// `-C` rather than the process's own working directory, because ai-team drives several
/// worktrees at once and a shared `chdir` would be a race between them.
#[derive(Debug, Clone)]
pub struct Planner {
    root: std::path::PathBuf,
    plan: Option<String>,
}

impl Planner {
    pub fn at(root: impl Into<std::path::PathBuf>) -> Planner {
        Planner {
            root: root.into(),
            plan: None,
        }
    }

    #[must_use]
    pub fn for_plan(mut self, plan: impl Into<String>) -> Planner {
        self.plan = Some(plan.into());
        self
    }

    pub fn plan_slug(&self) -> Option<&str> {
        self.plan.as_deref()
    }

    /// Is `aip` installed at all? `ait doctor` asks, so the answer is a reason rather
    /// than a bool.
    pub async fn check(&self) -> std::result::Result<String, String> {
        match self.output(&["--version"]).await {
            Ok(version) => Ok(version.trim().to_string()),
            Err(error) => Err(format!("ai-planner is not usable: {error}")),
        }
    }

    pub async fn slices(&self) -> Result<Vec<Slice>> {
        let json = self.output(&["slice", "ls", "--json"]).await?;
        serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!("could not read `aip slice ls --json`: {error}"))
        })
    }

    /// Take a slice for a worktree. `false` means somebody else holds it, which is a
    /// normal outcome of two runs racing and not an error.
    pub async fn claim(&self, key: &str, worktree: &Path) -> Result<bool> {
        match self
            .output_in(worktree, &["slice", "claim", key, "--json"])
            .await
        {
            Ok(_) => Ok(true),
            // `aip` refuses a slice another worktree holds. Distinguishing that from a
            // broken install matters: one is contention, the other needs a human.
            Err(error) if error.to_string().contains("claim") => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub async fn set_status(&self, key: &str, status: &str, reason: Option<&str>) -> Result<()> {
        let mut args = vec!["slice", "set", key, status];
        if let Some(reason) = reason {
            args.push("--reason");
            args.push(reason);
        }
        self.output(&args).await.map(drop)
    }

    /// Point the slice at the branch its work landed on, so review starts from the board.
    pub async fn set_branch(&self, key: &str, branch: &str) -> Result<()> {
        self.output(&["slice", "edit", key, "--branch", branch])
            .await
            .map(drop)
    }

    pub async fn release(&self, key: &str, worktree: &Path) -> Result<()> {
        self.output_in(worktree, &["slice", "release", key])
            .await
            .map(drop)
    }

    pub async fn log(&self, note: &str, slice: Option<&str>) -> Result<()> {
        let mut args = vec!["log", note];
        if let Some(slice) = slice {
            args.push("--slice");
            args.push(slice);
        }
        self.output(&args).await.map(drop)
    }

    async fn output(&self, args: &[&str]) -> Result<String> {
        let root = self.root.clone();
        self.output_in(&root, args).await
    }

    async fn output_in(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut command = Command::new("aip");
        command.arg("-C").arg(cwd);
        if let Some(plan) = &self.plan {
            // Every call names the plan. `aip` can infer one from the worktree, but a
            // leased worktree is a copy of the repo and inference there is a guess.
            command.arg("-p").arg(plan);
        }
        command
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let output = command.output().await.map_err(|error| {
            Error::invalid(format!(
                "could not run `aip`: {error}. ai-planner is a separate tool; install it \
                 from its own repo."
            ))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::invalid(format!(
                "`aip {}` failed: {}",
                args.join(" "),
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(scope: &str) -> Slice {
        Slice {
            key: "PR1".into(),
            title: "t".into(),
            status: "ready".into(),
            ord: 10,
            scope_md: Some(scope.to_string()),
            demo_md: None,
            claimed_by: None,
            worktree_path: None,
        }
    }

    #[test]
    fn the_touches_trailer_names_the_paths_that_route_a_slice() {
        let s = slice("Add the export.\n\nTouches: crates/**, ui/src/App.tsx");
        assert_eq!(s.touches(), ["crates/**", "ui/src/App.tsx"]);

        // Backticks are what a model writes when asked for paths in markdown.
        let quoted = slice("Scope.\n\nTouches: `ui/**`, `*.css`");
        assert_eq!(quoted.touches(), ["ui/**", "*.css"]);

        // The last trailer wins, so a rewritten scope does not leave a stale one behind.
        let twice = slice("Touches: old/**\n\nrewritten\n\nTouches: new/**");
        assert_eq!(twice.touches(), ["new/**"]);

        assert!(slice("no trailer here").touches().is_empty());
        assert!(Slice {
            scope_md: None,
            ..slice("")
        }
        .touches()
        .is_empty());
    }

    #[test]
    fn only_an_unclaimed_ready_slice_is_dispatchable() {
        let mut s = slice("Touches: crates/**");
        assert!(s.is_dispatchable());

        s.status = "done".into();
        assert!(!s.is_dispatchable());

        // Claimed by another worktree: contention, not an error, and not ours to take.
        s.status = "ready".into();
        s.claimed_by = Some("someone".into());
        assert!(!s.is_dispatchable());
    }
}
