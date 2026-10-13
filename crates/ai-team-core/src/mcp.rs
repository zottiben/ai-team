//! The MCP servers AI Team gives its own agents.
//!
//! A chat deliberately does not inherit the operator's global Pi registrations: its
//! seats run `PI_MCP_CONFIG_MODE=exclusive` so the generated planning and context
//! definitions cannot be replaced by a checkout, and the price of that is that a server
//! the operator uses in an ordinary Pi session is absent here. This module is how one
//! gets back: a row the operator added or imported, composed into the private config
//! each turn is given.
//!
//! Four properties the rest of the file exists to hold:
//!
//! - **Configured is not connected.** Nothing here opens a connection, runs a command or
//!   resolves a credential. A row says what a seat would be given; whether the server
//!   answers is learned when a turn starts, and the surfaces say so rather than showing a
//!   tick that means both.
//! - **An import is a snapshot, not inheritance.** Reading the operator's own Pi files
//!   produces candidates to review; importing one copies it into the store, disabled.
//!   The source changing afterwards changes nothing, and nothing is ever imported
//!   because it happened to be there. What is imported is what was *previewed*: the
//!   preview carries a fingerprint of the scope and of what the files held, the import
//!   quotes it back, and a file or a scope that moved in between is refused whole rather
//!   than re-read. A definition ai-team cannot copy exactly is not copied approximately.
//! - **Values never come back out.** `env` and `headers` are stored, but a surface sees
//!   their keys. They reach a turn as `${PLACEHOLDER}` in the generated file with the
//!   value in the child's environment - the same split the context sources use, for the
//!   same reason: the config is a file on disk beside the guard, and a credential written
//!   into it would outlive the turn.
//! - **The generated definitions win.** The reserved names are refused at every layer, so
//!   an app-owned row cannot replace ai-team's planner, the standalone one, or the
//!   read-only ClickUp and Figma allow-lists.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::store::Store;

/// Server names an app-owned row may never take.
///
/// `ai-team-planner` is the built-in planning service and `ai-planner` the standalone
/// one; `clickup` and `figma` carry the read-only allow-lists of D15. A row that could
/// take one of these names could widen what a seat may do by renaming itself into it.
pub const RESERVED: &[&str] = &["ai-planner", "ai-team-planner", "clickup", "figma"];

/// Programs an app-owned stdio server may not run.
///
/// `aip` and `ai-planner` would be the standalone planner reappearing beside the scoped
/// tools with none of its restrictions; `ait` and `ai-team` would be a second copy of the
/// built-in one, outside the chat scope its own server is given.
const RESERVED_COMMANDS: &[&str] = &["aip", "ai-planner", "ait", "ai-team"];

/// How many servers one scope may hold. Not a policy - a bound, so a malformed import
/// cannot turn into a config with a thousand entries in it.
const MOST_SERVERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            McpTransport::Stdio => "stdio",
            McpTransport::Http => "http",
        }
    }
}

/// One configured server, as a surface sees it.
///
/// `env_keys` and `header_keys` rather than the maps themselves: the keys say what a
/// server is configured with, which is what somebody reading the page needs, and the
/// values are the part that may be a credential.
#[derive(Debug, Clone, Serialize)]
pub struct McpServer {
    pub id: i64,
    /// `None` is an app default, offered to every project.
    pub project_id: Option<i64>,
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    /// `None` is every tool the server exposes.
    pub include_tools: Option<Vec<String>>,
    pub origin: String,
    pub origin_detail: String,
    pub rev: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewMcpServer {
    #[serde(default)]
    pub project_id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub transport: McpTransport,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// An empty list means every tool, which is what an empty field on the page means.
    #[serde(default)]
    pub include_tools: Option<Vec<String>>,
}

/// An edit, against the revision the surface was showing.
///
/// `env` and `headers` are write-only: omitted retains what is stored, present replaces
/// it whole. That is the only shape that works when the page was never given the values -
/// it cannot send back what it does not have, so sending nothing has to mean "leave it".
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerEdit {
    pub rev: i64,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// An empty list clears the allow-list back to every tool.
    #[serde(default)]
    pub include_tools: Option<Vec<String>>,
}

/// A definition with its values, for dispatch only. Never serialised.
#[derive(Debug, Clone)]
pub(crate) struct Definition {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) transport: McpTransport,
    pub(crate) command: Option<String>,
    pub(crate) args: Vec<String>,
    pub(crate) url: Option<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) include_tools: Option<Vec<String>>,
}

