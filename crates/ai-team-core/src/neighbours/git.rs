//! Just enough git to make a node's work survive its worktree.
//!
//! A leased worktree is borrowed: `awt return` cleans and resets it, so anything left
//! uncommitted is gone the moment the lease ends. The worktrees `awt` hands out are git
//! worktrees of the same repository, sharing one object store, so a commit made on a
//! branch inside one is still there afterwards - and a branch is what a human reviews.
//!
//! Committing is the control plane's job rather than the agent's. A model that forgets,
//! or commits half of it, loses work silently; this always runs and always reports what
//! it did.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::error::{Error, Result};

/// Put a leased checkout on the semantic branch it will eventually land before the
/// model starts. Older callers use the lease's current HEAD as the base.
pub(crate) async fn prepare_branch(worktree: &Path, branch: &str) -> Result<()> {
    git(worktree, &["checkout", "-B", branch]).await.map(drop)
}

/// How a checkout was put on a PR's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placed {
    /// A new branch, cut from the base.
    Started,
    /// The branch already carried work of its own, and carries on from it.
    Continued,
}

/// Put a checkout on a PR's branch: carry on with it when it already holds work, start it
/// from `base` when it does not.
///
/// `checkout -B` resets a branch to its start point, and a PR's branch outlives any one
/// build - a review sends it back for more, a recovered run picks it up again. Resetting
/// it would throw its commits away. So a branch with commits of its own is checked out as
/// it is, and only a new one, or one holding nothing beyond its base, is cut from the
/// base. The base is explicit even for the default branch: a lease usually comes back on
/// it, and building from that incidental `HEAD` drops a stacked slice's predecessor.
pub(crate) async fn put_on_branch(worktree: &Path, branch: &str, base: &str) -> Result<Placed> {
    if branch_exists(worktree, branch).await {
        let ahead = git(
            worktree,
            &[
                "rev-list",
                "--count",
                &format!("{base}..refs/heads/{branch}"),
            ],
        )
        .await?;
        if ahead.trim().parse::<u64>().unwrap_or(0) > 0 {
            git(worktree, &["checkout", "--quiet", branch]).await?;
            return Ok(Placed::Continued);
        }
    }
    git(
        worktree,
        &["checkout", "--quiet", "--no-track", "-B", branch, base],
    )
    .await?;
    Ok(Placed::Started)
}

/// Commit everything in a worktree onto a branch.
///
/// `None` means there was nothing to commit, which is a real outcome worth reporting:
/// a node that finished without changing a file did not build its slice.
pub(crate) async fn commit_paths(
    worktree: &Path,
    branch: &str,
    message: &str,
    paths: &[String],
) -> Result<Option<String>> {
    if paths.is_empty() {
        return Ok(None);
    }

    // Older callers may not have prepared the branch. Do not reset a prepared checkout
    // here: it already carries the model's uncommitted edits.
    if current_branch(worktree).await.as_deref() != Some(branch) {
        prepare_branch(worktree, branch).await?;
    }
    let mut add = vec!["add", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(worktree, &add).await?;

    // A path the agent deleted and the gates rebuilt, or one already matching HEAD, can
    // leave nothing staged. That is the same "nothing to commit" outcome.
    if git(worktree, &["diff", "--cached", "--name-only"])
        .await?
        .trim()
        .is_empty()
    {
        return Ok(None);
    }
    // The committer is ai-team, not whoever's name is in the repo's config: a human
    // reading `git log` should be able to tell which commits they did not write.
    git(
        worktree,
        &[
            "-c",
            "user.name=ai-team",
            "-c",
            "user.email=ai-team@localhost",
            "commit",
            "--no-verify",
            "-m",
            message,
        ],
    )
    .await?;
    let sha = git(worktree, &["rev-parse", "HEAD"]).await?;
    Ok(Some(sha.trim().to_string()))
}

/// The paths a turn changed.
///
/// Taken immediately after a turn and before the gates run, because running the gates
/// changes the worktree too: `cargo test` writes `target/` and `npm test` writes wherever
/// it likes. A repo without a `.gitignore` covering them would otherwise get a review
/// branch carrying a hundred files of build output. What ai-team commits is what the
/// agent did, not what checking it produced.
pub(crate) async fn changed_paths(worktree: &Path) -> Result<Vec<String>> {
    Ok(porcelain_paths(&porcelain(worktree).await?))
}

/// The paths in `git status --porcelain` output.
fn porcelain_paths(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| {
            // `XY path`, and for a rename `XY old -> new`; the new name is what to add.
            let path = line.get(3..)?.trim();
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            let path = path.trim_matches('"');
            (!path.is_empty()).then(|| path.to_string())
        })
        .collect()
}

/// What a checkout holds before a turn: the commit it is on, and what every path that
/// differs from it holds - a blob hash, or `None` for a deletion. So what the turn changed
/// can be told apart from what was already there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// `None` when it was not taken - after an interrupted process, say - and whether the
    /// seat committed anything itself cannot be known.
    head: Option<String>,
    paths: std::collections::BTreeMap<String, Option<String>>,
}

