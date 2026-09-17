//! Projects and the repos they touch.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::model::{NewProject, NewRepo, Project, ProjectKind, ProjectRepo, ProjectStatus};
use crate::store::{non_empty, Store};
use crate::util::{normalise_remote, now, slugify};

impl Store {
    pub fn create_project(&mut self, new: NewProject) -> Result<Project> {
        let name = new.name.trim().to_string();
        if name.is_empty() {
            return Err(Error::invalid("a project needs a name"));
        }
        let slug = match new.slug.as_deref().map(slugify) {
            Some(s) if !s.is_empty() => s,
            _ => slugify(&name),
        };
        if slug.is_empty() {
            return Err(Error::invalid(format!(
                "{name:?} does not reduce to a usable slug - pass one explicitly"
            )));
        }
        let at = now();
        let kind = new.kind.unwrap_or(ProjectKind::Repo);

        let id = self.db_mut().write(|tx| {
            let taken: bool = tx
                .query_row(
                    "SELECT 1 FROM project WHERE slug = ?1",
                    params![slug],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if taken {
                return Err(Error::DuplicateProject(slug.clone()));
            }
            tx.execute(
                "INSERT INTO project
                   (slug, name, kind, summary, brief_md, source, source_key, source_url,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    slug,
                    name,
                    kind,
                    new.summary,
                    new.brief_md,
                    new.source,
                    new.source_key,
                    new.source_url,
                    at
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.project(id)
    }

    pub fn project(&self, id: i64) -> Result<Project> {
        self.db()
            .conn()
            .query_row(
                &format!("{PROJECT_SELECT} WHERE id = ?1"),
                params![id],
                project_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchProject(id.to_string()))
    }

    /// Resolve by slug, then by id, then by a unique substring of the name. The last one
    /// is what makes `ait run "widget"` work; ambiguity is an error rather than a guess,
    /// because guessing here dispatches agents at the wrong repo.
    pub fn find_project(&self, needle: &str) -> Result<Project> {
        let needle = needle.trim();
        if needle.is_empty() {
            return Err(Error::invalid("which project?"));
        }
        if let Some(found) = self
            .db()
            .conn()
            .query_row(
                &format!("{PROJECT_SELECT} WHERE slug = ?1"),
                params![needle],
                project_from_row,
            )
            .optional()?
        {
            return Ok(found);
        }
        if let Ok(id) = needle.parse::<i64>() {
            if let Ok(found) = self.project(id) {
                return Ok(found);
            }
        }

        let pattern = format!("%{needle}%");
        let mut stmt = self.db().conn().prepare(&format!(
            "{PROJECT_SELECT} WHERE name LIKE ?1 OR slug LIKE ?1"
        ))?;
        let matches: Vec<Project> = stmt
            .query_map(params![pattern], project_from_row)?
            .collect::<rusqlite::Result<_>>()?;

        match matches.len() {
            0 => Err(Error::NoSuchProject(needle.to_string())),
            1 => Ok(matches.into_iter().next().expect("exactly one")),
            n => {
                let names: Vec<&str> = matches.iter().map(|p| p.slug.as_str()).collect();
                Err(Error::AmbiguousProject(
                    needle.to_string(),
                    n,
                    names.join(", "),
                ))
            }
        }
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .db()
            .conn()
            .prepare(&format!("{PROJECT_SELECT} ORDER BY updated_at DESC"))?;
        let rows = stmt
            .query_map([], project_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn set_project_status(&mut self, id: i64, status: ProjectStatus) -> Result<Project> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE project SET status = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![id, status, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchProject(id.to_string()));
            }
            Ok(())
        })?;
        self.project(id)
    }

    /// Point a project at the team that runs it. Separate from creation because the
    /// project has to exist before a team can reference it back.
    pub fn set_project_team(&mut self, project_id: i64, team_id: Option<i64>) -> Result<Project> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE project SET team_id = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![project_id, team_id, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchProject(project_id.to_string()));
            }
            Ok(())
        })?;
        self.project(project_id)
    }

    /// Attach a repo, or update the one already attached under the same normalised key.
    /// Idempotent on purpose: re-running `ait init` in a worktree must not accumulate
    /// duplicate rows for one repo.
    pub fn attach_repo(&mut self, project_id: i64, new: NewRepo) -> Result<ProjectRepo> {
        let remote = new
            .remote_url
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        let path = new.main_path.clone();
        let key = if remote.is_empty() {
            // A repo with no remote is still a repo. Key it by its path so two local-only
            // checkouts do not collide.
            match path.as_deref() {
                Some(p) if !p.trim().is_empty() => format!("local:{}", p.trim()),
                _ => return Err(Error::invalid("a repo needs a remote URL or a path")),
            }
        } else {
            normalise_remote(&remote)
        };
        let name = match new.name.as_deref().map(str::trim) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => key.rsplit('/').next().unwrap_or(&key).to_string(),
        };
        let at = now();

        let id = self.db_mut().write(|tx| {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM project_repo WHERE project_id = ?1 AND key = ?2",
                    params![project_id, key],
                    |r| r.get(0),
                )
                .optional()?;

            if let Some(id) = existing {
                tx.execute(
                    "UPDATE project_repo
                        SET name = ?2, remote_url = ?3, main_path = ?4, default_branch = ?5
                      WHERE id = ?1",
                    params![id, name, new.remote_url, new.main_path, new.default_branch],
                )?;
                return Ok(id);
            }

            let ord: i64 = tx.query_row(
                "SELECT COALESCE(MAX(ord) + 1, 0) FROM project_repo WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO project_repo
                   (project_id, ord, key, name, remote_url, main_path, default_branch, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    project_id,
                    ord,
                    key,
                    name,
                    new.remote_url,
                    new.main_path,
                    new.default_branch,
                    at
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })?;

        self.db()
            .conn()
            .query_row(
                "SELECT id, project_id, ord, key, name, remote_url, main_path, default_branch,
                        created_at
                   FROM project_repo WHERE id = ?1",
                params![id],
                |r| {
                    Ok(ProjectRepo {
                        id: r.get(0)?,
                        project_id: r.get(1)?,
                        ord: r.get(2)?,
                        key: r.get(3)?,
                        name: r.get(4)?,
                        remote_url: non_empty(r.get(5)?),
                        main_path: non_empty(r.get(6)?),
                        default_branch: non_empty(r.get(7)?),
                        created_at: r.get(8)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn project_repos(&self, project_id: i64) -> Result<Vec<ProjectRepo>> {
        let mut stmt = self.db().conn().prepare(
            "SELECT id, project_id, ord, key, name, remote_url, main_path, default_branch,
                    created_at
               FROM project_repo WHERE project_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(ProjectRepo {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    ord: r.get(2)?,
                    key: r.get(3)?,
                    name: r.get(4)?,
                    remote_url: non_empty(r.get(5)?),
                    main_path: non_empty(r.get(6)?),
                    default_branch: non_empty(r.get(7)?),
                    created_at: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn delete_project(&mut self, id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            let changed = tx.execute("DELETE FROM project WHERE id = ?1", params![id])?;
            if changed == 0 {
                return Err(Error::NoSuchProject(id.to_string()));
            }
            Ok(())
        })
    }
}

/// The column order here is exactly the order [`project_from_row`] reads by index, so
/// the two live next to each other. One list, one reader - adding a column to a second
/// copy and not the first is a silent misread rather than a compile error.
const PROJECT_SELECT: &str = "SELECT id, slug, name, kind, status, summary, brief_md, source, \
     source_key, source_url, team_id, rev, created_at, updated_at FROM project";

fn project_from_row(r: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        slug: r.get(1)?,
        name: r.get(2)?,
        kind: r.get(3)?,
        status: r.get(4)?,
        summary: non_empty(r.get(5)?),
        brief_md: r.get(6)?,
        source: r.get(7)?,
        source_key: non_empty(r.get(8)?),
        source_url: non_empty(r.get(9)?),
        team_id: r.get(10)?,
        rev: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::memory().unwrap()
    }

    #[test]
    fn a_project_gets_a_slug_from_its_name() {
        let mut s = store();
        let p = s
            .create_project(NewProject {
                name: "ACME-1234 - Reusable Date Range Picker".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.slug, "acme-1234-reusable-date-range-picker");
        assert_eq!(p.kind, ProjectKind::Repo);
        assert_eq!(p.status, ProjectStatus::Active);
        assert!(p.team_id.is_none());
    }

    #[test]
    fn a_duplicate_slug_is_refused_rather_than_silently_suffixed() {
        let mut s = store();
        s.create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
        let again = s.create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        });
        assert!(matches!(again, Err(Error::DuplicateProject(_))));
    }

    #[test]
    fn a_name_that_reduces_to_nothing_is_an_error() {
        let mut s = store();
        assert!(s
            .create_project(NewProject {
                name: "...".into(),
                ..Default::default()
            })
            .is_err());
        assert!(s
            .create_project(NewProject {
                name: "   ".into(),
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn finding_is_slug_then_id_then_a_unique_substring() {
        let mut s = store();
        let widget = s
            .create_project(NewProject {
                name: "Widget Service".into(),
                ..Default::default()
            })
            .unwrap();
        s.create_project(NewProject {
            name: "Gadget Service".into(),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(s.find_project("widget-service").unwrap().id, widget.id);
        assert_eq!(
            s.find_project(&widget.id.to_string()).unwrap().id,
            widget.id
        );
        assert_eq!(s.find_project("Widget").unwrap().id, widget.id);

        // Two projects match "Service", and picking one would dispatch agents at the
        // wrong repo.
        let err = s.find_project("Service").unwrap_err();
        assert!(matches!(err, Error::AmbiguousProject(_, 2, _)), "{err}");
        assert!(matches!(
            s.find_project("nope"),
            Err(Error::NoSuchProject(_))
        ));
    }

    #[test]
    fn attaching_the_same_repo_twice_updates_rather_than_duplicates() {
        let mut s = store();
        let p = s
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();

        let first = s
            .attach_repo(
                p.id,
                NewRepo {
                    remote_url: Some("git@github.com:acme/widget.git".into()),
                    main_path: Some("/home/me/widget".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(first.key, "github.com/acme/widget");
        assert_eq!(first.name, "widget");

        // The same repo over https is the same repo.
        let second = s
            .attach_repo(
                p.id,
                NewRepo {
                    remote_url: Some("https://github.com/acme/widget".into()),
                    main_path: Some("/home/me/widget2".into()),
                    default_branch: Some("main".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(second.id, first.id, "one repo, one row");
        assert_eq!(second.main_path.as_deref(), Some("/home/me/widget2"));
        assert_eq!(s.project_repos(p.id).unwrap().len(), 1);
    }

    #[test]
    fn a_project_may_have_no_repos_at_all() {
        // D6: a triage session or a ClickUp epic is a project with nothing checked out.
        let mut s = store();
        let p = s
            .create_project(NewProject {
                name: "Friday triage".into(),
                kind: Some(ProjectKind::Triage),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.kind, ProjectKind::Triage);
        assert!(s.project_repos(p.id).unwrap().is_empty());
    }

    #[test]
    fn a_local_only_repo_is_keyed_by_its_path() {
        let mut s = store();
        let p = s
            .create_project(NewProject {
                name: "Scratch".into(),
                ..Default::default()
            })
            .unwrap();
        let repo = s
            .attach_repo(
                p.id,
                NewRepo {
                    main_path: Some("/tmp/scratch".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(repo.key, "local:/tmp/scratch");
        assert!(
            s.attach_repo(p.id, NewRepo::default()).is_err(),
            "a repo with neither remote nor path is not a repo"
        );
    }
}
