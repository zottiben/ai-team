//! Skills ai-team reads for a seat, rather than hoping the seat opens them (PW13).
//!
//! A harness lists the skills it finds and leaves it to the model to open one. That is
//! fine for a person's session and wrong for a seat whose whole job is one method: a
//! planner that decides not to read the planning skill writes a plan the dispatcher cannot
//! route - which is rule 2's lesson about instructions, arriving by another door. So the
//! skill is read here and handed over in the seat's instructions, every time.
//!
//! ai-team does not own these files. ai-toolbox installs them; this finds them where a
//! harness would, and says so when they are missing.

use std::path::{Path, PathBuf};

/// The planning method the planner seat is given: ai-toolbox's `agile-plan` skill.
pub(crate) const PLANNING_SKILL: &str = "agile-plan";

/// A skill as installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skill {
    pub(crate) path: PathBuf,
    /// Its instructions, without the frontmatter a harness lists it by.
    pub(crate) body: String,
}

/// Find a skill the way a harness would: the repository's own first, then the operator's.
pub(crate) fn find(repo: &Path, name: &str) -> Option<Skill> {
    find_in(&places(repo), name)
}

/// Find a skill installed for the operator, wherever any repository would see it.
pub(crate) fn find_installed(name: &str) -> Option<Skill> {
    let home = crate::paths::home_dir().ok()?;
    find_in(&user_places(&home), name)
}

fn user_places(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".agents/skills"),
        home.join(".pi/agent/skills"),
        home.join(".claude/skills"),
    ]
}

/// Where skills are installed, most specific first. `.agents/skills` is where ai-toolbox
/// puts them and Pi and Codex read them; `.claude/skills` is Claude Code's, usually a link
/// to the same directory.
fn places(repo: &Path) -> Vec<PathBuf> {
    let mut places = vec![repo.join(".agents/skills"), repo.join(".claude/skills")];
    if let Ok(home) = crate::paths::home_dir() {
        places.extend(user_places(&home));
    }
    places
}

fn find_in(places: &[PathBuf], name: &str) -> Option<Skill> {
    places.iter().find_map(|place| {
        let path = place.join(name).join("SKILL.md");
        let text = std::fs::read_to_string(&path).ok()?;
        let body = without_frontmatter(&text).trim().to_string();
        (!body.is_empty()).then_some(Skill { path, body })
    })
}

/// A skill's text after its `---` frontmatter, which is for the harness's listing.
fn without_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---") else {
        return text;
    };
    match rest.find("\n---") {
        Some(end) => rest[end + "\n---".len()..].trim_start_matches(['\r', '\n']),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repositorys_own_skill_wins_and_its_frontmatter_is_left_behind() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        for (root, body) in [
            (repo.path(), "the repo's way"),
            (home.path(), "the operator's way"),
        ] {
            let dir = root.join(".agents/skills/agile-plan");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!(
                    "---\nname: agile-plan\ndescription: plan it\n---\n\n# agile-plan\n\n{body}\n"
                ),
            )
            .unwrap();
        }
        let places = [
            repo.path().join(".agents/skills"),
            home.path().join(".agents/skills"),
        ];

        let found = find_in(&places, PLANNING_SKILL).unwrap();
        assert_eq!(found.body, "# agile-plan\n\nthe repo's way");
        assert!(found.path.starts_with(repo.path()));

        std::fs::remove_dir_all(repo.path().join(".agents")).unwrap();
        let found = find_in(&places, PLANNING_SKILL).unwrap();
        assert!(found.body.ends_with("the operator's way"), "{}", found.body);

        assert_eq!(find_in(&places, "no-such-skill"), None);
    }

    #[test]
    fn a_file_without_frontmatter_is_all_body() {
        assert_eq!(without_frontmatter("# just text\n"), "# just text\n");
        assert_eq!(
            without_frontmatter("---\nunterminated"),
            "---\nunterminated"
        );
    }
}
