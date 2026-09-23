//! Which checkout sits under which, for the window (PW1, PW10).
//!
//! Git has no idea one worktree belongs under another; the nesting is ai-team's to say,
//! and it says it from facts it already has rather than a table of its own:
//!
//! - the project's registered checkout is the top of every tree;
//! - a worktree the operator made sits under it;
//! - a pull request's worktree sits under the checkout its run started in - or, when the PR
//!   stacks on another, under that PR's worktree, so the tree reads the way the work
//!   merges: from the leaves back down.
//!
//! An idle `awt` slot is nobody's and is not shown. It is a pool, not a place.

use serde::Serialize;

use crate::neighbours::{same_worktree, PoolEntry};

/// What a checkout is to ai-team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// The project's registered checkout.
    Main,
    /// A worktree the operator made.
    Manual,
    /// The worktree one pull request is built in.
    Pr { plan: String, slice_key: String },
}

/// One checkout, placed in the tree.
#[derive(Debug, Clone, Serialize)]
pub struct Placed {
    #[serde(flatten)]
    pub entry: PoolEntry,
    #[serde(flatten)]
    pub kind: Kind,
    /// The path of the checkout it sits under. `None` only for the top of the tree.
    pub parent: Option<String>,
}

/// What ai-team knows about the pull request built in one worktree, gathered by the
/// caller from the run that built it and from the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFacts {
    pub path: String,
    pub plan: String,
    pub slice_key: String,
    /// The checkout the run that built it started in.
    pub workspace: Option<String>,
    /// The branch it stacks on, when it stacks on another PR of its plan.
    pub stacked_on: Option<String>,
}

/// Place every checkout under its parent.
///
/// Order is kept from `entries`; the window draws the tree from the parent links.
pub fn place(entries: Vec<PoolEntry>, prs: &[PrFacts]) -> Vec<Placed> {
    let main = entries
        .iter()
        .find(|entry| entry.main)
        .map(|entry| entry.path.clone());
    // The path an entry is known by here, for a path given in some other spelling.
    let known = |path: &str| -> Option<String> {
        entries
            .iter()
            .find(|entry| same_worktree(&entry.path, path))
            .map(|entry| entry.path.clone())
    };
    let by_branch = |branch: &str| -> Option<String> {
        entries
            .iter()
            .find(|entry| entry.branch.as_deref() == Some(branch))
            .map(|entry| entry.path.clone())
    };

    let mut placed = Vec::new();
    for entry in &entries {
        if entry.main {
            placed.push(Placed {
                entry: entry.clone(),
                kind: Kind::Main,
                parent: None,
            });
            continue;
        }
        let pr = prs.iter().find(|pr| same_worktree(&pr.path, &entry.path));
        // A one-PR plan builds in the run's own checkout, which stays what it was.
        let pr = pr.filter(|pr| {
            pr.workspace
                .as_deref()
                .is_none_or(|workspace| !same_worktree(workspace, &entry.path))
        });
        if let Some(pr) = pr {
            let parent = pr
                .stacked_on
                .as_deref()
                .and_then(by_branch)
                .or_else(|| pr.workspace.as_deref().and_then(known))
                .or_else(|| main.clone());
            placed.push(Placed {
                entry: entry.clone(),
                kind: Kind::Pr {
                    plan: pr.plan.clone(),
                    slice_key: pr.slice_key.clone(),
                },
                parent,
            });
            continue;
        }
        // Waiting in the pool for somebody: not a place anyone is working.
        if entry.status == "available" {
            continue;
        }
        placed.push(Placed {
            entry: entry.clone(),
            kind: Kind::Manual,
            parent: main.clone(),
        });
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, status: &str, branch: Option<&str>, main: bool) -> PoolEntry {
        PoolEntry {
            name: path.rsplit('/').next().unwrap_or(path).into(),
            path: path.into(),
            status: status.into(),
            lease_holder: None,
            processes: Vec::new(),
            branch: branch.map(str::to_string),
            main,
        }
    }

    fn pr(path: &str, key: &str, workspace: &str, stacked_on: Option<&str>) -> PrFacts {
        PrFacts {
            path: path.into(),
            plan: "plan".into(),
            slice_key: key.into(),
            workspace: Some(workspace.into()),
            stacked_on: stacked_on.map(str::to_string),
        }
    }

    #[test]
    fn prs_sit_under_the_checkout_that_started_them_and_a_stack_under_its_parent() {
        let entries = vec![
            entry("/repo", "main", Some("ai-team/run-5"), true),
            entry(
                "/repo/.claude/worktrees/side",
                "linked",
                Some("side"),
                false,
            ),
            entry("/awt/1/repo", "leased", Some("plan/pr1"), false),
            entry("/awt/2/repo", "leased", Some("plan/pr2"), false),
            entry("/awt/3/repo", "leased", Some("plan/pr3"), false),
            entry("/awt/4/repo", "available", None, false),
            entry("/awt/5/repo", "leased", Some("other/pr1"), false),
        ];
        let prs = [
            pr("/awt/1/repo", "PR1", "/repo", None),
            pr("/awt/2/repo", "PR2", "/repo", Some("plan/pr1")),
            pr("/awt/3/repo", "PR3", "/repo", Some("plan/pr2")),
            // Started in the operator's own worktree, so it sits under that.
            pr("/awt/5/repo", "PR1", "/repo/.claude/worktrees/side", None),
        ];

        let placed = place(entries, &prs);

        let tree: Vec<(&str, Option<&str>, &Kind)> = placed
            .iter()
            .map(|p| (p.entry.path.as_str(), p.parent.as_deref(), &p.kind))
            .collect();
        assert_eq!(tree[0], ("/repo", None, &Kind::Main));
        assert_eq!(
            tree[1],
            ("/repo/.claude/worktrees/side", Some("/repo"), &Kind::Manual)
        );
        assert_eq!(tree[2].1, Some("/repo"));
        assert_eq!(tree[3].1, Some("/awt/1/repo"), "PR2 stacks on PR1");
        assert_eq!(tree[4].1, Some("/awt/2/repo"), "PR3 stacks on PR2");
        assert_eq!(tree[5].1, Some("/repo/.claude/worktrees/side"));
        assert!(matches!(tree[5].2, Kind::Pr { slice_key, .. } if slice_key == "PR1"));
        // The idle slot is not a place.
        assert_eq!(placed.len(), 6);
        assert!(placed.iter().all(|p| p.entry.path != "/awt/4/repo"));
    }

    #[test]
    fn a_one_pr_plan_built_in_place_leaves_its_checkout_what_it_was() {
        let placed = place(
            vec![
                entry("/repo", "main", Some("plan/pr1"), true),
                entry("/repo/wt", "linked", Some("plan2/pr1"), false),
            ],
            &[
                pr("/repo", "PR1", "/repo", None),
                pr("/repo/wt", "PR1", "/repo/wt", None),
            ],
        );
        assert_eq!(placed[0].kind, Kind::Main);
        assert_eq!(placed[1].kind, Kind::Manual);
        assert_eq!(placed[1].parent.as_deref(), Some("/repo"));
    }

    #[test]
    fn a_stack_whose_parent_is_gone_sits_under_its_run_instead() {
        // PR1 merged and its worktree returned: PR2 is not orphaned, it moves up.
        let placed = place(
            vec![
                entry("/repo", "main", Some("main"), true),
                entry("/awt/2/repo", "leased", Some("plan/pr2"), false),
            ],
            &[pr("/awt/2/repo", "PR2", "/repo", Some("plan/pr1"))],
        );
        assert_eq!(placed[1].parent.as_deref(), Some("/repo"));
    }
}
