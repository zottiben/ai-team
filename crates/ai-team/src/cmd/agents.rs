//! `ait agents` - the generated eve project.
//!
//! The database is the team (D2). This command is the only supported way to produce
//! `.ai-team/agents/`, and it replaces the tree rather than merging with it - a
//! hand-edit that survived would be a second source of truth.

use anyhow::{bail, Context, Result};

use ai_team_core::{Agent, NewAgent, Provider, Reasoning, Store, ToolEffect};

use crate::cli::{AgentAddArgs, AgentEditArgs, AgentToolsArgs, AgentsCommand};
use crate::cmd::{project_or_cwd, team_of};

pub(crate) fn run(command: AgentsCommand) -> Result<()> {
    let path = ai_team_core::default_db_path()?;
    let mut store = Store::open(&path).with_context(|| format!("opening {}", path.display()))?;

    match command {
        AgentsCommand::Ls { project } => {
            let project = project_or_cwd(&store, project.as_deref())?;
            let team = team_of(&store, &project)?;
            let rows: Vec<Vec<String>> = store
                .agents(team.id)?
                .into_iter()
                .map(|agent| {
                    vec![
                        agent.role.clone(),
                        format!("{}/{}", agent.provider, agent.model),
                        access(&agent).to_string(),
                        agent.reasoning.to_string(),
                        agent.name.clone(),
                    ]
                })
                .collect();
            crate::cmd::table(&["ROLE", "MODEL", "ACCESS", "EFFORT", "NAME"], &rows);
        }

        AgentsCommand::Show { project, role } => show(&store, project.as_deref(), &role)?,

        AgentsCommand::Add(args) => add(&mut store, args)?,
        AgentsCommand::Edit(args) => edit(&mut store, args)?,

        AgentsCommand::Rm { project, role, yes } => {
            let agent = seat(&store, project.as_deref(), &role)?;
            if !yes {
                bail!("pass --yes to remove the {role} seat");
            }
            store.delete_agent(agent.id)?;
            println!("Removed {role}");
            crate::cmd::team::stale_notice();
        }

        AgentsCommand::Disable { project, role } => {
            let agent = seat(&store, project.as_deref(), &role)?;
            store.set_agent_enabled(agent.id, false)?;
            println!("{role} disabled - it is no longer generated or dispatched");
            crate::cmd::team::stale_notice();
        }

        AgentsCommand::Enable { project, role } => {
            let agent = seat(&store, project.as_deref(), &role)?;
            store.set_agent_enabled(agent.id, true)?;
            println!("{role} enabled");
            crate::cmd::team::stale_notice();
        }

        AgentsCommand::Tools(args) => tools(&mut store, args)?,

        AgentsCommand::Generate { project, dry_run } => {
            generate(&store, &project, dry_run)?;
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
            let agent = seat(&store, Some(&project), &role)?;
            let before = format!("{}/{}", agent.provider, agent.model);
            let updated = store.set_agent_model(agent.id, provider, &model)?;
            if let Some(tokens) = context_window {
                store.set_agent_context_window(updated.id, Some(tokens))?;
            }
            println!("{role}: {before} -> {provider}/{model}");
            // The generated project is now stale, and the next run regenerates it - say
            // so rather than leaving someone wondering why nothing changed yet.
            crate::cmd::team::stale_notice();
        }
    }
    Ok(())
}

fn generate(store: &Store, project: &str, dry_run: bool) -> Result<()> {
    let project = store.find_project(project)?;
    let team_id = project
        .team_id
        .with_context(|| format!("{} has no team - run `ait init` first", project.slug))?;

    let registry = ai_team_core::ModelRegistry::load().context("loading the machine profile")?;
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
    Ok(())
}

fn access(agent: &Agent) -> &'static str {
    match (agent.enabled, agent.read_only) {
        (false, _) => "disabled",
        (true, true) => "read-only",
        (true, false) => "writes",
    }
}

