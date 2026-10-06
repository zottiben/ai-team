//! Read a standalone plan, show exactly what would be written, then write it once.
//!
//! Every refusal happens before a byte is written, and the write itself is one engine
//! transaction: a plan with half its slices, or a sidecar holding a plan whose chat never
//! got it, is not a state this can leave behind. The destination is one explicitly chosen
//! chat that is idle, empty and has no plan - never "the current one", and never a chat id
//! that an earlier plan is still attached to, because an `INTEGER PRIMARY KEY` is a reused
//! rowid and a fresh chat can be handed a deleted one's number.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::engine;
use super::import_report;
use super::import_source::{self, Source, SourcePlan};
use super::library;
use crate::plan_library::{
    chat_of_slug, PlanImportPreview, PlanImportRequest, PlanImported, PlanImportedInto, PlanSource,
    PlanSourcePlan, PlanSourceSurvey,
};
use crate::{Chat, Error, Project, Result, Store};

/// The section key the import record is written under, and the fallbacks if the source
/// already uses it. A collision must not overwrite the plan's own section.
const RECORD: &str = "ai-team-import";

impl Store {
    /// What a database holds, without choosing anything from it.
    pub fn read_plan_source(&self, path: &str) -> Result<PlanSourceSurvey> {
        let source = Source::open(path, &self.own_databases())?;
        let described = source.describe()?;
        let owned = self.owned_imports()?;
        let plans = source
            .plans()?
            .into_iter()
            .map(|plan| {
                let key = Source::import_key(&plan.repo_key, &plan.slug, &plan.created_at);
                PlanSourcePlan {
                    id: plan.id,
                    repo_key: plan.repo_key,
                    repo_name: plan.repo_name,
                    slug: plan.slug,
                    title: plan.title,
                    status: plan.status,
                    summary: plan.summary,
                    slices: plan.slices,
                    done: plan.done,
                    open_questions: plan.open_questions,
                    created_at: plan.created_at,
                    updated_at: plan.updated_at,
                    already_imported: owned.get(&key).cloned(),
                }
            })
            .collect();
        Ok(PlanSourceSurvey {
            source: described,
            plans,
        })
    }

    /// Everything the import would write, and everything it would not.
    pub fn preview_plan_import(&self, path: &str, plan_id: i64) -> Result<PlanImportPreview> {
        let source = Source::open(path, &self.own_databases())?;
        let described = source.describe()?;
        let plan = source.extract(plan_id)?;
        let report = import_report::report(&plan);
        let key = plan.key();
        let already = self.owned_imports()?.get(&key).cloned();
        let refusal = already.as_ref().map(|into| {
            format!(
                "this exact plan was already imported on {} and is chat {}'s plan ({}). \
                 Open it there rather than making a second copy.",
                into.imported_at, into.chat_id, into.title
            )
        });
        Ok(PlanImportPreview {
            plan: PlanSourcePlan {
                id: plan.id,
                repo_key: plan.repo_key.clone(),
                repo_name: plan.repo_name.clone(),
                slug: plan.slug.clone(),
                title: plan.title.clone(),
                status: plan.status,
                summary: plan.summary.clone(),
                slices: import_report::count(plan.slices.len()),
                done: import_report::count(
                    plan.slices
                        .iter()
                        .filter(|slice| slice.status == ai_planner_core::Status::Done)
                        .count(),
                ),
                open_questions: import_report::count(
                    plan.questions
                        .iter()
                        .filter(|question| question.status == "open")
                        .count(),
                ),
                created_at: plan.created_at.clone(),
                updated_at: plan.updated_at.clone(),
                already_imported: already,
            },
            fingerprint: import_source::fingerprint(&plan)?,
            counts: report.counts.clone(),
            preserved: report.preserved.clone(),
            warnings: report.warnings.clone(),
            evidence: report.evidence.clone(),
            source: described,
            refusal,
        })
    }

