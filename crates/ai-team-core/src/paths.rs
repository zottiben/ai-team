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

fn home_dir() -> Result<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    non_empty_env(key).map(PathBuf::from).ok_or(Error::NoHome)
}

/// An environment variable set to the empty string is how a shell spells "unset" by
/// accident; treating it as a path would resolve the database to `/team.db`.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These mutate process-wide environment, so they are one test rather than several:
    // cargo runs tests in threads and two of them racing on $HOME is a flake.
    #[test]
    fn paths_follow_their_overrides() {
        let tmp = tempfile::tempdir().expect("a temp dir");

        // SAFETY-adjacent: single-threaded within this test, and every path this
        // module exposes is derived fresh on each call rather than cached.
        std::env::set_var(HOME_ENV, tmp.path());
        assert_eq!(data_dir().unwrap(), tmp.path());
        assert_eq!(default_db_path().unwrap(), tmp.path().join("team.db"));

        std::env::set_var(HOME_ENV, "");
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("USERPROFILE", tmp.path());
        assert_eq!(data_dir().unwrap(), tmp.path().join(".ai-team"));

        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        assert_eq!(
            machine_profile_path().unwrap(),
            tmp.path().join("ai-team").join("machine.toml")
        );

        std::env::remove_var("XDG_CONFIG_HOME");
        assert_eq!(
            machine_profile_path().unwrap(),
            tmp.path()
                .join(".config")
                .join("ai-team")
                .join("machine.toml")
        );

        std::env::remove_var(HOME_ENV);
        let created = ensure_data_dir().unwrap();
        assert!(created.is_dir());
    }
}
