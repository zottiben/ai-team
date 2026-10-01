//! Candidate trees use a private index: no reset, shared exclude edit, or accidental
//! inclusion of files generated after verification. Publication is a ref CAS, never -B.

use super::Watch;
use crate::{Error, Result};
use std::{path::Path, time::Duration};
use tokio::process::Command;

const OUTPUT_DIRS: [&str; 5] = ["target", "node_modules", "vendor", "dist", ".output"];

pub(in crate::chat::team) async fn text(
    path: &Path,
    args: &[&str],
    watch: &Watch,
) -> Result<String> {
    String::from_utf8(raw(path, args, None, watch).await?)
        .map(|text| text.trim_end().to_string())
        .map_err(|_| Error::invalid("git returned non-UTF-8 metadata"))
}

async fn raw(path: &Path, args: &[&str], index: Option<&Path>, watch: &Watch) -> Result<Vec<u8>> {
    watch.check()?;
    let mut command = Command::new("git");
    command
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1");
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = crate::command::run(
        &mut command,
        Duration::from_secs(120),
        8 * 1024 * 1024,
        watch.clone().wait(),
    )
    .await?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "git {}: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    if output.truncated {
        return Err(Error::invalid(
            "git metadata exceeded the capture limit; refusing an incomplete snapshot",
        ));
    }
    Ok(output.stdout)
}

pub(in crate::chat::team) async fn bound(
    path: &Path,
    head: &str,
    branch: &str,
    watch: &Watch,
) -> Result<()> {
    if text(path, &["rev-parse", "HEAD"], watch).await? != head
        || text(path, &["symbolic-ref", "--short", "HEAD"], watch).await? != branch
    {
        return Err(Error::invalid(
            "the leased branch or approved HEAD changed; keep the work for inspection",
        ));
    }
    Ok(())
}

pub(in crate::chat::team) async fn snapshot(path: &Path, watch: &Watch) -> Result<String> {
    let dir = tempfile::tempdir_in(
        watch
            .db
            .parent()
            .ok_or_else(|| Error::invalid("the store has no parent directory"))?,
    )?;
    if dir.path().canonicalize()?.starts_with(path.canonicalize()?) {
        return Err(Error::invalid(
            "the verification index must live outside the lease",
        ));
    }
    let index = dir.path().join("index");
    raw(path, &["read-tree", "HEAD"], Some(&index), watch).await?;
    let tracked = paths(&raw(path, &["ls-files", "-z"], Some(&index), watch).await?)?;
    let source_outputs: std::collections::HashSet<_> = tracked
        .iter()
        .flat_map(|file| output_roots(file).map(str::to_string))
        .collect();
    raw(path, &["add", "-u"], Some(&index), watch).await?;
    // Prune large disposable dependency/build directories inside Git when no
    // committed source uses that name. Mixed cases are filtered by full prefix below.
    let excludes: Vec<_> = OUTPUT_DIRS
        .iter()
        .filter(|name| {
            !source_outputs
                .iter()
                .any(|root| root.rsplit('/').next() == Some(**name))
        })
        .map(|name| format!("--exclude={name}/"))
        .collect();
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z"];
    args.extend(excludes.iter().map(String::as_str));
    let untracked = raw(path, &args, Some(&index), watch).await?;
    // A committed generated directory (for example ui/dist) is source: its new
    // hashed assets must travel with changed tracked entrypoints. Only wholly
    // untracked output directories are disposable, not every new file named dist/*.
    let files: Vec<_> = paths(&untracked)?
        .into_iter()
        .filter(|file| output_roots(file).all(|root| source_outputs.contains(root)))
        .collect();
    for chunk in files.chunks(128) {
        let mut args = vec!["add", "--"];
        args.extend(chunk.iter().map(String::as_str));
        raw(path, &args, Some(&index), watch).await?;
    }
    String::from_utf8(raw(path, &["write-tree"], Some(&index), watch).await?)
        .map(|text| text.trim().into())
        .map_err(|_| Error::invalid("git returned an invalid tree id"))
}

fn output_roots(file: &str) -> impl Iterator<Item = &str> {
    file.match_indices('/')
        .map(|(end, _)| &file[..end])
        .filter(|parent| OUTPUT_DIRS.contains(&parent.rsplit('/').next().unwrap_or_default()))
}

pub(super) async fn changes(
    path: &Path,
    base: &str,
    tree: &str,
    watch: &Watch,
) -> Result<Vec<String>> {
    paths(
        &raw(
            path,
            &[
                "diff-tree",
                "--no-commit-id",
                "--no-renames",
                "--name-only",
                "-r",
                "-z",
                base,
                tree,
            ],
            None,
            watch,
        )
        .await?,
    )
}

fn paths(raw: &[u8]) -> Result<Vec<String>> {
    raw.split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            String::from_utf8(path.to_vec()).map_err(|_| {
                Error::invalid("a changed path is not valid UTF-8; cannot certify its scope")
            })
        })
        .collect()
}

pub(super) async fn candidate(
    path: &Path,
    tree: &str,
    base: &str,
    message: &str,
    watch: &Watch,
) -> Result<String> {
    text(
        path,
        &[
            "-c",
            "user.name=ai-team",
            "-c",
            "user.email=ai-team@localhost",
            "-c",
            "commit.gpgSign=false",
            "commit-tree",
            tree,
            "-p",
            base,
            "-m",
            message,
        ],
        watch,
    )
    .await
}

pub(in crate::chat::team) async fn publish_local(
    path: &Path,
    branch: &str,
    sha: &str,
    base: &str,
    watch: &Watch,
) -> Result<()> {
    let index = text(path, &["write-tree"], watch).await?;
    let tree = text(path, &["rev-parse", &format!("{sha}^{{tree}}")], watch).await?;
    let base_tree = text(path, &["rev-parse", &format!("{base}^{{tree}}")], watch).await?;
    if index != base_tree && index != tree {
        return Err(Error::invalid("the real index contains a different staged tree; keep it for inspection rather than overwriting staged work"));
    }
    text(
        path,
        &["update-ref", &format!("refs/heads/{branch}"), sha, base],
        watch,
    )
    .await?;
    // Update only the index, never working files. A private-index commit must not
    // leave the base index looking like a staged reversal of the verified draft.
    text(path, &["read-tree", sha], watch).await.map(drop)
}
