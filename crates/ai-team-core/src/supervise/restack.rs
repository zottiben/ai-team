//! A restack's turn, and what ai-team makes of it (PW11).
//!
//! ai-team chooses the commits and the orchestrator runs the rebase: a clean one needs
//! nobody, but a conflict needs somebody who can read both sides and what the pull request
//! set out to do. Then ai-team checks what it was left, from git and from the project's own
//! gates, because a rebase that reads as finished is not the same as one that is. Nothing
//! here publishes.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{EventKind, NewEvent, NodeRun, NodeStatus};
use crate::neighbours::git;
use crate::restack::Restack;
use crate::store::Store;
use crate::supervise::outcome_status;

use super::orchestrate::{Dispatched, Orchestrator, Rig};

impl Orchestrator {
    /// Restack one PR on the node the watch opened for it, and say what came of it.
    ///
    /// The PR keeps its worktree whatever happens - it is where it is reviewed, and a
    /// restack that stopped is one a person picks up there. What was restacked goes to
    /// delivery as accepted work: pushed over the one commit it was rewritten from, and
    /// pointed at its new base once it is, under the team's own policy (PW11).
    pub(crate) async fn restack(
        &self,
        store: &mut Store,
        restack: &Restack,
        node_run_id: i64,
    ) -> Result<Dispatched> {
        let node = store.node_run(node_run_id)?;
        let in_place = crate::neighbours::same_worktree(
            &restack.worktree.to_string_lossy(),
            &self.repo.to_string_lossy(),
        );
        // Taken back as it was left: still the PR's, and nothing else at work in it.
        let taken = if in_place {
            Ok(None)
        } else {
            self.worktrees.reattach(&restack.worktree).await.map(Some)
        };
        let restacked = match taken {
            Ok(lease) => {
                let restacked = restack_pr(store, &self.rig(), restack, &node).await;
                if let Some(lease) = lease {
                    lease.preserve();
                }
                restacked?
            }
            Err(error) => Restacked::Parked(error.to_string()),
        };

        let key = &restack.slice_key;
        let status = match &restacked {
            Restacked::Done { replaces } => {
                if let Some(sha) = replaces {
                    store.set_push_replaces(node.id, sha)?;
                }
                if let Some(base) = &restack.new_base {
                    // What delivery points the pull request at, once it has pushed it.
                    self.planner.set_slice_base(key, base).await?;
                }
                self.note(
                    store,
                    &node,
                    format!("{key} restacked onto {}", restack.onto_ref),
                )?;
                NodeStatus::Done
            }
            Restacked::Current => {
                // Nothing rewritten, so nothing of this node's to publish: the PR's
                // delivery stays with the node whose branch it is.
                store.attach_worktree(node.id, &restack.worktree.to_string_lossy(), None, None)?;
                self.note(
                    store,
                    &node,
                    format!(
                        "{key} is already on {} - nothing to restack",
                        restack.onto_ref
                    ),
                )?;
                NodeStatus::Done
            }
            Restacked::Parked(reason) => {
                store.block_node(node.id, reason)?;
                let _ = self
                    .planner
                    .log(
                        &format!("ai-team: {key} needs restacking by hand - {reason}"),
                        Some(key),
                    )
                    .await;
                NodeStatus::Parked
            }
            Restacked::Failed(reason) => {
                store.block_node(node.id, &format!("the restack could not run: {reason}"))?;
                NodeStatus::Failed
            }
        };
        store.set_node_status(node.id, status)?;
        notify(store, &node, restack, status)?;
        if status == NodeStatus::Done && matches!(restacked, Restacked::Done { .. }) {
            crate::delivery::automatic_delivery(&self.db_path, node.id).await;
        }
        let node = store.node_run(node.id)?;
        Ok(Dispatched {
            slice_key: key.clone(),
            role: node.role.clone(),
            worktree: restack.worktree.clone(),
            node_run_id: node.id,
            status,
            outcome: super::TurnOutcome::default(),
            branch: node.branch,
        })
    }

    fn note(&self, store: &mut Store, node: &NodeRun, what: String) -> Result<()> {
        store.append_event(
            self.run_id,
            NewEvent::new(EventKind::Note, what)
                .on_node(node.id)
                .by("ai-team"),
        )?;
        Ok(())
    }
}

