//! Publishing accepted branches across explicit policy boundaries.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::{
    DeliveryAction, DeliveryPolicy, EventKind, NewEvent, NodeRun, RemoteDeliveryStatus,
};
use crate::neighbours::{git, github};
use crate::Store;

/// Perform one operator-approved boundary, then continue any following boundaries whose
/// policy is automatic.
pub async fn approve_delivery(
    db_path: &Path,
    node_run_id: i64,
    action: DeliveryAction,
) -> Result<NodeRun> {
    perform(db_path, node_run_id, action, true).await?;
    automatic_delivery(db_path, node_run_id).await;
    Store::open(db_path)?.node_run(node_run_id)
}

/// Read GitHub's current PR and check state. The PR URL remains persisted on the node;
/// this live status is a view over the remote authority and is never guessed from age.
pub async fn delivery_status(db_path: &Path, node_run_id: i64) -> Result<RemoteDeliveryStatus> {
    let (repo, pr_url) = {
        let store = Store::open(db_path)?;
        let node = store.node_run(node_run_id)?;
        let run = store.run(node.run_id)?;
        let repo = store
            .project_repos(run.project_id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| Error::invalid("that project has no checkout for delivery"))?;
        let pr_url = node
            .pr_url
            .ok_or_else(|| Error::invalid("that accepted node has no pull request"))?;
        (PathBuf::from(repo), pr_url)
    };
    github::pr_status(&repo, &pr_url).await
}

/// Best-effort auto delivery. A publishing account or repository failure is evidence on
/// the node, never a reason to turn verified local work into failed work.
pub(crate) async fn automatic_delivery(db_path: &Path, node_run_id: i64) {
    for action in [
        DeliveryAction::Push,
        DeliveryAction::Pr,
        DeliveryAction::Merge,
    ] {
        let policy =
            Store::open(db_path).and_then(|store| delivery_policy(&store, node_run_id, action));
        if !matches!(policy, Ok(DeliveryPolicy::Auto)) {
            continue;
        }
        if perform(db_path, node_run_id, action, false).await.is_err() {
            break;
        }
    }
}

struct PendingDelivery {
    run_id: i64,
    node_run_id: i64,
    repo: PathBuf,
    plan_root: PathBuf,
    plan_slug: Option<String>,
    slice_key: Option<String>,
    branch: String,
    /// The remote commit a rewritten branch may replace (PW11); `None` must fast-forward.
    replaces: Option<String>,
    base: Option<String>,
    pr_url: Option<String>,
}

fn claim_pending(
    db_path: &Path,
    node_run_id: i64,
    action: DeliveryAction,
    human_approved: bool,
) -> Result<Option<PendingDelivery>> {
    let mut store = Store::open(db_path)?;
    let policy = delivery_policy(&store, node_run_id, action)?;
    if policy == DeliveryPolicy::Manual || (policy == DeliveryPolicy::Ask && !human_approved) {
        return Err(Error::invalid(format!(
            "{} delivery is {}; change the team policy or perform it manually",
            action.as_str(),
            policy.as_str()
        )));
    }
    let node = store.node_run(node_run_id)?;
    let run = store.run(node.run_id)?;
    let repo = store
        .project_repos(run.project_id)?
        .into_iter()
        .find(|repo| repo.main_path.is_some())
        .ok_or_else(|| Error::invalid("that project has no checkout for delivery"))?;
    let plan_root = run.workspace_path.as_deref().map_or_else(
        || PathBuf::from(repo.main_path.as_deref().expect("filtered above")),
        PathBuf::from,
    );
    let branch = node
        .branch
        .clone()
        .ok_or_else(|| Error::invalid("that accepted node has no branch"))?;
    if !store.claim_delivery(node_run_id, action)? {
        return Ok(None);
    }
    Ok(Some(PendingDelivery {
        run_id: node.run_id,
        node_run_id,
        repo: PathBuf::from(repo.main_path.expect("filtered above")),
        plan_root,
        plan_slug: run.plan_slug,
        slice_key: node.slice_key,
        branch,
        replaces: node.push_replaces,
        base: repo.default_branch,
        pr_url: node.pr_url,
    }))
}

/// What a delivery step left behind - the PR's URL, for that step - and what to say it did.
struct Delivered {
    value: Option<String>,
    said: String,
}

