//! Putting the guard extension where Pi can load it.
//!
//! The extension is five TypeScript files compiled into the binary, so a `cargo
//! install`ed ai-team carries its own guard rather than depending on a checkout. They are
//! written under the data directory once per version and passed to Pi with `--extension`.
//!
//! Rewritten whenever the contents differ, which is what makes an upgrade take effect:
//! the guard is a rule, and a stale rule on disk is the failure nobody notices.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const GUARD: &str = include_str!("assets/guard.ts");
const WORKTREE: &str = include_str!("assets/worktree.ts");
const IRREVERSIBLE: &str = include_str!("assets/irreversible.ts");
const LIFELINE: &str = include_str!("assets/lifeline.ts");
const PLAN: &str = include_str!("assets/plan.ts");

/// The files that make up the guard, and their contents.
const FILES: &[(&str, &str)] = &[
    ("guard.ts", GUARD),
    ("worktree.ts", WORKTREE),
    ("irreversible.ts", IRREVERSIBLE),
    ("lifeline.ts", LIFELINE),
    ("plan.ts", PLAN),
];

/// Write the guard into `dir` and return the entry point to hand to `--extension`.
///
/// Deliberately not in the worktree. A guard a node can edit is not a guard, and a lease
/// is exactly the directory the node is allowed to write to.
pub fn install_at(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| Error::UnusablePath {
        path: dir.to_path_buf(),
        reason: e.to_string(),
    })?;

    for (name, contents) in FILES {
        let path = dir.join(name);
        // Compare before writing: rewriting on every turn would churn mtimes for nothing,
        // and never rewriting would leave an upgraded ai-team enforcing the old rule.
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current != *contents {
            std::fs::write(&path, contents).map_err(|e| Error::UnusablePath {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        }
    }
    Ok(dir.join("guard.ts"))
}

/// The conventional place: `~/.ai-team/pi/`.
pub fn install() -> Result<PathBuf> {
    install_at(&crate::data_dir()?.join("pi"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_is_written_and_points_at_its_entry_point() {
        let dir = tempfile::tempdir().unwrap();
        let entry = install_at(dir.path()).unwrap();

        assert_eq!(entry, dir.path().join("guard.ts"));
        for (name, _) in FILES {
            assert!(dir.path().join(name).exists(), "{name} was not written");
        }
        // The entry point imports the others by relative path, so they have to be
        // siblings rather than merely present somewhere.
        let guard = std::fs::read_to_string(&entry).unwrap();
        assert!(guard.contains("./irreversible.ts"), "{guard}");
        assert!(guard.contains("./lifeline.ts"), "{guard}");
        assert!(guard.contains("./worktree.ts"), "{guard}");
        assert!(guard.contains("./plan.ts"), "{guard}");
    }

    #[test]
    fn an_upgraded_guard_replaces_the_one_on_disk() {
        // The failure this prevents is silent: ai-team upgrades, the rule changes, and
        // every turn keeps being judged by the version that happens to be on disk.
        let dir = tempfile::tempdir().unwrap();
        install_at(dir.path()).unwrap();
        std::fs::write(dir.path().join("guard.ts"), "// an older guard\n").unwrap();

        install_at(dir.path()).unwrap();
        let guard = std::fs::read_to_string(dir.path().join("guard.ts")).unwrap();
        assert!(
            !guard.contains("an older guard"),
            "the stale guard survived"
        );
        assert!(guard.contains("tool_call"));
    }

    #[test]
    fn installing_twice_is_free() {
        let dir = tempfile::tempdir().unwrap();
        install_at(dir.path()).unwrap();
        let before = std::fs::metadata(dir.path().join("guard.ts"))
            .unwrap()
            .modified()
            .unwrap();

        install_at(dir.path()).unwrap();
        let after = std::fs::metadata(dir.path().join("guard.ts"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "an unchanged guard was rewritten");
    }

    #[test]
    fn the_guard_carries_both_rules() {
        // Compiled in, so a `cargo install`ed binary has them without a checkout.
        assert!(GUARD.contains("tool_call"));
        assert!(WORKTREE.contains("AI_TEAM_WORKTREE"));
        assert!(IRREVERSIBLE.contains("git\\s+push"));
    }
}
