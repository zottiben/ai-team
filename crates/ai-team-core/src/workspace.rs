//! The checkout a run starts in (PW2).
//!
//! A run lives in a top-level worktree - the main checkout, or one the operator made by
//! hand - and that worktree may still be sitting on the last plan's branch. A run with
//! something new to plan puts it on a fresh branch cut from the default branch first, so
//! the plan is grounded in the code as it is now and nothing the last plan left behind is
//! carried into this one.
//!
//! Three refusals keep that honest: a checkout holding somebody's uncommitted work, a
//! checkout another live run is still working in, and - unless the run was told it may -
//! landing work on the default branch itself.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{Error, Result};
use crate::neighbours::{git, Planner};

/// What putting a run's checkout on its own branch did, for the run to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Branched {
    pub branch: String,
    /// What it was cut from: `origin/main`, or `main` in a repository with no remote.
    pub from: String,
    pub sha: String,
    /// Why the default branch could not be brought up to date first, when it could not.
    /// The run goes ahead from the last fetched copy, and says so.
    pub stale: Option<String>,
}

/// Refuse a checkout holding uncommitted work.
///
/// Its agents commit whatever changed, so anything a person left here would be switched
/// onto the run's branch and committed into its pull request as if an agent had written
/// it. Asked before the run row exists, so a refusal leaves nothing behind to explain.
pub(crate) async fn ensure_clean(worktree: &Path) -> Result<()> {
    let dirty = git::uncommitted(worktree).await?;
    if dirty.is_empty() {
        return Ok(());
    }
    Err(Error::invalid(dirty_reason(worktree, &dirty)))
}

fn dirty_reason(worktree: &Path, dirty: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut listed = dirty
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if dirty.len() > SHOWN {
        let _ = write!(listed, " and {} more", dirty.len() - SHOWN);
    }
    format!(
        "{} has uncommitted changes: {listed}. A run starts on a fresh branch and commits \
         what changed, so they would end up in its pull request. Commit or stash them, then \
         start the run again.",
        worktree.display()
    )
}

/// Put a run's checkout on a fresh branch off the default branch.
///
/// Fetched first, because a local `main` is only as current as the last pull. A fetch
/// that fails - offline, or a remote asking for credentials nobody is there to type - is
/// reported rather than fatal: the last fetched copy is still the default branch.
pub(crate) async fn branch_for_run(worktree: &Path, run_id: i64) -> Result<Branched> {
    let named = git::trunk(worktree).await?;
    let stale = git::fetch(worktree, &named.name)
        .await
        .err()
        .map(|error| error.to_string());
    // Asked again after the fetch: a repository that had never fetched its default branch
    // has an `origin/main` to start from now.
    let trunk = git::trunk(worktree).await?;
    let branch = unused_branch(worktree, run_id).await;
    let sha = git::start_branch(worktree, &branch, &trunk.start_point).await?;
    Ok(Branched {
        branch,
        from: trunk.start_point,
        sha,
        stale,
    })
}

/// `ai-team/run-<id>`, or the first free variant of it.
///
/// A run id is an SQLite rowid, which is reused once the newest row is deleted - so the
/// branch a deleted run left behind can already have this run's name, and it may hold
/// somebody's work. Never reset: pick the next name instead.
async fn unused_branch(worktree: &Path, run_id: i64) -> String {
    let base = format!("ai-team/run-{run_id}");
    if !git::branch_exists(worktree, &base).await {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !git::branch_exists(worktree, &candidate).await {
            return candidate;
        }
        n += 1;
    }
}

/// Keep a run told it may work on the default branch on it, and current.
///
/// The checkout has to be on the default branch already: switching to it would fail in
/// any worktree but the one that has it checked out, and guessing another is exactly what
/// the operator did not ask for. Brought up to date by fast-forward only - a local
/// default branch that has diverged from the remote is somebody's to sort out.
pub(crate) async fn stay_on_default_branch(worktree: &Path) -> Result<Branched> {
    let named = git::trunk(worktree).await?;
    let current = git::current_branch(worktree).await;
    if current.as_deref() != Some(named.name.as_str()) {
        return Err(Error::invalid(format!(
            "this run may work on {} itself, but {} is on {}. Start it from the checkout \
             that has {} checked out.",
            named.name,
            worktree.display(),
            current.as_deref().unwrap_or("a detached head"),
            named.name
        )));
    }
    let stale = git::fetch(worktree, &named.name)
        .await
        .err()
        .map(|error| error.to_string());
    let trunk = git::trunk(worktree).await?;
    if trunk.start_point != trunk.name {
        git::fast_forward(worktree, &trunk.start_point).await?;
    }
    let sha = git::rev_parse(worktree, "HEAD").await?;
    Ok(Branched {
        branch: trunk.name,
        from: trunk.start_point,
        sha,
        stale,
    })
}

