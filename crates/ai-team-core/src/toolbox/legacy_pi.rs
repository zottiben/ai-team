//! Explicit legacy import. Never asks the old engine to drop transport conflicts.
use crate::{Error, Result};
use ai_toolbox_core::{Action, Plan};
use serde_json::Value;
use std::{fs, path::Path};

fn read(path: &Path, optional: bool) -> Result<Value> {
    let text = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::json!({}))
        }
        Err(e) => {
            return Err(Error::invalid(format!(
                "{}: {e}; no legacy import was made",
                path.display()
            )))
        }
    };
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        Error::invalid(format!(
            "{}: {e}; keep comments and resolve this file manually before importing",
            path.display()
        ))
    })?;
    if !value.is_object() {
        return Err(Error::invalid("MCP configuration must be an object"));
    }
    Ok(value)
}
fn merge(to: &mut Value, from: Value, path: &str) -> Result<()> {
    if *to == from {
        return Ok(());
    }
    if let (Some(a), Some(b)) = (to.as_object_mut(), from.as_object()) {
        for (key, value) in b {
            if let Some(existing) = a.get_mut(key) {
                merge(existing, value.clone(), &format!("{path}/{key}"))?;
            } else {
                a.insert(key.clone(), value.clone());
            }
        }
        return Ok(());
    }
    Err(Error::invalid(format!(
        "legacy Pi import conflict at {path}; preserve both versions and reconcile manually"
    )))
}
pub(super) fn plan(root: &Path) -> Result<Plan> {
    let mut source = read(&root.join(".pi/mcp.json"), false)?;
    let mut target = read(&root.join(".pi/mcp-adapter.json"), true)?;
    let shared = read(&root.join(".mcp.json"), true)?;
    let servers = source
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::invalid("legacy Pi import requires an mcpServers object"))?;
    for (name, entry) in servers {
        let entry = entry
            .as_object_mut()
            .ok_or_else(|| Error::invalid("legacy server must be an object"))?;
        if entry.contains_key("exposure") || entry.contains_key("autoEnableCodemode") {
            return Err(Error::invalid(
                "native Pi MCP settings are not legacy adapter settings; keep them in .pi/mcp.json",
            ));
        }
        if let Some(transport) = entry.remove("transport") {
            if !matches!(transport.as_str(), Some("sse" | "streamable-http")) {
                return Err(Error::invalid(format!(
                    "unknown legacy transport for {name}; inspect it manually"
                )));
            }
            if entry.get("httpTransport").is_some_and(|v| *v != transport) {
                return Err(Error::invalid(format!(
                    "conflicting transport settings for {name}"
                )));
            }
            entry.insert("httpTransport".into(), transport);
        }
        if let Some(auth) = entry.get("auth").and_then(Value::as_object) {
            if auth.len() == 1 && auth.get("type").and_then(Value::as_str) == Some("oauth") {
                entry.insert("auth".into(), Value::String("oauth".into()));
            } else if !auth.contains_key("provider") {
                return Err(Error::invalid(format!(
                    "unknown legacy auth settings for {name}; no fields were dropped"
                )));
            }
        }
        if entry.contains_key("url") && entry.contains_key("command") {
            return Err(Error::invalid(format!(
                "mixed transport for {name}; reconcile before importing"
            )));
        }
        if let Some(base) = shared.get("mcpServers").and_then(|s| s.get(name)) {
            let mismatch = (entry.contains_key("url") && base.get("command").is_some())
                || (entry.contains_key("command") && base.get("url").is_some())
                || entry
                    .get("url")
                    .zip(base.get("url"))
                    .is_some_and(|(a, b)| a != b);
            if mismatch {
                return Err(Error::invalid(format!("shared/legacy transport conflict for {name}; keep both files and reconcile the destination before importing")));
            }
        }
    }
    merge(&mut target, source, "config")?;
    if let Some(servers) = target.get("mcpServers").and_then(Value::as_object) {
        for (name, entry) in servers {
            if ["command", "url", "socket"]
                .iter()
                .filter(|key| entry.get(**key).is_some())
                .count()
                > 1
            {
                return Err(Error::invalid(format!("destination transport conflict for {name}; preserve both configurations and reconcile manually")));
            }
        }
    }
    let mut plan = Plan::default();
    plan.push(Action::write(
        root.join(".pi/mcp-adapter.json"),
        ai_toolbox_core::merge::to_string(&target),
        "import legacy adapter configuration; normalize transport/auth without dropping fields",
    )?);
    plan.warn("Explicit Pi adapter import only. Shared MCP and the source .pi/mcp.json remain unchanged (current Pi can also read native MCP there). Adapter-only settings now live in mcp-adapter.json. No server is started or credential installed; inspect unsupported runtime-specific settings before connecting.");
    Ok(plan)
}
