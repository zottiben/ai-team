use crate::{
    toolbox::{RegistryChange, RegistryRoot, SavedOperation},
    Error, Result, Store,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

fn roots(conn: &Connection, id: i64) -> Result<Vec<RegistryRoot>> {
    let mut query=conn.prepare("SELECT path FROM toolbox_root WHERE project_id=?1 UNION SELECT main_path FROM project_repo WHERE project_id=?1 AND main_path IS NOT NULL ORDER BY 1")?;
    let paths = query
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    paths
        .into_iter()
        .map(|path| {
            let exists = match std::fs::symlink_metadata(&path) {
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(e.into()),
            };
            Ok(RegistryRoot { path, exists })
        })
        .collect()
}
fn restore_status(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT prior_status FROM toolbox_archived_project WHERE project_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?)
}
fn protected(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM run WHERE project_id=?1 AND status NOT IN ('done','failed','cancelled') UNION ALL SELECT 1 FROM run r JOIN chat_build_slice b ON b.run_id=r.id WHERE r.project_id=?1 AND b.lease_state!='released' UNION ALL SELECT 1 FROM chat c JOIN chat_delivery d ON d.chat_id=c.id WHERE c.project_id=?1 AND d.state IN ('running','inspection') UNION ALL SELECT 1 FROM reminder WHERE project_id=?1 AND status IN ('pending','fired') UNION ALL SELECT 1 FROM toolbox_preview WHERE project_id=?1 AND state='applying')",[id],|r|r.get(0))?)
}
impl Store {
    pub fn toolbox_registrations(&self) -> Result<Vec<RegistryChange>> {
        self.projects()?.into_iter().map(|p|Ok(RegistryChange{project:p.id,name:p.name,revision:p.rev,status:p.status.to_string(),next_status:p.status.to_string(),restore_status:restore_status(self.db().conn(),p.id)?,roots:roots(self.db().conn(),p.id)?,reason:protected(self.db().conn(),p.id)?.then(||"Project has active/retained work or scheduled work; settle it before forgetting its registration.".into())})).collect()
    }
    pub fn toolbox_scan_roots(&self) -> Result<Vec<String>> {
        let mut q = self
            .db()
            .conn()
            .prepare("SELECT path FROM toolbox_scan_root ORDER BY path")?;
        let rows = q
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub(crate) fn apply_toolbox_registry(&mut self, saved: &SavedOperation) -> Result<Vec<String>> {
        self.db_mut().write(|tx|{
            for change in &saved.registrations {
                let (revision,status):(i64,String)=tx.query_row("SELECT rev,status FROM project WHERE id=?1",[change.project],|r|Ok((r.get(0)?,r.get(1)?)))?;
                if revision!=change.revision || status!=change.status || roots(tx,change.project)?!=change.roots || restore_status(tx,change.project)?!=change.restore_status || protected(tx,change.project)? {return Err(Error::invalid("registration preview is stale or the project now owns active work; nothing changed"));}
            }
            if let Some(next)=&saved.scan_roots {
                let mut q=tx.prepare("SELECT path FROM toolbox_scan_root ORDER BY path")?;
                let current=q.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                if current!=saved.prior_scan_roots || next.iter().any(|p|!Path::new(p).is_dir()) {return Err(Error::invalid("discovery-root preview is stale"));}
                tx.execute("DELETE FROM toolbox_scan_root",[])?;
                for path in next {tx.execute("INSERT INTO toolbox_scan_root(path) VALUES (?1)",[path])?;}
            }
            let mut changed=Vec::new();
            for change in &saved.registrations {
                if change.next_status=="archived" {
                    tx.execute("INSERT INTO toolbox_archived_project(project_id,prior_status) VALUES (?1,?2) ON CONFLICT(project_id) DO UPDATE SET prior_status=excluded.prior_status",params![change.project,change.status])?;
                } else {tx.execute("DELETE FROM toolbox_archived_project WHERE project_id=?1",[change.project])?;}
                tx.execute("UPDATE project SET status=?2,rev=rev+1,updated_at=?3 WHERE id=?1 AND rev=?4",params![change.project,change.next_status,crate::now(),change.revision])?;
                changed.push(format!("{}: {}",change.name,change.next_status));
            }
            if saved.scan_roots.is_some(){changed.push("discovery roots".into());}
            Ok(changed)
        })
    }
}
