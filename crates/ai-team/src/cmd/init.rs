//! `ait init` - create the database, register a project, seed the team of six.
//!
//! A view over `core::register_project`, not a second implementation: the window registers
//! projects too, and two versions of "what does init do" drift.

use anyhow::{Context, Result};

use ai_team_core::{RoleModelDefault, Store};

use crate::cli::InitArgs;

pub(crate) fn run(args: InitArgs) -> Result<()> {
    let (profile_path, profile_created) = ai_team_core::ensure_machine_profile()?;
    if profile_created {
        println!(
            "Created {} (local allowed; account providers denied)",
            profile_path.display()
        );
    }

    let db_path = ai_team_core::default_db_path()?;
    let fresh = !db_path.exists();
    let mut store =
        Store::init(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
    if fresh {
        println!("Created {}", db_path.display());
    }

    let cwd = std::env::current_dir().context("reading the current directory")?;
    let done = ai_team_core::register_project(
        &mut store,
        &cwd,
        args.name.as_deref(),
        args.kind,
        RoleModelDefault::for_this_machine,
    )?;

    if done.created {
        println!("Registered {} ({})", done.project.slug, done.project.kind);
    } else {
        println!("Project {} already registered", done.project.slug);
    }

    match &done.repo_path {
        Some(path) => println!("  repo             {path}"),
        None => println!("  repo             none - this project is not a checkout"),
    }
    if done.seeded_team {
        println!("  team             seeded");
    }
    for (role, model, access) in &done.roster {
        println!("    {role:<14} {model}  {access}");
    }

    println!("\nNext:\n  ait doctor\n  ait db open      # the whole database, in TablePlus");
    Ok(())
}
