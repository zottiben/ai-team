//! Freeze complete outputs and guard every inspected input. Upstream Actions alone
//! omit write bytes in JSON and do not protect directory/link replacements.

use std::{collections::BTreeMap, fs, path::Path};

use ai_toolbox_core::action::Kind;
use serde::{Deserialize, Serialize};

use super::files::{self, Node, Snapshot};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Effect {
    pub path: String,
    pub summary: String,
    pub before: Node,
    pub after: Node,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Frozen {
    pub inputs: Snapshot,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Outcome {
    pub applied: Vec<String>,
    pub problem: Option<String>,
    pub uncertain: bool,
}

impl Frozen {
    pub(super) fn capture(
        mut inputs: Snapshot,
        staged: &Path,
        actions: Vec<ai_toolbox_core::Action>,
    ) -> Result<Self> {
        let mut effects = Vec::new();
        let mut parents = BTreeMap::new();
        for action in actions {
            let path = files::relative(staged, &action.path)?;
            let before = inputs.before(&path)?;
            let after = match action.kind {
                Kind::Write { contents, mode } => Node::File {
                    contents,
                    mode: mode.unwrap_or(match &before {
                        Node::File { mode, .. } => *mode,
                        _ => 0o644,
                    }),
                },
                Kind::CopyTree { from } => Node::read(&from)?,
                Kind::Symlink { target } => {
                    if path != ".claude/skills" || target != "../.agents/skills" {
                        return Err(Error::invalid(
                            "toolbox setup only links the canonical project skills directory",
                        ));
                    }
                    Node::Symlink { target }
                }
                Kind::Directory => match &before {
                    Node::Directory { .. } => before.clone(),
                    Node::Missing => empty_directory(),
                    _ => {
                        return Err(Error::invalid(
                            "cannot create a toolbox directory over existing content",
                        ))
                    }
                },
                Kind::Remove => Node::Missing,
            };
            // Do not trust upstream's no-op check: skill hashes omit permissions.
            if before == after {
                continue;
            }
            if matches!(after, Node::File { .. })
                && !matches!(before, Node::Missing | Node::File { .. })
            {
                return Err(Error::invalid(format!(
                    "refusing to replace {path} with a file; inspect its existing type"
                )));
            }
            inputs.capture_parents(Path::new(&path))?;
            for parent in Path::new(&path).ancestors().skip(1) {
                if parent.as_os_str().is_empty() {
                    continue;
                }
                let name = files::text(parent)?;
                if inputs.parents.get(&name) == Some(&None) {
                    parents.insert(name, empty_directory());
                }
            }
            effects.push(Effect {
                path,
                summary: action.summary.replace(
                    staged.to_string_lossy().as_ref(),
                    inputs.root.to_string_lossy().as_ref(),
                ),
                before,
                after,
            });
        }
        // Parent creation is an explicit effect in the preview, not a hidden side effect.
        let mut directories: Vec<_> = parents
            .into_iter()
            .map(|(path, after)| Effect {
                summary: format!("create directory {path}"),
                path,
                before: Node::Missing,
                after,
            })
            .collect();
        directories.sort_by_key(|e| e.path.matches('/').count());
        directories.retain(|d| !effects.iter().any(|e| e.path == d.path));
        directories.extend(effects);
        let frozen = Self {
            inputs,
            effects: directories,
        };
        frozen.inputs.validate()?;
        Ok(frozen)
    }

    pub(super) fn apply(&self) -> Outcome {
        let mut outcome = Outcome::default();
        if let Err(error) = self.inputs.validate() {
            outcome.problem = Some(error.to_string());
            return outcome;
        }
        let mut guards = self.inputs.clone();
        for effect in &self.effects {
            let result = (|| {
                guards.validate_parents()?;
                let target = guards.root.join(&effect.path);
                if Node::read(&target)? != effect.before {
                    return Err(files::stale(&effect.path));
                }
                perform(effect, &target)?;
                if guards.parents.contains_key(&effect.path) {
                    guards
                        .parents
                        .insert(effect.path.clone(), files::identity(&target)?);
                }
                Ok(())
            })();
            match result {
                Ok(()) => outcome.applied.push(effect.path.clone()),
                Err(error) => {
                    outcome.uncertain = true;
                    outcome.problem = Some(format!("{}: {error}. Stop and inspect; filesystem changes are not a multi-file transaction.", effect.path));
                    break;
                }
            }
        }
        outcome
    }
}

fn empty_directory() -> Node {
    Node::Directory {
        entries: BTreeMap::new(),
        mode: 0o755,
    }
}

fn perform(effect: &Effect, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| Error::invalid("toolbox target has no parent"))?;
    let staging = tempfile::Builder::new()
        .prefix(".ai-team-setup-")
        .tempdir_in(parent)?;
    let prepared = staging.path().join("prepared");
    effect.after.materialize(&prepared)?;
    // Preparation can take time for a skill tree. Check again before the first rename.
    if Node::read(target)? != effect.before {
        return Err(files::stale(&effect.path));
    }
    let backup = staging.path().join("previous");
    let had_before = effect.before != Node::Missing;
    if had_before {
        fs::rename(target, &backup)?;
    }
    if effect.after != Node::Missing {
        if let Err(error) = fs::rename(&prepared, target) {
            if had_before {
                if let Err(restore) = fs::rename(&backup, target) {
                    let retained = staging.keep();
                    return Err(Error::invalid(format!("write failed: {error}; restore failed: {restore}; previous contents retained at {}", retained.join("previous").display())));
                }
            }
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_skill_tree_and_mode_are_refused_before_any_write() {
        use std::os::unix::fs::PermissionsExt;
        let repo = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("SKILL.md"), "v2").unwrap();
        let skill = repo.path().join(".agents/skills/example");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "v1").unwrap();
        for mode_only in [false, true] {
            let inputs = Snapshot::capture(repo.path()).unwrap();
            let stage = inputs.stage().unwrap();
            let actions = vec![
                ai_toolbox_core::Action::write(
                    stage.path().join("AGENTS.md"),
                    "scaffold",
                    "scaffold",
                )
                .unwrap(),
                ai_toolbox_core::Action::copy_tree(
                    source.path(),
                    stage.path().join(".agents/skills/example"),
                    "update",
                )
                .unwrap(),
            ];
            let frozen = Frozen::capture(inputs, stage.path(), actions).unwrap();
            if mode_only {
                fs::set_permissions(skill.join("SKILL.md"), fs::Permissions::from_mode(0o700))
                    .unwrap();
            } else {
                fs::write(skill.join("SKILL.md"), "concurrent local edit").unwrap();
            }
            let outcome = frozen.apply();
            assert!(outcome.applied.is_empty());
            assert!(outcome.problem.unwrap().contains("stale"));
            assert!(!repo.path().join("AGENTS.md").exists());
            assert_eq!(
                fs::read_to_string(skill.join("SKILL.md")).unwrap(),
                "concurrent local edit"
            );
        }
    }

    #[test]
    fn a_filesystem_failure_keeps_and_reports_the_already_applied_prefix() {
        let repo = tempfile::tempdir().unwrap();
        let inputs = Snapshot::capture(repo.path()).unwrap();
        let stage = inputs.stage().unwrap();
        let actions = vec![
            ai_toolbox_core::Action::write(
                stage.path().join("AGENTS.md"),
                "approved first file",
                "first",
            )
            .unwrap(),
            ai_toolbox_core::Action::write(stage.path().join("CLAUDE.md"), "second", "second")
                .unwrap(),
        ];
        let mut frozen = Frozen::capture(inputs, stage.path(), actions).unwrap();
        // Fault the filesystem preparation of the second effect, after the first landed.
        frozen.effects[1].after = Node::Directory {
            mode: 0o755,
            entries: BTreeMap::from([("invalid\0name".into(), empty_directory())]),
        };
        let outcome = frozen.apply();
        assert_eq!(outcome.applied, vec!["AGENTS.md"]);
        assert!(outcome.uncertain);
        assert!(outcome
            .problem
            .unwrap()
            .contains("not a multi-file transaction"));
        assert_eq!(
            fs::read_to_string(repo.path().join("AGENTS.md")).unwrap(),
            "approved first file"
        );
        assert!(!repo.path().join("CLAUDE.md").exists());
    }

    #[test]
    fn a_frozen_tree_does_not_read_the_catalogue_on_apply() {
        let repo = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("SKILL.md"), "approved bytes").unwrap();
        let inputs = Snapshot::capture(repo.path()).unwrap();
        let stage = inputs.stage().unwrap();
        let action = ai_toolbox_core::Action::copy_tree(
            source.path(),
            stage.path().join(".agents/skills/example"),
            "install",
        )
        .unwrap();
        let frozen = Frozen::capture(inputs, stage.path(), vec![action]).unwrap();
        // Only serialized bytes and fingerprints survive the HTTP preview boundary.
        let encoded = serde_json::to_string(&frozen).unwrap();
        drop(stage);
        drop(source);
        let decoded: Frozen = serde_json::from_str(&encoded).unwrap();
        let outcome = decoded.apply();
        assert!(outcome.problem.is_none(), "{outcome:?}");
        assert_eq!(
            fs::read_to_string(repo.path().join(".agents/skills/example/SKILL.md")).unwrap(),
            "approved bytes"
        );
    }
}