async fn execute_delivery(pending: &PendingDelivery, action: DeliveryAction) -> Result<Delivered> {
    match action {
        DeliveryAction::Push => {
            git::push_branch(&pending.repo, &pending.branch, pending.replaces.as_deref()).await?;
            Ok(Delivered {
                value: None,
                said: format!("{} pushed to origin", pending.branch),
            })
        }
        DeliveryAction::Pr => {
            let (url, said) = create_pull_request(pending).await?;
            Ok(Delivered {
                value: Some(url),
                said,
            })
        }
        DeliveryAction::Merge => match pending.pr_url.as_deref() {
            Some(url) => {
                github::request_merge(&pending.repo, url).await?;
                Ok(Delivered {
                    value: None,
                    said: "requested merge after required checks".into(),
                })
            }
            None => Err(Error::invalid(
                "open the pull request before requesting its merge",
            )),
        },
    }
}

/// Open the branch's pull request - or find the one it has, pointed at the plan's base -
/// and say which.
async fn create_pull_request(pending: &PendingDelivery) -> Result<(String, String)> {
    let planner = pending
        .plan_slug
        .as_ref()
        .map(|plan| crate::Planner::at(&pending.plan_root).for_plan(plan));
    let planned_slice = match (planner.as_ref(), pending.slice_key.as_deref()) {
        (Some(planner), Some(key)) => planner
            .slices()
            .await
            .ok()
            .and_then(|slices| slices.into_iter().find(|slice| slice.key == key)),
        _ => None,
    };
    let planned_base = planned_slice
        .as_ref()
        .and_then(|slice| slice.base_branch.clone());
    let base = match planned_base.or_else(|| pending.base.clone()) {
        Some(base) => Some(base),
        None => git::default_branch(&pending.repo).await,
    }
    .ok_or_else(|| Error::invalid("could not determine the PR base branch"))?;
    let base = base.strip_prefix("origin/").unwrap_or(&base);
    let key = pending.slice_key.as_deref().unwrap_or("change");
    let (title, body) = pull_request_copy(
        pending.run_id,
        pending.node_run_id,
        key,
        planned_slice.as_ref(),
    );
    let (url, opened) =
        github::create_pr(&pending.repo, &pending.branch, base, &title, &body).await?;
    if let (Some(planner), Some(key)) = (planner.as_ref(), pending.slice_key.as_deref()) {
        planner.set_pr(key, &url).await?;
    }
    let said = pull_request_note(&url, &pending.branch, base, &opened);
    Ok((url, said))
}

fn pull_request_note(url: &str, branch: &str, base: &str, opened: &github::Opened) -> String {
    match opened {
        github::Opened::New => format!("opened pull request {url}"),
        github::Opened::Existing { was_based_on: None } => {
            format!("{branch} already has pull request {url}")
        }
        github::Opened::Existing {
            was_based_on: Some(before),
        } => format!("pull request {url} now merges into {base}, not {before}"),
    }
}

async fn perform(
    db_path: &Path,
    node_run_id: i64,
    action: DeliveryAction,
    human_approved: bool,
) -> Result<()> {
    let Some(pending) = claim_pending(db_path, node_run_id, action, human_approved)? else {
        return Ok(());
    };
    let result = execute_delivery(&pending, action).await;
    let mut store = Store::open(db_path)?;
    match result {
        Ok(delivered) => {
            store.complete_delivery(node_run_id, action, delivered.value.as_deref())?;
            store.append_event(
                pending.run_id,
                NewEvent::new(EventKind::Note, delivered.said)
                    .on_node(node_run_id)
                    .by("ai-team"),
            )?;
            Ok(())
        }
        Err(error) => {
            let reason = error.to_string();
            store.fail_delivery(node_run_id, action, &reason)?;
            store.append_event(
                pending.run_id,
                NewEvent::new(
                    EventKind::Failed,
                    format!("{} delivery failed: {reason}", action.as_str()),
                )
                .on_node(node_run_id)
                .by("ai-team"),
            )?;
            Err(error)
        }
    }
}

fn pull_request_copy(
    run_id: i64,
    node_run_id: i64,
    key: &str,
    planned: Option<&crate::Slice>,
) -> (String, String) {
    let title = planned
        .map(|slice| slice.title.trim())
        .filter(|title| !title.is_empty())
        .map_or_else(
            || format!("{key}: built by ai-team"),
            |title| format!("{key}: {title}"),
        );
    let mut body =
        format!("Built by ai-team run #{run_id} from node #{node_run_id}.\n\n## Slice\n\n`{key}`");
    if let Some(slice) = planned {
        if let Some(scope) = slice
            .scope_md
            .as_deref()
            .filter(|scope| !scope.trim().is_empty())
        {
            body.push_str("\n\n## Scope\n\n");
            body.push_str(scope.trim());
        }
        if let Some(demo) = slice
            .demo_md
            .as_deref()
            .filter(|demo| !demo.trim().is_empty())
        {
            body.push_str("\n\n## Verification\n\n");
            body.push_str(demo.trim());
        }
    }
    (title, body)
}

