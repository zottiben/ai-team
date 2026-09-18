//! The conventions a repository already carries.
//!
//! House rules are written by hand, per repo (Q16). ai-team does not learn them from
//! review feedback and does not generate them - some repos have a considered AGENTS.md
//! and a set of skills that check their patterns, and others have nothing at all. What
//! ai-team owes both is the same: read whatever is there, put it in front of the seat
//! working in that checkout, and say nothing when there is nothing.
//!
//! This has to be explicit precisely because the Claude-bridged seats set
//! `settingSources: []` (D7). That stops a node inheriting the *human's* Claude config,
//! which is right - but it is also why the *repository's* own rules have to be handed
//! over deliberately rather than picked up by accident.
//!
//! Read from the leased worktree rather than the main checkout, so a branch that changes
//! the house rules is judged by the rules it is proposing.

use std::path::Path;

/// Files worth reading, in the order a reader would weigh them.
///
/// `AGENTS.md` first because it is the one written *for* an agent. `CLAUDE.md` is
/// included because plenty of repos only have that one - but in repos that have both it
/// is usually a one-line import of AGENTS.md, which is why content is deduplicated below.
const CANDIDATES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "CONVENTIONS.md",
    ".cursor/rules",
    ".github/copilot-instructions.md",
];

/// How much house rule to send.
///
/// A repository is free to keep a very long document; a node's context is not. Sixteen
/// thousand characters is roughly four thousand tokens - enough for a considered
/// AGENTS.md, and small enough that it cannot crowd out the slice it is meant to inform.
const BUDGET: usize = 16_000;

/// One file's worth of house rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub path: String,
    pub body: String,
    /// True when the file was longer than the budget and the tail was dropped.
    pub truncated: bool,
}

/// Everything this checkout has to say about how work is done in it.
///
/// Returns empty when a repo carries none, which is a normal answer and not a problem to
/// report: most repositories have no house rules and inventing a section that says so
/// would spend context telling a model nothing.
pub fn read(worktree: &Path) -> Vec<Rules> {
    let mut found: Vec<Rules> = Vec::new();
    let mut spent = 0usize;

    for name in CANDIDATES {
        let path = worktree.join(name);
        let files = if path.is_dir() {
            // `.cursor/rules` is a directory of them. Sorted so the prompt is stable
            // between runs - an agent seeing its instructions reshuffle every turn is
            // reading a different document each time.
            let mut entries: Vec<_> = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext == "md" || ext == "mdc")
                })
                .collect();
            entries.sort();
            entries
        } else {
            vec![path]
        };

        for file in files {
            let Ok(body) = std::fs::read_to_string(&file) else {
                continue;
            };
            let body = body.trim().to_string();
            if body.is_empty() {
                continue;
            }

            // A CLAUDE.md that is only `@AGENTS.md` is a pointer, not a document. Sending
            // it wastes a line and, worse, reads as an instruction to open a file the
            // agent has already been given.
            if is_only_an_import(&body) {
                continue;
            }

            // Two files with the same content is the common shape when a repo keeps
            // CLAUDE.md and AGENTS.md in step. Send it once.
            if found.iter().any(|rules| rules.body == body) {
                continue;
            }

            let relative = file
                .strip_prefix(worktree)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();

            let remaining = BUDGET.saturating_sub(spent);
            if remaining == 0 {
                break;
            }
            let (body, truncated) = clip(&body, remaining);
            spent += body.len();
            found.push(Rules {
                path: relative,
                body,
                truncated,
            });
        }
    }
    found
}

/// Whether a file is nothing but a pointer at another one.
fn is_only_an_import(body: &str) -> bool {
    body.lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with("<!--"))
        .all(|line| {
            let line = line.trim();
            // `@AGENTS.md`, or a bare markdown link to one - both say "read that instead".
            line.starts_with('@') || (line.starts_with('[') && line.ends_with(')'))
        })
}

