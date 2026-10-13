//! The embedded planner's stdio surface. Both binaries host it themselves, so a new
//! desktop build never shells out to an older installed `ait` (or to standalone aip).

mod response;
mod tools;

use std::path::PathBuf;

use ai_team_core::planning::{PlanAction, PlanActor};
use ai_team_core::Store;
use clap::{Parser, Subcommand};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value};

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    /// Serve only the active execution's own chat plan. No standalone planner defaults.
    Serve {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        chat: i64,
        #[arg(long)]
        node: i64,
    },
}

impl PlanCommand {
    pub async fn run(self) -> ai_team_core::Result<()> {
        let Self::Serve { db, chat, node } = self;
        let server = PlannerMcp { db, chat, node };
        server.access()?;
        server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| {
                ai_team_core::Error::invalid(format!("starting embedded planner MCP: {error}"))
            })?
            .waiting()
            .await
            .map_err(|error| {
                ai_team_core::Error::invalid(format!("embedded planner MCP: {error}"))
            })?;
        Ok(())
    }
}

#[derive(Debug, Parser)]
struct DesktopControl {
    #[command(subcommand)]
    command: Control,
}
#[derive(Debug, Subcommand)]
enum Control {
    #[command(subcommand)]
    Plan(PlanCommand),
}

/// Called before creating a native window. A stdio helper must never show a GUI dialog.
pub fn desktop_command() -> Option<std::result::Result<PlanCommand, clap::Error>> {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new("plan")) {
        return None;
    }
    Some(
        DesktopControl::try_parse().map(|control| match control.command {
            Control::Plan(command) => command,
        }),
    )
}

#[derive(Debug, Clone)]
struct PlannerMcp {
    db: PathBuf,
    chat: i64,
    node: i64,
}

impl PlannerMcp {
    fn access(&self) -> ai_team_core::Result<ai_team_core::planning::PlanAccess> {
        Store::open_planning_host(&self.db)?.planning_access(self.chat, PlanActor::Agent(self.node))
    }