/// Tell the operator how a restack went: done, or that it needs them.
fn notify(store: &mut Store, node: &NodeRun, restack: &Restack, status: NodeStatus) -> Result<()> {
    let key = &restack.slice_key;
    let reason = store.node_run(node.id)?.blocked_reason;
    let (kind, title, body) = match status {
        NodeStatus::Done => (
            "completed",
            format!("{key} restacked"),
            format!("{key} is on {} again: {}.", restack.onto_ref, restack.why),
        ),
        NodeStatus::Parked => (
            "input_required",
            format!("{key} needs restacking"),
            reason.unwrap_or_else(|| format!("{key} could not be restacked.")),
        ),
        _ => (
            "failed",
            format!("{key} was not restacked"),
            reason.unwrap_or_else(|| format!("{key} could not be restacked.")),
        ),
    };
    let run = store.run(node.run_id)?;
    let project = store.project(run.project_id)?;
    store.notify_once(crate::model::NewNotification {
        dedupe_key: format!("node:{}:{}", node.id, status.as_str()),
        project_id: run.project_id,
        workspace_path: run.workspace_path,
        run_id: Some(run.id),
        node_run_id: Some(node.id),
        kind: kind.into(),
        title: format!("{} · {title}", project.name),
        body,
        action_path: None,
    })?;
    Ok(())
}

/// What a restack came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Restacked {
    /// On its new base with its own work kept and the project's checks passing, ready to
    /// publish. `replaces` is the commit origin has for the branch, which is the one its
    /// push may overwrite; `None` for a branch never pushed.
    Done { replaces: Option<String> },
    /// Already there: somebody rebased it, or there was nothing to move.
    Current,
    /// Stopped for a person, and why.
    Parked(String),
    /// The turn could not be taken at all - a model out of reach. Worth another go.
    Failed(String),
}

/// Rebase one stacked PR onto where its parent is now, in its own worktree, on `node`.
///
/// Left as it was found whenever it stops: a rebase given up part-way is undone, so the
/// PR's worktree is always on its branch with nothing half-applied in it.
pub(crate) async fn restack_pr(
    store: &mut Store,
    rig: &Rig,
    restack: &Restack,
    node: &NodeRun,
) -> Result<Restacked> {
    let worktree = restack.worktree.as_path();
    let branch = restack.branch.as_str();
    if let Some(reason) = unready(worktree, branch).await? {
        return Ok(Restacked::Parked(reason));
    }
    if git::is_ancestor(worktree, &restack.onto, branch).await? {
        return Ok(Restacked::Current);
    }
    let replaces = match published(worktree, branch).await? {
        Published::Nowhere => None,
        Published::At(sha) => Some(sha),
        Published::Diverged => {
            return Ok(Restacked::Parked(format!(
                "{branch} has commits on origin this worktree does not, and its own that \
                 origin does not. Bring them together before it can be restacked - \
                 rewriting it now would lose one side."
            )))
        }
    };

    let agent_id = node
        .agent_id
        .ok_or_else(|| Error::invalid("the restack's seat no longer exists"))?;
    store.set_node_status(node.id, NodeStatus::Running)?;
    let turn = rig.restack_seat(store, agent_id, node.id, worktree, &prompt(restack))?;
    let mut said = String::new();
    let (_, outcome) = crate::run_pi_turn(store, node.id, &turn, |event| {
        if let Some(message) = event.assistant_message() {
            said = message;
        }
    })
    .await?;
    let said = said.trim();

    // Read from git, not from the answer: "rebased cleanly" is what a turn that stopped
    // half-way says too.
    if git::rebase_in_progress(worktree).await? {
        git::abort_rebase(worktree).await?;
        return Ok(Restacked::Parked(format!(
            "the rebase was left part-way, so ai-team undid it and {branch} is as it was. {}",
            if said.is_empty() {
                "The turn said nothing about why."
            } else {
                said
            }
        )));
    }
    if outcome_status(&outcome) != NodeStatus::Done {
        let reason = crate::supervise::outcome::provider_diagnostic(&outcome)
            .unwrap_or_else(|| "the turn did not finish".to_string());
        return Ok(Restacked::Failed(reason));
    }
    if git::current_branch(worktree).await.as_deref() != Some(branch) {
        return Ok(Restacked::Parked(format!(
            "the turn left this worktree off {branch}, so ai-team published nothing"
        )));
    }
    if !git::porcelain(worktree).await?.trim().is_empty() {
        return Ok(Restacked::Parked(format!(
            "the rebase finished with changes left uncommitted in {branch}'s worktree, so \
             what it would publish is not what it was checked as"
        )));
    }
    if !git::is_ancestor(worktree, &restack.onto, branch).await? {
        // It stopped - asked, or could not - and aborted, which is what it was told to do.
        return Ok(Restacked::Parked(if said.is_empty() {
            format!("{branch} was not rebased, and the turn did not say why")
        } else {
            said.to_string()
        }));
    }
    if git::commits_between(worktree, &restack.onto, branch).await? == 0 {
        return Ok(Restacked::Parked(format!(
            "nothing of {branch}'s own is left on {}: its changes may already be there, and \
             an empty pull request is a question for a person",
            restack.onto_ref
        )));
    }

    let gates = crate::gates::discover_gates(worktree);
    let results = crate::gates::run_gates(worktree, &gates).await?;
    super::build::record_gates(store, node.run_id, node.id, &results)?;
    if !crate::gates::all_passed(&results) {
        return Ok(Restacked::Parked(format!(
            "{branch} is restacked, but the project's checks fail on it, so it was not \
             published:\n\n{}",
            crate::gates::evidence(&results)
        )));
    }
    if gates.is_empty() {
        store.append_event(
            node.run_id,
            NewEvent::new(
                EventKind::Note,
                "no gates found in this worktree - nothing automatic was run",
            )
            .on_node(node.id),
        )?;
    }
    Ok(Restacked::Done { replaces })
}

