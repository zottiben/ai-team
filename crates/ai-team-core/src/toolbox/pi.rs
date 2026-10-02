//! Keep the upstream engine's conversion rules, but use the current adapter's
//! override layer. Do not migrate or rewrite Pi's own .pi/mcp.json implicitly.

use std::{fs, path::Path};

use ai_toolbox_core::{Action, Plan};
use serde_json::{json, Value};

use crate::{Error, Result};

pub(super) fn adapt(root: &Path, plan: &mut Plan) -> Result<()> {
    let old = root.join(".pi/mcp.json");
    if let Some(at) = plan.actions.iter().position(|a| a.path == old) {
        let action = plan.actions.remove(at);
        let ai_toolbox_core::action::Kind::Write { contents, .. } = action.kind else {
            return Err(Error::invalid("unexpected Pi override action"));
        };
        let converted: Value = serde_json::from_slice(&contents)?;
        let servers = converted
            .get("mcpServers")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::invalid("Pi overrides need mcpServers"))?;
        let path = root.join(".pi/mcp-adapter.json");
        let mut document = read(&path)?;
        ai_toolbox_core::merge::mcp_servers(&mut document, servers);
        plan.push(Action::write(
            path,
            ai_toolbox_core::merge::to_string(&document),
            "Pi-only overrides in .pi/mcp-adapter.json",
        )?);
    }
    for warning in &mut plan.warnings {
        *warning = warning.replace(".pi/mcp.json", ".pi/mcp-adapter.json");
    }
    Ok(())
}

pub(super) fn survey(root: &Path, survey: &mut Value) -> Result<()> {
    let document = read(&root.join(".pi/mcp-adapter.json"))?;
    survey["inventory"]["pi"]["adapter_servers"] = json!(document
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|s| s.keys().collect::<Vec<_>>())
        .unwrap_or_default());
    survey["recommendation"]["notes"].as_array_mut().ok_or_else(|| Error::invalid("toolbox recommendation has no notes"))?.push(json!("Pi shell hooks are not wired by this catalogue. Shared MCP uses .mcp.json; adapter-specific overrides use .pi/mcp-adapter.json. Existing .pi/mcp.json is left unchanged. MCP clients and server prerequisites must already be installed; credentials are never installed by setup."));
    Ok(())
}

fn read(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(body) => {
            let parsed: Value = serde_json::from_str(&body).map_err(|error| Error::invalid(format!("{}: {error}; preserve comments or repair the JSON manually before previewing setup", path.display())))?;
            if !parsed.is_object() || parsed.get("mcpServers").is_some_and(|v| !v.is_object()) {
                return Err(Error::invalid(
                    "Pi adapter configuration needs an object with an optional mcpServers object",
                ));
            }
            Ok(parsed)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.into()),
    }
}
