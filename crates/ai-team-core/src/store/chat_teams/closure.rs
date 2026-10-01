use crate::{ChatBuildClosure, ChatRetainedBuild, Error, Result, Store};
use rusqlite::{Connection, OptionalExtension};

pub(in crate::store) fn require_open(conn: &Connection, run: i64) -> Result<()> {
    let closing: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_build_closure WHERE run_id = ?1)",
        [run],
        |row| row.get(0),
    )?;
    if closing {
        return Err(Error::invalid("this build's approval was withdrawn; finish its close-and-keep operation, not another model attempt"));
    }
    Ok(())
}

impl Store {
    pub fn chat_build_closure(&self, run: i64) -> Result<Option<ChatBuildClosure>> {
        Ok(self.db().conn().query_row("SELECT run_id,reason,requested_at,finished_at,issues_json FROM chat_build_closure WHERE run_id = ?1", [run], |row| {
            Ok(ChatBuildClosure { run_id: row.get(0)?, reason: row.get(1)?, requested_at: row.get(2)?, finished_at: row.get(3)?, issues: serde_json::from_str(&row.get::<_, String>(4)?).map_err(|error| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error)))? })
        }).optional()?)
    }

    /// Includes finished/archived chats' work. Closing a conversation must never hide
    /// the unresolved pool responsibility or imply its paths are available to reuse.
    pub fn retained_chat_builds(&self, chat: i64) -> Result<Vec<ChatRetainedBuild>> {
        self.chat(chat)?;
        let mut query = self.db().conn().prepare("SELECT t.run_id FROM chat_team_run t WHERE t.chat_id = ?1 AND t.phase IN ('blocked','finished')
            AND EXISTS(SELECT 1 FROM chat_build_slice s WHERE s.run_id = t.run_id AND s.lease_state != 'released') ORDER BY t.run_id DESC")?;
        let runs = query
            .query_map([chat], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        runs.into_iter()
            .map(|run| {
                Ok(ChatRetainedBuild {
                    execution: self
                        .chat_team_run(run)?
                        .ok_or_else(|| Error::invalid("the retained execution disappeared"))?,
                    closure: self.chat_build_closure(run)?,
                    slices: self
                        .chat_build_slices(run)?
                        .into_iter()
                        .filter(|row| row.lease_state != "released")
                        .collect(),
                })
            })
            .collect()
    }
}
