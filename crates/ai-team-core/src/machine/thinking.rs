//! Exact selected-model thinking capabilities, without a model request.

use std::{path::Path, process::Command, time::Duration};

use serde::{Deserialize, Serialize};

use super::ModelRegistry;
use crate::{Error, Provider, Reasoning, Result};

const EXTENSION: &str = include_str!("thinking.ts");
const UNAVAILABLE: &str = "could not confirm this model's thinking levels with Pi; check that the exact model is available and Pi supports model-specific thinking capabilities";

#[derive(Debug, Serialize)]
pub struct ThinkingCapabilities {
    pub provider: Provider,
    pub model: String,
    pub levels: Vec<Reasoning>,
    /// Policy fallback is disclosed, never confused with the configured model.
    pub fallback_reason: Option<String>,
}

impl ModelRegistry {
    /// Pi loads the same provider extensions as its catalogue. This short-lived RPC
    /// process receives no prompt or command: our extension reports only the selected
    /// model's levels at startup, then closed stdin shuts it down. In particular, a
    /// missing extension cannot make a slash-command fall through to a paid prompt.
    pub fn thinking_levels(&self, provider: Provider, model: &str) -> Result<ThinkingCapabilities> {
        self.thinking_with(Path::new("pi"), provider, model)
    }

    fn thinking_with(
        &self,
        program: &Path,
        provider: Provider,
        model: &str,
    ) -> Result<ThinkingCapabilities> {
        if model.is_empty()
            || model.len() > 512
            || model.trim() != model
            || model.chars().any(char::is_control)
        {
            return Err(Error::invalid("choose an exact model id"));
        }
        let resolution = self.resolve_choice("assistant", provider, model, None)?;
        Ok(ThinkingCapabilities {
            levels: probe(program, resolution.provider, &resolution.model)?,
            provider: resolution.provider,
            model: resolution.model,
            fallback_reason: resolution.fallback_reason,
        })
    }
}

