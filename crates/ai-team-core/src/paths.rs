//! Where ai-team keeps its things.
//!
//! Resolved here rather than at each call site so that the CLI, the UI server and the
//! desktop shell can never disagree about which database they are looking at - a class
//! of bug that presents as "my run vanished" and wastes an afternoon.
//!
//! Every path honours an environment override, which is what makes the store testable
//! without writing to the developer's real home directory.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Overrides the data directory, for tests and for running two installs side by side.
pub const HOME_ENV: &str = "AI_TEAM_HOME";

/// The data directory: `~/.ai-team`, or `$AI_TEAM_HOME`.
///
/// One directory for every project on the machine, matching ai-planner's shape - a
/// per-repo database would make "the one right next thing across every project"
/// impossible to answer without opening all of them.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = non_empty_env(HOME_ENV) {
        return Ok(PathBuf::from(dir));
    }
    Ok(home_dir()?.join(".ai-team"))
}

/// The database every surface reads and writes. `M0-S2` creates the schema in it.
pub fn default_db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("team.db"))
}

/// The machine profile that allows or denies each provider (D8).
///
/// Deliberately *not* under [`data_dir`]: it describes the machine, not the work, so it
/// must not travel with a restored backup of the database. On the work machine it is
/// the file that keeps a scheduled run from reaching a forbidden provider.
pub fn machine_profile_path() -> Result<PathBuf> {
    let base = match non_empty_env("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home_dir()?.join(".config"),
    };
    Ok(base.join("ai-team").join("machine.toml"))
}

/// Create the data directory if it is not there yet, and hand back its path.
pub fn ensure_data_dir() -> Result<PathBuf> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| Error::UnusablePath {
        path: dir.clone(),
        reason: e.to_string(),
    })?;
    Ok(dir)
}

/// The operator's home directory, as this machine spells it.
///
/// Public because a surface that has to *offer* somewhere to start browsing needs the same
/// answer the rest of this module uses, and a second reading of `$HOME` is a second thing
/// to keep right.
pub fn home_dir() -> Result<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    non_empty_env(key).map(PathBuf::from).ok_or(Error::NoHome)
}

/// Turn a path somebody typed into one the filesystem knows.
///
/// `~/src/nodifi-data` is a path every shell understands and no system call does: `~` is
/// expanded by the shell before the program ever sees it. A window has no shell in front
/// of it, so a person typing what they would type in a terminal got "the directory does
/// not exist" naming a literal `~` directory - which is true, and useless.
///
/// Only a leading `~` is expanded, and only when it is the whole first component. `~other`
/// is deliberately left alone: resolving another user's home means reading the password
/// database, and a directory genuinely called `~backup` is a likelier thing to meet than
/// somebody registering a checkout out of a colleague's home directory.
pub fn expand_user(path: &std::path::Path) -> Result<PathBuf> {
    let mut parts = path.components();
    let Some(std::path::Component::Normal(first)) = parts.next() else {
        return Ok(path.to_path_buf());
    };
    if first != "~" {
        return Ok(path.to_path_buf());
    }
    Ok(home_dir()?.join(parts.as_path()))
}

/// An environment variable set to the empty string is how a shell spells "unset" by
/// accident; treating it as a path would resolve the database to `/team.db`.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_their_overrides() {
        const CASE: &str = "AI_TEAM_PATHS_TEST_CASE";
        const ROOT: &str = "AI_TEAM_PATHS_TEST_ROOT";

        // Environment belongs to the whole process, while Rust tests run in parallel.
        // Each case therefore gets a child process with its own environment rather than
        // racing every other test by calling `set_var` here.
        if let (Ok(case), Ok(root)) = (std::env::var(CASE), std::env::var(ROOT)) {
            let root = std::path::PathBuf::from(root);
            match case.as_str() {
                "override" => {
                    assert_eq!(data_dir().unwrap(), root);
                    assert_eq!(default_db_path().unwrap(), root.join("team.db"));
                }
                "home" => {
                    assert_eq!(data_dir().unwrap(), root.join(".ai-team"));
                    assert_eq!(
                        machine_profile_path().unwrap(),
                        root.join(".config/ai-team/machine.toml")
                    );
                    assert!(ensure_data_dir().unwrap().is_dir());
                    assert_eq!(
                        expand_user(std::path::Path::new("~/src/nodifi-data")).unwrap(),
                        root.join("src/nodifi-data")
                    );
                    assert_eq!(expand_user(std::path::Path::new("~")).unwrap(), root);
                }
                "xdg" => assert_eq!(
                    machine_profile_path().unwrap(),
                    root.join("ai-team/machine.toml")
                ),
                other => panic!("unknown child case {other}"),
            }
            return;
        }

        let tmp = tempfile::tempdir().expect("a temp dir");
        for case in ["override", "home", "xdg"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .arg("--exact")
                .arg("paths::tests::paths_follow_their_overrides")
                .arg("--nocapture")
                .env(CASE, case)
                .env(ROOT, tmp.path())
                .env("HOME", tmp.path())
                .env("USERPROFILE", tmp.path())
                .env_remove(HOME_ENV)
                .env_remove("XDG_CONFIG_HOME");
            match case {
                "override" => {
                    child.env(HOME_ENV, tmp.path());
                }
                "xdg" => {
                    child.env("XDG_CONFIG_HOME", tmp.path());
                }
                _ => {}
            }
            let status = child.status().expect("run isolated path case");
            assert!(status.success(), "path environment case {case} failed");
        }
    }

    #[test]
    fn expansion_leaves_alone_everything_that_is_not_a_leading_tilde() {
        // An absolute path is already an answer; a `~` in the middle is a directory
        // somebody named that; and `~other` would mean reading the password database to
        // find a colleague's home, which is not a thing a checkout is registered out of.
        for given in [
            "/Users/x/src/thing",
            "relative/thing",
            "/tmp/~/thing",
            "~other/src",
            "./~",
        ] {
            let path = std::path::Path::new(given);
            assert_eq!(
                expand_user(path).unwrap(),
                path,
                "{given} should have been left as it was"
            );
        }
    }
}
