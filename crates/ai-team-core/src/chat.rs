//! Durable solo conversations. A chat owns its checkout and addresses, not a team's latest run.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    Agent, Error, ModelRegistry, NodeRun, NodeStatus, Provider, Reasoning, Result, Run, Store,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub workspace_path: String,
    pub provider: Provider,
    pub model: String,
    pub reasoning: Reasoning,
    pub active_node_id: Option<i64>,
    pub live_text: String,
    #[serde(skip)]
    pub(crate) supervisor_identity: Option<String>,
    #[serde(skip)]
    pub(crate) pi_identity: Option<String>,
    pub stop_requested: bool,
    pub archived: bool,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewChat {
    pub project_id: i64,
    pub workspace: PathBuf,
    pub provider: Provider,
    pub model: String,
    pub reasoning: Reasoning,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTurn {
    pub run: Run,
    pub node: NodeRun,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatSubmission {
    pub run_id: i64,
    pub node_id: i64,
    /// False for an idempotent replay. Only the first receipt may spawn a worker.
    pub started: bool,
}

/// A PID alone is not identity after a restart or reboot. Keep the OS-reported start
/// instant too; if the OS cannot answer, fail closed rather than guessing a writer died.
pub(crate) fn process_identity(pid: i64) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let identity = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !identity.is_empty()).then_some(identity)
}

fn process_matches(pid: Option<i64>, identity: Option<&str>) -> bool {
    pid.is_some_and(|pid| {
        crate::process_is_alive(pid)
            && identity.is_none_or(|expected| {
                process_identity(pid).is_none_or(|actual| actual == expected)
            })
    })
}

impl Chat {
    pub fn supervisor_alive(&self, node: &NodeRun) -> bool {
        process_matches(node.supervisor_pid, self.supervisor_identity.as_deref())
    }

    pub fn pi_alive(&self, node: &NodeRun) -> bool {
        process_matches(node.pi_pid, self.pi_identity.as_deref())
    }

    /// Resolve through the same policy as team seats without inserting a fake team member.
    pub(crate) fn agent(&self) -> Agent {
        Agent {
            id: 0,
            team_id: 0,
            ord: 0,
            role: "assistant".into(),
            name: "Assistant".into(),
            purpose: "Help the person in this conversation".into(),
            provider: self.provider,
            model: self.model.clone(),
            reasoning: self.reasoning,
            zone: "**".into(),
            prompt_preset: None,
            prompt_md: None,
            context_window: None,
            read_only: false,
            enabled: true,
            rev: 1,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

/// Resume a durable, already-claimed turn. No leasing, committing or returning a checkout.
pub async fn drive_chat(db: &Path, chat_id: i64, node_id: i64, recovering: bool) -> Result<()> {
    let mut store = Store::open(db)?;
    let result = drive(&mut store, chat_id, node_id, recovering).await;
    if let Err(error) = &result {
        if let Err(cleanup) = store.fail_chat_worker(chat_id, node_id, &error.to_string()) {
            return Err(Error::invalid(format!(
                "{error}; recording the failure also failed: {cleanup}"
            )));
        }
    }
    result
}

async fn drive(store: &mut Store, chat_id: i64, node_id: i64, recovering: bool) -> Result<()> {
    let chat = store.chat(chat_id)?;
    if chat.active_node_id != Some(node_id) {
        return Err(Error::invalid("that turn no longer owns this chat"));
    }
    let node = store.node_run(node_id)?;
    if node.supervisor_pid != Some(i64::from(std::process::id())) {
        return Err(Error::invalid("this process does not supervise that chat"));
    }
    if chat.stop_requested {
        return store.finish_chat_turn(
            chat_id,
            node_id,
            NodeStatus::Cancelled,
            Some("Stopped before the turn started."),
        );
    }
    let registry = ModelRegistry::load()?;
    // Recheck policy at the process boundary, including on interrupted-turn recovery.
    let mut requested = chat.agent();
    requested.provider = node.provider;
    requested.model.clone_from(&node.model);
    let resolved = registry.resolve(&requested)?;
    if resolved.provider != node.provider || resolved.model != node.model {
        return Err(Error::invalid(
            "model policy changed before this turn started; send a new message to resolve it again",
        ));
    }
    let project = store.project(chat.project_id)?;
    let repo = store
        .project_repos(project.id)?
        .into_iter()
        .find_map(|repo| repo.main_path)
        .ok_or_else(|| Error::invalid("this chat's project has no checkout"))?;
    let workspace = crate::Worktrees::at(repo)
        .resolve(Path::new(&chat.workspace_path))
        .await?;
    if !crate::same_worktree(&workspace.to_string_lossy(), &chat.workspace_path) {
        return Err(Error::invalid("this chat's checkout changed"));
    }
    let support = store
        .path()
        .parent()
        .ok_or_else(|| Error::invalid("the chat store needs a data directory"))?
        .join("seats")
        .join(&project.slug)
        .join(format!("chat-{chat_id}"));
    let run = store.run(node.run_id)?;
    let prompt = if recovering && node.session_id.is_some() {
        // Only recovery reuses this attempt; a follow-up has its own node.
        format!("The previous turn was interrupted. Inspect existing work before continuing; do not repeat completed changes or discard files. The last request may not have reached you yet. Finish it if needed, then report the result.\n\nLast user request:\n{}", run.prompt)
    } else {
        run.prompt
    };
    let turn = crate::pi::conversation_turn(
        &chat,
        &node,
        &support,
        &registry.context_sources(),
        prompt,
        store.path(),
    )?;
    let observer = Store::open(store.path())?;
    let stop = async move {
        loop {
            let reason = (|| -> Result<Option<String>> {
                let current = observer.chat(chat_id)?;
                if current.active_node_id != Some(node_id) {
                    return Ok(Some("This turn no longer owns the chat.".into()));
                }
                if current.stop_requested {
                    return Ok(Some(
                        "Stopped by you. Your conversation and working files are kept.".into(),
                    ));
                }
                Ok(crate::node_may_continue(&observer, node_id)?.map(|limit| limit.reason))
            })();
            match reason {
                Ok(Some(reason)) => return reason,
                Err(error) => return format!("Stopped because supervision failed: {error}"),
                Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            }
        }
    };
    if let Some(limit) = crate::node_may_continue(store, node_id)? {
        return Err(Error::invalid(limit.reason));
    }
    let (_, outcome) = crate::pi::run_until(store, node_id, &turn, |_| {}, stop).await?;
    let status = crate::outcome_status(&outcome);
    let reason = (status != NodeStatus::Done).then(|| {
        outcome
            .provider_message
            .unwrap_or_else(|| "The Pi process ended without completing the turn.".into())
    });
    store.finish_chat_turn(chat_id, node_id, status, reason.as_deref())?;
    Ok(())
}
