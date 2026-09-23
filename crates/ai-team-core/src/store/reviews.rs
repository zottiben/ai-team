//! Reviews and their comment threads.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::model::{CommentStatus, NewComment, Review, ReviewComment, ReviewStatus};
use crate::store::{non_empty, Store};
use crate::util::now;

impl Store {
    /// Open a review over a node's work. `node_run_id` is what lets a submitted review
    /// steer the node that is still parked, rather than opening a fresh slice (M3-S15).
    pub fn open_review(
        &mut self,
        project_id: i64,
        title: &str,
        run_id: Option<i64>,
        node_run_id: Option<i64>,
        branch: Option<&str>,
    ) -> Result<Review> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(Error::invalid("a review needs a title"));
        }
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO review (project_id, run_id, node_run_id, title, branch,
                                     created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![project_id, run_id, node_run_id, title, branch, at],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.review(id)
    }

    /// The review a pull request already has, opened again for another look.
    ///
    /// A PR is built again after review - a follow-up, a steered seat - and each time it
    /// used to get a review of its own, so one PR piled up a review per round. One PR has
    /// one review: when it lands again, the latest one still in play for its branch goes
    /// back to open and points at the turn that just finished. Comments already sent with a
    /// submission are marked outdated - still readable, but the code under them has moved
    /// and submitting again must not send them twice. A review somebody approved or
    /// dismissed is finished; `None` then, and the caller opens a new one.
    pub fn reopen_review(
        &mut self,
        project_id: i64,
        branch: &str,
        run_id: Option<i64>,
        node_run_id: Option<i64>,
    ) -> Result<Option<Review>> {
        let at = now();
        let reopened = self.db_mut().write(|tx| {
            let found: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT id, submitted_at FROM review
                      WHERE project_id = ?1 AND branch = ?2
                        AND status IN ('open', 'changes_requested')
                      ORDER BY id DESC LIMIT 1",
                    params![project_id, branch],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((id, submitted_at)) = found else {
                return Ok(None);
            };
            if let Some(submitted_at) = submitted_at {
                tx.execute(
                    "UPDATE review_comment SET status = 'outdated'
                      WHERE review_id = ?1 AND status = 'open' AND created_at <= ?2",
                    params![id, submitted_at],
                )?;
            }
            tx.execute(
                "UPDATE review
                    SET status = 'open', submitted_at = NULL,
                        run_id = COALESCE(?2, run_id), node_run_id = COALESCE(?3, node_run_id),
                        rev = rev + 1, updated_at = ?4
                  WHERE id = ?1",
                params![id, run_id, node_run_id, at],
            )?;
            Ok(Some(id))
        })?;
        reopened.map(|id| self.review(id)).transpose()
    }

    pub fn review(&self, id: i64) -> Result<Review> {
        self.db()
            .conn()
            .query_row(
                &format!("{REVIEW_SELECT} WHERE id = ?1"),
                params![id],
                review_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchReview(id.to_string()))
    }

    pub fn reviews(&self, project_id: Option<i64>, open_only: bool) -> Result<Vec<Review>> {
        let mut clauses = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(id) = project_id {
            clauses.push("project_id = ?".to_string());
            args.push(Box::new(id));
        }
        if open_only {
            clauses.push("status IN ('open','changes_requested')".to_string());
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        let mut stmt = self
            .db()
            .conn()
            .prepare(&format!("{REVIEW_SELECT}{where_sql} ORDER BY id DESC"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), review_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn set_review_range(&mut self, id: i64, base_sha: &str, head_sha: &str) -> Result<Review> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE review SET base_sha = ?2, head_sha = ?3, rev = rev + 1, updated_at = ?4
                  WHERE id = ?1",
                params![id, base_sha, head_sha, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchReview(id.to_string()));
            }
            Ok(())
        })?;
        self.review(id)
    }

    pub fn comment(&mut self, review_id: i64, new: NewComment) -> Result<ReviewComment> {
        let body = new.body.trim().to_string();
        if body.is_empty() {
            return Err(Error::invalid("a comment needs a body"));
        }
        // A range that runs backwards would render as an empty hunk and silently
        // swallow the comment.
        if let (Some(start), Some(end)) = (new.line_start, new.line_end) {
            if end < start {
                return Err(Error::invalid(format!(
                    "comment range {start}..{end} runs backwards"
                )));
            }
        }
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO review_comment
                   (review_id, parent_id, file_path, side, line_start, line_end, author, body,
                    created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    review_id,
                    new.parent_id,
                    new.file_path,
                    new.side,
                    new.line_start,
                    new.line_end,
                    new.author,
                    body,
                    at
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.review_comment(id)
    }

    pub fn review_comment(&self, id: i64) -> Result<ReviewComment> {
        self.db()
            .conn()
            .query_row(
                &format!("{COMMENT_SELECT} WHERE id = ?1"),
                params![id],
                comment_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid(format!("no comment {id}")))
    }

    pub fn comments(&self, review_id: i64) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{COMMENT_SELECT} WHERE review_id = ?1 ORDER BY file_path, line_start, id"
        ))?;
        let rows = stmt
            .query_map(params![review_id], comment_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn resolve_comment(&mut self, id: i64, status: CommentStatus) -> Result<ReviewComment> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE review_comment
                    SET status = ?2,
                        resolved_at = CASE WHEN ?2 = 'open' THEN NULL ELSE ?3 END
                  WHERE id = ?1",
                params![id, status, at],
            )?;
            if changed == 0 {
                return Err(Error::invalid(format!("no comment {id}")));
            }
            Ok(())
        })?;
        self.review_comment(id)
    }

    /// Submit the review. Approving with unresolved comments is refused: the two states
    /// contradict each other, and the node reading this would not know which to believe.
    pub fn submit_review(&mut self, id: i64, status: ReviewStatus) -> Result<Review> {
        if status == ReviewStatus::Approved {
            let unresolved = self.unresolved_count(id)?;
            if unresolved > 0 {
                return Err(Error::invalid(format!(
                    "review {id} has {unresolved} unresolved comment(s) - resolve them or request changes"
                )));
            }
        }
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE review SET status = ?2, submitted_at = ?3, rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![id, status, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchReview(id.to_string()));
            }
            Ok(())
        })?;
        self.review(id)
    }

    pub fn unresolved_count(&self, review_id: i64) -> Result<i64> {
        Ok(self.db().conn().query_row(
            "SELECT COUNT(*) FROM review_comment WHERE review_id = ?1 AND status = 'open'",
            params![review_id],
            |r| r.get(0),
        )?)
    }
}

