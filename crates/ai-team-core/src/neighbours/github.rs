//! GitHub delivery through the operator's authenticated `gh` CLI.
//!
//! This is intentionally not a GitHub API client or MCP. Rust chooses the exact branch
//! and action after policy/HITL; `gh` contributes the operator's existing authentication
//! and repository resolution.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::error::{Error, Result};
use crate::model::RemoteDeliveryStatus;

/// What a pull request is on GitHub now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequest {
    pub url: String,
    pub state: PrState,
    /// The branch it would merge into.
    pub base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrState {
    Open,
    Merged,
    Closed,
}

const PULL_REQUEST_FIELDS: &str = "url,state,baseRefName";

/// A pull request's state and base, by URL or by head branch.
pub(crate) async fn pull_request(repo: &Path, which: &str) -> Result<PullRequest> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        gh(repo, &["pr", "view", which, "--json", PULL_REQUEST_FIELDS]),
    )
    .await
    .map_err(|_| Error::invalid("timed out reading a GitHub pull request"))??;
    read_pull_request(&output)
}

fn read_pull_request(json: &str) -> Result<PullRequest> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| Error::invalid(format!("could not read gh's pull request: {error}")))?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::invalid(format!("gh's pull request has no {name}")))
    };
    let state = match field("state")?.as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        other => {
            return Err(Error::invalid(format!(
                "a pull request in a state ai-team does not know: {other}"
            )))
        }
    };
    Ok(PullRequest {
        url: field("url")?,
        state,
        base: field("baseRefName")?,
    })
}

/// Point a pull request at another base branch.
pub(crate) async fn retarget(repo: &Path, url: &str, base: &str) -> Result<()> {
    gh(repo, &["pr", "edit", url, "--base", base])
        .await
        .map(drop)
}

/// What asking for a branch's pull request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Opened {
    New,
    /// The branch had one already. `was_based_on` is where it merged into before it was
    /// pointed at the plan's base, when that moved it.
    Existing {
        was_based_on: Option<String>,
    },
}

pub(crate) async fn create_pr(
    repo: &Path,
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
) -> Result<(String, Opened)> {
    // A retry after the CLI succeeded but the database write failed must adopt the PR,
    // not create a duplicate. So must a branch rewritten onto a new base - a restack - whose
    // PR is still the one it had: adopted, and pointed where the plan now bases it (PW11).
    // That happens here, after the push that must come first, because a PR retargeted
    // while its branch still sits on the old parent shows that parent's commits as its own.
    if let Ok(found) = pull_request(repo, branch).await {
        let moved = found.state == PrState::Open && found.base != base;
        if moved {
            retarget(repo, &found.url, base).await?;
        }
        let was_based_on = moved.then_some(found.base);
        return Ok((found.url, Opened::Existing { was_based_on }));
    }
    let output = gh(
        repo,
        &[
            "pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body,
        ],
    )
    .await?;
    let url = output
        .lines()
        .rev()
        .find(|line| line.trim().starts_with("https://"))
        .map(str::trim)
        .ok_or_else(|| Error::invalid("`gh pr create` succeeded without returning a PR URL"))?;
    Ok((url.to_string(), Opened::New))
}

pub(crate) async fn pr_status(repo: &Path, pr_url: &str) -> Result<RemoteDeliveryStatus> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        gh(
            repo,
            &["pr", "view", pr_url, "--json", "state,statusCheckRollup"],
        ),
    )
    .await
    .map_err(|_| Error::invalid("timed out reading GitHub pull-request status"))??;
    let value: serde_json::Value = serde_json::from_str(&output)
        .map_err(|error| Error::invalid(format!("could not read gh PR status: {error}")))?;
    let state = value
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("UNKNOWN")
        .to_ascii_lowercase();
    let rollup = value
        .get("statusCheckRollup")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let checks = check_state(rollup).to_string();
    Ok(RemoteDeliveryStatus {
        pr_state: state,
        checks,
    })
}

fn check_state(rollup: &[serde_json::Value]) -> &'static str {
    if rollup.is_empty() {
        return "none";
    }
    let values = rollup.iter().flat_map(|check| {
        ["status", "state", "conclusion"]
            .into_iter()
            .filter_map(|key| check.get(key).and_then(serde_json::Value::as_str))
    });
    let mut pending = false;
    for value in values {
        match value {
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => return "failed",
            "EXPECTED" | "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" => {
                pending = true;
            }
            _ => {}
        }
    }
    if pending {
        "pending"
    } else {
        "passed"
    }
}