/// Cut to a budget on a line boundary, so the tail is not half a sentence.
fn clip(body: &str, budget: usize) -> (String, bool) {
    if body.len() <= budget {
        return (body.to_string(), false);
    }
    let mut kept = String::with_capacity(budget);
    for line in body.lines() {
        if kept.len() + line.len() + 1 > budget {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    (kept.trim_end().to_string(), true)
}

/// The house rules as a prompt section, or nothing at all.
///
/// Framed as the standard of the codebase rather than as reference material: a node that
/// treats them as background reading will follow them right up until they are
/// inconvenient, which is exactly when they matter.
pub fn section(rules: &[Rules]) -> String {
    use std::fmt::Write as _;

    if rules.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\nThis repository has house rules. They are the standard your work is held to, \
         and they win over your own habits wherever the two differ.\n",
    );
    for entry in rules {
        let _ = write!(out, "\n--- {} ---\n{}\n", entry.path, entry.body);
        if entry.truncated {
            // Said plainly, because a model that does not know it was cut off will
            // confidently act as though it read the whole thing.
            let _ = writeln!(out, "[truncated - read {} for the rest]", entry.path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_repo_with_no_house_rules_sends_no_section() {
        // Most repositories have none, and a section saying so would spend context
        // telling a model nothing.
        let dir = repo();
        assert!(read(dir.path()).is_empty());
        assert_eq!(section(&[]), "");
    }

    #[test]
    fn a_repo_with_an_agents_file_has_it_in_the_prompt() {
        let dir = repo();
        write(dir.path(), "AGENTS.md", "# House\n\nNever use an em dash.");

        let rules = read(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path, "AGENTS.md");

        let prompt = section(&rules);
        assert!(prompt.contains("Never use an em dash"));
        // And it is framed as a standard rather than as something to consult.
        assert!(prompt.contains("win over your own habits"));
    }

    #[test]
    fn a_claude_file_that_only_imports_agents_is_not_sent_twice() {
        // The exact shape this repository uses, and the one that would otherwise put a
        // stray `@AGENTS.md` in a prompt that already contains AGENTS.md.
        let dir = repo();
        write(dir.path(), "AGENTS.md", "the real rules");
        write(dir.path(), "CLAUDE.md", "@AGENTS.md\n");

        let rules = read(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path, "AGENTS.md");
    }

    #[test]
    fn two_files_kept_in_step_are_sent_once() {
        let dir = repo();
        write(dir.path(), "AGENTS.md", "identical rules");
        write(dir.path(), "CLAUDE.md", "identical rules");
        assert_eq!(read(dir.path()).len(), 1);
    }

    #[test]
    fn a_claude_file_with_rules_of_its_own_is_sent() {
        // Plenty of repos only have this one, and skipping it would ignore them.
        let dir = repo();
        write(dir.path(), "CLAUDE.md", "# Rules\n\nAlways run the linter.");
        let rules = read(dir.path());
        assert_eq!(rules.len(), 1);
        assert!(rules[0].body.contains("Always run the linter"));
    }

    #[test]
    fn a_directory_of_rules_is_read_in_a_stable_order() {
        // An agent whose instructions reshuffle every turn is reading a different
        // document each time.
        let dir = repo();
        write(dir.path(), ".cursor/rules/02-second.mdc", "second rule");
        write(dir.path(), ".cursor/rules/01-first.mdc", "first rule");
        write(dir.path(), ".cursor/rules/notes.txt", "not a rule file");

        let rules = read(dir.path());
        assert_eq!(rules.len(), 2);
        assert!(rules[0].path.ends_with("01-first.mdc"));
        assert!(rules[1].path.ends_with("02-second.mdc"));
    }

    #[test]
    fn a_very_long_file_is_cut_and_says_so() {
        // A model that does not know it was cut off will act as though it read the whole
        // thing, which is worse than knowing it has part of it.
        let dir = repo();
        let long = "a line of house rules that goes on\n".repeat(2_000);
        write(dir.path(), "AGENTS.md", &long);

        let rules = read(dir.path());
        assert_eq!(rules.len(), 1);
        assert!(rules[0].truncated);
        assert!(rules[0].body.len() <= BUDGET);
        // Cut on a line boundary, not mid-sentence.
        assert!(rules[0].body.ends_with("goes on"));
        assert!(section(&rules).contains("truncated"));
    }

    #[test]
    fn the_budget_is_across_every_file_not_each_one() {
        // Otherwise five candidate files is five times the context.
        let dir = repo();
        write(dir.path(), "AGENTS.md", &"first file\n".repeat(2_000));
        write(dir.path(), "CONVENTIONS.md", &"second file\n".repeat(2_000));

        let total: usize = read(dir.path()).iter().map(|r| r.body.len()).sum();
        assert!(total <= BUDGET, "{total} over budget");
    }

    #[test]
    fn an_empty_file_is_not_house_rules() {
        let dir = repo();
        write(dir.path(), "AGENTS.md", "\n\n   \n");
        assert!(read(dir.path()).is_empty());
    }
}
