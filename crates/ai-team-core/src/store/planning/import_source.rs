//! Reading somebody else's planner database without ever writing to it.
//!
//! The standalone tool's database is not opened through `planner::Store`: that runs the
//! engine's migrations and sets `journal_mode=WAL` on whatever it is handed, which would
//! modify the operator's live planner the moment ai-team looked at it. A plain read-only
//! connection is not enough either - a WAL database whose `-shm` is absent cannot be
//! opened read-only at all, which is exactly the state a cleanly closed `aip` leaves.
//!
//! So the source is snapshotted: the file and any journal beside it are copied into a
//! private temporary directory, digested before and after the copy so a database being
//! written during the read is refused rather than torn, and only the copy is ever opened.
//! The original is read with `File::read` and nothing else.

use std::io::Read;
use std::path::{Path, PathBuf};

use ai_planner_core::{self as planner, DecisionStatus, LogKind, Renders, Status};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// The engine revision this build pins. A source written by a newer ai-planner may hold
/// columns this code does not read, and silently dropping them is the one thing import
/// must not do - so it is refused and said, rather than half-imported.
const KNOWN_SCHEMA: i64 = 5;

/// Big enough for any plan database, small enough that a mistyped path naming a disk
/// image is refused before it is copied.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

const REQUIRED: [&str; 11] = [
    "repo",
    "plan",
    "plan_source",
    "plan_section",
    "decision",
    "slice",
    "slice_dep",
    "question",
    "gotcha",
    "log",
    "handoff",
];

pub(super) struct Source {
    /// Removed when this is dropped. Nothing outside this module sees the copy.
    _dir: tempfile::TempDir,
    conn: Connection,
    pub canonical: PathBuf,
    pub bytes: u64,
    pub digest: String,
    pub schema_version: i64,
}

impl Source {
    /// `forbidden` names ai-team's own databases: importing one into itself would be a
    /// second writable copy of a plan this store already owns.
    pub(super) fn open(path: &str, forbidden: &[PathBuf]) -> Result<Source> {
        let typed = crate::expand_user(Path::new(path.trim()))?;
        if typed.as_os_str().is_empty() {
            return Err(Error::invalid("name the planner database to read"));
        }
        let canonical = typed
            .canonicalize()
            .map_err(|error| Error::invalid(format!("cannot read {}: {error}", typed.display())))?;
        let metadata = std::fs::metadata(&canonical)?;
        if !metadata.is_file() {
            return Err(Error::invalid(format!(
                "{} is not a file",
                canonical.display()
            )));
        }
        if metadata.len() == 0 {
            return Err(Error::invalid(format!("{} is empty", canonical.display())));
        }
        if metadata.len() > MAX_BYTES {
            return Err(Error::invalid(format!(
                "{} is {} bytes, which is far larger than a plan database",
                canonical.display(),
                metadata.len()
            )));
        }
        for own in forbidden {
            if own.canonicalize().ok().as_deref() == Some(canonical.as_path()) {
                return Err(Error::invalid(
                    "that is ai-team's own database - import reads another tool's planner",
                ));
            }
        }

        let dir = tempfile::tempdir()?;
        let copy = dir.path().join("source.sqlite");
        let digest = snapshot(&canonical, &copy)?;

        // The copy, not the original: opening read-write lets SQLite recover the journal
        // that was copied with it, and every byte it writes is ours.
        let conn = Connection::open(&copy)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| {
            Error::invalid(format!(
                "{} is not a readable SQLite database: {error}",
                canonical.display()
            ))
        })?;
        conn.pragma_update(None, "query_only", true)?;
        let application: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
        if application == super::engine::APPLICATION_ID {
            return Err(Error::invalid(
                "this is an AI Team-owned planning store, not a standalone import source",
            ));
        }

