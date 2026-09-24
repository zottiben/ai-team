//! Keeping a stack of pull requests on its parents (PW11).
//!
//! A pull request stacked on another is built on that one's branch as it was. When the
//! parent moves - review follow-ups on it, or it merges - the child is left on commits that
//! are not its parent any more, and its review on GitHub shows them as its own. Restacking
//! rebases the child's own work onto where the parent is now: the orchestrator does the
//! rebase, because a conflict needs somebody who can read both sides; ai-team chooses the
//! commits beforehand and checks the result afterwards; and delivery publishes it, under
//! the team's own policy, like any accepted work.

use std::path::PathBuf;

/// A stacked pull request to rebase onto where its parent is now.
#[derive(Debug, Clone)]
pub(crate) struct Restack {
    pub(crate) plan: String,
    pub(crate) slice_key: String,
    pub(crate) title: String,
    /// Its scope as the plan has it: what it set out to do, for resolving a conflict.
    pub(crate) scope: String,
    pub(crate) branch: String,
    /// The PR's own worktree, where it was built and where the rebase happens (PW10).
    pub(crate) worktree: PathBuf,
    pub(crate) parent_key: String,
    /// What it goes onto, by name: `origin/main`, or the parent's branch as origin has it.
    pub(crate) onto_ref: String,
    /// `onto_ref`'s commit when the restack was decided. Pinned, so the attempt is one
    /// thing: this branch onto this commit (D15).
    pub(crate) onto: String,
    /// The parent commit it was built on. Everything after it is its own work.
    pub(crate) upstream: String,
    /// The branch it merges into from now on, when that changes: its parent merged.
    pub(crate) new_base: Option<String>,
    /// Why, for a person: `PR1 was merged into main`.
    pub(crate) why: String,
}

impl Restack {
    /// The run's prompt: what this restack is, in a line.
    pub(crate) fn headline(&self) -> String {
        format!(
            "Restack {} onto {}: {}",
            self.slice_key, self.onto_ref, self.why
        )
    }
}

/// How often the watch looks. GitHub is asked once for each pull request ai-team holds a
/// worktree for, so this stays well clear of its rate limit while still noticing a merge
/// inside the time it takes somebody to go and look.
pub(crate) const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(120);

/// One look at every pull request ai-team is holding a worktree for (PW10, D15).
///
/// A child whose parent moved or merged gets a restack started, parents first; a merged
/// PR gives its worktree back, its claim released and its slice done. Returns what it did,
/// a line each, for whoever is keeping the clock to print.
pub(crate) async fn watch(db: &std::path::Path) -> crate::Result<Vec<String>> {
    let repos: Vec<(i64, PathBuf)> = {
        let store = crate::Store::open(db)?;
        let mut repos = Vec::new();
        for project in store.projects()? {
            for repo in store.project_repos(project.id)? {
                if let Some(path) = repo.main_path {
                    repos.push((project.id, PathBuf::from(path)));
                }
            }
        }
        repos
    };
    let mut said = Vec::new();
    for (project_id, repo) in repos {
        if let Err(error) = watch_repo(db, project_id, &repo, &mut said).await {
            said.push(format!("{}: {error}", repo.display()));
        }
    }
    Ok(said)
}

/// The pull requests one repository's pool holds for ai-team, plan by plan.
async fn watch_repo(
    db: &std::path::Path,
    project_id: i64,
    repo: &std::path::Path,
    said: &mut Vec<String>,
) -> crate::Result<()> {
    if !repo.is_dir() {
        return Ok(());
    }
    let pool = crate::neighbours::Worktrees::at(repo).pool().await?;
    let held: Vec<(i64, String, PathBuf)> = pool
        .iter()
        .filter(|entry| entry.status == "leased")
        .filter_map(|entry| {
            let (run_id, key) = crate::supervise::held_by(entry.lease_holder.as_deref()?)?;
            Some((run_id, key, PathBuf::from(&entry.path)))
        })
        .collect();
    if held.is_empty() {
        return Ok(());
    }
    // Plan by plan: a lease names its run, and the run its plan and the checkout the plan
    // was written from.
    let mut plans: std::collections::BTreeMap<(PathBuf, String), Held> =
        std::collections::BTreeMap::default();
    {
        let store = crate::Store::open(db)?;
        for (run_id, key, worktree) in held {
            let Ok(run) = store.run(run_id) else { continue };
            let (Some(plan), Some(root)) = (run.plan_slug, run.workspace_path) else {
                continue;
            };
            let prs = plans.entry((PathBuf::from(root), plan)).or_default();
            // The newest run's lease, if an older one never went back.
            if prs.get(&key).is_none_or(|(held_by, _)| *held_by < run_id) {
                prs.insert(key, (run_id, worktree));
            }
        }
    }
    for ((root, plan), prs) in plans {
        let watched = Watched {
            db,
            project_id,
            repo,
            root: &root,
            plan: &plan,
            prs: &prs,
        };
        if let Err(error) = watched.look(said).await {
            said.push(format!("{plan}: {error}"));
        }
    }
    Ok(())
}