pub(crate) async fn snapshot(worktree: &Path) -> Result<Snapshot> {
    let head = rev_parse(worktree, "HEAD").await.ok();
    let status = git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;
    // A trailing slash is a nested repository or worktree: a checkout, not a file.
    let listed: Vec<String> = porcelain_paths(&status)
        .into_iter()
        .filter(|path| !path.ends_with('/'))
        .collect();
    let present: Vec<&str> = listed
        .iter()
        .map(String::as_str)
        .filter(|path| worktree.join(path).is_file())
        .collect();
    let hashes = hash_objects(worktree, &present).await?;
    let mut paths: std::collections::BTreeMap<String, Option<String>> = present
        .iter()
        .zip(hashes)
        .map(|(path, hash)| ((*path).to_string(), Some(hash)))
        .collect();
    for path in listed {
        paths.entry(path).or_insert(None);
    }
    Ok(Snapshot { head, paths })
}

/// What one turn did to a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Changes {
    /// Paths whose content differs from what the snapshot recorded.
    pub(crate) paths: Vec<String>,
    /// Whether the checkout's own commit moved: the seat committed something itself.
    pub(crate) committed: bool,
}

/// What changed since `before`: uncommitted paths whose content is different, and whether
/// the seat committed work of its own.
///
/// What one turn changed, and nothing else: the gates leave output behind that no ignore
/// file covers, and committing that as the seat's work would be ai-team's mess on
/// somebody's pull request (rule 9). A path the turn put back to `HEAD` is not listed -
/// there is nothing of it left to commit. A seat is told not to commit, but one that does
/// has still built something, and must not read as a turn that changed nothing.
pub(crate) async fn changed_since(worktree: &Path, before: &Snapshot) -> Result<Changes> {
    let now = snapshot(worktree).await?;
    let committed = match (&before.head, &now.head) {
        (Some(before), Some(now)) => before != now,
        _ => false,
    };
    Ok(Changes {
        paths: now
            .paths
            .into_iter()
            .filter(|(path, hash)| before.paths.get(path) != Some(hash))
            .map(|(path, _)| path)
            .collect(),
        committed,
    })
}

/// Blob hashes for files, in the order given. Paths go in on stdin rather than as
/// arguments: a turn that leaves a few thousand untracked files behind would otherwise
/// overflow the command line.
async fn hash_objects(worktree: &Path, paths: &[&str]) -> Result<Vec<String>> {
    use tokio::io::AsyncWriteExt as _;

    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin-paths"])
        .current_dir(worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| Error::invalid(format!("could not run git: {error}")))?;
    let mut input = paths.join("\n");
    input.push('\n');
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::invalid("git hash-object took no input"))?;
    // Written from its own task: git answers as it reads, and a full stdout pipe would
    // otherwise stop it reading before all the paths are in.
    let writer = tokio::spawn(async move {
        let written = stdin.write_all(input.as_bytes()).await;
        drop(stdin);
        written
    });
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| Error::invalid(format!("could not run git: {error}")))?;
    writer
        .await
        .map_err(|error| Error::invalid(format!("writing to git hash-object: {error}")))?
        .map_err(|error| Error::invalid(format!("writing to git hash-object: {error}")))?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "`git hash-object` failed in {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let hashes: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    if hashes.len() != paths.len() {
        return Err(Error::invalid(format!(
            "`git hash-object` answered {} of {} paths",
            hashes.len(),
            paths.len()
        )));
    }
    Ok(hashes)
}

/// What a person has changed and not committed, untracked files included.
///
/// A run starts its checkout on a fresh branch, and the agents commit whatever changed:
/// anything left here would be switched along with the branch and then committed into
/// somebody's pull request as if an agent had written it. Untracked files are listed one
/// by one, so a linked worktree kept inside this checkout - `.claude/worktrees/<name>`
/// is where Claude Code puts them - is recognised as a checkout rather than as work.
pub(crate) async fn uncommitted(worktree: &Path) -> Result<Vec<String>> {
    let status = git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;
    let here = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let nested: Vec<String> = worktrees(worktree)
        .await?
        .into_iter()
        .filter_map(|(path, _)| {
            let path = Path::new(&path).canonicalize().ok()?;
            let inside = path.strip_prefix(&here).ok()?;
            (!inside.as_os_str().is_empty()).then(|| format!("{}/", inside.to_string_lossy()))
        })
        .collect();
    Ok(porcelain_paths(&status)
        .into_iter()
        .filter(|path| !nested.contains(path))
        .collect())
}

/// A repository's trunk: what plans and pull requests are based on, and where new work
/// is cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Trunk {
    /// The short name, `main`. What a plan's base and a pull request's base are called.
    pub name: String,
    /// What a fresh branch starts from: `origin/main` once the remote has one, because a
    /// local `main` is only as current as the last time somebody pulled it.
    pub start_point: String,
}

