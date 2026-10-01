//! Provider reachability for `ait doctor`.

use std::process::Command;
use std::time::Duration;

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
    /// The command that signs this provider in, when there is one to run.
    ///
    /// Present whether or not it is currently needed, because a surface offering to
    /// re-authenticate an expired subscription needs the same command as one offering to
    /// authenticate a new machine. `None` where signing in is not a command - a key in
    /// the environment, or a gateway on loopback that has no account (D25).
    pub sign_in: Option<&'static str>,
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

/// The command that signs a provider in, for a surface that offers to run it (D25).
///
/// A credential flow, not an install: these touch only the operator's own accounts, they
/// open a browser and wait for a person, and none of them can run unattended. Installing a
/// neighbour stays a command that is copied and never run, which is what D17 is about.
pub fn sign_in(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Claude => Some("claude auth login"),
        Provider::OpenAi => Some("codex login"),
        // A GLM Coding Plan key is a value to paste, not a flow to run, and the gateway on
        // loopback has no account at all. Offering a button here would be offering to run
        // a command that does not exist.
        Provider::ZAi | Provider::Local => None,
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
    let (state, detail) = match result {
        Ok(detail) => (ProviderState::Allowed, detail),
        Err(detail) => (ProviderState::Unreachable, detail),
    };
    ProviderStatus {
        provider,
        state,
        detail,
        sign_in: sign_in(provider),
    }
}

/// What the runtime that will actually use this provider says about it.
///
/// Asked first, because ai-team does not hold credentials - Pi does (D20), and a provider
/// Pi cannot authenticate is one no turn can use however healthy its own CLI looks.
///
/// `provider_not_found` is not an answer about credentials, it is Pi saying it has never
/// heard of that provider - which is what it says for an extension-supplied one like
/// `claude-subscription`. That falls through to the provider's own CLI rather than being
/// reported as signed out.
fn probe_pi(provider: Provider) -> Option<std::result::Result<String, String>> {
    let name = crate::pi::provider_name(provider);
    let mut command = Command::new("pi");
    // This subcommand uses Pi's built-in auth runtime, not session extensions. Its
    // separate parser does not accept the adapter's --mcp-config flag.
    command.args([
        "auth",
        "check",
        "--provider",
        name,
        "--json",
        "--no-refresh",
    ]);
    let output = match output_with_timeout(command) {
        Ok(output) => output,
        Err(crate::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return None
        }
        Err(_) => return Some(Err(format!("Pi authentication check failed for {name}"))),
    };
    let Ok(answer) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Some(Err("Pi returned an unreadable auth status".into()));
    };
    if answer["provider"] != name {
        return Some(Err(
            "Pi returned auth status for a different provider".into()
        ));
    }
    if answer["status"] == "not_ready" && answer["reason"] == "provider_not_found" {
        return None;
    }
    let kind = answer["authType"].as_str();
    let subscription = kind == Some("oauth")
        // The Claude extension may wrap its CLI with an API-key-shaped credential.
        // probe_claude still requires that CLI's first-party subscription evidence.
        || (provider == Provider::Claude && kind == Some("api_key"));
    if output.status.success() && answer["status"] == "ready" && subscription {
        Some(Ok(format!(
            "Pi has {name} subscription authentication ready"
        )))
    } else {
        Some(Err(format!(
            "Pi cannot confirm subscription authentication for {name}"
        )))
    }
}

fn probe_claude() -> std::result::Result<String, String> {
    if let Some(answer) = probe_pi(Provider::Claude) {
        answer?;
    }
    let mut command = Command::new("claude");
    command.args(["auth", "status", "--json"]);
    let output = output_with_timeout(command)
        .map_err(|_| "Could not check Claude Code authentication".to_string())?;
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Claude Code returned an unreadable auth status".to_string())?;
    if !output.status.success() || status["loggedIn"] != true {
        return Err("Claude Code is not signed in".into());
    }
    if status["apiProvider"] != "firstParty"
        || !matches!(
            status["authMethod"].as_str(),
            Some("claude.ai" | "oauth_token")
        )
    {
        return Err(
            "Claude Code did not report a first-party subscription login; run `claude auth login`"
                .into(),
        );
    }
    Ok("Claude subscription authenticated through Claude Code".into())
}

fn probe_chatgpt() -> std::result::Result<String, String> {
    if let Some(answer) = probe_pi(Provider::OpenAi) {
        return answer;
    }
    let mut codex = Command::new("codex");
    codex.args(["login", "status"]);
    let output = output_with_timeout(codex)
        .map_err(|_| "Could not check Codex authentication".to_string())?;
    // Codex writes login status to stderr. Accept stdout too, but never expose an
    // unexpected status body: an API-key login can print part of the credential.
    let status = [output.stdout, output.stderr].concat();
    if output.status.success()
        && String::from_utf8_lossy(&status).trim() == "Logged in using ChatGPT"
    {
        return Ok("ChatGPT subscription authenticated through Codex".into());
    }
    Err("Codex did not report a ChatGPT login; run `codex login` to use your subscription".into())
}

fn output_with_timeout(mut command: Command) -> crate::Result<crate::command::Output> {
    command.env("LC_ALL", "C");
    let output = crate::command::run_blocking(command, Duration::from_secs(3), 64_000)?;
    if output.truncated {
        return Err(crate::Error::invalid(
            "authentication status exceeded its capture limit",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_in_is_a_command_only_where_there_is_an_account_to_sign_in_to() {
        // Asserted here rather than through the route, because succeeding through the
        // route means starting a real OAuth flow and opening somebody's browser.
        assert_eq!(sign_in(Provider::Claude), Some("claude auth login"));
        assert_eq!(sign_in(Provider::OpenAi), Some("codex login"));

        // A GLM Coding Plan key is a value to paste and the gateway on loopback has no
        // account at all. A button offering either would run a command that does not
        // exist, and then report its failure as a sign-in problem.
        assert_eq!(sign_in(Provider::ZAi), None);
        assert_eq!(sign_in(Provider::Local), None);
    }

    #[test]
    fn every_sign_in_command_names_the_tool_the_provider_is_actually_reached_through() {
        // The wording this replaced told somebody to run `/login` in `eve dev`, which has
        // not been the runtime since D20 - so the instruction named a program they do not
        // have, and it stayed wrong because nothing checked it.
        for provider in Provider::ALL {
            let Some(command) = sign_in(*provider) else {
                continue;
            };
            let tool = command.split_whitespace().next().unwrap_or_default();
            assert!(
                !tool.contains("eve"),
                "{provider} still points at the old runtime: {command}"
            );
            assert!(
                matches!(tool, "claude" | "codex" | "pi"),
                "{provider} names {tool}, which is not a tool ai-team reaches a provider \
                 through - see D8's table"
            );
        }
    }
}
