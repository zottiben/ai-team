//! Durable operator attention, kept separate from authoritative run/event state.
//!
//! A notification may be inserted by two processes observing the same transition, so
//! `dedupe_key` is the identity and insertion is idempotent. Native delivery is claimed
//! before it is attempted for the same reason reminders are: `ait ui` and `ait daemon`
//! may both be alive.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::model::{NewNotification, Notification};
use crate::store::{non_empty, Store};
use crate::util::now;

impl Store {
    /// Record one attention item, returning `None` when another observer already did.
    pub fn notify_once(&mut self, new: NewNotification) -> Result<Option<Notification>> {
        let at = now();
        let id = self.db_mut().write(|tx| {
            let rows = tx.execute(
                "INSERT OR IGNORE INTO notification
                   (dedupe_key, project_id, workspace_path, run_id, node_run_id, kind,
                    title, body, action_path, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    new.dedupe_key,
                    new.project_id,
                    new.workspace_path,
                    new.run_id,
                    new.node_run_id,
                    new.kind,
                    new.title,
                    new.body,
                    new.action_path,
                    at,
                ],
            )?;
            Ok((rows > 0).then(|| tx.last_insert_rowid()))
        })?;
        id.map(|id| self.notification(id)).transpose()
    }

    pub fn notification(&self, id: i64) -> Result<Notification> {
        self.db()
            .conn()
            .query_row(
                &format!("{NOTIFICATION_SELECT} WHERE id = ?1"),
                params![id],
                notification_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid(format!("no notification {id}")))
    }

    pub fn notifications(&self, limit: i64) -> Result<Vec<Notification>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{NOTIFICATION_SELECT} WHERE cleared_at IS NULL ORDER BY id DESC LIMIT ?1"
        ))?;
        let rows = stmt
            .query_map(params![limit.clamp(1, 200)], notification_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn unread_notification_count(&self) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT COUNT(*) FROM notification WHERE read_at IS NULL AND cleared_at IS NULL",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn read_notification(&mut self, id: i64) -> Result<Notification> {
        let at = now();
        let rows = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE notification SET read_at = COALESCE(read_at, ?2) WHERE id = ?1",
                params![id, at],
            )?)
        })?;
        if rows == 0 {
            return Err(Error::invalid(format!("no notification {id}")));
        }
        self.notification(id)
    }

    /// Read only the inbox snapshot the operator saw; later arrivals stay unread.
    pub fn read_notifications(&mut self, through_id: i64) -> Result<usize> {
        if through_id <= 0 {
            return Err(Error::invalid("notification boundary must be positive"));
        }
        let at = now();
        self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE notification SET read_at = ?2
                 WHERE id <= ?1 AND read_at IS NULL AND cleared_at IS NULL",
                params![through_id, at],
            )?)
        })
    }

    pub fn clear_notification(&mut self, id: i64) -> Result<()> {
        let at = now();
        let rows = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE notification SET cleared_at = COALESCE(cleared_at, ?2),
                   read_at = COALESCE(read_at, ?2) WHERE id = ?1",
                params![id, at],
            )?)
        })?;
        if rows == 0 {
            return Err(Error::invalid(format!("no notification {id}")));
        }
        Ok(())
    }

    /// Keep dedupe keys so an observer cannot recreate a cleared notification.
    pub fn clear_notifications(&mut self, through_id: i64) -> Result<usize> {
        if through_id <= 0 {
            return Err(Error::invalid("notification boundary must be positive"));
        }
        let at = now();
        self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE notification SET cleared_at = ?2, read_at = COALESCE(read_at, ?2)
                 WHERE id <= ?1 AND cleared_at IS NULL",
                params![through_id, at],
            )?)
        })
    }

    pub fn notification_revision(&self) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT revision FROM notification_inbox_revision WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Claim pending native deliveries in one write transaction.
    pub fn claim_notification_delivery(&mut self, limit: i64) -> Result<Vec<Notification>> {
        let at = now();
        self.db_mut().write(|tx| {
            let mut stmt = tx.prepare(&format!(
                "{NOTIFICATION_SELECT}
                  WHERE delivered_at IS NULL AND read_at IS NULL AND cleared_at IS NULL
                    AND (delivery_claimed_at IS NULL
                         OR julianday(delivery_claimed_at) < julianday('now', '-5 minutes'))
                  ORDER BY id
                  LIMIT ?1"
            ))?;
            let pending: Vec<Notification> = stmt
                .query_map(params![limit.clamp(1, 100)], notification_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            let mut claimed = Vec::with_capacity(pending.len());
            for item in pending {
                let rows = tx.execute(
                    "UPDATE notification SET delivery_claimed_at = ?2
                      WHERE id = ?1 AND delivered_at IS NULL
                        AND read_at IS NULL AND cleared_at IS NULL
                        AND (delivery_claimed_at IS NULL
                             OR julianday(delivery_claimed_at) < julianday('now', '-5 minutes'))",
                    params![item.id, at],
                )?;
                if rows == 1 {
                    claimed.push(item);
                }
            }
            Ok(claimed)
        })
    }

    /// Finish a native-delivery claim. Failed delivery waits for the stale-claim window
    /// before retrying, so an unavailable platform notifier is not hammered every poll.
    pub fn complete_notification_delivery(&mut self, id: i64, delivered: bool) -> Result<()> {
        let at = now();
        self.db_mut().write(|tx| {
            if delivered {
                tx.execute(
                    "UPDATE notification
                        SET delivered_at = ?2, delivery_claimed_at = NULL
                      WHERE id = ?1",
                    params![id, at],
                )?;
            } else {
                tx.execute(
                    "UPDATE notification SET delivery_claimed_at = ?2 WHERE id = ?1",
                    params![id, at],
                )?;
            }
            Ok(())
        })
    }

    pub fn latest_notification_id(&self) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT COALESCE(MAX(id), -1) FROM notification",
            [],
            |row| row.get(0),
        )?)
    }
}

