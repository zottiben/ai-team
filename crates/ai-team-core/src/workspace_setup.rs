//! Explicit AWT acquisition followed by locked dependency setup. Never returns or
//! resets a retained lease on failure, cancellation, lost acknowledgement or drop.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::{
    chat::team::{children, ownership::Ownership},
    Error, Result, Store, Worktrees,
};

#[derive(Debug, Serialize)]
pub struct Setup {
    pub id: i64,
    pub project_id: i64,
    pub repo_path: String,
    pub request_id: String,
    pub branch: String,
    pub workspace_path: Option<String>,
    pub state: String,
    pub detail: String,
    pub rev: i64,
    pub steps: Vec<Step>,
}
#[derive(Debug, Serialize)]
pub struct Step {
    pub command: String,
    pub passed: bool,
    pub output: String,
    pub at: String,
}

pub(crate) fn holder(db: &Path, id: i64) -> Result<String> {
    let db = db.canonicalize()?;
    let digest = format!("{:x}", Sha256::digest(db.to_string_lossy().as_bytes()));
    Ok(format!("ai-team workspace {}-{id}", &digest[..16]))
}
fn branch(setup: &Setup) -> String {
    if setup.branch.is_empty() {
        format!("ai-team/workspace-{}", setup.id)
    } else {
        setup.branch.clone()
    }
}

