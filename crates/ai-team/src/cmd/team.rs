//! `ait team` - the roster itself.
//!
//! The team is the database (D2), so this is where a team is actually changed. Every
//! command here changes rows, which the next turn reads: a seat is resolved at dispatch,
//! `ait agents generate`, and `ait run` does it before a turn, so an edit never silently
//! half-applies to a project that is mid-run.

use anyhow::{bail, Context, Result};

use ai_team_core::{Guardrails, Store, Team};

use crate::cli::{TeamCommand, TeamEditArgs};
use crate::cmd::project_or_cwd;

pub(crate) fn run(command: TeamCommand) -> Result<()> {
    let path = ai_team_core::default_db_path()?;
    let mut store = Store::open(&path).with_context(|| format!("opening {}", path.display()))?;

    match command {
        TeamCommand::Ls => {
            let teams = store.teams(None)?;
            if teams.is_empty() {
                println!("No teams yet. `ait init` seeds one.");
                return Ok(());
            }
            let mut rows = Vec::new();
            for team in teams {
                rows.push(vec![
                    team.slug.clone(),
                    // A project can own several teams while running exactly one, so
                    // naming the project alone would not say which one does the work.
                    belongs_to(&store, &team)?,
                    store.agents(team.id)?.len().to_string(),
                    team.name.clone(),
                ]);
            }
            crate::cmd::table(&["TEAM", "PROJECT", "SEATS", "NAME"], &rows);
        }

        TeamCommand::Show { team } => {
            let team = resolve(&store, team.as_deref())?;
            show(&store, &team)?;
        }

        TeamCommand::Edit(args) => {
            let team = resolve(&store, args.team.as_deref())?;
            let guardrails = apply_guardrails(team.guardrails, &args)?;
            let name = args.name.unwrap_or_else(|| team.name.clone());
            let description = args.description.unwrap_or_else(|| team.description.clone());

            let updated = store.update_team(team.id, &name, &description, guardrails)?;
            println!("{} updated", updated.slug);
            if updated.slug != team.slug {
                println!("  slug               {} -> {}", team.slug, updated.slug);
            }
            if updated.guardrails != team.guardrails {
                print_guardrails(&updated.guardrails);
            }
            stale_notice();
        }

        TeamCommand::Clone {
            from,
            to,
            name,
            replace,
        } => clone(&mut store, &from, &to, name, replace)?,

        TeamCommand::Rm { team, yes } => remove(&mut store, &team, yes)?,
    }
    Ok(())
}

fn clone(
    store: &mut Store,
    from: &str,
    to: &str,
    name: Option<String>,
    replace: bool,
) -> Result<()> {
    let source = store.find_team_for(from)?;
    let destination = store.find_project(to)?;
    if source.project_id == Some(destination.id) {
        bail!("{} already runs {}", destination.slug, source.slug);
    }
    let previous = match destination.team_id {
        Some(previous) => Some(store.team(previous)?),
        None => None,
    };
    let name = name.unwrap_or_else(|| format!("{} team", destination.name));

    // Replacing deletes first, so the clone can take the name the old team was holding
    // rather than wearing a `-2` suffix that outlives the thing it was avoiding. The
    // destructive step is what --replace asks for, and everything it depends on has
    // already been resolved above.
    if replace {
        if let Some(previous) = &previous {
            store.delete_team(previous.id)?;
        }
    }

    let clone = store.clone_team(source.id, destination.id, &name)?;
    println!(
        "Cloned {} onto {} as {} ({} seats)",
        source.slug,
        destination.slug,
        clone.slug,
        store.agents(clone.id)?.len()
    );

    // The destination almost always had a seeded team already. Saying what happened to
    // it is the difference between a clone and a silent swap.
    //
    // Do not ask whether the previous id differs from the clone's: SQLite hands a
    // deleted rowid straight back to the next insert, so after --replace the new team
    // frequently *is* the old id, and that comparison silently swallows this line.
    if let Some(previous) = previous {
        if replace {
            println!("Deleted the team it was running ({})", previous.slug);
        } else {
            println!(
                "It was running {}, which is now unused - `ait team rm {} --yes` removes it.",
                previous.slug, previous.slug
            );
        }
    }
    println!(
        "The next `ait run -p {}` picks this up - a seat is resolved at dispatch.",
        destination.slug
    );
    Ok(())
}

