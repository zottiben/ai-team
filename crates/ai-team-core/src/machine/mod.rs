//! The provider registry and the policy of this particular machine.
//!
//! Team rows say what a seat prefers. They do not grant permission to use it: the
//! machine profile does that, and dispatch resolves the preference through it every
//! time. This is the security boundary for an unattended scheduled run (D8).

use crate::error::{Error, Result};
use crate::model::{Agent, Provider};

mod ailocal;
mod probe;
mod profile;

use ailocal::{non_empty_env, AilocalSettings};
use probe::{implemented, probe, provider_default};
pub use probe::{ProviderState, ProviderStatus};
pub use profile::{ensure_machine_profile, MachineProfile, DEFAULT_MACHINE_PROFILE};

const ZAI_KEY_ENV: &str = "AI_TEAM_ZAI_KEY";
const AILOCAL_KEY_ENV: &str = "AI_TEAM_AILOCAL_KEY";

/// What policy selected for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResolution {
    pub role: String,
    pub requested_provider: Provider,
    pub requested_model: String,
    pub provider: Provider,
    pub model: String,
    pub context_window: Option<i64>,
    /// Present only when the machine denied the requested provider.
    pub fallback_reason: Option<String>,
}

impl ModelResolution {
    pub fn fell_back(&self) -> bool {
        self.fallback_reason.is_some()
    }

    pub fn notice(&self) -> Option<String> {
        self.fallback_reason.as_ref().map(|reason| {
            format!(
                "{}: {}/{} -> {}/{} ({reason})",
                self.role, self.requested_provider, self.requested_model, self.provider, self.model
            )
        })
    }

    pub(crate) fn apply(&self, agent: &Agent) -> Agent {
        let mut effective = agent.clone();
        effective.provider = self.provider;
        effective.model.clone_from(&self.model);
        effective.context_window = self.context_window;
        effective
    }
}

/// Policy plus the machine-owned configuration needed to run the allowed providers.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    profile: MachineProfile,
    ailocal: AilocalSettings,
}

impl ModelRegistry {
    pub fn load() -> Result<Self> {
        Ok(Self::new(MachineProfile::load_default()?))
    }

    pub fn new(profile: MachineProfile) -> Self {
        ModelRegistry {
            profile,
            ailocal: AilocalSettings::load(),
        }
    }

    pub fn local_only() -> Self {
        Self::new(MachineProfile::local_only())
    }

    pub fn profile(&self) -> &MachineProfile {
        &self.profile
    }

