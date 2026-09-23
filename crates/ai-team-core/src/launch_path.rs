//! What PATH is, when nobody logged in to start this process.
//!
//! An app launched from Finder or the Dock inherits launchd's environment, and launchd's
//! PATH is `/usr/bin:/bin:/usr/sbin:/sbin`. That is not a macOS curiosity to work around
//! at the edges - it is most of ai-team failing at once, because almost everything this
//! program does is spawn somebody else's program:
//!
//! - `aip` and `awt` are how a run is planned and leased (D4), and they read as missing
//! - `claude` and `codex` are how a provider is signed in (D8), and they read as absent
//! - `pi` is the runtime (D20), so no turn could start at all
//!
//! None of it is visible from a terminal. `ait doctor` and `cargo test` both inherit a
//! login shell and pass, which is exactly the shape of failure D12 warns about: the
//! platform that matters is the one the author cannot hand-test.
//!
//! So ask the shell. `$SHELL -l -i -c 'printenv PATH'` is the operator's own answer,
//! produced by the same startup files that produced the PATH in their terminal - whatever
//! manages their versions has already run by the time it prints (D22). Guessing at
//! `~/.local/bin` and `/opt/homebrew/bin` would cover this machine and quietly fail on one
//! using nix, asdf or a custom prefix, while looking fixed.
//!
//! `-i` is not optional, and leaving it off is the version of this that looks correct and
//! is not. On the reporting machine a login shell answers with `/usr/local/bin:...:
//! /opt/homebrew/bin:~/.cargo/bin`, and an *interactive* login shell adds `~/.local/bin` -
//! where `awt`, `claude`, `codex` and `ai-toolbox` all live. zsh reads `.zprofile` on
//! login and `.zshrc` only when interactive, and `.zshrc` is where a person actually puts
//! things. It cost a build to find, because the two tools that happened to also have a
//! stale copy in `~/.cargo/bin` made the half-fix look like a whole one.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the login shell gets to answer.
///
/// A login shell runs the operator's startup files, which on a well-loved machine is not
/// instant. It is also the first thing that happens at startup, so it cannot be allowed to
/// hang the window: past this, the process keeps the PATH it was given and the readiness
/// report says what is missing, which is the same honest outcome as before.
const PATIENCE: Duration = Duration::from_secs(5);

/// Merge the login shell's PATH into this process's, once.
///
/// Call it from `main`, before starting a runtime or spawning a thread. `set_var` mutates
/// process-global state that every other thread may be reading, and PATH is read by every
/// `Command` this program creates.
///
/// Returns the directories that were added, which is what a caller prints when asked to
/// explain itself. Empty means there was nothing to add - the usual case for a process
/// started from a terminal.
pub fn adopt_login_path() -> Vec<PathBuf> {
    adopt_launchd_ssh_agent();
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let Some(login) = login_shell_path() else {
        return Vec::new();
    };

    let merged = merge(&inherited, &OsString::from(login));
    let added: Vec<PathBuf> = std::env::split_paths(&merged)
        .filter(|dir| !std::env::split_paths(&inherited).any(|had| had == *dir))
        .collect();
    if added.is_empty() {
        return Vec::new();
    }

    std::env::set_var("PATH", &merged);
    added
}