        let schema_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|_| {
                Error::invalid(format!(
                    "{} is not an ai-planner database",
                    canonical.display()
                ))
            })?;
        if schema_version < 1 {
            return Err(Error::invalid(format!(
                "{} has no ai-planner schema",
                canonical.display()
            )));
        }
        if schema_version > KNOWN_SCHEMA {
            return Err(Error::invalid(format!(
                "that database is at ai-planner schema {schema_version}; this build reads \
                 up to {KNOWN_SCHEMA}, and importing it could drop what the newer schema \
                 added. Update ai-team before importing it."
            )));
        }
        for table in REQUIRED {
            let present: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )?;
            if !present {
                return Err(Error::invalid(format!(
                    "{} is missing the ai-planner `{table}` table",
                    canonical.display()
                )));
            }
        }

        Ok(Source {
            _dir: dir,
            conn,
            canonical,
            bytes: metadata.len(),
            digest,
            schema_version,
        })
    }

    pub(super) fn describe(&self) -> Result<crate::plan_library::PlanSource> {
        Ok(crate::plan_library::PlanSource {
            path: self.canonical.to_string_lossy().into_owned(),
            bytes: self.bytes,
            digest: self.digest.clone(),
            schema_version: self.schema_version,
            plans: self
                .conn
                .query_row("SELECT COUNT(*) FROM plan", [], |row| row.get(0))?,
        })
    }

    /// Every plan in the source, as a list to choose one from.
    pub(super) fn plans(&self) -> Result<Vec<Listed>> {
        let mut statement = self.conn.prepare(
            "SELECT p.id, r.key, r.name, p.slug, p.title, p.status, p.summary,
                    p.created_at, p.updated_at,
                    (SELECT COUNT(*) FROM slice s WHERE s.plan_id = p.id),
                    (SELECT COUNT(*) FROM slice s WHERE s.plan_id = p.id AND s.status = 'done'),
                    (SELECT COUNT(*) FROM question q WHERE q.plan_id = p.id AND q.status = 'open')
             FROM plan p JOIN repo r ON r.id = p.repo_id
             ORDER BY p.updated_at DESC, p.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Listed {
                id: row.get(0)?,
                repo_key: row.get(1)?,
                repo_name: row.get(2)?,
                slug: row.get(3)?,
                title: row.get(4)?,
                status: row.get(5)?,
                summary: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                slices: row.get(9)?,
                done: row.get(10)?,
                open_questions: row.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Everything one plan holds, read in a single transaction so the parts cannot
    /// disagree, and validated here so nothing fails halfway through a write later.
    pub(super) fn extract(&self, plan_id: i64) -> Result<SourcePlan> {
        let read = self.conn.unchecked_transaction()?;
        let summary = self
            .plans()?
            .into_iter()
            .find(|plan| plan.id == plan_id)
            .ok_or_else(|| Error::invalid("that plan is not in this database"))?;

        let (raw_md, source_path, ticket_key, ticket_url, base_branch, owner) =
            self.conn.query_row(
                "SELECT raw_md, source_path, ticket_key, ticket_url, base_branch, owner
                 FROM plan WHERE id = ?1",
                [plan_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )?;

        let plan = SourcePlan {
            repo_key: summary.repo_key.clone(),
            repo_name: summary.repo_name.clone(),
            repo_remote: self.conn.query_row(
                "SELECT r.remote_url FROM repo r JOIN plan p ON p.repo_id = r.id WHERE p.id = ?1",
                [plan_id],
                |row| row.get(0),
            )?,
            repo_main_path: self.conn.query_row(
                "SELECT r.main_path FROM repo r JOIN plan p ON p.repo_id = r.id WHERE p.id = ?1",
                [plan_id],
                |row| row.get(0),
            )?,
            id: summary.id,
            slug: summary.slug.clone(),
            title: summary.title.clone(),
            status: summary.status,
            summary: summary.summary.clone(),
            ticket_key,
            ticket_url,
            base_branch,
            owner,
            raw_md,
            source_path,
            created_at: summary.created_at.clone(),
            updated_at: summary.updated_at.clone(),
            sources: self.sources(plan_id)?,
            sections: self.sections(plan_id)?,
            decisions: self.decisions(plan_id)?,
            slices: self.slices(plan_id)?,
            deps: self.deps(plan_id)?,
            questions: self.questions(plan_id)?,
            gotchas: self.gotchas(plan_id)?,
            log: self.log(plan_id)?,
            handoffs: self.handoffs(plan_id)?,
            file_imports: self.file_imports(plan_id)?,
            affinities: self.count("plan_affinity", plan_id)?,
            embeddings: self.count("embedding", plan_id)?,
        };
        drop(read);
        Ok(plan)
    }

    fn count(&self, table: &str, plan_id: i64) -> Result<i64> {
        let present: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if !present {
            return Ok(0);
        }
        Ok(self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE plan_id = ?1"),
            [plan_id],
            |row| row.get(0),
        )?)
    }

    fn sources(&self, plan_id: i64) -> Result<Vec<SourceRef>> {
        let mut statement = self.conn.prepare(
            "SELECT kind, ref, note, created_at FROM plan_source WHERE plan_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceRef {
                kind: row.get(0)?,
                reference: row.get(1)?,
                note: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn sections(&self, plan_id: i64) -> Result<Vec<SourceSection>> {
        let mut statement = self.conn.prepare(
            "SELECT ord, key, title, body, renders, created_at, updated_at
             FROM plan_section WHERE plan_id = ?1 ORDER BY ord, id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceSection {
                ord: row.get(0)?,
                key: row.get(1)?,
                title: row.get(2)?,
                body: row.get(3)?,
                renders: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        let sections = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for section in &sections {
            Renders::parse(&section.renders)
                .map_err(|error| Error::invalid(format!("section {}: {error}", section.key)))?;
        }
        Ok(sections)
    }

    fn decisions(&self, plan_id: i64) -> Result<Vec<SourceDecision>> {
        let mut statement = self.conn.prepare(
            "SELECT ord, key, title, body, status, superseded_by, supersede_note,
                    decided_at, updated_at
             FROM decision WHERE plan_id = ?1 ORDER BY ord, id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceDecision {
                ord: row.get(0)?,
                key: row.get(1)?,
                title: row.get(2)?,
                body: row.get(3)?,
                status: row.get(4)?,
                superseded_by: row.get(5)?,
                supersede_note: row.get(6)?,
                decided_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        let decisions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for decision in &decisions {
            DecisionStatus::parse(&decision.status)
                .map_err(|error| Error::invalid(format!("decision {}: {error}", decision.key)))?;
        }
        Ok(decisions)
    }

    fn slices(&self, plan_id: i64) -> Result<Vec<SourceSlice>> {
        let mut statement = self.conn.prepare(
            "SELECT ord, key, title, status, scope_md, demo_md, estimate_files, branch,
                    base_branch, pr_url, worktree_path, claimed_by, claimed_at,
                    blocked_reason, started_at, completed_at, created_at, updated_at
             FROM slice WHERE plan_id = ?1 ORDER BY ord, id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceSlice {
                ord: row.get(0)?,
                key: row.get(1)?,
                title: row.get(2)?,
                status: row.get(3)?,
                scope_md: row.get(4)?,
                demo_md: row.get(5)?,
                estimate_files: row.get(6)?,
                branch: row.get(7)?,
                base_branch: row.get(8)?,
                pr_url: row.get(9)?,
                worktree_path: row.get(10)?,
                claimed_by: row.get(11)?,
                claimed_at: row.get(12)?,
                blocked_reason: row.get(13)?,
                started_at: row.get(14)?,
                completed_at: row.get(15)?,
                created_at: row.get(16)?,
                updated_at: row.get(17)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Dependency edges, by key: the ids are the source's and mean nothing here.
    fn deps(&self, plan_id: i64) -> Result<Vec<SourceDep>> {
        let mut statement = self.conn.prepare(
            "SELECT a.key, b.key FROM slice_dep d
             JOIN slice a ON a.id = d.slice_id
             JOIN slice b ON b.id = d.depends_on_id
             WHERE a.plan_id = ?1 AND b.plan_id = ?1
             ORDER BY a.key, b.key",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceDep {
                slice: row.get(0)?,
                depends_on: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn questions(&self, plan_id: i64) -> Result<Vec<SourceQuestion>> {
        let mut statement = self.conn.prepare(
            "SELECT s.key, q.body, q.status, q.answer, q.asked_at, q.answered_at
             FROM question q LEFT JOIN slice s ON s.id = q.slice_id
             WHERE q.plan_id = ?1 ORDER BY q.id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceQuestion {
                slice: row.get(0)?,
                body: row.get(1)?,
                status: row.get(2)?,
                answer: row.get(3)?,
                asked_at: row.get(4)?,
                answered_at: row.get(5)?,
            })
        })?;
        let questions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for question in &questions {
            if !matches!(question.status.as_str(), "open" | "answered" | "dropped") {
                return Err(Error::invalid(format!(
                    "a question has status {:?}, which this build does not know",
                    question.status
                )));
            }
        }
        Ok(questions)
    }

    fn gotchas(&self, plan_id: i64) -> Result<Vec<SourceGotcha>> {
        let mut statement = self
            .conn
            .prepare("SELECT title, body, created_at FROM gotcha WHERE plan_id = ?1 ORDER BY id")?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceGotcha {
                title: row.get(0)?,
                body: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn log(&self, plan_id: i64) -> Result<Vec<SourceLog>> {
        let mut statement = self.conn.prepare(
            "SELECT s.key, l.at, l.actor, l.kind, l.branch, l.worktree_path, l.body
             FROM log l LEFT JOIN slice s ON s.id = l.slice_id
             WHERE l.plan_id = ?1 ORDER BY l.id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceLog {
                slice: row.get(0)?,
                at: row.get(1)?,
                actor: row.get(2)?,
                kind: row.get(3)?,
                branch: row.get(4)?,
                worktree_path: row.get(5)?,
                body: row.get(6)?,
            })
        })?;
        let log = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for entry in &log {
            LogKind::parse(&entry.kind)
                .map_err(|error| Error::invalid(format!("progress note: {error}")))?;
        }
        Ok(log)
    }

    fn handoffs(&self, plan_id: i64) -> Result<Vec<SourceHandoff>> {
        let mut statement = self.conn.prepare(
            "SELECT worktree_path, branch, head_sha, gates_json, resume_md, next_md, actor, at
             FROM handoff WHERE plan_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceHandoff {
                worktree_path: row.get(0)?,
                branch: row.get(1)?,
                head_sha: row.get(2)?,
                gates_json: row.get(3)?,
                resume_md: row.get(4)?,
                next_md: row.get(5)?,
                actor: row.get(6)?,
                at: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn file_imports(&self, plan_id: i64) -> Result<Vec<SourceFileImport>> {
        let present: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'plan_import')",
            [],
            |row| row.get(0),
        )?;
        if !present {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT source_path, sha256, bytes, imported_at FROM plan_import
             WHERE plan_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([plan_id], |row| {
            Ok(SourceFileImport {
                source_path: row.get(0)?,
                sha256: row.get(1)?,
                bytes: row.get(2)?,
                imported_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Is this exact plan already here? Asked of the owned store, keyed on the source's
    /// own identity rather than on the path, so a moved or copied database is still the
    /// same plan and still refused.
    pub(super) fn import_key(repo_key: &str, slug: &str, created_at: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"ai-team-plan-import\0");
        hasher.update(repo_key.as_bytes());
        hasher.update(b"\0");
        hasher.update(slug.as_bytes());
        hasher.update(b"\0");
        hasher.update(created_at.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// What the preview promised to write. Anything else is a different import.
pub(super) fn fingerprint(plan: &SourcePlan) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-team-plan-import-preview\0");
    hasher.update(serde_json::to_vec(plan)?);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copy the database and whatever journal sits beside it, and prove the original did not
/// move while we read it. A database mid-write would copy as a main file from one instant
/// and a journal from another, which is how a "successful" import ends up missing commits.
fn snapshot(source: &Path, copy: &Path) -> Result<String> {
    const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];
    for _ in 0..3 {
        let before = digest(source)?;
        // A checkpoint can remove the source WAL between attempts. Never replay a
        // previous attempt's journal onto the new main file, or copy lock-state SHM.
        for suffix in SIDECARS {
            match std::fs::remove_file(with_suffix(copy, suffix)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        std::fs::copy(source, copy)?;
        for suffix in ["-wal", "-journal"] {
            let beside = with_suffix(source, suffix);
            match std::fs::metadata(&beside) {
                Ok(_) => {
                    std::fs::copy(&beside, with_suffix(copy, suffix))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if digest(copy)? == before && digest(source)? == before {
            return Ok(before);
        }
    }
    Err(Error::invalid(
        "that database is being written right now; close the program using it and read it again",
    ))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// The database and its write-ahead log together: a commit that is only in the `-wal` is
/// still a change to the source, and a digest over the main file alone would miss it.
fn digest(source: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    for suffix in ["", "-wal", "-journal"] {
        let path = with_suffix(source, suffix);
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if !suffix.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
                hasher.update(b"absent\0");
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return Err(Error::invalid(
                "the source and its journals must be bounded regular files",
            ));
        }
        let mut buffer = vec![0u8; 1024 * 1024];
        hasher.update(suffix.as_bytes());
        hasher.update(metadata.len().to_le_bytes());
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > MAX_BYTES {
                return Err(Error::invalid("the planner snapshot exceeds 512 MiB"));
            }
            hasher.update(&buffer[..read]);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// One plan as the chooser sees it, before anything is extracted.
pub(super) struct Listed {
    pub id: i64,
    pub repo_key: String,
    pub repo_name: String,
    pub slug: String,
    pub title: String,
    pub status: Status,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub slices: i64,
    pub done: i64,
    pub open_questions: i64,
}

/// Everything one source plan holds. Serialized verbatim for the preview fingerprint, so
/// adding a field here also tightens what a stale preview means - which is the right way
/// round.
#[derive(Debug, Clone, Serialize)]
pub(super) struct SourcePlan {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_remote: Option<String>,
    pub repo_main_path: Option<String>,
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub status: Status,
    pub summary: Option<String>,
    pub ticket_key: Option<String>,
    pub ticket_url: Option<String>,
    pub base_branch: Option<String>,
    pub owner: Option<String>,
    pub raw_md: Option<String>,
    pub source_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub sources: Vec<SourceRef>,
    pub sections: Vec<SourceSection>,
    pub decisions: Vec<SourceDecision>,
    pub slices: Vec<SourceSlice>,
    pub deps: Vec<SourceDep>,
    pub questions: Vec<SourceQuestion>,
    pub gotchas: Vec<SourceGotcha>,
    pub log: Vec<SourceLog>,
    pub handoffs: Vec<SourceHandoff>,
    pub file_imports: Vec<SourceFileImport>,
    pub affinities: i64,
    pub embeddings: i64,
}

impl SourcePlan {
    pub(super) fn key(&self) -> String {
        Source::import_key(&self.repo_key, &self.slug, &self.created_at)
    }

    pub(super) fn held(&self) -> Vec<&SourceSlice> {
        self.slices.iter().filter(|slice| slice.held()).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceRef {
    pub kind: String,
    pub reference: String,
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceSection {
    pub ord: i64,
    pub key: String,
    pub title: String,
    pub body: String,
    pub renders: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceDecision {
    pub ord: i64,
    pub key: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub superseded_by: Option<String>,
    pub supersede_note: Option<String>,
    pub decided_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceSlice {
    pub ord: i64,
    pub key: String,
    pub title: String,
    pub status: Status,
    pub scope_md: String,
    pub demo_md: Option<String>,
    pub estimate_files: Option<i64>,
    pub branch: Option<String>,
    pub base_branch: Option<String>,
    pub pr_url: Option<String>,
    pub worktree_path: Option<String>,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<String>,
    pub blocked_reason: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl SourceSlice {
    /// Execution state from another machine: a claim, a lease, a branch, a pull request.
    /// None of it is authority here, so all of it is reported before anything is written.
    pub(super) fn held(&self) -> bool {
        self.claimed_by.is_some()
            || self.claimed_at.is_some()
            || self.worktree_path.is_some()
            || self.branch.is_some()
            || self.base_branch.is_some()
            || self.pr_url.is_some()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceDep {
    pub slice: String,
    pub depends_on: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceQuestion {
    pub slice: Option<String>,
    pub body: String,
    pub status: String,
    pub answer: Option<String>,
    pub asked_at: String,
    pub answered_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceGotcha {
    pub title: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceLog {
    pub slice: Option<String>,
    pub at: String,
    pub actor: Option<String>,
    pub kind: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceHandoff {
    pub worktree_path: String,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub gates_json: Option<String>,
    pub resume_md: String,
    pub next_md: String,
    pub actor: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceFileImport {
    pub source_path: String,
    pub sha256: String,
    pub bytes: i64,
    pub imported_at: String,
}

/// The engine this build pins, named once so a dependency bump fails here rather than
/// quietly reading a schema it does not know.
pub(super) fn engine_schema_is_known(store: &planner::Store) -> Result<()> {
    let version = store.db().schema_version()?;
    if version != KNOWN_SCHEMA {
        return Err(Error::invalid(format!(
            "the embedded planner engine is at schema {version}, not the {KNOWN_SCHEMA} this \
             import was written against"
        )));
    }
    Ok(())
}

/// Where a source plan already landed here, if it did. The duplicate check.
pub(super) struct Imported {
    pub slug: String,
    pub imported_at: String,
}

pub(super) fn imported_plan(conn: &Connection, key: &str) -> Result<Option<Imported>> {
    Ok(conn
        .query_row(
            "SELECT p.slug, i.imported_at
             FROM plan_import i JOIN plan p ON p.id = i.plan_id
             WHERE i.sha256 = ?1 ORDER BY i.id LIMIT 1",
            [key],
            |row| {
                Ok(Imported {
                    slug: row.get(0)?,
                    imported_at: row.get(1)?,
                })
            },
        )
        .optional()?)
}

#[cfg(test)]
#[path = "import_source_tests.rs"]
mod tests;