    /// Resolve at the policy boundary. Reachability is deliberately not part of this
    /// choice: a transient outage must not silently send work to another account.
    pub fn resolve(&self, agent: &Agent) -> Result<ModelResolution> {
        if self.profile.allowed(agent.provider) {
            return Ok(ModelResolution {
                role: agent.role.clone(),
                requested_provider: agent.provider,
                requested_model: agent.model.clone(),
                provider: agent.provider,
                model: agent.model.clone(),
                context_window: agent.context_window,
                fallback_reason: None,
            });
        }

        let fallback = self
            .profile
            .fallback()
            .iter()
            .copied()
            .find(|provider| self.profile.allowed(*provider) && implemented(*provider))
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{} is denied for role {:?}, and this machine has no allowed, implemented fallback",
                    agent.provider, agent.role
                ))
            })?;
        let (model, context_window) = provider_default(fallback);
        Ok(ModelResolution {
            role: agent.role.clone(),
            requested_provider: agent.provider,
            requested_model: agent.model.clone(),
            provider: fallback,
            model: model.to_string(),
            context_window: Some(context_window),
            fallback_reason: Some(format!(
                "{} is denied by the machine profile",
                agent.provider
            )),
        })
    }

    pub fn resolve_agents(&self, agents: &[Agent]) -> Result<(Vec<Agent>, Vec<ModelResolution>)> {
        let resolutions: Vec<_> = agents
            .iter()
            .map(|agent| self.resolve(agent))
            .collect::<Result<_>>()?;
        let effective = agents
            .iter()
            .zip(&resolutions)
            .map(|(agent, resolution)| resolution.apply(agent))
            .collect();
        Ok((effective, resolutions))
    }

    pub fn ailocal_base_url(&self) -> &str {
        &self.ailocal.base_url
    }

    /// Values the supervisor may pass to the generated process. No generic API-key
    /// names are accepted here: each name corresponds to a provider in D8's table.
    pub fn provider_environment(&self, required: &[&'static str]) -> Result<Vec<(String, String)>> {
        required
            .iter()
            .filter(|key| key.ends_with("_KEY"))
            .map(|key| {
                let value = match *key {
                    AILOCAL_KEY_ENV => self.ailocal.key.clone().ok_or_else(|| {
                        Error::invalid(self.ailocal.issue.clone().unwrap_or_else(|| {
                            "ailocal has no gateway key; start or configure ailocal first".into()
                        }))
                    })?,
                    ZAI_KEY_ENV => non_empty_env(ZAI_KEY_ENV).ok_or_else(|| {
                        Error::invalid(
                            "z.ai is selected but AI_TEAM_ZAI_KEY is not set to a GLM Coding Plan key",
                        )
                    })?,
                    other => {
                        return Err(Error::invalid(format!(
                            "generated project requested unregistered credential {other}"
                        )))
                    }
                };
                Ok(((*key).to_string(), value))
            })
            .collect()
    }

    pub fn statuses(&self) -> Vec<ProviderStatus> {
        Provider::ALL
            .iter()
            .copied()
            .map(|provider| {
                if self.profile.allowed(provider) {
                    probe(provider, &self.ailocal)
                } else {
                    ProviderStatus {
                        provider,
                        state: ProviderState::Denied,
                        detail: "blocked by machine.toml".into(),
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Reasoning;
    use crate::roles::preset;

    fn agent(provider: Provider, model: &str) -> Agent {
        let new = preset("backend").unwrap().to_new_agent(0);
        Agent {
            id: 1,
            team_id: 1,
            ord: 0,
            role: new.role,
            name: new.name,
            purpose: new.purpose,
            provider,
            model: model.into(),
            reasoning: Reasoning::Medium,
            zone: new.zone,
            prompt_preset: new.prompt_preset,
            prompt_md: None,
            context_window: Some(123_456),
            read_only: false,
            enabled: true,
            rev: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn profile(providers: &str, fallback: &str) -> MachineProfile {
        MachineProfile::parse(&format!(
            "version = 1\nfallback = [{fallback}]\n[providers]\n{providers}"
        ))
        .unwrap()
    }

    #[test]
    fn default_is_fail_closed_except_for_loopback() {
        let profile = MachineProfile::local_only();
        assert!(profile.allowed(Provider::Local));
        for provider in [Provider::Claude, Provider::OpenAi, Provider::ZAi] {
            assert!(!profile.allowed(provider));
        }
    }

    #[test]
    fn every_provider_and_a_total_fallback_order_are_required() {
        let missing = "version = 1\nfallback = [\"local\"]\n[providers]\nlocal = true\n";
        assert!(MachineProfile::parse(missing).is_err());

        let duplicate = "version = 1\nfallback = [\"local\", \"local\", \"zai\", \"openai\"]\n\
                         [providers]\nclaude=false\nopenai=false\nzai=false\nlocal=true\n";
        assert!(MachineProfile::parse(duplicate).is_err());
    }

    #[test]
    fn an_allowed_preference_is_preserved_exactly() {
        let registry = ModelRegistry::new(profile(
            "claude=false\nopenai=false\nzai=true\nlocal=true",
            "\"claude\", \"openai\", \"zai\", \"local\"",
        ));
        let selected = registry
            .resolve(&agent(Provider::ZAi, "glm-custom"))
            .unwrap();
        assert_eq!(selected.provider, Provider::ZAi);
        assert_eq!(selected.model, "glm-custom");
        assert_eq!(selected.context_window, Some(123_456));
        assert!(!selected.fell_back());
    }

    #[test]
    fn a_denied_provider_falls_back_in_the_profiles_order() {
        let registry = ModelRegistry::new(profile(
            "claude=false\nopenai=true\nzai=false\nlocal=true",
            "\"claude\", \"openai\", \"zai\", \"local\"",
        ));
        let selected = registry.resolve(&agent(Provider::ZAi, "glm-4.6")).unwrap();
        assert_eq!(selected.provider, Provider::OpenAi);
        assert_eq!(selected.model, "gpt-5.6-luna-fast");
        assert_eq!(selected.context_window, Some(32_768));
        assert_eq!(
            selected.notice().as_deref(),
            Some(
                "backend: zai/glm-4.6 -> openai/gpt-5.6-luna-fast (zai is denied by the machine profile)"
            )
        );
    }

    #[test]
    fn claude_is_not_used_as_a_fallback_before_its_bridge_exists() {
        let registry = ModelRegistry::new(profile(
            "claude=true\nopenai=false\nzai=false\nlocal=true",
            "\"claude\", \"openai\", \"zai\", \"local\"",
        ));
        let selected = registry.resolve(&agent(Provider::ZAi, "glm-4.6")).unwrap();
        assert_eq!(selected.provider, Provider::Local);
    }

    #[test]
    fn profile_unknown_fields_are_refused() {
        let source = "version=1\nfallback=[\"claude\",\"openai\",\"zai\",\"local\"]\n\
                      surprise=true\n[providers]\nclaude=false\nopenai=false\nzai=false\nlocal=true\n";
        assert!(MachineProfile::parse(source).is_err());
    }
}