/// Merge after checks using a merge commit, without deleting a branch underneath a
/// stacked child PR. Prefer GitHub's durable auto-merge. GitHub refuses auto-merge when
/// the target branch has no protection rule (common for an intermediate stack branch),
/// so in that exact case wait for every reported check and then merge directly.
pub(crate) async fn request_merge(repo: &Path, pr_url: &str) -> Result<()> {
    if gh(
        repo,
        &["pr", "view", pr_url, "--json", "state", "--jq", ".state"],
    )
    .await
    .is_ok_and(|state| state.trim() == "MERGED")
    {
        return Ok(());
    }
    match gh(repo, &["pr", "merge", pr_url, "--auto", "--merge"]).await {
        Ok(_) => Ok(()),
        Err(error) if auto_merge_unavailable(&error.to_string()) => {
            tokio::time::timeout(
                std::time::Duration::from_hours(1),
                wait_for_checks(repo, pr_url),
            )
            .await
            .map_err(|_| Error::invalid("timed out waiting for GitHub checks"))??;
            gh(repo, &["pr", "merge", pr_url, "--merge"])
                .await
                .map(drop)
        }
        Err(error) => Err(error),
    }
}

async fn wait_for_checks(repo: &Path, pr_url: &str) -> Result<()> {
    // GitHub can create the PR before attaching its workflow runs. An immediate "no
    // checks" answer is therefore not proof that this repository has no CI. Give the
    // check suite a short registration window before treating an empty rollup as final.
    for attempt in 0..8 {
        match gh(repo, &["pr", "checks", pr_url, "--watch", "--fail-fast"]).await {
            Ok(_) => return Ok(()),
            Err(error) if no_checks_reported(&error.to_string()) && attempt < 7 => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(error) if no_checks_reported(&error.to_string()) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn auto_merge_unavailable(message: &str) -> bool {
    message.contains("Auto merge is not allowed for this repository")
        || message.contains("Protected branch rules not configured for this branch")
}

fn no_checks_reported(message: &str) -> bool {
    message.contains("no checks reported")
}

async fn gh(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| Error::invalid(format!("could not run gh: {error}")))?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "`gh {}` failed in {}: {}",
            args.join(" "),
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        auto_merge_unavailable, check_state, no_checks_reported, read_pull_request, PrState,
        PullRequest,
    };

    #[test]
    fn a_pull_request_reads_as_open_merged_or_closed_with_its_base() {
        let read = |json: &str| read_pull_request(json).unwrap();
        assert_eq!(
            read(r#"{"url":"https://x/pull/2","state":"OPEN","baseRefName":"p/pr1"}"#),
            PullRequest {
                url: "https://x/pull/2".into(),
                state: PrState::Open,
                base: "p/pr1".into()
            }
        );
        assert_eq!(
            read(r#"{"url":"u","state":"MERGED","baseRefName":"main"}"#).state,
            PrState::Merged
        );
        assert_eq!(
            read(r#"{"url":"u","state":"CLOSED","baseRefName":"main"}"#).state,
            PrState::Closed
        );
        // Not a state to guess at: acting on a PR whose state is unknown is how a merged
        // one's worktree goes back while it is still open.
        assert!(read_pull_request(r#"{"url":"u","state":"DRAFTY","baseRefName":"main"}"#).is_err());
        assert!(read_pull_request(r#"{"url":"u","state":"OPEN"}"#).is_err());
    }

    #[test]
    fn falls_back_only_for_githubs_explicit_auto_merge_configuration_errors() {
        assert!(auto_merge_unavailable(
            "GraphQL: Auto merge is not allowed for this repository"
        ));
        assert!(auto_merge_unavailable(
            "GraphQL: Pull request Protected branch rules not configured for this branch"
        ));
        assert!(!auto_merge_unavailable("required review is missing"));
        assert!(!auto_merge_unavailable("HTTP 502 from github.com"));
    }

    #[test]
    fn an_absent_check_suite_is_not_a_failed_check() {
        assert!(no_checks_reported(
            "no checks reported on the 'ai-team/s1' branch"
        ));
        assert!(!no_checks_reported("build failed after 2m14s"));
    }

    #[test]
    fn check_rollups_distinguish_none_pending_failed_and_passed() {
        assert_eq!(check_state(&[]), "none");
        assert_eq!(
            check_state(&[serde_json::json!({ "status": "IN_PROGRESS" })]),
            "pending"
        );
        assert_eq!(
            check_state(&[
                serde_json::json!({ "conclusion": "SUCCESS" }),
                serde_json::json!({ "conclusion": "FAILURE" }),
            ]),
            "failed"
        );
        assert_eq!(
            check_state(&[
                serde_json::json!({ "conclusion": "SUCCESS" }),
                serde_json::json!({ "state": "SUCCESS" }),
            ]),
            "passed"
        );
    }
}
