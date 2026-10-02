//! Durable exact setup approvals. Filesystem writes stay in the toolbox service.

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};

use crate::{
    toolbox::{Outcome, Preview, Saved},
    Error, Result, Store,
};

impl Store {
    pub(crate) fn register_toolbox_root(&mut self, project: i64, path: &Path) -> Result<()> {
        self.project(project)?;
        let path = path.canonicalize()?.to_string_lossy().into_owned();
        self.db_mut().write(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO toolbox_root (project_id,path) VALUES (?1,?2)",
                params![project, path],
            )?;
            Ok(())
        })
    }

    pub fn toolbox_roots(&self, project: i64) -> Result<Vec<String>> {
        self.project(project)?;
        let mut roots: Vec<String> = self
            .project_repos(project)?
            .into_iter()
            .filter_map(|r| r.main_path)
            .collect();
        let mut query = self
            .db()
            .conn()
            .prepare("SELECT path FROM toolbox_root WHERE project_id=?1")?;
        roots.extend(
            query
                .query_map([project], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        roots.sort();
        roots.dedup();
        Ok(roots)
    }

    pub(crate) fn toolbox_root(&self, project: i64, root: &str) -> Result<PathBuf> {
        let path = Path::new(root).canonicalize()?;
        if !self.toolbox_roots(project)?.iter().any(|r| {
            Path::new(r)
                .canonicalize()
                .is_ok_and(|registered| registered == path)
        }) {
            return Err(Error::invalid(
                "toolbox target is not a registered root of this project",
            ));
        }
        Ok(path)
    }

    pub(crate) fn save_toolbox_preview(&mut self, project: i64, saved: &Saved) -> Result<Preview> {
        let root = saved.frozen.inputs.root.to_string_lossy().into_owned();
        self.toolbox_root(project, &root)?;
        let json = serde_json::to_string(saved)?;
        let id = self.db_mut().write(|tx| {
            tx.execute("INSERT INTO toolbox_preview(project_id,root,snapshot_json,created_at) VALUES (?1,?2,?3,?4)", params![project,root,json,crate::now()])?;
            Ok(tx.last_insert_rowid())
        })?;
        self.toolbox_preview(project, id)
    }

    pub(crate) fn saved_toolbox_preview(
        &self,
        project: i64,
        id: i64,
    ) -> Result<(Saved, String, Option<Outcome>)> {
        let row = self.db().conn().query_row("SELECT snapshot_json,state,outcome_json FROM toolbox_preview WHERE project_id=?1 AND id=?2", params![project,id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<String>>(2)?))).optional()?
            .ok_or_else(|| Error::invalid("no toolbox preview in this project"))?;
        Ok((
            serde_json::from_str(&row.0)?,
            row.1,
            row.2.map(|v| serde_json::from_str(&v)).transpose()?,
        ))
    }

    pub fn toolbox_history(&self, project: i64) -> Result<Vec<crate::toolbox::Record>> {
        self.project(project)?;
        let mut query = self.db().conn().prepare("SELECT id,root,state,outcome_json FROM toolbox_preview WHERE project_id=?1 AND state!='preview' ORDER BY CASE WHEN state='applying' THEN 0 ELSE 1 END,id DESC LIMIT 30")?;
        let rows = query
            .query_map([project], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, root, state, json)| {
                Ok(crate::toolbox::Record {
                    id,
                    root,
                    state,
                    outcome: json.map(|v| serde_json::from_str(&v)).transpose()?,
                })
            })
            .collect()
    }

    pub fn toolbox_preview(&self, project: i64, id: i64) -> Result<Preview> {
        let (saved, state, outcome) = self.saved_toolbox_preview(project, id)?;
        Ok(Preview {
            id,
            project_id: project,
            root: saved.frozen.inputs.root.to_string_lossy().into_owned(),
            catalogue_revision: saved.catalogue_revision,
            effects: saved.frozen.effects,
            warnings: saved.warnings,
            state,
            outcome,
        })
    }

    pub(crate) fn claim_toolbox_preview(&mut self, project: i64, id: i64) -> Result<Saved> {
        let (saved, state, _) = self.saved_toolbox_preview(project, id)?;
        if state != "preview" {
            return Err(Error::invalid("toolbox preview was already used; an interrupted apply requires inspection, not replay"));
        }
        let root = saved.frozen.inputs.root.to_string_lossy().into_owned();
        self.toolbox_root(project, &root)?;
        self.db_mut().write(|tx| {
            let busy: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM toolbox_preview WHERE state='applying' UNION ALL SELECT 1 FROM toolbox_operation WHERE state='applying')", [], |r| r.get(0))?;
            if busy { return Err(Error::invalid("another setup is applying or was interrupted here; inspect it before making further changes")); }
            if tx.execute("UPDATE toolbox_preview SET state='applying' WHERE project_id=?1 AND id=?2 AND state='preview'", params![project,id])? != 1 {
                return Err(Error::invalid("toolbox preview changed before approval"));
            }
            Ok(())
        })?;
        Ok(saved)
    }

    pub(crate) fn finish_toolbox_preview(
        &mut self,
        project: i64,
        id: i64,
        outcome: &Outcome,
    ) -> Result<()> {
        let state = match (&outcome.problem, outcome.applied.is_empty()) {
            (None, _) => "applied",
            (Some(_), true) if !outcome.uncertain => "refused",
            (Some(_), _) => "partial",
        };
        let json = serde_json::to_string(outcome)?;
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE toolbox_preview SET state=?3,outcome_json=?4 WHERE project_id=?1 AND id=?2 AND state='applying'", params![project,id,state,json])? != 1 {
                return Err(Error::invalid("toolbox apply outcome could not be recorded; inspect the files before retrying"));
            }
            Ok(())
        })
    }
}