/// What a turn is given: server definitions for its config, the environment the
/// placeholders in them resolve from, and the names this scope has switched off.
///
/// `suppressed` is not the absence of a server - it is a row saying there is to be none
/// of that name. The checkout's own `.mcp.json` is merged into the config *before* these
/// are applied, so a switched-off row that merely declined to add itself would leave the
/// repository's server of that name in front of the seat, and the tick on the page would
/// be describing something that did not happen.
#[derive(Debug, Default, Clone)]
pub(crate) struct Composed {
    pub(crate) servers: Vec<(String, Value)>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) suppressed: Vec<String>,
}

/// Is this a browser-driving server?
///
/// Planning seats are refused one (rule 10's companion): a coordinator whose required
/// context failed must say so rather than open a browser and guess. Matched on the name
/// and on what a stdio server actually runs, because an app-owned row names itself.
pub(crate) fn is_browser(name: &str, command: Option<&str>, args: &[String]) -> bool {
    if matches!(name, "playwright" | "chrome-devtools") {
        return true;
    }
    command
        .into_iter()
        .chain(args.iter().map(String::as_str))
        .any(|word| {
            let word = word.to_ascii_lowercase();
            word.contains("chrome-devtools-mcp") || word.contains("playwright")
        })
}

/// The same question about a definition that is already JSON, for the checkout's own
/// servers. One predicate, so the two paths cannot drift apart.
pub(crate) fn is_browser_value(name: &str, server: &Value) -> bool {
    let args: Vec<String> = server
        .get("args")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    is_browser(name, server.get("command").and_then(Value::as_str), &args)
}

fn check_name(name: &str) -> Result<String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() || name.len() > 64 {
        return Err(Error::invalid("give the server a name of 1 to 64 letters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(Error::invalid(
            "a server name may hold letters, digits, dot, dash and underscore only",
        ));
    }
    if RESERVED.contains(&name.as_str()) {
        return Err(Error::invalid(format!(
            "{name} is one of AI Team's own generated servers and cannot be replaced here"
        )));
    }
    Ok(name)
}

/// Why a value is refused rather than stored.
///
/// A leading `!` is a command the adapter runs to produce a credential, and `${VAR}` is
/// an environment reference it resolves. Both would be executed or resolved somewhere
/// ai-team cannot see, and `${VAR}` additionally cannot survive the placeholder
/// indirection below - it would reach the server as the literal text. So both are
/// refused with the one instruction that works: paste the value.
fn check_values(kind: &str, values: &BTreeMap<String, String>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (key, value) in values {
        if key.trim().is_empty() {
            return Err(Error::invalid(format!("a {kind} name cannot be empty")));
        }
        if !seen.insert(sanitise(key)) {
            return Err(Error::invalid(format!(
                "two {kind} names differ only by punctuation ({key}); AI Team cannot keep them apart"
            )));
        }
        if value.is_empty() {
            return Err(Error::invalid(format!(
                "{key} has no value; remove it instead of storing it empty"
            )));
        }
        if value.starts_with('!') {
            return Err(Error::invalid(format!(
                "{key} runs a command to produce its value; AI Team does not run one, so paste the value itself"
            )));
        }
        if value.contains("${") || value.contains("$env:") || value.contains("{env:") {
            return Err(Error::invalid(format!(
                "{key} refers to an environment variable; AI Team supplies the value itself, so paste it rather than a reference"
            )));
        }
    }
    Ok(())
}

fn check_transport(
    transport: McpTransport,
    command: Option<&str>,
    args: &[String],
    url: Option<&str>,
) -> Result<()> {
    match transport {
        McpTransport::Stdio => {
            let command = command.map(str::trim).unwrap_or_default();
            if command.is_empty() {
                return Err(Error::invalid("a stdio server needs a command to run"));
            }
            if url.is_some_and(|url| !url.trim().is_empty()) {
                return Err(Error::invalid("a stdio server has a command, not a URL"));
            }
            let program = Path::new(command)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(command);
            if RESERVED_COMMANDS
                .iter()
                .any(|reserved| program.eq_ignore_ascii_case(reserved))
            {
                return Err(Error::invalid(format!(
                    "{program} is a planner; AI Team's own planning server is generated for each seat and cannot be configured here"
                )));
            }
            if args.iter().any(|arg| arg.trim().is_empty()) {
                return Err(Error::invalid("an empty argument is not an argument"));
            }
        }
        McpTransport::Http => {
            let url = url.map(str::trim).unwrap_or_default();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(Error::invalid(
                    "an HTTP server needs an http:// or https:// URL",
                ));
            }
            if command.is_some_and(|command| !command.trim().is_empty()) || !args.is_empty() {
                return Err(Error::invalid("an HTTP server has a URL, not a command"));
            }
        }
    }
    Ok(())
}

