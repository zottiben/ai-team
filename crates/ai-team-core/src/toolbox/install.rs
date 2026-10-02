//! Adapt copy-mode installs without ever planning a write through a live symlink.

use std::{fs, path::Path};

use ai_toolbox_core::{action::Kind, install, Action, Catalogue, Harness, Plan};

use super::files::Node;
use crate::{Error, Result};

pub(super) fn skills(
    plan: &mut Plan,
    root: &Path,
    catalogue: &Catalogue,
    keys: &[String],
    harnesses: &[Harness],
    no_symlink: bool,
) -> Result<()> {
    if no_symlink && harnesses.contains(&Harness::Claude) && !keys.is_empty() {
        plan.warn("Claude skills are independent copies. Later changes to canonical skills do not propagate; explicitly reinstall selected skills to update the copies.");
    }
    let link = root.join(".claude/skills");
    let linked = fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink());
    if keys.is_empty() || !harnesses.contains(&Harness::Claude) || !no_symlink || !linked {
        install::skills(plan, root, catalogue, keys, harnesses, !no_symlink)?;
        return Ok(());
    }
    // Snapshot permits only the canonical relative link. Replace it on the private
    // planning copy, preserving every unselected canonical skill in the new copy.
    let canonical = Node::read(&root.join(".agents/skills"))?;
    fs::remove_file(&link)?;
    if canonical == Node::Missing {
        fs::create_dir(&link)?;
    } else {
        canonical.materialize(&link)?;
    }
    let mut chosen = Plan::default();
    install::skills(&mut chosen, root, catalogue, keys, harnesses, false)?;
    for action in chosen.actions {
        if action.path.starts_with(&link) {
            let Kind::CopyTree { from } = &action.kind else {
                return Err(Error::invalid(
                    "unexpected engine skill-copy action; no project files changed",
                ));
            };
            let bytes = Node::read(from)?;
            if action.path.exists() {
                fs::remove_dir_all(&action.path)?;
            }
            bytes.materialize(&action.path)?;
        } else {
            plan.push(action);
        }
    }
    plan.warnings.extend(chosen.warnings);
    plan.push(Action::copy_tree(
        &link,
        &link,
        "replace canonical Claude skills link with an independent copy (including unselected skills)",
    )?);
    Ok(())
}
