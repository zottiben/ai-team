//! Provider reachability for `ait doctor`.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::model::Provider;

use super::ailocal::{non_empty_env, AilocalSettings};
use super::ZAI_KEY_ENV;

/// The three states `ait doctor` presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderState {
    Allowed,
    Denied,
    Unreachable,
}

impl ProviderState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderState::Allowed => "allowed",
            ProviderState::Denied => "denied",
            ProviderState::Unreachable => "unreachable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStatus {
    pub provider: Provider,
    pub state: ProviderState,
    pub detail: String,
}

/// Every provider in D8's registry now has a generated execution path.
pub(super) fn implemented(_provider: Provider) -> bool {
    true
}

pub(crate) fn provider_default(provider: Provider) -> (&'static str, i64) {
    match provider {
        Provider::Claude => ("sonnet", 200_000),
        Provider::OpenAi => ("gpt-5.6-luna-fast", 32_768),
        Provider::ZAi => ("glm-4.6", 32_768),
        Provider::Local => ("auto", 32_768),
    }
}

pub(super) fn probe(provider: Provider, ailocal: &AilocalSettings) -> ProviderStatus {
    let result = match provider {
        Provider::Claude => probe_claude(),
        Provider::OpenAi => probe_chatgpt(),
        Provider::ZAi => non_empty_env(ZAI_KEY_ENV)
            .map(|_| "GLM Coding Plan key is set".to_string())
            .ok_or_else(|| "AI_TEAM_ZAI_KEY is not set".to_string()),
        Provider::Local => ailocal.probe(),
    };
    match result {
        Ok(detail) => ProviderStatus {
            provider,
            state: ProviderState::Allowed,
            detail,
        },
        Err(detail) => ProviderStatus {
            provider,
            state: ProviderState::Unreachable,
            detail,
        },
    }
}

fn probe_claude() -> std::result::Result<String, String> {
    let mut command = Command::new("claude");
    command.args(["auth", "status", "--json"]);
    let output = output_with_timeout(command)
        .map_err(|_| "Claude Code CLI is not installed".to_string())?
        .ok_or_else(|| "Claude Code auth status timed out".to_string())?;
    if !output.status.success() {
        return Err("Claude Code is not signed in".into());
    }
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Claude Code returned an unreadable auth status".to_string())?;
    if status.get("loggedIn").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("Claude Code is not signed in".into());
    }
    Ok("Claude subscription authenticated through Claude Code".into())
}

fn probe_chatgpt() -> std::result::Result<String, String> {
    let mut codex = Command::new("codex");
    codex.args(["login", "status"]);
    if success_with_timeout(codex) {
        return Ok("ChatGPT subscription authenticated through Codex".into());
    }
    if eve_chatgpt_secret_exists() {
        return Ok("ChatGPT subscription authenticated through eve".into());
    }
    Err("no ChatGPT subscription login; use /login in `eve dev`".into())
}

fn eve_chatgpt_secret_exists() -> bool {
    // eve's just-secrets store base64-encodes these two identifiers. We ask only whether
    // the item exists; its value is never printed or returned to ai-team.
    const SERVICE: &str = "secrets:v1:ZXZl";
    const NAME: &str = "secrets:v1:Y2hhdGdwdA==";
    if cfg!(target_os = "macos") {
        let mut command = Command::new("/usr/bin/security");
        command.args([
            "-q",
            "find-generic-password",
            "-s",
            SERVICE,
            "-a",
            NAME,
            "login.keychain-db",
        ]);
        return success_with_timeout(command);
    }
    if cfg!(target_os = "linux") {
        // `lookup` emits the secret on stdout; discard it at the OS pipe boundary.
        let mut command = Command::new("/usr/bin/secret-tool");
        command.args([
            "lookup",
            "application",
            "secrets",
            "service",
            SERVICE,
            "name",
            NAME,
        ]);
        return success_with_timeout(command);
    }
    false
}

fn success_with_timeout(mut command: Command) -> bool {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Err(_) => return false,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn output_with_timeout(mut command: Command) -> std::io::Result<Option<Output>> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait()? {
            Some(_) => return child.wait_with_output().map(Some),
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                child.kill()?;
                child.wait()?;
                return Ok(None);
            }
        }
    }
}
