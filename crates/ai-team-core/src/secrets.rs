//! The few credentials ai-team holds, in the place the operating system keeps credentials.
//!
//! There are exactly two - the ClickUp and Figma context tokens (D9) - and they exist here
//! because a desktop app cannot be handed an environment variable. The window is started
//! from Finder, Finder does not read `.zshrc`, and "export this before launching" is not an
//! instruction that can be followed by double-clicking an icon.
//!
//! Not in `machine.toml` (D23): that file is policy, it is meant to be hand-edited, and it
//! is the sort of thing that ends up in a dotfiles repository. A credential in it travels.
//!
//! The platform store is reached by running its command rather than by linking a crate,
//! which is the same call `machine/probe.rs` already makes: `security` and `secret-tool`
//! are the interfaces macOS and Linux publish, they are already installed, and neither
//! needs a build-time dependency on a system library that CI would have to carry.
//!
//! **A value read out of here is never returned to a surface.** Every function that a
//! route can reach answers whether a token is set, never what it is. The only thing that
//! gets the value is the supervisor, putting it into a child's environment.
//!
//! **Asking is cached; the value is not.** `/doctor` reports on both sources, and the
//! health banner re-reads it on every event tick - so an uncached `held` spawned two
//! `security` processes per tick, which during a run is continuous. macOS notices that
//! kind of traffic and starts putting dialogs in front of somebody who is trying to work.
//! Presence is therefore remembered for [`REMEMBER_FOR`] and invalidated by any write
//! through here; the secret itself is never held in memory between calls.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::machine::ContextSource;

/// What the platform store files these under.
const SERVICE: &str = "ai-team";

/// Where pi-mcp-adapter keeps OAuth credentials in the operating-system store.
///
/// ai-team never reads the value. It asks only whether the account exists, so Settings
/// can distinguish "connect in the browser" from "already connected" without moving a
/// Pi credential through an HTTP response or holding it in this process.
const PI_MCP_OAUTH_SERVICE: &str = "pi-mcp-adapter.oauth";

/// How long "there is a token" is believed without asking again.
///
/// Long enough to collapse a burst of polls into one call, short enough that a token added
/// outside ai-team is picked up within a heartbeat rather than needing a restart. Only
/// presence is cached - never the value.
const REMEMBER_FOR: Duration = Duration::from_secs(5);

/// What the store last said, and when.
static PRESENCE: Mutex<Vec<(ContextSource, bool, Instant)>> = Mutex::new(Vec::new());
static OAUTH_PRESENCE: Mutex<Vec<(ContextSource, bool, Instant)>> = Mutex::new(Vec::new());

fn remembered(source: ContextSource) -> Option<bool> {
    let seen = PRESENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    seen.iter().find_map(|(candidate, present, at)| {
        (*candidate == source && at.elapsed() < REMEMBER_FOR).then_some(*present)
    })
}

fn remember(source: ContextSource, present: bool) -> bool {
    let mut seen = PRESENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    seen.retain(|(candidate, _, _)| *candidate != source);
    seen.push((source, present, Instant::now()));
    present
}

/// Forget what the store said, because ai-team has just changed it.
fn forget(source: ContextSource) {
    PRESENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|(candidate, _, _)| *candidate != source);
}

/// The environment variable a seat's MCP config reads this token from.
///
/// Unchanged from when it was the operator's job to export it, which is the point: only
/// the *source* of the value is new, so a machine that already exports one keeps working
/// and nothing downstream had to learn about a store.
pub fn token_env(source: ContextSource) -> String {
    format!("AI_TEAM_{}_TOKEN", source.as_str().to_uppercase())
}

/// The token for a context source, from wherever this machine keeps it.
///
/// The environment wins over the store. Somebody who already exports one is describing
/// this particular run - a CI job, a `AI_TEAM_CLICKUP_TOKEN=… ait run` - and a stored
/// value quietly overriding that would be the window deciding something the command line
/// had already decided.
pub fn token(source: ContextSource) -> Option<String> {
    if let Some(from_env) = std::env::var(token_env(source))
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Some(from_env);
    }
    read(source.as_str())
}

/// Whether there is one, without producing it.
///
/// What the settings page and the readiness report ask. Separate from [`token`] so that
/// reaching for the value is a deliberate act with one caller, rather than something a
/// route does by accident while rendering a page.
pub fn has_token(source: ContextSource) -> bool {
    held(source) != Held::Absent
}

