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
    let chat = store.chat(chat_id)?;
    let node = store.node_run(node_id)?;
    let execution = store
        .chat_team_run(node.run_id)?
        .filter(|run| run.chat_id == chat_id)
        .ok_or_else(|| Error::invalid("this node is not a member of this chat's team execution"))?;
    let access = store.planning_access(chat_id, PlanActor::Agent(node_id))?;
    if !matches!(access, PlanAccess::Coordinator | PlanAccess::Planner) {
        return Err(Error::invalid(
            "this driver starts only team planning seats",
        ));
    }
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
    let worktree = Path::new(&chat.workspace_path).canonicalize()?;
    if !node
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
    let sources = registry.context_sources();
    let mut config =
        super::seat::mcp_config(&sources, None).unwrap_or_else(|| json!({"mcpServers":{}}));
    super::seat::merge_safe_project_servers(&worktree, &mut config, true)?;
    config["mcpServers"]["ai-team-planner"] = json!({
        "command": std::env::current_exe()?,
        "args": ["plan", "serve", "--db", database, "--chat", chat_id.to_string(), "--node", node_id.to_string()],
        "transport": "stdio", "lifecycle": "eager", "directTools": true,
        "includeTools": access.tools(),
    });
    let mcp = support.join("mcp.json");
    std::fs::write(&mcp, serde_json::to_string_pretty(&config)?)?;
    let mut turn = PiTurn::new(&worktree, prompt);
    turn.provider = Some(super::seat::provider_name(node.provider).into());
    turn.model = Some(node.model);
    turn.thinking = Some(super::seat::thinking(agent.reasoning).into());
    // Use the immutable capability, not a mutable read_only team preference.
    turn.exclude_tools = [
        "write",
        "edit",
        "web_search",
        "source_check",
        "fetch_content",
        "get_search_content",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    turn.guard = Some(super::guard::install_at(&support)?);
    turn.mcp_config = Some(mcp);
    turn.environment = super::seat::context_environment(&sources);
    turn.environment
        .push(("PI_MCP_CONFIG_MODE".into(), "exclusive".into()));
    let team_id = store
        .run(execution.run_id)?
        .team_id
        .ok_or_else(|| Error::invalid("this execution's team no longer exists"))?;
    turn.instructions = Some(super::instructions::for_chat_planning(
        &agent,
        access,
        &store.agents(team_id)?,
        &worktree,
    ));
    Ok(turn)
}