/// Why a PR's worktree cannot be restacked as it stands, if it cannot.
async fn unready(worktree: &Path, branch: &str) -> Result<Option<String>> {
    if git::rebase_in_progress(worktree).await? {
        return Ok(Some(format!(
            "a rebase is already part-way in {branch}'s worktree - somebody's, and not \
             ai-team's to finish or throw away"
        )));
    }
    if git::current_branch(worktree).await.as_deref() != Some(branch) {
        return Ok(Some(format!(
            "{} is not on {branch} any more, so it is not the pull request's to rebase",
            worktree.display()
        )));
    }
    if !git::porcelain(worktree).await?.trim().is_empty() {
        return Ok(Some(format!(
            "{branch}'s worktree has uncommitted changes, which a rebase would carry into \
             somebody else's history"
        )));
    }
    Ok(None)
}

enum Published {
    Nowhere,
    At(String),
    Diverged,
}

/// Where origin has the PR's branch, brought into this checkout if somebody pushed to it.
///
/// A commit on origin that the worktree lacks - a suggestion accepted on GitHub - is part
/// of the PR, so it is taken in before the rebase rather than overwritten by the push.
async fn published(worktree: &Path, branch: &str) -> Result<Published> {
    if git::fetch(worktree, branch).await.is_err() {
        // No such branch on origin: never pushed, so nothing there to replace.
        return Ok(Published::Nowhere);
    }
    let remote = format!("refs/remotes/origin/{branch}");
    let Ok(sha) = git::rev_parse(worktree, &remote).await else {
        return Ok(Published::Nowhere);
    };
    if git::is_ancestor(worktree, &sha, branch).await? {
        return Ok(Published::At(sha));
    }
    if git::is_ancestor(worktree, branch, &sha).await? {
        git::fast_forward(worktree, &sha).await?;
        return Ok(Published::At(sha));
    }
    Ok(Published::Diverged)
}

