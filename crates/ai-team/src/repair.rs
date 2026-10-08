//! Explain a desktop bundle damaged by the pre-0.5 updater without treating an app
//! launch as approval to download, replace programs or restart anything.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

const REINSTALL: &str = "curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh";

pub(crate) fn needed() -> Option<PathBuf> {
    if !launched(std::env::args_os().skip(1)) {
        return None;
    }
    let cli = std::env::current_exe().ok()?;
    ai_team_core::replaced_app(&cli).map(|_| cli)
}

/// Finder passes none, or on older macOS a process serial number. Explicit CLI
/// arguments must keep working even when the executable happens to be in a bundle.
fn launched(mut args: impl Iterator<Item = OsString>) -> bool {
    args.all(|arg| arg.to_string_lossy().starts_with("-psn_"))
}

pub(crate) fn run(cli: &Path) -> i32 {
    let message = format!(
        "AI Team could not open: {} contains the command-line tool instead of the desktop app. \
         An old updater installed the wrong program. No programs were changed or restarted.\n\n\
         Save your work, finish all AI Team runs, and quit AI Team before choosing to reinstall \
         from Terminal:\n\n{REINSTALL}\n\nThen open the app again. Your chats and settings are kept.",
        cli.display()
    );
    eprintln!("ait: {message}");
    ai_team_core::alert(&message);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = OsString> {
        list.iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn opening_the_app_is_told_apart_from_running_a_command() {
        assert!(launched(args(&[])));
        assert!(launched(args(&["-psn_0_12345"])));
        assert!(!launched(args(&["ui"])));
        assert!(!launched(args(&["--version"])));
        assert!(!launched(args(&["-psn_0_12345", "doctor"])));
    }
}