fn show(store: &Store, project: Option<&str>, role: &str) -> Result<()> {
    let agent = seat(store, project, role)?;
    println!("{} - {}", agent.role, agent.name);
    println!("\n{}\n", agent.purpose);
    println!("  model              {}/{}", agent.provider, agent.model);
    println!(
        "  context window     {}",
        agent.context_window.map_or_else(
            || "provider default".to_string(),
            |tokens| format!("{tokens} tokens")
        )
    );
    println!("  reasoning          {}", agent.reasoning);
    println!("  access             {}", access(&agent));
    println!(
        "  prompt             {}",
        match (&agent.prompt_preset, &agent.prompt_md) {
            (Some(preset), _) => format!("preset {preset}"),
            (None, Some(prompt)) => format!("custom, {} chars", prompt.len()),
            (None, None) => "none".to_string(),
        }
    );
    let zone: Vec<_> = agent
        .zone
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    println!(
        "  owns               {}",
        if zone.is_empty() {
            "nothing - the orchestrator decides".to_string()
        } else {
            zone.join(", ")
        }
    );

    let policies = store.tool_policies(agent.id)?;
    if policies.is_empty() {
        println!("  tool rules         none");
    } else {
        println!("  tool rules");
        for policy in policies {
            println!(
                "    {:<8} {:<20} {}",
                policy.effect,
                policy.tool,
                policy.note.unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn seat(store: &Store, project: Option<&str>, role: &str) -> Result<Agent> {
    let project = project_or_cwd(store, project)?;
    let team = team_of(store, &project)?;
    let agents = store.agents(team.id)?;
    agents
        .iter()
        .find(|agent| agent.role == role)
        .cloned()
        .with_context(|| {
            format!(
                "{} has no {role} seat. It has: {}",
                project.slug,
                agents
                    .iter()
                    .map(|agent| agent.role.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Repeated `--zone` values become the newline-separated column the matcher reads. One
/// empty value is how a zone is cleared, since clap cannot tell "absent" from "empty".
fn zone_of(values: &[String]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_prompt(path: &str) -> Result<String> {
    let prompt = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    if prompt.trim().is_empty() {
        bail!("{path} is empty - a custom prompt has to say something");
    }
    Ok(prompt)
}

fn add(store: &mut Store, args: AgentAddArgs) -> Result<()> {
    let project = project_or_cwd(store, args.project.as_deref())?;
    let team = team_of(store, &project)?;

    // A preset is a starting point, not a constraint: every field it fills can be
    // overridden on the same command line.
    let preset = match &args.preset {
        Some(name) => Some(
            ai_team_core::preset(name)
                .with_context(|| format!("no built-in preset called {name:?}"))?,
        ),
        None => ai_team_core::preset(&args.role),
    };
    let ord = args.ord.unwrap_or_else(|| {
        store
            .agents(team.id)
            .map_or(0, |agents| i64::try_from(agents.len()).unwrap_or(i64::MAX))
    });
    let base = preset.map(|preset| preset.to_new_agent(ord));

    let mut new = NewAgent {
        role: args.role.clone(),
        name: args
            .name
            .or_else(|| base.as_ref().map(|base| base.name.clone()))
            .unwrap_or_else(|| args.role.clone()),
        purpose: args
            .purpose
            .or_else(|| base.as_ref().map(|base| base.purpose.clone()))
            .unwrap_or_default(),
        provider: args.provider.unwrap_or(Provider::Local),
        model: args
            .model
            .unwrap_or_else(|| ai_team_core::DEFAULT_LOCAL_MODEL.to_string()),
        reasoning: args
            .reasoning
            .or_else(|| base.as_ref().map(|base| base.reasoning))
            .unwrap_or(Reasoning::Medium),
        zone: if args.zone.is_empty() {
            base.as_ref()
                .map(|base| base.zone.clone())
                .unwrap_or_default()
        } else {
            zone_of(&args.zone)
        },
        prompt_preset: base.as_ref().and_then(|base| base.prompt_preset.clone()),
        prompt_md: None,
        context_window: args.context_window,
        read_only: args.read_only || base.as_ref().is_some_and(|base| base.read_only),
        enabled: true,
        ord,
    };
    if let Some(path) = &args.prompt_file {
        new.prompt_md = Some(read_prompt(path)?);
        new.prompt_preset = None;
    }
    // A seat with no preset and no file still needs instructions, and the role's own
    // purpose is a better prompt than refusing outright.
    if new.prompt_preset.is_none() && new.prompt_md.is_none() {
        if new.purpose.trim().is_empty() {
            bail!(
                "{} matches no preset, so it needs --purpose or --prompt-file",
                args.role
            );
        }
        new.prompt_md = Some(new.purpose.clone());
    }

    let created = store.add_agent(team.id, new)?;
    println!(
        "Added {} to {} as {}/{}",
        created.role, team.slug, created.provider, created.model
    );
    crate::cmd::team::stale_notice();
    Ok(())
}

fn edit(store: &mut Store, args: AgentEditArgs) -> Result<()> {
    let agent = seat(store, args.project.as_deref(), &args.role)?;
    let mut update = NewAgent::from(&agent);

    if let Some(role) = args.new_role {
        update.role = role;
    }
    if let Some(name) = args.name {
        update.name = name;
    }
    if let Some(purpose) = args.purpose {
        update.purpose = purpose;
    }
    if let Some(provider) = args.provider {
        // Changing provider without a model would leave the old provider's model name
        // behind, which fails at the model call rather than here.
        if args.model.is_none() && provider != agent.provider {
            bail!("--provider changes the account, so pass --model too");
        }
        update.provider = provider;
        // The old window described the old model; let the registry re-default it.
        if args.context_window.is_none() && provider != agent.provider {
            update.context_window = None;
        }
    }
    if let Some(model) = args.model {
        update.model = model;
    }
    if let Some(reasoning) = args.reasoning {
        update.reasoning = reasoning;
    }
    if !args.zone.is_empty() {
        update.zone = zone_of(&args.zone);
    }
    if let Some(tokens) = args.context_window {
        update.context_window = (tokens > 0).then_some(tokens);
    }
    if let Some(path) = &args.prompt_file {
        update.prompt_md = Some(read_prompt(path)?);
        update.prompt_preset = None;
    }
    if let Some(preset) = args.preset {
        ai_team_core::preset(&preset)
            .with_context(|| format!("no built-in preset called {preset:?}"))?;
        update.prompt_preset = Some(preset);
        update.prompt_md = None;
    }
    if args.read_only {
        update.read_only = true;
    }
    if args.writes {
        update.read_only = false;
    }
    if let Some(ord) = args.ord {
        update.ord = ord;
    }

    let updated = store.update_agent(agent.id, update)?;
    println!(
        "{} is now {}/{}, {}",
        updated.role,
        updated.provider,
        updated.model,
        access(&updated)
    );
    crate::cmd::team::stale_notice();
    Ok(())
}

fn tools(store: &mut Store, args: AgentToolsArgs) -> Result<()> {
    let agent = seat(store, args.project.as_deref(), &args.role)?;
    let changing = !args.allow.is_empty() || !args.deny.is_empty() || !args.clear.is_empty();

    for tool in &args.allow {
        store.set_tool_policy(agent.id, tool, ToolEffect::Allow, args.note.as_deref())?;
    }
    for tool in &args.deny {
        store.set_tool_policy(agent.id, tool, ToolEffect::Deny, args.note.as_deref())?;
    }
    for tool in &args.clear {
        if !store.remove_tool_policy(agent.id, tool)? {
            println!("{} had no rule for {tool}", args.role);
        }
    }

    let policies = store.tool_policies(agent.id)?;
    if policies.is_empty() {
        println!("{} has no tool rules", args.role);
    } else {
        for policy in policies {
            println!(
                "{:<8} {:<20} {}",
                policy.effect,
                policy.tool,
                policy.note.unwrap_or_default()
            );
        }
    }
    if changing {
        // Worth stating: read_only is a property of the seat, and the generator obeys it
        // whatever a tool rule says.
        if agent.read_only {
            println!(
                "\nNote: {} is read-only, so write_file and edit_file are not generated \
                 at all, whatever these rules say.",
                args.role
            );
        }
        crate::cmd::team::stale_notice();
    }
    Ok(())
}
