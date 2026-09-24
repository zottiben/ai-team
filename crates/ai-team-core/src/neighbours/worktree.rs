//! ai-worktree, over the `awt` CLI.
//!
//! Isolation is the whole point: one leased worktree per node, one eve process bound to
//! it (D3, D10). `awt` owns the pool, the cleaning and the reuse; ai-team only borrows
//! and returns (D4).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{Error, Result};

/// Something `awt` found running inside a worktree.
///
/// Reported because it is the difference between a worktree somebody is using and one
/// that merely was not returned - `awt` will not hand either out, and only the second is
/// a thing to clean up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub pid: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolEntry {
    pub name: String,
    pub path: String,
    pub status: String,
    /// Who holds the lease - or, when `awt` has decided nobody does, why.
    ///
    /// Not always a name. A tree left leased across a reboot reports
    /// `"orphaned: machine restarted while in use; resume with 'awt enter' or release
    /// with 'awt return'"` in this field, which is a sentence rather than a holder.
    // Renamed on the way **in** only. `awt` prints `leaseHolder`; every other field this
    // workspace serialises to the window is snake_case, and a plain `rename` applies in
    // both directions - which quietly published one camelCase field to the frontend and
    // cost a round trip to notice.
    #[serde(
        rename(deserialize = "leaseHolder"),
        default,
        deserialize_with = "holder_or_nobody"
    )]
    pub lease_holder: Option<String>,
    #[serde(default)]
    pub processes: Vec<Process>,
    /// The branch checked out in it, joined on from git.
    ///
    /// `awt status` does not report this, and it is the only thing that tells four
    /// worktrees of one repository apart - so it is asked of git and matched by path.
    #[serde(default)]
    pub branch: Option<String>,
    /// The repository's registered checkout rather than a linked task checkout.
    ///
    /// The main checkout is included in the same list because it is a workspace too; the
    /// window should not need a second source of truth just to render the first child.
    #[serde(default)]
    pub main: bool,
}

/// A tree returned to the pool reports `"leaseHolder": ""`. That is nobody, not somebody
/// with no name: read as a holder, every tree ever returned showed in the window as
/// leased.
fn holder_or_nobody<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.filter(|holder| !holder.trim().is_empty()))
}