/// Whether Pi already holds an OAuth credential for this MCP server.
///
/// Pi keys these by the configured **server name**, not the URL. ai-team deliberately
/// names its definitions `clickup` and `figma`, so OAuth completed in an operator's own
/// Pi is the same OAuth its seats use. Only presence is queried; the token value never
/// leaves Pi's credential store.
pub fn has_oauth(source: ContextSource) -> bool {
    let seen = OAUTH_PRESENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(present) = seen.iter().find_map(|(candidate, present, at)| {
        (*candidate == source && at.elapsed() < REMEMBER_FOR).then_some(*present)
    }) {
        return present;
    }
    drop(seen);

    let present = oauth_present(&oauth_account(source));
    let mut seen = OAUTH_PRESENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    seen.retain(|(candidate, _, _)| *candidate != source);
    seen.push((source, present, Instant::now()));
    present
}

/// Forget OAuth status after the browser flow has had a chance to change it.
///
/// The flow itself belongs to Pi, so ai-team cannot invalidate at the exact write like it
/// does for its own token store. "Done - check again" calls this before reloading.
fn oauth_account(source: ContextSource) -> String {
    format!(
        "sha256-{}",
        crate::update::sha256(source.as_str().as_bytes())
    )
}

pub fn forget_oauth(source: ContextSource) {
    OAUTH_PRESENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|(candidate, _, _)| *candidate != source);
}

/// Where the value that is in force came from, for a surface that has to explain itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Held {
    /// Exported into the process ai-team is running in. Not editable from the window,
    /// because the window cannot unset a variable its own process was started with.
    Environment,
    /// In this machine's keychain, put there here.
    Keychain,
    /// Nowhere.
    Absent,
}

pub fn held(source: ContextSource) -> Held {
    // The environment is free to read and always wins, so it is answered before anything
    // is cached or asked of the operating system.
    if std::env::var(token_env(source)).is_ok_and(|value| !value.trim().is_empty()) {
        return Held::Environment;
    }
    let present = match remembered(source) {
        Some(present) => present,
        None => remember(source, read(source.as_str()).is_some()),
    };
    if present {
        Held::Keychain
    } else {
        Held::Absent
    }
}

/// Keep a token, replacing whatever was there.
///
/// An empty value clears rather than storing nothing, because that is what somebody
/// emptying the field in a window means by it.
pub fn set_token(source: ContextSource, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return clear_token(source);
    }
    forget(source);
    write(source.as_str(), value)
}

pub fn clear_token(source: ContextSource) -> Result<()> {
    forget(source);
    remove(source.as_str())
}

// --- which store the policy above talks to ---------------------------------------------
//
// Under `cfg(test)` the policy talks to a map in memory, and the platform call is exercised
// by one `#[ignore]`d test instead. This is not a convenience: the earlier version had every
// unit test read and write the operator's **login keychain**, which on macOS meant a stream
// of authorization dialogs at somebody who was trying to work - and a `find-generic-password`
// for an item that is not there is exactly the "does not exist" modal they reported. A test
// suite is not entitled to a developer's credential store.

#[cfg(not(test))]
use platform::{oauth_present, read, remove, write};

#[cfg(test)]
use fake::{oauth_present, read, remove, write};

/// The store the unit tests see. Never compiled into a release.
#[cfg(test)]
pub(crate) mod fake {
    use super::{Error, Result};
    use std::sync::Mutex;

    static KEPT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    static OAUTH: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Held for the length of any test that touches the store.
    ///
    /// The store is global because the real one is, so tests sharing this process share
    /// it too - and `cargo test` runs them in parallel. Taking turns is the whole fix;
    /// it costs nothing now that no test reaches the operating system.
    pub(crate) static TURN: Mutex<()> = Mutex::new(());

    /// Set by a test that wants to see what a machine with no credential store does.
    pub(super) static BROKEN: Mutex<bool> = Mutex::new(false);

    fn kept() -> std::sync::MutexGuard<'static, Vec<(String, String)>> {
        KEPT.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn broken() -> bool {
        *BROKEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn oauth_present(account: &str) -> bool {
        OAUTH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|candidate| candidate == account)
    }