fn check_tools(tools: Option<&[String]>) -> Option<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for tool in tools? {
        let tool = tool.trim();
        if tool.is_empty() {
            continue;
        }
        if !kept.iter().any(|seen| seen == tool) {
            kept.push(tool.to_string());
        }
    }
    // An empty allow-list would be a server with no tools at all, which nobody means by
    // leaving the field blank. Blank is every tool.
    (!kept.is_empty()).then_some(kept)
}

/// A definition's shape, as it arrives from a surface or out of a Pi file.
///
/// One struct rather than eight arguments, so the store's add, the store's edit and the
/// import cannot check slightly different things.
pub(crate) struct Shape<'a> {
    pub(crate) name: &'a str,
    pub(crate) transport: McpTransport,
    pub(crate) command: Option<&'a str>,
    pub(crate) args: &'a [String],
    pub(crate) url: Option<&'a str>,
    pub(crate) env: &'a BTreeMap<String, String>,
    pub(crate) headers: &'a BTreeMap<String, String>,
    pub(crate) include_tools: Option<&'a [String]>,
}

/// Everything a row must satisfy, in one place, so the store and the import agree.
pub(crate) fn check(shape: Shape<'_>) -> Result<(String, Option<Vec<String>>)> {
    let name = check_name(shape.name)?;
    if shape
        .command
        .into_iter()
        .chain(shape.url)
        .chain(shape.args.iter().map(String::as_str))
        .any(|value| redact_word(value) != value)
    {
        return Err(Error::invalid("embedded URL credentials are not supported; configure secrets in the write-only headers or environment fields instead"));
    }
    if shape
        .command
        .into_iter()
        .chain(shape.url)
        .chain(shape.args.iter().map(String::as_str))
        .any(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("figma") || value.contains("clickup")
        })
    {
        return Err(Error::invalid("configure ClickUp and Figma through AI Team's managed read-only context settings, not an unrestricted MCP alias"));
    }
    check_transport(shape.transport, shape.command, shape.args, shape.url)?;
    check_values("environment variable", shape.env)?;
    check_values("header", shape.headers)?;
    if shape.transport == McpTransport::Stdio && !shape.headers.is_empty() {
        return Err(Error::invalid(
            "headers belong to an HTTP server; a stdio server is configured with environment variables",
        ));
    }
    Ok((name, check_tools(shape.include_tools)))
}

/// The environment variable a value travels in.
///
/// Keyed by row id so two servers whose names punctuate differently cannot collide, and
/// `check_values` already refused two keys in one server that would.
fn placeholder(id: i64, kind: &str, key: &str) -> String {
    format!("AI_TEAM_MCP_{id}_{kind}_{}", sanitise(key))
}

fn sanitise(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// What this project's seats get, resolved and turned into adapter definitions.
///
/// Reserved names and disabled rows are dropped here as well as refused at the store,
/// because this is the last place before a definition becomes a seat's capability.
pub(crate) fn compose(store: &Store, project: i64, exclude_browsers: bool) -> Result<Composed> {
    Ok(build(store.mcp_definitions(project)?, exclude_browsers))
}

fn build(definitions: Vec<Definition>, exclude_browsers: bool) -> Composed {
    let mut composed = Composed::default();
    for definition in definitions {
        if RESERVED.contains(&definition.name.as_str()) {
            continue;
        }
        if !definition.enabled {
            // Switched off here, or switched off by a project's own row replacing an app
            // default - the two are the same decision, because `mcp_definitions` has
            // already resolved which row of the name wins.
            composed.suppressed.push(definition.name);
            continue;
        }
        if exclude_browsers
            && is_browser(
                &definition.name,
                definition.command.as_deref(),
                &definition.args,
            )
        {
            // Not suppressed: this row is enabled, and what is being refused is a browser
            // for a planning seat. A checkout's own server of this name is judged a
            // browser on its own definition, where it is merged.
            continue;
        }
        let mut server = Map::new();
        match definition.transport {
            McpTransport::Stdio => {
                server.insert("command".into(), json!(definition.command));
                server.insert("args".into(), json!(definition.args));
                server.insert("transport".into(), json!("stdio"));
            }
            McpTransport::Http => {
                server.insert("url".into(), json!(definition.url));
            }
        }
        for (kind, values, field) in [
            ("ENV", &definition.env, "env"),
            ("HDR", &definition.headers, "headers"),
        ] {
            if values.is_empty() {
                continue;
            }
            let mut written = Map::new();
            for (key, value) in values {
                let name = placeholder(definition.id, kind, key);
                written.insert(key.clone(), json!(format!("${{{name}}}")));
                composed.environment.push((name, value.clone()));
            }
            server.insert(field.into(), Value::Object(written));
        }
        if let Some(tools) = &definition.include_tools {
            server.insert("includeTools".into(), json!(tools));
        }
        // Without this the server's tools sit behind the adapter's proxy and are not in
        // the model's tool list, so a seat told to use one sees no such tool and works
        // around it - which is the complaint this whole surface exists to answer. The
        // cost is the tool definitions on every turn, and the allow-list is the lever
        // for trimming it.
        server.insert("directTools".into(), json!(true));
        composed
            .servers
            .push((definition.name.clone(), Value::Object(server)));
    }
    composed
}

/// Put the composed servers into a generated config, and take out the names this scope
/// has switched off.
///
/// Inserted before ai-team's own generated definitions are written and after the
/// checkout's, so an app-owned row beats a checkout's server of the same name and loses
/// to nothing that carries an allow-list of ai-team's.
///
/// A reserved name is never removed, whatever the composer hands over: `clickup` and
/// `figma` are already in the map by the time this runs, and taking one out here would
/// turn a row on a settings page into a way of deleting a generated definition.
pub(crate) fn apply(config: &mut Value, composed: &Composed) -> Result<()> {
    let target = config
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::invalid("generated MCP config has no server map"))?;
    for name in &composed.suppressed {
        if RESERVED.contains(&name.as_str()) {
            continue;
        }
        target.remove(name);
    }
    for (name, server) in &composed.servers {
        target.insert(name.clone(), server.clone());
    }
    Ok(())
}

