//! Supervised child processes: `npm install`, `eve build`, `eve start`.
//!
//! Two properties matter more than anything else here.
//!
//! **A started process must not outlive its supervisor.** `eve start` is a Node server
//! holding a port; leaking one on every crash leaves a developer with a machine full of
//! servers serving agents nobody is driving. [`EveProcess`] kills on drop.
//!
//! **A build must report progress.** `eve build` takes tens of seconds and the UI has to
//! show something, so its output is streamed line by line rather than collected at the
//! end (M1-S4's bar).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::error::{Error, Result};

/// How long to wait for a started process to answer `/eve/v1/health`.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

/// Credential/routing switches that could turn a subscription-backed generated model
/// into metered API traffic merely because the operator's shell exported one. Keep the
/// rest of the environment (PATH, HOME, git/cloud credentials used by authored tools),
/// and preserve Claude OAuth credentials: those are the permitted subscription path.
const METERED_MODEL_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "AI_GATEWAY_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

fn remove_metered_environment(command: &mut Command) {
    for key in METERED_MODEL_ENV {
        command.env_remove(key);
    }
}

/// The environment a generated project needs, assembled in one place so a call site
/// cannot forget one and get a confusing failure three layers down.
#[derive(Debug, Clone)]
pub struct EveEnv {
    /// The leased worktree this process may touch (D3, D10).
    pub worktree: PathBuf,
    /// The shared secret its channel checks.
    pub token: String,
    /// Provider keys, as `(name, value)`.
    pub provider_keys: Vec<(String, String)>,
}

impl EveEnv {
    fn apply(&self, command: &mut Command) {
        command.env("AI_TEAM_WORKTREE", &self.worktree);
        command.env("AI_TEAM_EVE_TOKEN", &self.token);
        for (key, value) in &self.provider_keys {
            command.env(key, value);
        }
    }
}

/// A line of output from a supervised command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressLine {
    pub text: String,
    /// True when it came from stderr. npm and eve both use it for ordinary progress, so
    /// this is a channel marker rather than a severity.
    pub stderr: bool,
}

/// Run a command to completion, handing each output line to `on_line` as it arrives.
///
/// Both streams are read concurrently: reading stdout to the end first deadlocks as soon
/// as the child fills the stderr pipe, which npm does reliably on a cold install.
pub(super) async fn run_streaming<F>(
    program: &str,
    args: &[&str],
    dir: &Path,
    env: Option<&EveEnv>,
    mut on_line: F,
) -> Result<()>
where
    F: FnMut(ProgressLine) + Send,
{
    let mut command = Command::new(program);
    remove_metered_environment(&mut command);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(env) = env {
        env.apply(&mut command);
    }

    let mut child = command.spawn().map_err(|e| {
        Error::invalid(format!("could not run `{program} {}`: {e}", args.join(" ")))
    })?;

    let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();
    let mut stderr = BufReader::new(child.stderr.take().expect("piped")).lines();

    loop {
        tokio::select! {
            line = stdout.next_line() => match line? {
                Some(text) => on_line(ProgressLine { text, stderr: false }),
                None => break,
            },
            line = stderr.next_line() => match line? {
                Some(text) => on_line(ProgressLine { text, stderr: true }),
                None => break,
            },
        }
    }

    // One pipe closed; drain whatever is left on the other before waiting.
    while let Some(text) = stdout.next_line().await? {
        on_line(ProgressLine {
            text,
            stderr: false,
        });
    }
    while let Some(text) = stderr.next_line().await? {
        on_line(ProgressLine { text, stderr: true });
    }

    let status = child.wait().await?;
    if status.success() {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "`{program} {}` failed with {status}",
        args.join(" ")
    )))
}

/// A running `eve start`, bound to one port and one worktree.
#[derive(Debug)]
pub struct EveProcess {
    child: Child,
    port: u16,
    token: String,
}