/// Called in the same transaction as the authoritative chat transition and its event.
/// The event supplies the body and scope; attention never grants execution authority.
pub(super) fn notify_chat_event(
    tx: &rusqlite::Transaction<'_>,
    event: i64,
    kind: &str,
) -> Result<()> {
    let label = match kind {
        "completed" => "turn complete",
        "plan_ready" => "plan ready to review",
        "failed" => "needs attention",
        _ => "work stopped",
    };
    tx.execute(
        "INSERT OR IGNORE INTO notification
         (dedupe_key, project_id, workspace_path, run_id, node_run_id, kind, title, body, created_at)
         SELECT 'chat-event:' || e.id, c.project_id, c.workspace_path, e.run_id,
                COALESCE(e.node_run_id, t.node_id), ?2, c.title || ' · ' || ?3, e.summary, e.at
         FROM event e JOIN chat_turn t ON t.run_id = e.run_id JOIN chat c ON c.id = t.chat_id
         WHERE e.id = ?1",
        params![event, kind, label],
    )?;
    Ok(())
}

const NOTIFICATION_SELECT: &str = "SELECT id, project_id, workspace_path, run_id,
     node_run_id, kind, title, body, action_path, read_at, delivered_at, created_at,
     COALESCE(notification.chat_id,
      (SELECT t.chat_id FROM chat_turn t JOIN chat c ON c.id = t.chat_id
       WHERE t.run_id = notification.run_id AND c.project_id = notification.project_id))
     FROM notification";

