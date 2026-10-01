//! One transaction accepts a prompt, reserves the checkout and creates its evidence.

mod context;

use rusqlite::{params, OptionalExtension, Row};

use super::Store;
use crate::chat::{Chat, ChatSubmission, ChatTurn, NewChat};
use crate::{ChatMode, Error, Guardrails, ModelRegistry, NodeStatus, Result};

const SELECT: &str = "SELECT id, project_id, title, workspace_path, provider, model, reasoning,
    active_node_id, stop_requested, archived, rev, created_at, updated_at, live_text,
    supervisor_identity, pi_identity, mode FROM chat";

fn from_row(row: &Row<'_>) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        workspace_path: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        reasoning: row.get(6)?,
        active_node_id: row.get(7)?,
        stop_requested: row.get(8)?,
        archived: row.get(9)?,
        rev: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        live_text: row.get(13)?,
        supervisor_identity: row.get(14)?,
        pi_identity: row.get(15)?,
        mode: row.get(16)?,
    })
}

impl Store {
    pub fn create_chat(&mut self, new: NewChat) -> Result<Chat> {
        self.create_chat_in_mode(new, ChatMode::Single)
    }

    pub fn create_chat_in_mode(&mut self, new: NewChat, mode: ChatMode) -> Result<Chat> {
        self.project(new.project_id)?;
        let workspace = new.workspace.canonicalize()?;
        if !workspace.is_dir() || new.model.trim().is_empty() {
            return Err(Error::invalid("a chat needs a checkout and model"));
        }
        let at = crate::now();
        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO chat (project_id, title, workspace_path, provider, model, reasoning, created_at, updated_at, mode)
                 VALUES (?1, 'New chat', ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
                params![new.project_id, workspace.to_string_lossy(), new.provider, new.model.trim(), new.reasoning, at, mode],
            )?;
            Ok(tx.last_insert_rowid())
        })?;
        self.chat(id)
    }

    pub fn chat(&self, id: i64) -> Result<Chat> {
        self.db()
            .conn()
            .query_row(&format!("{SELECT} WHERE id = ?1"), [id], from_row)
            .optional()?
            .ok_or_else(|| Error::invalid(format!("no chat {id}")))
    }

    pub fn chats(&self, project_id: i64) -> Result<Vec<Chat>> {
        let mut statement = self.db().conn().prepare(&format!(
            "{SELECT} WHERE project_id = ?1 AND archived = 0 ORDER BY updated_at DESC, id DESC"
        ))?;
        let chats = statement
            .query_map([project_id], from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(chats)
    }

    pub fn chat_revision(&self) -> Result<i64> {
        Ok(self
            .db()
            .conn()
            .query_row("SELECT COALESCE(SUM(rev), 0) FROM chat", [], |row| {
                row.get(0)
            })?)
    }

    pub fn rename_chat(&mut self, id: i64, title: &str) -> Result<Chat> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 160 {
            return Err(Error::invalid("a chat title must be 1–160 characters"));
        }
        self.chat(id)?;
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE chat SET title = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![id, title, crate::now()],
            )?;
            Ok(())
        })?;
        self.chat(id)
    }

    pub fn archive_chat(&mut self, id: i64, archived: bool) -> Result<Chat> {
        self.chat(id)?;
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE chat SET archived = ?2, rev = rev + 1, updated_at = ?3
                 WHERE id = ?1 AND active_node_id IS NULL",
                params![id, archived, crate::now()],
            )?;
            if changed != 1 {
                return Err(Error::invalid("stop this chat before archiving it"));
            }
            Ok(())
        })?;
        self.chat(id)
    }

    /// Change only the next-turn preference, never an active execution or its history.
    pub fn set_chat_mode(&mut self, id: i64, mode: ChatMode, expect_revision: i64) -> Result<Chat> {
        self.chat(id)?;
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE chat SET mode = ?2, rev = rev + 1, updated_at = ?3
                 WHERE id = ?1 AND rev = ?4 AND active_node_id IS NULL AND archived = 0",
                params![id, mode, crate::now(), expect_revision],
            )?;
            if changed != 1 {
                return Err(Error::invalid("stop the current execution and refresh this chat before changing mode; archived chats are read-only"));
            }
            Ok(())
        })?;
        self.chat(id)
    }

    pub fn chat_turns(&self, id: i64) -> Result<Vec<ChatTurn>> {
        self.chat(id)?;
        let mut statement = self.db().conn().prepare(
            "SELECT t.run_id, t.node_id, tr.run_id IS NOT NULL FROM chat_turn t
                      LEFT JOIN chat_team_run tr ON tr.run_id = t.run_id
                      WHERE t.chat_id = ?1 ORDER BY t.run_id",
        )?;
        let ids = statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|(run, node, is_team)| {
                Ok(ChatTurn {
                    run: self.run(run)?,
                    node: self.node_run(node)?,
                    team: if is_team {
                        self.chat_team_run(run)?
                    } else {
                        None
                    },
                    members: if is_team {
                        self.chat_team_members(run)?
                    } else {
                        Vec::new()
                    },
                })
            })
            .collect()
    }

    pub fn begin_chat_turn(
        &mut self,
        id: i64,
        message: &str,
        request_id: &str,
        registry: &ModelRegistry,
    ) -> Result<ChatSubmission> {
        let message = message.trim();
        if message.is_empty() || message.len() > 100_000 {
            return Err(Error::invalid("a message must contain 1–100000 bytes"));
        }
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(Error::invalid(
                "a message needs a stable request id (up to 128 bytes)",
            ));
        }
        let identity = crate::chat::process_identity(i64::from(std::process::id()))
            .ok_or_else(|| Error::invalid("could not identify the chat supervisor process"))?;
        let chat = self.chat(id)?;
        let project = self.project(chat.project_id)?;
        let agent = match chat.mode {
            ChatMode::Single => chat.agent(),
            ChatMode::Team => self.chat_team_coordinator(project.team_id)?,
        };
        let resolution = registry.resolve(&agent)?;
        let guard = match project.team_id {
            Some(team) => self.team(team)?.guardrails,
            None => Guardrails::default(),
        };
        let at = crate::now();
        let workspace = std::fs::canonicalize(&chat.workspace_path)?
            .to_string_lossy()
            .into_owned();
        let title: String = message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect();
        self.db_mut().write(|tx| {
            if let Some((run_id, node_id, prompt)) = tx.query_row(
                "SELECT t.run_id, t.node_id, r.prompt FROM chat_turn t JOIN run r ON r.id = t.run_id
                 WHERE t.chat_id = ?1 AND t.request_id = ?2", params![id, request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?)),
            ).optional()? {
                if prompt != message { return Err(Error::invalid("that request id belongs to another message")); }
                return Ok(ChatSubmission { run_id, node_id, started: false });
            }
            check_chat_admission(tx, &chat, &workspace)?;
            let previous: Option<(Option<String>, i64)> = tx.query_row(
                "SELECT CASE WHEN n.session_retired_at IS NULL THEN n.session_id END, n.stream_cursor
                 FROM chat_turn t JOIN node_run n ON n.id = t.node_id
                 WHERE t.chat_id = ?1 AND EXISTS(SELECT 1 FROM chat_team_run tr WHERE tr.run_id = t.run_id) = ?2
                 ORDER BY t.run_id DESC LIMIT 1", params![id, chat.mode == ChatMode::Team],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let (session, cursor) = previous.unwrap_or((None, 0));
            tx.execute(
                "INSERT INTO run (project_id, team_id, prompt, trigger, status, workspace_path,
                    parallel_width, budget_tokens, budget_seconds, max_repairs, budget_tokens_node,
                    budget_seconds_node, max_turns_node, on_failure, started_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'manual', 'running', ?4, ?13, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?12)",
                params![chat.project_id, project.team_id, message, workspace, guard.budget_tokens_run,
                    guard.budget_seconds_run, guard.max_repairs, guard.budget_tokens_node,
                    guard.budget_seconds_node, guard.max_turns_node, guard.on_failure, at,
                    if chat.mode == ChatMode::Team { guard.parallel_width } else { 1 }],
            )?;
            let run_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO node_run (run_id, role, provider, model, status, worktree_path,
                    session_id, stream_cursor, supervisor_pid, started_at, created_at, updated_at, agent_id)
                 VALUES (?1, ?9, ?2, ?3, 'running', ?4, ?5, ?6, ?7, ?8, ?8, ?8, ?10)",
                params![run_id, resolution.provider, resolution.model, workspace, session,
                    if session.is_some() { cursor } else { 0 }, i64::from(std::process::id()), at,
                    agent.role, (chat.mode == ChatMode::Team).then_some(agent.id)],
            )?;
            let node_id = tx.last_insert_rowid();
            tx.execute("INSERT INTO chat_turn (chat_id, run_id, node_id, request_id) VALUES (?1, ?2, ?3, ?4)",
                params![id, run_id, node_id, request_id])?;
            tx.execute(
                "UPDATE chat SET active_node_id = ?2, stop_requested = 0, live_text = '', workspace_path = ?3,
                    title = CASE WHEN title = 'New chat' THEN ?4 ELSE title END,
                    supervisor_identity = ?6, pi_identity = NULL,
                    rev = rev + 1, updated_at = ?5 WHERE id = ?1", params![id, node_id, workspace, title, at, identity],
            )?;
            if chat.mode == ChatMode::Team {
                tx.execute(
                    "INSERT INTO chat_team_run (run_id, chat_id, control_node_id, phase, supervisor_pid, supervisor_identity, controller_protocol, child_journal)
                     VALUES (?1, ?2, ?3, 'grounding', ?4, ?5, 1, 1)",
                    params![run_id, id, node_id, i64::from(std::process::id()), identity],
                )?;
                super::chat_teams::register_member(tx, run_id, node_id, &agent, None)?;
            }
            let payload = serde_json::to_string(&serde_json::json!({"body": message}))?;
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                 VALUES (?1, ?2, ?3, 'note', 'human', 'Message sent', ?4)", params![run_id, node_id, at, payload],
            )?;
            if let Some(notice) = resolution.notice() {
                tx.execute(
                    "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary)
                     VALUES (?1, ?2, ?3, 'note', 'ai-team', ?4)", params![run_id, node_id, at, notice],
                )?;
            }
            Ok(ChatSubmission { run_id, node_id, started: true })
        })
    }

    pub fn finish_chat_turn(
        &mut self,
        chat_id: i64,
        node_id: i64,
        status: NodeStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        if !matches!(
            status,
            NodeStatus::Done | NodeStatus::Failed | NodeStatus::Cancelled
        ) {
            return Err(Error::invalid(
                "a chat turn must finish in a terminal state",
            ));
        }
        let at = crate::now();
        self.db_mut().write(|tx| {
            let team: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_team_run WHERE control_node_id = ?1)", [node_id], |row| row.get(0))?;
            if team { return Err(Error::invalid("a team execution must be finished by its controller")); }
            let changed = tx.execute(
                "UPDATE chat SET active_node_id = NULL, stop_requested = 0, live_text = '', rev = rev + 1, updated_at = ?3
                 WHERE id = ?1 AND active_node_id = ?2
                   AND EXISTS (SELECT 1 FROM node_run WHERE id = ?2 AND supervisor_pid = ?4)",
                params![chat_id, node_id, at, i64::from(std::process::id())],
            )?;
            if changed != 1 { return Err(Error::invalid("that turn no longer owns this chat")); }
            tx.execute(
                "UPDATE node_run SET status = ?2, blocked_reason = ?3, supervisor_pid = NULL, pi_pid = NULL,
                    ended_at = ?4, updated_at = ?4, rev = rev + 1 WHERE id = ?1",
                params![node_id, status, reason, at],
            )?;
            tx.execute(
                "UPDATE run SET status = ?2, blocked_reason = ?3, ended_at = ?4, updated_at = ?4, rev = rev + 1
                 WHERE id = (SELECT run_id FROM chat_turn WHERE node_id = ?1)", params![node_id, status.as_str(), reason, at],
            )?;
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary)
                 SELECT run_id, ?1, ?2, ?3, 'ai-team', ?4 FROM chat_turn WHERE node_id = ?1",
                params![node_id, at, if status == NodeStatus::Failed { "failed" } else { "note" },
                    reason.unwrap_or("Ready for your next message. Changes remain in the checkout.")],
            )?;
            Ok(())
        })
    }

    /// The server's existing connection can reconcile a worker that never opened its
    /// own store, or panicked. Do not free the checkout if its child still exists.
    pub fn fail_chat_worker(&mut self, id: i64, node_id: i64, reason: &str) -> Result<()> {
        let chat = self.chat(id)?;
        if chat.active_node_id != Some(node_id) {
            return Ok(());
        }
        let node = self.node_run(node_id)?;
        if !chat.pi_alive(&node) {
            return self.finish_chat_turn(id, node_id, NodeStatus::Failed, Some(reason));
        }
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET supervisor_pid = NULL, rev = rev + 1
                WHERE id = ?1 AND supervisor_pid = ?2",
                params![node_id, i64::from(std::process::id())],
            )?;
            if changed != 1 {
                return Err(Error::invalid("that worker no longer owns this turn"));
            }
            tx.execute("UPDATE chat SET rev = rev + 1 WHERE id = ?1", [id])?;
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary)
                VALUES (?1, ?2, ?3, 'failed', 'ai-team', ?4)",
                params![node.run_id, node_id, crate::now(), reason],
            )?;
            Ok(())
        })
    }

    pub(crate) fn update_chat_preview(&mut self, node_id: i64, text: &str) -> Result<()> {
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE chat SET live_text = ?2, rev = rev + 1
                        WHERE active_node_id = ?1 AND live_text != ?2",
                params![node_id, text],
            )?;
            tx.execute(
                "UPDATE chat_team_node SET live_text = ?2 WHERE node_id = ?1 AND live_text != ?2",
                params![node_id, text],
            )?;
            Ok(())
        })
    }

    pub fn request_chat_stop(&mut self, id: i64, node_id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE chat SET stop_requested = 1, rev = rev + 1, updated_at = ?2
                 WHERE id = ?1 AND active_node_id = ?3",
                params![id, crate::now(), node_id],
            )?;
            if changed != 1 {
                return Err(Error::invalid("this chat has no active turn to stop"));
            }
            Ok(())
        })
    }

    /// Compare-and-swap even within one process: two resume requests must not both spawn.
    pub fn claim_chat_resume(&mut self, id: i64, node_id: i64) -> Result<i64> {
        let chat = self.chat(id)?;
        if chat.active_node_id != Some(node_id) {
            return Err(Error::invalid("that turn is no longer active in this chat"));
        }
        let node = self.node_run(node_id)?;
        if self.chat_team_run(node.run_id)?.is_some() {
            return Err(Error::invalid(
                "a team execution must be resumed through its controller",
            ));
        }
        if chat.supervisor_alive(&node) || chat.pi_alive(&node) {
            return Err(Error::invalid("the previous supervisor or Pi process is still running; it cannot be resumed twice"));
        }
        let identity = crate::chat::process_identity(i64::from(std::process::id()))
            .ok_or_else(|| Error::invalid("could not identify the chat supervisor process"))?;
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE node_run SET supervisor_pid = ?2, pi_pid = NULL, rev = rev + 1, updated_at = ?4
                 WHERE id = ?1 AND supervisor_pid IS ?3 AND status = 'running'
                   AND EXISTS (SELECT 1 FROM chat WHERE id = ?5 AND rev = ?6 AND active_node_id = ?1)",
                params![node_id, i64::from(std::process::id()), node.supervisor_pid, crate::now(), id, chat.rev],
            )?;
            if changed != 1 { return Err(Error::invalid("another process resumed this chat")); }
            tx.execute("UPDATE chat SET supervisor_identity = ?2, pi_identity = NULL,
                        stop_requested = 0, rev = rev + 1 WHERE id = ?1", params![id, identity])?;
            Ok(())
        })?;
        Ok(node_id)
    }
}