    /// Approve one reviewed plan into one chosen chat.
    pub fn import_plan(&mut self, request: &PlanImportRequest) -> Result<PlanImported> {
        if request.fingerprint.trim().is_empty() {
            return Err(Error::invalid(
                "an import is approved from a preview; this request carries no fingerprint",
            ));
        }
        let chat = self.chat(request.chat_id)?;
        let project = self.project(chat.project_id)?;
        let planning = self.planning_path()?;

        // Read and check outside the write lock: copying and digesting a database is not
        // something to hold every other planning writer behind.
        let source = Source::open(&request.path, &self.own_databases())?;
        let described = source.describe()?;
        let plan = source.extract(request.plan_id)?;
        if import_source::fingerprint(&plan)? != request.fingerprint {
            return Err(Error::invalid(
                "that plan changed in the source since you previewed it; read it again and \
                 review what is different before importing",
            ));
        }
        let report = import_report::report(&plan);
        let at = crate::now();
        let record = import_report::provenance(&plan, &described, &report, &at);
        let key = plan.key();

        let (plan_id, revision) = self.db_mut().write(|tx| {
            // Re-checked here, inside the lock that every planning writer shares: the chat
            // could have taken a turn between the preview and this approval.
            library::importable(tx, chat.id)?;
            let store = engine::open(&planning, true)?
                .ok_or_else(|| Error::invalid("the owned planning store could not be opened"))?;
            import_source::engine_schema_is_known(&store)?;
            let conn = store.db().conn();
            refuse_reused_chat(conn, chat.id)?;
            refuse_duplicate(conn, &key)?;

            let write = conn.unchecked_transaction()?;
            let plan_id = write_plan(
                &write,
                Destination {
                    chat: &chat,
                    project: &project,
                },
                Incoming {
                    plan: &plan,
                    source: &described,
                    record: &record,
                    key: &key,
                    at: &at,
                },
            )?;
            write.commit()?;
            Ok((plan_id, engine::revision(conn, plan_id)?))
        })?;

        Ok(PlanImported {
            chat_id: chat.id,
            project_id: project.id,
            project_slug: project.slug,
            plan_id,
            slug: format!("chat-{}", chat.id),
            title: plan.title,
            revision,
            counts: report.counts,
            warnings: report.warnings,
            source: described,
        })
    }

    /// ai-team's own files. Handed to the reader so importing one of them into itself is
    /// refused rather than producing a second writable copy of a plan already here.
    fn own_databases(&self) -> Vec<PathBuf> {
        let mut own = vec![self.path().to_path_buf()];
        if let Ok(planning) = self.planning_path() {
            own.push(planning);
        }
        own
    }

    /// Every plan here that came from a source database, by its source identity.
    pub(super) fn owned_imports(&self) -> Result<HashMap<String, PlanImportedInto>> {
        let Some(store) = engine::open(&self.planning_path()?, false)? else {
            return Ok(HashMap::new());
        };
        let conn = store.db().conn();
        let mut statement = conn.prepare(
            "SELECT i.sha256, p.slug, p.title, i.imported_at, r.key
             FROM plan_import i JOIN plan p ON p.id = i.plan_id JOIN repo r ON r.id = p.repo_id
             ORDER BY i.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut found = HashMap::new();
        for row in rows {
            let (key, slug, title, imported_at, repo_key) = row?;
            let Some(chat_id) = chat_of_slug(&slug) else {
                continue;
            };
            let project_slug = crate::plan_library::project_of_repo_key(&repo_key)
                .and_then(|id| self.project(id).ok())
                .map(|project| project.slug);
            found.insert(
                key,
                PlanImportedInto {
                    chat_id,
                    project_slug,
                    title,
                    imported_at,
                },
            );
        }
        Ok(found)
    }
}

/// A plan already attached to this chat id, including one left by a chat that no longer
/// exists. Either way the chat is not empty of planning, and an import into it would
/// collide with or hide the plan that is there.
fn refuse_reused_chat(conn: &Connection, chat_id: i64) -> Result<()> {
    let slug = format!("chat-{chat_id}");
    let held: Option<(String, String)> = conn
        .query_row(
            "SELECT r.key, p.title FROM plan p JOIN repo r ON r.id = p.repo_id WHERE p.slug = ?1",
            [&slug],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match held {
        None => Ok(()),
        Some((repo, title)) => Err(Error::invalid(format!(
            "the owned planning store already holds a plan for chat {chat_id} ({title}, under \
             {repo}). Import into a different chat; a chat id can be reused, and a plan left \
             behind by an earlier one is not this chat's."
        ))),
    }
}

fn refuse_duplicate(conn: &Connection, key: &str) -> Result<()> {
    match import_source::imported_plan(conn, key)? {
        None => Ok(()),
        Some(found) => Err(Error::invalid(format!(
            "that plan was already imported on {} and is chat {}'s plan. AI Team keeps one \
             writable copy of a plan, so a second import is refused.",
            found.imported_at,
            chat_of_slug(&found.slug)
                .map(|id| id.to_string())
                .unwrap_or(found.slug)
        ))),
    }
}

struct Destination<'a> {
    chat: &'a Chat,
    project: &'a Project,
}

struct Incoming<'a> {
    plan: &'a SourcePlan,
    source: &'a PlanSource,
    record: &'a str,
    key: &'a str,
    at: &'a str,
}