/// Refuse to land work on the default branch unless this run was told it may.
///
/// Checked before a lease is taken or a model started: `git checkout -B main` in a lease
/// would reset the trunk to whatever the slice was based on.
pub(crate) fn refuse_default_branch(branch: &str, trunk: &str, allowed: bool) -> Result<()> {
    if allowed || branch.trim() != trunk {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "this would build on {trunk} itself, and nothing lands on the default branch unless \
         the run was started with --on-default-branch"
    )))
}

/// Tie this checkout to the plan a run just made, and correct its bases.
///
/// Asking for the plan by name records it as the checkout's answer. On the run's fresh
/// branch that is the first answer the branch has ever had, so it sticks: the window and
/// `aip` in a terminal both resolve to it from here on, where before they kept answering
/// with whichever plan this checkout had resolved to most.
///
/// Bases are corrected while nothing has been built: a plan created on the run's branch,
/// and every slice copied from it, would otherwise target a branch that exists only to
/// coordinate. No trunk means nothing to correct them to, and they are left alone.
pub(crate) async fn adopt_plan(
    planner: &Planner,
    repo: &Path,
    slug: &str,
    trunk: Option<&git::Trunk>,
) -> Result<()> {
    let planner = planner.clone().for_plan(slug);
    planner.current().await?;
    let Some(trunk) = trunk else {
        return Ok(());
    };
    let run_branch = git::current_branch(repo).await;
    let run_branch = run_branch.as_deref();
    let base = planner
        .plans()
        .await?
        .into_iter()
        .find(|plan| plan.slug == slug)
        .and_then(|plan| plan.base_branch);
    if needs_trunk_base(base.as_deref(), &trunk.name, run_branch) {
        planner.set_plan_base(&trunk.name).await?;
    }
    for slice in planner.slices().await? {
        if needs_trunk_base(slice.base_branch.as_deref(), &trunk.name, run_branch) {
            planner.set_slice_base(&slice.key, &trunk.name).await?;
        }
    }
    Ok(())
}

