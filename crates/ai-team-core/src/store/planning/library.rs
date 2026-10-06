//! Every plan ai-team owns, across every project, resolved back to the chat it belongs to.
//!
//! The board is a read and nothing else. It asks the owned engine store for its plans and
//! the team database for the projects and chats those plans name, and it refuses to guess
//! when the two disagree: a plan whose chat is gone, or whose chat belongs to a different
//! project than its repo key says, is listed as detached rather than attached to whatever
//! row happens to hold that id now. That is the same rule the import destination list
//! follows, which is why both are here.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use super::engine;
use crate::plan_library::{
    chat_of_slug, project_of_repo_key, DetachedPlan, PlanImportTarget, PlanLibrary,
    PlanLibraryEntry, PlanLibraryFilter, PlanLibraryProject, PlanProvenance,
};
use crate::{Error, Project, Result, Store};

/// A chat an import may be written into: nobody is talking in it, nothing has run in it,
/// and it is not filed away. Shared by the listing and by the approval, so the board
/// cannot offer a destination the write would refuse.
const IDLE_EMPTY: &str = "c.archived = 0 AND c.stop_requested = 0 AND c.active_node_id IS NULL
     AND NOT EXISTS(SELECT 1 FROM chat_turn t WHERE t.chat_id = c.id)";

impl Store {
    pub fn plan_library(&self, filter: &PlanLibraryFilter) -> Result<PlanLibrary> {
        let projects: HashMap<i64, Project> = self
            .projects()?
            .into_iter()
            .map(|project| (project.id, project))
            .collect();
        // A filter naming a project that is not here is a question the caller asked
        // wrongly, not a fault in the board: said as an invalid request so the window
        // shows the sentence rather than a server error.
        let wanted = filter
            .project
            .as_deref()
            .map(|slug| {
                self.find_project(slug)
                    .map_err(|error| Error::invalid(error.to_string()))
            })
            .transpose()?
            .map(|project| project.id);

        let mut entries = Vec::new();
        let mut detached = Vec::new();
        let mut counted: HashMap<i64, i64> = HashMap::new();
        let mut taken: HashSet<i64> = HashSet::new();

        for plan in self.owned_plans()? {
            let Some(chat_id) = chat_of_slug(&plan.slug) else {
                detached.push(plan.detach("this plan's slug does not name a chat"));
                continue;
            };
            taken.insert(chat_id);
            let Some(project_id) = project_of_repo_key(&plan.repo_key) else {
                detached.push(plan.detach("this plan's repository is not an ai-team project"));
                continue;
            };
            let Some(project) = projects.get(&project_id) else {
                detached.push(plan.detach("this plan's project no longer exists"));
                continue;
            };
            let Some(chat) = self.chat_row(chat_id)? else {
                detached.push(plan.detach(
                    "this plan's chat no longer exists, so nothing can open it. Its chat id \
                     stays reserved rather than being handed to a new chat",
                ));
                continue;
            };
            if chat.project_id != project_id {
                detached.push(
                    plan.detach(
                        "this plan's chat belongs to a different project than the plan does",
                    ),
                );
                continue;
            }
            *counted.entry(project_id).or_default() += 1;
            if wanted.is_some_and(|id| id != project_id) {
                continue;
            }
            if !filter.status.is_empty() && !filter.status.contains(&plan.status) {
                continue;
            }
            entries.push(PlanLibraryEntry {
                plan_id: plan.id,
                slug: plan.slug,
                title: plan.title,
                status: plan.status,
                summary: plan.summary,
                project_id,
                project_slug: project.slug.clone(),
                project_name: project.name.clone(),
                chat_id,
                chat_title: chat.title,
                chat_archived: chat.archived,
                slices: plan.slices,
                done: plan.done,
                open_questions: plan.open_questions,
                updated_at: plan.updated_at,
                last_activity: plan.last_activity,
                imported: plan.imported,
            });
        }

        let mut library_projects: Vec<PlanLibraryProject> = counted
            .into_iter()
            .filter_map(|(id, plans)| {
                projects.get(&id).map(|project| PlanLibraryProject {
                    id,
                    slug: project.slug.clone(),
                    name: project.name.clone(),
                    plans,
                })
            })
            .collect();
        library_projects.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(PlanLibrary {
            entries,
            projects: library_projects,
            destinations: self.import_destinations(&projects, &taken)?,
            detached,
        })
    }

    /// Chats that can take an import right now, in every project.
    fn import_destinations(
        &self,
        projects: &HashMap<i64, Project>,
        taken: &HashSet<i64>,
    ) -> Result<Vec<PlanImportTarget>> {
        let mut statement = self.db().conn().prepare(&format!(
            "SELECT c.id, c.project_id, c.title, c.workspace_path, c.created_at
             FROM chat c WHERE {IDLE_EMPTY} ORDER BY c.updated_at DESC, c.id DESC"
        ))?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut targets = Vec::new();
        for row in rows {
            let (chat_id, project_id, title, workspace_path, created_at) = row?;
            // A chat id an owned plan still names is not free, even when the chat row
            // itself looks empty: the plan would collide with it or be hidden by it.
            if taken.contains(&chat_id) {
                continue;
            }
            let Some(project) = projects.get(&project_id) else {
                continue;
            };
            targets.push(PlanImportTarget {
                chat_id,
                project_id,
                project_slug: project.slug.clone(),
                project_name: project.name.clone(),
                title,
                workspace_path,
                created_at,
            });
        }
        targets.sort_by(|a, b| {
            a.project_name
                .cmp(&b.project_name)
                .then(b.chat_id.cmp(&a.chat_id))
        });
        Ok(targets)
    }

