pub(crate) mod agents;
pub(crate) mod db;
pub(crate) mod doctor;
pub(crate) mod init;
pub(crate) mod run;
pub(crate) mod team;
pub(crate) mod ui;

use anyhow::{Context, Result};

use ai_team_core::{Project, Store, Team};

/// Print a header and rows with every column sized to its widest cell.
///
/// Fixed widths look right until the first long role name shunts every later column out
/// of alignment, and a roster is exactly where long names turn up.
pub(crate) fn table(header: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = header
        .iter()
        .enumerate()
        .map(|(column, title)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .chain(std::iter::once(title.chars().count()))
                .max()
                .unwrap_or_default()
        })
        .collect();

    let line = |cells: &[String]| {
        let mut out = String::new();
        for (column, cell) in cells.iter().enumerate() {
            // The last column is never padded: trailing spaces are invisible until they
            // show up in a diff or a copied line.
            if column + 1 == cells.len() {
                out.push_str(cell);
            } else {
                let pad = widths[column].saturating_sub(cell.chars().count());
                out.push_str(cell);
                out.push_str(&" ".repeat(pad + 2));
            }
        }
        println!("{}", out.trim_end());
    };

    line(
        &header
            .iter()
            .map(|title| (*title).to_string())
            .collect::<Vec<_>>(),
    );
    for row in rows {
        line(row);
    }
}

/// Which project a command means when `--project` was left off.
///
/// The checkout you are standing in decides, because that is the one fact the shell
/// already carries and it survives a project being renamed. Falling back to "the only
/// project" would be friendlier on a fresh install and wrong on every machine with two.
pub(crate) fn project_or_cwd(store: &Store, project: Option<&str>) -> Result<Project> {
    if let Some(needle) = project {
        return Ok(store.find_project(needle)?);
    }
    let cwd = std::env::current_dir().context("reading the current directory")?;
    if let Some(project) = store.project_at(&cwd)? {
        return Ok(project);
    }
    // A project with no repo attached (D6 allows that) is still findable by the name of
    // the directory it was created in, which is what `ait init` defaults to.
    let name = cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    store
        .find_project(&ai_team_core::slugify(&name))
        .with_context(|| {
            format!(
                "no project is registered for {} - pass --project, or run `ait init` here",
                cwd.display()
            )
        })
}

/// The team a project is running, with the error a human can act on.
pub(crate) fn team_of(store: &Store, project: &Project) -> Result<Team> {
    let team_id = project.team_id.with_context(|| {
        format!(
            "{} has no team - `ait init` seeds one, or `ait team clone --to {}`",
            project.slug, project.slug
        )
    })?;
    Ok(store.team(team_id)?)
}