const REVIEW_SELECT: &str = "SELECT id, project_id, run_id, node_run_id, title, status, branch, \
     base_sha, head_sha, submitted_at, rev, created_at, updated_at FROM review";

fn review_from_row(r: &Row<'_>) -> rusqlite::Result<Review> {
    Ok(Review {
        id: r.get(0)?,
        project_id: r.get(1)?,
        run_id: r.get(2)?,
        node_run_id: r.get(3)?,
        title: r.get(4)?,
        status: r.get(5)?,
        branch: non_empty(r.get(6)?),
        base_sha: non_empty(r.get(7)?),
        head_sha: non_empty(r.get(8)?),
        submitted_at: non_empty(r.get(9)?),
        rev: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

const COMMENT_SELECT: &str = "SELECT id, review_id, parent_id, file_path, side, line_start, \
     line_end, author, body, status, created_at, resolved_at FROM review_comment";

fn comment_from_row(r: &Row<'_>) -> rusqlite::Result<ReviewComment> {
    Ok(ReviewComment {
        id: r.get(0)?,
        review_id: r.get(1)?,
        parent_id: r.get(2)?,
        file_path: non_empty(r.get(3)?),
        side: r.get(4)?,
        line_start: r.get(5)?,
        line_end: r.get(6)?,
        author: r.get(7)?,
        body: r.get(8)?,
        status: r.get(9)?,
        created_at: r.get(10)?,
        resolved_at: non_empty(r.get(11)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiffSide, NewProject, RunTrigger};

    fn reviewable() -> (Store, i64, i64) {
        let mut s = Store::memory().unwrap();
        let p = s
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = s.seed_default_team(p.id).unwrap();
        let run = s.create_run(p.id, "ship it", RunTrigger::Manual).unwrap();
        let backend = s
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        let node = s
            .dispatch(
                run.id,
                backend.id,
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        (s, p.id, node.id)
    }

    fn comment_on(line: i64) -> NewComment {
        NewComment {
            parent_id: None,
            file_path: Some("crates/ai-team-core/src/db.rs".into()),
            side: Some(DiffSide::New),
            line_start: Some(line),
            line_end: Some(line),
            author: "human".into(),
            body: format!("this on line {line}?"),
        }
    }

    #[test]
    fn a_review_remembers_which_node_produced_it() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(
                project,
                "PR1: the store",
                None,
                Some(node),
                Some("slice/PR1"),
            )
            .unwrap();
        assert_eq!(review.node_run_id, Some(node));
        assert_eq!(review.status, ReviewStatus::Open);
    }

    #[test]
    fn a_pr_built_again_reopens_its_review_rather_than_piling_up_another() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), Some("csv/pr1"))
            .unwrap();
        let sent = s.comment(review.id, comment_on(7)).unwrap();
        s.submit_review(review.id, ReviewStatus::ChangesRequested)
            .unwrap();

        // The follow-up lands again on the same branch.
        let reopened = s
            .reopen_review(project, "csv/pr1", None, Some(node))
            .unwrap()
            .expect("the PR's review is still in play");
        assert_eq!(reopened.id, review.id);
        assert_eq!(reopened.status, ReviewStatus::Open);
        assert!(reopened.submitted_at.is_none());
        // Already sent, and the code under it has moved: kept, but not sent again.
        let comments = s.comments(review.id).unwrap();
        assert_eq!(comments[0].id, sent.id);
        assert_eq!(comments[0].status, CommentStatus::Outdated);
        assert_eq!(s.unresolved_count(review.id).unwrap(), 0);

        // Not submitted since: a comment written meanwhile is still to be sent.
        s.comment(review.id, comment_on(9)).unwrap();
        s.reopen_review(project, "csv/pr1", None, Some(node))
            .unwrap()
            .unwrap();
        assert_eq!(s.unresolved_count(review.id).unwrap(), 1);

        // A review somebody finished is not reopened; the PR gets a fresh one.
        s.resolve_comment(
            s.comments(review.id).unwrap()[1].id,
            CommentStatus::Resolved,
        )
        .unwrap();
        s.submit_review(review.id, ReviewStatus::Approved).unwrap();
        assert!(s
            .reopen_review(project, "csv/pr1", None, Some(node))
            .unwrap()
            .is_none());
        // And another branch's review is none of this one's business.
        assert!(s
            .reopen_review(project, "csv/pr2", None, Some(node))
            .unwrap()
            .is_none());
    }

    #[test]
    fn approving_with_unresolved_comments_is_refused() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();
        s.comment(review.id, comment_on(42)).unwrap();

        let approved = s.submit_review(review.id, ReviewStatus::Approved);
        assert!(
            approved.is_err(),
            "an approval that contradicts its comments"
        );

        // Requesting changes is always allowed - that is what the comments mean.
        let changes = s
            .submit_review(review.id, ReviewStatus::ChangesRequested)
            .unwrap();
        assert_eq!(changes.status, ReviewStatus::ChangesRequested);
        assert!(changes.submitted_at.is_some());
    }

    #[test]
    fn resolving_every_comment_unblocks_the_approval() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();
        let a = s.comment(review.id, comment_on(10)).unwrap();
        let b = s.comment(review.id, comment_on(20)).unwrap();

        assert_eq!(s.unresolved_count(review.id).unwrap(), 2);
        s.resolve_comment(a.id, CommentStatus::Resolved).unwrap();
        // "outdated" also counts as dealt with: the line it pointed at is gone.
        let b = s.resolve_comment(b.id, CommentStatus::Outdated).unwrap();
        assert!(b.resolved_at.is_some());

        assert_eq!(s.unresolved_count(review.id).unwrap(), 0);
        assert_eq!(
            s.submit_review(review.id, ReviewStatus::Approved)
                .unwrap()
                .status,
            ReviewStatus::Approved
        );
    }

    #[test]
    fn reopening_a_comment_clears_its_resolution_time() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();
        let c = s.comment(review.id, comment_on(1)).unwrap();

        let resolved = s.resolve_comment(c.id, CommentStatus::Resolved).unwrap();
        assert!(resolved.resolved_at.is_some());
        let reopened = s.resolve_comment(c.id, CommentStatus::Open).unwrap();
        assert!(reopened.resolved_at.is_none());
    }

    #[test]
    fn a_backwards_range_is_refused() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();
        let mut bad = comment_on(10);
        bad.line_end = Some(4);
        assert!(s.comment(review.id, bad).is_err());
    }

    #[test]
    fn a_thread_hangs_off_its_parent_and_dies_with_it() {
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();
        let root = s.comment(review.id, comment_on(7)).unwrap();

        let mut reply = comment_on(7);
        reply.parent_id = Some(root.id);
        reply.author = "backend".into();
        reply.body = "fixed".into();
        s.comment(review.id, reply).unwrap();
        assert_eq!(s.comments(review.id).unwrap().len(), 2);

        s.db_mut()
            .write(|tx| {
                tx.execute("DELETE FROM review_comment WHERE id = ?1", params![root.id])?;
                Ok(())
            })
            .unwrap();
        assert!(
            s.comments(review.id).unwrap().is_empty(),
            "the reply went with it"
        );
    }

    #[test]
    fn the_same_line_on_each_side_is_two_different_comments() {
        // Without `side`, a comment on a removed line and one on an added line at the
        // same number are indistinguishable, and a rebase lands them in the wrong place.
        let (mut s, project, node) = reviewable();
        let review = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();

        let mut old = comment_on(12);
        old.side = Some(DiffSide::Old);
        old.body = "why was this removed?".into();
        s.comment(review.id, old).unwrap();
        s.comment(review.id, comment_on(12)).unwrap();

        let comments = s.comments(review.id).unwrap();
        assert_eq!(comments.len(), 2);
        let sides: Vec<_> = comments.iter().filter_map(|c| c.side).collect();
        assert!(sides.contains(&DiffSide::Old) && sides.contains(&DiffSide::New));
    }

    #[test]
    fn open_reviews_are_the_ones_still_wanting_something() {
        let (mut s, project, node) = reviewable();
        let a = s
            .open_review(project, "PR1", None, Some(node), None)
            .unwrap();
        let b = s
            .open_review(project, "PR2", None, Some(node), None)
            .unwrap();
        s.submit_review(a.id, ReviewStatus::Approved).unwrap();

        let open = s.reviews(Some(project), true).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, b.id);
        assert_eq!(s.reviews(Some(project), false).unwrap().len(), 2);
    }
}
