//! Changing `machine.toml` without rewriting it.
//!
//! The file is hand-editable and most of it is explanation - why local is allowed, what
//! the fallback order means, why a work laptop leaves ClickUp off. Parsing it into a
//! struct and serialising that back would be four lines of code and would delete every
//! one of those comments, punishing exactly the person who read the file and edited it by
//! hand.
//!
//! So this edits lines. It finds the key inside its section and changes the value,
//! leaving every other byte where it was. That is more code than a round trip and it is
//! the honest amount of code for "change one value in somebody's file".
//!
//! The awkward cases are all about not matching the wrong line: a commented-out key looks
//! like the key, the same key can appear under two sections, and a key that is not there
//! yet has to be added *inside* its section rather than appended to the end of the file -
//! where TOML would read it as belonging to whichever section came last.

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::Provider;

use super::profile::ContextSource;

/// Allow or deny a provider.
pub fn set_provider(path: &Path, provider: Provider, allowed: bool) -> Result<()> {
    set_bool(path, "providers", provider.as_str(), allowed)
}

/// Enable or disable a read-only context source.
pub fn set_context(path: &Path, source: ContextSource, allowed: bool) -> Result<()> {
    set_bool(path, "context", source.as_str(), allowed)
}

/// Reorder the fallback ranking.
///
/// Rewritten whole because it is one line and an order, not a set of independent values -
/// a surgical edit of a list would be harder to read than replacing it.
pub fn set_fallback(path: &Path, order: &[Provider]) -> Result<()> {
    // Every provider exactly once, or the profile will refuse to load next time and the
    // window will have written a file that breaks itself.
    if order.len() != Provider::ALL.len()
        || Provider::ALL
            .iter()
            .any(|provider| !order.contains(provider))
    {
        return Err(Error::invalid(
            "the fallback order must rank every provider exactly once",
        ));
    }

    let names: Vec<String> = order
        .iter()
        .map(|provider| format!("\"{}\"", provider.as_str()))
        .collect();
    let line = format!("fallback = [{}]", names.join(", "));

    let text = read(path)?;
    let mut out = Vec::new();
    let mut replaced = false;
    for raw in text.lines() {
        if !replaced && key_of(raw).as_deref() == Some("fallback") {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(raw.to_string());
        }
    }
    if !replaced {
        // Before the first section, which is where a top-level key has to live.
        let at = out
            .iter()
            .position(|raw| raw.trim_start().starts_with('['))
            .unwrap_or(out.len());
        out.insert(at, line);
    }
    write(path, &out.join("\n"))
}

/// Set one boolean key inside one section.
fn set_bool(path: &Path, section: &str, key: &str, value: bool) -> Result<()> {
    let text = read(path)?;
    let mut out: Vec<String> = Vec::new();
    let mut here: Option<String> = None;
    let mut done = false;
    // Where the section ends, so a missing key can be added inside it rather than after
    // whatever section happens to be last.
    let mut section_end: Option<usize> = None;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            // Leaving the section we care about: remember where it stopped.
            if here.as_deref() == Some(section) && section_end.is_none() {
                section_end = Some(out.len());
            }
            here = Some(name.trim().to_string());
            out.push(raw.to_string());
            continue;
        }

        if !done && here.as_deref() == Some(section) && key_of(raw).as_deref() == Some(key) {
            // Indentation preserved, because somebody chose it.
            let indent: String = raw.chars().take_while(|c| c.is_whitespace()).collect();
            out.push(format!("{indent}{key} = {value}"));
            done = true;
            continue;
        }
        out.push(raw.to_string());
    }

    if !done {
        // Trailing blank lines belong after the key, not before it.
        let at = section_end.unwrap_or(out.len());
        let at = out[..at]
            .iter()
            .rposition(|raw| !raw.trim().is_empty())
            .map_or(at, |last| last + 1);
        if here.is_none() && !out.iter().any(|raw| raw.trim() == format!("[{section}]")) {
            out.push(String::new());
            out.push(format!("[{section}]"));
            out.push(format!("{key} = {value}"));
        } else if out.iter().any(|raw| raw.trim() == format!("[{section}]")) {
            out.insert(at, format!("{key} = {value}"));
        } else {
            out.push(String::new());
            out.push(format!("[{section}]"));
            out.push(format!("{key} = {value}"));
        }
    }

    write(path, &out.join("\n"))
}