/// One transaction, in the engine's own schema.
///
/// Written as SQL rather than through the engine's write API on purpose: that API stamps
/// `now()` on everything it creates, and a progress log that arrives dated today is not
/// the plan's history, it is a copy of it with the history removed.
fn write_plan(tx: &Transaction<'_>, into: Destination<'_>, incoming: Incoming<'_>) -> Result<i64> {
    let plan_id = write_header(tx, into, &incoming)?;
    write_narrative(tx, plan_id, &incoming)?;
    let ids = write_slices(tx, plan_id, incoming.plan)?;
    write_notes(tx, plan_id, incoming.plan, &ids)?;
    // The one import record, and the only thing in this table: `aip import` writes here
    // too, but its code never runs against the store ai-team owns.
    tx.execute(
        "INSERT INTO plan_import (plan_id, source_path, sha256, bytes, imported_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            plan_id,
            incoming.source.path,
            incoming.key,
            i64::try_from(incoming.source.bytes).unwrap_or(i64::MAX),
            incoming.at
        ],
    )?;
    Ok(plan_id)
}

/// The repository row this project's plans live under, and the plan itself.
fn write_header(
    tx: &Transaction<'_>,
    into: Destination<'_>,
    incoming: &Incoming<'_>,
) -> Result<i64> {
    let Destination { chat, project } = into;
    let Incoming { plan, at, .. } = *incoming;
    let repo_key = format!("ai-team-project-{}", project.id);
    let slug = format!("chat-{}", chat.id);

    tx.execute(
        "INSERT INTO repo (key, name, remote_url, main_path, created_at)
         VALUES (?1, ?2, NULL, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET name = excluded.name,
             main_path = COALESCE(excluded.main_path, repo.main_path)",
        params![repo_key, project.name, chat.workspace_path, at],
    )?;
    let repo_id: i64 = tx.query_row("SELECT id FROM repo WHERE key = ?1", [&repo_key], |row| {
        row.get(0)
    })?;

    tx.execute(
        "INSERT INTO plan (repo_id, slug, title, status, summary, ticket_key, ticket_url,
                           base_branch, owner, raw_md, source_path, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![
            repo_id,
            slug,
            plan.title,
            plan.status,
            plan.summary,
            plan.ticket_key,
            plan.ticket_url,
            plan.base_branch,
            plan.owner,
            plan.raw_md,
            plan.source_path,
            at,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Sources, sections and decisions: the document, plus the record of where it came from.
fn write_narrative(tx: &Transaction<'_>, plan_id: i64, incoming: &Incoming<'_>) -> Result<()> {
    let Incoming {
        plan,
        source,
        record,
        at,
        ..
    } = *incoming;

    for reference in &plan.sources {
        tx.execute(
            "INSERT INTO plan_source (plan_id, kind, ref, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                plan_id,
                reference.kind,
                reference.reference,
                reference.note,
                reference.created_at
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO plan_source (plan_id, kind, ref, note, created_at)
         VALUES (?1, 'ai-team-import', ?2, ?3, ?4)",
        params![
            plan_id,
            format!("{}/{}", plan.repo_key, plan.slug),
            format!("imported from {} (sha256 {})", source.path, source.digest),
            at,
        ],
    )?;

    for section in &plan.sections {
        tx.execute(
            "INSERT INTO plan_section (plan_id, ord, key, title, body, renders, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![plan_id, section.ord, section.key, section.title, section.body,
                section.renders, section.created_at, section.updated_at],
        )?;
    }
    let taken: Vec<&str> = plan
        .sections
        .iter()
        .map(|section| section.key.as_str())
        .collect();
    tx.execute(
        "INSERT INTO plan_section (plan_id, ord, key, title, body, renders, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'Imported from ai-planner', ?4, 'body', ?5, ?5)",
        params![
            plan_id,
            plan.sections
                .iter()
                .map(|section| section.ord)
                .min()
                .unwrap_or(10)
                - 10,
            free_key(&taken)?,
            record,
            at,
        ],
    )?;

    for decision in &plan.decisions {
        tx.execute(
            "INSERT INTO decision (plan_id, ord, key, title, body, status, superseded_by,
                                   supersede_note, decided_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                plan_id,
                decision.ord,
                decision.key,
                decision.title,
                decision.body,
                decision.status,
                decision.superseded_by,
                decision.supersede_note,
                decision.decided_at,
                decision.updated_at
            ],
        )?;
    }
    Ok(())
}

/// The slices and the edges between them.
///
/// Deliberately without `branch`, `base_branch`, `pr_url`, `worktree_path`, `claimed_by`
/// and `claimed_at`: those are another machine's live state. They are in the import
/// record, where they are evidence rather than a claim ai-team holds.
fn write_slices<'a>(
    tx: &Transaction<'_>,
    plan_id: i64,
    plan: &'a SourcePlan,
) -> Result<HashMap<&'a str, i64>> {
    let mut ids: HashMap<&str, i64> = HashMap::new();
    for slice in &plan.slices {
        tx.execute(
            "INSERT INTO slice (plan_id, ord, key, title, status, scope_md, demo_md,
                                estimate_files, blocked_reason, started_at, completed_at,
                                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                plan_id,
                slice.ord,
                slice.key,
                slice.title,
                slice.status,
                slice.scope_md,
                slice.demo_md,
                slice.estimate_files,
                slice.blocked_reason,
                slice.started_at,
                slice.completed_at,
                slice.created_at,
                slice.updated_at
            ],
        )?;
        ids.insert(slice.key.as_str(), tx.last_insert_rowid());
    }
    for dep in &plan.deps {
        let (Some(slice), Some(on)) = (
            ids.get(dep.slice.as_str()),
            ids.get(dep.depends_on.as_str()),
        ) else {
            continue;
        };
        tx.execute(
            "INSERT OR IGNORE INTO slice_dep (slice_id, depends_on_id) VALUES (?1, ?2)",
            params![slice, on],
        )?;
    }
    Ok(ids)
}

