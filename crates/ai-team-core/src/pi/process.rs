//! One Pi turn, as a supervised child process.
//!
//! This is the whole of what replaced `supervise/http.rs`, `supervise/client.rs` and most
//! of `supervise/process.rs`: Pi writes NDJSON to stdout, so the transport is a pipe.
//! There is no port to allocate, no token to mint, no readiness to wait for and no
//! chunked framing to get right - a line is a line.
//!
//! Two things carry over unchanged from the eve supervisor, because both were learned the
//! hard way rather than designed:
//!
//! - **Read both pipes concurrently.** Draining stdout first deadlocks the moment the
//!   child fills the stderr pipe buffer and blocks writing to it.
//! - **Signal the process group, not the child.** Pi spawns MCP servers and tool
//!   subprocesses; killing only the direct child leaves them running. The child is put in
//!   its own group so the group can be signalled without touching ai-team itself.

use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::event::PiEvent;
use crate::error::{Error, Result};

/// Environment variables that reach a metered account, removed from every child.
///
/// Not emitting them is not enough: the operator's own shell may already export them, and
/// a child inherits the environment. D8 is about which account pays, so it has to be
/// enforced where the process is created rather than where the model is chosen.
const METERED_MODEL_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
];

/// How a seat is invoked.
///
/// Every field is a Pi flag rather than generated TypeScript, which is the practical
/// difference D20 makes: changing a seat's model is an argument, not a rebuild.
#[derive(Debug, Clone)]
pub struct PiTurn {
    /// The leased worktree. Pi's working directory, which is also what its AGENTS.md,
    /// CLAUDE.md, skill and `.mcp.json` discovery resolves against (D19).
    pub worktree: PathBuf,
    /// What to do this turn.
    pub prompt: String,
    /// The provider as the registry resolved it, already checked against the machine
    /// profile (D8). `None` leaves Pi on its own default, which only a test wants.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Pi's thinking level, from the seat's effort.
    pub thinking: Option<String>,
    /// Tools this seat may not use. A read-only seat withholds `write` and `edit`.
    pub exclude_tools: Vec<String>,
    /// The MCP config for this seat, carrying D15's per-tool allow-lists.
    pub mcp_config: Option<PathBuf>,
    /// Resume this session rather than starting one.
    pub session_id: Option<String>,
    /// The ai-team guard extension (D20). Pi's own `bash`, `read`, `write` and `edit`
    /// answer to nobody; this is what holds them to the lease and refuses to publish.
    pub guard: Option<PathBuf>,
    /// What this seat is.
    ///
    /// Carried at the top of the prompt rather than through `--append-system-prompt`,
    /// which looks like the right place and silently is not: on `claude-subscription`
    /// the Claude Code CLI owns the system prompt and Pi's additions never reach the
    /// model. A fact placed there could not be read back out; the same fact in the
    /// prompt arrives every time. One mechanism, working on every provider, is worth
    /// more than the right-looking one that works on some.
    ///
    /// Not optional in practice: a seat with no instructions is a general-purpose coding
    /// assistant holding a `bash` tool, which is how the first Pi run had the
    /// orchestrator write the code instead of the plan.
    pub instructions: Option<String>,
    /// What this seat's process is given, as `NAME=value` pairs.
    ///
    /// The context tokens its own MCP servers need (D23), and only the ones its sources
    /// actually use - a seat with no ClickUp server is not handed a ClickUp token - plus
    /// `AI_PLANNER_PLAN`, so its own `aip` calls act on its run's plan. They arrive here
    /// rather than through the ambient environment because the window is started from
    /// Finder, which has never read a shell profile.
    pub environment: Vec<(String, String)>,
}

impl PiTurn {
    /// A turn in a worktree, with nothing else decided.
    pub fn new(worktree: impl Into<PathBuf>, prompt: impl Into<String>) -> PiTurn {
        PiTurn {
            worktree: worktree.into(),
            prompt: prompt.into(),
            provider: None,
            model: None,
            thinking: None,
            exclude_tools: Vec::new(),
            mcp_config: None,
            session_id: None,
            guard: None,
            instructions: None,
            environment: Vec::new(),
        }
    }

    /// The argument list, in one place so it can be asserted on without spawning.
    fn args(&self) -> Vec<String> {
        let mut args = vec!["--mode".into(), "json".into(), "--print".into()];
        if let Some(provider) = &self.provider {
            args.push("--provider".into());
            args.push(provider.clone());
        }
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if let Some(thinking) = &self.thinking {
            args.push("--thinking".into());
            args.push(thinking.clone());
        }
        if !self.exclude_tools.is_empty() {
            args.push("--exclude-tools".into());
            args.push(self.exclude_tools.join(","));
        }
        if let Some(config) = &self.mcp_config {
            args.push("--mcp-config".into());
            args.push(config.to_string_lossy().into_owned());
        }
        if let Some(session) = &self.session_id {
            args.push("--session-id".into());
            args.push(session.clone());
        }
        if let Some(guard) = &self.guard {
            args.push("--extension".into());
            args.push(guard.to_string_lossy().into_owned());
        }

        // `--` ends option parsing, so a prompt that happens to begin with a dash is a
        // prompt rather than an unknown flag.
        args.push("--".into());
        args.push(match &self.instructions {
            Some(instructions) => format!("{instructions}\n\n---\n\n{}", self.prompt),
            None => self.prompt.clone(),
        });
        args
    }
}

