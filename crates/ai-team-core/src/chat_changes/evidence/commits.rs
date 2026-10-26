use std::{path::Path, time::Duration};

use super::{capture, BorrowedFile, PreviewEvidence};
use crate::{Error, Result};

const PATCH_BYTES: usize = 2 * 1024 * 1024;

async fn local_git(cwd: &Path, git_dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .current_dir(cwd)
        .args(["--no-pager", "-c", "core.fsmonitor=false"]);
    if let Some(git_dir) = git_dir {
        command
            .arg("--git-dir")
            .arg(git_dir)
            .arg("--work-tree")
            .arg(cwd);
    }
    command
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        // A pinned preview may read local objects, not fetch missing promisor objects.
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_NO_LAZY_FETCH", "1");
    crate::command::strip_git_overrides(&mut command);
    let output = crate::command::run(
        &mut command,
        Duration::from_secs(15),
        PATCH_BYTES,
        std::future::pending(),
    )
    .await?;
    if output.truncated {
        return Err(Error::invalid(
            "local Git output exceeded the 2 MiB capture limit; no partial patch was parsed",
        ));
    }
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "local Git could not read the pinned objects ({})",
            output.status
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| {
        Error::invalid(
            "pinned patch contains non-UTF-8 text; no replacement characters were substituted",
        )
    })
}
fn hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) async fn pull_request(repo: &Path, base: &str, target: &str) -> Result<PreviewEvidence> {
    if !hash(base) || !hash(target) {
        return Err(Error::invalid(
            "PR evidence requires full commit identities",
        ));
    }
    let common = match local_git(repo, None, &["merge-base", "--all", base, target]).await {
        Ok(common) => common,
        Err(error) => return Ok(PreviewEvidence::unavailable(&format!("PR target {target} / captured base {base}"), &format!("Local PR comparison unavailable: {error}. No working-checkout or last-commit patch was substituted."))),
    };
    let common = common.strip_suffix('\n').unwrap_or(&common);
    if !hash(common) {
        return Ok(PreviewEvidence::unavailable(
            &format!("PR target {target} / captured base {base}"),
            "No single exact merge-base was available; a PR diff was not inferred.",
        ));
    }
    let mut evidence = commit(repo, Some(common), target).await?;
    evidence.basis = format!("Local PR comparison from merge-base {common} of captured base {base} to {target} — not a fetched GitHub patch");
    Ok(evidence)
}

pub(crate) async fn commit(
    repo: &Path,
    base: Option<&str>,
    target: &str,
) -> Result<PreviewEvidence> {
    if !hash(target) || base.is_some_and(|value| !hash(value)) {
        return Err(Error::invalid(
            "pinned evidence requires full commit identities",
        ));
    }
    match read_commit(repo, base, target).await {
        Ok(evidence) => Ok(evidence),
        Err(error) => Ok(PreviewEvidence::unavailable(
            &format!("Pinned commit {target}"),
            &format!("Code evidence unavailable: {}. No current checkout patch was substituted. Publication includes reachable history, not only the last commit's patch.", error.to_string().chars().take(1024).collect::<String>()),
        )),
    }
}
async fn read_commit(repo: &Path, base: Option<&str>, target: &str) -> Result<PreviewEvidence> {
    let location = local_git(repo, None, &["rev-parse", "--absolute-git-dir"]).await?;
    let git_dir = std::fs::canonicalize(location.strip_suffix('\n').unwrap_or(&location))?;
    // Even a commit-to-commit diff can reuse a stat-matching worktree file when its
    // blob is missing. An empty worktree/cwd forces evidence to come from objects.
    let empty = tempfile::tempdir()?;
    let parent = if base.is_none() {
        let parents = local_git(
            empty.path(),
            Some(&git_dir),
            &["show", "--no-patch", "--format=%P", target, "--"],
        )
        .await?;
        if !parents.split_whitespace().all(hash) {
            return Err(Error::invalid("Git did not return exact parent identities"));
        }
        parents.split_whitespace().next().map(str::to_owned)
    } else {
        None
    };
    let from = base.or(parent.as_deref());
    let flags = [
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
        "--unified=3",
        "--submodule=short",
    ];
    let mut args = if from.is_some() {
        vec!["diff"]
    } else {
        vec!["diff-tree", "--root", "-p", "--no-commit-id"]
    };
    args.extend(flags);
    if let Some(from) = from {
        args.push(from);
    }
    args.extend([target, "--"]);
    let raw = local_git(empty.path(), Some(&git_dir), &args).await?;
    let files = crate::parse_diff(&raw);
    if !raw.trim().is_empty() && files.is_empty() {
        return Err(Error::invalid(
            "pinned patch format was not understood; it was not treated as an empty change",
        ));
    }
    let candidates = files
        .iter()
        .map(|file| BorrowedFile {
            area: "commit",
            file,
            revision: Some(target),
            reason: None,
            hunks_omitted: false,
        })
        .collect::<Vec<_>>();
    let basis = match (base, from) {
        (Some(base), _) => format!("Captured draft range {base} → {target}"),
        (None, Some(parent)) => format!("Last commit {target} relative to its first parent {parent} — not the full publication history"),
        (None, None) => format!("Root commit {target} — not a working-checkout diff"),
    };
    let mut evidence = capture(&basis, &candidates)?;
    let publication = "Publishing a commit includes its reachable history, not only the displayed patch. Binary content and raw Git metadata are not rendered as text hunks.";
    evidence.note = Some(match evidence.note.take() {
        Some(note) => format!("{note} {publication}"),
        None => publication.into(),
    });
    Ok(evidence)
}