fn remove(store: &mut Store, needle: &str, yes: bool) -> Result<()> {
    let team = resolve(store, Some(needle))?;
    let seats = store.agents(team.id)?.len();
    if !yes {
        bail!(
            "{} has {seats} seat(s). Pass --yes to delete it.",
            team.slug
        );
    }
    let project = team.project_id;
    store.delete_team(team.id)?;
    println!("Deleted {} and its {seats} seat(s)", team.slug);

    // Deleting the active team leaves the project unable to run. Promoting a leftover
    // automatically would be a surprise, so point at it instead.
    let Some(project) = project else {
        return Ok(());
    };
    let project = store.project(project)?;
    if project.team_id.is_none() {
        match store.teams(Some(project.id))?.first() {
            Some(spare) => println!(
                "{} now runs no team. `ait team clone --from {} --to {}` picks its other \
                 one up.",
                project.slug, spare.slug, project.slug
            ),
            None => println!("{} now runs no team - `ait init` seeds one.", project.slug),
        }
    }
    Ok(())
}

fn resolve(store: &Store, needle: Option<&str>) -> Result<Team> {
    if let Some(needle) = needle {
        return Ok(store.find_team_for(needle)?);
    }
    let project = project_or_cwd(store, None)?;
    crate::cmd::team_of(store, &project)
}

/// Which project a team belongs to, and whether that project actually runs it. A
/// template belongs to none, which is exactly what makes it cloneable.
fn belongs_to(store: &Store, team: &Team) -> Result<String> {
    let Some(project_id) = team.project_id else {
        return Ok("(template)".to_string());
    };
    let project = store.project(project_id)?;
    Ok(if project.team_id == Some(team.id) {
        project.slug
    } else {
        format!("{} (unused)", project.slug)
    })
}

fn show(store: &Store, team: &Team) -> Result<()> {
    let project = belongs_to(store, team)?;
    println!("{} - {}", team.slug, team.name);
    if !team.description.is_empty() {
        println!("{}", team.description);
    }
    println!("\n  project            {project}");
    print_guardrails(&team.guardrails);

    println!();
    let rows: Vec<Vec<String>> = store
        .agents(team.id)?
        .into_iter()
        .map(|agent| {
            let access = match (agent.enabled, agent.read_only) {
                (false, _) => "disabled",
                (true, true) => "read-only",
                (true, false) => "writes",
            };
            let zone = agent
                .zone
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                agent.role.clone(),
                format!("{}/{}", agent.provider, agent.model),
                access.to_string(),
                if zone.is_empty() {
                    "-".to_string()
                } else {
                    zone
                },
            ]
        })
        .collect();
    crate::cmd::table(&["ROLE", "MODEL", "ACCESS", "ZONE"], &rows);
    Ok(())
}

fn print_guardrails(guardrails: &Guardrails) {
    let budget = |value: Option<i64>| match value {
        Some(value) => value.to_string(),
        None => "unlimited".to_string(),
    };
    println!("  parallel width     {}", guardrails.parallel_width);
    println!(
        "  tokens             {} per run, {} per seat",
        budget(guardrails.budget_tokens_run),
        budget(guardrails.budget_tokens_node)
    );
    println!(
        "  seconds            {} per run, {} per seat",
        budget(guardrails.budget_seconds_run),
        budget(guardrails.budget_seconds_node)
    );
    println!("  turns per seat     {}", budget(guardrails.max_turns_node));
    println!(
        "  on failure         {} after {} repair(s)",
        guardrails.on_failure, guardrails.max_repairs
    );
}

/// `0` clears a budget rather than setting one, because a zero-token ceiling would mean
/// a seat that cannot think, which nobody wants and everybody would type by accident.
fn apply_guardrails(mut guardrails: Guardrails, args: &TeamEditArgs) -> Result<Guardrails> {
    let optional = |value: i64| (value > 0).then_some(value);

    if let Some(width) = args.parallel_width {
        if width < 1 {
            bail!("parallel width must be at least 1");
        }
        guardrails.parallel_width = width;
    }
    if let Some(value) = args.budget_tokens_run {
        guardrails.budget_tokens_run = optional(value);
    }
    if let Some(value) = args.budget_tokens_node {
        guardrails.budget_tokens_node = optional(value);
    }
    if let Some(value) = args.budget_seconds_run {
        guardrails.budget_seconds_run = optional(value);
    }
    if let Some(value) = args.budget_seconds_node {
        guardrails.budget_seconds_node = optional(value);
    }
    if let Some(value) = args.max_turns_node {
        guardrails.max_turns_node = optional(value);
    }
    if let Some(value) = args.max_repairs {
        if value < 0 {
            bail!("max repairs cannot be negative");
        }
        guardrails.max_repairs = value;
    }
    if let Some(on_failure) = args.on_failure {
        guardrails.on_failure = on_failure;
    }
    Ok(guardrails)
}

pub(crate) fn stale_notice() {
    println!("`ait run` regenerates and rebuilds before the next turn.");
}