    /// What this seat may call. `request_publication` appears only while the person's own
    /// explicit push instruction is still live for this exact turn - a tool the model can
    /// see is one it keeps trying, so an unauthorised turn is not offered it at all.
    fn tools(&self) -> ai_team_core::Result<Vec<&'static str>> {
        let store = Store::open_planning_host(&self.db)?;
        let mut names = store
            .planning_access(self.chat, PlanActor::Agent(self.node))?
            .tools()
            .to_vec();
        if store
            .chat_push_grant_for_node(self.chat, self.node)?
            .is_some()
        {
            names.push(ai_team_core::chat_push::PUBLICATION_TOOL);
        }
        Ok(names)
    }

    fn invoke(&self, name: &str, mut arguments: Map<String, Value>) -> ai_team_core::Result<Value> {
        let mut store = Store::open_planning_host(&self.db)?;
        let actor = PlanActor::Agent(self.node);
        if !self.tools()?.contains(&name) {
            return Err(ai_team_core::Error::invalid(
                "that tool is not allowed for this seat",
            ));
        }
        if name == ai_team_core::chat_push::PUBLICATION_TOOL {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Request {
                commit: String,
            }
            let request: Request = serde_json::from_value(arguments.into())?;
            // The grant is rechecked inside, against the rows, so a stale tool list
            // cannot turn into permission.
            return Ok(Value::String(tokio::runtime::Handle::current().block_on(
                ai_team_core::chat_push::request_publication(
                    &self.db,
                    self.chat,
                    self.node,
                    &request.commit,
                ),
            )?));
        }
        if matches!(name, "list_worktrees" | "request_worktree") {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Request {
                path: PathBuf,
            }
            let root = ai_team_core::chat_workspaces::repository(&store, self.chat)?;
            let runtime = tokio::runtime::Handle::current();
            if name == "list_worktrees" {
                if !arguments.is_empty() {
                    return Err(ai_team_core::Error::invalid(
                        "list_worktrees accepts no scope overrides",
                    ));
                }
                return Ok(serde_json::to_value(runtime.block_on(
                    ai_team_core::chat_workspaces::owned_choices(&self.db, &root),
                )?)?);
            }
            let request: Request = serde_json::from_value(arguments.into())?;
            let target = runtime.block_on(ai_team_core::chat_workspaces::validate_owned(
                &self.db,
                &root,
                &request.path,
            ))?;
            return Ok(serde_json::to_value(
                store.request_chat_workspace(self.chat, actor, &target)?,
            )?);
        }
        if name == "list_chat_images" {
            if !arguments.is_empty() {
                return Err(ai_team_core::Error::invalid(
                    "list_chat_images accepts no scope overrides",
                ));
            }
            return Ok(serde_json::to_value(store.chat_turns(self.chat)?.into_iter().filter(|turn| !turn.images.is_empty()).map(|turn| serde_json::json!({"node":turn.node.id,"at":turn.run.created_at,"caption":turn.run.prompt.chars().take(160).collect::<String>(),"images":turn.images})).collect::<Vec<_>>())?);
        }
        if name == "read_chat_image" {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Image {
                node: i64,
                index: usize,
            }
            let request: Image = serde_json::from_value(arguments.into())?;
            return Ok(serde_json::to_value(store.chat_image(
                self.chat,
                request.node,
                request.index,
            )?)?);
        }
        if name == "get_plan" {
            if !arguments.is_empty() {
                return Err(ai_team_core::Error::invalid(
                    "get_plan accepts no scope overrides",
                ));
            }
            return response::snapshot(&store.chat_plan(self.chat, actor)?);
        }
        if name == "read_plan" {
            // Scope overrides are rejected by the request's own unknown fields.
            let request: response::Page = serde_json::from_value(arguments.into())?;
            return response::page(&store.chat_plan(self.chat, actor)?, request);
        }
        if arguments.contains_key("action") {
            return Err(ai_team_core::Error::invalid(
                "the tool name selects the action",
            ));
        }
        arguments.insert("action".into(), name.into());
        let action: PlanAction = serde_json::from_value(arguments.into())?;
        response::snapshot(&store.change_chat_plan(self.chat, actor, action)?)
    }
}

impl ServerHandler for PlannerMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ai-team-planner", env!("CARGO_PKG_VERSION")))
            .with_instructions("Planning belongs only to the bound ai-team chat. Call get_plan first; each mutation requires its expect_revision and returns the updated snapshot. A plan too large for one response answers with partial:true instead: it is whole and unchanged, and nothing has been removed from it. That answer still carries the revision every mutation needs, so you can write straight away; to read the plan itself, call read_plan from offset 0 and follow each next_offset once until it is null, then join the pages in order. Human questions must be answered in the chat's Board, never by the agent. If the user asks to use another worktree, call list_worktrees and request_worktree with its exact path, then end this turn. The request needs human approval in chat after this turn settles; do not change directories or continue work in another checkout yourself. Do not invoke standalone aip.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let server = self.clone();
        let names = tokio::task::spawn_blocking(move || server.tools())
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?
            .map_err(|error| ErrorData::invalid_request(error.to_string(), None))?;
        Ok(ListToolsResult::with_all_items(
            names.iter().map(|name| tools::definition(name)).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let server = self.clone();
        let image = request.name == "read_chat_image";
        let result = tokio::task::spawn_blocking(move || {
            server.invoke(&request.name, request.arguments.unwrap_or_default())
        })
        .await;
        let result = match result {
            Ok(Ok(snapshot)) if image => {
                serde_json::from_value::<ai_team_core::chat_images::ImageInput>(snapshot)
                    .map(|input| ContentBlock::image(input.data, input.media_type))
                    .map_err(|error| error.to_string())
            }
            Ok(Ok(snapshot)) => serde_json::to_string(&snapshot)
                .map(ContentBlock::text)
                .map_err(|error| error.to_string()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(error) => Err(format!("planning request failed: {error}")),
        };
        Ok(match result {
            Ok(content) => CallToolResult::success(vec![content]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        })
    }
}