/// Only the first accepted receipt can start acquisition. A repeated HTTP request
/// returns its durable outcome, never a second awt get.
pub async fn run(db: &Path, project: i64, id: i64) -> Result<Setup> {
    let mut store = Store::open(db)?;
    let setup = store.workspace_setup(project, id)?;
    if setup.state != "pending" {
        return Ok(setup);
    }
    let owner = store.claim_workspace_setup(project, id, setup.rev, false)?;
    let outcome = owner.track(prepare(&owner, &setup)).await;
    settle(&owner, project, id, outcome)?;
    Store::open(db)?.workspace_setup(project, id)
}
fn settle(owner: &Ownership, project: i64, id: i64, outcome: Result<()>) -> Result<()> {
    let mut store = Store::open(&owner.db)?;
    let setup = store.workspace_setup(project, id)?;
    let (state,detail)=match outcome {
        Ok(()) if owner.quiescent() => ("ready","AWT checkout and detected locked dependencies are ready. The lease is retained; no agent has started.".into()),
        Ok(()) => ("inspection","Setup finished with uncertain child lifetime; inspect before use.".into()),
        Err(error) => (if setup.workspace_path.is_none() || !owner.quiescent() { "inspection" } else { "failed" },error.to_string()),
    };
    store.finish_workspace_setup(owner, state, &detail)
}
async fn prepare(owner: &Arc<Ownership>, setup: &Setup) -> Result<()> {
    let repo = Path::new(&setup.repo_path);
    let chosen = branch(setup);
    step(
        owner,
        repo,
        "git",
        &["check-ref-format", "--branch", &chosen],
    )
    .await?;
    let pool = Worktrees::at(repo).pool().await?;
    Store::open(&owner.db)?.check_setup_pool(&pool)?;
    // Do not use Lease here: its Drop returns and cleans the checkout on errors.
    let output = step(
        owner,
        repo,
        "awt",
        &[
            "get",
            "--lease",
            "--lease-holder",
            &holder(&owner.db, setup.id)?,
        ],
    )
    .await?;
    let path = Worktrees::at(repo)
        .resolve(Path::new(output.trim()))
        .await?;
    if crate::same_worktree(&path.to_string_lossy(), &setup.repo_path) {
        return Err(Error::invalid(
            "awt returned the main checkout instead of an isolated worktree",
        ));
    }
    verify_lease(&owner.db, setup, &path).await?;
    Store::open(&owner.db)?.setup_path(owner, &path)?;
    step(
        owner,
        &path,
        "git",
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "checkout",
            "--no-track",
            "-b",
            &chosen,
        ],
    )
    .await?;
    dependencies(owner, &path).await
}
async fn verify_lease(db: &Path, setup: &Setup, path: &Path) -> Result<()> {
    let expected = holder(db, setup.id)?;
    let pool = Worktrees::at(&setup.repo_path).pool().await?;
    if !pool.iter().any(|entry| {
        crate::same_worktree(&entry.path, &path.to_string_lossy())
            && entry.lease_holder.as_deref() == Some(expected.as_str())
    }) {
        return Err(Error::invalid(
            "AWT did not confirm this setup's exact retained lease; inspect before continuing",
        ));
    }
    Ok(())
}
async fn dependencies(owner: &Ownership, path: &Path) -> Result<()> {
    check_manifests(path)?;
    let steps = crate::gates::refresh_dependency_setup(path);
    for setup in steps {
        let cwd = path.join(&setup.dir).canonicalize()?;
        if !cwd.starts_with(path) {
            return Err(Error::invalid(
                "a dependency manifest directory resolves outside the new checkout",
            ));
        }
        for name in ["node_modules", "vendor", ".venv"] {
            let target = cwd.join(name);
            if target
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err(Error::invalid(format!(
                    "{} is a symlink; refusing to install into another checkout's dependencies",
                    target.display()
                )));
            }
        }
        let args = setup.args.iter().map(String::as_str).collect::<Vec<_>>();
        step(owner, &cwd, &setup.program, &args).await?;
    }
    Ok(())
}
fn check_manifests(path: &Path) -> Result<()> {
    if path.join("composer.json").is_file() && !path.join("composer.lock").is_file() {
        return Err(Error::invalid(
            "Composer setup requires composer.lock; this checkout is not ready",
        ));
    }
    let package = std::fs::read(path.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let workspaces = package.as_ref().and_then(|value| value.get("workspaces"));
    let workspaces = workspaces.and_then(|value| {
        value
            .as_array()
            .or_else(|| value.get("packages").and_then(serde_json::Value::as_array))
    });
    for dir in [".", "ui", "web", "frontend", "app"] {
        let root = path.join(dir);
        if !root.join("package.json").is_file() {
            continue;
        }
        let member = dir != "."
            && workspaces.is_some_and(|patterns| {
                patterns
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .any(|pattern| pattern == dir || pattern == "*")
            });
        if !member
            && ![
                "bun.lock",
                "bun.lockb",
                "pnpm-lock.yaml",
                "yarn.lock",
                "package-lock.json",
            ]
            .iter()
            .any(|name| root.join(name).is_file())
        {
            return Err(Error::invalid(format!("{dir}/package.json has no supported lockfile; this checkout is not ready for automatic installation")));
        }
    }
    Ok(())
}

async fn step(owner: &Ownership, cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let label = format!("{} {}", program, args.join(" "));
    Store::open(&owner.db)?.setup_progress(owner, &format!("{} · {label}", cwd.display()))?;
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    crate::command::strip_git_overrides(&mut command);
    let result = crate::command::run(
        &mut command,
        Duration::from_mins(15),
        64_000,
        std::future::pending(),
    )
    .await;
    let (passed, output) = match &result {
        Ok(output) => (
            output.status.success(),
            format!(
                "{}{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                if output.truncated {
                    "\n[output truncated]"
                } else {
                    ""
                }
            ),
        ),
        Err(error) => (false, error.to_string()),
    };
    let tail = output
        .chars()
        .rev()
        .take(6000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    Store::open(&owner.db)?.setup_step(owner, &label, passed, &tail)?;
    let result = result?;
    if !passed {
        return Err(Error::invalid(format!(
            "setup `{label}` failed; its checkout and lease are retained:\n{tail}"
        )));
    }
    if program == "awt" && result.truncated {
        return Err(Error::invalid(
            "AWT's path response was truncated; inspect rather than acquiring again",
        ));
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}

/// Inspection drains only this receipt's children and finds its exact AWT holder.
/// It does not replay acquisition/hooks, return a lease, reset files or claim readiness.
pub async fn inspect(db: &Path, project: i64, id: i64, revision: i64) -> Result<Setup> {
    let mut store = Store::open(db)?;
    let setup = store.workspace_setup(project, id)?;
    let owner = store.claim_workspace_setup(project, id, revision, true)?;
    let result = owner
        .track(async {
            for child in Store::open(db)?.workspace_setup_children(id)? {
                children::drain(&child).await?;
                if child.state != "drained" {
                    Store::open(db)?.finish_chat_child(&owner, child.id)?;
                }
            }
            let pool = Worktrees::at(&setup.repo_path).pool().await?;
            let wanted = holder(db, id)?;
            let found = pool
                .iter()
                .filter(|entry| entry.lease_holder.as_deref() == Some(wanted.as_str()))
                .collect::<Vec<_>>();
            if found.len() > 1 {
                return Err(Error::invalid(
                    "multiple AWT leases name this receipt; keep its fence",
                ));
            }
            if let Some(entry) = found.first() {
                let path = Worktrees::at(&setup.repo_path)
                    .resolve(Path::new(&entry.path))
                    .await?;
                Store::open(db)?.setup_path(&owner, &path)?;
            } else if setup.workspace_path.is_some() {
                return Err(Error::invalid(
                    "the recorded AWT lease changed; keep its fence",
                ));
            }
            Ok(())
        })
        .await;
    let (state,detail)=match result {
        Ok(()) if owner.quiescent() => ("failed","Inspection drained the recorded commands. No setup was replayed and no readiness was granted. Any retained checkout stays unchanged.".into()),
        Ok(()) => ("inspection","Child lifetime remains uncertain; keep the setup fence.".into()),
        Err(error) => ("inspection",error.to_string()),
    };
    Store::open(db)?.finish_workspace_setup(&owner, state, &detail)?;
    Store::open(db)?.workspace_setup(project, id)
}

/// Explicit retry only of dependencies in the same retained checkout, never awt get
/// or branch creation. Earlier step/child evidence remains append-only.
pub async fn retry_dependencies(db: &Path, project: i64, id: i64, revision: i64) -> Result<Setup> {
    let mut store = Store::open(db)?;
    let setup = store.workspace_setup(project, id)?;
    if setup.state != "failed" {
        return Err(Error::invalid(
            "inspect interrupted setup before requesting another dependency attempt",
        ));
    }
    let path = PathBuf::from(setup.workspace_path.as_deref().ok_or_else(|| {
        Error::invalid("there is no located checkout; request a new AWT setup explicitly")
    })?);
    let owner = store.claim_workspace_setup(project, id, revision, true)?;
    let result=owner.track(async {
        verify_lease(db,&setup,&path).await?;
        let current=crate::neighbours::git::current_branch(&path).await;
        if current.as_deref()!=Some(branch(&setup).as_str()) { return Err(Error::invalid("the setup branch changed or was never created; inspect the retained checkout manually, not by resetting it")); }
        dependencies(&owner,&path).await
    }).await;
    settle(&owner, project, id, result)?;
    Store::open(db)?.workspace_setup(project, id)
}
