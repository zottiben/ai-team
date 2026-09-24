//! `ait ingest` - a ticket becomes a project.
//!
//! What this does *not* do is read ClickUp. ai-team has no ClickUp credentials of its
//! own: the seats do, through their read-only connections (D9), and a seat reading the
//! ticket it is working on is the same seat that will act on it. This records what the
//! URL already says, so there is something to run against, and leaves the rest to them.

use anyhow::{Context, Result};

use ai_team_core::{
    figma_links, parse_url, NewProject, ProjectKind, ProjectSource, RoleModelDefault, Source, Store,
};

use crate::cli::IngestArgs;

pub(crate) fn run(args: IngestArgs) -> Result<()> {
    let source = parse_url(&args.url)?;
    let db = ai_team_core::default_db_path()?;
    let mut store = Store::init(&db).with_context(|| format!("opening {}", db.display()))?;

    let brief = match &args.brief_file {
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?,
        None => String::new(),
    };

    // A ticket is one piece of work; a list is a container of them. That is exactly the
    // difference D6 draws between a ticket and an epic.
    let kind = match &source {
        Source::ClickUpTask { .. } => ProjectKind::Ticket,
        Source::ClickUpList { .. } => ProjectKind::Epic,
        // A design on its own is not a ticket. It is somewhere to hang work from.
        Source::Figma { .. } => ProjectKind::Adhoc,
    };
    let name = args.name.unwrap_or_else(|| default_name(&source));

    let project = store.create_project(NewProject {
        name,
        kind: Some(kind),
        brief_md: brief.clone(),
        source: Some(match source.kind() {
            "clickup" => ProjectSource::ClickUp,
            _ => ProjectSource::Figma,
        }),
        source_key: Some(source.key()),
        source_url: Some(args.url.clone()),
        ..Default::default()
    })?;
    let team = store.seed_default_team(project.id, &RoleModelDefault::for_this_machine())?;

    println!("{} ({}) from {}", project.slug, project.kind, source.key());
    println!(
        "  team {} with {} seats",
        team.slug,
        store.agents(team.id)?.len()
    );

    // The designs a ticket points at are the context the UI seat needs, and they arrive
    // as links in prose rather than as a field.
    let designs = figma_links(&brief);
    if designs.is_empty() {
        if args.brief_file.is_some() {
            println!("  no Figma links in the brief");
        }
    } else {
        println!("  {} design(s) referenced:", designs.len());
        for design in &designs {
            println!("    {}", design.key());
        }
    }

    println!(
        "\nThe ticket itself is read by the seats, not by ai-team: `ait doctor` says \
         whether this machine allows ClickUp and Figma at all."
    );
    Ok(())
}

/// Something to call it until a seat reads the real title.
fn default_name(source: &Source) -> String {
    match source {
        Source::ClickUpTask { id } => format!("ticket {id}"),
        Source::ClickUpList { id } => format!("epic {id}"),
        Source::Figma { key, .. } => format!("design {key}"),
    }
}
