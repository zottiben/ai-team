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
    let mut store = Store::open(&path).with_context(|| format!("opening {}", path.display()))?;

    match command {
        AgentsCommand::Generate { project, dry_run } => {
            let project = store.find_project(&project)?;
            let team_id = project
                .team_id
                .with_context(|| format!("{} has no team - run `ait init` first", project.slug))?;

            let registry =
                ai_team_core::ModelRegistry::load().context("loading the machine profile")?;
            let root = store.agents_dir(&project.slug)?;
            let generated = store
                .generate_project_for_machine(team_id, &root, &registry)
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

            for resolution in &generated.resolutions {
                if let Some(notice) = resolution.notice() {
                    println!("  fallback           {notice}");
                }
            }

            let provider_keys: Vec<_> = generated
                .required_env
                .iter()
                .copied()
                .filter(|key| key.ends_with("_KEY"))
                .collect();
            if !provider_keys.is_empty() {
                println!("\nThis project needs:");
                for key in provider_keys {
                    let state = if registry.provider_environment(&[key]).is_ok() {
                        "configured"
                    } else {
                        "NOT configured"
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

        AgentsCommand::SetModel {
            project,
            role,
            provider,
            model,
            context_window,
        } => {
            let project = store.find_project(&project)?;
            let team_id = project
                .team_id
                .with_context(|| format!("{} has no team", project.slug))?;
            let agent = store
                .agents(team_id)?
                .into_iter()
                .find(|a| a.role == role)
                .with_context(|| format!("{} has no {role} seat", project.slug))?;

            let before = format!("{}/{}", agent.provider, agent.model);
            let updated = store.set_agent_model(agent.id, provider, &model)?;
            if let Some(tokens) = context_window {
                store.set_agent_context_window(updated.id, Some(tokens))?;
            }
            println!("{role}: {before} -> {provider}/{model}");
            // The generated project is now stale, and the next run regenerates it - say
            // so rather than leaving someone wondering why nothing changed yet.
            println!("`ait run` will regenerate and rebuild before the next turn.");
        }
    }
    Ok(())
}