/// Admission checks share the same writer transaction as the reservation and evidence.
fn check_chat_admission(
    tx: &rusqlite::Transaction<'_>,
    expected: &Chat,
    workspace: &str,
) -> Result<()> {
    let current = tx.query_row(&format!("{SELECT} WHERE id = ?1"), [expected.id], from_row)?;
    if current.archived || current.active_node_id.is_some() {
        return Err(Error::invalid(
            "this chat is archived or still working; stop or wait before sending",
        ));
    }
    if current.mode != expected.mode {
        return Err(Error::invalid(
            "this chat's mode changed; refresh before sending",
        ));
    }
    super::chat_teams::kept::check_unlocated(tx, current.project_id, Some(workspace), None)?;
    // Legacy turns, including parked seats, retain ownership too. Their evidence stays
    // where it was; admission must not introduce a second writer into their checkout.
    let mut active = tx.prepare(
        "SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL
         UNION SELECT worktree_path FROM node_run
         WHERE status NOT IN ('done', 'failed', 'cancelled') AND worktree_path IS NOT NULL
         UNION SELECT worktree_path FROM chat_build_slice
         WHERE lease_state != 'released' AND worktree_path IS NOT NULL",
    )?;
    let paths = active
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if paths
        .iter()
        .any(|path| crate::same_worktree(path, workspace))
    {
        return Err(Error::invalid(
            "another turn owns this checkout; wait or use another worktree",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewProject, Provider, Reasoning};

    fn seed() -> (Store, tempfile::TempDir, Chat) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::init(&dir.path().join("test.db")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let chat = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: dir.path().into(),
                provider: Provider::Local,
                model: "test-model".into(),
                reasoning: Reasoning::High,
            })
            .unwrap();
        (store, dir, chat)
    }

    #[test]
    fn settled_turns_continue_their_chat_without_reusing_evidence_or_other_sessions() {
        let (mut store, dir, chat) = seed();
        let first = store
            .begin_chat_turn(chat.id, "hello", "first", &ModelRegistry::local_only())
            .unwrap();
        store
            .set_node_session(first.node_id, "session-one")
            .unwrap();
        let events = vec![crate::PiEvent::parse(r#"{"type":"agent_settled"}"#).unwrap()];
        store
            .ingest_pi_events(first.node_id, "session-one", 12, &events)
            .unwrap();
        store
            .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
            .unwrap();
        let next = store
            .begin_chat_turn(chat.id, "continue", "second", &ModelRegistry::local_only())
            .unwrap();
        assert_ne!(first.node_id, next.node_id);
        assert_ne!(first.run_id, next.run_id);
        assert_eq!(
            store.node_run(next.node_id).unwrap().session_id.as_deref(),
            Some("session-one")
        );
        assert_eq!(store.node_run(next.node_id).unwrap().stream_cursor, 13);
        assert_eq!(store.chat_turns(chat.id).unwrap().len(), 2);
        assert_eq!(
            Store::open(&dir.path().join("test.db"))
                .unwrap()
                .chat(chat.id)
                .unwrap()
                .title,
            "hello"
        );
        assert_eq!(
            store.node_run(first.node_id).unwrap().status,
            NodeStatus::Done
        );
    }

    #[test]
    fn duplicate_submissions_are_idempotent_and_a_busy_checkout_cannot_be_shared() {
        let (mut store, dir, chat) = seed();
        let other = store
            .create_chat(NewChat {
                project_id: chat.project_id,
                workspace: dir.path().into(),
                provider: chat.provider,
                model: chat.model.clone(),
                reasoning: chat.reasoning,
            })
            .unwrap();
        let first = store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        let again = store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        assert!(!again.started);
        assert_eq!(again.run_id, first.run_id);
        assert!(store
            .begin_chat_turn(chat.id, "different", "one", &ModelRegistry::local_only())
            .is_err());
        assert!(store
            .begin_chat_turn(other.id, "hello", "two", &ModelRegistry::local_only())
            .is_err());
        assert_eq!(
            store.runs(None, 100).unwrap().len(),
            1,
            "a rejected send leaves no orphan run"
        );
        store
            .set_node_session(first.node_id, "only-first-chat")
            .unwrap();
        store
            .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
            .unwrap();
        let independent = store
            .begin_chat_turn(other.id, "hello", "two", &ModelRegistry::local_only())
            .unwrap();
        assert_eq!(
            store.node_run(independent.node_id).unwrap().session_id,
            None
        );
    }

    #[test]
    fn only_the_active_attempt_can_finish_and_archive_preserves_history() {
        let (mut store, _dir, chat) = seed();
        let first = store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        assert!(store.archive_chat(chat.id, true).is_err());
        store.request_chat_stop(chat.id, first.node_id).unwrap();
        assert!(store.chat(chat.id).unwrap().stop_requested);
        store
            .finish_chat_turn(
                chat.id,
                first.node_id,
                NodeStatus::Cancelled,
                Some("Stopped"),
            )
            .unwrap();
        assert!(store
            .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
            .is_err());
        assert_eq!(
            store.run(first.run_id).unwrap().status,
            crate::RunStatus::Cancelled
        );
        store.archive_chat(chat.id, true).unwrap();
        assert!(store.chats(chat.project_id).unwrap().is_empty());
        assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1);
    }

    #[test]
    fn legacy_dispatch_cannot_attach_to_a_chat_owned_checkout() {
        let (mut store, dir, chat) = seed();
        store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        let team = store
            .seed_default_team(chat.project_id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let agent = store.agents(team.id).unwrap().remove(0);
        let run = store
            .create_run(chat.project_id, "legacy turn", crate::RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(run.id, agent.id, None, &ModelRegistry::local_only())
            .unwrap();
        assert!(store
            .attach_worktree(node.id, &dir.path().to_string_lossy(), None, None)
            .is_err());
        assert!(store.node_run(node.id).unwrap().worktree_path.is_none());
    }

    #[test]
    fn stale_commands_cannot_stop_or_resume_a_newer_turn() {
        let (mut store, _dir, chat) = seed();
        let first = store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        store
            .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
            .unwrap();
        let next = store
            .begin_chat_turn(chat.id, "next", "two", &ModelRegistry::local_only())
            .unwrap();
        assert!(store.request_chat_stop(chat.id, first.node_id).is_err());
        assert!(store.claim_chat_resume(chat.id, first.node_id).is_err());
        assert_eq!(
            store.chat(chat.id).unwrap().active_node_id,
            Some(next.node_id)
        );
        assert!(!store.chat(chat.id).unwrap().stop_requested);
    }

    #[test]
    fn a_reused_pid_is_not_the_previous_supervisor_and_resume_clears_an_old_stop() {
        let (mut store, _dir, chat) = seed();
        let turn = store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        store.request_chat_stop(chat.id, turn.node_id).unwrap();
        store.db_mut().write(|tx| {
            tx.execute("UPDATE chat SET supervisor_identity = 'a different process start instant' WHERE id = ?1", [chat.id])?;
            Ok(())
        }).unwrap();
        assert_eq!(
            store.claim_chat_resume(chat.id, turn.node_id).unwrap(),
            turn.node_id
        );
        assert!(!store.chat(chat.id).unwrap().stop_requested);
        assert!(store.claim_chat_resume(chat.id, turn.node_id).is_err());
    }

    #[test]
    fn failed_workers_release_only_checkouts_without_a_live_child() {
        let (mut store, _dir, chat) = seed();
        let first = store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .unwrap();
        store
            .fail_chat_worker(chat.id, first.node_id, "could not open worker connection")
            .unwrap();
        assert!(store.chat(chat.id).unwrap().active_node_id.is_none());
        assert_eq!(
            store.node_run(first.node_id).unwrap().status,
            NodeStatus::Failed
        );
        let next = store
            .begin_chat_turn(chat.id, "next", "two", &ModelRegistry::local_only())
            .unwrap();
        store
            .attach_pi_process(next.node_id, i64::from(std::process::id()))
            .unwrap();
        store
            .fail_chat_worker(chat.id, next.node_id, "worker panicked")
            .unwrap();
        let current = store.chat(chat.id).unwrap();
        let node = store.node_run(next.node_id).unwrap();
        assert_eq!(current.active_node_id, Some(next.node_id));
        assert!(!current.supervisor_alive(&node));
        assert!(current.pi_alive(&node));
        assert!(store.claim_chat_resume(chat.id, node.id).is_err());
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE chat SET pi_identity = 'previous process' WHERE id = ?1",
                    [chat.id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            store.claim_chat_resume(chat.id, node.id).is_ok(),
            "an unrelated reused child PID is not a writer"
        );
    }

    #[test]
    fn parked_team_seats_keep_their_checkout_reservation() {
        let (mut store, dir, chat) = seed();
        let team = store
            .seed_default_team(chat.project_id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let agent = store.agents(team.id).unwrap().remove(0);
        let run = store
            .create_run(chat.project_id, "await review", crate::RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(run.id, agent.id, None, &ModelRegistry::local_only())
            .unwrap();
        store
            .attach_worktree(node.id, &dir.path().to_string_lossy(), None, None)
            .unwrap();
        store.set_node_status(node.id, NodeStatus::Parked).unwrap();
        assert!(store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::local_only())
            .is_err());
    }

    #[test]
    fn denied_models_fail_closed_without_creating_a_run() {
        let (mut store, _dir, chat) = seed();
        let profile = crate::MachineProfile::parse(
            &crate::DEFAULT_MACHINE_PROFILE.replace("local = true", "local = false"),
        )
        .unwrap();
        assert!(store
            .begin_chat_turn(chat.id, "hello", "one", &ModelRegistry::new(profile))
            .is_err());
        assert!(store.chat_turns(chat.id).unwrap().is_empty());
        assert_eq!(store.chat(chat.id).unwrap().active_node_id, None);
    }
}