pub(crate) async fn trunk(worktree: &Path) -> Result<Trunk> {
    let found = default_branch(worktree).await.ok_or_else(|| {
        Error::invalid(format!(
            "could not tell which branch is the default in {}: there is no origin/HEAD, \
             main or master",
            worktree.display()
        ))
    })?;
    let name = found.strip_prefix("origin/").unwrap_or(&found).to_string();
    let remote = format!("origin/{name}");
    let start_point = if git(
        worktree,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}"),
        ],
    )
    .await
    .is_ok_and(|sha| !sha.trim().is_empty())
    {
        remote
    } else {
        name.clone()
    };
    Ok(Trunk { name, start_point })
}

/// How long a fetch may take before the run goes ahead without it.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Bring `origin/<branch>` up to date. `Ok(false)` means there is no `origin` to ask.
///
/// Never prompts: a credential or passphrase prompt would wait on a terminal nobody is
/// watching, which for a scheduled run is forever. A slow network gets a minute.
pub(crate) async fn fetch(worktree: &Path, branch: &str) -> Result<bool> {
    if git(worktree, &["remote", "get-url", "origin"])
        .await
        .is_err()
    {
        return Ok(false);
    }
    let fetched = Command::new("git")
        .args(["fetch", "--quiet", "origin", branch])
        .current_dir(worktree)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(FETCH_TIMEOUT, fetched)
        .await
        .map_err(|_| {
            Error::invalid(format!(
                "`git fetch origin {branch}` took longer than {}s",
                FETCH_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|error| Error::invalid(format!("could not run git: {error}")))?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "`git fetch origin {branch}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(true)
}

pub(crate) async fn branch_exists(worktree: &Path, branch: &str) -> bool {
    git(
        worktree,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .is_ok_and(|sha| !sha.trim().is_empty())
}

/// Move the checked-out branch forward to `target`, refusing anything but a fast-forward.
pub(crate) async fn fast_forward(worktree: &Path, target: &str) -> Result<()> {
    git(worktree, &["merge", "--ff-only", "--quiet", target])
        .await
        .map(drop)
}

/// Put a checkout on a new branch cut from `start_point`, and say which commit that is.
///
/// `--no-track`, because a branch cut from `origin/main` would otherwise take `main` as
/// its upstream, and a bare `git push` from it would then be aimed at the trunk.
pub(crate) async fn start_branch(
    worktree: &Path,
    branch: &str,
    start_point: &str,
) -> Result<String> {
    git(
        worktree,
        &[
            "checkout",
            "--quiet",
            "--no-track",
            "-b",
            branch,
            start_point,
        ],
    )
    .await?;
    rev_parse(worktree, "HEAD").await
}

/// What changed, in `git status --porcelain` form. Empty means a clean tree.
///
/// Only the trailing newline is trimmed. A porcelain line is `XY path`, and an unstaged
/// modification's X is a space - so trimming the front eats it, and every path then
/// starts one character late.
pub(crate) async fn porcelain(worktree: &Path) -> Result<String> {
    Ok(git(worktree, &["status", "--porcelain"])
        .await?
        .trim_end()
        .to_string())
}

/// Keep the gates' own output out of git's view, for this lease only.
///
/// `.git/info/exclude` is git's per-checkout ignore file: it is not tracked, so this
/// does not edit the repository, and the worktree is returned to the pool afterwards
/// anyway. A repo whose own .gitignore already covers these loses nothing.
pub(crate) async fn ignore_build_output(worktree: &Path) -> Result<()> {
    let info = git(worktree, &["rev-parse", "--git-path", "info/exclude"]).await?;
    let path = worktree.join(info.trim());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains("# ai-team") {
        return Ok(());
    }
    let added = format!(
        "{existing}\n# ai-team: what running this project's checks leaves behind.\n\
         target/\nnode_modules/\nvendor/\ndist/\n.output/\n.eve/\n"
    );
    std::fs::write(&path, added)
        .map_err(|error| Error::invalid(format!("could not write {}: {error}", path.display())))?;
    Ok(())
}

/// The commit a branch forked from, which is what a review should be measured against.
///
/// Diffing a branch against the *tip* of main shows every change anybody else landed in
/// the meantime as though the agent had made it. The fork point shows only its work.
pub(crate) async fn merge_base(worktree: &Path, base: &str, head: &str) -> Result<String> {
    Ok(git(worktree, &["merge-base", base, head])
        .await?
        .trim()
        .to_string())
}

pub(crate) async fn rev_parse(worktree: &Path, rev: &str) -> Result<String> {
    Ok(git(worktree, &["rev-parse", rev]).await?.trim().to_string())
}

/// Whether `ancestor` is in `descendant`'s history.
pub(crate) async fn is_ancestor(worktree: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| Error::invalid(format!("could not run git: {error}")))?;
    // 1 is git's "no"; anything else that is not 0 is git failing to answer.
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::invalid(format!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

/// The commit of `parent` that `branch` was built on: where its own work begins.
///
/// `--fork-point` reads the parent's reflog, so a parent rewritten since - restacked in
/// its turn, or amended - still gives the commit this branch sits on. A plain merge-base
/// would reach back past the parent's old commits to where the two last agreed, and a
/// rebase from there replays those old commits as this branch's own. A parent with no
/// reflog entry to go on falls back to the merge-base, which is right for any parent
/// that only ever moved forward.
pub(crate) async fn fork_point(worktree: &Path, parent: &str, branch: &str) -> Result<String> {
    match git(worktree, &["merge-base", "--fork-point", parent, branch]).await {
        Ok(found) if !found.trim().is_empty() => Ok(found.trim().to_string()),
        _ => merge_base(worktree, parent, branch).await,
    }
}

/// How many commits `to` has that `from` does not.
pub(crate) async fn commits_between(worktree: &Path, from: &str, to: &str) -> Result<u64> {
    let count = git(worktree, &["rev-list", "--count", &format!("{from}..{to}")]).await?;
    count
        .trim()
        .parse()
        .map_err(|_| Error::invalid(format!("git counted `{}` commits", count.trim())))
}

/// Whether a rebase stopped part-way in this checkout, on a conflict or a question.
///
/// Asked of git rather than of a path: a linked worktree keeps its rebase state under the
/// main repository's `.git/worktrees/<name>`, not beside its files.
pub(crate) async fn rebase_in_progress(worktree: &Path) -> Result<bool> {
    for state in ["rebase-merge", "rebase-apply"] {
        let path = git(worktree, &["rev-parse", "--git-path", state]).await?;
        if worktree.join(path.trim()).exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Put a checkout back as it was before a rebase that did not finish.
pub(crate) async fn abort_rebase(worktree: &Path) -> Result<()> {
    git(worktree, &["rebase", "--abort"]).await.map(drop)
}

/// The branch work is cut from, as this repository actually names it.
///
/// Tried in order rather than assumed: `origin/HEAD` is what the remote says, and the two
/// fallbacks cover a repo with no remote. Returning `None` is a real answer - a checkout
/// with none of them has nothing to measure a branch against.
pub(crate) async fn default_branch(worktree: &Path) -> Option<String> {
    if let Ok(name) = git(
        worktree,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        let name = name.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    for candidate in ["main", "master"] {
        if git(worktree, &["rev-parse", "--verify", "--quiet", candidate])
            .await
            .is_ok_and(|sha| !sha.trim().is_empty())
        {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Which branch this checkout is on, or `None` on a detached head.
pub async fn current_branch(worktree: &Path) -> Option<String> {
    let name = git(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()?
        .trim()
        .to_string();
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// The diff from a commit to what is on disk right now, uncommitted edits included.
///
/// A local review can show work in progress, which a hosted one cannot - there is no
/// working tree on a server. Used when the checkout is sitting on the branch under
/// review, so a human who edits a file by hand sees the effect without committing first.
pub(crate) async fn diff_worktree(worktree: &Path, base: &str) -> Result<String> {
    git(
        worktree,
        &["diff", "--no-color", "--find-renames", "--unified=3", base],
    )
    .await
}

/// The unified diff between two commits.
///
/// `--no-color` because a configured `color.ui = always` would otherwise wrap every line
/// in escape codes and the parser would read them as content. `--find-renames` so a moved
/// file reads as a move rather than as a whole file deleted and another one written.
pub(crate) async fn diff(worktree: &Path, base: &str, head: &str) -> Result<String> {
    git(
        worktree,
        &[
            "diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            base,
            head,
        ],
    )
    .await
}

/// What is changed here, unstaged and staged.
///
/// Two diffs rather than one: `git diff` is the worktree against the index and
/// `git diff --cached` is the index against HEAD. A surface that shows only their sum
/// cannot tell you what pressing commit would actually record.
pub async fn worktree_diff(worktree: &Path) -> Result<String> {
    git(
        worktree,
        &["diff", "--no-color", "--find-renames", "--unified=3"],
    )
    .await
}

pub async fn staged_diff(worktree: &Path) -> Result<String> {
    git(
        worktree,
        &[
            "diff",
            "--cached",
            "--no-color",
            "--find-renames",
            "--unified=3",
        ],
    )
    .await
}

/// Files git does not know about yet.
///
/// Untracked files have no diff at all, so they would be invisible in a view built only
/// from `git diff` - which is how a new file an agent wrote gets left out of a commit.
pub async fn untracked(worktree: &Path) -> Result<Vec<String>> {
    Ok(
        git(worktree, &["ls-files", "--others", "--exclude-standard"])
            .await?
            .lines()
            .map(str::to_string)
            .filter(|line| !line.is_empty())
            .collect(),
    )
}

pub async fn stage(worktree: &Path, path: &str) -> Result<()> {
    git(worktree, &["add", "--", path]).await.map(|_| ())
}

pub async fn unstage(worktree: &Path, path: &str) -> Result<()> {
    // `restore --staged` rather than `reset`, because on a repository with no commits yet
    // there is no HEAD to reset against and the whole view would fail on a fresh repo.
    git(worktree, &["restore", "--staged", "--", path])
        .await
        .map(|_| ())
}

/// Stage exactly one hunk by feeding git a patch containing only that hunk.
///
/// `git apply --cached` is the only honest way to do this: reconstructing the file and
/// writing it would stage the human's *other* unstaged edits to the same file along with
/// the hunk they picked, which is precisely what per-hunk staging exists to avoid.
///
/// `--unidiff-zero` is not passed: the patch carries its context lines, and git verifying
/// that context is what stops a stale hunk applying in the wrong place after the file has
/// moved on underneath the view.
pub async fn apply_cached(worktree: &Path, patch: &str) -> Result<()> {
    let mut child = Command::new("git")
        .args(["apply", "--cached", "-"])
        .current_dir(worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| Error::invalid(format!("could not run git apply: {error}")))?;

    {
        use tokio::io::AsyncWriteExt as _;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::invalid("git apply took no stdin"))?;
        stdin
            .write_all(patch.as_bytes())
            .await
            .map_err(|error| Error::invalid(format!("writing the patch: {error}")))?;
        // Dropped here on purpose: git apply reads until EOF, and holding the pipe open
        // would wait forever for a patch that is already complete.
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|error| Error::invalid(format!("git apply: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    // Almost always means the index already has it: the window polls, so two clicks on
    // stage, or one click against a view that has not caught up, both land here. Saying
    // "the file changed" would be wrong - `--cached` applies to the index, and editing
    // the worktree afterwards does not invalidate a patch.
    Err(Error::invalid(format!(
        "that hunk does not apply - it is probably already staged ({})",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Commit what is staged.
pub async fn commit(worktree: &Path, message: &str) -> Result<String> {
    if message.trim().is_empty() {
        return Err(Error::invalid("a commit needs a message"));
    }
    // `-F -` would need another pipe; `-m` twice is how git takes a subject and a body,
    // and splitting on the blank line is what the author already typed.
    let (subject, body) = message
        .trim()
        .split_once("\n\n")
        .unwrap_or((message.trim(), ""));
    let mut args = vec!["commit", "-m", subject];
    if !body.trim().is_empty() {
        args.push("-m");
        args.push(body.trim());
    }
    git(worktree, &args).await?;
    rev_parse(worktree, "HEAD").await
}

/// Every worktree of this repository, as `(path, branch)`.
///
/// `--porcelain` rather than the human listing, which aligns columns and puts the branch
/// in square brackets - a format to parse rather than read, and one git is free to change.
/// The porcelain form is a stanza per worktree, blank-line separated, with `worktree` and
/// `branch` on their own lines.
///
/// A detached worktree is still a workspace. Its branch is absent rather than invented,
/// but its path must remain in the result or the window would make a real checkout
/// impossible to select.
pub(crate) async fn worktrees(repo: &Path) -> Result<Vec<(String, Option<String>)>> {
    let listed = git(repo, &["worktree", "list", "--porcelain"]).await?;
    let mut found = Vec::new();
    let mut path: Option<String> = None;
    let mut branch: Option<String> = None;
    for line in listed.lines().chain(std::iter::once("")) {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(previous) = path.replace(rest.trim().to_string()) {
                found.push((previous, branch.take()));
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            // `refs/heads/chore/review-7223` -> `chore/review-7223`. Stripped by prefix
            // rather than by taking the last segment, which would turn a branch with a
            // slash in it into its final word - and most branches here have one.
            branch = Some(
                rest.trim()
                    .strip_prefix("refs/heads/")
                    .unwrap_or(rest.trim())
                    .to_string(),
            );
        } else if line.trim().is_empty() {
            if let Some(path) = path.take() {
                found.push((path, branch.take()));
            }
        }
    }
    Ok(found)
}

/// Every local branch, and which one is checked out.
pub async fn branches(worktree: &Path) -> Result<Vec<String>> {
    Ok(git(
        worktree,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .await?
    .lines()
    .map(str::to_string)
    .filter(|line| !line.is_empty())
    .collect())
}

pub async fn checkout(worktree: &Path, branch: &str) -> Result<()> {
    git(worktree, &["checkout", branch]).await.map(|_| ())
}

/// Push an exact accepted branch from any checkout of the repository. A returned maker
/// lease is detached, so delivery must not depend on whichever branch that path shows.
///
/// `replaces` is for a branch that was rewritten - restacked onto a moved parent - and so
/// cannot fast-forward: the remote commit it was rewritten from, and the only one it may
/// replace. Pinned to that commit rather than left to `origin/<branch>`, because fetching
/// moves `origin/<branch>`, and a lease checked against it would agree to replace a commit
/// somebody pushed since.
pub(crate) async fn push_branch(
    worktree: &Path,
    branch: &str,
    replaces: Option<&str>,
) -> Result<String> {
    let lease = replaces.map(|sha| format!("--force-with-lease=refs/heads/{branch}:{sha}"));
    let mut args = vec!["push", "--set-upstream"];
    args.extend(lease.as_deref());
    args.extend(["origin", branch]);
    git(worktree, &args).await
}

/// Push the current branch, setting upstream if it has none.
///
/// Publishing is not a *node's* call - `irreversible.ts` refuses this from a generated
/// agent - but it is squarely the operator's, and this is their surface.
pub async fn push(worktree: &Path) -> Result<String> {
    let branch = current_branch(worktree)
        .await
        .ok_or_else(|| Error::invalid("a detached head has no branch to push"))?;
    git(worktree, &["push", "--set-upstream", "origin", &branch]).await
}

async fn git(worktree: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| Error::invalid(format!("could not run git: {error}")))?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "."],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git(dir.path(), &args).await.unwrap();
        }
        std::fs::write(dir.path().join("README.md"), "start\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "init"]).await.unwrap();
        dir
    }

    #[tokio::test]
    async fn a_slice_has_its_meaningful_branch_before_the_agent_starts() {
        let dir = repo().await;

        prepare_branch(dir.path(), "ai-team/S1").await.unwrap();

        assert_eq!(
            current_branch(dir.path()).await.as_deref(),
            Some("ai-team/S1")
        );
        std::fs::write(dir.path().join("new.txt"), "work\n").unwrap();
        let paths = changed_paths(dir.path()).await.unwrap();
        commit_paths(dir.path(), "ai-team/S1", "S1: work", &paths)
            .await
            .unwrap();
        assert_eq!(
            current_branch(dir.path()).await.as_deref(),
            Some("ai-team/S1")
        );
    }

    #[tokio::test]
    async fn a_stacked_slice_starts_from_its_declared_base() {
        let dir = repo().await;
        let original = current_branch(dir.path()).await.unwrap();
        git(dir.path(), &["checkout", "-qb", "feature/first"])
            .await
            .unwrap();
        std::fs::write(dir.path().join("first.txt"), "first\n").unwrap();
        git(dir.path(), &["add", "first.txt"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "first slice"])
            .await
            .unwrap();
        let base = git(dir.path(), &["rev-parse", "HEAD"])
            .await
            .unwrap()
            .trim()
            .to_string();
        git(dir.path(), &["checkout", "-q", &original])
            .await
            .unwrap();

        let placed = put_on_branch(dir.path(), "feature/second", "feature/first")
            .await
            .unwrap();

        assert_eq!(placed, Placed::Started);
        assert_eq!(
            current_branch(dir.path()).await.as_deref(),
            Some("feature/second")
        );
        assert_eq!(
            git(dir.path(), &["rev-parse", "HEAD"])
                .await
                .unwrap()
                .trim(),
            base
        );
        assert!(dir.path().join("first.txt").exists());
    }

    #[tokio::test]
    async fn a_branch_that_already_carries_work_is_continued_not_reset() {
        let dir = repo().await;
        let original = current_branch(dir.path()).await.unwrap();
        put_on_branch(dir.path(), "ai-team/pr1", &original)
            .await
            .unwrap();
        std::fs::write(dir.path().join("built.txt"), "the first build\n").unwrap();
        git(dir.path(), &["add", "built.txt"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "PR1 T1: build"])
            .await
            .unwrap();
        let built = rev_parse(dir.path(), "HEAD").await.unwrap();
        git(dir.path(), &["checkout", "-q", &original])
            .await
            .unwrap();

        // Sent back for more - by a review, or a recovered run. Its commits stay.
        let placed = put_on_branch(dir.path(), "ai-team/pr1", &original)
            .await
            .unwrap();
        assert_eq!(placed, Placed::Continued);
        assert_eq!(rev_parse(dir.path(), "HEAD").await.unwrap(), built);

        // A branch with nothing of its own is simply started again from the base.
        git(dir.path(), &["checkout", "-q", &original])
            .await
            .unwrap();
        git(dir.path(), &["branch", "-q", "ai-team/empty"])
            .await
            .unwrap();
        let placed = put_on_branch(dir.path(), "ai-team/empty", &original)
            .await
            .unwrap();
        assert_eq!(placed, Placed::Started);
    }

    #[tokio::test]
    async fn work_lands_on_a_branch_that_outlives_the_worktree() {
        let dir = repo().await;
        std::fs::write(dir.path().join("new.txt"), "work\n").unwrap();

        let paths = changed_paths(dir.path()).await.unwrap();
        assert_eq!(paths, ["new.txt"]);
        let sha = commit_paths(dir.path(), "ai-team/PR1", "PR1: do the thing", &paths)
            .await
            .unwrap()
            .expect("something changed, so something was committed");
        assert_eq!(sha.len(), 40, "a full sha: {sha}");

        // The branch exists and carries the message, which is what review reads.
        let log = git(
            dir.path(),
            &["log", "-1", "--pretty=%s%n%an", "ai-team/PR1"],
        )
        .await
        .unwrap();
        assert!(log.contains("PR1: do the thing"), "{log}");
        assert!(
            log.contains("ai-team"),
            "the author says who wrote it: {log}"
        );
        assert!(porcelain(dir.path()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_node_that_changed_nothing_says_so_rather_than_committing_air() {
        // An empty commit would put a green branch on the board for work nobody did.
        let dir = repo().await;
        assert_eq!(
            commit_paths(dir.path(), "ai-team/PR2", "PR2: nothing", &[])
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn an_unstaged_modification_keeps_the_first_letter_of_its_path() {
        // `git status --porcelain` writes ` M path` for an unstaged edit. Trimming the
        // whole output eats that leading space, and every path then starts one character
        // late - which reaches git as `add -- rates/widget/src/lib.rs`.
        let dir = repo().await;
        std::fs::create_dir_all(dir.path().join("crates/widget/src")).unwrap();
        std::fs::write(dir.path().join("crates/widget/src/lib.rs"), "one\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "add lib"])
            .await
            .unwrap();

        std::fs::write(dir.path().join("crates/widget/src/lib.rs"), "two\n").unwrap();
        assert_eq!(
            changed_paths(dir.path()).await.unwrap(),
            ["crates/widget/src/lib.rs"]
        );
    }

    #[tokio::test]
    async fn a_renamed_file_is_added_under_the_name_it_now_has() {
        let dir = repo().await;
        std::fs::write(dir.path().join("old.txt"), "content\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-qm", "add old"])
            .await
            .unwrap();
        git(dir.path(), &["mv", "old.txt", "new.txt"])
            .await
            .unwrap();

        let paths = changed_paths(dir.path()).await.unwrap();
        assert!(paths.contains(&"new.txt".to_string()), "{paths:?}");
    }

    #[tokio::test]
    async fn the_gates_own_output_is_excluded_for_this_lease_only() {
        let dir = repo().await;
        ignore_build_output(dir.path()).await.unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/out.bin"), "junk").unwrap();
        std::fs::write(dir.path().join("real.rs"), "work\n").unwrap();

        assert_eq!(changed_paths(dir.path()).await.unwrap(), ["real.rs"]);
        // Written to .git/info/exclude, so the repository's own files are untouched.
        assert!(!dir.path().join(".gitignore").exists());

        // Running it twice must not keep appending.
        ignore_build_output(dir.path()).await.unwrap();
        let exclude =
            std::fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap_or_default();
        assert_eq!(exclude.matches("# ai-team").count(), 1);
    }

    #[tokio::test]
    async fn build_output_from_checking_the_work_is_not_committed_with_it() {
        // The gates run in the worktree, so `cargo test` leaves `target/` behind. A repo
        // whose .gitignore does not cover it would otherwise get a review branch with a
        // hundred files of build output on it, which is ai-team's mess, not the agent's.
        let dir = repo().await;
        std::fs::write(dir.path().join("src.rs"), "the agent's work\n").unwrap();

        // Captured after the turn, before the gates run.
        let paths = changed_paths(dir.path()).await.unwrap();

        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/out.bin"), "junk").unwrap();

        commit_paths(dir.path(), "ai-team/PR4", "PR4: work", &paths)
            .await
            .unwrap()
            .unwrap();

        let files = git(
            dir.path(),
            &["show", "--name-only", "--pretty=", "ai-team/PR4"],
        )
        .await
        .unwrap();
        assert!(files.contains("src.rs"), "{files}");
        assert!(
            !files.contains("target/"),
            "build output leaked in: {files}"
        );
    }

    /// Commit `file` with `text` on whatever `dir` has checked out, and say which commit.
    async fn commit_file(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", file]).await.unwrap();
        git(dir, &["commit", "-qm", file]).await.unwrap();
        rev_parse(dir, "HEAD").await.unwrap()
    }

    /// `main` with PR1 on it, and PR2 built on PR1: the stack a restack starts from.
    async fn stack() -> (tempfile::TempDir, String) {
        let dir = repo().await;
        git(dir.path(), &["branch", "-M", "main"]).await.unwrap();
        git(dir.path(), &["checkout", "-qb", "p/pr1"])
            .await
            .unwrap();
        let built_on = commit_file(dir.path(), "one.txt", "one\n").await;
        git(dir.path(), &["checkout", "-qb", "p/pr2"])
            .await
            .unwrap();
        commit_file(dir.path(), "two.txt", "two\n").await;
        (dir, built_on)
    }

    #[tokio::test]
    async fn a_stacked_branch_knows_the_parent_commit_it_was_built_on_after_the_parent_moves() {
        let (dir, built_on) = stack().await;
        let dir = dir.path();

        // Moved on: review follow-ups on PR1.
        git(dir, &["checkout", "-q", "p/pr1"]).await.unwrap();
        let moved = commit_file(dir, "one-more.txt", "more\n").await;
        assert_eq!(fork_point(dir, "p/pr1", "p/pr2").await.unwrap(), built_on);
        assert!(!is_ancestor(dir, &moved, "p/pr2").await.unwrap());
        assert!(is_ancestor(dir, &built_on, "p/pr2").await.unwrap());

        // Rewritten: main moved and PR1 was rebased onto it, the commit PR2 sits on
        // included. A plain merge-base now reaches back to main, and a rebase from there
        // would replay PR1's old commits into PR2 as though they were its own.
        git(dir, &["checkout", "-q", "main"]).await.unwrap();
        let trunk = commit_file(dir, "main.txt", "main moved\n").await;
        git(dir, &["checkout", "-q", "p/pr1"]).await.unwrap();
        git(dir, &["rebase", "-q", "main"]).await.unwrap();
        assert!(!is_ancestor(dir, &built_on, "p/pr1").await.unwrap());
        assert_ne!(merge_base(dir, "p/pr1", "p/pr2").await.unwrap(), built_on);
        assert_eq!(fork_point(dir, "p/pr1", "p/pr2").await.unwrap(), built_on);
        assert!(is_ancestor(dir, &trunk, "p/pr1").await.unwrap());
    }

    #[tokio::test]
    async fn a_rebase_left_part_way_is_seen_and_undone() {
        let (dir, _) = stack().await;
        let dir = dir.path();
        git(dir, &["checkout", "-q", "main"]).await.unwrap();
        commit_file(dir, "two.txt", "main's two\n").await;
        git(dir, &["checkout", "-q", "p/pr2"]).await.unwrap();
        let before = rev_parse(dir, "HEAD").await.unwrap();
        assert!(!rebase_in_progress(dir).await.unwrap());

        // Both sides wrote two.txt: git stops on it.
        assert!(git(dir, &["rebase", "main"]).await.is_err());
        assert!(rebase_in_progress(dir).await.unwrap());

        abort_rebase(dir).await.unwrap();
        assert!(!rebase_in_progress(dir).await.unwrap());
        assert_eq!(rev_parse(dir, "HEAD").await.unwrap(), before);
        assert_eq!(current_branch(dir).await.as_deref(), Some("p/pr2"));
    }

    #[tokio::test]
    async fn a_rewritten_branch_replaces_only_the_remote_commit_it_was_rewritten_from() {
        let (dir, _) = stack().await;
        let dir = dir.path();
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare", "."])
            .await
            .unwrap();
        git(
            dir,
            &["remote", "add", "origin", &origin.path().to_string_lossy()],
        )
        .await
        .unwrap();
        push_branch(dir, "p/pr2", None).await.unwrap();
        let published = rev_parse(dir, "origin/p/pr2").await.unwrap();

        // Restacked: the same work on a new base, so not a fast-forward.
        git(dir, &["rebase", "-q", "--onto", "main", "p/pr1"])
            .await
            .unwrap();
        push_branch(dir, "p/pr2", Some(&published)).await.unwrap();
        let restacked = rev_parse(dir, "HEAD").await.unwrap();
        assert_eq!(rev_parse(dir, "origin/p/pr2").await.unwrap(), restacked);

        // Somebody pushed since the commit this rewrite was pinned to. Theirs survives.
        let theirs = tempfile::tempdir().unwrap();
        git(
            theirs.path(),
            &[
                "clone",
                "-q",
                "--branch",
                "p/pr2",
                &origin.path().to_string_lossy(),
                ".",
            ],
        )
        .await
        .unwrap();
        git(theirs.path(), &["config", "user.email", "o@o"])
            .await
            .unwrap();
        git(theirs.path(), &["config", "user.name", "o"])
            .await
            .unwrap();
        let their_commit = commit_file(theirs.path(), "theirs.txt", "theirs\n").await;
        git(theirs.path(), &["push", "-q", "origin", "p/pr2"])
            .await
            .unwrap();
        git(dir, &["commit", "-q", "--amend", "-m", "rewritten again"])
            .await
            .unwrap();
        // The fetch a watch does moves origin/p/pr2 to theirs, which is exactly what an
        // unpinned lease would then have agreed to replace.
        fetch(dir, "p/pr2").await.unwrap();

        let refused = push_branch(dir, "p/pr2", Some(&restacked)).await;

        assert!(
            refused.is_err(),
            "a push over somebody else's commit went through"
        );
        git(theirs.path(), &["fetch", "-q", "origin"])
            .await
            .unwrap();
        assert_eq!(
            rev_parse(theirs.path(), "origin/p/pr2").await.unwrap(),
            their_commit
        );
    }

    #[tokio::test]
    async fn re_running_a_slice_reuses_its_branch_instead_of_failing_on_it() {
        let dir = repo().await;
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let paths = changed_paths(dir.path()).await.unwrap();
        commit_paths(dir.path(), "ai-team/PR3", "first", &paths)
            .await
            .unwrap();

        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let paths = changed_paths(dir.path()).await.unwrap();
        let second = commit_paths(dir.path(), "ai-team/PR3", "second", &paths)
            .await
            .unwrap();
        assert!(second.is_some(), "a retry must not fail on its own branch");
    }
}
