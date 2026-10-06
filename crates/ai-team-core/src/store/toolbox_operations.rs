use crate::{
    toolbox::{Operation, Outcome, SavedOperation},
    Error, Result, Store,
};
use rusqlite::{params, OptionalExtension};

#[cfg(test)]
#[path = "toolbox_operations_tests.rs"]
mod tests;

impl Store {
    pub(crate) fn check_toolbox_target(
        &self,
        project: i64,
        workspace: &str,
        approval: Option<i64>,
    ) -> Result<()> {
        super::workspace_setup::check_project(self.db().conn(), project)?;
        super::chat_changes::check_workspace_except_toolbox(self.db().conn(), workspace, approval)?;
        super::chat_teams::kept::check_unlocated(self.db().conn(), project, Some(workspace), None)?;
        let mut q=self.db().conn().prepare("SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL
            UNION SELECT worktree_path FROM node_run WHERE status NOT IN ('done','failed','cancelled') AND worktree_path IS NOT NULL
            UNION SELECT worktree_path FROM chat_build_slice WHERE lease_state!='released' AND worktree_path IS NOT NULL")?;
        let paths = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if paths.iter().any(|p| crate::same_worktree(p, workspace)) {
            return Err(Error::invalid(
                "active or retained work owns this target checkout; settle it before convergence",
            ));
        }
        Ok(())
    }

    pub(crate) fn save_toolbox_operation(&mut self, saved: &SavedOperation) -> Result<Operation> {
        let kind = saved.authority.kind();
        let json = serde_json::to_string(saved)?;
        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO toolbox_operation(kind,snapshot_json,created_at) VALUES (?1,?2,?3)",
                params![kind, json, crate::now()],
            )?;
            Ok(tx.last_insert_rowid())
        })?;
        self.toolbox_operation(id, kind)
    }
    fn saved_toolbox_operation(
        &self,
        id: i64,
        kind: &str,
    ) -> Result<(SavedOperation, String, Option<Outcome>)> {
        let row = self.db().conn().query_row("SELECT snapshot_json,state,outcome_json FROM toolbox_operation WHERE id=?1 AND kind=?2",params![id,kind],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or_else(||Error::invalid("no toolbox approval in this scope"))?;
        Ok((
            serde_json::from_str(&row.0)?,
            row.1,
            row.2.map(|s| serde_json::from_str(&s)).transpose()?,
        ))
    }
    pub fn toolbox_operation(&self, id: i64, kind: &str) -> Result<Operation> {
        let (saved, state, outcome) = self.saved_toolbox_operation(id, kind)?;
        Ok(saved.view(id, state, outcome))
    }
    pub fn toolbox_operations(&self, kind: &str) -> Result<Vec<crate::toolbox::OperationRecord>> {
        let mut query=self.db().conn().prepare("SELECT id,json_extract(snapshot_json,'$.authority'),state,outcome_json FROM toolbox_operation WHERE kind=?1 AND state!='preview' ORDER BY CASE WHEN state='applying' THEN 0 ELSE 1 END,id DESC LIMIT 30")?;
        let rows = query
            .query_map([kind], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, authority, state, outcome)| {
                Ok(crate::toolbox::OperationRecord {
                    id,
                    authority: serde_json::from_str(&authority)?,
                    state,
                    outcome: outcome.map(|s| serde_json::from_str(&s)).transpose()?,
                })
            })
            .collect()
    }
    pub(crate) fn claim_toolbox_operation(
        &mut self,
        id: i64,
        kind: &str,
    ) -> Result<SavedOperation> {
        let (saved, state, _) = self.saved_toolbox_operation(id, kind)?;
        if state != "preview" {
            return Err(Error::invalid(
                "toolbox approval already used; inspect interrupted outcomes, never replay",
            ));
        }
        self.db_mut().write(|tx|{
            let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM toolbox_operation WHERE state='applying' UNION ALL SELECT 1 FROM toolbox_preview WHERE state='applying')",[],|r|r.get(0))?;
            if busy {return Err(Error::invalid("a setup is applying or interrupted; inspect it before further setup"));}
            if tx.execute("UPDATE toolbox_operation SET state='applying' WHERE id=?1 AND kind=?2 AND state='preview'",params![id,kind])?!=1 {return Err(Error::invalid("toolbox approval changed"));}
            Ok(())
        })?;
        Ok(saved)
    }
    pub(crate) fn finish_toolbox_operation(&mut self, id: i64, outcome: &Outcome) -> Result<()> {
        let state = match (
            &outcome.problem,
            outcome.applied.is_empty(),
            outcome.uncertain,
        ) {
            (None, _, _) => "applied",
            (Some(_), true, false) => "refused",
            _ => "partial",
        };
        self.db_mut().write(|tx|{
            if tx.execute("UPDATE toolbox_operation SET state=?2,outcome_json=?3 WHERE id=?1 AND state='applying'",params![id,state,serde_json::to_string(outcome)?])?!=1 {return Err(Error::invalid("could not record setup result; inspect files, do not retry"));}
            Ok(())
        })
    }
}