/// A running Pi turn.
#[derive(Debug)]
pub struct PiProcess {
    child: Child,
    pid: i32,
}

impl PiProcess {
    /// Start a turn.
    ///
    /// The worktree must exist: Pi resolves everything relative to it, and a missing
    /// directory fails as an opaque spawn error several layers from the cause.
    pub fn start(turn: &PiTurn) -> Result<PiProcess> {
        if !turn.worktree.is_dir() {
            return Err(Error::UnusablePath {
                path: turn.worktree.clone(),
                reason: "not a directory - a turn runs inside its lease".into(),
            });
        }

        let mut command = Command::new("pi");
        for key in METERED_MODEL_ENV {
            command.env_remove(key);
        }
        command
            .args(turn.args())
            .current_dir(&turn.worktree)
            // The lease, named explicitly rather than inferred from the working
            // directory. The guard reads it, and a rule that reads `cwd` is a rule a
            // `cd` changes (D10).
            .env("AI_TEAM_WORKTREE", &turn.worktree)
            // After `env_remove`, deliberately: what this seat is *given* is the last word
            // on its environment, and a credential removed as metered must stay removed.
            // Nothing in `METERED_MODEL_ENV` is a name this list may carry, because these
            // are ai-team's own `AI_TEAM_*_TOKEN` variables and the run's plan, nothing else.
            .envs(
                turn.environment
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str())),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Its own group, so the group can be signalled without killing ai-team. Pi starts
        // MCP servers and tool subprocesses; signalling only the direct child leaves
        // those running.
        //
        // Unix only, because process groups are. Windows gets `kill_on_drop` alone, which
        // reaches the child and not its descendants - a real difference, and one nobody
        // is running into, because ai-team targets macOS and is built on Linux (D12). The
        // Windows binary exists so the release does not lie about what it builds.
        #[cfg(unix)]
        command.process_group(0);

        let child = command.spawn().map_err(|e| {
            Error::invalid(format!(
                "starting `pi`: {e}. ai-team drives Pi as its runtime (D20) and does not \
                 install it - `pi --version` should work in this shell."
            ))
        })?;
        let pid = child.id().unwrap_or(0).cast_signed();
        Ok(PiProcess { child, pid })
    }

    /// Read the turn to its end, handing every parsed event to `on_event`.
    ///
    /// Both pipes are read concurrently. Draining stdout first deadlocks the moment the
    /// child fills the stderr buffer, which on a failing turn is exactly when it happens.
    pub async fn drive<F>(&mut self, mut on_event: F) -> Result<TurnOutcome>
    where
        F: FnMut(&PiEvent) + Send,
    {
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or_else(|| Error::invalid("pi produced no stdout"))?;
        let stderr = self
            .child
            .stderr
            .take()
            .ok_or_else(|| Error::invalid("pi produced no stderr"))?;

        let mut out = BufReader::new(stdout).lines();
        let mut err = BufReader::new(stderr).lines();

        let mut outcome = TurnOutcome::default();
        let mut latest_provider_turn_failed = false;
        let mut stderr_tail = Vec::new();
        let mut out_open = true;
        let mut err_open = true;

        while out_open || err_open {
            tokio::select! {
                line = out.next_line(), if out_open => match line {
                    Ok(Some(line)) => {
                        let Some(event) = PiEvent::parse(&line) else { continue };
                        if let Some(id) = event.session_id() {
                            outcome.session_id = Some(id.to_string());
                        }
                        if let Some(usage) = event.usage() {
                            outcome.usage = Some(usage);
                        }
                        if let Some(text) = event.assistant_message() {
                            outcome.said = Some(text);
                        }
                        if event.is_failure() {
                            outcome.failed = true;
                        }
                        if let Some(failed) = event.provider_turn_failed() {
                            // A later provider retry may succeed before `agent_settled`.
                            latest_provider_turn_failed = failed;
                        }
                        if event.is_terminal() {
                            outcome.settled = true;
                        }
                        on_event(&event);
                    }
                    _ => out_open = false,
                },
                line = err.next_line(), if err_open => match line {
                    Ok(Some(line)) => {
                        // Bounded: a failing child can write without limit, and the point
                        // of keeping this is a readable reason, not a transcript.
                        if stderr_tail.len() < 40 {
                            stderr_tail.push(line);
                        }
                    }
                    _ => err_open = false,
                },
            }
        }

        let status = self
            .child
            .wait()
            .await
            .map_err(|e| Error::invalid(format!("waiting for pi: {e}")))?;
        outcome.exit_code = status.code();
        outcome.stderr = stderr_tail.join("\n");
        outcome.failed |= latest_provider_turn_failed;

        // A turn that never settled is not a turn that succeeded, whatever its exit code:
        // the stream is the record, and a truncated one means the child died mid-turn.
        if !outcome.settled && outcome.exit_code == Some(0) {
            outcome.failed = true;
        }
        Ok(outcome)
    }

    /// Stop the turn and everything it started.
    #[cfg(unix)]
    pub fn stop(&mut self) {
        if self.pid > 0 {
            if let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
            }
        }
    }

    /// Stop the turn.
    ///
    /// No process group to signal, so this reaches the child and not what it started.
    #[cfg(not(unix))]
    pub fn stop(&mut self) {
        let _ = self.pid;
        let _ = self.child.start_kill();
    }
}

