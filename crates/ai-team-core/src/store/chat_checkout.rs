//! Checkout ownership and immutable operator receipts, separate from team verification.
#[cfg(test)]
mod tests;
use crate::{
    chat::team::ownership::Ownership,
    chat_changes::checkout::{Finding, Operation, Snapshot},
    Error, Result, Store,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::sync::Arc;

const SELECT: &str =
    "SELECT id,chat_id,snapshot_json,state,result,rev FROM chat_checkout_operation";
impl Store {
    pub fn checkout_operations(&self, chat: i64) -> Result<Vec<Operation>> {
        self.chat(chat)?;
        let mut q=self.db().conn().prepare(&format!("{SELECT} WHERE chat_id=?1 AND state!='preview' ORDER BY CASE WHEN state IN ('running','inspection') THEN 0 ELSE 1 END,id DESC LIMIT 100"))?;
        let rows = q
            .query_map([chat], from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub fn checkout_operation(&self, chat: i64, id: i64) -> Result<Operation> {
        self.db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND id=?2"),
                params![chat, id],
                from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no checkout approval in this chat"))
    }
    pub(crate) fn checkout_available(&self, chat: i64) -> Result<()> {
        available(self.db().conn(), chat, None)
    }
    pub(crate) fn record_checkout_preview(
        &mut self,
        chat: i64,
        snapshot: &Snapshot,
    ) -> Result<Operation> {
        let json = serde_json::to_string(snapshot)?;
        let id=self.db_mut().write(|tx| {
            available(tx,chat,None)?;
            tx.execute("INSERT INTO chat_checkout_operation(chat_id,workspace_path,snapshot_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",params![chat,snapshot.workspace,json,crate::now()])?;
            Ok(tx.last_insert_rowid())
        })?;
        self.checkout_operation(chat, id)
    }
    pub(crate) fn claim_checkout_operation(
        &mut self,
        chat: i64,
        id: i64,
        rev: i64,
        recover: bool,
    ) -> Result<Arc<Ownership>> {
        self.checkout_operation(chat, id)?;
        let owner = Ownership::acquire_checkout(self.path(), id)?;
        self.db_mut().write(|tx| {
            if !recover { available(tx,chat,Some(id))?; }
            let state=if recover { "state IN ('running','inspection')" } else { "state='preview'" };
            if tx.execute(&format!("UPDATE chat_checkout_operation SET state='running',child_epoch=child_epoch+1,supervisor_pid=?4,rev=rev+1,updated_at=?5 WHERE id=?1 AND chat_id=?2 AND rev=?3 AND {state}"),params![id,chat,rev,i64::from(std::process::id()),crate::now()])? != 1 { return Err(Error::invalid("checkout approval changed or was already used; refresh")); }
            tick(tx,chat)
        })?;
        owner.bind(self)?;
        Ok(owner)
    }
    pub(crate) fn attempt_checkout_operation(&mut self, chat: i64, id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat_checkout_operation SET attempted=1 WHERE id=?1 AND chat_id=?2 AND state='running' AND supervisor_pid=?3",params![id,chat,i64::from(std::process::id())])? != 1 { return Err(Error::invalid("checkout operation is no longer owned")); } Ok(())
        })
    }
    pub(crate) fn settle_checkout_operation(
        &mut self,
        chat: i64,
        id: i64,
        state: &str,
        result: &str,
    ) -> Result<Operation> {
        if !matches!(state, "done" | "refused" | "inspection" | "acknowledged") {
            return Err(Error::invalid("invalid checkout outcome"));
        }
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat_checkout_operation SET state=?3,result=?4,rev=rev+1,supervisor_pid=NULL,updated_at=?5 WHERE id=?1 AND chat_id=?2 AND state='running'",params![id,chat,state,result,crate::now()])? != 1 { return Err(Error::invalid("checkout operation is no longer running")); } tick(tx,chat)
        })?;
        self.checkout_operation(chat, id)
    }
    pub fn checkout_findings(&self, chat: i64) -> Result<Vec<Finding>> {
        self.chat(chat)?;
        let mut q=self.db().conn().prepare("SELECT id,fingerprint,head,area,path,side,line,body,created_at FROM chat_checkout_finding WHERE chat_id=?1 ORDER BY id DESC LIMIT 100")?;
        let rows = q
            .query_map([chat], |r| {
                Ok(Finding {
                    id: r.get(0)?,
                    fingerprint: r.get(1)?,
                    head: r.get(2)?,
                    area: r.get(3)?,
                    path: r.get(4)?,
                    side: r.get(5)?,
                    line: r.get(6)?,
                    body: r.get(7)?,
                    created_at: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub(crate) fn record_checkout_finding(&mut self, chat: i64, f: &Finding) -> Result<()> {
        self.db_mut().write(|tx| {
            available(tx,chat,None)?;
            tx.execute("INSERT INTO chat_checkout_finding(chat_id,fingerprint,head,area,path,side,line,body,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![chat,f.fingerprint,f.head,f.area,f.path,f.side,f.line,f.body,crate::now()])?;
            tick(tx,chat)
        })
    }
}
fn available(conn: &Connection, chat: i64, operation: Option<i64>) -> Result<()> {
    let (workspace,project):(String,i64)=conn.query_row("SELECT c.workspace_path,c.project_id FROM chat c JOIN project p ON p.id=c.project_id WHERE c.id=?1 AND c.active_node_id IS NULL AND c.archived=0 AND p.status!='archived'",[chat],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or_else(||Error::invalid("wait for this chat to be idle; archived chats/projects are read-only"))?;
    match operation {
        Some(id) => super::chat_changes::check_workspace_except_checkout(conn, &workspace, id)?,
        None => super::chat_changes::check_workspace(conn, &workspace)?,
    }
    super::chat_teams::kept::check_unlocated(conn, project, Some(&workspace), None)?;
    let mut q=conn.prepare("SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL UNION SELECT worktree_path FROM node_run WHERE status NOT IN ('done','failed','cancelled') AND worktree_path IS NOT NULL UNION SELECT worktree_path FROM chat_build_slice WHERE lease_state!='released' AND worktree_path IS NOT NULL UNION SELECT workspace_path FROM run WHERE status IN ('queued','running','planning') AND supervisor_pid IS NOT NULL AND workspace_path IS NOT NULL")?;
    let paths = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if paths.iter().any(|p| crate::same_worktree(p, &workspace)) {
        return Err(Error::invalid(
            "another execution or retained lease owns this checkout",
        ));
    }
    Ok(())
}
pub(super) fn check_child_owner(conn: &Connection, owner: &Ownership, id: i64) -> Result<()> {
    let current:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_checkout_operation WHERE id=?1 AND child_epoch=?2 AND state='running' AND supervisor_pid=?3)",params![id,owner.epoch(),i64::from(std::process::id())],|r|r.get(0))?;
    if !current || owner.epoch() == 0 {
        return Err(Error::invalid(
            "this checkout command no longer owns its journal",
        ));
    }
    Ok(())
}
fn tick(conn: &Connection, chat: i64) -> Result<()> {
    conn.execute(
        "UPDATE chat SET rev=rev+1,updated_at=?2 WHERE id=?1",
        params![chat, crate::now()],
    )?;
    Ok(())
}
fn from_row(r: &Row<'_>) -> rusqlite::Result<Operation> {
    let json: String = r.get(2)?;
    let snapshot = serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Operation {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        snapshot,
        state: r.get(3)?,
        result: r.get(4)?,
        rev: r.get(5)?,
    })
}