/// Questions, gotchas, the progress log and the handoffs - everything dated, with its
/// own dates.
fn write_notes(
    tx: &Transaction<'_>,
    plan_id: i64,
    plan: &SourcePlan,
    ids: &HashMap<&str, i64>,
) -> Result<()> {
    for question in &plan.questions {
        tx.execute(
            "INSERT INTO question (plan_id, slice_id, body, status, answer, asked_at, answered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                plan_id,
                question.slice.as_deref().and_then(|key| ids.get(key)),
                question.body,
                question.status,
                question.answer,
                question.asked_at,
                question.answered_at
            ],
        )?;
    }

    for gotcha in &plan.gotchas {
        tx.execute(
            "INSERT INTO gotcha (plan_id, title, body, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![plan_id, gotcha.title, gotcha.body, gotcha.created_at],
        )?;
    }

    for entry in &plan.log {
        tx.execute(
            "INSERT INTO log (plan_id, slice_id, at, actor, kind, branch, worktree_path, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                plan_id,
                entry.slice.as_deref().and_then(|key| ids.get(key)),
                entry.at,
                entry.actor,
                entry.kind,
                entry.branch,
                entry.worktree_path,
                entry.body
            ],
        )?;
    }

    for handoff in &plan.handoffs {
        tx.execute(
            "INSERT INTO handoff (plan_id, worktree_path, branch, head_sha, gates_json,
                                  resume_md, next_md, actor, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                plan_id,
                handoff.worktree_path,
                handoff.branch,
                handoff.head_sha,
                handoff.gates_json,
                handoff.resume_md,
                handoff.next_md,
                handoff.actor,
                handoff.at
            ],
        )?;
    }
    Ok(())
}

/// A section key the plan is not already using. Bounded, because an unbounded search for
/// a free name is an unbounded loop in a write transaction.
fn free_key(taken: &[&str]) -> Result<String> {
    (0..=taken.len())
        .map(|n| match n {
            0 => RECORD.to_string(),
            n => format!("{RECORD}-{}", n + 1),
        })
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .ok_or_else(|| Error::invalid("this plan has no free section key for its import record"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_record_never_overwrites_a_section_the_plan_already_has() {
        assert_eq!(free_key(&["outcome", "log"]).unwrap(), "ai-team-import");
        assert_eq!(free_key(&["ai-team-import"]).unwrap(), "ai-team-import-2");
        assert_eq!(
            free_key(&["ai-team-import", "ai-team-import-2"]).unwrap(),
            "ai-team-import-3"
        );
    }
}
