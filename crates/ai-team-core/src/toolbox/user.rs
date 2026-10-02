//! User setup is selected explicitly, independent of every project registration.
use super::{
    effects::Frozen,
    files::{self, Snapshot},
    install,
    operations::{self, Authority, SavedOperation},
    Operation,
};
use crate::{Error, Result, Store};
use ai_toolbox_core::{Harness, Plan};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSelection {
    pub harnesses: Vec<Harness>,
    pub skills: Vec<String>,
    pub no_symlink: bool,
    pub charter: bool,
    pub charter_path: Option<String>,
}

pub fn user_authority() -> Result<Authority> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| Error::invalid("HOME is not configured; user setup is unavailable"))?;
    let home = PathBuf::from(home).canonicalize()?;
    let pi_agent = std::env::var_os("PI_CODING_AGENT_DIR")
        .map_or_else(|| home.join(".pi/agent"), PathBuf::from);
    if !pi_agent.is_absolute() {
        return Err(Error::invalid("Pi agent directory must be absolute"));
    }
    Ok(Authority::User {
        home,
        pi_agent,
        charter_targets: vec![],
    })
}

/// Resolve a named file through an existing ancestor without creating anything.
/// Retain the spelling too: approval checks aliases have not been retargeted.
pub(super) fn address(path: &Path) -> Result<(PathBuf, String)> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::invalid(
            "choose an absolute path without dot components",
        ));
    }
    let mut parent = path
        .parent()
        .ok_or_else(|| Error::invalid("choose a file, not the filesystem root"))?;
    while !parent.exists() {
        parent = parent
            .parent()
            .ok_or_else(|| Error::invalid("no existing target parent"))?;
    }
    let relative = files::text(
        path.strip_prefix(parent)
            .map_err(|_| Error::invalid("invalid target path"))?,
    )?;
    Ok((parent.canonicalize()?, relative))
}

fn append_charter(
    plan: &mut Plan,
    catalogue: &ai_toolbox_core::Catalogue,
    harnesses: &[Harness],
    snapshot: &Snapshot,
    stage: &Path,
    relative: &str,
) -> Result<()> {
    match snapshot.before(relative)? {
        super::Node::Missing => Ok(()),
        super::Node::File { contents, .. } => {
            std::str::from_utf8(&contents).map(|_| ()).map_err(|_| {
                Error::invalid("charter target is not UTF-8; preserve it and choose a text file")
            })
        }
        _ => Err(Error::invalid("charter target must be a regular text file")),
    }?;
    // Explicit target avoids global discovery. The engine may abbreviate staging
    // paths under HOME as ~/...; descriptions must name the approved destination.
    let start = plan.actions.len();
    ai_toolbox_core::install::base_charter(
        plan,
        catalogue,
        harnesses,
        Some(&stage.join(relative)),
    )?;
    for action in &mut plan.actions[start..] {
        action.summary = format!(
            "append base charter -> {}",
            snapshot.root.join(relative).display()
        );
    }
    Ok(())
}

pub fn preview_user(
    store: &mut Store,
    authority: Authority,
    selection: UserSelection,
) -> Result<Operation> {
    let Authority::User { home, pi_agent, .. } = &authority else {
        return Err(Error::invalid("user setup needs user scope"));
    };
    if selection.harnesses.is_empty() {
        return Err(Error::invalid("select at least one harness"));
    }
    if selection.skills.is_empty() && !selection.charter {
        return Err(Error::invalid("select skills or the base charter"));
    }
    let home = home.canonicalize()?;
    let package = operations::package()?;
    let mut inputs: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    if !selection.skills.is_empty() {
        let mut paths = vec![".agents/skills".into()];
        if selection.harnesses.contains(&Harness::Claude) {
            paths.push(".claude/skills".into());
        }
        inputs.insert(home.clone(), paths);
    }
    let mut charter_targets = Vec::new();
    let mut aliases = Vec::new();
    if selection.charter {
        let targets = match selection.charter_path {
            Some(path) if !path.trim().is_empty() => vec![PathBuf::from(path)],
            _ => selection
                .harnesses
                .iter()
                .map(|h| h.charter_path(&home, pi_agent))
                .collect(),
        };
        for path in targets {
            let (base, rest) = address(&path)?;
            let resolved = base.join(rest);
            aliases.push((path, resolved.clone()));
            let (base, rest) = if let Ok(relative) = resolved.strip_prefix(&home) {
                (home.clone(), files::text(relative)?)
            } else {
                address(&resolved)?
            };
            let entries = inputs.entry(base.clone()).or_default();
            if !entries.contains(&rest) {
                entries.push(rest.clone());
                charter_targets.push((base, rest));
            }
        }
    }
    let authority = Authority::User {
        home: home.clone(),
        pi_agent: pi_agent.clone(),
        charter_targets: aliases
            .iter()
            .map(|(_, resolved)| resolved.clone())
            .collect(),
    };
    let mut saved = SavedOperation::new(authority);
    saved.aliases = aliases;
    saved.warnings.push("USER scope: these files affect future sessions in every project for the selected harnesses. No global MCP registrations, package installations or credentials are changed. Existing charter markers are left unchanged.".into());
    for (base, paths) in inputs {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let snapshot = Snapshot::capture_only(&base, &paths)?;
        let stage = snapshot.stage()?;
        let mut plan = Plan::default();
        if base == home && !selection.skills.is_empty() {
            install::skills(
                &mut plan,
                stage.path(),
                &package.catalogue,
                &selection.skills,
                &selection.harnesses,
                selection.no_symlink,
            )?;
        }
        for (_, relative) in charter_targets.iter().filter(|(root, _)| *root == base) {
            append_charter(
                &mut plan,
                &package.catalogue,
                &selection.harnesses,
                &snapshot,
                stage.path(),
                relative,
            )?;
        }
        saved.warnings.extend(plan.warnings);
        saved
            .files
            .push(Frozen::capture(snapshot, stage.path(), plan.actions)?);
    }
    // Independent groups must not overlap: otherwise one would invalidate another's
    // guards after the first write, or hide a wider scope in a skill-directory effect.
    for (i, a) in saved.files.iter().enumerate() {
        for b in saved.files.iter().skip(i + 1) {
            if a.inputs.root.starts_with(&b.inputs.root)
                || b.inputs.root.starts_with(&a.inputs.root)
            {
                return Err(Error::invalid(
                    "overlapping user setup roots; preview these operations separately",
                ));
            }
        }
    }
    store.save_toolbox_operation(&saved)
}
