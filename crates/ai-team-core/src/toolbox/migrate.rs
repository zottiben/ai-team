//! Preserve complete trees before asking the engine to re-point harness configs.
//! Its legacy mover drops differing hooks and unrecognised skill files, and its
//! Pi fold predates the current adapter. Neither may decide which local edits win.

use std::{fs, path::Path};

use ai_toolbox_core::{action::Kind, Action, Plan};
use serde_json::Value;

use super::files::{self, Node, Snapshot};
use crate::{Error, Result};

const MOVES: &[(&str, &str)] = &[
    (".claude/hooks", ".agents/hooks"),
    (".codex/hooks", ".agents/hooks"),
    (".claude/mcp", ".agents/mcp"),
    (".codex/mcp", ".agents/mcp"),
    (".pi/mcp", ".agents/mcp"),
    (".claude/skills", ".agents/skills"),
];

pub(super) fn plan(inputs: &Snapshot, staged: &Path) -> Result<Plan> {
    let mut plan = Plan::default();
    for (source, destination) in MOVES {
        let from = staged.join(source);
        let to = staged.join(destination);
        let source_node = Node::read(&from)?;
        if source_node == Node::Missing || matches!(source_node, Node::Symlink { .. }) {
            continue;
        }
        if !matches!(source_node, Node::Directory { .. }) {
            return Err(Error::invalid(format!(
                "layout migration needs a directory at {source}"
            )));
        }
        let mut combined = Node::read(&to)?;
        merge(&mut combined, &source_node, destination)?;
        replace(&to, &combined)?;
        fs::remove_dir_all(&from)?;
        plan.warn(format!(
            "Move {source} to {destination}; preserve complete contents and permissions."
        ));
    }
    let skills = staged.join(".claude/skills");
    if staged.join(".agents/skills").is_dir() && Node::read(&skills)? == Node::Missing {
        replace(
            &skills,
            &Node::Symlink {
                target: "../.agents/skills".into(),
            },
        )?;
    }

    // Legacy override folding can discard conflicting transport/custom fields and
    // expose Pi-only headers to other harnesses. Keep both configs private to Pi.
    let old_pi = staged.join(".pi/mcp.json");
    let old = Node::read(&old_pi)?;
    if old != Node::Missing {
        fs::remove_file(&old_pi)?;
    }
    let mut rewrites = Plan::default();
    let report = ai_toolbox_core::migrate::plan(&mut rewrites, staged)?;
    for action in rewrites.actions {
        let relative = files::relative(staged, &action.path)?;
        if matches!(action.kind, Kind::Remove)
            && [".claude", ".codex", ".pi"].contains(&relative.as_str())
        {
            // Other harness files weren't inspected. Never prune their parent.
            continue;
        }
        inputs.before(&relative)?;
        match action.kind {
            Kind::Write { contents, mode } => {
                let mode = mode.unwrap_or(match Node::read(&action.path)? {
                    Node::File { mode, .. } => mode,
                    _ => 0o644,
                });
                replace(&action.path, &Node::File { contents, mode })?;
            }
            _ => {
                return Err(Error::invalid(format!(
                    "unexpected engine migration action at {relative}; no project files changed"
                )))
            }
        }
    }
    plan.warnings.extend(report.notes);
    plan.warnings.extend(report.conflicts);
    replace(&old_pi, &old)?;
    for relative in [".pi/mcp.json", ".pi/mcp-adapter.json"] {
        repoint_pi(&staged.join(relative))?;
    }
    if old != Node::Missing {
        plan.warn("Legacy .pi/mcp.json is preserved, not imported or folded into shared MCP. Current Pi reads .pi/mcp-adapter.json. Review any obsolete transport/auth settings manually; layout migration only re-points moved helper paths.");
    }
    // Collapse moves to disjoint inspected roots. Removing children followed by
    // their parent would otherwise compare the parent against already-changed bytes.
    for (relative, before) in &inputs.nodes {
        let path = staged.join(relative);
        let after = Node::read(&path)?;
        if *before == after {
            continue;
        }
        let kind = match after {
            Node::Missing => Kind::Remove,
            Node::File { contents, mode } => Kind::Write {
                contents,
                mode: Some(mode),
            },
            Node::Directory { .. } => Kind::CopyTree { from: path.clone() },
            Node::Symlink { target } => Kind::Symlink { target },
        };
        plan.push(Action {
            path,
            kind,
            summary: format!("migrate shared layout: {relative}"),
            noop: false,
            expect: None,
        });
    }
    Ok(plan)
}

fn merge(target: &mut Node, source: &Node, path: &str) -> Result<()> {
    if *target == Node::Missing {
        *target = source.clone();
        return Ok(());
    }
    if target == source {
        return Ok(());
    }
    if let (
        Node::Directory {
            entries: to,
            mode: a,
        },
        Node::Directory {
            entries: from,
            mode: b,
        },
    ) = (&mut *target, source)
    {
        if a == b {
            for (name, node) in from {
                merge(
                    to.entry(name.clone()).or_insert(Node::Missing),
                    node,
                    &format!("{path}/{name}"),
                )?;
            }
            return Ok(());
        }
    }
    Err(Error::invalid(format!("layout migration conflict at {path}: differing contents, types or permissions; reconcile by hand first. No project files changed.")))
}

/// Only ever called on the private planning copy, never on the approved root.
fn replace(path: &Path, node: &Node) -> Result<()> {
    match Node::read(path)? {
        Node::Missing => {}
        Node::Directory { .. } => fs::remove_dir_all(path)?,
        _ => fs::remove_file(path)?,
    }
    if *node != Node::Missing {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        node.materialize(path)?;
    }
    Ok(())
}

fn repoint_pi(path: &Path) -> Result<()> {
    fn command(value: &mut Value) {
        if let Value::String(text) = value {
            *text = ai_toolbox_core::convert::canonical_helper_path(text);
        }
    }
    let Node::File { contents, mode } = Node::read(path)? else {
        return Ok(());
    };
    let document: Value = serde_json::from_slice(&contents).map_err(|error| {
        Error::invalid(format!(
            "{}: {error}; preserve comments or repair the JSON manually before migrating",
            path.display()
        ))
    })?;
    let mut rewritten = document.clone();
    if let Some(servers) = rewritten
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
    {
        for server in servers.values_mut() {
            if let Some(value) = server.get_mut("command") {
                command(value);
            }
            if let Some(args) = server.get_mut("args").and_then(Value::as_array_mut) {
                for value in args {
                    command(value);
                }
            }
            if let Some(headers) = server.get_mut("headers").and_then(Value::as_object_mut) {
                for value in headers.values_mut() {
                    if value.as_str().is_some_and(|text| text.starts_with('!')) {
                        command(value);
                    }
                }
            }
        }
    }
    if rewritten != document {
        replace(
            path,
            &Node::File {
                contents: ai_toolbox_core::merge::to_string(&rewritten).into_bytes(),
                mode,
            },
        )?;
    }
    Ok(())
}