/// Write a generated config where only this machine's owner can read it.
///
/// The values are placeholders rather than credentials, so this is not what keeps a
/// secret - it is that a file naming every server a seat may reach has no business being
/// world-readable on a shared machine.
pub(crate) fn write_private(path: &Path, body: &str) -> Result<()> {
    std::fs::write(path, body).map_err(|error| Error::UnusablePath {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| Error::UnusablePath {
                path: path.to_path_buf(),
                reason: error.to_string(),
            },
        )?;
    }
    Ok(())
}

/// One of the operator's own Pi definitions, offered for review.
///
/// Values are read so an accepted candidate can be pinned whole, and never reported:
/// the keys say what would come across, which is what a decision needs. `command`,
/// `args` and `url` are the destination *as shown* - a URL carries its credentials in
/// the same string as its host, so what reaches a page has had them taken out, and the
/// unshown originals beside them are what an accepted candidate is pinned from.
#[derive(Debug, Clone, Serialize)]
pub struct McpImportCandidate {
    pub name: String,
    pub source: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    pub include_tools: Option<Vec<String>>,
    /// Whether this one can be pinned as it stands.
    pub importable: bool,
    /// Why not, or what is carried across - said plainly rather than left to be guessed.
    pub note: String,
    /// A server of this name is already configured in the scope being imported into.
    pub already: bool,
    #[serde(skip)]
    raw: Destination,
    #[serde(skip)]
    env: BTreeMap<String, String>,
    #[serde(skip)]
    headers: BTreeMap<String, String>,
}

/// Where a candidate actually points, before anything was taken out of it for display.
#[derive(Debug, Clone, Default)]
struct Destination {
    command: Option<String>,
    args: Vec<String>,
    url: Option<String>,
}

/// What a preview showed, and the token that pins an import to it.
///
/// The fingerprint covers the scope and everything the source files held - including the
/// values a surface is never shown, because a credential that changed is a different
/// thing to pin. An import quotes it back; anything else is a second read of files that
/// may have moved, which is the whole gap this closes.
#[derive(Debug, Clone, Serialize)]
pub struct McpImportPreview {
    /// The scope these candidates were read against. `None` is the app defaults.
    pub project_id: Option<i64>,
    pub fingerprint: String,
    pub candidates: Vec<McpImportCandidate>,
    /// A source that could not be read as configuration, said rather than skipped - a
    /// page that showed nothing would be reporting "you have no servers".
    pub problems: Vec<String>,
}

/// The operator's own user-level Pi MCP files.
///
/// Read, never written: a definition is copied out of them and nothing is copied back.
/// `PI_CODING_AGENT_DIR` is honoured because it is how Pi itself locates this directory,
/// and because it is what lets a test point at its own.
pub fn user_sources() -> Result<Vec<PathBuf>> {
    let dir = match std::env::var_os("PI_CODING_AGENT_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => crate::home_dir()?.join(".pi/agent"),
    };
    if !dir.is_absolute() {
        return Err(Error::invalid("the Pi agent directory must be absolute"));
    }
    Ok(vec![dir.join("mcp-adapter.json"), dir.join("mcp.json")])
}

/// How much of a Pi MCP file is read.
///
/// A configuration file is kilobytes. The bound is here because this is reached from a
/// GET on loopback, and a source that is not what it claims to be - a huge file, or
/// something that is not a file at all - must end in a sentence on the page rather than
/// in a read that does not return.
const MOST_SOURCE_BYTES: u64 = 1 << 20;

