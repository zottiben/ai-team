//! Small shared helpers. Everything here is pure, so it is tested directly.

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// ISO-8601 UTC, second precision. Every timestamp in the database uses this, so rows
/// sort lexically and read cleanly in TablePlus.
pub fn now() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// The same format, `seconds` from now. Used for scheduling, where "in two hours" is how
/// a human says it and an absolute instant is what the clock compares.
pub fn rfc3339_in(seconds: i64) -> String {
    (OffsetDateTime::now_utc() + time::Duration::seconds(seconds))
        .replace_nanosecond(0)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// `ACME-1234 - Reusable Date Range Picker` -> `acme-1234-reusable-date-range-picker`
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Normalise a git remote to a stable, transport-independent key.
/// `git@github.com:org/repo.git` and `https://github.com/org/repo` both become
/// `github.com/org/repo`, so the same repo reached two ways is one row.
pub fn normalise_remote(url: &str) -> String {
    let mut s = url.trim();
    for prefix in ["ssh://", "git+ssh://", "https://", "http://", "git://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    if let Some((userinfo, rest)) = s.split_once('@') {
        if !userinfo.contains('/') {
            s = rest;
        }
    }
    // What is left is `host/path`, `host:path` (scp-style) or `host:port/path`. The
    // colon means different things in the last two, so the split has to look ahead.
    let (host, path) = match s.find([':', '/']) {
        Some(i) if s.as_bytes()[i] == b':' => {
            let after = &s[i + 1..];
            let segment_end = after.find('/').unwrap_or(after.len());
            if !after[..segment_end].is_empty()
                && after[..segment_end].chars().all(|c| c.is_ascii_digit())
            {
                (&s[..i], after[segment_end..].trim_start_matches('/'))
            } else {
                (&s[..i], after)
            }
        }
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };

    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{host}/{path}")
        .to_lowercase()
        .split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Does `path` fall inside one of the agent's zone globs?
///
/// Deliberately small: `*` stops at a path separator, `**` does not, and everything else
/// is a literal. That covers `crates/**`, `ui/src/*.tsx` and `Cargo.toml`, which is the
/// whole vocabulary zone ownership needs. A full glob crate would buy edge cases nobody
/// is going to write and a dependency to audit.
pub fn zone_matches(zone: &str, path: &str) -> bool {
    zone_specificity(zone, path).is_some()
}

/// How specifically a zone claims a path, or `None` if it does not.
///
/// Higher is more specific. This exists so two seats whose zones overlap resolve by *what
/// they said* rather than by which was created first - `ui/**` beats `**` for `ui/App.tsx`
/// whichever order the roster is in. Without it a catch-all zone silently starves every
/// other seat, which is the ambiguity D14 warns about arriving through the back door.
///
/// Measured as the number of literal characters in the matching pattern: `ui/**` has two,
/// `crates/core/**` has eleven, `**` has none. Crude, and right for the thing it decides -
/// a longer pattern is a more deliberate claim.
pub(crate) fn zone_specificity(zone: &str, path: &str) -> Option<usize> {
    zone.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|pattern| glob_matches(pattern, path))
        .map(|pattern| pattern.chars().filter(|c| *c != '*').count())
        .max()
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), path.chars().collect());
    matches_from(&p, 0, &t, 0)
}

fn matches_from(p: &[char], mut pi: usize, t: &[char], mut ti: usize) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                let doubled = p.get(pi + 1) == Some(&'*');
                let rest = pi + if doubled { 2 } else { 1 };
                // `a/**/b` should also match `a/b`, so a `**` is allowed to swallow the
                // separator that follows it.
                let rest = if doubled && p.get(rest) == Some(&'/') {
                    rest + 1
                } else {
                    rest
                };
                if rest >= p.len() {
                    // A trailing `*` must not cross a separator; a trailing `**` may.
                    return doubled || !t[ti..].contains(&'/');
                }
                for skip in ti..=t.len() {
                    if !doubled && t[ti..skip].contains(&'/') {
                        break;
                    }
                    if matches_from(p, rest, t, skip) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if ti >= t.len() || t[ti] == '/' {
                    return false;
                }
            }
            c => {
                if ti >= t.len() || t[ti] != c {
                    return false;
                }
            }
        }
        pi += 1;
        ti += 1;
    }
    ti == t.len()
}

