//! The embedded planner's stdio surface. Both binaries host it themselves, so a new
//! desktop build never shells out to an older installed `ait` (or to standalone aip).

mod tools;

use std::path::PathBuf;

use ai_team_core::planning::{ChatPlan, PlanAction, PlanActor};
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

    fn invoke(
        &self,
        name: &str,
        mut arguments: Map<String, Value>,
    ) -> ai_team_core::Result<ChatPlan> {
        let mut store = Store::open_planning_host(&self.db)?;
        let actor = PlanActor::Agent(self.node);
        if !store
            .planning_access(self.chat, actor)?
            .tools()
            .contains(&name)
        {
            return Err(ai_team_core::Error::invalid(
                "that tool is not allowed for this seat",
            ));
        }
        if name == "get_plan" {
            if !arguments.is_empty() {
                return Err(ai_team_core::Error::invalid(
                    "get_plan accepts no scope overrides",
                ));
            }
            return store.chat_plan(self.chat, actor);
        }
        if arguments.contains_key("action") {
            return Err(ai_team_core::Error::invalid(
                "the tool name selects the action",
            ));
        }
        arguments.insert("action".into(), name.into());
        let action: PlanAction = serde_json::from_value(arguments.into())?;
        store.change_chat_plan(self.chat, actor, action)
    }
}

impl ServerHandler for PlannerMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ai-team-planner", env!("CARGO_PKG_VERSION")))
            .with_instructions("Planning belongs only to the bound ai-team chat. Call get_plan first; each mutation requires its expect_revision and returns the updated snapshot. Human questions must be answered in Overview, never by the agent. Do not invoke standalone aip.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let server = self.clone();
        let access = tokio::task::spawn_blocking(move || server.access())
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?
            .map_err(|error| ErrorData::invalid_request(error.to_string(), None))?;
        Ok(ListToolsResult::with_all_items(
            access
                .tools()
                .iter()
                .map(|name| tools::definition(name))
                .collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let server = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            server.invoke(&request.name, request.arguments.unwrap_or_default())
        })
        .await;
        let result = match result {
            Ok(Ok(snapshot)) => serde_json::to_string(&snapshot).map_err(|error| error.to_string()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(error) => Err(format!("planning request failed: {error}")),
        };
        Ok(match result {
            Ok(json) => CallToolResult::success(vec![ContentBlock::text(json)]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        })
    }
}
