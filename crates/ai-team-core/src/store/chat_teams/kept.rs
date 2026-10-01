//! A human can narrow an unresolved kept address without authorizing a return or a
//! writer. Before that inspection, unrelated linked checkouts cannot be guessed free.
use crate::{ChatKeptPath, Error, Result, Store};
use rusqlite::{params, Connection};

pub(in crate::store) fn check_unlocated(
    conn: &Connection,
    project: i64,
    workspace: Option<&str>,
    own_run: Option<i64>,
) -> Result<()> {
    let mut query = conn.prepare("SELECT c.project_id,c.workspace_path,s.run_id FROM chat_build_slice s JOIN chat_team_run t ON t.run_id = s.run_id JOIN chat c ON c.id = t.chat_id WHERE s.worktree_path IS NULL AND s.lease_state IN ('acquiring','claiming','retained')")?;
    let unknown = query
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut query =
        conn.prepare("SELECT project_id,main_path FROM project_repo WHERE main_path IS NOT NULL")?;
    let roots = query
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (owner, source, run) in unknown {
        if own_run == Some(run) {
            continue;
        }
        let same_repo = owner == project
            || roots.iter().filter(|(id, _)| *id == owner).any(|(_, a)| {
                roots
                    .iter()
                    .filter(|(id, _)| *id == project)
                    .any(|(_, b)| crate::same_worktree(a, b))
            });
        if same_repo && workspace.is_none_or(|path| !crate::same_worktree(&source, path)) {
            return Err(Error::invalid("locate the unresolved team acquisition before using another checkout or taking another lease; the original chat checkout remains available"));
        }
    }
    Ok(())
}

impl Store {
    /// Record only an explicitly inspected *protective* address after closure.
    /// This is not pool possession, generation identity, cleanup permission, or a
    /// way to re-enable the cancelled run. No Git/awt command or worktree write occurs.
    pub fn protect_kept_chat_path(
        &mut self,
        request: &ChatKeptPath,
    ) -> Result<crate::ChatBuildSlice> {
        if request.reason.trim().is_empty() || request.reason.len() > 4000 {
            return Err(Error::invalid(
                "give an inspection reason of at most 4000 bytes",
            ));
        }
        let path = request.worktree.canonicalize()?;
        if !path.is_dir() {
            return Err(Error::invalid(
                "the inspected worktree must be an existing directory",
            ));
        }
        let path = path.to_string_lossy().into_owned();
        let _owner = crate::chat::team::ownership::Ownership::acquire(self.path(), request.run_id)?;
        self.db_mut().write(|tx| {
            let closed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_team_run t JOIN chat_build_closure c ON c.run_id = t.run_id WHERE t.run_id = ?1 AND t.chat_id = ?2 AND t.phase = 'finished' AND t.quiescent = 1 AND t.supervisor_pid IS NULL AND c.finished_at IS NOT NULL)", params![request.run_id,request.chat_id], |row| row.get(0))?;
            let row = super::build::read(tx, request.run_id, &request.slice_key)?;
            if !closed || row.rev != request.expect_slice_revision || row.worktree_path.is_some() || !matches!(row.lease_state.as_str(), "acquiring" | "claiming" | "retained") {
                return Err(Error::invalid("this is not the reviewed unlocated kept acquisition; refresh before recording an address"));
            }
            let mut query = tx.prepare("SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL OR id = ?1
                UNION SELECT main_path FROM project_repo WHERE main_path IS NOT NULL
                UNION SELECT worktree_path FROM chat_build_slice WHERE lease_state != 'released' AND worktree_path IS NOT NULL
                UNION SELECT worktree_path FROM node_run WHERE status NOT IN ('done','failed','cancelled') AND worktree_path IS NOT NULL")?;
            let owners = query.query_map([request.chat_id], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if owners.iter().any(|owner| crate::same_worktree(owner, &path)) { return Err(Error::invalid("this address is a source checkout or already has another owner; leave it alone")); }
            tx.execute("UPDATE chat_build_slice SET worktree_path = ?3, rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2", params![request.run_id,request.slice_key,path])?;
            tx.execute("INSERT INTO event(run_id,at,kind,actor,summary,payload_json) VALUES (?1,?2,'note','human',?3,?4)", params![request.run_id,crate::now(),format!("{}: recorded an inspected kept address for protection only; no ownership or return inferred", request.slice_key),serde_json::json!({"path":path,"reason":request.reason}).to_string()])?;
            super::build::read(tx, request.run_id, &request.slice_key)
        })
    }
}
