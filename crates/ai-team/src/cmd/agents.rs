//! `ait agents` - the generated eve project.
//!
//! The database is the team (D2). This command is the only supported way to produce
//! `.ai-team/agents/`, and it replaces the tree rather than merging with it - a
//! hand-edit that survived would be a second source of truth.

use anyhow::{Context, Result};

use ai_team_core::Store;

use crate::cli::AgentsCommand;

pub(crate) fn run(command: AgentsCommand) -> Result<()> {
    let path = ai_team_core::default_db_path()?;
    let store = Store::open(&path).with_context(|| format!("opening {}", path.display()))?;

    match command {
        AgentsCommand::Generate { project, dry_run } => {
            let project = store.find_project(&project)?;
            let team_id = project
                .team_id
                .with_context(|| format!("{} has no team - run `ait init` first", project.slug))?;

            let root = store.agents_dir(&project.slug)?;
            let generated = store
                .generate_project(team_id, &root)
                .context("generating the eve project")?;

            if dry_run {
                println!("{} files for {}:", generated.files.len(), project.slug);
                for file in &generated.files {
                    println!("  {}", file.path.display());
                }
            } else {
                generated.write()?;
                println!(
                    "Wrote {} files to {}",
                    generated.files.len(),
                    generated.root.display()
                );
            }

            if !generated.required_env.is_empty() {
                println!("\nThis project needs:");
                for key in &generated.required_env {
                    let state = if std::env::var_os(key).is_some() {
                        "set"
                    } else {
                        "NOT set"
                    };
                    println!("  {key:<24} {state}");
                }
            }
            println!(
                "\n`npm install` in that directory, then `eve build`. ai-team supervises \
                 both from M1-S4."
            );
        }

        AgentsCommand::Path { project } => {
            let project = store.find_project(&project)?;
            println!("{}", store.agents_dir(&project.slug)?.display());
        }
    }
    Ok(())
}
