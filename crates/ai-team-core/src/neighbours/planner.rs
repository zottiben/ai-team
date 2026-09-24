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

    /// The tasks this PR is built as, read out of its scope (PW4). Empty for a slice that
    /// is one piece of work.
    pub fn tasks(&self) -> crate::tasks::TaskList {
        crate::tasks::parse(self.scope_md.as_deref().unwrap_or_default())
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

/// One plan's header, as `aip ls --json` reports it. Read to find a free slug, never kept
/// (D4).
#[derive(Debug, Clone, Deserialize)]
pub struct PlanHeader {
    pub slug: String,
    pub title: String,
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

    /// Start a plan in this checkout's repository and say which slug it got.
    ///
    /// A run's plan is created here, by name, before any seat touches the board - so the
    /// seats can be pointed at it rather than left to infer one (see `AI_PLANNER_PLAN`).
    /// `base` is what its pull requests target; every slice copies it as it is added. The
    /// slug is the title's, cut to something a branch name can carry, and chosen so that
    /// every plan in the repository can still be named (see [`free_slug`]).
    pub async fn create(&self, title: &str, base: Option<&str>) -> Result<String> {
        let slug = free_slug(title, &self.plans().await?);
        let mut args = vec!["new", title, "--slug", &slug];
        if let Some(base) = base {
            args.push("--base");
            args.push(base);
        }
        self.output(&args).await?;
        Ok(slug)
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

/// The longest slug a plan is given. A plan's slug names its pull requests' branches,
/// `<plan>/pr1`, and a whole title there reads like a sentence.
const SLUG_LIMIT: usize = 40;

/// A slug for `title` that leaves every plan - the new one and all the repository's
/// others - possible to name.
///
/// ai-planner finds a plan by matching the name against every slug and title that
/// contains it, and refuses when more than one does: an exact slug does not win. So a new
/// slug may not be one another plan's slug or title contains, and may not contain another
/// plan's slug - `csv-export-2` would leave `csv-export` unnameable. Tried in order: the
/// title's words cut at [`SLUG_LIMIT`], then numbered, then with leading words dropped.
fn free_slug(title: &str, plans: &[PlanHeader]) -> String {
    let words: Vec<String> = crate::util::slugify(title)
        .split('-')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    let usable = |slug: &str| {
        plans.iter().all(|plan| {
            !plan.slug.contains(slug)
                && !plan.title.to_lowercase().contains(slug)
                && !slug.contains(plan.slug.as_str())
        })
    };
    let bases = (0..words.len())
        .map(|start| cut(&words[start..]))
        .chain(std::iter::once("plan".to_string()));
    for base in bases {
        // Bounded: among one more number than there are plans, one is always unclaimed.
        let numbered = (2..=plans.len() + 2).map(|n| format!("{base}-{n}"));
        if let Some(slug) = std::iter::once(base.clone())
            .chain(numbered)
            .find(|slug| usable(slug))
        {
            return slug;
        }
    }
    format!("plan-{}", plans.len() + 1)
}

/// Words joined into a slug no longer than [`SLUG_LIMIT`], cut where a word ends.
fn cut(words: &[String]) -> String {
    let mut slug = String::new();
    for word in words {
        if !slug.is_empty() && slug.len() + 1 + word.len() > SLUG_LIMIT {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(word);
    }
    slug.truncate(SLUG_LIMIT);
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(slug: &str, title: &str) -> PlanHeader {
        PlanHeader {
            slug: slug.into(),
            title: title.into(),
        }
    }

    /// Whether naming `needle` to ai-planner finds exactly one of `plans`: it matches any
    /// slug or title containing it, an exact slug included, and refuses more than one.
    fn names_one(needle: &str, plans: &[PlanHeader]) -> bool {
        plans
            .iter()
            .filter(|plan| plan.slug.contains(needle) || plan.title.to_lowercase().contains(needle))
            .count()
            == 1
    }

    #[test]
    fn a_plan_slug_is_its_title_cut_at_a_word() {
        assert_eq!(free_slug("CSV export", &[]), "csv-export");
        // A title that runs on is cut where a word ends, not through one.
        let long = free_slug(
            "Let greet.sh take a --name=VALUE form as well as the positional name",
            &[],
        );
        assert_eq!(long, "let-greet-sh-take-a-name-value-form-as");
        assert!(long.len() <= SLUG_LIMIT);
        // Nothing sluggable still names a plan.
        assert_eq!(free_slug("!!!", &[]), "plan");
    }

    #[test]
    fn a_new_plan_never_makes_any_plan_ambiguous_to_name() {
        // ai-planner matches a name against every slug and title containing it and refuses
        // more than one - an exact slug included. `greet-name-flag-web` beside
        // `greet-name-flag` made the older plan impossible to name at all.
        for (title, existing) in [
            (
                "greet.sh --name flag, advertised on the web",
                vec![plan("greet-name-flag", "greet.sh - accept --name=VALUE")],
            ),
            ("CSV export", vec![plan("csv-export", "CSV export")]),
            (
                "CSV export",
                vec![
                    plan("csv-export", "CSV export"),
                    plan("export", "Export"),
                    plan("csv-export-2", "CSV export again"),
                ],
            ),
            // A slug a title already contains.
            ("Ledger", vec![plan("q3", "Rebuild the ledger totals")]),
        ] {
            let slug = free_slug(title, &existing);
            let mut after = existing.clone();
            after.push(plan(&slug, title));
            assert!(names_one(&slug, &after), "{slug} is ambiguous ({title})");
            // Every plan that could be named still can. (Some here could not to begin
            // with: `csv-export-2` beside `csv-export`.)
            for plan in existing
                .iter()
                .filter(|plan| names_one(&plan.slug, &existing))
            {
                assert!(
                    names_one(&plan.slug, &after),
                    "{slug} made {} ambiguous ({title})",
                    plan.slug
                );
            }
        }
    }

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
