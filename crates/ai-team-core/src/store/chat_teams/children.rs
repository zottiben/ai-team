use crate::chat::team::{children::ChildRecord, ownership::Ownership};
use crate::{Error, Result, Store};
use rusqlite::{params, Connection};
use std::path::Path;

impl Store {
    pub(crate) fn chat_child_epoch(&self, run: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT child_epoch FROM chat_team_run WHERE run_id = ?1",
            [run],
            |row| row.get(0),
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
        self.db_mut().write(|tx| {
            check(tx, owner)?;
            tx.execute("INSERT INTO chat_child (run_id,epoch,kind,program,workspace,boot,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![owner.run, owner.epoch(), kind, program, workspace.map(|p| p.to_string_lossy().into_owned()), boot, crate::now()])?;
            Ok(ChildRecord { id: tx.last_insert_rowid(), run: owner.run, boot: boot.into(), pid: None, identity: None, state: "intent".into() })
        })
    }
    pub(crate) fn record_chat_child_pid(
        &mut self,
        owner: &Ownership,
        id: i64,
        pid: i64,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            check(tx, owner)?;
            if tx.execute("UPDATE chat_child SET pid = ?2 WHERE id = ?1 AND run_id = ?3 AND epoch = ?4 AND state = 'intent' AND pid IS NULL", params![id,pid,owner.run,owner.epoch()])? != 1 { return Err(Error::invalid("spawn intent no longer belongs to this controller")); }
            Ok(())
        })
    }

    pub(crate) fn attach_chat_child(
        &mut self,
        owner: &Ownership,
        id: i64,
        pid: i64,
        identity: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            check(tx, owner)?;
            if tx.execute("UPDATE chat_child SET identity = ?3, state = 'running' WHERE id = ?1 AND run_id = ?4 AND epoch = ?5 AND state = 'intent' AND pid = ?2 AND identity IS NULL", params![id, pid, identity, owner.run, owner.epoch()])? != 1 { return Err(Error::invalid("child intent no longer belongs to this controller")); }
            Ok(())
        })
    }
    pub(crate) fn finish_chat_child(&mut self, owner: &Ownership, id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            check(tx, owner)?;
            if tx.execute("UPDATE chat_child SET state = 'drained', ended_at = ?2 WHERE id = ?1 AND run_id = ?3", params![id, crate::now(), owner.run])? != 1 { return Err(Error::invalid("child does not belong to this execution")); }
            Ok(())
        })
    }
    pub(crate) fn chat_children(&self, run: i64) -> Result<Vec<ChildRecord>> {
        let mut query = self.db().conn().prepare("SELECT id,run_id,boot,pid,identity,state FROM chat_child WHERE run_id = ?1 ORDER BY id")?;
        let rows = query.query_map([run], |row| {
            Ok(ChildRecord {
                id: row.get(0)?,
                run: row.get(1)?,
                boot: row.get(2)?,
                pid: row.get(3)?,
                identity: row.get(4)?,
                state: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

pub(super) fn check(conn: &Connection, owner: &Ownership) -> Result<()> {
    if let Some(delivery) = owner.delivery {
        return super::super::chat_changes::check_child_owner(conn, owner, delivery);
    }
    let current: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_team_run t JOIN chat c ON c.id = t.chat_id WHERE t.run_id = ?1 AND t.child_epoch = ?2 AND t.child_journal = 1 AND t.supervisor_pid = ?3 AND t.phase != 'finished' AND c.active_node_id = t.control_node_id AND c.archived = 0)", params![owner.run, owner.epoch(), i64::from(std::process::id())], |row| row.get(0))?;
    if !current || owner.epoch() == 0 {
        return Err(Error::invalid(
            "this child controller no longer owns the execution",
        ));
    }
    Ok(())
}
