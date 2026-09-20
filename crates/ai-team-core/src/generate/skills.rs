//! The repository's own skills, handed to every seat whatever it runs on.
//!
//! D19 gave a Claude-bridged seat the repo's skills through `settingSources: ['project']`,
//! which is the Agent SDK reading `.claude/skills` for itself. That route does not exist
//! for a local, GLM or ChatGPT seat: those are plain eve nodes, and eve has no settings
//! layer to read.
//!
//! But eve has skills of its own, and it reads the **same layout** - a directory under
//! `agent/skills/` containing a `SKILL.md`, which is exactly what `.agents/skills/`
//! already is. So the repository's skills reach every seat by being generated into the
//! eve project, with no bridge involved and nothing provider-specific about it.
//!
//! **Read from the checkout, not the lease.** House rules are read per-lease so a branch
//! is judged by the rules it proposes (`house.rs`), but skills cannot be: one eve project
//! is built once and shared by every seat in a run, before any worktree is leased. A
//! branch that adds a skill therefore does not offer it until the next run - which is the
//! honest trade for skills being a build-time input to eve.
//!
//! Only `SKILL.md` is copied. A skill's `scripts/`, `references/` and `assets/` stay in
//! the repository and are reached through `bash` in the lease, which is both simpler and
//! more correct: the agent runs the repo's actual scripts rather than a copy that went
//! stale when the build did.

use std::collections::BTreeMap;
use std::path::Path;

/// Where a repository keeps skills, in the order they win.
///
/// `.agents/skills` first because it is the harness-neutral location and the one a
/// `.claude/skills` symlink usually points at; the second is read too, for a repo that
/// only has the Claude one. Names collide by design - a repo with both has one set.
const SKILL_ROOTS: &[&str] = &[".agents/skills", ".claude/skills"];

/// One skill as the repository wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skill {
    /// The directory name, which is what eve registers it as.
    pub name: String,
    /// The file's full text, frontmatter included.
    pub markdown: String,
    /// Where the real skill lives, relative to the repository root, so the note below can
    /// point at files that were deliberately not copied.
    pub source: String,
}

/// Every skill a checkout carries.
///
/// Returns empty when there are none, which is the normal case and not worth reporting.
pub(crate) fn read(repo: &Path) -> Vec<Skill> {
    // Keyed by name so `.claude/skills` pointing at `.agents/skills` - the usual shape -
    // yields one skill rather than two identical ones under different paths.
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();

    for root in SKILL_ROOTS {
        let dir = repo.join(root);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `is_dir` follows symlinks, which is what a `.claude/skills` link needs.
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if found.contains_key(name) {
                continue;
            }
            // eve matches the filename case-insensitively, so a repo using `skill.md`
            // is not a repo without skills.
            let Some(markdown) = read_skill_file(&path) else {
                continue;
            };
            if markdown.trim().is_empty() {
                continue;
            }
            found.insert(
                name.to_string(),
                Skill {
                    name: name.to_string(),
                    markdown,
                    source: format!("{root}/{name}"),
                },
            );
        }
    }

    found.into_values().collect()
}

/// A directory's `SKILL.md`, whatever case it was written in.
fn read_skill_file(dir: &Path) -> Option<String> {
    for candidate in ["SKILL.md", "skill.md", "Skill.md"] {
        if let Ok(body) = std::fs::read_to_string(dir.join(candidate)) {
            return Some(body);
        }
    }
    None
}

/// What to write into a seat's `skills/<name>/SKILL.md`.
///
/// The repository's text, unchanged, with one appended line saying where the skill really
/// lives. Without it a skill that says "run `scripts/check.sh`" resolves against the
/// generated project, which holds no scripts - the files stayed in the repo on purpose.
pub(crate) fn skill_md(skill: &Skill) -> String {
    let body = skill.markdown.trim_end();
    format!(
        "{body}\n\n---\n\nThis skill belongs to the repository you are working in. Its own \
         files - `scripts/`, `references/`, `assets/` - were not copied here: they live at \
         `{}` inside your worktree. Resolve any relative path in this skill against that \
         directory, and read or run those files with your worktree tools.\n",
        skill.source
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_repo_with_no_skills_has_none() {
        assert!(read(repo().path()).is_empty());
    }

    #[test]
    fn a_skill_is_read_with_its_frontmatter_intact() {
        // eve parses the frontmatter for the name and description, so passing the file
        // through unchanged is the whole trick - there is nothing to translate.
        let dir = repo();
        write(
            dir.path(),
            ".agents/skills/house-style/SKILL.md",
            "---\nname: house-style\ndescription: How this repo names things.\n---\n# Style\n\nFull words.",
        );

        let skills = read(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "house-style");
        assert!(skills[0]
            .markdown
            .contains("description: How this repo names things."));
        assert_eq!(skills[0].source, ".agents/skills/house-style");
    }

    #[test]
    fn the_claude_directory_is_read_too() {
        // A repo that only has the Claude location still has skills.
        let dir = repo();
        write(
            dir.path(),
            ".claude/skills/only-here/SKILL.md",
            "---\nname: only-here\ndescription: d\n---\nBody.",
        );

        let skills = read(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source, ".claude/skills/only-here");
    }

    #[test]
    fn one_skill_in_both_places_is_one_skill() {
        // `.claude/skills` is usually a symlink to `.agents/skills`, and the toolbox
        // writes it that way - so reading both naively doubles every skill.
        let dir = repo();
        write(
            dir.path(),
            ".agents/skills/shared/SKILL.md",
            "---\nname: shared\ndescription: d\n---\nBody.",
        );
        write(
            dir.path(),
            ".claude/skills/shared/SKILL.md",
            "---\nname: shared\ndescription: d\n---\nBody.",
        );

        let skills = read(dir.path());
        assert_eq!(skills.len(), 1);
        // The harness-neutral location wins, because it is the one the other points at.
        assert_eq!(skills[0].source, ".agents/skills/shared");
    }

    #[test]
    fn a_lowercase_skill_file_is_still_a_skill() {
        // eve classifies the entry case-insensitively, so ai-team must not be stricter
        // than the runtime it generates for.
        let dir = repo();
        write(
            dir.path(),
            ".agents/skills/quiet/skill.md",
            "---\nname: quiet\ndescription: d\n---\nBody.",
        );

        assert_eq!(read(dir.path()).len(), 1);
    }

    #[test]
    fn a_directory_without_a_skill_file_is_not_a_skill() {
        let dir = repo();
        write(dir.path(), ".agents/skills/notes/README.md", "Not a skill.");

        assert!(read(dir.path()).is_empty());
    }

    #[test]
    fn the_generated_file_says_where_the_real_one_lives() {
        // The skill's own scripts and references were deliberately left in the repo, so
        // a relative path in the markdown has to be told what to resolve against.
        let skill = Skill {
            name: "house-style".into(),
            markdown: "# Style\n\nRun `scripts/check.sh`.".into(),
            source: ".agents/skills/house-style".into(),
        };

        let written = skill_md(&skill);
        assert!(written.starts_with("# Style"));
        assert!(written.contains("Run `scripts/check.sh`."));
        assert!(written.contains(".agents/skills/house-style"));
    }
}