impl EveProcess {
    /// Start the built output on a port the OS chose.
    ///
    /// The port is picked by binding and immediately releasing, which has a race that
    /// does not matter here: the window is microseconds and the only other thing racing
    /// for ports on this machine is another ai-team node, which would fail loudly at
    /// bind rather than silently share.
    pub fn start(dir: &Path, env: &EveEnv) -> Result<EveProcess> {
        let port = free_port()?;
        let mut command = Command::new("npx");
        command
            .args(["eve", "start", "--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            .current_dir(dir)
            .stdin(Stdio::null())
            // eve's own logging is not ai-team's event stream; the interesting output is
            // on /eve/v1, and keeping these pipes open would need a reader forever.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // npx is a wrapper. Killing only it leaves its Node child serving forever, so
        // every supervised tree gets its own process group and the group is what Drop
        // terminates. This is available on both daily-use targets (macOS and Linux).
        #[cfg(unix)]
        command.process_group(0);
        remove_metered_environment(&mut command);
        env.apply(&mut command);

        let child = command
            .spawn()
            .map_err(|e| Error::invalid(format!("could not start eve: {e}")))?;

        Ok(EveProcess {
            child,
            port,
            token: env.token.clone(),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn client(&self) -> super::EveClient {
        super::EveClient::new(self.port, &self.token)
    }

    /// Wait for the process to serve, failing if it died on the way up.
    pub async fn wait_until_ready(&mut self) -> Result<()> {
        let client = self.client();
        let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            // Check liveness first: a crashed child would otherwise be polled until the
            // timeout, and report "never became healthy" instead of why it exited.
            if let Some(status) = self.child.try_wait()? {
                return Err(Error::invalid(format!(
                    "eve exited with {status} before it began serving on port {}",
                    self.port
                )));
            }
            if client.healthy().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(Error::invalid(format!(
            "eve did not answer on port {} within {}s",
            self.port,
            HEALTH_TIMEOUT.as_secs()
        )))
    }

    /// Still running?
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Stop it, and wait for it to actually be gone.
    pub async fn stop(mut self) -> Result<()> {
        signal_process_tree(&mut self.child, "TERM");
        if tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .is_err()
        {
            signal_process_tree(&mut self.child, "KILL");
            let _ = self.child.wait().await;
        }
        Ok(())
    }
}

impl Drop for EveProcess {
    fn drop(&mut self) {
        signal_process_tree(&mut self.child, "KILL");
    }
}

fn signal_process_tree(child: &mut Child, signal: &str) {
    #[cfg(unix)]
    if let Some(pid) = child
        .id()
        .and_then(|pid| rustix::process::Pid::from_raw(pid.cast_signed()))
    {
        let signal = match signal {
            "TERM" => rustix::process::Signal::TERM,
            _ => rustix::process::Signal::KILL,
        };
        // The child was placed in a group whose ID is its PID at spawn time.
        let _ = rustix::process::kill_process_group(pid, signal);
    }
    // Windows has no Unix process groups; this at least preserves the old direct-child
    // guarantee there. macOS and Linux take the group path above.
    let _ = child.start_kill();
}

/// Ask the OS for a free loopback port.
pub fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::invalid(format!("no free loopback port: {e}")))?;
    Ok(listener.local_addr()?.port())
}

/// A per-process shared secret, hex-encoded.
///
/// Minted fresh each time rather than stored: it protects one process for as long as
/// that process lives, and a secret on disk is a secret that outlives what it protects.
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
    use super::*;

    fn env() -> EveEnv {
        EveEnv {
            worktree: PathBuf::from("/tmp"),
            token: "secret".into(),
            provider_keys: vec![("AI_TEAM_AILOCAL_KEY".into(), "k".into())],
        }
    }

    #[tokio::test]
    async fn output_is_streamed_line_by_line_from_both_pipes() {
        let mut lines = Vec::new();
        run_streaming(
            "sh",
            &["-c", "echo one; echo two >&2; echo three"],
            Path::new("."),
            None,
            |line| lines.push(line),
        )
        .await
        .unwrap();

        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"one"), "{texts:?}");
        assert!(texts.contains(&"three"), "{texts:?}");
        // stderr is progress, not severity: npm and eve both write there routinely.
        let two = lines.iter().find(|l| l.text == "two").expect("stderr line");
        assert!(two.stderr);
    }

