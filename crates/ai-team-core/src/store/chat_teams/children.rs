use crate::chat::team::{children::ChildRecord, ownership::Ownership};
use crate::{Error, Result, Store};
use rusqlite::{params, Connection};
use std::path::Path;

// Both owners use the same spawn/identity/drain protocol, but operator checkout work
// never manufactures a team execution. SQL identifiers are fixed here, not supplied.
fn scope(owner: &Ownership) -> (&'static str, &'static str, i64) {
    match (owner.setup, owner.checkout) {
        (Some(id), _) => ("workspace_setup_child", "setup_id", id),
        (_, Some(id)) => ("checkout_child", "operation_id", id),
        _ => ("chat_child", "run_id", owner.run),
    }
}
impl Store {
    pub(crate) fn chat_child_epoch(&self, run: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT child_epoch FROM chat_team_run WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn checkout_child_epoch(&self, id: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT child_epoch FROM chat_checkout_operation WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn begin_chat_child(
        &mut self,
        owner: &Ownership,
        kind: &str,
        program: &str,
        workspace: Option<&Path>,
        boot: &str,
    ) -> Result<ChildRecord> {
        let (table, key, id) = scope(owner);
        self.db_mut().write(|tx| {
            check(tx,owner)?;
            tx.execute(&format!("INSERT INTO {table} ({key},epoch,kind,program,workspace,boot,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)"), params![id,owner.epoch(),kind,program,workspace.map(|p|p.to_string_lossy().into_owned()),boot,crate::now()])?;
            Ok(ChildRecord { id:tx.last_insert_rowid(),run:id,boot:boot.into(),pid:None,identity:None,state:"intent".into() })
        })
    }
    pub(crate) fn record_chat_child_pid(
        &mut self,
        owner: &Ownership,
        child: i64,
        pid: i64,
    ) -> Result<()> {
        let (table, key, id) = scope(owner);
        self.db_mut().write(|tx| {
            check(tx,owner)?;
            if tx.execute(&format!("UPDATE {table} SET pid=?2 WHERE id=?1 AND {key}=?3 AND epoch=?4 AND state='intent' AND pid IS NULL"),params![child,pid,id,owner.epoch()])? != 1 { return Err(Error::invalid("spawn intent no longer belongs to this controller")); }
            Ok(())
        })
    }
    pub(crate) fn attach_chat_child(
        &mut self,
        owner: &Ownership,
        child: i64,
        pid: i64,
        identity: &str,
    ) -> Result<()> {
        let (table, key, id) = scope(owner);
        self.db_mut().write(|tx| {
            check(tx,owner)?;
            if tx.execute(&format!("UPDATE {table} SET identity=?3,state='running' WHERE id=?1 AND {key}=?4 AND epoch=?5 AND state='intent' AND pid=?2 AND identity IS NULL"), params![child,pid,identity,id,owner.epoch()])? != 1 { return Err(Error::invalid("child intent no longer belongs to this controller")); }
            Ok(())
        })
    }
    pub(crate) fn finish_chat_child(&mut self, owner: &Ownership, child: i64) -> Result<()> {
        let (table, key, id) = scope(owner);
        self.db_mut().write(|tx| {
            check(tx, owner)?;
            if tx.execute(
                &format!("UPDATE {table} SET state='drained',ended_at=?2 WHERE id=?1 AND {key}=?3"),
                params![child, crate::now(), id],
            )? != 1
            {
                return Err(Error::invalid("child does not belong to this execution"));
            }
            Ok(())
        })
    }
    pub(crate) fn chat_children(&self, run: i64) -> Result<Vec<ChildRecord>> {
        records(self.db().conn(), "chat_child", "run_id", run)
    }
    pub(crate) fn checkout_children(&self, id: i64) -> Result<Vec<ChildRecord>> {
        records(self.db().conn(), "checkout_child", "operation_id", id)
    }
    pub(crate) fn workspace_setup_children(&self, id: i64) -> Result<Vec<ChildRecord>> {
        records(self.db().conn(), "workspace_setup_child", "setup_id", id)
    }
}
fn records(conn: &Connection, table: &str, key: &str, id: i64) -> Result<Vec<ChildRecord>> {
    let mut query = conn.prepare(&format!(
        "SELECT id,{key},boot,pid,identity,state FROM {table} WHERE {key}=?1 ORDER BY id"
    ))?;
    let rows = query
        .query_map([id], |r| {
            Ok(ChildRecord {
                id: r.get(0)?,
                run: r.get(1)?,
                boot: r.get(2)?,
                pid: r.get(3)?,
                identity: r.get(4)?,
                state: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
pub(super) fn check(conn: &Connection, owner: &Ownership) -> Result<()> {
    if let Some(id) = owner.setup {
        return super::super::workspace_setup::check_child_owner(conn, owner, id);
    }
    if let Some(id) = owner.checkout {
        return super::super::chat_checkout::check_child_owner(conn, owner, id);
    }
    if let Some(delivery) = owner.delivery {
        return super::super::chat_changes::check_child_owner(conn, owner, delivery);
    }
    let current: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_team_run t JOIN chat c ON c.id=t.chat_id WHERE t.run_id=?1 AND t.child_epoch=?2 AND t.child_journal=1 AND t.supervisor_pid=?3 AND t.phase!='finished' AND c.active_node_id=t.control_node_id AND c.archived=0)",params![owner.run,owner.epoch(),i64::from(std::process::id())],|r|r.get(0))?;
    if !current || owner.epoch() == 0 {
        return Err(Error::invalid(
            "this child controller no longer owns the execution",
        ));
    }
    Ok(())
}
