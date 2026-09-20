//! The conventions a repository already carries.
//!
//! House rules are written by hand, per repo (Q16). ai-team does not learn them from
//! review feedback and does not generate them - some repos have a considered AGENTS.md
//! and a set of skills that check their patterns, and others have nothing at all. What
//! ai-team owes both is the same: read whatever is there, put it in front of the seat
//! working in that checkout, and say nothing when there is nothing.
//!
//! This has to be explicit because a seat's own settings layer does not cover it. A
//! Claude-bridged seat reads the repository's `project` settings and its CLAUDE.md files
//! (D19), but AGENTS.md is not a Claude convention and is not among them - and a local,
//! GLM or ChatGPT seat is a plain eve node with no settings layer at all. House rules are
//! the one channel every seat has, whatever it runs on.
//!
//! Read from the leased worktree rather than the main checkout, so a branch that changes
//! the house rules is judged by the rules it is proposing.
//!
//! **A repository puts its rules next to the code they govern.** `ui/AGENTS.md` says what
//! the frontend does differently, and a seat editing there is held to the root file alone
//! unless the nested one is found too. So the walk below looks for them, and picks by what
//! the slice actually touches: in a monorepo, every AGENTS.md in the tree is not context,
//! it is noise that crowds out the slice.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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

/// The names a repository puts in a subdirectory to govern that subdirectory.
///
/// Only these two: `CONVENTIONS.md` and the Cursor and Copilot files are conventionally
/// one-per-repo, and walking a tree looking for them finds unrelated documents.
const NESTED_CANDIDATES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// Directories never worth descending into.
///
/// Build output and dependencies, where a vendored package's own AGENTS.md is somebody
/// else's instructions to somebody else's agent.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    ".output",
    ".eve",
    "vendor",
    ".venv",
    "build",
];

/// How deep to look.
///
/// Deep enough for `crates/<name>/src`, and shallow enough that a large monorepo does not
/// turn one prompt into a filesystem crawl.
const MAX_DEPTH: usize = 6;

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
    read_for(worktree, &[])
}

/// The same, narrowed to the rules that govern the paths a slice touches.
///
/// The root files always apply. A nested one applies when it governs something this slice
/// is going to edit - `ui/AGENTS.md` for a slice touching `ui/src/App.tsx` - which is the
/// same question routing already answers about zones, asked of a document instead of a
/// seat.
///
/// With no paths given, every nested file in the tree applies. That is the honest answer
/// for a turn with no slice behind it, where nothing narrows what the agent might edit.
pub fn read_for(worktree: &Path, touches: &[String]) -> Vec<Rules> {
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

    // Then the nested ones, deepest first. Depth is the selection order rather than the
    // presentation order: under a budget the file nearest the code is the one worth
    // keeping, but it reads better after the general rules it qualifies.
    let mut nested = nested_rules(worktree, touches);
    nested.sort_by(|a, b| depth(&b.0).cmp(&depth(&a.0)).then_with(|| a.0.cmp(&b.0)));

    let first_nested = found.len();
    for (relative, body) in nested {
        if is_only_an_import(&body) || found.iter().any(|rules| rules.body == body) {
            continue;
        }
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

    // Present shallowest first, so a nested file reads as qualifying the one above it.
    found[first_nested..].sort_by_key(|rules| depth(&rules.path));
    found
}

/// How many directories deep a repo-relative path sits.
fn depth(relative: &str) -> usize {
    relative.matches('/').count()
}

/// Every nested house-rule file that governs something this slice touches.
fn nested_rules(worktree: &Path, touches: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(worktree.to_path_buf(), 0usize)];

    while let Some((dir, level)) = stack.pop() {
        if level >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if SKIP_DIRS.contains(&name) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(worktree) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if governs(&relative, touches) {
                for candidate in NESTED_CANDIDATES {
                    let file = path.join(candidate);
                    let key = format!("{relative}/{candidate}");
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let Ok(body) = std::fs::read_to_string(&file) else {
                        continue;
                    };
                    let body = body.trim().to_string();
                    if !body.is_empty() {
                        out.push((key, body));
                    }
                }
            }
            stack.push((path, level + 1));
        }
    }
    out
}

