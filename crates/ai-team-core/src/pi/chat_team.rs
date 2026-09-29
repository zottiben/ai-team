//! Chat seats never pass through the standalone planner adapter or shared role configs.

use std::path::Path;

use serde_json::json;

use crate::planning::{PlanAccess, PlanActor};
use crate::{Error, ModelRegistry, PiTurn, Result, Store};

pub(crate) fn planning_turn(
    store: &Store,
    chat_id: i64,
    node_id: i64,
    registry: &ModelRegistry,
    prompt: String,
) -> Result<PiTurn> {
    seat_turn(store, chat_id, node_id, registry, prompt, true)
}

pub(crate) fn worker_turn(
    store: &Store,
    chat_id: i64,
    node_id: i64,
    registry: &ModelRegistry,
    prompt: String,
) -> Result<PiTurn> {
    seat_turn(store, chat_id, node_id, registry, prompt, false)
}

fn seat_turn(
    store: &Store,
    chat_id: i64,
    node_id: i64,
    registry: &ModelRegistry,
    prompt: String,
    planning: bool,
) -> Result<PiTurn> {
    let chat = store.chat(chat_id)?;
    let node = store.node_run(node_id)?;
    store
        .chat_team_run(node.run_id)?
        .filter(|run| run.chat_id == chat_id)
        .ok_or_else(|| Error::invalid("this node is not a member of this chat's team execution"))?;
    let access = store.planning_access(chat_id, PlanActor::Agent(node_id))?;
    if planning != matches!(access, PlanAccess::Coordinator | PlanAccess::Planner) {
        return Err(Error::invalid(
            "this driver starts only team planning seats",
        ));
    }
    let agent = resolved_agent(store, &node, registry)?;
    let worktree = Path::new(
        node.worktree_path
            .as_deref()
            .ok_or_else(|| Error::invalid("this team seat has no bound worktree"))?,
    )
    .canonicalize()?;
    if planning
        && !node
            .worktree_path
            .as_deref()
            .is_some_and(|path| crate::same_worktree(path, &chat.workspace_path))
    {
        return Err(Error::invalid(
            "this planning seat is not attached to its chat checkout",
        ));
    }
    let project = store.project(chat.project_id)?;
    let database = store.path().canonicalize()?;
    let support = database
        .parent()
        .ok_or_else(|| Error::invalid("the chat store needs a data directory"))?
        .join("seats")
        .join(project.slug)
        .join(format!("chat-{chat_id}"))
        .join(format!("node-{node_id}"));
    std::fs::create_dir_all(&support)?;
    let support = support.canonicalize()?;
    if support.starts_with(&worktree) {
        return Err(Error::invalid(
            "team support files must live outside the chat checkout",
        ));
    }
    let mut sources = registry.context_sources();
    if !planning {
        sources.retain(|source| {
            access == PlanAccess::Maker
                && agent.role == "frontend"
                && *source == crate::ContextSource::Figma
        });
    }
    let mut config =
        super::seat::mcp_config(&sources, None).unwrap_or_else(|| json!({"mcpServers":{}}));
    super::seat::merge_safe_project_servers(&worktree, &mut config, planning)?;
    config["mcpServers"]["ai-team-planner"] = json!({
        "command": std::env::current_exe()?,
        "args": ["plan", "serve", "--db", database, "--chat", chat_id.to_string(), "--node", node_id.to_string()],
        "transport": "stdio", "lifecycle": "eager", "directTools": true,
        "includeTools": access.tools(),
    });
    let mcp = support.join("mcp.json");
    std::fs::write(&mcp, serde_json::to_string_pretty(&config)?)?;
    let instructions = seat_instructions(store, &agent, &node, chat_id, access, &worktree)?;
    let mut turn = PiTurn::new(&worktree, prompt);
    turn.provider = Some(super::seat::provider_name(node.provider).into());
    turn.model = Some(node.model);
    turn.thinking = Some(super::seat::thinking(agent.reasoning).into());
    // Use the immutable capability, not a mutable read_only team preference.
    turn.exclude_tools = [
        "web_search",
        "source_check",
        "fetch_content",
        "get_search_content",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    if access != PlanAccess::Maker {
        turn.exclude_tools.extend(["write".into(), "edit".into()]);
    }
    turn.guard = Some(super::guard::install_at(&support)?);
    turn.mcp_config = Some(mcp);
    turn.environment = super::seat::context_environment(&sources);
    turn.environment
        .push(("PI_MCP_CONFIG_MODE".into(), "exclusive".into()));
    turn.instructions = Some(instructions);
    Ok(turn)
}

fn resolved_agent(
    store: &Store,
    node: &crate::NodeRun,
    registry: &ModelRegistry,
) -> Result<crate::Agent> {
    let mut agent = store.agent(
        node.agent_id
            .ok_or_else(|| Error::invalid("this seat no longer exists"))?,
    )?;
    if !agent.enabled {
        return Err(Error::invalid(
            "this seat was disabled before its turn started",
        ));
    }
    agent.provider = node.provider;
    agent.model.clone_from(&node.model);
    let resolved = registry.resolve(&agent)?;
    if resolved.provider != node.provider || resolved.model != node.model {
        return Err(Error::invalid(
            "model policy changed before this team turn started; retry with the current policy",
        ));
    }
    Ok(agent)
}

fn seat_instructions(
    store: &Store,
    agent: &crate::Agent,
    node: &crate::NodeRun,
    chat_id: i64,
    access: PlanAccess,
    worktree: &Path,
) -> Result<String> {
    let team_id = store
        .run(node.run_id)?
        .team_id
        .ok_or_else(|| Error::invalid("this execution's team no longer exists"))?;
    Ok(
        if matches!(access, PlanAccess::Coordinator | PlanAccess::Planner) {
            super::instructions::for_chat_planning(agent, access, &store.agents(team_id)?, worktree)
        } else {
            let plan = store.chat_plan(chat_id, PlanActor::Agent(node.id))?;
            let slice = plan
                .bundle
                .and_then(|bundle| {
                    bundle
                        .slices
                        .into_iter()
                        .find(|slice| Some(&slice.key) == node.slice_key.as_ref())
                })
                .ok_or_else(|| {
                    Error::invalid("this worker has no assigned slice in the chat's plan")
                })?;
            super::instructions::for_chat_worker(agent, access, &slice, worktree)
        },
    )
}