/// Pull requests held for one plan: key to the run that holds it and its worktree.
type Held = std::collections::HashMap<String, (i64, PathBuf)>;

struct Watched<'a> {
    db: &'a std::path::Path,
    project_id: i64,
    repo: &'a std::path::Path,
    root: &'a std::path::Path,
    plan: &'a str,
    prs: &'a Held,
}

impl Watched<'_> {
    async fn look(&self, said: &mut Vec<String>) -> crate::Result<()> {
        use crate::neighbours::github::{self, PrState};

        let planner = crate::Planner::at(self.root).for_plan(self.plan);
        let slices = planner.slices().await?;
        let mut merged = std::collections::HashSet::new();
        for slice in &slices {
            if !self.prs.contains_key(&slice.key) || slice.status != "in_review" {
                continue;
            }
            let Some(url) = slice.pr_url.as_deref() else {
                continue;
            };
            match github::pull_request(self.repo, url).await {
                Ok(pr) if pr.state == PrState::Merged => {
                    merged.insert(slice.key.clone());
                }
                Ok(_) => {}
                Err(error) => said.push(format!("{} {}: {error}", self.plan, slice.key)),
            }
        }

        // Parents first. A child whose parent is itself moving this time waits for it to
        // arrive: rebased onto where the parent is going, not where it has been.
        let mut moving = std::collections::HashSet::new();
        for child in by_depth(&slices) {
            let Some(parent) = crate::stack::parent(child, &slices) else {
                continue;
            };
            if !self.prs.contains_key(&child.key)
                || child.status != "in_review"
                || merged.contains(&child.key)
                || moving.contains(&parent.key)
            {
                continue;
            }
            let (Some(branch), Some(parent_branch)) =
                (child.branch.as_deref(), parent.branch.as_deref())
            else {
                continue;
            };
            let (onto_branch, new_base, why) = if merged.contains(&parent.key) {
                let Some(base) = parent.base_branch.clone() else {
                    continue;
                };
                let why = format!("{} was merged into {base}", parent.key);
                (base.clone(), Some(base), why)
            } else if parent.status == "in_review" {
                let why = format!("{} changed since {} was built on it", parent.key, child.key);
                (parent_branch.to_string(), None, why)
            } else {
                continue;
            };
            match self
                .restack(child, parent, branch, &onto_branch, new_base, why)
                .await
            {
                Ok(Some(note)) => {
                    moving.insert(child.key.clone());
                    said.push(note);
                }
                Ok(None) => {}
                Err(error) => {
                    moving.insert(child.key.clone());
                    said.push(format!("{} {}: {error}", self.plan, child.key));
                }
            }
        }

        for slice in slices.iter().filter(|slice| merged.contains(&slice.key)) {
            match self.give_back(&planner, slice).await {
                Ok(note) => said.push(note),
                Err(error) => said.push(format!("{} {}: {error}", self.plan, slice.key)),
            }
        }
        Ok(())
    }

    /// Start restacking `child` onto `onto_branch` as origin has it, if it is not on it.
    async fn restack(
        &self,
        child: &crate::Slice,
        parent: &crate::Slice,
        branch: &str,
        onto_branch: &str,
        new_base: Option<String>,
        why: String,
    ) -> crate::Result<Option<String>> {
        use crate::neighbours::git;

        if !git::fetch(self.repo, onto_branch).await? {
            return Ok(None);
        }
        let onto_ref = format!("origin/{onto_branch}");
        let onto = git::rev_parse(self.repo, &format!("refs/remotes/{onto_ref}")).await?;
        if git::is_ancestor(self.repo, &onto, branch).await? {
            return Ok(None);
        }
        // The parent's own branch, whose reflog remembers where it was when the child was
        // built on it, even after it was rewritten; origin's copy if this checkout has none.
        let parent_branch = parent.branch.as_deref().unwrap_or_default();
        let parent_ref = if git::branch_exists(self.repo, parent_branch).await {
            format!("refs/heads/{parent_branch}")
        } else {
            format!("refs/remotes/origin/{parent_branch}")
        };
        let upstream = git::fork_point(self.repo, &parent_ref, branch).await?;
        let (_, worktree) = &self.prs[&child.key];
        let restack = Restack {
            plan: self.plan.to_string(),
            slice_key: child.key.clone(),
            title: child.title.clone(),
            scope: child.scope_md.clone().unwrap_or_default(),
            branch: branch.to_string(),
            worktree: worktree.clone(),
            parent_key: parent.key.clone(),
            onto_ref,
            onto,
            upstream,
            new_base,
            why,
        };

        let opened = {
            let mut store = crate::Store::open(self.db)?;
            let project = store.project(self.project_id)?;
            let team_id = project
                .team_id
                .ok_or_else(|| crate::Error::NoTeam(project.slug.clone()))?;
            let orchestrator = store
                .agents(team_id)?
                .into_iter()
                .find(|agent| agent.role == crate::ROOT_ROLE && agent.enabled)
                .ok_or_else(|| crate::Error::invalid("the team has no enabled orchestrator"))?;
            let resolution = crate::ModelRegistry::load()?.resolve(&orchestrator)?;
            store.open_restack(crate::model::NewRestack {
                project_id: self.project_id,
                workspace: self.root,
                plan: self.plan,
                slice_key: &restack.slice_key,
                branch,
                worktree: &worktree.to_string_lossy(),
                onto: &restack.onto,
                prompt: &restack.headline(),
                agent_id: orchestrator.id,
                resolution: &resolution,
            })?
        };
        let Some((run, node)) = opened else {
            return Ok(None);
        };
        let started = format!("run #{}: {}", run.id, restack.headline());
        let db = self.db.to_path_buf();
        // Detached, as a scheduled run is: a rebase and its checks take minutes, and the
        // watch has other pull requests to look at.
        tokio::spawn(async move {
            if let Err(error) = crate::workflow::restack_at(&db, run.id, node.id, restack).await {
                eprintln!("restack: {error}");
            }
        });
        Ok(Some(started))
    }

    /// A merged PR's worktree goes back to the pool, its claim with it, and its slice is
    /// done (PW10) - unless something is still at work in it, in which case the next look
    /// gives it back.
    async fn give_back(
        &self,
        planner: &crate::Planner,
        slice: &crate::Slice,
    ) -> crate::Result<String> {
        let key = slice.key.as_str();
        let (_, worktree) = &self.prs[key];
        let busy = crate::Store::open(self.db)?.worktree_at_work(&worktree.to_string_lossy())?;
        if busy {
            return Ok(format!(
                "{} {key} merged; its worktree goes back once nothing is at work in it",
                self.plan
            ));
        }
        planner
            .set_status(key, "done", Some("merged on GitHub"))
            .await?;
        if let Some(branch) = slice.branch.as_deref() {
            crate::Store::open(self.db)?.close_merged_reviews(self.project_id, branch)?;
        }
        planner.release(key, worktree).await?;
        crate::neighbours::Worktrees::at(self.repo)
            .release(worktree)
            .await?;
        let note = format!(
            "{} {key} merged: its worktree went back to the pool",
            self.plan
        );
        let _ = planner
            .log(
                &format!("ai-team: {key} merged; its worktree went back"),
                Some(key),
            )
            .await;
        Ok(note)
    }
}