/// The key a line assigns, if it assigns one.
///
/// `None` for a comment, which is the case that matters: `# claude = false` is
/// documentation and flipping it would change nothing while reporting success.
fn key_of(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.trim_matches('"').to_string())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|error| Error::UnusablePath {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn write(path: &Path, text: &str) -> Result<()> {
    // A trailing newline, because the file is read by people and by `git diff`.
    let text = format!("{}\n", text.trim_end());
    std::fs::write(path, text).map_err(|error| Error::UnusablePath {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::DEFAULT_MACHINE_PROFILE;

    fn profile() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machine.toml");
        std::fs::write(&path, DEFAULT_MACHINE_PROFILE).unwrap();
        (dir, path)
    }

    fn text(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn allowing_a_provider_keeps_every_comment() {
        // The whole reason this is line editing rather than a round trip. Most of the file
        // is explanation, and deleting it punishes the person who read it.
        let (_dir, path) = profile();
        let before = text(&path);
        let comments: Vec<&str> = before
            .lines()
            .filter(|l| l.trim().starts_with('#'))
            .collect();
        assert!(
            comments.len() >= 4,
            "the fixture needs comments to preserve"
        );

        set_provider(&path, Provider::Claude, true).unwrap();

        let after = text(&path);
        assert!(after.contains("claude = true"), "{after}");
        for comment in comments {
            assert!(after.contains(comment), "lost: {comment}");
        }
    }

    #[test]
    fn only_the_key_asked_for_changes() {
        let (_dir, path) = profile();
        set_provider(&path, Provider::Claude, true).unwrap();
        let after = text(&path);

        assert!(after.contains("claude = true"));
        assert!(after.contains("openai = false"), "{after}");
        assert!(after.contains("zai = false"), "{after}");
        assert!(after.contains("local = true"), "{after}");
    }

    #[test]
    fn the_result_still_loads() {
        // The failure worth preventing: a window that writes a file the next start cannot
        // read. Every edit is checked by loading it back.
        let (_dir, path) = profile();
        set_provider(&path, Provider::Claude, true).unwrap();
        set_provider(&path, Provider::Local, false).unwrap();
        set_context(&path, ContextSource::ClickUp, true).unwrap();

        let loaded = crate::machine::MachineProfile::load(&path).unwrap();
        assert!(loaded.allowed(Provider::Claude));
        assert!(!loaded.allowed(Provider::Local));
        assert!(loaded.context_allowed(ContextSource::ClickUp));
    }

    #[test]
    fn a_commented_out_key_is_documentation_and_not_the_key() {
        // Matching it would change nothing and report success, which is the worst of both.
        let (_dir, path) = profile();
        std::fs::write(
            &path,
            "version = 1\nfallback = [\"claude\", \"openai\", \"zai\", \"local\"]\n\n\
             [providers]\n# claude = true   <- how you would allow it\nclaude = false\n",
        )
        .unwrap();

        set_provider(&path, Provider::Claude, true).unwrap();
        let after = text(&path);

        assert!(
            after.contains("# claude = true   <- how you would allow it"),
            "the comment should survive: {after}"
        );
        assert!(after.contains("\nclaude = true"), "{after}");
    }

    #[test]
    fn the_same_key_in_another_section_is_left_alone() {
        // `providers.local` and a hypothetical `[something].local` are different keys, and
        // a line-based edit that ignores sections would hit whichever came first.
        let (_dir, path) = profile();
        std::fs::write(
            &path,
            "version = 1\n\n[other]\nclaude = false\n\n[providers]\nclaude = false\n",
        )
        .unwrap();

        set_provider(&path, Provider::Claude, true).unwrap();
        let after = text(&path);
        let lines: Vec<&str> = after.lines().collect();
        let other = lines.iter().position(|l| l.trim() == "[other]").unwrap();
        let providers = lines
            .iter()
            .position(|l| l.trim() == "[providers]")
            .unwrap();

        assert_eq!(lines[other + 1].trim(), "claude = false", "{after}");
        assert_eq!(lines[providers + 1].trim(), "claude = true", "{after}");
    }

    #[test]
    fn a_key_that_is_not_there_yet_is_added_inside_its_section() {
        // Appended to the end of the file, TOML would read it as belonging to whichever
        // section came last - which is how "allow ClickUp" silently becomes a provider.
        let (_dir, path) = profile();
        std::fs::write(
            &path,
            "version = 1\nfallback = [\"claude\", \"openai\", \"zai\", \"local\"]\n\n\
             [context]\nclickup = false\n\n\
             [providers]\nclaude = false\nopenai = false\nzai = false\nlocal = true\n",
        )
        .unwrap();

        set_context(&path, ContextSource::Figma, true).unwrap();
        let after = text(&path);
        let lines: Vec<&str> = after.lines().collect();
        let context = lines.iter().position(|l| l.trim() == "[context]").unwrap();
        let providers = lines
            .iter()
            .position(|l| l.trim() == "[providers]")
            .unwrap();
        let figma = lines
            .iter()
            .position(|l| l.trim().starts_with("figma"))
            .unwrap();

        assert!(
            figma > context && figma < providers,
            "figma landed outside [context]: {after}"
        );
        assert!(crate::machine::MachineProfile::load(&path)
            .unwrap()
            .context_allowed(ContextSource::Figma));
    }

    #[test]
    fn a_missing_section_is_created() {
        let (_dir, path) = profile();
        std::fs::write(
            &path,
            "version = 1\nfallback = [\"claude\", \"openai\", \"zai\", \"local\"]\n\n\
             [providers]\nclaude = true\nopenai = false\nzai = false\nlocal = true\n",
        )
        .unwrap();

        set_context(&path, ContextSource::ClickUp, true).unwrap();
        assert!(crate::machine::MachineProfile::load(&path)
            .unwrap()
            .context_allowed(ContextSource::ClickUp));
    }

    #[test]
    fn whitespace_around_the_equals_does_not_matter() {
        let (_dir, path) = profile();
        std::fs::write(&path, "version = 1\n[providers]\n  claude=false\n").unwrap();

        set_provider(&path, Provider::Claude, true).unwrap();
        let after = text(&path);
        // And the indentation somebody chose is kept.
        assert!(after.contains("  claude = true"), "{after}");
    }

    #[test]
    fn reordering_the_fallback_needs_every_provider_exactly_once() {
        // A partial list makes the profile refuse to load, so the window would have
        // written a file that breaks itself.
        let (_dir, path) = profile();
        assert!(set_fallback(&path, &[Provider::Claude]).is_err());
        assert!(set_fallback(
            &path,
            &[
                Provider::Claude,
                Provider::Claude,
                Provider::ZAi,
                Provider::Local
            ]
        )
        .is_err());

        set_fallback(
            &path,
            &[
                Provider::Local,
                Provider::Claude,
                Provider::OpenAi,
                Provider::ZAi,
            ],
        )
        .unwrap();
        let loaded = crate::machine::MachineProfile::load(&path).unwrap();
        assert_eq!(loaded.fallback().first(), Some(&Provider::Local));
    }

    #[test]
    fn editing_twice_is_the_same_as_editing_once() {
        // The window polls and somebody clicks twice.
        let (_dir, path) = profile();
        set_provider(&path, Provider::Claude, true).unwrap();
        let once = text(&path);
        set_provider(&path, Provider::Claude, true).unwrap();
        assert_eq!(text(&path), once);
    }
}