/// What the orchestrator is asked to do: the exact rebase, and what the PR is for.
fn prompt(restack: &Restack) -> String {
    let short = |sha: &str| sha.get(..8).unwrap_or(sha).to_string();
    let mut out = format!(
        "Restack {key} - {title} - onto {onto_ref}. {why}.\n\n\
         {branch} sits on `{upstream}`, the commit of {parent} it was built on; everything \
         after that is this pull request's own work. Rebase that work onto {onto_ref}, which \
         is `{onto}` now:\n\n    git rebase --onto {onto_full} {upstream_full}\n",
        key = restack.slice_key,
        title = restack.title,
        onto_ref = restack.onto_ref,
        why = restack.why,
        branch = restack.branch,
        upstream = short(&restack.upstream),
        parent = restack.parent_key,
        onto = short(&restack.onto),
        onto_full = restack.onto,
        upstream_full = restack.upstream,
    );
    if let Some(base) = &restack.new_base {
        let _ = write!(
            out,
            "\nFrom now on it merges into `{base}`: ai-team points the pull request there once \
             it has published the result.\n"
        );
    }
    let scope = restack.scope.trim();
    if !scope.is_empty() {
        let _ = write!(out, "\n## What this pull request is for\n\n{scope}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::ModelRegistry;
    use crate::model::NewRestack;

    /// A stand-in for the orchestrator: runs the rebase its prompt names, then does what
    /// its `mode` file says a model would with a conflict - resolve it keeping both
    /// sides, ask about it, or walk away from it.
    const FAKE_PI: &str = r#"#!/bin/sh
here=$(cd "$(dirname "$0")" && pwd)
mode=$(cat "$here/mode")
echo called >> "$here/calls"
prompt=""
for arg in "$@"; do prompt="$arg"; done
settle() {
  printf '{"type":"session","id":"restack"}\n'
  printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\n' "$1"
  printf '{"type":"agent_settled"}\n'
}
rebase=$(printf '%s\n' "$prompt" | sed -n 's/^ *\(git rebase --onto [0-9a-f]* [0-9a-f]*\)$/\1/p' | head -n 1)
if [ "$mode" = "unreachable" ]; then
  printf '{"type":"session","id":"restack"}\n'
  printf '{"type":"agent_error","error":"the model could not be reached"}\n'
  printf '{"type":"agent_settled"}\n'
  exit 0
fi
if $rebase >/dev/null 2>&1; then settle "rebased cleanly"; exit 0; fi
case "$mode" in
  resolve)
    printf 'from main\nfrom this pull request\n' > shared.txt
    git add shared.txt
    GIT_EDITOR=true git rebase --continue >/dev/null 2>&1
    settle "rebased; kept both sides of shared.txt" ;;
  ask)
    git rebase --abort
    settle "shared.txt: main rewrote the line this pull request changes. Which wins?" ;;
  *) settle "rebased cleanly" ;;