/// A fresh secret for a loopback surface.
///
/// Minted per process and never written down: the only thing it protects is a server on
/// 127.0.0.1 that lives as long as that process, and a secret on disk is a secret that
/// outlives what it protects.
pub fn mint_token() -> String {
    // Two sources, so neither being weak on its own matters: the OS clock at nanosecond
    // resolution, and the address of a fresh heap allocation.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let boxed = Box::new(0u8);
    let addr = std::ptr::from_ref::<u8>(&*boxed) as usize;
    let pid = u128::from(std::process::id());
    let mixed = nanos ^ (addr as u128).rotate_left(64) ^ pid.rotate_left(32);
    format!("{mixed:032x}")
}

#[cfg(test)]
mod tests {
    use super::zone_specificity;

    #[test]
    fn a_more_specific_zone_outranks_a_catch_all() {
        // Two seats whose zones overlap have to resolve by what they said rather than by
        // which was created first, or a catch-all starves everything else.
        let specific = zone_specificity("ui/**", "ui/src/App.tsx").unwrap();
        let catch_all = zone_specificity("**", "ui/src/App.tsx").unwrap();
        assert!(specific > catch_all, "{specific} vs {catch_all}");
    }

    #[test]
    fn a_deeper_zone_outranks_a_shallower_one() {
        let deep = zone_specificity("crates/core/**", "crates/core/src/lib.rs").unwrap();
        let shallow = zone_specificity("crates/**", "crates/core/src/lib.rs").unwrap();
        assert!(deep > shallow);
    }

    #[test]
    fn a_zone_that_does_not_match_has_no_specificity() {
        assert!(zone_specificity("ui/**", "crates/core/src/lib.rs").is_none());
        assert!(zone_specificity("", "anything").is_none());
    }

    #[test]
    fn the_best_line_in_a_multi_line_zone_decides() {
        // A seat listing both a catch-all and something specific claims the specific thing
        // specifically.
        let zone = "**\nui/**";
        assert_eq!(
            zone_specificity(zone, "ui/App.tsx"),
            zone_specificity("ui/**", "ui/App.tsx")
        );
    }
    use super::*;

    #[test]
    fn slugs_are_kebab_ascii() {
        assert_eq!(
            slugify("ACME-1234 - Reusable Date Range Picker"),
            "acme-1234-reusable-date-range-picker"
        );
        assert_eq!(slugify("  ...  "), "");
    }

    #[test]
    fn every_remote_transport_normalises_to_one_key() {
        let expected = "github.com/acme/widget";
        for url in [
            "git@github.com:acme/widget.git",
            "https://github.com/acme/widget.git",
            "https://github.com/acme/widget",
            "ssh://git@github.com/acme/widget.git",
            "ssh://git@github.com:22/acme/widget.git",
            "git://github.com/Acme/widget.git/",
        ] {
            assert_eq!(normalise_remote(url), expected, "for {url}");
        }
    }

    #[test]
    fn a_single_star_stops_at_a_separator() {
        assert!(glob_matches("ui/src/*.tsx", "ui/src/App.tsx"));
        assert!(!glob_matches("ui/src/*.tsx", "ui/src/views/App.tsx"));
        assert!(glob_matches("Cargo.toml", "Cargo.toml"));
        assert!(!glob_matches("Cargo.toml", "crates/a/Cargo.toml"));
    }

    #[test]
    fn a_double_star_crosses_separators_and_may_match_nothing() {
        assert!(glob_matches("crates/**", "crates/ai-team-core/src/lib.rs"));
        assert!(glob_matches("crates/**/Cargo.toml", "crates/a/Cargo.toml"));
        assert!(glob_matches(
            "crates/**/Cargo.toml",
            "crates/a/b/Cargo.toml"
        ));
        // The case that catches naive implementations: `**/` must be allowed to
        // collapse to nothing at all.
        assert!(glob_matches("crates/**/Cargo.toml", "crates/Cargo.toml"));
        assert!(!glob_matches("crates/**", "ui/src/App.tsx"));
    }

    #[test]
    fn a_zone_is_any_of_its_lines_and_ignores_comments() {
        let zone = "# the frontend\nui/**\ncrates/ai-team-ui/**\n\n";
        assert!(zone_matches(zone, "ui/src/App.tsx"));
        assert!(zone_matches(zone, "crates/ai-team-ui/src/lib.rs"));
        assert!(!zone_matches(zone, "crates/ai-team-core/src/db.rs"));
        assert!(!zone_matches("", "anything"));
        // A comment line must never be read as a pattern.
        assert!(!zone_matches("# ui/**", "ui/src/App.tsx"));
    }

    #[test]
    fn timestamps_are_second_precision_utc() {
        let at = now();
        assert!(at.ends_with('Z'), "{at} must be UTC");
        assert_eq!(at.len(), 20, "{at} must be second precision");
    }
}