fn notification_from_row(row: &Row<'_>) -> rusqlite::Result<Notification> {
    Ok(Notification {
        id: row.get(0)?,
        project_id: row.get(1)?,
        workspace_path: non_empty(row.get(2)?),
        run_id: row.get(3)?,
        node_run_id: row.get(4)?,
        kind: row.get(5)?,
        title: row.get(6)?,
        body: row.get(7)?,
        action_path: non_empty(row.get(8)?),
        read_at: non_empty(row.get(9)?),
        delivered_at: non_empty(row.get(10)?),
        created_at: row.get(11)?,
        chat_id: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewProject;

    fn seeded() -> (Store, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..NewProject::default()
            })
            .unwrap();
        (store, project.id)
    }

    fn notice(project_id: i64) -> NewNotification {
        NewNotification {
            dedupe_key: "run:1:done".into(),
            project_id,
            workspace_path: Some("/repo/task".into()),
            run_id: None,
            node_run_id: None,
            kind: "completed".into(),
            title: "Backend finished".into(),
            body: "S1 is ready to review".into(),
            action_path: Some("/runs/1".into()),
        }
    }

    #[test]
    fn insertion_is_deduplicated_and_read_state_is_independent() {
        let (mut store, project_id) = seeded();
        let first = store.notify_once(notice(project_id)).unwrap().unwrap();
        assert!(store.notify_once(notice(project_id)).unwrap().is_none());
        assert!(first.read_at.is_none());

        let read = store.read_notification(first.id).unwrap();
        assert!(read.read_at.is_some());
        assert_eq!(store.notifications(10).unwrap().len(), 1);
    }

    #[test]
    fn inbox_actions_keep_dedupe_and_later_arrivals_without_replaying_native_alerts() {
        let (mut store, project) = seeded();
        let first = store.notify_once(notice(project)).unwrap().unwrap();
        let mut next = notice(project);
        next.dedupe_key = "later".into();
        let later = store.notify_once(next).unwrap().unwrap();
        let revision = store.notification_revision().unwrap();
        assert_eq!(store.read_notifications(first.id).unwrap(), 1);
        assert!(store.notification(first.id).unwrap().read_at.is_some());
        assert!(store.notification(later.id).unwrap().read_at.is_none());
        assert!(store.notification_revision().unwrap() > revision);
        assert_eq!(store.unread_notification_count().unwrap(), 1);
        let read_revision = store.notification_revision().unwrap();
        assert_eq!(store.read_notifications(first.id).unwrap(), 0);
        assert_eq!(store.notification_revision().unwrap(), read_revision);
        assert_eq!(store.clear_notifications(first.id).unwrap(), 1);
        assert_eq!(store.notifications(10).unwrap().len(), 1);
        assert!(store.notify_once(notice(project)).unwrap().is_none());
        assert_eq!(
            store.claim_notification_delivery(10).unwrap()[0].id,
            later.id
        );
        store.clear_notification(later.id).unwrap();
        assert!(store.notifications(10).unwrap().is_empty());
        assert_eq!(store.unread_notification_count().unwrap(), 0);
        assert!(store.claim_notification_delivery(10).unwrap().is_empty());
        let cleared_revision = store.notification_revision().unwrap();
        store.clear_notification(later.id).unwrap();
        assert_eq!(store.notification_revision().unwrap(), cleared_revision);
        assert!(store.notification(first.id).is_ok());
        assert!(store.notification(later.id).is_ok());
        assert!(store.clear_notifications(-1).is_err());
        assert!(store.read_notifications(0).is_err());
        assert!(store.clear_notification(999).is_err());
    }

    #[test]
    fn an_abandoned_native_delivery_claim_is_atomically_reclaimed_once() {
        let (mut store, project) = seeded();
        let notice = store.notify_once(notice(project)).unwrap().unwrap();
        assert_eq!(store.claim_notification_delivery(10).unwrap().len(), 1);
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE notification
                        SET delivery_claimed_at = '2000-01-01T00:00:00Z'
                      WHERE id = ?1",
                    params![notice.id],
                )?;
                Ok(())
            })
            .unwrap();

        assert_eq!(store.claim_notification_delivery(10).unwrap().len(), 1);
        assert!(store.claim_notification_delivery(10).unwrap().is_empty());
    }

    #[test]
    fn a_failed_native_delivery_waits_before_retrying() {
        let (mut store, project) = seeded();
        let notice = store.notify_once(notice(project)).unwrap().unwrap();
        assert_eq!(store.claim_notification_delivery(10).unwrap().len(), 1);
        store
            .complete_notification_delivery(notice.id, false)
            .unwrap();
        assert!(store.claim_notification_delivery(10).unwrap().is_empty());
    }

    #[test]
    fn native_delivery_can_only_be_claimed_once() {
        let (mut store, project_id) = seeded();
        store.notify_once(notice(project_id)).unwrap();
        let claimed = store.claim_notification_delivery(10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(store.claim_notification_delivery(10).unwrap().is_empty());
        store
            .complete_notification_delivery(claimed[0].id, true)
            .unwrap();
        assert!(store
            .notification(claimed[0].id)
            .unwrap()
            .delivered_at
            .is_some());
    }
}
