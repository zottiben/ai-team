//! The rows behind the app-owned MCP servers. The only place their SQL lives.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension, Row};

use crate::mcp::{self, Definition, McpServer, McpServerEdit, McpTransport, NewMcpServer, Shape};
use crate::{Error, Result, Store};

/// A row as a surface sees it, and the two value maps it is never shown.
type Listed = (
    McpServer,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
);

/// Everything but the values, which no surface is given.
fn listed(row: &Row<'_>) -> rusqlite::Result<Listed> {
    let transport: String = row.get("transport")?;
    let args: String = row.get("args_json")?;
    let env: String = row.get("env_json")?;
    let headers: String = row.get("headers_json")?;
    let tools: Option<String> = row.get("include_tools_json")?;
    let decode = |field: &str, body: &str| -> rusqlite::Result<serde_json::Value> {
        serde_json::from_str(body).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(format!(
                    "stored MCP {field} is not JSON: {error}"
                ))),
            )
        })
    };
    let args: Vec<String> = serde_json::from_value(decode("arguments", &args)?).unwrap_or_default();
    let env: BTreeMap<String, String> =
        serde_json::from_value(decode("environment", &env)?).unwrap_or_default();
    let headers: BTreeMap<String, String> =
        serde_json::from_value(decode("headers", &headers)?).unwrap_or_default();
    let include_tools: Option<Vec<String>> = tools
        .map(|body| decode("allow-list", &body))
        .transpose()?
        .and_then(|value| serde_json::from_value(value).ok());
    let transport = McpTransport::parse_row(&transport)?;
    Ok((
        McpServer {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            name: row.get("name")?,
            enabled: row.get::<_, i64>("enabled")? == 1,
            transport,
            command: row.get("command")?,
            args,
            url: row.get("url")?,
            env_keys: env.keys().cloned().collect(),
            header_keys: headers.keys().cloned().collect(),
            include_tools,
            origin: row.get("origin")?,
            origin_detail: row.get("origin_detail")?,
            rev: row.get("rev")?,
            updated_at: row.get("updated_at")?,
        },
        env,
        headers,
    ))
}

impl McpTransport {
    fn parse_row(text: &str) -> rusqlite::Result<McpTransport> {
        match text {
            "stdio" => Ok(McpTransport::Stdio),
            "http" => Ok(McpTransport::Http),
            _ => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other("unknown stored MCP transport")),
            )),
        }
    }
}

const COLUMNS: &str = "id,project_id,name,enabled,transport,command,args_json,url,env_json,headers_json,include_tools_json,origin,origin_detail,rev,updated_at";

impl Store {
    /// A named scope has to exist, and not existing is the caller's mistake rather than
    /// this server's - otherwise a stale project id in a URL reads as a crash.
    fn mcp_scope(&self, project: Option<i64>) -> Result<()> {
        if let Some(project) = project {
            self.project(project).map_err(|_| {
                Error::invalid(format!("no project {project} to scope MCP servers to"))
            })?;
        }
        Ok(())
    }

