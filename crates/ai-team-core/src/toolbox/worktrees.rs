//! Read-only comparison against the registered checkout. Git discovery uses ai-team's
//! bounded, sanitized runner, not the toolbox engine's unbounded subprocess helper.

use std::{path::Path, process::Command, time::Duration};

use serde::Serialize;

use super::{files::Snapshot, Node};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub different: Vec<String>,
    pub problem: Option<String>,
}

pub(super) fn inspect(root: &Path) -> Result<Vec<Worktree>> {
    if !root.join(".git").exists() {
        return Ok(Vec::new());
    }
    let root = root.canonicalize()?;
    let reference = Snapshot::capture(&root)?;
    let mut command = Command::new("git");
    command.current_dir(&root).args([
        "-c",
        "core.fsmonitor=false",
        "worktree",
        "list",
        "--porcelain",
        "-z",
    ]);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0");
    let output = crate::command::run_blocking(command, Duration::from_secs(5), 256 * 1024)?;
    if !output.status.success() || output.truncated {
        return Err(Error::invalid(format!(
            "worktree inventory unavailable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let listed = String::from_utf8(output.stdout)
        .map_err(|_| Error::invalid("non-UTF8 worktree paths require manual inspection"))?;
    let mut records = Vec::new();
    for record in listed.split("\0\0") {
        let fields: Vec<_> = record.split('\0').collect();
        let Some(path) = fields.iter().find_map(|f| f.strip_prefix("worktree ")) else {
            continue;
        };
        if records.len() >= 64 {
            return Err(Error::invalid(
                "more than 64 worktrees; inspect the remaining worktrees manually",
            ));
        }
        let branch = fields
            .iter()
            .find_map(|f| f.strip_prefix("branch refs/heads/"))
            .map(str::to_owned);
        let compared = (|| -> Result<Vec<String>> {
            let here = Snapshot::capture(Path::new(path))?;
            let mut different = Vec::new();
            for name in [
                "AGENTS.md",
                "CLAUDE.md",
                ".mcp.json",
                ".agents",
                ".claude/settings.json",
                ".claude/skills",
                ".codex/config.toml",
                ".pi/mcp.json",
                ".pi/mcp-adapter.json",
            ] {
                let wanted = reference.nodes.get(name).unwrap_or(&Node::Missing);
                if here.nodes.get(name).unwrap_or(&Node::Missing) != wanted {
                    different.push(name.into());
                }
            }
            here.validate()?;
            Ok(different)
        })();
        let (different, problem) = match compared {
            Ok(d) => (d, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        records.push(Worktree {
            path: path.into(),
            branch,
            different,
            problem,
        });
    }
    reference.validate()?;
    Ok(records)
}