/// A plan's slices, each after the one it stacks on.
fn by_depth(slices: &[crate::Slice]) -> Vec<&crate::Slice> {
    let depth = |slice: &crate::Slice| {
        let mut depth = 0;
        let mut at = slice;
        while let Some(parent) = crate::stack::parent(at, slices) {
            depth += 1;
            // A loop is a stack problem reported at dispatch; here it only must not hang.
            if depth > slices.len() {
                break;
            }
            at = parent;
        }
        depth
    };
    let mut ordered: Vec<&crate::Slice> = slices.iter().collect();
    ordered.sort_by_key(|slice| depth(slice));
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(key: &str, branch: &str, base: &str) -> crate::Slice {
        crate::Slice {
            key: key.into(),
            status: "in_review".into(),
            branch: Some(branch.into()),
            base_branch: Some(base.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_stack_is_looked_at_from_its_root_up_whatever_order_the_plan_lists_it_in() {
        let slices = [
            slice("PR3", "p/pr3", "p/pr2"),
            slice("PR2", "p/pr2", "p/pr1"),
            slice("PR1", "p/pr1", "main"),
            slice("PR4", "p/pr4", "p/pr4-loop"),
            slice("PR5", "p/pr4-loop", "p/pr4"),
        ];

        let keys: Vec<&str> = by_depth(&slices)
            .iter()
            .map(|slice| slice.key.as_str())
            .collect();

        assert_eq!(&keys[..3], ["PR1", "PR2", "PR3"]);
        // A loop is reported elsewhere; here it only has to end.
        assert_eq!(keys.len(), 5);
    }
}
