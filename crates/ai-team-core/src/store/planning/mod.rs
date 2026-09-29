//! Serialize scoped mutations through the team store; the pinned engine owns plan rows.

pub(super) mod build;
mod claims;
mod engine;
mod results;
mod writes;

use std::path::{Path, PathBuf};

use ai_planner_core as planner;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::Store;
use crate::planning::{ChatPlan, PlanAccess, PlanAction, PlanActor};
use crate::{Chat, Error, Result};

impl Store {
    /// Validate the explicit MCP --db before the normal migration runner can write to it.
    pub fn open_planning_host(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('chat', 'chat_turn', 'node_run', 'project')", [], |row| row.get(0))?;
        if tables != 4 {
            return Err(Error::invalid(
                "planning needs an existing ai-team chat database",
            ));
        }
        drop(conn);
        Self::open(path)
    }

    pub fn planning_path(&self) -> Result<PathBuf> {
        engine::path(self.path())
    }

    pub fn planning_revision(&self) -> Result<i64> {
        // In-memory unit stores have no sidecar, nor should they discover one in HOME.
        if self.path() == Path::new(":memory:") {
            return Ok(0);
        }
        engine::total_revision(&self.planning_path()?)
    }

    pub fn planning_access(&self, chat_id: i64, actor: PlanActor) -> Result<PlanAccess> {
        self.chat(chat_id)?;
        Ok(scope(self.db().conn(), chat_id, actor, false)?.0)
    }

    pub fn chat_plan(&self, chat_id: i64, actor: PlanActor) -> Result<ChatPlan> {
        let chat = self.chat(chat_id)?;
        scope(self.db().conn(), chat_id, actor, false)?;
        let Some(store) = engine::open(&self.planning_path()?, false)? else {
            return Ok(empty(&chat));
        };
        snapshot(&store, &chat)
    }

    pub fn change_chat_plan(
        &mut self,
        chat_id: i64,
        actor: PlanActor,
        action: PlanAction,
    ) -> Result<ChatPlan> {
        let chat = self.chat(chat_id)?;
        let project = self.project(chat.project_id)?;
        let path = self.planning_path()?;
        // All transports share this lock, including separate MCP processes. Validate
        // active ownership and the revision inside it, not before acquiring it.
        self.db_mut().write(|tx| {
            let (access, slice) = scope(tx, chat_id, actor, true)?;
            authorize(access, slice.as_deref(), &action)?;
            build::freeze(tx, chat_id, access, &action)?;
            let create = matches!(action, PlanAction::CreatePlan { .. });
            let mut store = engine::open(&path, create)?
                .ok_or_else(|| Error::invalid("this chat has no plan yet"))?;
            store.set_actor(match actor {
                PlanActor::Human => "human".into(),
                PlanActor::Agent(node) => format!("ai-team node {node}"),
            });
            let before = snapshot(&store, &chat)?;
            if before.revision != action.revision() {
                return Err(Error::invalid(
                    "this plan changed since you read it; refresh and retry",
                ));
            }
            if create {
                if before.bundle.is_some() {
                    return Err(Error::invalid("this chat already has a plan"));
                }
                create_plan(&mut store, &chat, &project.name, action)?;
            } else {
                let plan = before
                    .bundle
                    .ok_or_else(|| Error::invalid("this chat has no plan yet"))?
                    .plan;
                writes::apply(&mut store, &plan, action)?;
            }
            snapshot(&store, &chat)
        })
    }
}

struct Binding {
    role: String,
    slice: Option<String>,
    read_only: Option<bool>,
    member_access: Option<String>,
    worktree: Option<String>,
    lease: Option<String>,
}

