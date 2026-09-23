//! Opening `ai-team.app` when an old updater has put `ait` inside it.
//!
//! [`ai_team_core::replaced_app`] explains how that happens. This is the part with somebody
//! in front of it: they opened an app, nothing appeared, and there is no terminal to say
//! why. So every outcome is said on screen, and the one that works ends with the app open.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where reinstalling starts, for when the repair cannot finish on its own.
const REINSTALL: &str = "curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh";

/// This binary, when it has been started by Finder in place of the app.
pub(crate) fn needed() -> Option<PathBuf> {
    if !launched(std::env::args_os().skip(1)) {
        return None;
    }
    let cli = std::env::current_exe().ok()?;
    ai_team_core::replaced_app(&cli).map(|_| cli)
}

/// Whether these arguments are an app being opened rather than a command being run.
///
/// Finder passes none, and macOS before 10.9 passed a process serial number. Anything else
/// is somebody using the CLI on purpose, whatever path it is at, and they get the CLI - no
/// arguments at all is never a useful command, because it only prints `--help`.
fn launched(mut args: impl Iterator<Item = OsString>) -> bool {
    args.all(|arg| arg.to_string_lossy().starts_with("-psn_"))
}

/// Put the app back and open it. Returns the exit code.
pub(crate) fn run(cli: &Path) -> i32 {
    eprintln!(
        "ait: {} is holding `ait` instead of the app - putting the app back",
        cli.display()
    );
    let repaired = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ai_team_core::Error::invalid(error.to_string()))
        .and_then(|runtime| runtime.block_on(repair(cli)));

    let app = match repaired {
        Ok(app) => app,
        Err(error) => {
            return fail(&format!(
                "ai-team did not open because an update from ai-team 0.5.0 or earlier put \
                 the `ait` command-line tool where the app should be, and putting the app \
                 back failed: {error}\n\nTo reinstall ai-team, run this in Terminal:\n\n\
                 {REINSTALL}"
            ))
        }
    };

    // `-n` starts the app on disk whether or not macOS still counts this process - which
    // it launched as ai-team.app, and which has not exited yet - as the app running.
    match std::process::Command::new("open")
        .arg("-n")
        .arg(&app)
        .status()
    {
        Ok(status) if status.success() => 0,
        _ => fail(&format!(
            "ai-team has been repaired but could not be started - open {} again.",
            app.display()
        )),
    }
}

async fn repair(cli: &Path) -> ai_team_core::Result<PathBuf> {
    // The download is tens of megabytes, and until it finishes the Dock icon is the only
    // sign anything is happening.
    ai_team_core::notify(
        "ai-team",
        "Finishing an update that went wrong - ai-team will open in a moment.",
    )
    .await;
    ai_team_core::repair_app(cli, |step| eprintln!("ait: {step:?}")).await
}

fn fail(message: &str) -> i32 {
    eprintln!("ait: {message}");
    ai_team_core::alert(message);
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
        // How Finder starts an app, now and before 10.9.
        assert!(launched(args(&[])));
        assert!(launched(args(&["-psn_0_12345"])));
        // Somebody at a terminal, even at the app's path, is using the CLI.
        assert!(!launched(args(&["ui"])));
        assert!(!launched(args(&["--version"])));
        assert!(!launched(args(&["-psn_0_12345", "doctor"])));
    }
}