/// Read one source, or say why it was not read.
///
/// `metadata` follows symlinks, which is right: the operator may well symlink their Pi
/// configuration. What is refused is a FIFO or a device, because reading one blocks, and
/// a GET that blocks is a page that never loads.
fn read_source(path: &Path) -> std::result::Result<Option<String>, String> {
    use std::io::Read as _;
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        // Absent is the ordinary case: most machines have one of the two files.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    if !metadata.is_file() {
        return Err(format!(
            "{} is not a regular file, so AI Team did not read it",
            path.display()
        ));
    }
    if metadata.len() > MOST_SOURCE_BYTES {
        return Err(format!(
            "{} is larger than the {} KB AI Team reads, so it was not read",
            path.display(),
            MOST_SOURCE_BYTES / 1024
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(
            i32::try_from(rustix::fs::OFlags::NONBLOCK.bits())
                .expect("O_NONBLOCK fits the OS flags type"),
        );
    }
    let file = options
        .open(path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!(
            "{} changed or is not a regular file",
            path.display()
        ));
    }
    let mut body = String::new();
    file.take(MOST_SOURCE_BYTES + 1)
        .read_to_string(&mut body)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    if body.len() as u64 > MOST_SOURCE_BYTES {
        return Err(format!(
            "{} grew beyond the MCP import size limit",
            path.display()
        ));
    }
    Ok(Some(body))
}

/// What could be imported, with the reason beside anything that could not, and the
/// fingerprint that pins an import to this exact answer.
///
/// Reads files. Runs nothing, connects to nothing and resolves no credential, which is
/// the property that makes this safe to call from a GET.
pub fn import_candidates(store: &Store, project: Option<i64>) -> Result<McpImportPreview> {
    preview_in(&user_sources()?, store, project)
}