fn scope(
    conn: &Connection,
    chat: i64,
    actor: PlanActor,
    writing: bool,
) -> Result<(PlanAccess, Option<String>)> {
    let archived: bool =
        conn.query_row("SELECT archived FROM chat WHERE id = ?1", [chat], |row| {
            row.get(0)
        })?;
    if writing && archived {
        return Err(Error::invalid("archived chats have read-only plans"));
    }
    let PlanActor::Agent(node) = actor else {
        return Ok((PlanAccess::Human, None));
    };
    let binding = conn.query_row(
        "SELECT n.role, n.slice_key, a.read_only, m.plan_access, n.worktree_path, s.worktree_path FROM chat c
         JOIN chat_turn t ON t.chat_id = c.id AND t.node_id = c.active_node_id
         JOIN run r ON r.id = t.run_id
         JOIN node_run n ON n.run_id = t.run_id LEFT JOIN agent a ON a.id = n.agent_id
         LEFT JOIN chat_team_run tr ON tr.run_id = t.run_id
         LEFT JOIN chat_team_node m ON m.node_id = n.id AND m.run_id = tr.run_id
         LEFT JOIN chat_build_slice s ON s.run_id = tr.run_id AND s.slice_key = n.slice_key AND s.lease_state = 'leased'
         WHERE c.id = ?1 AND n.id = ?2 AND n.status = 'running' AND c.archived = 0 AND c.stop_requested = 0
           AND r.status = 'running'
           AND ((tr.run_id IS NULL AND n.id = t.node_id)
             OR (tr.chat_id = c.id AND tr.control_node_id = t.node_id AND m.plan_access IS NOT NULL
                 AND tr.phase IN ('grounding', 'planning', 'building')
                 AND (m.plan_access != 'planner' OR tr.phase != 'building')
                 AND (m.plan_access NOT IN ('maker', 'reader') OR (tr.phase = 'building'
                     AND s.worktree_path IS NOT NULL AND n.worktree_path IS NOT NULL))))",
        [chat, node], |row| Ok(Binding { role: row.get(0)?, slice: row.get(1)?, read_only: row.get(2)?, member_access: row.get(3)?, worktree: row.get(4)?, lease: row.get(5)? }),
    ).optional()?.ok_or_else(|| Error::invalid("this agent no longer owns an active turn in this chat"))?;
    let access = if let Some(access) = &binding.member_access {
        PlanAccess::from_member(access)?
    } else {
        match binding.role.as_str() {
            "assistant" | "orchestrator" | "planner" => PlanAccess::Planner,
            "verifier" | "reviewer" => PlanAccess::Reader,
            _ if binding.read_only == Some(true) => PlanAccess::Reader,
            _ => PlanAccess::Maker,
        }
    };
    if binding.member_access.is_some()
        && matches!(access, PlanAccess::Maker | PlanAccess::Reader)
        && !binding
            .worktree
            .as_deref()
            .zip(binding.lease.as_deref())
            .is_some_and(|(worktree, lease)| crate::same_worktree(worktree, lease))
    {
        return Err(Error::invalid(
            "this agent is not attached to its assigned lease",
        ));
    }
    Ok((access, binding.slice))
}

fn authorize(access: PlanAccess, slice: Option<&str>, action: &PlanAction) -> Result<()> {
    if access == PlanAccess::Human {
        return Ok(());
    }
    if !access.tools().contains(&action.name()) {
        return Err(Error::invalid(
            "this seat is not allowed to perform that planning action",
        ));
    }
    if access == PlanAccess::Maker && (slice.is_none() || action.slice() != slice) {
        return Err(Error::invalid("a maker may update only its assigned slice"));
    }
    Ok(())
}

fn empty(chat: &Chat) -> ChatPlan {
    ChatPlan {
        chat_id: chat.id,
        project_id: chat.project_id,
        revision: 0,
        bundle: None,
    }
}

fn snapshot(store: &planner::Store, chat: &Chat) -> Result<ChatPlan> {
    // Keep the cursor and every bundle query on the same SQLite read snapshot.
    let _read = store.db().conn().unchecked_transaction()?;
    let Some(plan) = engine::find(store, chat)? else {
        return Ok(empty(chat));
    };
    Ok(ChatPlan {
        chat_id: chat.id,
        project_id: chat.project_id,
        revision: engine::revision(store.db().conn(), plan.id)?,
        bundle: Some(store.bundle(plan.id)?),
    })
}

fn create_plan(
    store: &mut planner::Store,
    chat: &Chat,
    name: &str,
    action: PlanAction,
) -> Result<()> {
    let PlanAction::CreatePlan { title, summary, .. } = action else {
        return Err(Error::invalid("expected a plan creation"));
    };
    writes::text(&title, "title", 240)?;
    if let Some(summary) = &summary {
        writes::text(summary, "summary", 16_000)?;
    }
    // Stable explicit IDs, not GitContext::detect/cwd/branch affinity resolution.
    let repo = store.ensure_repo(&planner::GitContext {
        repo_key: engine::repo_key(chat),
        repo_name: name.into(),
        remote_url: None,
        main_path: PathBuf::from(&chat.workspace_path),
        worktree: PathBuf::from(&chat.workspace_path),
        branch: None,
        head_sha: None,
    })?;
    store.create_plan(planner::NewPlan {
        repo_id: repo.id,
        title,
        summary,
        slug: Some(format!("chat-{}", chat.id)),
        ..Default::default()
    })?;
    Ok(())
}