/// Whether a directory governs any of the paths a slice touches.
///
/// A glob is matched on its literal prefix rather than expanded: `crates/**` names the
/// directory `crates` whatever follows, and a slice that touches `**` - which is how the
/// orchestrator says "anywhere" - is governed by every rule in the tree.
fn governs(dir: &str, touches: &[String]) -> bool {
    if touches.is_empty() {
        return true;
    }
    touches.iter().any(|touch| {
        let touch = touch.trim().trim_start_matches("./");
        let literal: String = touch
            .split('/')
            .take_while(|segment| !segment.contains('*') && !segment.contains('?'))
            .collect::<Vec<_>>()
            .join("/");
        // `**` alone leaves nothing literal, which is the orchestrator saying the slice
        // may touch anywhere - so every directory governs it.
        if literal.is_empty() {
            return true;
        }
        // The rule applies when its directory contains the touched path (`ui` governs
        // `ui/src/App.tsx`), and also when the touched path contains the directory
        // (`crates/**` reaches `crates/ai-team-core`, whose own file applies).
        let dir_path = PathBuf::from(dir);
        let touch_path = PathBuf::from(&literal);
        touch_path.starts_with(&dir_path) || dir_path.starts_with(&touch_path)
    })
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

#[cfg(test)]
mod nested_tests {
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

    fn paths(rules: &[Rules]) -> Vec<String> {
        rules.iter().map(|r| r.path.clone()).collect()
    }

    #[test]
    fn a_rule_next_to_the_code_reaches_the_seat_editing_there() {
        // The whole point: `ui/AGENTS.md` says what the frontend does differently, and a
        // seat editing there was held to the root file alone.
        let dir = repo();
        write(dir.path(), "AGENTS.md", "Repo-wide: keep it small.");
        write(
            dir.path(),
            "ui/AGENTS.md",
            "Frontend: every colour is a token.",
        );
        write(dir.path(), "ui/src/App.tsx", "export default 1;");

        let rules = read_for(dir.path(), &["ui/src/App.tsx".into()]);
        assert_eq!(paths(&rules), ["AGENTS.md", "ui/AGENTS.md"]);
    }

    #[test]
    fn the_rest_of_a_monorepo_is_not_sent() {
        // A nested file is context only for the code it governs. Sending all of them
        // crowds out the slice with instructions for directories nobody is touching.
        let dir = repo();
        write(dir.path(), "ui/AGENTS.md", "Frontend rules.");
        write(dir.path(), "server/AGENTS.md", "Backend rules.");
        write(dir.path(), "docs/AGENTS.md", "Docs rules.");

        let rules = read_for(dir.path(), &["ui/src/App.tsx".into()]);
        assert_eq!(paths(&rules), ["ui/AGENTS.md"]);
    }

    #[test]
    fn a_glob_is_matched_on_its_literal_prefix() {
        // `plan_add_slice` writes `Touches: crates/**`, so the directory a glob names has
        // to be read off the part before the wildcard rather than expanded.
        let dir = repo();
        write(dir.path(), "crates/core/AGENTS.md", "Core rules.");
        write(dir.path(), "ui/AGENTS.md", "Frontend rules.");

        let rules = read_for(dir.path(), &["crates/**".into()]);
        assert_eq!(paths(&rules), ["crates/core/AGENTS.md"]);
    }

    #[test]
    fn touching_anywhere_is_governed_by_everything() {
        // `**` is the orchestrator saying a slice may edit anywhere, so nothing narrows.
        let dir = repo();
        write(dir.path(), "ui/AGENTS.md", "Frontend rules.");
        write(dir.path(), "server/AGENTS.md", "Backend rules.");

        let rules = read_for(dir.path(), &["**".into()]);
        assert_eq!(paths(&rules), ["server/AGENTS.md", "ui/AGENTS.md"]);
    }

    #[test]
    fn the_more_specific_rule_is_read_last() {
        // Presentation order is shallowest first, so a nested file reads as qualifying
        // the one above it rather than being contradicted by it.
        let dir = repo();
        write(dir.path(), "AGENTS.md", "Root.");
        write(dir.path(), "a/AGENTS.md", "One deep.");
        write(dir.path(), "a/b/AGENTS.md", "Two deep.");
        write(dir.path(), "a/b/c/AGENTS.md", "Three deep.");

        let rules = read_for(dir.path(), &["a/b/c/x.rs".into()]);
        assert_eq!(
            paths(&rules),
            [
                "AGENTS.md",
                "a/AGENTS.md",
                "a/b/AGENTS.md",
                "a/b/c/AGENTS.md"
            ]
        );
    }

    #[test]
    fn build_output_and_dependencies_are_not_house_rules() {
        // A vendored package's AGENTS.md is somebody else's instructions to somebody
        // else's agent, and `target/` is not a place rules live.
        let dir = repo();
        write(dir.path(), "node_modules/pkg/AGENTS.md", "Vendor rules.");
        write(dir.path(), "target/debug/AGENTS.md", "Build rules.");
        write(dir.path(), ".git/AGENTS.md", "Git rules.");

        assert!(read_for(dir.path(), &["**".into()]).is_empty());
    }

    #[test]
    fn a_nested_pointer_file_is_still_skipped() {
        // The same rule as the root: `@AGENTS.md` is a pointer, and sending it reads as
        // an instruction to open a file the agent already has.
        let dir = repo();
        write(
            dir.path(),
            "ui/AGENTS.md",
            "Frontend: every colour is a token.",
        );
        write(dir.path(), "ui/CLAUDE.md", "@AGENTS.md");

        let rules = read_for(dir.path(), &["ui/".into()]);
        assert_eq!(paths(&rules), ["ui/AGENTS.md"]);
    }

    #[test]
    fn the_budget_still_covers_the_nested_ones() {
        // The budget is the whole point of choosing by relevance: a deep tree of rules
        // must not be able to crowd out the slice it is meant to inform.
        let dir = repo();
        write(dir.path(), "AGENTS.md", &"r".repeat(BUDGET - 10));
        write(dir.path(), "ui/AGENTS.md", &"u".repeat(5_000));

        let rules = read_for(dir.path(), &["ui/x".into()]);
        let total: usize = rules.iter().map(|r| r.body.len()).sum();
        assert!(total <= BUDGET, "{total} over budget");
        assert!(rules.iter().any(|r| r.truncated));
    }

    #[test]
    fn a_turn_with_no_slice_gets_every_nested_rule() {
        // Talking to a seat has no slice behind it, so nothing narrows what it might
        // edit and the honest answer is all of them.
        let dir = repo();
        write(dir.path(), "ui/AGENTS.md", "Frontend rules.");
        write(dir.path(), "server/AGENTS.md", "Backend rules.");

        let rules = read_for(dir.path(), &[]);
        assert_eq!(paths(&rules), ["server/AGENTS.md", "ui/AGENTS.md"]);
    }
}
