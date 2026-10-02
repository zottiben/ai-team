//! New authority scopes share the same frozen filesystem effect executor as project setup.
use super::{
    catalogue,
    effects::Frozen,
    files::{self, Snapshot},
    worktrees, Effect, Node, Outcome,
};
use crate::{Error, Result, Store};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Authority {
    User {
        home: PathBuf,
        pi_agent: PathBuf,
        charter_targets: Vec<PathBuf>,
    },
    Converge {
        project: i64,
        reference: PathBuf,
        target: PathBuf,
    },
    Registry,
}
impl Authority {
    fn same_user(&self, current: Option<&Self>) -> bool {
        matches!((self,current),(Self::User{home,pi_agent,..},Some(Self::User{home:other,pi_agent:agent,..})) if home==other && pi_agent==agent)
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Converge { .. } => "converge",
            Self::Registry => "registry",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    pub root: String,
    pub effects: Vec<Effect>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: i64,
    pub authority: Authority,
    pub changes: Vec<ChangeSet>,
    pub registrations: Vec<super::registry::Change>,
    pub scan_roots: Option<Vec<String>>,
    pub warnings: Vec<String>,
    pub state: String,
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: i64,
    pub authority: Authority,
    pub state: String,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedOperation {
    pub authority: Authority,
    pub files: Vec<Frozen>,
    pub guards: Vec<Snapshot>,
    pub aliases: Vec<(PathBuf, PathBuf)>,
    pub registrations: Vec<super::registry::Change>,
    pub scan_roots: Option<Vec<String>>,
    pub prior_scan_roots: Vec<String>,
    pub warnings: Vec<String>,
}
impl SavedOperation {
    pub(super) fn new(authority: Authority) -> Self {
        Self { authority, files: vec![], guards: vec![], aliases:vec![], registrations: vec![], scan_roots: None, prior_scan_roots: vec![], warnings: vec!["Apply only this saved approval. No software, credentials, model turns, Git commits or publication. Filesystem changes are not a multi-file transaction; interrupted outcomes require inspection, never automatic replay.".into()] }
    }
    pub(crate) fn view(self, id: i64, state: String, outcome: Option<Outcome>) -> Operation {
        Operation {
            id,
            authority: self.authority,
            changes: self
                .files
                .into_iter()
                .map(|f| ChangeSet {
                    root: f.inputs.root.to_string_lossy().into_owned(),
                    effects: f.effects,
                })
                .collect(),
            registrations: self.registrations,
            scan_roots: self.scan_roots,
            warnings: self.warnings,
            state,
            outcome,
        }
    }
}

pub fn apply_operation(
    store: &mut Store,
    id: i64,
    kind: &str,
    current_user: Option<&Authority>,
) -> Result<Operation> {
    let saved = store.claim_toolbox_operation(id, kind)?;
    let mut outcome = Outcome::default();
    let checked = (|| -> Result<()> {
        match &saved.authority {
            Authority::User { .. } if !saved.authority.same_user(current_user) => {
                return Err(Error::invalid(
                    "user setup destination changed; make a fresh user preview",
                ))
            }
            Authority::Converge {
                project,
                reference,
                target,
            } => {
                store.toolbox_root(*project, files::text(reference)?.as_str())?;
                require_linked(reference, target)?;
                store.check_toolbox_target(*project, files::text(target)?.as_str(), Some(id))?;
            }
            _ => {}
        }
        for (spelling, expected) in &saved.aliases {
            let (base, rest) = super::user::address(spelling)?;
            if base.join(rest) != *expected {
                return Err(Error::invalid("user target alias changed; preview again"));
            }
        }
        for guard in &saved.guards {
            guard.validate()?;
        }
        for frozen in &saved.files {
            frozen.inputs.validate()?;
        }
        Ok(())
    })();
    if let Err(error) = checked {
        outcome.problem = Some(error.to_string());
    } else if saved.authority == Authority::Registry {
        match store.apply_toolbox_registry(&saved) {
            Ok(changed) => outcome.applied = changed,
            Err(error) => outcome.problem = Some(error.to_string()),
        }
    } else {
        for frozen in &saved.files {
            let next = frozen.apply();
            outcome.applied.extend(
                next.applied
                    .iter()
                    .map(|p| frozen.inputs.root.join(p).to_string_lossy().into_owned()),
            );
            if next.problem.is_some() {
                outcome.uncertain = next.uncertain || !outcome.applied.is_empty();
                outcome.problem = next.problem;
                break;
            }
        }
    }
    store.finish_toolbox_operation(id, &outcome)?;
    store.toolbox_operation(id, kind)
}

fn require_linked(reference: &Path, target: &Path) -> Result<()> {
    if reference == target {
        return Err(Error::invalid("choose a different linked worktree"));
    }
    for (a, b) in [(reference, target), (target, reference)] {
        if !worktrees::inspect(a)?
            .iter()
            .any(|w| Path::new(&w.path).canonicalize().is_ok_and(|p| p == b) && w.problem.is_none())
        {
            return Err(Error::invalid(
                "target is not a usable linked Git worktree of the reference",
            ));
        }
    }
    Ok(())
}

pub fn preview_convergence(
    store: &mut Store,
    project: i64,
    reference: &str,
    target: &str,
) -> Result<Operation> {
    let reference = store.toolbox_root(project, reference)?;
    let target = Path::new(target).canonicalize()?;
    require_linked(&reference, &target)?;
    store.check_toolbox_target(project, files::text(&target)?.as_str(), None)?;
    let source = Snapshot::capture_only(&reference, worktrees::MANAGED)?;
    let inputs = Snapshot::capture_only(&target, worktrees::MANAGED)?;
    let stage = inputs.stage()?;
    let mut plan = ai_toolbox_core::Plan::default();
    for &path in worktrees::MANAGED {
        let from = source.before(path)?;
        let mut to = inputs.before(path)?;
        if from == Node::Missing {
            continue;
        }
        if path == ".claude/skills"
            && ((matches!(from, Node::Symlink { .. }) && matches!(to, Node::Directory { .. }))
                || (matches!(from, Node::Directory { .. }) && matches!(to, Node::Symlink { .. })))
        {
            plan.warn("Kept the target's Claude skills link/copy layout. Its layout differs from the reference; migrate or install its skills separately. This path was not converged.");
            continue;
        }
        overlay(&mut to, &from, path)?;
        if inputs.before(path)? != to {
            stage_node(&stage.path().join(path), &to)?;
            plan.push(node_action(
                stage.path().join(path),
                to,
                format!("converge {path}; replace matching files, retain target-only paths"),
            ));
        }
    }
    let mut saved = SavedOperation::new(Authority::Converge {
        project,
        reference,
        target,
    });
    saved.warnings.push("Convergence replaces matching files in full, including MCP/settings files: inspect every before/after. Target-only filesystem paths are kept; target-only fields inside a replaced file are not merged. Missing reference paths never delete target files.".into());
    saved.warnings.extend(plan.warnings);
    saved.guards.push(source);
    saved
        .files
        .push(Frozen::capture(inputs, stage.path(), plan.actions)?);
    for guard in &saved.guards {
        guard.validate()?;
    }
    store.save_toolbox_operation(&saved)
}
fn overlay(target: &mut Node, source: &Node, path: &str) -> Result<()> {
    match (&mut *target, source) {
        (Node::Directory { entries: a, .. }, Node::Directory { entries: b, .. }) => {
            for (name, node) in b {
                overlay(
                    a.entry(name.clone()).or_insert(Node::Missing),
                    node,
                    &format!("{path}/{name}"),
                )?;
            }
        }
        (Node::Missing, _)
        | (Node::File { .. }, Node::File { .. })
        | (Node::Symlink { .. }, Node::Symlink { .. }) => *target = source.clone(),
        _ => {
            return Err(Error::invalid(format!(
                "convergence type conflict at {path}; preserve and reconcile it manually"
            )))
        }
    }
    Ok(())
}
pub(super) fn node_action(path: PathBuf, node: Node, summary: String) -> ai_toolbox_core::Action {
    use ai_toolbox_core::action::Kind;
    let kind = match node {
        Node::Missing => Kind::Remove,
        Node::File { contents, mode } => Kind::Write {
            contents,
            mode: Some(mode),
        },
        Node::Directory { .. } => Kind::CopyTree { from: path.clone() },
        Node::Symlink { target } => Kind::Symlink { target },
    };
    ai_toolbox_core::Action {
        path,
        kind,
        summary,
        noop: false,
        expect: None,
    }
}
pub(super) fn stage_node(path: &Path, node: &Node) -> Result<()> {
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

pub(super) fn package() -> Result<catalogue::Packaged> {
    catalogue::Packaged::load()
}
