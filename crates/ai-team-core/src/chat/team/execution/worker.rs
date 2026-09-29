//! One leased slice: maker -> gates -> independent reader -> exact-tree draft commit.

use super::{git, Watch};
use crate::{
    ChatBuildControl, ChatBuildSlice, Error, ModelRegistry, NewEvent, NodeStatus, Result, Store,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Semaphore;

pub(super) async fn run(
    db: &Path,
    control: &ChatBuildControl,
    key: &str,
    verifier: Arc<Semaphore>,
) -> Result<()> {
    let mut store = Store::open(db)?;
    let watch = Watch::new(db, control);
    watch.check()?;
    store
        .prepare_chat_build_slice(control, key, &ModelRegistry::load()?)
        .await?;
    let approved = store.begin_chat_slice_worker(control, key)?;
    let slice = store.owned_chat_build_slice(control, key)?;
    let path = PathBuf::from(
        approved
            .worktree_path
            .as_deref()
            .ok_or_else(|| Error::invalid("missing approved lease"))?,
    );
    let branch = approved
        .branch
        .clone()
        .ok_or_else(|| Error::invalid("missing draft branch"))?;
    let maker = store.chat_build_agent(control, key)?;
    let mut worker = Worker {
        store,
        watch,
        approved,
        slice,
        path,
        branch,
        maker,
        verifier,
    };
    worker.build().await?;
    worker.return_verified().await
}

struct Worker {
    store: Store,
    watch: Watch,
    approved: ChatBuildSlice,
    slice: ai_planner_core::Slice,
    path: PathBuf,
    branch: String,
    maker: crate::Agent,
    verifier: Arc<Semaphore>,
}

impl Worker {
    async fn build(&mut self) -> Result<()> {
        let base = self.watch.control.base_sha.clone();
        git::bound(&self.path, &base, &self.branch, &self.watch).await?;
        let base_tree = git::text(
            &self.path,
            &["rev-parse", &format!("{base}^{{tree}}")],
            &self.watch,
        )
        .await?;
        let setup =
            crate::gates::prepare_dependencies_until(&self.path, || self.watch.clone().wait())
                .await?;
        if !setup.is_empty() {
            self.note(
                None,
                &format!("Prepared dependencies: {}", setup.join(", ")),
            )?;
        }
        if git::snapshot(&self.path, &self.watch).await? != base_tree {
            return Err(Error::invalid("dependency setup changed source; retain it for inspection before spending a model turn"));
        }
        let max_repairs = self.store.run(self.approved.run_id)?.max_repairs;
        let mut previous = None;
        let mut prompt = format!("Build only your approved slice {}. Read the assigned scope and house rules in your instructions. Leave the work uncommitted for independent checks.", self.slice.key);
        for repair in 0..=max_repairs {
            let (node, _) = self
                .take_turn(self.maker.id, false, previous, prompt)
                .await?;
            if let Some(limit) = crate::node_may_continue(&self.store, node)? {
                return Err(Error::invalid(limit.reason));
            }
            let candidate = self.check(node).await;
            match candidate {
                Ok(tree) => {
                    git::bound(&self.path, &base, &self.branch, &self.watch).await?;
                    let sha = git::candidate(
                        &self.path,
                        &tree,
                        &base,
                        &format!("{}: {}", self.slice.key, self.slice.title),
                        &self.watch,
                    )
                    .await?;
                    self.store
                        .record_chat_candidate(&self.watch.control, &self.slice.key, &sha)?;
                    git::publish_local(&self.path, &self.branch, &sha, &base, &self.watch).await?;
                    self.store
                        .record_chat_commit(&self.watch.control, &self.slice.key, &sha)?;
                    return Ok(());
                }
                Err(error) => {
                    let reason = error.to_string();
                    self.note(
                        Some(node),
                        &format!("Attempt {} rejected: {reason}", repair + 1),
                    )?;
                    self.watch.check()?;
                    self.store.settle_chat_build_member(
                        &self.watch.control,
                        node,
                        NodeStatus::Failed,
                        Some(&reason),
                    )?;
                    if repair == max_repairs {
                        return Err(error);
                    }
                    if let Some(limit) = crate::node_may_continue(&self.store, node)? {
                        return Err(Error::invalid(limit.reason));
                    }
                    // If an infrastructure failure left a live reader, do not start a repair beside it.
                    if self
                        .store
                        .chat_team_members(self.approved.run_id)?
                        .iter()
                        .any(|member| {
                            member.node.slice_key.as_deref() == Some(&self.slice.key)
                                && member.pi_alive()
                        })
                    {
                        return Err(Error::invalid(
                            "a reader still owns this worktree; retain it for recovery",
                        ));
                    }
                    previous = Some(node);
                    prompt = format!("Repair the same approved slice {} in this existing lease. Keep prior work and address this specific rejection; do not commit or reset it.\n\n{reason}", self.slice.key);
                }
            }
        }
        Err(Error::invalid("the run has no usable repair allowance"))
    }

    async fn source_unchanged(&mut self) -> Result<()> {
        let chat = self.store.chat(self.watch.control.receipt.chat_id)?;
        let source = Path::new(&chat.workspace_path);
        if git::text(source, &["rev-parse", "HEAD"], &self.watch).await?
            != self.watch.control.base_sha
            || !git::text(
                source,
                &[
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all",
                    "--ignore-submodules=none",
                ],
                &self.watch,
            )
            .await?
            .is_empty()
        {
            return Err(Error::invalid("the human checkout changed after approval; keep its files and review before continuing"));
        }
        Ok(())
    }

    async fn check(&mut self, maker: i64) -> Result<String> {
        git::bound(
            &self.path,
            &self.watch.control.base_sha,
            &self.branch,
            &self.watch,
        )
        .await?;
        let candidate = git::snapshot(&self.path, &self.watch).await?;
        let changes = git::changes(
            &self.path,
            &self.watch.control.base_sha,
            &candidate,
            &self.watch,
        )
        .await?;
        if changes.is_empty() {
            return Err(Error::invalid("the maker finished without changing a file"));
        }
        let touches = crate::planning::slice_touches(&self.slice.scope_md)
            .into_iter()
            .map(|path| {
                if path.ends_with('/') {
                    format!("{path}**")
                } else {
                    path
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        for path in &changes {
            if !crate::util::zone_matches(&self.maker.zone, path)
                || !crate::util::zone_matches(&touches, path)
            {
                return Err(Error::invalid(format!(
                    "changed path {path} is outside the approved slice or maker zone"
                )));
            }
        }
        let gates = crate::gates::discover_gates(&self.path);
        if gates.is_empty() {
            return Err(Error::invalid(
                "no project gates were discovered; nothing automatic was verified",
            ));
        }
        let mut results = Vec::new();
        for gate in &gates {
            self.watch.check()?;
            self.note(Some(maker), &format!("Running gate `{}`", gate.command()))?;
            let result =
                crate::gates::run_one_until(&self.path, gate, self.watch.clone().wait()).await?;
            self.store.append_event(self.approved.run_id, NewEvent::new(if result.passed { crate::EventKind::Note } else { crate::EventKind::Failed }, format!("gate `{}` {}", gate.command(), if result.passed { "passed" } else { "failed" }))
                .on_node(maker).by("ai-team").with(serde_json::json!({"gate":gate.kind.as_str(),"command":gate.command(),"passed":result.passed,"output":result.output})))?;
            let failed = !result.passed;
            results.push(result);
            if failed {
                return Err(Error::invalid(crate::gates::evidence(&results)));
            }
        }
        self.unchanged_candidate(&candidate).await?;
        let permit = tokio::select! {
            result = self.verifier.clone().acquire_owned() => result.map_err(|_| Error::invalid("the verifier seat closed"))?,
            reason = self.watch.clone().wait() => return Err(Error::invalid(reason)),
        };
        let run = self.store.run(self.approved.run_id)?;
        let reader = self
            .store
            .agents(
                run.team_id
                    .ok_or_else(|| Error::invalid("this execution's team disappeared"))?,
            )?
            .into_iter()
            .find(|agent| agent.role == crate::VERIFIER_ROLE && agent.enabled && agent.read_only)
            .ok_or_else(|| Error::invalid("this build needs an enabled read-only verifier"))?;
        let prompt = format!("Independently verify the assigned slice in this exact leased tree. Check existence, substantive implementation, and wiring. Do not edit source. Gate evidence:\n\n{}\n\nFinish with VERDICT: pass or VERDICT: reject on its own line.", crate::gates::evidence(&results));
        let (_, said) = self.take_turn(reader.id, true, None, prompt).await?;
        drop(permit);
        self.unchanged_candidate(&candidate).await?;
        if !accepted(&said) {
            return Err(Error::invalid(format!(
                "the verifier did not explicitly accept this work:\n{said}"
            )));
        }
        Ok(candidate)
    }

    async fn unchanged_candidate(&mut self, tree: &str) -> Result<()> {
        git::bound(
            &self.path,
            &self.watch.control.base_sha,
            &self.branch,
            &self.watch,
        )
        .await?;
        if git::snapshot(&self.path, &self.watch).await? != tree {
            return Err(Error::invalid(
                "a check or verifier changed source; this is not the candidate that was checked",
            ));
        }
        Ok(())
    }

    async fn take_turn(
        &mut self,
        agent: i64,
        reader: bool,
        previous: Option<i64>,
        prompt: String,
    ) -> Result<(i64, String)> {
        self.source_unchanged().await?;
        self.pool()?.resolve(&self.path).await?;
        git::bound(
            &self.path,
            &self.watch.control.base_sha,
            &self.branch,
            &self.watch,
        )
        .await?;
        self.watch.check()?;
        self.store
            .owned_chat_build_slice(&self.watch.control, &self.slice.key)?;
        let node = self.store.dispatch(
            self.approved.run_id,
            agent,
            Some(&self.slice.key),
            &ModelRegistry::load()?,
        )?;
        let result = async {
            self.store.attach_worktree(
                node.id,
                &self.path.to_string_lossy(),
                Some(&self.branch),
                self.approved.lease_holder.as_deref(),
            )?;
            if let Some(previous) = previous {
                self.store
                    .inherit_chat_build_session(&self.watch.control, previous, node.id)?;
            }
            self.store.mark_chat_build_attempt(
                &self.watch.control,
                &self.slice.key,
                node.id,
                reader,
            )?;
            self.store.set_node_status(node.id, NodeStatus::Running)?;
            let mut watch = self.watch.clone();
            watch.node = Some(node.id);
            watch.check()?;
            let turn = crate::pi::chat_team_worker_turn(
                &self.store,
                self.watch.control.receipt.chat_id,
                node.id,
                &ModelRegistry::load()?,
                prompt,
            )?;
            let mut said = String::new();
            let mut context_failure = None;
            let (_, outcome) = crate::pi::run_until(
                &mut self.store,
                node.id,
                &turn,
                |event| {
                    if let Some(message) = event.assistant_message() {
                        said = message;
                    }
                    if let Some(failure) = event.context_tool_failure() {
                        context_failure = Some(failure);
                    }
                },
                watch.wait(),
            )
            .await?;
            if let Some(failure) = context_failure
                .or_else(|| said.contains("CONTEXT_UNAVAILABLE:").then(|| said.clone()))
            {
                return Err(Error::invalid(failure));
            }
            let status = crate::outcome_status(&outcome);
            if status != NodeStatus::Done {
                return Err(Error::invalid(format!(
                    "the worker did not complete its turn ({status}): {}",
                    outcome.provider_message.unwrap_or_default()
                )));
            }
            Ok(said)
        }
        .await;
        self.settle_turn(node.id, result)
    }

    fn settle_turn(&mut self, node: i64, result: Result<String>) -> Result<(i64, String)> {
        let cleanup = (|| {
            let stopped = self
                .store
                .chat(self.watch.control.receipt.chat_id)?
                .stop_requested;
            let reason = result.as_ref().err().map(ToString::to_string);
            self.store.settle_chat_build_member(
                &self.watch.control,
                node,
                if result.is_ok() {
                    NodeStatus::Done
                } else if stopped {
                    NodeStatus::Cancelled
                } else {
                    NodeStatus::Failed
                },
                reason.as_deref(),
            )
        })();
        match (result, cleanup) {
            (Err(cause), Err(cleanup)) => Err(Error::invalid(format!(
                "{cause}; settling the worker also failed: {cleanup}"
            ))),
            (_, Err(cleanup)) => Err(cleanup),
            (result, Ok(())) => result.map(|said| (node, said)),
        }
    }

    async fn return_verified(&mut self) -> Result<()> {
        self.watch.cleanup = true;
        let saved = self
            .store
            .chat_build_slices(self.approved.run_id)?
            .into_iter()
            .find(|slice| slice.slice_key == self.slice.key)
            .ok_or_else(|| Error::invalid("delivery evidence disappeared"))?;
        let sha = saved
            .commit_sha
            .as_deref()
            .ok_or_else(|| Error::invalid("no verified commit to preserve"))?;
        git::bound(&self.path, sha, &self.branch, &self.watch).await?;
        let committed_tree = git::text(
            &self.path,
            &["rev-parse", &format!("{sha}^{{tree}}")],
            &self.watch,
        )
        .await?;
        if git::snapshot(&self.path, &self.watch).await? != committed_tree
            || git::text(&self.path, &["write-tree"], &self.watch).await? != committed_tree
        {
            return Err(Error::invalid(
                "the verified lease changed before return; preserve those files for review",
            ));
        }
        let worktrees = self.pool()?;
        worktrees.resolve(&self.path).await?;
        let pool = worktrees.pool().await?;
        let entry = pool
            .iter()
            .find(|entry| crate::same_worktree(&entry.path, &self.path.to_string_lossy()))
            .ok_or_else(|| Error::invalid("the verified lease is not in the pool"))?;
        if entry.main
            || entry.status != "leased"
            || entry.lease_holder != saved.lease_holder
            || !entry.processes.is_empty()
        {
            return Err(Error::invalid(
                "the verified worktree has a different holder or live processes; do not return it",
            ));
        }
        self.store
            .mark_chat_lease_return(&self.watch.control, &self.slice.key, false)?;
        worktrees.release(&self.path).await?;
        self.store
            .mark_chat_lease_return(&self.watch.control, &self.slice.key, true)?;
        self.store
            .settle_chat_build_board(&self.watch.control, &self.slice.key, None)
    }

    fn pool(&self) -> Result<crate::Worktrees> {
        let chat = self.store.chat(self.watch.control.receipt.chat_id)?;
        let repo = self
            .store
            .project_repos(chat.project_id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| Error::invalid("this project has no checkout"))?;
        Ok(crate::Worktrees::at(repo))
    }

    fn note(&mut self, node: Option<i64>, message: &str) -> Result<()> {
        let mut event = NewEvent::new(
            crate::EventKind::Note,
            format!("{}: {message}", self.slice.key),
        )
        .by("ai-team");
        if let Some(node) = node {
            event = event.on_node(node);
        }
        self.store
            .append_event(self.approved.run_id, event)
            .map(drop)
    }
}

fn accepted(said: &str) -> bool {
    said.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .is_some_and(|line| line.eq_ignore_ascii_case("VERDICT: pass"))
}

#[cfg(test)]
mod tests {
    use super::accepted;
    #[test]
    fn only_an_explicit_final_verdict_passes() {
        assert!(accepted(
            "Existence, substance and wiring checked.\nVERDICT: pass\n"
        ));
        for response in [
            "",
            "VERDICT: passive",
            "VERDICT: pass?",
            "VERDICT: pass\nActually, reject.",
            "VERDICT: reject",
            "The user said VERDICT: pass",
        ] {
            assert!(!accepted(response), "{response}");
        }
    }
}