/// The same, against named files - which is how a test reads a fixture rather than the
/// operator's own definitions, and how this crate avoids mutating a process-wide
/// variable to arrange one.
pub fn preview_in(
    sources: &[PathBuf],
    store: &Store,
    project: Option<i64>,
) -> Result<McpImportPreview> {
    let existing: BTreeSet<String> = store
        .mcp_servers(project)?
        .into_iter()
        .filter(|server| project.is_none() || server.project_id == project)
        .map(|server| server.name)
        .collect();
    let mut found: Vec<McpImportCandidate> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for path in sources {
        let body = match read_source(path) {
            Ok(Some(body)) => body,
            Ok(None) => continue,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };
        let Ok(document) = serde_json::from_str::<Value>(&body) else {
            problems.push(format!(
                "{} is not JSON, so AI Team read nothing from it",
                path.display()
            ));
            continue;
        };
        let Some(servers) = document.get("mcpServers").and_then(Value::as_object) else {
            // No `mcpServers` map is the shape of a file that is about something else.
            // Pi ignores it too, so it is not a problem worth reporting.
            continue;
        };
        for (name, server) in servers {
            // The first file that declares a name is the one offered, matching the order
            // the adapter reads them in.
            let read = read_candidate(name, server, path);
            if found.iter().any(|candidate| candidate.name == read.name) {
                continue;
            }
            found.push(read);
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    // Bound to the sources and the scope, so it has to be taken before `already` - which
    // is a fact about the store, not about what was read.
    let fingerprint = fingerprint(project, &found, &problems);
    for candidate in &mut found {
        candidate.already = existing.contains(&candidate.name);
    }
    Ok(McpImportPreview {
        project_id: project,
        fingerprint,
        candidates: found,
        problems,
    })
}

/// Just the candidates, for the tests that are about what a definition reads as rather
/// than about what pins an import to it.
#[cfg(test)]
fn candidates_in(
    sources: &[PathBuf],
    store: &Store,
    project: Option<i64>,
) -> Result<Vec<McpImportCandidate>> {
    Ok(preview_in(sources, store, project)?.candidates)
}

/// What a preview was, in 32 characters.
///
/// Every field a candidate was read from goes in, values included: a header whose token
/// was rotated between the preview and the import is a different thing to pin, and the
/// person who approved the list did not see that happen. Each piece is length-prefixed,
/// so no rearrangement of two fields can produce the same digest as another.
fn fingerprint(
    project: Option<i64>,
    candidates: &[McpImportCandidate],
    problems: &[String],
) -> String {
    let mut hasher = Sha256::new();
    let mut feed = |label: &str, text: &str| {
        hasher.update(label.as_bytes());
        hasher.update(b" ");
        hasher.update(text.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(text.as_bytes());
        hasher.update(b"\n");
    };
    feed("version", "ai-team mcp import 1");
    feed(
        "scope",
        &project.map_or_else(|| "app".to_string(), |id| format!("project {id}")),
    );
    for problem in problems {
        feed("problem", problem);
    }
    for candidate in candidates {
        feed("name", &candidate.name);
        feed("source", &candidate.source);
        feed("transport", candidate.transport.as_str());
        feed("command", candidate.raw.command.as_deref().unwrap_or(""));
        for arg in &candidate.raw.args {
            feed("arg", arg);
        }
        feed("url", candidate.raw.url.as_deref().unwrap_or(""));
        for (key, value) in &candidate.env {
            feed("env", key);
            feed("env-value", value);
        }
        for (key, value) in &candidate.headers {
            feed("header", key);
            feed("header-value", value);
        }
        match &candidate.include_tools {
            Some(tools) => {
                for tool in tools {
                    feed("tool", tool);
                }
                feed("tools", "listed");
            }
            None => feed("tools", "all"),
        }
        feed(
            "importable",
            if candidate.importable { "yes" } else { "no" },
        );
        feed("note", &candidate.note);
    }
    format!("{:x}", hasher.finalize())[..32].to_string()
}

/// A destination with anything that reads like a credential taken out of it.
///
/// A URL carries its userinfo and its query in the same string as its host, so the one
/// field that says what a server reaches is also a field a token is written into. The
/// host and the ordinary parameters survive, because the point of showing a destination
/// is that somebody can check it by eye.
fn redact_url(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(at) => url.split_at(at + 3),
        None => ("", url),
    };
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let authority = match authority.rfind('@') {
        Some(at) => format!("***@{}", &authority[at + 1..]),
        None => authority.to_string(),
    };
    let tail = match tail.find('?') {
        Some(at) => {
            let (path, query) = tail.split_at(at + 1);
            let (query, fragment) = match query.find('#') {
                Some(at) => query.split_at(at),
                None => (query, ""),
            };
            let query: Vec<String> = query.split('&').map(redact_parameter).collect();
            format!("{path}{}{fragment}", query.join("&"))
        }
        None => tail.to_string(),
    };
    format!("{scheme}{authority}{tail}")
}

/// Query parameter names that are somebody's credential often enough to assume it. A
/// false positive costs a parameter nobody needed to read; a miss publishes a token.
const SECRETISH: &[&str] = &[
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "pwd",
    "auth",
    "credential",
    "signature",
    "sig",
    "session",
];

fn redact_parameter(pair: &str) -> String {
    let Some((key, value)) = pair.split_once('=') else {
        return pair.to_string();
    };
    if value.is_empty() {
        return pair.to_string();
    }
    let lowered = key.to_ascii_lowercase();
    if SECRETISH.iter().any(|word| lowered.contains(word)) {
        format!("{key}=***")
    } else {
        pair.to_string()
    }
}

/// A command word, which is a URL often enough to matter: `npx mcp-remote https://…`
/// puts an endpoint and its token in an argument.
///
/// On the scheme, not on the `@`: `chrome-devtools-mcp@latest` is a package version, and
/// an argument that came back as `***@latest` would be a destination nobody could check
/// by eye - which is the one thing showing it is for.
fn redact_word(word: &str) -> String {
    if word.contains("://") {
        redact_url(word)
    } else {
        word.to_string()
    }
}

/// Keys a definition may hold.
///
/// `command`, `args`, `url`, `env`, `headers` and `includeTools` are carried. The four
/// below them are ai-team's own dispatch knobs, set for every seat by the composer, so a
/// value in the source would be overridden rather than lost - which the note says.
/// Anything else is refused by name: a key nobody has read is a behaviour nobody has
/// decided about, and guessing it is harmless is how a capability arrives unannounced.
const CARRIED: &[&str] = &["command", "args", "url", "env", "headers", "includeTools"];
const OVERRIDDEN: &[&str] = &["transport", "type", "directTools", "lifecycle"];

/// The keys worth a sentence of their own, because what they do is not in their name.
const NAMED_UNSUPPORTED: &[(&str, &str)] = &[
    ("socket", "a Unix socket transport"),
    ("requestHeadersCommand", "a per-request header command"),
    ("bearerToken", "a literal bearer token"),
    ("bearerTokenEnv", "a bearer token from the environment"),
    ("bearerTokenStore", "a bearer token from Pi's own store"),
    ("excludeTools", "a block-list of tools"),
    (
        "auth",
        "a Pi provider's own token, which is user-global configuration",
    ),
    (
        "cwd",
        "a working directory, which AI Team gives a seat itself",
    ),
];

/// A list of strings, or the reason it is not one.
///
/// Filtering what does not parse is what this used to do, and it is the quiet half of an
/// import that arrives changed: `["--port", 8080]` became `["--port"]`, which is a
/// different command.
fn strings(server: &Value, field: &str) -> std::result::Result<Option<Vec<String>>, String> {
    let Some(value) = server.get(field) else {
        return Ok(None);
    };
    let Some(values) = value.as_array() else {
        return Err(format!("has a {field} that is not a list"));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                // The entry itself is not quoted back: a definition that put an object
                // where text belongs may well have put a credential in it.
                .ok_or_else(|| format!("has a {field} entry that is not text"))
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map(Some)
}

/// A map of names to values, or the reason it is not one. Same argument as `strings`: a
/// value that is an object is a credential Pi resolves some other way, and dropping it
/// produces a server that starts and cannot authenticate.
fn map(
    server: &Value,
    field: &str,
) -> std::result::Result<Option<BTreeMap<String, String>>, String> {
    let Some(value) = server.get(field) else {
        return Ok(None);
    };
    let Some(values) = value.as_object() else {
        return Err(format!("has a {field} block that is not a set of values"));
    };
    values
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_string()))
                .ok_or_else(|| format!("has a {field} value for {key} that is not text"))
        })
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()
        .map(Some)
}