esac
"#;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, text: &str) -> String {
        commit_as(dir, file, text, file)
    }

    fn commit_as(dir: &Path, file: &str, text: &str, message: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-qm", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    /// PR1 on main and PR2 stacked on it, both pushed, then PR1 squash-merged: the stack a
    /// restack meets. `ui_test` is the only gate.
    struct Stack {
        store: Store,
        node: NodeRun,
        rig: Rig,
        restack: Restack,
        repo: tempfile::TempDir,
        origin: tempfile::TempDir,
        support: tempfile::TempDir,
        own: String,
    }

    fn stack(mode: &str, ui_test: &str) -> Stack {
        let repo = tempfile::tempdir().unwrap();
        let origin = tempfile::tempdir().unwrap();
        let dir = repo.path();
        git(origin.path(), &["init", "-q", "--bare", "."]);
        git(dir, &["init", "-q", "-b", "main", "."]);
        for [key, value] in [
            ["user.email", "t@t"],
            ["user.name", "t"],
            ["commit.gpgsign", "false"],
        ] {
            git(dir, &["config", key, value]);
        }
        git(
            dir,
            &["remote", "add", "origin", &origin.path().to_string_lossy()],
        );
        std::fs::create_dir(dir.join("ui")).unwrap();
        std::fs::write(
            dir.join("ui/package.json"),
            serde_json::json!({ "scripts": { "test": ui_test } }).to_string(),
        )
        .unwrap();
        git(dir, &["add", "ui/package.json"]);
        commit(dir, "shared.txt", "as it was\n");
        git(dir, &["checkout", "-qb", "p/pr1"]);
        commit(dir, "one.txt", "one\n");
        git(dir, &["checkout", "-qb", "p/pr2"]);
        let own = commit(dir, "two.txt", "two\n");
        git(dir, &["push", "-q", "origin", "main", "p/pr1", "p/pr2"]);
        let upstream = git(dir, &["rev-parse", "p/pr1"]);
        // PR1 squash-merged: main has its change, not its commit. Titled as GitHub titles
        // a squash - and not as PR1's commit was, or within the same second the two are
        // the same commit, and main is already under PR2.
        git(dir, &["checkout", "-q", "main"]);
        commit_as(dir, "one.txt", "one\n", "Add one (#1)");
        git(dir, &["push", "-q", "origin", "main"]);
        git(dir, &["checkout", "-q", "p/pr2"]);
        let onto = git(dir, &["rev-parse", "main"]);

        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let orchestrator = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "orchestrator")
            .unwrap();
        let resolution = ModelRegistry::local_only().resolve(&orchestrator).unwrap();
        let worktree = dir.to_string_lossy().into_owned();
        let (_, node) = store
            .open_restack(NewRestack {
                project_id: project.id,
                workspace: dir,
                plan: "p",
                slice_key: "PR2",
                branch: "p/pr2",
                worktree: &worktree,
                onto: &onto,
                prompt: "Restack PR2 onto origin/main: PR1 was merged into main",
                agent_id: orchestrator.id,
                resolution: &resolution,
            })
            .unwrap()
            .unwrap();

        let support = tempfile::tempdir().unwrap();
        let pi = support.path().join("pi");
        std::fs::write(&pi, FAKE_PI).unwrap();
        std::fs::write(support.path().join("mode"), mode).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let rig = Rig {
            support: support.path().into(),
            plan_root: dir.into(),
            plan: None,
            sources: Vec::new(),
            registry: ModelRegistry::local_only(),
            pi: Some(pi),
        };
        Stack {
            store,
            node,
            rig,
            restack: Restack {
                plan: "p".into(),
                slice_key: "PR2".into(),
                title: "Show both forms".into(),
                scope: "Show the shout form beside the normal one.\n\nTouches: web/**".into(),
                branch: "p/pr2".into(),
                worktree: dir.into(),
                parent_key: "PR1".into(),
                onto_ref: "origin/main".into(),
                onto,
                upstream,
                new_base: Some("main".into()),
                why: "PR1 was merged into main".into(),
            },
            repo,
            origin,
            support,
            own,
        }
    }

    impl Stack {
        async fn restack(&mut self) -> Restacked {
            restack_pr(&mut self.store, &self.rig, &self.restack, &self.node)
                .await
                .unwrap()
        }

        fn dir(&self) -> &Path {
            self.repo.path()
        }

        /// The commits `branch` has beyond `main`, by message.
        fn own_commits(&self) -> String {
            git(self.dir(), &["log", "--format=%s", "main..p/pr2"])
        }

        fn turns(&self) -> usize {
            std::fs::read_to_string(self.support.path().join("calls"))
                .map_or(0, |calls| calls.lines().count())
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pr_whose_parent_merged_is_rebased_onto_where_it_landed_with_only_its_own_work() {
        let mut s = stack("clean", "true");
        let published = git(s.origin.path(), &["rev-parse", "p/pr2"]);

        let restacked = s.restack().await;

        assert_eq!(
            restacked,
            Restacked::Done {
                replaces: Some(published)
            }
        );
        // PR1's commit is not replayed on top of its own squash: two.txt alone.
        assert_eq!(s.own_commits(), "two.txt");
        assert_ne!(git(s.dir(), &["rev-parse", "p/pr2"]), s.own);
        let checked = s.store.node_events(s.node.id, 50).unwrap();
        assert!(
            checked
                .iter()
                .any(|event| event.summary == "gate test `(ui) npm run test` passed"),
            "{checked:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_conflict_is_resolved_by_the_orchestrator_and_the_result_checked() {
        let mut s = stack("resolve", "true");
        // Both sides changed shared.txt: main since, and PR2 on top of PR1.
        git(s.dir(), &["checkout", "-q", "main"]);
        let onto = commit(s.dir(), "shared.txt", "from main\n");
        git(s.dir(), &["checkout", "-q", "p/pr2"]);
        commit(s.dir(), "shared.txt", "from this pull request\n");
        s.restack.onto = onto;

        assert!(matches!(s.restack().await, Restacked::Done { .. }));
        assert_eq!(
            std::fs::read_to_string(s.dir().join("shared.txt")).unwrap(),
            "from main\nfrom this pull request\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_orchestrator_unsure_of_a_conflict_asks_and_the_branch_is_left_as_it_was() {
        let mut s = stack("ask", "true");
        git(s.dir(), &["checkout", "-q", "main"]);
        let onto = commit(s.dir(), "shared.txt", "from main\n");
        git(s.dir(), &["checkout", "-q", "p/pr2"]);
        let before = commit(s.dir(), "shared.txt", "from this pull request\n");
        s.restack.onto = onto;

        let restacked = s.restack().await;

        assert_eq!(
            restacked,
            Restacked::Parked(
                "shared.txt: main rewrote the line this pull request changes. Which wins?".into()
            )
        );
        assert_eq!(git(s.dir(), &["rev-parse", "HEAD"]), before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rebase_walked_away_from_is_undone_whatever_the_turn_said() {
        let mut s = stack("walk away", "true");
        git(s.dir(), &["checkout", "-q", "main"]);
        let onto = commit(s.dir(), "shared.txt", "from main\n");
        git(s.dir(), &["checkout", "-q", "p/pr2"]);
        let before = commit(s.dir(), "shared.txt", "from this pull request\n");
        s.restack.onto = onto;

        let Restacked::Parked(reason) = s.restack().await else {
            panic!("a half-finished rebase was taken for a finished one");
        };

        assert!(reason.contains("left part-way"), "{reason}");
        assert!(!git::rebase_in_progress(s.dir()).await.unwrap());
        assert_eq!(git(s.dir(), &["rev-parse", "HEAD"]), before);
        assert_eq!(git::current_branch(s.dir()).await.as_deref(), Some("p/pr2"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_restack_the_checks_fail_on_is_not_ready_to_publish() {
        let mut s = stack("clean", "false");

        let Restacked::Parked(reason) = s.restack().await else {
            panic!("published a branch its own checks fail on");
        };
        assert!(reason.contains("the project's checks fail"), "{reason}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_that_could_not_run_failed_rather_than_asked() {
        let mut s = stack("unreachable", "true");
        assert!(matches!(s.restack().await, Restacked::Failed(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nothing_is_asked_of_a_model_for_a_worktree_that_is_not_ready_or_a_pr_already_moved() {
        let mut s = stack("clean", "true");
        std::fs::write(s.dir().join("two.txt"), "edited by hand\n").unwrap();
        let Restacked::Parked(reason) = s.restack().await else {
            panic!("rebased over somebody's uncommitted work");
        };
        assert!(reason.contains("uncommitted changes"), "{reason}");

        let mut s = stack("clean", "true");
        git(s.dir(), &["rebase", "-q", "--onto", "main", "p/pr1"]);
        assert_eq!(s.restack().await, Restacked::Current);
        assert_eq!(s.turns(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_commit_pushed_to_the_pr_on_github_is_kept_not_overwritten() {
        let mut s = stack("clean", "true");
        let theirs = tempfile::tempdir().unwrap();
        let origin = s.origin.path().to_string_lossy().into_owned();
        git(
            theirs.path(),
            &["clone", "-q", "--branch", "p/pr2", &origin, "."],
        );
        git(theirs.path(), &["config", "user.email", "o@o"]);
        git(theirs.path(), &["config", "user.name", "o"]);
        let suggestion = commit(theirs.path(), "suggested.txt", "accepted on GitHub\n");
        git(theirs.path(), &["push", "-q", "origin", "p/pr2"]);

        let restacked = s.restack().await;

        assert_eq!(
            restacked,
            Restacked::Done {
                replaces: Some(suggestion)
            }
        );
        assert_eq!(s.own_commits(), "suggested.txt\ntwo.txt");

        // Diverged: theirs on origin, and a commit of this worktree's origin lacks.
        let mut s = stack("clean", "true");
        let origin = s.origin.path().to_string_lossy().into_owned();
        let theirs = tempfile::tempdir().unwrap();
        git(
            theirs.path(),
            &["clone", "-q", "--branch", "p/pr2", &origin, "."],
        );
        git(theirs.path(), &["config", "user.email", "o@o"]);
        git(theirs.path(), &["config", "user.name", "o"]);
        commit(theirs.path(), "suggested.txt", "accepted on GitHub\n");
        git(theirs.path(), &["push", "-q", "origin", "p/pr2"]);
        commit(s.dir(), "local.txt", "not pushed\n");
        let Restacked::Parked(reason) = s.restack().await else {
            panic!("rewrote a branch that had diverged from origin");
        };
        assert!(reason.contains("Bring them together"), "{reason}");
    }
}