impl Drop for PiProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a finished turn amounted to.
#[derive(Debug, Clone, Default)]
pub struct TurnOutcome {
    /// Pi's session, so a later turn can resume this one.
    pub session_id: Option<String>,
    /// The last usage the stream reported.
    pub usage: Option<crate::model::Usage>,
    /// The assistant's last words, captured from the stream rather than read back out of
    /// the event table (rule 8).
    pub said: Option<String>,
    /// Whether the stream reached `agent_settled`.
    pub settled: bool,
    pub failed: bool,
    pub exit_code: Option<i32>,
    /// A bounded tail of stderr, for the failure message.
    pub stderr: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_names_its_mode_and_ends_option_parsing() {
        let turn = PiTurn::new("/tmp", "do the thing");
        let args = turn.args();
        assert!(args.starts_with(&["--mode".into(), "json".into(), "--print".into()]));
        // `--` before the prompt, so a prompt starting with a dash is not read as a flag.
        let end = args.iter().position(|a| a == "--").expect("-- separator");
        assert_eq!(args[end + 1], "do the thing");
        assert_eq!(end + 2, args.len(), "the prompt is last");
    }

    #[test]
    fn a_seat_s_model_and_effort_are_flags_not_a_rebuild() {
        // The practical difference D20 makes: changing a seat is an argument.
        let mut turn = PiTurn::new("/tmp", "x");
        turn.provider = Some("claude-subscription".into());
        turn.model = Some("claude-sonnet-5".into());
        turn.thinking = Some("high".into());
        let args = turn.args().join(" ");
        assert!(args.contains("--provider claude-subscription"), "{args}");
        assert!(args.contains("--model claude-sonnet-5"), "{args}");
        assert!(args.contains("--thinking high"), "{args}");
    }

    #[test]
    fn a_read_only_seat_withholds_the_writing_tools() {
        // The access guarantee, expressed where Pi can enforce it. `bash` is still there,
        // so this is "cannot edit source through its tools", not "cannot write a byte" -
        // the same wording the eve seats carried.
        let mut turn = PiTurn::new("/tmp", "x");
        turn.exclude_tools = vec!["write".into(), "edit".into()];
        let args = turn.args().join(" ");
        assert!(args.contains("--exclude-tools write,edit"), "{args}");
    }

    #[test]
    fn a_turn_with_nothing_chosen_passes_no_empty_flags() {
        // An empty `--exclude-tools` would disable nothing while looking deliberate, and
        // an empty `--model` is an error Pi reports three layers from the cause.
        let args = PiTurn::new("/tmp", "x").args().join(" ");
        assert!(!args.contains("--exclude-tools"), "{args}");
        assert!(!args.contains("--model"), "{args}");
        assert!(!args.contains("--session-id"), "{args}");
        assert!(!args.contains("--mcp-config"), "{args}");
    }

    #[test]
    fn a_seat_s_instructions_ride_in_the_prompt() {
        // `--append-system-prompt` is the right-looking place and silently does nothing
        // on `claude-subscription`: the Claude Code CLI owns the system prompt. A fact
        // put there could not be read back out of the model; the same fact in the prompt
        // arrives every time.
        let mut turn = PiTurn::new("/tmp", "Add a subtract function.");
        turn.instructions = Some("You are the orchestrator. Do not write code.".into());
        let args = turn.args();

        assert!(
            !args.iter().any(|a| a == "--append-system-prompt"),
            "{args:?}"
        );
        let prompt = args.last().expect("a prompt");
        assert!(prompt.starts_with("You are the orchestrator."), "{prompt}");
        assert!(prompt.contains("Add a subtract function."), "{prompt}");
    }

    #[test]
    fn resuming_names_the_session() {
        let mut turn = PiTurn::new("/tmp", "carry on");
        turn.session_id = Some("01a0-bd7f".into());
        assert!(turn.args().join(" ").contains("--session-id 01a0-bd7f"));
    }

    #[tokio::test]
    async fn a_turn_outside_a_directory_fails_before_spawning() {
        // Pi resolves its whole world relative to the lease, so a missing one has to fail
        // here rather than as an opaque spawn error.
        let turn = PiTurn::new("/tmp/definitely-not-a-worktree-92f1", "x");
        let started = PiProcess::start(&turn);
        assert!(started.is_err());
    }
}
