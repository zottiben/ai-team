//! Idempotent human setup receipts and the fence shared by checkout/turn admission.
use std::{path::Path, sync::Arc};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{
    chat::team::ownership::Ownership,
    workspace_setup::{Setup, Step},
    Error, Result, Store,
};

#[cfg(test)]
mod tests;

const SELECT: &str = "SELECT id,project_id,repo_path,request_id,branch,workspace_path,state,detail,rev FROM workspace_setup";
fn row(r: &Row<'_>) -> rusqlite::Result<Setup> {
    Ok(Setup {
        id: r.get(0)?,
        project_id: r.get(1)?,
        repo_path: r.get(2)?,
        request_id: r.get(3)?,
        branch: r.get(4)?,
        workspace_path: r.get(5)?,
        state: r.get(6)?,
        detail: r.get(7)?,
        rev: r.get(8)?,
        steps: Vec::new(),
    })
}
impl Store {
    /// AWT cannot see pinned chats, pre-spawn reservations or retained app work.
    /// A clean idle checkout can still hold commits/drafts a chat must not lose.
    /// Refuse get if any such pool slot lacks a lease protecting it from reuse.
    pub(crate) fn check_setup_pool(&self, pool: &[crate::PoolEntry]) -> Result<()> {
        let conn = self.db().conn();
        let mut query = conn.prepare("SELECT workspace_path FROM chat
            UNION SELECT workspace_path FROM run WHERE status IN ('queued','planning','running') AND supervisor_pid IS NOT NULL AND workspace_path IS NOT NULL
            UNION SELECT worktree_path FROM node_run WHERE status NOT IN ('done','failed','cancelled') AND worktree_path IS NOT NULL
            UNION SELECT worktree_path FROM chat_build_slice WHERE lease_state NOT IN ('pending','released') AND worktree_path IS NOT NULL")?;
        let protected = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for entry in pool
            .iter()
            .filter(|entry| !entry.main && entry.status != "linked" && entry.lease_holder.is_none())
        {
            super::chat_changes::check_workspace(conn, &entry.path)?;
            if protected
                .iter()
                .any(|path| crate::same_worktree(path, &entry.path))
            {
                return Err(Error::invalid(format!("AWT pool slot {} has active or retained app work without a protecting lease; inspect it before acquisition", entry.path)));
            }
        }
        Ok(())
    }
    pub fn workspace_setup_revision(&self) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT COALESCE(SUM(rev),0) FROM workspace_setup",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn workspace_setup(&self, project: i64, id: i64) -> Result<Setup> {
        let mut setup = self
            .db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE id=?1 AND project_id=?2"),
                params![id, project],
                row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no workspace setup in this project"))?;
        let mut q=self.db().conn().prepare("SELECT command,passed,output,at FROM workspace_setup_step WHERE setup_id=?1 ORDER BY id DESC LIMIT 40")?;
        setup.steps = q
            .query_map([id], |r| {
                Ok(Step {
                    command: r.get(0)?,
                    passed: r.get(1)?,
                    output: r.get(2)?,
                    at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        setup.steps.reverse();
        Ok(setup)
    }
    pub fn workspace_setups(&self, project: i64) -> Result<Vec<Setup>> {
        self.project(project)?;
        let mut q=self.db().conn().prepare("SELECT id FROM workspace_setup WHERE project_id=?1 ORDER BY CASE WHEN state IN ('pending','running','inspection') THEN 0 ELSE 1 END,id DESC LIMIT 40")?;
        let ids = q
            .query_map([project], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| self.workspace_setup(project, id))
            .collect()
    }
    pub fn request_workspace_setup(
        &mut self,
        project: i64,
        repo: &Path,
        request: &str,
        branch: &str,
    ) -> Result<(Setup, bool)> {
        if request.trim().is_empty() || request.len() > 160 || branch.len() > 160 {
            return Err(Error::invalid(
                "a bounded request identity and branch are required",
            ));
        }
        let repo = repo.canonicalize()?.to_string_lossy().into_owned();
        let (id, started)=self.db_mut().write(|tx| {
            if let Some(previous)=tx.query_row(&format!("{SELECT} WHERE project_id=?1 AND request_id=?2"),params![project,request],row).optional()? {
                if previous.repo_path!=repo || previous.branch!=branch { return Err(Error::invalid("this setup request already names a different checkout or branch")); }
                return Ok((previous.id, false));
            }
            let allowed:bool=tx.query_row("SELECT status!='archived' FROM project WHERE id=?1",[project],|r|r.get(0))?;
            if !allowed { return Err(Error::invalid("restore this project before setting up a checkout")); }
            check_project_idle(tx,project)?;
            let unfinished:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_setup WHERE repo_path=?1 AND state IN ('pending','running','inspection'))",[&repo],|r|r.get(0))?;
            if unfinished { return Err(Error::invalid("inspect or finish the existing AWT setup before requesting another")); }
            tx.execute("INSERT INTO workspace_setup(project_id,repo_path,request_id,branch,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",params![project,repo,request,branch,crate::now()])?;
            Ok((tx.last_insert_rowid(), true))
        })?;
        Ok((self.workspace_setup(project, id)?, started))
    }
    pub(crate) fn claim_workspace_setup(
        &mut self,
        project: i64,
        id: i64,
        revision: i64,
        recover: bool,
    ) -> Result<Arc<Ownership>> {
        self.workspace_setup(project, id)?;
        let owner = Ownership::acquire_setup(self.path(), id)?;
        self.db_mut().write(|tx| {
            check_project_idle(tx,project)?;
            let states=if recover { "('pending','running','inspection','failed')" } else { "('pending')" };
            if tx.execute(&format!("UPDATE workspace_setup SET state='running',child_epoch=child_epoch+1,supervisor_pid=?4,rev=rev+1,updated_at=?5 WHERE id=?1 AND project_id=?2 AND rev=?3 AND state IN {states}"),params![id,project,revision,i64::from(std::process::id()),crate::now()])?!=1 { return Err(Error::invalid("setup changed or already started; refresh its receipt")); }
            Ok(())
        })?;
        owner.bind(self)?;
        Ok(owner)
    }
    pub(crate) fn workspace_setup_epoch(&self, id: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT child_epoch FROM workspace_setup WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn setup_progress(&mut self, owner: &Ownership, detail: &str) -> Result<()> {
        self.db_mut().write(|tx| {
            check_child_owner(tx, owner, owner.setup.unwrap_or(0))?;
            tx.execute(
                "UPDATE workspace_setup SET detail=?2,rev=rev+1,updated_at=?3 WHERE id=?1",
                params![owner.setup, detail, crate::now()],
            )?;
            Ok(())
        })
    }
    pub(crate) fn setup_path(&mut self, owner: &Ownership, path: &Path) -> Result<()> {
        let path = path.canonicalize()?.to_string_lossy().into_owned();
        self.db_mut().write(|tx| {
            check_child_owner(tx, owner, owner.setup.unwrap_or(0))?;
            tx.execute(
                "UPDATE workspace_setup SET workspace_path=?2,rev=rev+1,updated_at=?3 WHERE id=?1",
                params![owner.setup, path, crate::now()],
            )?;
            Ok(())
        })
    }
    pub(crate) fn setup_step(
        &mut self,
        owner: &Ownership,
        command: &str,
        passed: bool,
        output: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            check_child_owner(tx,owner,owner.setup.unwrap_or(0))?;
            tx.execute("INSERT INTO workspace_setup_step(setup_id,epoch,command,passed,output,at) VALUES(?1,?2,?3,?4,?5,?6)",params![owner.setup,owner.epoch(),command,passed,output,crate::now()])?;
            Ok(())
        })
    }
    pub(crate) fn finish_workspace_setup(
        &mut self,
        owner: &Ownership,
        state: &str,
        detail: &str,
    ) -> Result<()> {
        if !matches!(state, "ready" | "failed" | "inspection") {
            return Err(Error::invalid("invalid setup outcome"));
        }
        self.db_mut().write(|tx| {
            check_child_owner(tx,owner,owner.setup.unwrap_or(0))?;
            if state!="inspection" {
                let uncertain:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_setup_child WHERE setup_id=?1 AND state!='drained')",[owner.setup],|r|r.get(0))?;
                if uncertain { return Err(Error::invalid("setup children have not drained; keep its fence")); }
            }
            tx.execute("UPDATE workspace_setup SET state=?2,detail=?3,supervisor_pid=NULL,rev=rev+1,updated_at=?4 WHERE id=?1",params![owner.setup,state,detail,crate::now()])?;
            Ok(())
        })
    }
    pub(crate) fn ready_setup_lease(&self, path: &str, holder: &str) -> Result<bool> {
        let mut q=self.db().conn().prepare("SELECT id,workspace_path FROM workspace_setup WHERE state='ready' AND workspace_path IS NOT NULL")?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, p) in rows {
            if crate::same_worktree(&p, path)
                && crate::workspace_setup::holder(self.path(), id)? == holder
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
pub(super) fn check_child_owner(conn: &Connection, owner: &Ownership, id: i64) -> Result<()> {
    let current:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workspace_setup WHERE id=?1 AND child_epoch=?2 AND state='running' AND supervisor_pid=?3)",params![id,owner.epoch(),i64::from(std::process::id())],|r|r.get(0))?;
    if !current || owner.epoch() == 0 {
        return Err(Error::invalid(
            "this setup worker no longer owns its journal",
        ));
    }
    Ok(())
}
pub(super) fn check_project(conn: &Connection, project: i64) -> Result<()> {
    let mut q=conn.prepare("SELECT s.project_id,s.repo_path,r.main_path FROM workspace_setup s LEFT JOIN project_repo r ON r.project_id=?1 WHERE s.state IN ('pending','running','inspection')")?;
    let rows = q
        .query_map([project], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.iter().any(|(id, repo, root)| {
        *id == project
            || root
                .as_deref()
                .is_some_and(|root| crate::same_worktree(root, repo))
    }) {
        return Err(Error::invalid(
            "finish or inspect this repository's AWT setup before starting more work",
        ));
    }
    Ok(())
}
fn check_project_idle(conn: &Connection, project: i64) -> Result<()> {
    super::chat_teams::kept::check_unlocated(conn, project, None, None)?;
    let busy:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM chat WHERE project_id=?1 AND active_node_id IS NOT NULL) OR EXISTS(SELECT 1 FROM run WHERE project_id=?1 AND status IN ('queued','planning','running') AND supervisor_pid IS NOT NULL) OR EXISTS(SELECT 1 FROM node_run n JOIN run r ON r.id=n.run_id WHERE r.project_id=?1 AND n.status NOT IN ('done','failed','cancelled')) OR EXISTS(SELECT 1 FROM chat_delivery d JOIN chat c ON c.id=d.chat_id WHERE c.project_id=?1 AND d.state IN ('running','inspection')) OR EXISTS(SELECT 1 FROM chat_checkout_operation o JOIN chat c ON c.id=o.chat_id WHERE c.project_id=?1 AND o.state IN ('running','inspection'))",[project],|r|r.get(0))?;
    if busy {
        return Err(Error::invalid("wait for this project's execution and checkout actions before acquiring a pooled worktree"));
    }
    Ok(())
}
/// While acquisition has no path yet, every checkout in that project is fenced.
/// A failed dependency install only holds its known path. No failure returns a lease.
pub(super) fn check_workspace(conn: &Connection, workspace: &str) -> Result<()> {
    let mut q=conn.prepare("SELECT s.workspace_path,r.main_path FROM workspace_setup s LEFT JOIN project_repo r ON r.project_id=s.project_id WHERE s.state!='ready' AND (s.state!='failed' OR s.workspace_path IS NOT NULL)")?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (path, root) in rows {
        if path
            .as_deref()
            .is_some_and(|path| crate::same_worktree(path, workspace))
        {
            return Err(Error::invalid(
                "this worktree's setup has not completed; inspect its setup receipt",
            ));
        }
        if path.is_none() {
            if let Some(root) = root {
                if crate::same_worktree(&root, workspace) {
                    return Err(Error::invalid("AWT acquisition is running or uncertain in this repository; inspect it first"));
                }
            }
        }
    }
    Ok(())
}
