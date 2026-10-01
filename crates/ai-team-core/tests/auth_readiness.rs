//! Isolated metadata probes, never live credentials or provider requests.
#![cfg(unix)]

use ai_team_core::{
    MachineProfile, ModelRegistry, Provider, ProviderState, ProviderStatus, DEFAULT_MACHINE_PROFILE,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = std::env::var_os("PATH").unwrap();
        for (key, _) in std::env::vars_os() {
            std::env::remove_var(key);
        }
        std::env::set_var(
            "PATH",
            format!("{}:{}", root.display(), path.to_string_lossy()),
        );
        for key in ["HOME", "XDG_CONFIG_HOME", "AI_TEAM_HOME", "AUTH_FIXTURE"] {
            std::env::set_var(key, &root);
        }
        for key in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "CLAUDE_CODE_USE_BEDROCK",
        ] {
            std::env::set_var(key, "DECOY");
        }
        for program in ["pi", "claude", "codex"] {
            let file = root.join(program);
            std::fs::write(&file, include_str!("fixtures/auth-probe.sh")).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { _dir: dir, root }
    }
    fn status(&self, provider: Provider, mode: &str) -> ProviderStatus {
        std::fs::write(self.root.join("mode"), mode).unwrap();
        let text = DEFAULT_MACHINE_PROFILE.replace(
            &format!("{provider} = false"),
            &format!("{provider} = true"),
        );
        ModelRegistry::new(MachineProfile::parse(&text).unwrap())
            .statuses()
            .into_iter()
            .find(|status| status.provider == provider)
            .unwrap()
    }
    fn subscription_evidence(&self) {
        for provider in [Provider::Claude, Provider::OpenAi] {
            assert_eq!(
                self.status(provider, "fallback").state,
                ProviderState::Allowed
            );
            assert!(
                !self.root.join("inherited").exists(),
                "metadata probes inherited metered credentials"
            );
            for mode in ["key", "unknown"] {
                let result = self.status(provider, mode);
                assert_eq!(
                    result.state,
                    ProviderState::Unreachable,
                    "{provider}/{mode}: {result:?}"
                );
                assert!(!result.detail.contains("SECRET_DECOY"));
            }
        }
        assert_eq!(
            self.status(Provider::Claude, "token").state,
            ProviderState::Allowed
        );
        assert_eq!(
            self.status(Provider::Claude, "third-party").state,
            ProviderState::Unreachable
        );
    }
    fn runtime_evidence(&self) {
        assert_eq!(
            self.status(Provider::OpenAi, "pi-ready").state,
            ProviderState::Allowed
        );
        for mode in ["pi-api-key", "pi-foreign", "pi-failed", "pi-unknown-type"] {
            assert_eq!(
                self.status(Provider::OpenAi, mode).state,
                ProviderState::Unreachable,
                "{mode}"
            );
        }
        assert!(
            std::fs::read_to_string(self.root.join("calls"))
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("pi "))
                .all(|line| line.contains("--no-refresh")),
            "readiness must not refresh credentials"
        );
    }
    fn bounded_children(&self) {
        let began = Instant::now();
        assert_eq!(
            self.status(Provider::OpenAi, "orphan").state,
            ProviderState::Allowed
        );
        assert!(
            began.elapsed() < Duration::from_secs(7),
            "an exited probe's descendant held stdout open"
        );
        assert_eq!(
            self.status(Provider::OpenAi, "loud").state,
            ProviderState::Unreachable,
            "truncated metadata cannot certify authentication"
        );
    }
}

// The sole test owns this process's environment; even API-key decoys reach only fixtures.
#[test]
fn readiness_requires_subscription_evidence_without_spending_or_leaking_credentials() {
    let fixture = Fixture::new();
    fixture.subscription_evidence();
    fixture.runtime_evidence();
    fixture.bounded_children();
}
