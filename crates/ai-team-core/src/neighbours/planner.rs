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

/// The one board reason Rust owns. It is used only to keep a completed plan inert until
/// its run's human approves it; every other blocked reason belongs to the plan itself.
const APPROVAL_HOLD: &str = "Awaiting plan approval from ai-team";
// W7 shipped after the first installed planning run. That older orchestrator used this
// exact visible board reason before the Rust-owned approval hold existed. Recognising the
// precise string lets the operator finish that board without treating arbitrary blocked
// work as approved.
const LEGACY_APPROVAL_HOLD: &str =
    "Awaiting human review and approval of the plan before ai-team dispatches build work.";

/// One slice as ai-planner reports it.
///
/// This is a read-through DTO, not a second plan model: every value is deserialised from
/// `aip slice ls --json` for the request that needs it and never stored in ai-team's
/// database (D4). Dispatch reads only a few fields; the board needs the delivery facts
/// ai-planner already publishes in order to open the same useful ticket drawer.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct Slice {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub plan_id: i64,
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
    pub estimate_files: Option<i64>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub claimed_at: Option<String>,
    #[serde(default)]
    pub blocked_reason: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub rev: i64,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// One append-only progress note, again read directly from ai-planner.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct PlanLogEntry {
    pub id: i64,
    pub plan_id: i64,
    #[serde(default)]
    pub slice_key: Option<String>,
    pub at: String,
    #[serde(default)]
    pub actor: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    pub body: String,
}

impl Slice {
    /// A slice nobody is building and nothing is holding.
    pub fn is_dispatchable(&self) -> bool {
        self.status == "ready" && self.claimed_by.is_none()
    }

    /// Held only for a person's approval, not blocked by a substantive failure.
    pub fn is_approval_held(&self) -> bool {
        self.status == "blocked"
            && matches!(
                self.blocked_reason.as_deref(),
                Some(APPROVAL_HOLD | LEGACY_APPROVAL_HOLD)
            )
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

/// Which plan a checkout is working, as ai-planner reports it.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct PlanSummary {
    pub plan: String,
    pub title: String,
    pub status: String,
    /// The slice this worktree has claimed, when it has one.
    #[serde(default)]
    pub slice: Option<String>,
}

/// One plan's header, as `aip ls --json` reports it. Read to check a base branch, never
/// kept (D4).
#[derive(Debug, Clone, Deserialize)]
pub struct PlanHeader {
    pub slug: String,
    #[serde(default)]
    pub base_branch: Option<String>,
}

/// A question ai-planner is holding for a human.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Question {
    pub body: String,
    pub status: String,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub asked_at: Option<String>,
    #[serde(default)]
    pub slice_key: Option<String>,
}

/// The `aip` CLI, rooted at one checkout.
///
/// `-C` rather than the process's own working directory, because ai-team drives several
/// worktrees at once and a shared `chdir` would be a race between them.
#[derive(Debug, Clone)]
pub struct Planner {
    root: std::path::PathBuf,
    plan: Option<String>,
    /// A database other than the operator's, for tests that drive the real `aip`.
    db: Option<std::path::PathBuf>,
}

impl Planner {
    pub fn at(root: impl Into<std::path::PathBuf>) -> Planner {
        Planner {
            root: root.into(),
            plan: None,
            db: None,
        }
    }

