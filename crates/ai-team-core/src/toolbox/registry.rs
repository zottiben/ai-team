//! Read-only bounded discovery and reversible ai-team registration management.
use super::{
    operations::{Authority, SavedOperation},
    Operation,
};
use crate::{Error, Result, Store};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Root {
    pub path: String,
    pub exists: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub project: i64,
    pub name: String,
    pub revision: i64,
    pub status: String,
    pub next_status: String,
    pub restore_status: Option<String>,
    pub roots: Vec<Root>,
    pub reason: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistrySelection {
    Forget { projects: Vec<i64> },
    Restore { projects: Vec<i64> },
    Prune,
    ScanRoots { roots: Vec<String> },
}

pub fn preview_registry(store: &mut Store, selection: RegistrySelection) -> Result<Operation> {
    let mut saved = SavedOperation::new(Authority::Registry);
    let current = store.toolbox_registrations()?;
    let (ids, next) = match selection {
        RegistrySelection::Forget { projects } => (projects, "archived"),
        RegistrySelection::Restore { projects } => (projects, "restore"),
        RegistrySelection::Prune => (
            current
                .iter()
                .filter(|p| {
                    p.status != "archived"
                        && !p.roots.is_empty()
                        && p.roots.iter().all(|r| !r.exists)
                        && p.reason.is_none()
                })
                .map(|p| p.project)
                .collect(),
            "archived",
        ),
        RegistrySelection::ScanRoots { roots } => {
            if roots.len() > 16 {
                return Err(Error::invalid("at most 16 discovery roots"));
            }
            let mut normalized = BTreeSet::new();
            for root in roots {
                let path = Path::new(&root);
                if !path.is_absolute() || !path.is_dir() {
                    return Err(Error::invalid(
                        "discovery roots must be existing absolute directories",
                    ));
                }
                normalized.insert(path.canonicalize()?.to_string_lossy().into_owned());
            }
            saved.prior_scan_roots = store.toolbox_scan_roots()?;
            saved.scan_roots = Some(normalized.into_iter().collect());
            return store.save_toolbox_operation(&saved);
        }
    };
    for id in ids.into_iter().collect::<BTreeSet<_>>() {
        let mut change = current
            .iter()
            .find(|p| p.project == id)
            .cloned()
            .ok_or_else(|| Error::invalid("unknown project registration"))?;
        if let Some(reason) = &change.reason {
            return Err(Error::invalid(reason));
        }
        if next == "restore" {
            if change.status != "archived" {
                return Err(Error::invalid(
                    "only archived registrations can be restored",
                ));
            }
            change.next_status = change
                .restore_status
                .clone()
                .unwrap_or_else(|| "active".into());
        } else {
            if change.status == next {
                continue;
            }
            change.next_status = next.into();
        }
        saved.registrations.push(change);
    }
    saved.warnings.push("Forget/prune hides registrations from the active project list (archive), not disk deletion. All chats, runs, teams, planning data, setup receipts and checkout paths remain intact. Restore makes them visible again. Active work, retained leases, pending delivery and scheduled work block forgetting.".into());
    store.save_toolbox_operation(&saved)
}

#[derive(Debug, Serialize)]
pub struct Discovered {
    pub path: String,
    pub project: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct Discovery {
    pub roots: Vec<String>,
    pub repositories: Vec<Discovered>,
    pub warnings: Vec<String>,
}
pub fn discover(store: &Store, roots: Vec<String>) -> Result<Discovery> {
    if roots.is_empty() || roots.len() > 16 {
        return Err(Error::invalid("choose 1–16 discovery roots"));
    }
    let known = store.toolbox_registrations()?;
    let mut result = Discovery {
        roots: vec![],
        repositories: vec![],
        warnings: vec![],
    };
    let mut pending = Vec::new();
    for root in roots {
        let p = Path::new(&root);
        if !p.is_absolute() {
            return Err(Error::invalid("discovery roots must be absolute"));
        }
        let path = p.canonicalize()?;
        if !path.is_dir() {
            return Err(Error::invalid("discovery root is not a directory"));
        }
        result.roots.push(path.to_string_lossy().into_owned());
        pending.push((path, 0));
    }
    let mut visited = BTreeSet::new();
    let mut entries = 0;
    'scan: while let Some((path, depth)) = pending.pop() {
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            result.warnings.push(format!(
                "Skipped directory replaced by a symlink: {}",
                path.display()
            ));
            continue;
        }
        if !visited.insert(path.clone()) {
            continue;
        }
        if result.repositories.len() >= 200 || entries >= 4096 {
            result.warnings.push("Discovery limit reached (200 repositories/4096 entries); narrow the roots and scan again.".into());
            break;
        }
        if fs::symlink_metadata(path.join(".git")).is_ok() {
            let text = path.to_string_lossy().into_owned();
            let project = known
                .iter()
                .find(|p| {
                    p.roots
                        .iter()
                        .any(|r| Path::new(&r.path).canonicalize().is_ok_and(|p| p == path))
                })
                .map(|p| p.project);
            result.repositories.push(Discovered {
                path: text,
                project,
            });
            continue;
        }
        if depth >= 6 {
            result
                .warnings
                .push(format!("Depth limit at {}", path.display()));
            continue;
        }
        let children = match fs::read_dir(&path) {
            Ok(v) => v,
            Err(e) => {
                result.warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        for child in children {
            entries += 1;
            if entries > 4096 {
                result
                    .warnings
                    .push("Discovery entry limit reached; narrow the roots and scan again.".into());
                break 'scan;
            }
            let child = child?;
            let name = child.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || ["node_modules", "target", "vendor", "dist"].contains(&name.as_ref())
            {
                continue;
            }
            if child.file_type()?.is_dir() {
                pending.push((child.path(), depth + 1));
            }
        }
    }
    result.repositories.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}
