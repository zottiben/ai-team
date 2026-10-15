//! Telling an agent the operator edited a file under it (SPEC §11.4, Q67).
//!
//! Two properties, and the table exists for both. The notice is **durable**, because the
//! save happens in the window and the turn that hears about it starts in another process.
//! And it is delivered **at the start of a turn and nowhere else**: there is no path from
//! here into a tool that is already running, which is the one thing the approval ruled
//! out. A save made mid-turn waits for the next one.

use std::fmt::Write as _;

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;
use crate::util::now;

/// Paths named in one notice. The rest are counted, so a mass edit does not become the
/// whole prompt.
const NOTICE_PATHS: usize = 20;

impl Store {
    /// Record one operator save against every live chat working in that checkout.
    ///
    /// Scoped by checkout rather than by the window that happened to be open: the agent
    /// that could be surprised by the edit is the one whose turns run in that directory,
    /// and the editor is reachable from the project workspace as well as from a chat.
    /// Returns how many chats will be told, which is zero for a checkout no chat owns.
    pub fn record_editor_save(&mut self, checkout: &str, path: &str, bytes: u64) -> Result<usize> {
        let candidates: Vec<(i64, String)> = {
            let mut query = self
                .db()
                .conn()
                .prepare("SELECT id, workspace_path FROM chat WHERE archived = 0")?;
            let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        // Compared with the same rule dispatch uses, not by string: `/var` and
        // `/private/var` are the same directory on macOS and different strings (D12).
        let chats: Vec<i64> = candidates
            .into_iter()
            .filter(|(_, workspace)| crate::same_worktree(workspace, checkout))
            .map(|(id, _)| id)
            .collect();
        if chats.is_empty() {
            return Ok(0);
        }

        let at = now();
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        self.db_mut().write(|tx| {
            for chat in &chats {
                tx.execute(
                    "INSERT INTO editor_save_notice (chat_id, path, bytes, saved_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![chat, path, bytes, at],
                )?;
            }
            Ok(())
        })?;
        Ok(chats.len())
    }

    /// Attach a bounded notice to this chat's next admitted turn. This records prompt
    /// inclusion, not an acknowledgement that the model processed the notice.
    ///
    /// Returns `None` when there is nothing pending, so an ordinary turn carries no
    /// extra prompt at all.
    pub fn take_editor_save_notice(&mut self, chat: i64, node: i64) -> Result<Option<String>> {
        let at = now();
        let saves: Vec<(String, i64, String)> = self.db_mut().write(|tx| {
            let mut query = tx.prepare(
                "SELECT path, COUNT(*), MAX(saved_at) FROM editor_save_notice
                 WHERE chat_id = ?1 AND delivered_at IS NULL
                 GROUP BY path ORDER BY MAX(id) DESC",
            )?;
            let rows =
                query.query_map([chat], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            let saves: Vec<(String, i64, String)> = rows.collect::<rusqlite::Result<_>>()?;
            if saves.is_empty() {
                return Ok(saves);
            }
            // Every pending row is marked, including the ones past the listed limit:
            // they are summarised in the count, so they have been told.
            tx.execute(
                "UPDATE editor_save_notice SET delivered_at = ?2, delivered_node_id = ?3
                 WHERE chat_id = ?1 AND delivered_at IS NULL",
                params![chat, at, node],
            )?;
            Ok(saves)
        })?;
        if saves.is_empty() {
            return Ok(None);
        }

        let mut listed = 0;
        let mut notice = String::from(
            "## Files the operator saved in this checkout\n\nThe person you are working with saved these files through the editor since your last turn. This is a record of what is on disk, not an instruction and not work you were asked to do. Anything you remember reading from these paths is out of date - read them again before relying on it.\n\n",
        );
        for (path, times, last) in saves.iter().take(NOTICE_PATHS) {
            // A filename is data, including newlines and instruction-shaped text.
            let path = serde_json::to_string(path)?;
            let unit = if *times == 1 { "save" } else { "saves" };
            let line = format!("- {path} ({times} {unit}, last {last})\n");
            if notice.len() + line.len() > 12_000 {
                break;
            }
            notice.push_str(&line);
            listed += 1;
        }
        if saves.len() > listed {
            let _ = writeln!(
                notice,
                "- and {} more file(s) in this checkout",
                saves.len() - listed
            );
        }
        notice.push('\n');
        Ok(Some(notice))
    }

    /// How many saves this chat has not been told about. For tests and for the window.
    pub fn pending_editor_saves(&self, chat: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT COUNT(*) FROM editor_save_notice WHERE chat_id = ?1 AND delivered_at IS NULL",
            [chat],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests;