impl PoolEntry {
    /// Whether `awt` is describing a lease nobody is holding any more.
    ///
    /// Matched on the prefix `awt` writes rather than on the whole sentence, which names
    /// the two commands to fix it and will not stay stable.
    pub fn orphaned(&self) -> bool {
        self.lease_holder
            .as_deref()
            .is_some_and(|holder| holder.starts_with("orphaned:"))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Pool {
    #[serde(default)]
    worktrees: Vec<PoolEntry>,
}

/// A worktree held for one node, returned to the pool when it is dropped.
///
/// `awt` will not hand a leased tree to a later `get` and will not prune it, so a lease
/// that is never returned leaks a worktree out of the pool. Returning is therefore tied
/// to the value's lifetime rather than to remembering.
#[derive(Debug)]
pub struct Lease {
    path: PathBuf,
    repo: PathBuf,
    returned: bool,
}

impl Lease {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Give the worktree back, cleaned. Returns nothing useful on purpose: a failure to
    /// return is reported, but it must not fail the work that was already done in it.
    pub async fn release(mut self) -> Result<()> {
        self.returned = true;
        Worktrees::at(&self.repo).release(&self.path).await
    }

    /// Keep the lease exactly as it is when a recovery attempt itself is interrupted.
    /// Dropping normally returns and cleans the worktree; that would destroy the very
    /// uncommitted work a later retry needs to recover.
    pub fn preserve(mut self) {
        self.returned = true;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.returned {
            return;
        }
        // A synchronous best-effort return on the unhappy path. The async `release` is
        // the intended route; this is what stops a panic or an early `?` from quietly
        // shrinking the pool.
        let _ = std::process::Command::new("awt")
            .arg("return")
            .arg(&self.path)
            .arg("--force")
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The `awt` pool for one repository.
#[derive(Debug, Clone)]
pub struct Worktrees {
    repo: PathBuf,
}

impl Worktrees {
    pub fn at(repo: impl Into<PathBuf>) -> Worktrees {
        Worktrees { repo: repo.into() }
    }

    pub async fn check(&self) -> std::result::Result<String, String> {
        match self.output(&["--version"]).await {
            Ok(version) => Ok(version.trim().to_string()),
            Err(error) => Err(format!("ai-worktree is not usable: {error}")),
        }
    }

    /// Lease a worktree. `--lease` prints only the path on stdout and puts its banners
    /// on stderr, which is the contract this depends on.
    pub async fn lease(&self, holder: &str) -> Result<Lease> {
        let path = self
            .output(&["get", "--lease", "--lease-holder", holder])
            .await?;
        let path = PathBuf::from(path.trim());
        if !path.is_dir() {
            return Err(Error::invalid(format!(
                "`awt get --lease` printed {} which is not a directory",
                path.display()
            )));
        }
        Ok(Lease {
            path,
            repo: self.repo.clone(),
            returned: false,
        })
    }

    pub async fn release(&self, path: &Path) -> Result<()> {
        self.output(&["return", &path.to_string_lossy(), "--force"])
            .await
            .map(drop)
    }

    /// Reattach ai-team to the exact lease an interrupted node was already using.
    ///
    /// A fresh `awt get` would choose another pool slot and lose the partial work. The
    /// stored path plus semantic holder identify the existing lease; an orphaned holder
    /// is accepted after a machine restart, but a worktree with any live process is not.
    pub async fn resume(&self, path: &Path, holder: &str) -> Result<Lease> {
        let requested = path.canonicalize().map_err(|error| {
            Error::invalid(format!(
                "could not resolve interrupted lease {}: {error}",
                path.display()
            ))
        })?;
        let pool = self.pool().await?;
        let entry = pool
            .iter()
            .find(|entry| same_worktree(&entry.path, &requested.to_string_lossy()))
            .ok_or_else(|| {
                Error::invalid("the interrupted worktree is no longer in the awt pool")
            })?;
        let held_by_node = entry.lease_holder.as_deref() == Some(holder);
        if entry.status != "leased" || (!held_by_node && !entry.orphaned()) {
            return Err(Error::invalid(format!(
                "the interrupted worktree is no longer leased by {holder}"
            )));
        }
        if !entry.processes.is_empty() {
            return Err(Error::invalid(format!(
                "the interrupted worktree still has {} live process(es); refusing duplicate supervision",
                entry.processes.len()
            )));
        }
        Ok(Lease {
            path: requested,
            repo: self.repo.clone(),
            returned: false,
        })
    }

    /// Every checkout of this repository, with `awt`'s lease state where it has one.
    ///
    /// Two programs are asked, because neither knows the whole answer: `awt` owns its
    /// pool, statuses and leases, while git owns every linked checkout and branch. The
    /// merge is what includes all three useful cases without copying any of them into
    /// ai-team: main, a human task worktree, and an orchestrator lease.
    pub async fn pool(&self) -> Result<Vec<PoolEntry>> {
        let json = self.output(&["status", "--json"]).await?;
        let pool: Pool = serde_json::from_str(&json).map_err(|error| {
            Error::invalid(format!("could not read `awt status --json`: {error}"))
        })?;
        let branches = self.branches().await?;
        let main = self.repo.canonicalize().map_err(|error| {
            Error::invalid(format!(
                "could not resolve checkout {}: {error}",
                self.repo.display()
            ))
        })?;

        let mut entries: Vec<PoolEntry> = pool
            .worktrees
            .into_iter()
            .map(|mut entry| {
                entry.branch = branches
                    .iter()
                    .find(|(path, _)| same_worktree(path, &entry.path))
                    .and_then(|(_, branch)| branch.clone());
                entry.main = same_worktree(&entry.path, &main.to_string_lossy());
                entry
            })
            .collect();

        // Git-only entries are human-created linked worktrees (or the registered main
        // checkout). They are selectable even though `awt` has no lease metadata for
        // them; calling them "linked" is more truthful than calling them available.
        for (path, branch) in branches {
            if entries
                .iter()
                .any(|entry| same_worktree(&entry.path, &path))
            {
                continue;
            }
            let is_main = same_worktree(&path, &main.to_string_lossy());
            let name = if is_main {
                "main".to_string()
            } else {
                Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("worktree")
                    .to_string()
            };
            entries.push(PoolEntry {
                name,
                path,
                status: if is_main { "main" } else { "linked" }.to_string(),
                lease_holder: None,
                processes: Vec::new(),
                branch,
                main: is_main,
            });
        }
        entries.sort_by_key(|entry| (!entry.main, entry.branch.clone(), entry.path.clone()));
        Ok(entries)
    }

    /// Resolve a requested checkout and prove git considers it part of this repository.
    ///
    /// Paths cannot be trusted merely because the browser got them from `/worktrees`: a
    /// caller can forge an API request. Membership in `git worktree list` is the boundary
    /// that lets a linked worktree live outside the main checkout without allowing an
    /// arbitrary directory on disk.
    pub async fn resolve(&self, requested: &Path) -> Result<PathBuf> {
        let requested = requested.canonicalize().map_err(|error| {
            Error::invalid(format!(
                "could not resolve workspace {}: {error}",
                requested.display()
            ))
        })?;
        let listed = self.branches().await?;
        if listed
            .iter()
            .any(|(path, _)| same_worktree(path, &requested.to_string_lossy()))
        {
            return Ok(requested);
        }
        Err(Error::invalid(format!(
            "{} is not a worktree of {}",
            requested.display(),
            self.repo.display()
        )))
    }

    /// Path to branch, for every git worktree of this repository.
    async fn branches(&self) -> Result<Vec<(String, Option<String>)>> {
        crate::neighbours::git::worktrees(&self.repo).await
    }

    async fn output(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("awt")
            .args(args)
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| {
                Error::invalid(format!(
                    "could not run `awt`: {error}. ai-worktree is a separate tool; install \
                     it from its own repo."
                ))
            })?;
        if !output.status.success() {
            return Err(Error::invalid(format!(
                "`awt {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Whether two paths name the same directory.
///
/// Compared after resolving, because macOS hands back `/private/var/...` where `awt`
/// printed `/var/...` - the same directory under two names, which a string comparison
/// calls two worktrees and shows with no branch (D12). Falls back to the literal
/// comparison when either side cannot be resolved, which is what happens for a worktree
/// that has just been removed.
pub fn same_worktree(left: &str, right: &str) -> bool {
    match (
        std::fs::canonicalize(left).ok(),
        std::fs::canonicalize(right).ok(),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_json_shape_is_what_awt_actually_prints() {
        // Captured from `awt status --json` v0.2.0 rather than written from the docs.
        let pool: Pool = serde_json::from_str(
            r#"{"poolDir":"/home/me/.awt/repo-d0e174","worktrees":[
                 {"name":"1","path":"/home/me/.awt/repo-d0e174/1/repo","status":"leased",
                  "leaseHolder":"ai-team","processes":[]},
                 {"name":"2","path":"/home/me/.awt/repo-d0e174/2/repo","status":"available",
                  "processes":[]}]}"#,
        )
        .unwrap();
        assert_eq!(pool.worktrees.len(), 2);
        assert_eq!(pool.worktrees[0].status, "leased");
        assert_eq!(pool.worktrees[0].lease_holder.as_deref(), Some("ai-team"));
        // An available tree carries no holder, and that must not fail to parse.
        assert_eq!(pool.worktrees[1].lease_holder, None);
    }

    #[test]
    fn a_tree_returned_to_the_pool_is_held_by_nobody() {
        // Captured from `awt status --json` v0.1.0 after ai-team returned a lease: the
        // key stays, empty.
        let pool: Pool = serde_json::from_str(
            r#"{"poolDir":"/Users/me/.awt/widget-45ea46","worktrees":[
                 {"name":"1","path":"/Users/me/.awt/widget-45ea46/1/widget",
                  "status":"available","leaseHolder":"","processes":[]}]}"#,
        )
        .unwrap();
        assert_eq!(pool.worktrees[0].lease_holder, None);
        assert!(!pool.worktrees[0].orphaned());
    }

    #[test]
    fn the_real_shapes_awt_prints_all_parse() {
        // Captured from `awt status --json` against a repository with four worktrees, not
        // written from the docs. Two things the fixture above did not have and the real
        // output does: a `processes` list, and `in-use` as a status distinct from
        // `leased` - a tree somebody is working in rather than one ai-team took out.
        let pool: Pool = serde_json::from_str(
            r#"{"poolDir":"/Users/me/.awt/nodifi-data-178f5b","worktrees":[
                 {"name":"1","path":"/Users/me/.awt/nodifi-data-178f5b/1/nodifi-data",
                  "status":"leased",
                  "leaseHolder":"orphaned: machine restarted while in use; resume with 'awt enter' or release with 'awt return'",
                  "processes":[{"pid":30722,"name":"nvim"},{"pid":31459,"name":"zsh"}]},
                 {"name":"3","path":"/Users/me/.awt/nodifi-data-178f5b/3/nodifi-data",
                  "status":"in-use","processes":[{"pid":1,"name":"node"}]}]}"#,
        )
        .unwrap();

        assert_eq!(pool.worktrees.len(), 2);
        assert_eq!(pool.worktrees[0].processes.len(), 2);
        assert_eq!(pool.worktrees[0].processes[0].name, "nvim");
        assert_eq!(pool.worktrees[1].status, "in-use");
        // No `leaseHolder` key at all on the second, which must not fail to parse.
        assert_eq!(pool.worktrees[1].lease_holder, None);
    }

    #[test]
    fn what_awt_calls_lease_holder_reaches_the_window_as_lease_holder() {
        // `awt` prints `leaseHolder` and the window reads `lease_holder`. A plain
        // `#[serde(rename)]` renames in both directions, so the obvious spelling parses
        // awt correctly and then publishes camelCase to the frontend - where it is
        // `undefined`, every lease looks unheld, and nothing errors.
        let entry: PoolEntry = serde_json::from_str(
            r#"{"name":"1","path":"/x","status":"leased","leaseHolder":"ai-team"}"#,
        )
        .unwrap();
        assert_eq!(entry.lease_holder.as_deref(), Some("ai-team"));

        let published = serde_json::to_value(&entry).unwrap();
        assert_eq!(published["lease_holder"], "ai-team");
        assert!(
            published.get("leaseHolder").is_none(),
            "awt's spelling leaked to the window: {published}"
        );
    }

    #[test]
    fn an_orphaned_lease_is_recognised_by_what_awt_writes() {
        // The state that needs a person: `awt` will not hand the tree out and nobody is
        // holding it. Matched on the prefix, because the rest of the sentence names two
        // commands and is not something to depend on.
        let entry = |holder: Option<&str>| PoolEntry {
            name: "1".into(),
            path: "/x".into(),
            status: "leased".into(),
            lease_holder: holder.map(str::to_string),
            processes: Vec::new(),
            branch: None,
            main: false,
        };

        assert!(entry(Some(
            "orphaned: machine restarted while in use; resume with 'awt enter' or release with 'awt return'"
        ))
        .orphaned());
        assert!(!entry(Some("ai-team")).orphaned());
        assert!(!entry(None).orphaned());
    }

    #[tokio::test]
    async fn the_branch_of_every_worktree_is_read_from_git() {
        // The only thing that tells four directories called 1, 2, 3 and 4 apart. `awt`
        // does not report it, so it is joined on from git by path.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "master"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a"), "a").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "one"]);
        // A branch with a slash in it, which is what these are actually called - and what
        // taking the last path segment of `refs/heads/...` would mangle.
        run(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "chore/review-7223",
            dir.path().join("wt").to_str().unwrap(),
        ]);

        let found = crate::neighbours::git::worktrees(&repo).await.unwrap();
        let branches: Vec<&str> = found
            .iter()
            .filter_map(|(_, branch)| branch.as_deref())
            .collect();
        assert!(branches.contains(&"master"), "{found:?}");
        assert!(
            branches.contains(&"chore/review-7223"),
            "a branch with a slash was mangled: {found:?}"
        );
    }

    #[test]
    fn a_worktree_is_matched_to_its_branch_through_the_path_macos_reports() {
        // On macOS `/var` and `/tmp` are symlinks into `/private`, so `awt` and git can
        // name one directory two ways - and a string comparison then shows every worktree
        // with no branch (D12). Runs on both legs: on Linux the two spellings are already
        // equal, so this asserts the comparison is at least not wrong there.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("tree");
        std::fs::create_dir_all(&real).unwrap();

        let resolved = real.canonicalize().unwrap();
        assert!(same_worktree(
            &real.to_string_lossy(),
            &resolved.to_string_lossy()
        ));
        assert!(!same_worktree(
            &real.to_string_lossy(),
            &dir.path().join("other").to_string_lossy()
        ));
    }
}