    #[tokio::test]
    async fn a_failing_command_reports_the_command_and_the_status() {
        let err = run_streaming("sh", &["-c", "exit 3"], Path::new("."), None, |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("exit"), "{err}");
        assert!(err.contains("sh"), "{err}");
    }

    #[tokio::test]
    async fn a_command_that_does_not_exist_says_so_rather_than_hanging() {
        let err = run_streaming(
            "definitely-not-a-program",
            &[],
            Path::new("."),
            None,
            |_| {},
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("definitely-not-a-program"), "{err}");
    }

    #[tokio::test]
    async fn a_lot_of_output_on_both_pipes_does_not_deadlock() {
        // The failure this guards: reading stdout to the end first wedges as soon as the
        // child fills the stderr pipe, which npm does on any cold install.
        let mut count = 0usize;
        run_streaming(
            "sh",
            &[
                "-c",
                "for i in $(seq 1 2000); do echo out-$i; echo err-$i >&2; done",
            ],
            Path::new("."),
            None,
            |_| count += 1,
        )
        .await
        .unwrap();
        assert_eq!(count, 4000);
    }

    #[tokio::test]
    async fn the_environment_reaches_the_child() {
        let mut seen = Vec::new();
        run_streaming(
            "sh",
            &[
                "-c",
                "echo $AI_TEAM_WORKTREE $AI_TEAM_EVE_TOKEN $AI_TEAM_AILOCAL_KEY",
            ],
            Path::new("."),
            Some(&env()),
            |line| seen.push(line.text),
        )
        .await
        .unwrap();
        assert_eq!(seen, ["/tmp secret k"]);
    }

    #[tokio::test]
    async fn metered_model_credentials_never_reach_a_generated_process() {
        let mut command = Command::new("sh");
        for key in METERED_MODEL_ENV {
            command.env(key, "must-not-leak");
        }
        // OAuth is the Claude subscription credential and must survive the filter.
        command.env("CLAUDE_CODE_OAUTH_TOKEN", "subscription-ok");
        remove_metered_environment(&mut command);
        command.args([
            "-c",
            "test -z \"$ANTHROPIC_API_KEY$ANTHROPIC_AUTH_TOKEN$ANTHROPIC_BASE_URL\
             $OPENAI_API_KEY$AI_GATEWAY_API_KEY$CLAUDE_CODE_USE_BEDROCK\
             $CLAUDE_CODE_USE_VERTEX$CLAUDE_CODE_USE_FOUNDRY\" &&\
             test \"$CLAUDE_CODE_OAUTH_TOKEN\" = subscription-ok",
        ]);
        let status = command.status().await.unwrap();
        assert!(status.success(), "metered model environment leaked");
    }

    #[test]
    fn a_minted_token_is_hex_and_not_repeated() {
        let a = mint_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, mint_token());
    }

    #[test]
    fn free_ports_are_actually_free() {
        let port = free_port().unwrap();
        // If it were still held, binding again would fail.
        let bound = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(bound.is_ok(), "port {port} was not released");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_wrappers_descendant_is_killed_when_the_eve_handle_drops() {
        // npx -> node is the real shape. Testing `sleep` directly only proves that
        // kill_on_drop kills the wrapper, which is how seven old eve servers escaped.
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child.pid");
        let script = format!("sleep 300 & echo $! > {}; wait", pid_file.display());
        let mut command = Command::new("sh");
        command
            .args(["-c", &script])
            .process_group(0)
            .kill_on_drop(true)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().unwrap();
        let process = EveProcess {
            child,
            port: 0,
            token: "test".into(),
        };

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let descendant: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(process_running(descendant));

        drop(process);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while process_running(descendant) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !process_running(descendant),
            "descendant {descendant} outlived its eve handle"
        );
    }

    #[cfg(unix)]
    fn process_running(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // A CI container's PID 1 may leave the killed grandchild as a zombie for
            // longer than this test. `kill -0` still sees a zombie, but it cannot serve
            // requests and the group termination guarantee has succeeded.
            return stat
                .rsplit_once(") ")
                .is_some_and(|(_, tail)| !tail.starts_with('Z'));
        }

        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}