/// Whether a base ai-planner recorded should be the default branch instead.
///
/// ai-planner bases a new plan, and every slice added to it, on whatever branch the
/// checkout is on - and a run's checkout is on its own fresh branch. That branch is where
/// the run coordinates, not something a pull request can target. An empty base means the
/// same. Anything else - another slice's branch, for a stack, or a trunk the planner
/// named on purpose - is the plan's to decide.
pub(crate) fn needs_trunk_base(base: Option<&str>, trunk: &str, run_branch: Option<&str>) -> bool {
    match base.map(str::trim).filter(|base| !base.is_empty()) {
        None => true,
        Some(base) => base != trunk && Some(base) == run_branch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// git with an identity and no signing, so a machine's own config cannot change what
    /// these tests see.
    fn git_in(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str) -> String {
        std::fs::write(dir.join(file), file).unwrap();
        git_in(dir, &["add", file]);
        git_in(dir, &["commit", "-qm", file]);
        git_in(dir, &["rev-parse", "HEAD"])
    }

    /// An `origin` with one commit on main, and a clone of it sitting on last plan's
    /// branch - the checkout a second run starts in.
    fn cloned() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        commit(&origin, "first");
        git_in(dir.path(), &["clone", "-q", "origin", "checkout"]);
        let checkout = dir.path().join("checkout");
        git_in(&checkout, &["checkout", "-qb", "ai-team/last-plan"]);
        commit(&checkout, "last-plan-work");
        (dir, origin, checkout)
    }

    #[tokio::test]
    async fn a_run_starts_on_a_fresh_branch_from_what_origin_has_now() {
        let (_dir, origin, checkout) = cloned();
        // Merged on the remote since this checkout last fetched.
        let merged = commit(&origin, "merged-since");

        let branched = branch_for_run(&checkout, 7).await.unwrap();

        assert_eq!(branched.branch, "ai-team/run-7");
        assert_eq!(branched.from, "origin/main");
        assert_eq!(branched.sha, merged);
        assert_eq!(branched.stale, None);
        assert_eq!(
            git::current_branch(&checkout).await.as_deref(),
            Some("ai-team/run-7")
        );
        // Nothing of the last plan comes along.
        assert!(!checkout.join("last-plan-work").exists());
        // No upstream, so a bare `git push` from here is not aimed at main.
        let upstream = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "ai-team/run-7@{upstream}"])
            .current_dir(&checkout)
            .output()
            .unwrap();
        assert!(!upstream.status.success());

        // A reused run id never resets the branch a deleted run left behind.
        git_in(&checkout, &["checkout", "-q", "ai-team/last-plan"]);
        let again = branch_for_run(&checkout, 7).await.unwrap();
        assert_eq!(again.branch, "ai-team/run-7-2");
    }

    #[tokio::test]
    async fn a_failed_fetch_is_reported_and_the_run_starts_from_what_it_has() {
        let (_dir, _origin, checkout) = cloned();
        let known = git_in(&checkout, &["rev-parse", "origin/main"]);
        git_in(
            &checkout,
            &["remote", "set-url", "origin", "/nowhere/that/exists"],
        );

        let branched = branch_for_run(&checkout, 3).await.unwrap();

        assert!(branched.stale.is_some());
        assert_eq!(branched.from, "origin/main");
        assert_eq!(branched.sha, known);
    }

    #[tokio::test]
    async fn a_repository_with_no_remote_branches_from_its_own_default() {
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "-q", "-b", "main"]);
        let first = commit(dir.path(), "first");
        git_in(dir.path(), &["checkout", "-qb", "feature"]);
        commit(dir.path(), "feature-work");

        let branched = branch_for_run(dir.path(), 1).await.unwrap();

        assert_eq!(branched.from, "main");
        assert_eq!(branched.sha, first);
        assert_eq!(branched.stale, None);
    }

    #[tokio::test]
    async fn uncommitted_work_is_refused_but_a_worktree_kept_inside_is_not_work() {
        let (_dir, _origin, checkout) = cloned();
        // Where Claude Code keeps the operator's own worktrees, unignored here.
        git_in(
            &checkout,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                ".claude/worktrees/side",
            ],
        );
        ensure_clean(&checkout).await.unwrap();

        std::fs::write(checkout.join("first"), "edited").unwrap();
        std::fs::create_dir_all(checkout.join("notes")).unwrap();
        std::fs::write(checkout.join("notes/todo.md"), "todo").unwrap();
        let refused = ensure_clean(&checkout).await.unwrap_err().to_string();
        assert!(refused.contains("first"), "{refused}");
        assert!(refused.contains("notes/todo.md"), "{refused}");
        assert!(!refused.contains(".claude"), "{refused}");
    }

    /// The real `aip`, against a scratch database.
    fn aip_in(dir: &Path, db: &Path, args: &[&str]) {
        let output = std::process::Command::new("aip")
            .arg("--db")
            .arg(db)
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "aip {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn a_run_builds_the_plan_it_made_not_the_one_its_checkout_remembers() {
        if std::process::Command::new("aip")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: `aip` is not on PATH");
            return;
        }
        let (dir, _origin, checkout) = cloned();
        let db = dir.path().join("planner.db");
        let planner = Planner::at(&checkout).with_db(&db);
        planner.ensure().await.unwrap();

        // The last run's plan, which this checkout has resolved to before.
        aip_in(
            &checkout,
            &db,
            &["new", "Last plan", "--slug", "last-plan", "--base", "main"],
        );
        for _ in 0..3 {
            aip_in(&checkout, &db, &["-p", "last-plan", "current"]);
        }
        // A planner in the same checkout creates a new plan. Asking the checkout which
        // plan it is on - what a run used to do - still answers with the old one.
        aip_in(&checkout, &db, &["new", "Stale", "--slug", "stale"]);
        assert_eq!(planner.current().await.unwrap().plan, "last-plan");

        // Now as a run does it: a fresh branch first, then the planner's plan and slices,
        // based - as `create_plan` and `add_slice` base them - on the branch it ran from.
        let branched = branch_for_run(&checkout, 9).await.unwrap();
        aip_in(&checkout, &db, &["new", "This run", "--slug", "this-run"]);
        aip_in(
            &checkout,
            &db,
            &["-p", "this-run", "slice", "add", "PR1", "first"],
        );
        aip_in(
            &checkout,
            &db,
            &[
                "-p",
                "this-run",
                "slice",
                "add",
                "PR2",
                "stacked",
                "--base",
                "ai-team/pr1",
            ],
        );
        let mine = planner.clone().for_plan("this-run");
        assert_eq!(
            mine.slices().await.unwrap()[0].base_branch.as_deref(),
            Some(branched.branch.as_str())
        );

        let trunk = git::trunk(&checkout).await.unwrap();
        adopt_plan(&planner, &checkout, "this-run", Some(&trunk))
            .await
            .unwrap();

        assert_eq!(planner.current().await.unwrap().plan, "this-run");
        let header = planner
            .plans()
            .await
            .unwrap()
            .into_iter()
            .find(|plan| plan.slug == "this-run")
            .unwrap();
        assert_eq!(header.base_branch.as_deref(), Some("main"));
        let bases: Vec<(String, Option<String>)> = mine
            .slices()
            .await
            .unwrap()
            .into_iter()
            .map(|slice| (slice.key, slice.base_branch))
            .collect();
        assert_eq!(
            bases,
            [
                ("PR1".to_string(), Some("main".to_string())),
                ("PR2".to_string(), Some("ai-team/pr1".to_string())),
            ]
        );
    }

    #[tokio::test]
    async fn working_on_the_default_branch_fast_forwards_it_and_nothing_else() {
        let (_dir, origin, checkout) = cloned();
        let merged = commit(&origin, "merged-since");

        // Asked from a checkout on another branch, it refuses rather than guess.
        let refused = stay_on_default_branch(&checkout).await.unwrap_err();
        assert!(
            refused.to_string().contains("ai-team/last-plan"),
            "{refused}"
        );

        git_in(&checkout, &["checkout", "-q", "main"]);
        let stayed = stay_on_default_branch(&checkout).await.unwrap();
        assert_eq!(stayed.branch, "main");
        assert_eq!(stayed.sha, merged);

        // A local main that has diverged is somebody's to sort out, not ours to merge.
        commit(&checkout, "local-only");
        commit(&origin, "remote-only");
        assert!(stay_on_default_branch(&checkout).await.is_err());
    }

    #[test]
    fn a_dirty_checkout_is_refused_with_the_paths_named() {
        let few = dirty_reason(Path::new("/w"), &["a.rs".into(), "b.rs".into()]);
        assert!(few.contains("a.rs, b.rs."), "{few}");
        assert!(few.contains("Commit or stash"), "{few}");

        let many: Vec<String> = (1..=8).map(|n| format!("f{n}")).collect();
        let reason = dirty_reason(Path::new("/w"), &many);
        assert!(reason.contains("f1, f2, f3, f4, f5 and 3 more"), "{reason}");
        assert!(!reason.contains("f6"), "{reason}");
    }

    #[test]
    fn nothing_lands_on_the_default_branch_unless_the_run_says_so() {
        assert!(refuse_default_branch("main", "main", false).is_err());
        assert!(refuse_default_branch(" main ", "main", false).is_err());
        assert!(refuse_default_branch("main", "main", true).is_ok());
        assert!(refuse_default_branch("ai-team/pr1", "main", false).is_ok());
        // `master` is only the default branch where it is the default branch.
        assert!(refuse_default_branch("master", "main", false).is_ok());
    }

    #[test]
    fn only_the_runs_own_branch_or_nothing_is_replaced_with_the_trunk() {
        let run = Some("ai-team/run-7");
        assert!(needs_trunk_base(None, "main", run));
        assert!(needs_trunk_base(Some("  "), "main", run));
        assert!(needs_trunk_base(Some("ai-team/run-7"), "main", run));
        assert!(!needs_trunk_base(Some("main"), "main", run));
        // A stack is the plan's structure, not a mistake to correct.
        assert!(!needs_trunk_base(Some("ai-team/pr1"), "main", run));
        assert!(!needs_trunk_base(Some("develop"), "main", run));
        // On the trunk itself there is nothing to correct.
        assert!(!needs_trunk_base(Some("main"), "main", Some("main")));
    }
}