    /// The app defaults, and - when a project is named - that project's own rows.
    ///
    /// Both scopes are returned rather than resolved, because a page that only showed the
    /// winner could not explain why a default is not in force.
    pub fn mcp_servers(&self, project: Option<i64>) -> Result<Vec<McpServer>> {
        self.mcp_scope(project)?;
        let mut query = self.db().conn().prepare(&format!(
            "SELECT {COLUMNS} FROM mcp_server WHERE project_id IS NULL OR project_id=?1 ORDER BY name,project_id IS NULL"
        ))?;
        let rows = query
            .query_map(params![project], |row| {
                listed(row).map(|(server, _, _)| server)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn mcp_server(&self, id: i64) -> Result<McpServer> {
        self.db()
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM mcp_server WHERE id=?1"),
                params![id],
                |row| listed(row).map(|(server, _, _)| server),
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no such MCP server"))
    }

    /// What this project's seats are given, with the app defaults a project row replaces
    /// already resolved away.
    pub(crate) fn mcp_definitions(&self, project: i64) -> Result<Vec<Definition>> {
        self.project(project)?;
        let mut query = self.db().conn().prepare(&format!(
            "SELECT {COLUMNS} FROM mcp_server WHERE project_id IS NULL OR project_id=?1 ORDER BY name,project_id IS NULL"
        ))?;
        let rows = query
            .query_map(params![project], listed)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // A project's own row sorts first within a name, so the first of each name wins.
        let mut resolved: Vec<Definition> = Vec::new();
        for (server, env, headers) in rows {
            if resolved.iter().any(|kept| kept.name == server.name) {
                continue;
            }
            resolved.push(Definition {
                id: server.id,
                name: server.name,
                enabled: server.enabled,
                transport: server.transport,
                command: server.command,
                args: server.args,
                url: server.url,
                env,
                headers,
                include_tools: server.include_tools,
            });
        }
        Ok(resolved)
    }

    /// How many rows this scope holds in its own right. An app default is not one of a
    /// project's, which is why this is not simply the length of what a page lists.
    fn mcp_held(&self, project: Option<i64>) -> Result<usize> {
        Ok(self
            .mcp_servers(project)?
            .into_iter()
            .filter(|server| server.project_id == project)
            .count())
    }

    pub fn add_mcp_server(&mut self, new: NewMcpServer) -> Result<McpServer> {
        self.add_mcp_server_from(new, "manual", "")
    }

    /// Pin a whole reviewed batch, or none of it.
    ///
    /// One transaction rather than a loop over `add_mcp_server_from`: a second name that
    /// collides would otherwise leave the first imported, and the page would have to
    /// explain a half-done approval. Every row arrives switched off - `mcp::import_from`
    /// has already settled that each one can be pinned exactly as it stands.
    pub(crate) fn import_mcp_servers(
        &mut self,
        project: Option<i64>,
        chosen: Vec<(NewMcpServer, String)>,
    ) -> Result<Vec<McpServer>> {
        self.mcp_scope(project)?;
        let mut rows = Vec::new();
        for (new, detail) in &chosen {
            let (name, include_tools) = mcp::check(Shape {
                name: &new.name,
                transport: new.transport,
                command: new.command.as_deref(),
                args: &new.args,
                url: new.url.as_deref(),
                env: &new.env,
                headers: &new.headers,
                include_tools: new.include_tools.as_deref(),
            })?;
            let (command, url) = match new.transport {
                McpTransport::Stdio => (
                    new.command.as_deref().map(str::trim).map(str::to_string),
                    None,
                ),
                McpTransport::Http => (None, new.url.as_deref().map(str::trim).map(str::to_string)),
            };
            rows.push((
                name,
                new.transport,
                command,
                serde_json::to_string(&new.args)?,
                url,
                serde_json::to_string(&new.env)?,
                serde_json::to_string(&new.headers)?,
                include_tools
                    .map(|tools| serde_json::to_string(&tools))
                    .transpose()?,
                detail.clone(),
            ));
        }
        mcp::check_room(self.mcp_held(project)?, rows.len())?;
        let at = crate::now();
        let ids = self.db_mut().write(|tx| {
            let mut ids = Vec::new();
            for (name, transport, command, args, url, env, headers, tools, detail) in &rows {
                tx.execute(
                    "INSERT INTO mcp_server(project_id,name,enabled,transport,command,args_json,url,env_json,headers_json,include_tools_json,origin,origin_detail,created_at,updated_at) \
                     VALUES(?1,?2,0,?3,?4,?5,?6,?7,?8,?9,'import',?10,?11,?11)",
                    params![project,name,transport.as_str(),command,args,url,env,headers,tools,detail,at],
                ).map_err(|error| match error {
                    rusqlite::Error::SqliteFailure(failure, _) if failure.code == rusqlite::ErrorCode::ConstraintViolation => {
                        Error::invalid(format!("{name} is already configured in this scope; edit that one instead"))
                    }
                    other => other.into(),
                })?;
                ids.push(tx.last_insert_rowid());
            }
            Ok(ids)
        })?;
        ids.into_iter().map(|id| self.mcp_server(id)).collect()
    }

    pub(crate) fn add_mcp_server_from(
        &mut self,
        new: NewMcpServer,
        origin: &str,
        origin_detail: &str,
    ) -> Result<McpServer> {
        self.mcp_scope(new.project_id)?;
        let (name, include_tools) = mcp::check(Shape {
            name: &new.name,
            transport: new.transport,
            command: new.command.as_deref(),
            args: &new.args,
            url: new.url.as_deref(),
            env: &new.env,
            headers: &new.headers,
            include_tools: new.include_tools.as_deref(),
        })?;
        mcp::check_room(self.mcp_held(new.project_id)?, 1)?;
        let args = serde_json::to_string(&new.args)?;
        let env = serde_json::to_string(&new.env)?;
        let headers = serde_json::to_string(&new.headers)?;
        let tools = include_tools
            .map(|tools| serde_json::to_string(&tools))
            .transpose()?;
        let at = crate::now();
        let (command, url) = match new.transport {
            McpTransport::Stdio => (
                new.command.as_deref().map(str::trim).map(str::to_string),
                None,
            ),
            McpTransport::Http => (None, new.url.as_deref().map(str::trim).map(str::to_string)),
        };
        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO mcp_server(project_id,name,enabled,transport,command,args_json,url,env_json,headers_json,include_tools_json,origin,origin_detail,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
                params![new.project_id,name,i64::from(new.enabled),new.transport.as_str(),command,args,url,env,headers,tools,origin,origin_detail,at],
            ).map_err(|error| match error {
                rusqlite::Error::SqliteFailure(failure, _) if failure.code == rusqlite::ErrorCode::ConstraintViolation => {
                    Error::invalid(format!("{name} is already configured in this scope; edit that one instead"))
                }
                other => other.into(),
            })?;
            Ok(tx.last_insert_rowid())
        })?;
        self.mcp_server(id)
    }

    /// Edit against the revision the surface was showing.
    ///
    /// `env` and `headers` omitted retains what is stored: the page was never given the
    /// values, so it cannot send them back, and treating silence as "clear it" would
    /// empty a credential every time somebody ticked a box.
    ///
    /// Which is exactly why a change of *destination* is refused while they are omitted.
    /// A credential was given to a particular command, its arguments or a particular URL;
    /// carrying it across to another one hands the operator's token to whatever is there
    /// now, and the save that did it looked like editing a text field. So a destination
    /// change has to say what the values are - a replacement map, or an empty one to
    /// clear them. Everything that is not the destination still saves in silence.
    pub fn edit_mcp_server(&mut self, id: i64, edit: McpServerEdit) -> Result<McpServer> {
        let (current, stored_env, stored_headers) = self
            .db()
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM mcp_server WHERE id=?1"),
                params![id],
                listed,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no such MCP server"))?;
        let command = match (current.transport, &edit.command) {
            (McpTransport::Stdio, Some(command)) => Some(command.trim().to_string()),
            (McpTransport::Stdio, None) => current.command.clone(),
            (McpTransport::Http, _) => None,
        };
        let url = match (current.transport, &edit.url) {
            (McpTransport::Http, Some(url)) => Some(url.trim().to_string()),
            (McpTransport::Http, None) => current.url.clone(),
            (McpTransport::Stdio, _) => None,
        };
        let args = match (current.transport, &edit.args) {
            (McpTransport::Stdio, Some(args)) => args.clone(),
            (McpTransport::Stdio, None) => current.args.clone(),
            (McpTransport::Http, _) => Vec::new(),
        };
        let moved = match current.transport {
            McpTransport::Stdio => command != current.command || args != current.args,
            McpTransport::Http => url != current.url,
        };
        if moved {
            let carried: Vec<&str> = [
                (!stored_env.is_empty() && edit.env.is_none()).then_some("environment variables"),
                (!stored_headers.is_empty() && edit.headers.is_none()).then_some("headers"),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !carried.is_empty() {
                return Err(Error::invalid(format!(
                    "this change points {} somewhere else, and its stored {} would go with it unseen. Send their replacement values with the change, or an empty object to clear them.",
                    current.name,
                    carried.join(" and ")
                )));
            }
        }
        let env = edit.env.clone().unwrap_or(stored_env);
        let headers = edit.headers.clone().unwrap_or(stored_headers);
        let include_tools = match &edit.include_tools {
            Some(tools) => Some(tools.clone()),
            None => current.include_tools.clone(),
        };
        let (_, include_tools) = mcp::check(Shape {
            name: &current.name,
            transport: current.transport,
            command: command.as_deref(),
            args: &args,
            url: url.as_deref(),
            env: &env,
            headers: &headers,
            include_tools: include_tools.as_deref(),
        })?;
        let enabled = edit.enabled.unwrap_or(current.enabled);
        let args = serde_json::to_string(&args)?;
        let env = serde_json::to_string(&env)?;
        let headers = serde_json::to_string(&headers)?;
        let tools = include_tools
            .map(|tools| serde_json::to_string(&tools))
            .transpose()?;
        let at = crate::now();
        self.db_mut().write(|tx| {
            if tx.execute(
                "UPDATE mcp_server SET enabled=?3,command=?4,args_json=?5,url=?6,env_json=?7,headers_json=?8,include_tools_json=?9,rev=rev+1,updated_at=?10 \
                 WHERE id=?1 AND rev=?2",
                params![id,edit.rev,i64::from(enabled),command,args,url,env,headers,tools,at],
            )? != 1 {
                return Err(Error::invalid("this MCP server changed since the page read it; look again before saving"));
            }
            Ok(())
        })?;
        self.mcp_server(id)
    }

    pub fn remove_mcp_server(&mut self, id: i64, rev: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            if tx.execute(
                "DELETE FROM mcp_server WHERE id=?1 AND rev=?2",
                params![id, rev],
            )? != 1
            {
                return Err(Error::invalid(
                    "this MCP server changed since the page read it; look again before removing",
                ));
            }
            Ok(())
        })
    }
}