/// A definition understood whole: nothing in it was skipped, trimmed or guessed at.
struct Understood {
    raw: Destination,
    env: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    include_tools: Option<Vec<String>>,
}

/// Everything about a definition that decides whether it can be copied exactly.
///
/// Separate from the candidate it fills in, so a refusal is one `?` rather than a chain
/// of assignments each of which has to remember to stop.
fn read_destination(
    server: &Value,
    transport: McpTransport,
) -> std::result::Result<Understood, String> {
    let Some(fields) = server.as_object() else {
        return Err("is not a server definition".into());
    };
    if let Some((key, what)) = NAMED_UNSUPPORTED
        .iter()
        .find(|(key, _)| fields.contains_key(*key))
    {
        return Err(format!(
            "uses {key}, {what}, which AI Team does not configure; add it here by hand if you need it"
        ));
    }
    if let Some(key) = fields
        .keys()
        .find(|key| !CARRIED.contains(&key.as_str()) && !OVERRIDDEN.contains(&key.as_str()))
    {
        return Err(format!(
            "sets {key}, which AI Team does not read; it would arrive as a server that behaves differently, so add it here by hand"
        ));
    }
    // `transport` and `type` are the same statement said twice. Carried only when they
    // agree with what the definition itself shows - `sse` is not the HTTP this configures.
    for key in ["transport", "type"] {
        if let Some(stated) = fields.get(key) {
            let Some(stated) = stated.as_str() else {
                return Err(format!("has a {key} that is not text"));
            };
            if !stated.eq_ignore_ascii_case(transport.as_str()) {
                return Err(format!(
                    "is a {stated} server, and AI Team configures stdio and http only"
                ));
            }
        }
    }
    let command = fields
        .get("command")
        .map(|command| {
            command
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "has a command that is not text".to_string())
        })
        .transpose()?;
    let url = fields
        .get("url")
        .map(|url| {
            url.as_str()
                .map(str::to_string)
                .ok_or_else(|| "has a url that is not text".to_string())
        })
        .transpose()?;
    let args = strings(server, "args")?.unwrap_or_default();
    let env = map(server, "env")?.unwrap_or_default();
    let headers = map(server, "headers")?.unwrap_or_default();
    let include_tools = strings(server, "includeTools")?;
    // A copy keeps the allow-list it was given or it is not a copy. `check_tools` drops
    // blanks and duplicates, which is right for somebody typing into the page and wrong
    // here, and an empty list would arrive meaning every tool - a server the operator
    // gave no tools to, offering all of them.
    if let Some(tools) = &include_tools {
        if check_tools(Some(tools)).as_ref() != Some(tools) {
            return Err(if tools.is_empty() {
                "has an empty tool allow-list, which AI Team would read as every tool".to_string()
            } else {
                "has a tool allow-list AI Team cannot copy exactly (a blank or repeated entry)"
                    .to_string()
            });
        }
    }
    Ok(Understood {
        raw: Destination { command, args, url },
        env,
        headers,
        include_tools,
    })
}