fn delivery_policy(
    store: &Store,
    node_run_id: i64,
    action: DeliveryAction,
) -> Result<DeliveryPolicy> {
    let node = store.node_run(node_run_id)?;
    let run = store.run(node.run_id)?;
    let project = store.project(run.project_id)?;
    let team_id = project
        .team_id
        .ok_or_else(|| Error::invalid("that project no longer has a team"))?;
    let delivery = store.team(team_id)?.delivery;
    Ok(match action {
        DeliveryAction::Push => delivery.push,
        DeliveryAction::Pr => delivery.pr,
        DeliveryAction::Merge => delivery.merge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, NewRepo, NodeStatus, RunTrigger};

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-qm", file]);
    }

    /// A project whose checkout is `repo`, and an accepted node on `branch` in it for
    /// each call of the function returned.
    fn project(db: &Path, repo: &Path) -> impl Fn(&str) -> i64 {
        let mut store = Store::init(db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .attach_repo(
                project.id,
                NewRepo {
                    main_path: Some(repo.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let orchestrator = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "orchestrator")
            .unwrap()
            .id;
        let (db, repo) = (db.to_path_buf(), repo.to_path_buf());
        move |branch| {
            let mut store = Store::open(&db).unwrap();
            let run = store
                .create_run(project.id, "restack", RunTrigger::Review)
                .unwrap();
            let node = store
                .dispatch(
                    run.id,
                    orchestrator,
                    Some("PR2"),
                    &crate::machine::ModelRegistry::local_only(),
                )
                .unwrap();
            store
                .attach_worktree(node.id, &repo.to_string_lossy(), Some(branch), None)
                .unwrap();
            store.set_node_status(node.id, NodeStatus::Done).unwrap();
            node.id
        }
    }

    #[test]
    fn the_pull_request_step_says_whether_it_opened_one_or_pointed_one_that_was_there() {
        let url = "https://github.com/o/w/pull/2";
        assert_eq!(
            pull_request_note(url, "p/pr2", "main", &github::Opened::New),
            "opened pull request https://github.com/o/w/pull/2"
        );
        assert_eq!(
            pull_request_note(
                url,
                "p/pr2",
                "main",
                &github::Opened::Existing { was_based_on: None }
            ),
            "p/pr2 already has pull request https://github.com/o/w/pull/2"
        );
        // A restack's: nothing was opened, and saying so hides the move that happened.
        assert_eq!(
            pull_request_note(
                url,
                "p/pr2",
                "main",
                &github::Opened::Existing {
                    was_based_on: Some("p/pr1".into())
                }
            ),
            "pull request https://github.com/o/w/pull/2 now merges into main, not p/pr1"
        );
    }

    #[tokio::test]
    async fn a_rewritten_branch_is_pushed_over_the_commit_it_was_rewritten_from_and_no_other() {
        let repo = tempfile::tempdir().unwrap();
        let origin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (repo, db) = (repo.path(), home.path().join("team.db"));
        git(origin.path(), &["init", "-q", "--bare", "."]);
        git(repo, &["init", "-q", "-b", "main", "."]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        git(
            repo,
            &["remote", "add", "origin", &origin.path().to_string_lossy()],
        );
        commit(repo, "readme");
        git(repo, &["checkout", "-qb", "p/pr1"]);
        commit(repo, "one");
        git(repo, &["checkout", "-qb", "p/pr2"]);
        commit(repo, "two");
        git(repo, &["push", "-q", "origin", "p/pr2"]);
        let published = git(repo, &["rev-parse", "HEAD"]);
        // PR1 merged, so PR2 is restacked onto main: the same work, not a fast-forward.
        git(repo, &["checkout", "-q", "main"]);
        commit(repo, "merged");
        git(repo, &["checkout", "-q", "p/pr2"]);
        git(repo, &["rebase", "-q", "--onto", "main", "p/pr1"]);
        let restacked = git(repo, &["rev-parse", "HEAD"]);

        let accepted = project(&db, repo);

        // Nothing pinned: an ordinary push, which must fast-forward, and this cannot.
        let plain = accepted("p/pr2");
        assert!(approve_delivery(&db, plain, DeliveryAction::Push)
            .await
            .is_err());
        assert_eq!(git(origin.path(), &["rev-parse", "p/pr2"]), published);

        let pinned = accepted("p/pr2");
        Store::open(&db)
            .unwrap()
            .set_push_replaces(pinned, &published)
            .unwrap();
        let node = approve_delivery(&db, pinned, DeliveryAction::Push)
            .await
            .unwrap();

        assert!(node.pushed_at.is_some());
        assert_eq!(git(origin.path(), &["rev-parse", "p/pr2"]), restacked);
    }
}
