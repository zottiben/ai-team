//! Explicit reconciliation after ordinary draining. Unknown crashed processes and
//! unverified work remain retained; this operation never starts or retries a model.
use super::execution::{git, Watch};
use crate::{
    ChatBuildControl, ChatBuildSlice, ChatTeamRun, Error, EventKind, NewEvent, Result, Store,
    Worktrees,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatBuildRecovery {
    pub chat_id: i64,
    pub run_id: i64,
    pub node_id: i64,
    pub expect_revision: i64,
}

#[derive(Debug, Serialize)]
pub struct ChatBuildRecoveryReport {
    pub execution: ChatTeamRun,
    pub slices: Vec<ChatBuildSlice>,
    pub issues: Vec<String>,
}

/// Reconcile exactly the displayed, quiescent execution. A report with issues is a
/// retained pause, not successful delivery or permission to discard its files.
pub async fn reconcile_chat_team_build(
    db: &Path,
    target: &ChatBuildRecovery,
) -> Result<ChatBuildRecoveryReport> {
    let mut store = Store::open(db)?;
    let mut control = store.claim_chat_build_recovery(target)?;
    let result = control
        .ownership
        .track(reconcile(&mut store, &control))
        .await;
    let reason = match &result {
        Ok(issues) if !issues.is_empty() => Some(issues.join("\n")),
        Err(error) => Some(error.to_string()),
        _ => None,
    };
    store
        .finish_chat_build(&mut control, reason.as_deref())
        .map_err(|cleanup| {
            Error::invalid(format!(
                "{}; finishing reconciliation also failed: {cleanup}",
                reason.as_deref().unwrap_or("Reconciliation finished")
            ))
        })?;
    let issues = result?;
    Ok(ChatBuildRecoveryReport {
        execution: store
            .chat_team_run(target.run_id)?
            .ok_or_else(|| Error::invalid("the execution disappeared"))?,
        slices: store.chat_build_slices(target.run_id)?,
        issues,
    })
}

async fn reconcile(store: &mut Store, control: &ChatBuildControl) -> Result<Vec<String>> {
    let chat = store.chat(control.receipt.chat_id)?;
    let repo = store
        .project_repos(chat.project_id)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .ok_or_else(|| Error::invalid("this project has no checkout"))?;
    let mut recovery = Recovery {
        watch: Watch::new(store.path(), control),
        repo: PathBuf::from(repo),
    };
    recovery.watch.cleanup = true;
    let mut issues = Vec::new();
    for row in store.chat_build_slices(control.receipt.run_id)? {
        if let Err(error) = recovery.slice(store, control, &row).await {
            let reason = format!("{}: {error}", row.slice_key);
            if let Err(error) = store.append_event(
                row.run_id,
                NewEvent::new(EventKind::Note, &reason).by("ai-team"),
            ) {
                issues.push(format!("recording recovery issue failed: {error}"));
            }
            issues.push(reason);
        }
    }
    Ok(issues)
}

struct Recovery {
    watch: Watch,
    repo: PathBuf,
}
impl Recovery {
    async fn slice(
        &self,
        store: &mut Store,
        control: &ChatBuildControl,
        row: &ChatBuildSlice,
    ) -> Result<()> {
        self.watch.check()?;
        store.owned_chat_build_slice(control, &row.slice_key)?;
        if row.lease_state == "released" {
            if row.commit_sha.is_some() {
                self.draft(row).await?;
            }
            return store.settle_chat_build_board(control, &row.slice_key, row.reason.as_deref());
        }
        if row.lease_state == "pending" {
            store.release_unacquired_chat_slice(control, &row.slice_key)?;
            return store.settle_chat_build_board(
                control,
                &row.slice_key,
                Some("Cancelled before acquisition was attempted"),
            );
        }
        let pool = Worktrees::at(&self.repo);
        let entries = pool.pool().await?;
        let path = self.locate(store, control, row, &pool, &entries).await?;
        let entry = entries
            .iter()
            .find(|entry| crate::same_worktree(&entry.path, &path.to_string_lossy()))
            .ok_or_else(|| {
                Error::invalid(
                    "the recorded worktree is not in the pool; inspect it without reacquiring",
                )
            })?;
        if entry.main {
            return Err(Error::invalid(
                "a main checkout cannot be a returned team lease",
            ));
        }
        let our_lease = entry.status == "leased"
            && entry.lease_holder == row.lease_holder
            && row.lease_holder.is_some();
        if row.release_started && !our_lease {
            // Re-leased entries belong to somebody else. Acknowledge only our durable
            // verified return intent, touching neither their files nor their lease.
            if row.commit_sha.is_none()
                || (entry.status == "available" && entry.lease_holder.is_some())
                || !matches!(entry.status.as_str(), "available" | "leased")
                || (entry.status == "leased" && (entry.lease_holder.is_none() || entry.orphaned()))
            {
                return Err(Error::invalid(
                    "return outcome is ambiguous; do not return this pool entry again",
                ));
            }
            self.draft(row).await?;
            store.reconcile_lease_state(control, &row.slice_key, true)?;
            return store.settle_chat_build_board(control, &row.slice_key, None);
        }
        if row.release_started {
            return Err(Error::invalid("the previous return outcome is uncertain; a holder label is not a lease generation, so do not return this entry a second time"));
        }
        if !our_lease || !entry.processes.is_empty() {
            return Err(Error::invalid(
                "the lease has another/unknown holder or live processes; leave it alone",
            ));
        }
        let sha = row.candidate_sha.as_deref().ok_or_else(|| Error::invalid("unverified work is kept in this exact lease/session; reconciliation will not restart or discard it"))?;
        let tree = self.candidate(row, sha).await?;
        let branch = row
            .branch
            .as_deref()
            .ok_or_else(|| Error::invalid("this lease has no approved branch"))?;
        let head = git::text(&path, &["rev-parse", "HEAD"], &self.watch).await?;
        if head != sha && head != control.base_sha {
            return Err(Error::invalid(
                "the lease moved away from both approved base and verified candidate",
            ));
        }
        git::bound(&path, &head, branch, &self.watch).await?;
        let index = git::text(&path, &["write-tree"], &self.watch).await?;
        let base_tree = git::text(
            &path,
            &["rev-parse", &format!("{}^{{tree}}", control.base_sha)],
            &self.watch,
        )
        .await?;
        if git::snapshot(&path, &self.watch).await? != tree || (index != tree && index != base_tree)
        {
            return Err(Error::invalid("the lease or staged tree differs from the verified candidate; preserve these files"));
        }
        if head == control.base_sha {
            git::publish_local(&path, branch, sha, &control.base_sha, &self.watch).await?;
        } else if index != tree {
            git::text(&path, &["read-tree", sha], &self.watch).await?;
        }
        if row.commit_sha.is_none() {
            store.record_chat_commit(control, &row.slice_key, sha)?;
        }
        store.reconcile_lease_state(control, &row.slice_key, false)?;
        store.mark_chat_lease_return(control, &row.slice_key, false)?;
        pool.release(&path).await?;
        store.mark_chat_lease_return(control, &row.slice_key, true)?;
        store.settle_chat_build_board(control, &row.slice_key, None)
    }

    async fn locate(
        &self,
        store: &mut Store,
        control: &ChatBuildControl,
        row: &ChatBuildSlice,
        pool: &Worktrees,
        entries: &[crate::PoolEntry],
    ) -> Result<PathBuf> {
        if let Some(path) = &row.worktree_path {
            return pool.resolve(Path::new(path)).await;
        }
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| {
                !entry.main
                    && entry.status == "leased"
                    && entry.lease_holder == row.lease_holder
                    && row.lease_holder.is_some()
            })
            .collect();
        if matches.len() != 1 {
            return Err(Error::invalid("the acquisition holder has no unique pool entry; do not guess or take another lease"));
        }
        let path = pool.resolve(Path::new(&matches[0].path)).await?;
        store.record_recovered_lease(control, &row.slice_key, &path.to_string_lossy())?;
        Ok(path)
    }

    async fn candidate(&self, row: &ChatBuildSlice, sha: &str) -> Result<String> {
        if row.maker_node_id.is_none()
            || row.verifier_node_id.is_none()
            || row
                .commit_sha
                .as_deref()
                .is_some_and(|commit| commit != sha)
        {
            return Err(Error::invalid(
                "publication evidence is incomplete or inconsistent",
            ));
        }
        let parents = git::text(
            &self.repo,
            &["rev-list", "--parents", "-n", "1", sha],
            &self.watch,
        )
        .await?;
        let expected = format!("{sha} {}", self.watch.control.base_sha);
        if parents != expected {
            return Err(Error::invalid(
                "the recorded candidate is not a direct child of the approved base",
            ));
        }
        git::text(
            &self.repo,
            &["rev-parse", &format!("{sha}^{{tree}}")],
            &self.watch,
        )
        .await
    }

    async fn draft(&self, row: &ChatBuildSlice) -> Result<()> {
        let sha = row
            .commit_sha
            .as_deref()
            .ok_or_else(|| Error::invalid("no verified commit"))?;
        self.candidate(row, sha).await?;
        let branch = row
            .branch
            .as_deref()
            .ok_or_else(|| Error::invalid("no draft branch"))?;
        if git::text(
            &self.repo,
            &["rev-parse", &format!("refs/heads/{branch}")],
            &self.watch,
        )
        .await?
            != sha
        {
            return Err(Error::invalid(
                "the recorded draft branch changed; do not reset it",
            ));
        }
        Ok(())
    }
}