fn read_candidate(name: &str, server: &Value, source: &Path) -> McpImportCandidate {
    let transport = if server.get("url").is_some() {
        McpTransport::Http
    } else {
        McpTransport::Stdio
    };
    let name = name.trim().to_ascii_lowercase();
    let Understood {
        raw,
        env,
        headers,
        include_tools,
    } = match read_destination(server, transport) {
        Ok(read) => read,
        Err(problem) => {
            return McpImportCandidate {
                name,
                source: source.display().to_string(),
                transport,
                // Nothing was understood, so nothing is shown but the problem.
                command: None,
                args: Vec::new(),
                url: None,
                env_keys: Vec::new(),
                header_keys: Vec::new(),
                include_tools: None,
                importable: false,
                note: problem,
                already: false,
                raw: Destination::default(),
                env: BTreeMap::new(),
                headers: BTreeMap::new(),
            };
        }
    };
    let problem = check(Shape {
        name: &name,
        transport,
        command: raw.command.as_deref(),
        args: &raw.args,
        url: raw.url.as_deref(),
        env: &env,
        headers: &headers,
        include_tools: include_tools.as_deref(),
    })
    .err()
    .map(|problem| problem.to_string());
    let overridden: Vec<&str> = server
        .as_object()
        .map(|fields| {
            OVERRIDDEN
                .iter()
                .copied()
                .filter(|key| fields.contains_key(*key) && *key != "transport" && *key != "type")
                .collect()
        })
        .unwrap_or_default();
    let note = if let Some(problem) = &problem {
        problem.clone()
    } else {
        let pinned = if env.is_empty() && headers.is_empty() {
            "Pinned as a copy, switched off until you enable it."
        } else {
            "Pinned as a copy, with its stored values. Switched off until you enable it."
        };
        if overridden.is_empty() {
            pinned.to_string()
        } else {
            format!(
                "{pinned} AI Team sets {} itself, so that part is not carried across.",
                overridden.join(" and ")
            )
        }
    };
    McpImportCandidate {
        name,
        source: source.display().to_string(),
        transport,
        command: raw.command.as_deref().map(redact_word),
        args: raw.args.iter().map(|arg| redact_word(arg)).collect(),
        url: raw.url.as_deref().map(redact_url),
        env_keys: env.keys().cloned().collect(),
        header_keys: headers.keys().cloned().collect(),
        include_tools: include_tools.clone(),
        importable: problem.is_none(),
        note,
        already: false,
        raw,
        env,
        headers,
    }
}

/// Pin the previewed candidates into the store, disabled.
///
/// Disabled is the point: importing is reviewing what a server would be, and enabling is
/// agreeing to run its command or reach its endpoint. Three things this refuses rather
/// than works around: a `fingerprint` that is not the one the preview answered with,
/// because the files or the scope have moved and what would be pinned is not what was
/// approved; a name that is already configured, because the row that is there may be the
/// one the operator edited; and a partial result - every name is settled before any row
/// is written, and the rows are written together.
pub fn import(
    store: &mut Store,
    project: Option<i64>,
    names: &[String],
    fingerprint: &str,
) -> Result<Vec<McpServer>> {
    import_from(&user_sources()?, store, project, names, fingerprint)
}

pub fn import_from(
    sources: &[PathBuf],
    store: &mut Store,
    project: Option<i64>,
    names: &[String],
    fingerprint: &str,
) -> Result<Vec<McpServer>> {
    if names.is_empty() {
        return Err(Error::invalid("choose at least one server to import"));
    }
    if fingerprint.trim().is_empty() {
        return Err(Error::invalid(
            "this import did not say which list it was approved against; look again before importing",
        ));
    }
    let preview = preview_in(sources, store, project)?;
    if preview.fingerprint != fingerprint {
        return Err(Error::invalid(
            "your Pi MCP configuration or this scope has changed since that list was read; look again, and import from the list you can see",
        ));
    }
    let at = crate::now();
    let mut chosen: Vec<(NewMcpServer, String)> = Vec::new();
    for name in names {
        let name = name.trim().to_ascii_lowercase();
        if chosen.iter().any(|(new, _)| new.name == name) {
            return Err(Error::invalid(format!("{name} was chosen twice")));
        }
        let candidate = preview
            .candidates
            .iter()
            .find(|candidate| candidate.name == name)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{name} is not among your Pi definitions any more; look again before importing"
                ))
            })?;
        if !candidate.importable {
            return Err(Error::invalid(format!(
                "{name} cannot be imported: {}",
                candidate.note
            )));
        }
        if candidate.already {
            return Err(Error::invalid(format!(
                "{name} is already configured in this scope; edit that one instead"
            )));
        }
        chosen.push((
            NewMcpServer {
                project_id: project,
                name: candidate.name.clone(),
                enabled: false,
                transport: candidate.transport,
                command: candidate.raw.command.clone(),
                args: candidate.raw.args.clone(),
                url: candidate.raw.url.clone(),
                env: candidate.env.clone(),
                headers: candidate.headers.clone(),
                include_tools: candidate.include_tools.clone(),
            },
            format!("copied from {} on {at}", candidate.source),
        ));
    }
    store.import_mcp_servers(project, chosen)
}

/// Refuse a scope that has grown past what one config should hold.
pub(crate) fn check_room(held: usize, adding: usize) -> Result<()> {
    if held + adding > MOST_SERVERS {
        return Err(Error::invalid(format!(
            "a scope holds at most {MOST_SERVERS} servers, and this one holds {held}; remove one before adding another"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