    fn chat_row(&self, id: i64) -> Result<Option<ChatRow>> {
        Ok(rusqlite::OptionalExtension::optional(
            self.db().conn().query_row(
                "SELECT title, archived, project_id FROM chat WHERE id = ?1",
                [id],
                |row| {
                    Ok(ChatRow {
                        title: row.get(0)?,
                        archived: row.get(1)?,
                        project_id: row.get(2)?,
                    })
                },
            ),
        )?)
    }

    fn owned_plans(&self) -> Result<Vec<OwnedPlan>> {
        if self.path() == std::path::Path::new(":memory:") {
            return Ok(Vec::new());
        }
        let Some(store) = engine::open(&self.planning_path()?, false)? else {
            return Ok(Vec::new());
        };
        let conn = store.db().conn();
        // One read snapshot, so the counts cannot describe a different moment than the
        // rows they are counting.
        let read = conn.unchecked_transaction()?;
        let mut statement = conn.prepare(
            "SELECT p.id, r.key, p.slug, p.title, p.status, p.summary, p.updated_at,
                    (SELECT COUNT(*) FROM slice s WHERE s.plan_id = p.id),
                    (SELECT COUNT(*) FROM slice s WHERE s.plan_id = p.id AND s.status = 'done'),
                    (SELECT COUNT(*) FROM question q WHERE q.plan_id = p.id AND q.status = 'open'),
                    (SELECT MAX(l.at) FROM log l WHERE l.plan_id = p.id),
                    (SELECT i.source_path FROM plan_import i WHERE i.plan_id = p.id
                     ORDER BY i.id LIMIT 1),
                    (SELECT i.imported_at FROM plan_import i WHERE i.plan_id = p.id
                     ORDER BY i.id LIMIT 1),
                    (SELECT s.ref FROM plan_source s WHERE s.plan_id = p.id
                     AND s.kind = 'ai-team-import' ORDER BY s.id LIMIT 1)
             FROM plan p JOIN repo r ON r.id = p.repo_id
             ORDER BY COALESCE((SELECT MAX(l.at) FROM log l WHERE l.plan_id = p.id),
                               p.updated_at) DESC, p.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let source_path: Option<String> = row.get(11)?;
            let imported_at: Option<String> = row.get(12)?;
            let source_plan: Option<String> = row.get(13)?;
            Ok(OwnedPlan {
                id: row.get(0)?,
                repo_key: row.get(1)?,
                slug: row.get(2)?,
                title: row.get(3)?,
                status: row.get(4)?,
                summary: row.get(5)?,
                updated_at: row.get(6)?,
                slices: row.get(7)?,
                done: row.get(8)?,
                open_questions: row.get(9)?,
                last_activity: row.get(10)?,
                imported: source_path
                    .zip(imported_at)
                    .map(|(path, at)| PlanProvenance {
                        source_path: path,
                        source_plan,
                        imported_at: at,
                    }),
            })
        })?;
        let plans = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(read);
        Ok(plans)
    }
}

/// Why an import into this chat would be refused, in the words the operator needs.
pub(super) fn importable(conn: &Connection, chat_id: i64) -> Result<()> {
    let (archived, stop_requested, active, turns): (bool, bool, Option<i64>, i64) = conn
        .query_row(
            "SELECT c.archived, c.stop_requested, c.active_node_id,
                    (SELECT COUNT(*) FROM chat_turn t WHERE t.chat_id = c.id)
             FROM chat c WHERE c.id = ?1",
            [chat_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| Error::invalid(format!("no chat {chat_id}")))?;
    if archived {
        return Err(Error::invalid(
            "that chat is archived, and an archived chat's plan is read-only",
        ));
    }
    if active.is_some() || stop_requested {
        return Err(Error::invalid(
            "that chat has a turn of its own running; import into an idle chat so nothing \
             is working against a plan while it appears",
        ));
    }
    if turns > 0 {
        return Err(Error::invalid(
            "that chat has already run work; import into an empty chat rather than dropping \
             somebody else's plan into a conversation that is under way",
        ));
    }
    Ok(())
}

struct ChatRow {
    title: String,
    archived: bool,
    project_id: i64,
}

struct OwnedPlan {
    id: i64,
    repo_key: String,
    slug: String,
    title: String,
    status: crate::planning::PlanStatus,
    summary: Option<String>,
    updated_at: String,
    slices: i64,
    done: i64,
    open_questions: i64,
    last_activity: Option<String>,
    imported: Option<PlanProvenance>,
}

impl OwnedPlan {
    fn detach(self, why: &str) -> DetachedPlan {
        DetachedPlan {
            plan_id: self.id,
            slug: self.slug,
            title: self.title,
            repo_key: self.repo_key,
            why: why.to_string(),
        }
    }
}