/// Everything the process had, then everything the shell knows, each directory once.
///
/// Inherited first and deliberately: an environment that was *narrowed* on purpose - a
/// test harness, a sandbox, a `PATH=` prefix on the command line - keeps its own
/// precedence, and this only ever appends places to look. First-seen order is preserved so
/// the shell's own precedence survives among the entries it contributes.
/// Finder launches inherit launchd's environment, but stripped launch environments and
/// some app launchers omit the SSH agent socket. Private Git remotes then fail only in
/// the desktop app even though they work in the operator's terminal. Never replace a
/// socket the caller supplied; ask the user's launchd instance only when it is absent.
#[cfg(target_os = "macos")]
fn adopt_launchd_ssh_agent() {
    if std::env::var_os("SSH_AUTH_SOCK").is_some_and(|value| !value.is_empty()) {
        return;
    }
    let Ok(output) = Command::new("/bin/launchctl")
        .args(["getenv", "SSH_AUTH_SOCK"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    if let Some(value) = launchctl_value(&output.stdout) {
        std::env::set_var("SSH_AUTH_SOCK", value);
    }
}

#[cfg(not(target_os = "macos"))]
fn adopt_launchd_ssh_agent() {}

#[cfg(any(target_os = "macos", test))]
fn launchctl_value(output: &[u8]) -> Option<OsString> {
    let value = std::str::from_utf8(output).ok()?.trim();
    (!value.is_empty()).then(|| OsString::from(value))
}

fn merge(inherited: &OsString, login: &OsString) -> OsString {
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(inherited).chain(std::env::split_paths(login)) {
        // An empty entry means "the current directory" to most shells, which is not
        // something to inherit into a program that spawns tools by name.
        if dir.as_os_str().is_empty() || seen.contains(&dir) {
            continue;
        }
        seen.push(dir);
    }
    std::env::join_paths(seen).unwrap_or_else(|_| inherited.clone())
}

/// Markers around the answer, so a shell that greets you does not get parsed as one.
///
/// An interactive shell prints motd, version-manager notices and whatever the operator's
/// rc files feel like saying. Reading the first plausible line out of that is how this
/// would work on one machine and not on the next.
const BEGIN: &str = "__ai_team_env_begin__";
const END: &str = "__ai_team_env_end__";

/// Ask the operator's shell what PATH is.
///
/// Asks `printenv` rather than expanding `$PATH` because the shells a person might have set
/// disagree about the variable: in fish `$PATH` is a list, and `printf '%s' $PATH` hands
/// back every directory run together with no separator - a plausible-looking string naming
/// nowhere. `printenv` is a program, so every shell hands it the same exported,
/// colon-joined value, because that is the form the operating system holds it in. And it
/// prints that value alone - which `env` does not, and that difference is
/// [`path_between_markers`]'s to explain.
///
/// Every failure is `None` rather than an error. This runs before the process has anywhere
/// to report to, and a machine with no usable shell is one where the inherited PATH is the
/// only answer available - the readiness report will then say which tools are missing,
/// which is the outcome this is trying to avoid but not a worse one than today.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())?;

    // Interactive *and* login, with a plain login shell as the fallback: an rc file that
    // refuses to run without a terminal is rare, and would otherwise cost the whole answer
    // rather than the part only `-i` contributes.
    ask(&shell, &["-l", "-i", "-c"]).or_else(|| ask(&shell, &["-l", "-c"]))
}

fn ask(shell: &str, flags: &[&str]) -> Option<String> {
    let script = format!("printf '%s\\n' {BEGIN}; printenv PATH; printf '%s\\n' {END}");

    // Not `/bin/sh` as a fallback: a shell that is not the operator's reads startup files
    // that are not theirs, and would answer with a PATH they have never seen.
    let mut command = Command::new(shell);
    command
        .args(flags)
        .arg(&script)
        // An interactive shell with a terminal on stdin sits waiting for a command. It has
        // none coming, so it is told so immediately rather than after the timeout.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // The exit status is deliberately not checked. An interactive rc file ending in a
    // failing command - a `command -v` guard, a git call in a prompt - leaves a non-zero
    // status behind having printed a perfectly good environment.
    let output = wait_briefly(command)?;
    path_between_markers(&String::from_utf8_lossy(&output.stdout))
}

/// Pull `PATH` out from between the markers, whatever the shell said around them.
///
/// Everything between them is the value, newlines included. A line break inside PATH is
/// a broken line in a startup file - an `export PATH="...` carried onto the next line
/// inside its quotes - and it cost this machine far more than one directory when PATH was
/// read a line at a time out of `env`: everything after the break went with it, which was
/// `/opt/homebrew/bin` and `~/.local/bin`, and so `pi`, `aip`, `awt` and `claude`. So the
/// entry the break lands in is dropped, because the shell cannot find anything there
/// either, and every entry around it is kept.
fn path_between_markers(output: &str) -> Option<String> {
    let body = output.split_once(BEGIN)?.1;
    let body = body.split_once(END).map_or(body, |(before, _)| before);
    // `printf '%s\n'` ends the marker's line and `printenv` ends the value's.
    let value = body.strip_prefix('\n').unwrap_or(body);
    let value = value.strip_suffix('\n').unwrap_or(value);
    let dirs = std::env::split_paths(value)
        .filter(|dir| !dir.as_os_str().to_string_lossy().contains('\n'));
    let path = std::env::join_paths(dirs).ok()?.into_string().ok()?;
    (!path.is_empty()).then_some(path)
}

/// Run a command, and give up on it rather than wait forever.
///
/// Written out rather than reached for from tokio because this runs before there is a
/// runtime - that ordering is the whole point of [`adopt_login_path`].
fn wait_briefly(mut command: Command) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + PATIENCE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Err(_) => return None,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn joined(value: &OsString) -> Vec<String> {
        std::env::split_paths(value)
            .map(|dir| dir.display().to_string())
            .collect()
    }

    #[test]
    fn the_shells_directories_are_added_after_the_ones_already_there() {
        // The launchd case, which is the whole reason this module exists: four system
        // directories, and everything the operator installed somewhere else.
        let launchd = OsString::from("/usr/bin:/bin:/usr/sbin:/sbin");
        let login = OsString::from("/Users/x/.local/bin:/opt/homebrew/bin:/usr/bin:/bin");

        let merged = merge(&launchd, &login);
        assert_eq!(
            joined(&merged),
            [
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
                "/Users/x/.local/bin",
                "/opt/homebrew/bin",
            ]
        );
    }

    #[test]
    fn a_process_started_from_a_terminal_is_left_exactly_as_it_was() {
        // The common case, and the one that must not change: `ait` in a shell already has
        // the answer, so adopting it is a no-op rather than a reordering.
        let full = OsString::from("/Users/x/.local/bin:/opt/homebrew/bin:/usr/bin:/bin");
        assert_eq!(joined(&merge(&full, &full)), joined(&full));
    }

    #[test]
    fn a_narrowed_environment_keeps_its_own_precedence() {
        // A sandbox or a test harness that put something first did so deliberately. This
        // only ever appends places to look; it never promotes one.
        let narrowed = OsString::from("/opt/fixtures/bin:/usr/bin");
        let login = OsString::from("/opt/homebrew/bin:/usr/bin");

        let merged = merge(&narrowed, &login);
        assert_eq!(
            joined(&merged),
            ["/opt/fixtures/bin", "/usr/bin", "/opt/homebrew/bin"]
        );
    }

    #[test]
    fn an_empty_entry_is_dropped_rather_than_inherited() {
        // Most shells read an empty PATH entry as the current directory. Spawning tools by
        // name out of whatever directory a run happens to be in is not something to adopt.
        let merged = merge(&OsString::from("/usr/bin::/bin"), &OsString::from(""));
        assert_eq!(joined(&merged), ["/usr/bin", "/bin"]);
    }

    #[test]
    fn nothing_from_the_shell_leaves_the_path_alone() {
        let inherited = OsString::from("/usr/bin:/bin");
        assert_eq!(
            joined(&merge(&inherited, &OsString::new())),
            joined(&inherited)
        );
    }

    #[test]
    fn a_greeting_shell_does_not_get_parsed_as_an_answer() {
        // What the markers are for. An interactive shell prints motd, a version manager's
        // notice and whatever the rc files feel like saying - none of which is PATH, and
        // one line of which could easily look like it.
        let noisy = format!(
            "Welcome to this machine\nPATH=/nonsense/from/a/banner\n\
             {BEGIN}\n/usr/bin:/bin\n{END}\nlogout\n"
        );
        assert_eq!(
            path_between_markers(&noisy).as_deref(),
            Some("/usr/bin:/bin")
        );
    }

    #[test]
    fn a_shell_with_no_path_answers_nothing_rather_than_something() {
        // Unset, where `printenv` prints nothing, and set to nothing.
        assert_eq!(path_between_markers(&format!("{BEGIN}\n{END}\n")), None);
        assert_eq!(path_between_markers(&format!("{BEGIN}\n\n{END}\n")), None);
        // No markers at all is a shell that never ran the script - a failure, not an empty
        // answer to be merged in.
        assert_eq!(path_between_markers("/usr/bin"), None);
    }

    #[test]
    fn a_line_break_inside_path_costs_that_entry_and_not_the_rest() {
        // The reporting machine's .zshrc, give or take the home directory: an `export
        // PATH="...:$ANDROID_HOME` continued onto the next line inside its quotes. Read a
        // line at a time, PATH ended at the break and took every tool ai-team runs with it.
        let answer = format!(
            "{BEGIN}\n/Users/x/.opencode/bin:/Users/x/Library/Android/sdk\n  /tools/bin:\
             /opt/homebrew/bin:/Users/x/.local/bin:/usr/bin:/bin\n{END}\n"
        );
        assert_eq!(
            path_between_markers(&answer).as_deref(),
            Some("/Users/x/.opencode/bin:/opt/homebrew/bin:/Users/x/.local/bin:/usr/bin:/bin")
        );
    }

    #[test]
    fn the_shell_answers_on_both_platforms() {
        // Runs on both CI legs on purpose (D12): macOS logs in with zsh and Linux with
        // bash, the two disagree about which startup files each mode reads, and this is
        // the one place ai-team depends on that behaviour. It is also the only part of
        // this module that proves the script is spelled in a dialect both understand.
        //
        // Skipped rather than failed where there is no $SHELL - a container image that
        // does not set one is a real environment, and the code path already handles it.
        if std::env::var("SHELL").is_err() {
            return;
        }
        let Some(path) = login_shell_path() else {
            return;
        };
        assert!(
            !path.contains('\n'),
            "a newline would join one directory to the next: {path:?}"
        );
        assert!(
            std::env::split_paths(&path).any(|dir| dir == Path::new("/usr/bin")),
            "every shell should know about /usr/bin: {path:?}"
        );
    }

    #[test]
    fn launchctl_environment_values_are_trimmed_and_blank_answers_are_rejected() {
        assert_eq!(
            launchctl_value(b"/var/run/com.apple.launchd.test/Listeners\n"),
            Some(OsString::from("/var/run/com.apple.launchd.test/Listeners"))
        );
        assert_eq!(launchctl_value(b" \n"), None);
        assert_eq!(launchctl_value(b"not utf8: \xff"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_launch_recovers_the_live_launchd_ssh_agent() {
        const CHILD: &str = "AI_TEAM_TEST_LAUNCHD_SSH_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert!(std::env::var_os("SSH_AUTH_SOCK").is_none());
            adopt_launchd_ssh_agent();
            let expected = Command::new("/bin/launchctl")
                .args(["getenv", "SSH_AUTH_SOCK"])
                .output()
                .unwrap();
            assert_eq!(
                std::env::var_os("SSH_AUTH_SOCK"),
                launchctl_value(&expected.stdout)
            );
            return;
        }

        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "launch_path::tests::finder_launch_recovers_the_live_launchd_ssh_agent",
                "--nocapture",
            ])
            .env_remove("SSH_AUTH_SOCK")
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn merging_twice_changes_nothing_the_second_time() {
        // What makes a second `adopt_login_path` a no-op, asserted on the pure function
        // rather than by calling the real one: `set_var` races every other test spawning a
        // process, and this crate's tests spawn plenty. A merge that was not idempotent
        // would grow PATH on every call.
        let inherited = OsString::from("/usr/bin:/bin");
        let login = OsString::from("/opt/homebrew/bin:/usr/bin");

        let once = merge(&inherited, &login);
        assert_eq!(joined(&merge(&once, &login)), joined(&once));
    }
}