    /// Point every call at a scratch database. Per call rather than through
    /// `AI_PLANNER_DB`, because setting a variable in a process running tests on several
    /// threads races every other thread reading one.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_db(mut self, db: impl Into<std::path::PathBuf>) -> Planner {
        self.db = Some(db.into());
        self
    }

    #[must_use]
    pub fn for_plan(mut self, plan: impl Into<String>) -> Planner {
        self.plan = Some(plan.into());
        self
    }

    pub fn plan_slug(&self) -> Option<&str> {
        self.plan.as_deref()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Is `aip` installed at all? `ait doctor` asks, so the answer is a reason rather
    /// than a bool.
    pub async fn check(&self) -> std::result::Result<String, String> {
        match self.output(&["--version"]).await {
            Ok(version) => Ok(version.trim().to_string()),
            Err(error) => Err(format!("ai-planner is not usable: {error}")),
        }
    }

    /// Make sure there is a database and this checkout is registered in it.
    ///
    /// `aip init` creates the file if it is missing and is a no-op when it is not, so
    /// this is safe to run before every workflow. It has to run before anything else:
    /// `aip serve` refuses to start without a database, and a planning seat whose MCP
    /// server did not start has no planning tools - so it decides the board is
    /// unavailable and writes the code itself, which is how the first Pi run went.
    ///
    /// Not a violation of D17. ai-team may create its own state and may never install
    /// software; this creates a file with a command the neighbour publishes for exactly
    /// that purpose.
    pub async fn ensure(&self) -> Result<()> {
        self.output(&["init"]).await.map(|_| ())
    }

    /// Which plan this checkout resolves to, and what it is called.
    pub async fn current(&self) -> Result<PlanSummary> {
        let json = self.output(&["current", "--json"]).await?;
        serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!("could not read `aip current --json`: {error}"))
        })
    }

    /// The questions the plan is holding, unanswered.
    pub async fn open_questions(&self) -> Result<Vec<Question>> {
        let json = self.output(&["question", "ls", "--json"]).await?;
        let all: Vec<Question> = serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!("could not read `aip question ls --json`: {error}"))
        })?;
        // ai-planner's own word for it, rather than inferring from a missing answer:
        // a question can be closed without one.
        Ok(all.into_iter().filter(|q| q.status == "open").collect())
    }

    pub async fn slices(&self) -> Result<Vec<Slice>> {
        let json = self.output(&["slice", "ls", "--json"]).await?;
        serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!("could not read `aip slice ls --json`: {error}"))
        })
    }

    pub async fn slice(&self, key: &str) -> Result<Slice> {
        let json = self.output(&["slice", "show", key, "--json"]).await?;
        serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!(
                "could not read `aip slice show {key} --json`: {error}"
            ))
        })
    }

    /// Hold only slices the planner actually offered for dispatch. Drafts and slices
    /// blocked for substantive reasons stay exactly as the board says.
    pub async fn hold_ready_for_approval(
        &self,
        mut heartbeat: impl FnMut() -> Result<()>,
    ) -> Result<usize> {
        let ready: Vec<Slice> = self
            .slices()
            .await?
            .into_iter()
            .filter(Slice::is_dispatchable)
            .collect();
        for slice in &ready {
            heartbeat()?;
            self.set_status(&slice.key, "blocked", Some(APPROVAL_HOLD))
                .await?;
            heartbeat()?;
        }
        Ok(ready.len())
    }

    /// Release precisely ai-team's approval holds; no other blocked work is advanced.
    pub async fn approve_held(&self) -> Result<usize> {
        let held: Vec<Slice> = self
            .slices()
            .await?
            .into_iter()
            .filter(Slice::is_approval_held)
            .collect();
        for slice in &held {
            self.set_status(&slice.key, "ready", None).await?;
        }
        Ok(held.len())
    }

    pub async fn logs(&self, key: &str) -> Result<Vec<PlanLogEntry>> {
        let json = self
            .output(&["logs", "--slice", key, "--limit", "200", "--json"])
            .await?;
        serde_json::from_str(&json)
            .map_err(|error| Error::invalid(format!("could not read `aip logs --json`: {error}")))
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

    /// Add a slice, choosing a key that is not taken.
    ///
    /// ai-planner keys are the human's handle for a slice, so this picks the next free
    /// number under a prefix rather than inventing something unpronounceable.
    pub async fn add_slice(
        &self,
        prefix: &str,
        title: &str,
        scope: &str,
        demo: &str,
        touches: &[&str],
    ) -> Result<String> {
        let taken: Vec<String> = self.slices().await?.into_iter().map(|s| s.key).collect();
        // Bounded: one more than the number taken is always free, so this cannot run on.
        let key = (1..=taken.len() + 1)
            .map(|n| format!("{prefix}{n}"))
            .find(|key| !taken.contains(key))
            .unwrap_or_else(|| format!("{prefix}1"));

        // The `Touches:` trailer is what zone routing reads, so a slice without one can
        // never be dispatched to anybody (D14).
        let scope = format!("{}\n\nTouches: {}", scope.trim(), touches.join(", "));
        self.output(&[
            "slice", "add", &key, title, "--scope", &scope, "--demo", demo,
        ])
        .await?;
        Ok(key)
    }

    /// Append to a slice's scope, keeping what is already there.
    ///
    /// Used when a review changes what the work is. The verifier checks a commit against
    /// the slice spec, so feedback that only reaches the agent produces work that is
    /// right and rejected.
    pub async fn amend_scope(&self, key: &str, addition: &str) -> Result<()> {
        let existing = self
            .slices()
            .await?
            .into_iter()
            .find(|slice| slice.key == key)
            .and_then(|slice| slice.scope_md)
            .unwrap_or_default();
        let scope = format!("{}\n\n{}", existing.trim_end(), addition.trim());
        self.output(&["slice", "edit", key, "--scope", &scope])
            .await?;
        Ok(())
    }

    pub async fn set_status(&self, key: &str, status: &str, reason: Option<&str>) -> Result<()> {
        let mut args = vec!["slice", "set", key, status];
        if let Some(reason) = reason {
            args.push("--reason");
            args.push(reason);
        }
        self.output(&args).await.map(drop)
    }

    /// The plans in this checkout's repository.
    pub async fn plans(&self) -> Result<Vec<PlanHeader>> {
        let json = self.output(&["ls", "--json"]).await?;
        serde_json::from_str(&json)
            .map_err(|error| Error::invalid(format!("could not read `aip ls --json`: {error}")))
    }

    /// The branch this plan's slices stack onto by default.
    pub async fn set_plan_base(&self, base: &str) -> Result<()> {
        self.output(&["edit", "--base", base]).await.map(drop)
    }

    /// The branch one slice is built on and its pull request targets.
    pub async fn set_slice_base(&self, key: &str, base: &str) -> Result<()> {
        self.output(&["slice", "edit", key, "--base", base])
            .await
            .map(drop)
    }

    /// Point the slice at the branch its work landed on, so review starts from the board.
    pub async fn set_branch(&self, key: &str, branch: &str) -> Result<()> {
        self.output(&["slice", "edit", key, "--branch", branch])
            .await
            .map(drop)
    }

    pub async fn set_pr(&self, key: &str, url: &str) -> Result<()> {
        self.output(&["slice", "edit", key, "--pr", url])
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
        if let Some(db) = &self.db {
            command.arg("--db").arg(db);
        }
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
            ..Default::default()
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
    fn only_the_exact_ai_team_holds_are_plan_approval() {
        for reason in [APPROVAL_HOLD, LEGACY_APPROVAL_HOLD] {
            let mut held = slice("Touches: crates/**");
            held.status = "blocked".into();
            held.blocked_reason = Some(reason.into());
            assert!(held.is_approval_held());
        }

        let mut failed = slice("Touches: crates/**");
        failed.status = "blocked".into();
        failed.blocked_reason = Some("gates failed".into());
        assert!(!failed.is_approval_held());
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