    #[cfg(test)]
    pub(crate) fn set_oauth(account: &str, present: bool) {
        let mut entries = OAUTH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|candidate| candidate != account);
        if present {
            entries.push(account.to_string());
        }
    }

    pub(super) fn read(account: &str) -> Option<String> {
        kept()
            .iter()
            .find(|(name, _)| name == account)
            .map(|(_, value)| value.clone())
    }

    pub(super) fn write(account: &str, value: &str) -> Result<()> {
        if broken() {
            return Err(Error::invalid("this machine's keychain is unreachable"));
        }
        let mut kept = kept();
        kept.retain(|(name, _)| name != account);
        kept.push((account.to_string(), value.to_string()));
        Ok(())
    }

    // Fallible to match `platform::remove`, which is the point of this module: the policy
    // above must not be able to tell which store it is talking to.
    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn remove(account: &str) -> Result<()> {
        kept().retain(|(name, _)| name != account);
        Ok(())
    }
}

// --- the platform stores -------------------------------------------------------------
//
// Shelled out to rather than linked, which is the same call `machine/probe.rs` makes for
// the same reason: these are the interfaces macOS and Linux publish, they are already
// installed, and neither needs a system library that CI would have to carry.
#[allow(dead_code)]
mod platform {
    use super::{Error, Result, PI_MCP_OAUTH_SERVICE, SERVICE};
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    pub(super) fn oauth_present(account: &str) -> bool {
        // Deliberately no `-w`: status asks Keychain for metadata only. The credential is
        // Pi's, and there is no reason for its value to enter ai-team merely to draw a
        // green dot. Unit tests call the fake above, never this command.
        Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                PI_MCP_OAUTH_SERVICE,
                "-a",
                account,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn oauth_present(account: &str) -> bool {
        // @napi-rs/keyring uses the Secret Service service/username attributes. Discard
        // stdout: `secret-tool lookup` prints the secret and status needs only its exit
        // code. As on macOS, normal tests replace this module with the in-memory fake.
        Command::new("secret-tool")
            .args([
                "lookup",
                "service",
                PI_MCP_OAUTH_SERVICE,
                "username",
                account,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn oauth_present(_account: &str) -> bool {
        false
    }

    #[cfg(target_os = "macos")]
    pub(super) fn read(account: &str) -> Option<String> {
        let mut command = Command::new("/usr/bin/security");
        command.args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"]);
        let output = command
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn write(account: &str, value: &str) -> Result<()> {
        // Two flags, each of which cost something to get right:
        //
        // `-U` updates in place. Without it a second store fails with "item already exists",
        // which reads as a bug in ai-team rather than as the one flag it forgot.
        //
        // `-w` **last, with no value**. Given a value it goes in this process's argv, where
        // `ps` can read it; given last it prompts, and the prompt reads from stdin. `-X` looks
        // like the safer option and is not: it takes hex as an argument, so the secret is
        // still in argv, only spelled differently.
        let mut command = Command::new("/usr/bin/security");
        command.args([
            "add-generic-password",
            "-U",
            "-s",
            SERVICE,
            "-a",
            account,
            "-D",
            "ai-team context token",
            "-w",
        ]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                Error::invalid(format!("this machine's keychain is unreachable: {error}"))
            })?;

        // Twice: it asks for the password and then to retype it. Answering once leaves it
        // waiting for the second, and "passwords don't match" is what it says when the two
        // differ - so the round-trip test is the only thing checking this is still how the
        // tool behaves.
        let typed = format!("{value}\n{value}\n");
        child
            .stdin
            .take()
            .ok_or_else(|| Error::invalid("the keychain tool took no input"))?
            .write_all(typed.as_bytes())
            .map_err(|error| Error::invalid(format!("could not write to the keychain: {error}")))?;

        finish(child, account)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn remove(account: &str) -> Result<()> {
        let mut command = Command::new("/usr/bin/security");
        command.args(["delete-generic-password", "-s", SERVICE, "-a", account]);
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Not there is the state being asked for, so it is not a failure.
        match status {
            Ok(_) => Ok(()),
            Err(error) => Err(Error::invalid(format!(
                "this machine's keychain is unreachable: {error}"
            ))),
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn read(account: &str) -> Option<String> {
        let output = Command::new("secret-tool")
            .args(["lookup", "service", SERVICE, "account", account])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn write(account: &str, value: &str) -> Result<()> {
        // `store` reads the secret from stdin, which is what keeps it out of argv.
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label=ai-team context token",
                "service",
                SERVICE,
                "account",
                account,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                Error::invalid(format!(
                    "no secret service on this machine ({error}) - set {} in the environment instead",
                    format_args!("AI_TEAM_{}_TOKEN", account.to_uppercase())
                ))
            })?;

        child
            .stdin
            .take()
            .ok_or_else(|| Error::invalid("the secret tool took no input"))?
            .write_all(value.as_bytes())
            .map_err(|error| Error::invalid(format!("could not write to the keyring: {error}")))?;

        finish(child, account)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn remove(account: &str) -> Result<()> {
        let status = Command::new("secret-tool")
            .args(["clear", "service", SERVICE, "account", account])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match status {
            Ok(_) => Ok(()),
            Err(error) => Err(Error::invalid(format!(
                "no secret service on this machine: {error}"
            ))),
        }
    }

    /// Neither macOS nor Linux, which is every platform ai-team is not built for (D12).
    ///
    /// A stub rather than a compile error, so the crate still builds where somebody is porting
    /// it, and so the failure is a sentence about the environment instead of a missing symbol.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn read(_account: &str) -> Option<String> {
        None
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn write(account: &str, _value: &str) -> Result<()> {
        Err(Error::invalid(format!(
            "this platform has no credential store ai-team knows - set AI_TEAM_{}_TOKEN in the environment",
            account.to_uppercase()
        )))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn remove(_account: &str) -> Result<()> {
        Ok(())
    }

    /// Wait for a store command and turn its complaint into a sentence.
    ///
    /// The value has already been written to its stdin, so this closes the pipe by dropping
    /// what is left of the child's handles and reads what it said about it.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn finish(child: std::process::Child, account: &str) -> Result<()> {
        let output = child.wait_with_output().map_err(|error| {
            Error::invalid(format!("the credential store did not answer: {error}"))
        })?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.trim();
        Err(Error::invalid(format!(
            "could not store the {account} token{}",
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Take the store, and leave it as a fresh machine would have it.
    ///
    /// Returned as a guard rather than called for its effect: the caller has to hold it
    /// for the length of the test, which is what stops the next test clearing the store
    /// halfway through this one.
    fn blank() -> std::sync::MutexGuard<'static, ()> {
        let turn = fake::TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *fake::BROKEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        for source in ContextSource::ALL {
            clear_token(*source).unwrap();
            fake::set_oauth(&oauth_account(*source), false);
            forget_oauth(*source);
        }
        turn
    }

    #[test]
    fn the_variable_name_is_the_one_a_seat_already_reads() {
        // Unchanged on purpose: only the source of the value is new. A different name here
        // would mean every generated MCP config had to be reasoned about again.
        assert_eq!(token_env(ContextSource::ClickUp), "AI_TEAM_CLICKUP_TOKEN");
        assert_eq!(token_env(ContextSource::Figma), "AI_TEAM_FIGMA_TOKEN");
    }

    #[test]
    fn pi_oauth_presence_uses_the_server_name_account_without_reading_a_token() {
        let _turn = blank();
        let source = ContextSource::ClickUp;
        // This exact account is the one current pi-mcp-adapter writes. The full digest is
        // deliberate: an older investigation mistook its 16-character legacy directory
        // name for the current credential-store account.
        assert_eq!(
            oauth_account(source),
            "sha256-c0d67255424148d869b8c00b4e7c27645b97147e36aaf9538a661a3d87883098"
        );
        assert!(!has_oauth(source));

        fake::set_oauth(&oauth_account(source), true);
        forget_oauth(source);
        assert!(has_oauth(source));

        fake::set_oauth(&oauth_account(source), false);
        forget_oauth(source);
        assert!(!has_oauth(source));
    }

    #[test]
    fn a_token_is_kept_replaced_and_cleared() {
        let _turn = blank();
        let source = ContextSource::ClickUp;

        assert_eq!(held(source), Held::Absent);
        assert!(!has_token(source));

        set_token(source, "shhh").unwrap();
        assert_eq!(held(source), Held::Keychain);
        assert_eq!(token(source).as_deref(), Some("shhh"));

        set_token(source, "different").unwrap();
        assert_eq!(token(source).as_deref(), Some("different"));

        clear_token(source).unwrap();
        assert_eq!(held(source), Held::Absent);
        // Clearing what is already gone is the state being asked for, not a failure.
        clear_token(source).unwrap();
    }

    #[test]
    fn an_empty_value_clears_rather_than_storing_one() {
        // What emptying the field in a window means by it. A stored empty string would
        // read back as "set" and then fail at the first call a seat makes - which is a
        // long way from the page where it was typed.
        let _turn = blank();
        let source = ContextSource::Figma;

        set_token(source, "something").unwrap();
        assert!(has_token(source));

        set_token(source, "   ").unwrap();
        assert!(!has_token(source), "whitespace is not a token");
    }

    #[test]
    fn a_token_is_surrounded_by_whitespace_no_more() {
        // Pasting from a browser brings a trailing newline with it more often than not.
        let _turn = blank();
        let source = ContextSource::ClickUp;
        set_token(source, "  pk_123\n").unwrap();
        assert_eq!(token(source).as_deref(), Some("pk_123"));
    }

    #[test]
    fn one_sources_token_is_not_anothers() {
        let _turn = blank();
        set_token(ContextSource::ClickUp, "clickup-one").unwrap();

        assert_eq!(
            token(ContextSource::ClickUp).as_deref(),
            Some("clickup-one")
        );
        assert_eq!(token(ContextSource::Figma), None);
    }

    #[test]
    fn a_write_that_fails_is_reported_rather_than_swallowed() {
        // A machine with no credential store has to say so, or somebody pastes a token,
        // sees nothing happen, and pastes it again.
        let _turn = blank();
        *fake::BROKEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;

        let refused = set_token(ContextSource::ClickUp, "shhh").unwrap_err();
        assert!(
            refused.to_string().contains("keychain"),
            "{refused} should say what could not be reached"
        );
        assert!(!has_token(ContextSource::ClickUp));
    }

    #[test]
    fn asking_twice_asks_the_store_once() {
        // What stops `/doctor` spawning a `security` process per source per event tick.
        // Counted through the cache rather than the store, because the store under test is
        // a map - what matters is that a second `held` inside the window does not go back
        // to it, which is the property `remembered` provides.
        let _turn = blank();
        let source = ContextSource::Figma;

        assert_eq!(held(source), Held::Absent);
        assert_eq!(
            remembered(source),
            Some(false),
            "the first answer should have been remembered"
        );

        // A write through ai-team invalidates it, so the page does not keep saying "no
        // token" for five seconds after somebody pressed Save.
        set_token(source, "now-there-is").unwrap();
        assert_eq!(
            remembered(source),
            None,
            "a write must forget what was remembered"
        );
        assert_eq!(held(source), Held::Keychain);
    }

    /// The real `security` / `secret-tool` invocation, against the machine's own store.
    ///
    /// Ignored, and it has to stay that way. On macOS this writes to the login keychain
    /// and every call can raise an authorization dialog - which is what the earlier
    /// version of this module did on every `cargo test`, at somebody who was trying to
    /// work. The invocation is finicky enough to be worth a test (`-U` or a second write
    /// fails; `-w` last or the secret lands in `ps`), so it is kept and run deliberately:
    ///
    /// ```text
    /// cargo test -p ai-team-core --lib secrets -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "writes to the operator's real keychain and can raise an OS dialog"]
    fn the_platform_store_round_trips() {
        // A token is somebody else's string: it may begin with a dash, or contain a quote,
        // a space or a dollar, any of which a command line would interpret and a store
        // that pasted it into one would silently mangle.
        const AWKWARD: &str = "-x 'quoted' \"and\" $dollar `tick` \\slash";
        const ACCOUNT: &str = "ai-team-manual-probe";

        platform::write(ACCOUNT, "shhh").expect("this machine should have a credential store");
        assert_eq!(platform::read(ACCOUNT).as_deref(), Some("shhh"));

        // Replaced rather than failing with "item already exists", which is what `-U` buys.
        platform::write(ACCOUNT, AWKWARD).unwrap();
        assert_eq!(platform::read(ACCOUNT).as_deref(), Some(AWKWARD));

        platform::remove(ACCOUNT).unwrap();
        assert_eq!(platform::read(ACCOUNT), None);
    }
}