fn probe(program: &Path, provider: Provider, model: &str) -> Result<Vec<Reasoning>> {
    let dir = tempfile::tempdir()?;
    let extension = dir.path().join("thinking.ts");
    let config = dir.path().join("mcp.json");
    std::fs::write(&extension, EXTENSION)?;
    std::fs::write(&config, br#"{"mcpServers":{}}"#)?;
    let nonce = crate::util::mint_token();
    let runtime = crate::pi_provider(provider);
    let mut command = Command::new(program);
    command
        .args([
            "--offline",
            "--mode",
            "rpc",
            "--no-session",
            "--no-tools",
            "--no-mcp",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--provider",
            runtime,
            "--model",
            model,
            "--extension",
        ])
        .arg(extension)
        .arg("--mcp-config")
        .arg(config)
        .env("PI_MCP_CONFIG_MODE", "exclusive")
        .env("AI_TEAM_THINKING_PROBE", &nonce);
    // Bounded, concurrently drained pipes and whole-group cleanup on every outcome.
    // Never return Pi's raw diagnostics: extension output can contain credentials.
    let output = crate::command::run_blocking(command, Duration::from_secs(20), 32 * 1024)
        .map_err(|_| Error::invalid(UNAVAILABLE))?;
    if !output.status.success() || output.truncated {
        return Err(Error::invalid(UNAVAILABLE));
    }
    parse(&output.stdout, &nonce, runtime, model)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    r#type: String,
    nonce: String,
    provider: String,
    model: String,
    levels: Vec<String>,
}

fn parse(bytes: &[u8], nonce: &str, provider: &str, model: &str) -> Result<Vec<Reasoning>> {
    let reports: Vec<Report> = bytes
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .filter(|report: &Report| report.r#type == "ai_team_thinking" && report.nonce == nonce)
        .collect();
    let [report] = reports.as_slice() else {
        return Err(Error::invalid(UNAVAILABLE));
    };
    if report.provider != provider || report.model != model || report.levels.is_empty() {
        return Err(Error::invalid(UNAVAILABLE));
    }
    let mut levels = Vec::new();
    for native in &report.levels {
        let level: Reasoning = native.parse().map_err(|_| Error::invalid(UNAVAILABLE))?;
        // Aliases belong to human input, not a capability receipt from Pi.
        if crate::pi::thinking(level) != native || levels.contains(&level) {
            return Err(Error::invalid(UNAVAILABLE));
        }
        levels.push(level);
    }
    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(provider: &str, model: &str, levels: &[&str]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "type":"ai_team_thinking", "nonce":"exact", "provider":provider,
            "model":model, "levels":levels,
        }))
        .unwrap()
    }

    #[test]
    fn exact_native_capabilities_are_not_guessed_from_a_provider_or_boolean() {
        let all = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
        assert_eq!(
            parse(
                &report("provider", "model", &all),
                "exact",
                "provider",
                "model"
            )
            .unwrap(),
            Reasoning::ALL
        );
        assert_eq!(
            parse(
                &report("provider", "model", &["low", "max"]),
                "exact",
                "provider",
                "model"
            )
            .unwrap(),
            [Reasoning::Low, Reasoning::Max]
        );
        assert_eq!(
            parse(
                &report("provider", "model", &["off"]),
                "exact",
                "provider",
                "model"
            )
            .unwrap(),
            [Reasoning::None]
        );
        for levels in [
            vec![],
            vec!["min"],
            vec!["none"],
            vec!["high", "high"],
            vec!["unknown"],
        ] {
            assert!(parse(
                &report("provider", "model", &levels),
                "exact",
                "provider",
                "model"
            )
            .is_err());
        }
        let bytes = report("provider", "model", &all);
        for (nonce, provider, model) in [
            ("other", "provider", "model"),
            ("exact", "other", "model"),
            ("exact", "provider", "other"),
        ] {
            assert!(parse(&bytes, nonce, provider, model).is_err());
        }
        let duplicate = [bytes.clone(), b"\n".to_vec(), bytes].concat();
        assert!(parse(&duplicate, "exact", "provider", "model").is_err());
        assert!(!parse(b"private-token", "exact", "provider", "model")
            .unwrap_err()
            .to_string()
            .contains("private-token"));
    }

    #[test]
    fn capability_discovery_refuses_denied_providers_and_invalid_names_without_pi() {
        let registry = ModelRegistry::local_only();
        let denied = ModelRegistry::new(
            crate::MachineProfile::parse(crate::DEFAULT_MACHINE_PROFILE).unwrap(),
        );
        assert!(denied
            .thinking_levels(Provider::Claude, "model")
            .unwrap_err()
            .to_string()
            .contains("denied"));
        for model in ["", " leading", "model\n", &"x".repeat(513)] {
            assert!(registry
                .thinking_levels(Provider::Local, model)
                .unwrap_err()
                .to_string()
                .contains("exact model"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_metadata_process_has_no_prompt_or_mcp_and_does_not_leak_diagnostics() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("pi");
        std::fs::write(&program, r"#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
assert args[:3] == ['--offline', '--mode', 'rpc']
for flag in ['--no-session', '--no-tools', '--no-mcp', '--no-skills', '--no-prompt-templates', '--no-context-files']:
    assert flag in args
assert '--thinking' not in args and '--print' not in args
assert sys.stdin.read() == ''
assert os.environ['PI_MCP_CONFIG_MODE'] == 'exclusive'
assert json.load(open(args[args.index('--mcp-config') + 1])) == {'mcpServers': {}}
assert 'getSupportedThinkingLevels(model)' in open(args[args.index('--extension') + 1]).read()
assert 'ANTHROPIC_API_KEY' not in os.environ
model = args[args.index('--model') + 1]
if model == 'failed':
    print('private-token', file=sys.stderr)
    sys.exit(1)
if model == 'oversized':
    print('x' * 40000)
print(json.dumps({'type':'ai_team_thinking','nonce':os.environ['AI_TEAM_THINKING_PROBE'], 'provider':'llama.cpp','model':model,'levels':['minimal','xhigh','max']}))
").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            probe(&program, Provider::Local, "fixture").unwrap(),
            [Reasoning::Minimal, Reasoning::XHigh, Reasoning::Max]
        );
        let fallback = ModelRegistry::local_only()
            .thinking_with(&program, Provider::OpenAi, "preferred-model")
            .unwrap();
        assert_eq!(fallback.provider, Provider::Local);
        assert_eq!(
            fallback.model,
            ModelRegistry::default_model(Provider::Local)
        );
        assert!(fallback
            .fallback_reason
            .unwrap()
            .contains("openai is denied"));
        assert_eq!(
            fallback.levels,
            [Reasoning::Minimal, Reasoning::XHigh, Reasoning::Max]
        );
        for model in ["failed", "oversized"] {
            assert_eq!(
                probe(&program, Provider::Local, model)
                    .unwrap_err()
                    .to_string(),
                Error::invalid(UNAVAILABLE).to_string()
            );
        }
    }
}
