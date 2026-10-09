//! Bind an explicitly requested new branch without granting arbitrary branch changes.

use super::{grammar::Request, PushGrant};
use crate::{
    chat_changes::{checkout, git},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewBranchScope {
    pub base: Option<String>,
    pub name: Option<String>,
    /// Both local and origin branches present when the human sent the request.
    pub existing: Vec<String>,
}

pub(super) async fn prepare(
    repo: &Path,
    origin: &str,
    request: &Request,
) -> Result<(String, Option<NewBranchScope>)> {
    let Some(new) = &request.new_branch else {
        return Ok((
            git(repo, &["rev-parse", "--verify", "HEAD"])
                .await?
                .trim()
                .to_ascii_lowercase(),
            None,
        ));
    };
    let head = if let Some(base) = &new.base {
        let name = base.strip_prefix("origin/").unwrap_or(base);
        git(repo, &["check-ref-format", &format!("refs/heads/{name}")]).await?;
        // Prefer the recorded origin base. Fetching or moving branches remains the
        // requested task, not an unapproved side effect of resolving push authority.
        match git(repo, &["rev-parse", "--verify", &format!("refs/remotes/origin/{name}^{{commit}}")]).await {
            Ok(head) => head,
            Err(_) => git(repo, &["rev-parse", "--verify", &format!("refs/heads/{name}^{{commit}}")]).await
                .map_err(|_| Error::invalid("the requested new-branch base is not available locally; fetch it before asking again"))?,
        }
    } else {
        git(repo, &["rev-parse", "--verify", "HEAD"]).await?
    };
    let local = git(
        repo,
        &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads/"],
    )
    .await?;
    let remote = git(repo, &["ls-remote", "--heads", origin]).await?;
    let mut existing: Vec<_> = local
        .lines()
        .map(str::to_owned)
        .chain(remote.lines().filter_map(|line| {
            line.split_once('\t')
                .and_then(|(_, name)| name.strip_prefix("refs/heads/"))
                .map(str::to_owned)
        }))
        .collect();
    existing.sort();
    existing.dedup();
    if existing.len() > 10_000 {
        return Err(Error::invalid(
            "too many existing branches to snapshot this new-branch approval",
        ));
    }
    if let Some(name) = &new.name {
        checkout::publishable_branch(repo, origin, name).await?;
        if existing_branch(repo, &existing, name).await? {
            return Err(Error::invalid(
                "the requested new branch already exists; it cannot be treated as new work",
            ));
        }
    }
    Ok((
        head.trim().to_ascii_lowercase(),
        Some(NewBranchScope {
            base: new.base.clone(),
            name: new.name.clone(),
            existing,
        }),
    ))
}

async fn existing_branch(repo: &Path, existing: &[String], branch: &str) -> Result<bool> {
    if existing.iter().any(|name| name == branch) {
        return Ok(true);
    }
    let common = git(repo, &["rev-parse", "--git-common-dir"]).await?;
    let refs = repo.join(common.trim()).join("refs/heads");
    let Some(actual) = resolve_ref(&refs, branch)? else {
        return Ok(false);
    };
    // A case/Unicode alias on APFS is still the original ref, including when a
    // new loose ref shadows a packed one. On a case-sensitive filesystem it differs.
    for name in existing {
        if resolve_ref(&refs, name)?.as_ref() == Some(&actual) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn resolve_ref(refs: &Path, name: &str) -> Result<Option<std::path::PathBuf>> {
    if name.is_empty()
        || !Path::new(name)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err(Error::invalid(
            "the recorded branch name is not a safe Git ref",
        ));
    }
    match refs.join(name).canonicalize() {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn validate(grant: &PushGrant, repo: &Path, branch: &str) -> Result<()> {
    if let Some(pinned) = &grant.pinned_branch {
        if pinned != branch {
            return Err(Error::invalid(
                "a different branch is already pinned for this push; it will not be retargeted",
            ));
        }
    }
    if let Some(scope) = &grant.new_branch {
        git(repo, &["check-ref-format", &format!("refs/heads/{branch}")]).await?;
        if existing_branch(repo, &scope.existing, branch).await?
            || scope.name.as_deref().is_some_and(|name| name != branch)
        {
            return Err(Error::invalid(
                "this is not the new branch the person requested; nothing was pinned",
            ));
        }
        checkout::publishable_branch(repo, &grant.origin_url, branch).await?;
    } else if branch != grant.branch {
        return Err(Error::invalid(
            "this checkout is no longer on the branch the person authorised; nothing was pinned",
        ));
    }
    Ok(())
}
