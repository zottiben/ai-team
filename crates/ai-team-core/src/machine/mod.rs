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
pub(crate) use probe::provider_default;
use probe::{implemented, probe};
pub use probe::{ProviderState, ProviderStatus};
mod edit;

pub use edit::{set_context, set_fallback, set_provider};
pub(crate) use profile::ensure_machine_profile_at;
pub use profile::{ensure_machine_profile, ContextSource, MachineProfile, DEFAULT_MACHINE_PROFILE};

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
    /// The model a provider defaults to.
    ///
    /// Exposed so a surface changing a seat's provider does not have to know the names -
    /// and so it cannot leave the old provider's model behind, which fails at the model
    /// call rather than where the change was made.
    pub fn default_model(provider: Provider) -> &'static str {
        provider_default(provider).0
    }

    /// The provider a brand-new team should prefer, and its default model.
    ///
    /// A creation-time choice, not a dispatch-time one - which is the distinction D13
    /// rests on. Seeding every team with `local` meant a machine that had just allowed
    /// Claude in the window still had six seats pointing at a gateway it does not run, and
    /// nothing said so until a run failed. Picking the first provider in the ranking that
    /// is both allowed and actually answering is the default being sensible; it is written
    /// into the roster, visible in `ait agents ls`, and editable.
    ///
    /// Falls back to `local` when nothing answers, because a team has to be seeded with
    /// something and that is the one provider that needs no account.
    pub fn preferred_seat(&self) -> (Provider, String) {
        self.statuses()
            .into_iter()
            .filter(|status| status.state == ProviderState::Allowed)
            .filter(|status| implemented(status.provider))
            // The ranking decides, not the order they were probed in.
            .min_by_key(|status| {
                self.profile
                    .fallback()
                    .iter()
                    .position(|ranked| *ranked == status.provider)
                    .unwrap_or(usize::MAX)
            })
            .map_or_else(
                || {
                    (
                        Provider::Local,
                        provider_default(Provider::Local).0.to_string(),
                    )
                },
                |status| {
                    (
                        status.provider,
                        provider_default(status.provider).0.to_string(),
                    )
                },
            )
    }

    pub fn resolve(&self, agent: &Agent) -> Result<ModelResolution> {
        if self.profile.allowed(agent.provider) {
            return Ok(ModelResolution {
                role: agent.role.clone(),
                requested_provider: agent.provider,
                requested_model: agent.model.clone(),
                provider: agent.provider,
                model: agent.model.clone(),
                context_window: Some(
                    agent
                        .context_window
                        .unwrap_or_else(|| provider_default(agent.provider).1),
                ),
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

    /// Which context sources this machine allows (D9).
    pub fn context_sources(&self) -> Vec<ContextSource> {
        ContextSource::ALL
            .iter()
            .copied()
            .filter(|source| self.profile.context_allowed(*source))
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
    /// A registry over a profile the test wrote, so "what this machine allows" is
    /// describable without touching the environment.
    fn registry_allowing(allowed: &[Provider], order: &[Provider]) -> ModelRegistry {
        let names: Vec<String> = order
            .iter()
            .map(|provider| format!("\"{}\"", provider.as_str()))
            .collect();
        let mut source = format!(
            "version = 1\nfallback = [{}]\n\n[providers]\n",
            names.join(", ")
        );
        for provider in Provider::ALL {
            use std::fmt::Write as _;
            let _ = writeln!(
                source,
                "{} = {}",
                provider.as_str(),
                allowed.contains(provider)
            );
        }
        ModelRegistry::new(MachineProfile::parse(&source).unwrap())
    }

    #[test]
    fn a_new_team_is_seeded_on_a_provider_the_machine_can_reach() {
        // The confusion this removes: allowing Claude in the window left six seats
        // pointing at a local gateway the machine does not run, and nothing said so until
        // a run failed. This is a creation-time choice, so D13's "never reroute on a
        // health failure" is untouched - it is written into the roster and editable.
        //
        // Only `local` needs no account, so it is the one this can assert on a machine
        // with no credentials at all.
        let local_only = registry_allowing(&[Provider::Local], Provider::ALL);
        let (provider, model) = local_only.preferred_seat();
        assert_eq!(provider, Provider::Local);
        assert_eq!(model, "auto");
    }

    #[test]
    fn nothing_allowed_still_seeds_a_team() {
        // A team has to be seeded with something, and local is the only provider that
        // needs no account - so it is the floor rather than an error.
        let nothing = registry_allowing(&[], Provider::ALL);
        assert_eq!(nothing.preferred_seat().0, Provider::Local);
    }

    #[test]
    fn the_fallback_ranking_decides_which_provider_a_team_prefers() {
        // Not the order they happened to be probed in. With the ranking reversed, the same
        // set of allowed providers has to produce a different preference.
        let reversed = registry_allowing(
            &[Provider::Local],
            &[
                Provider::Local,
                Provider::ZAi,
                Provider::OpenAi,
                Provider::Claude,
            ],
        );
        assert_eq!(reversed.preferred_seat().0, Provider::Local);
    }
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
    fn an_allowed_claude_preference_gets_the_registry_context_default() {
        let registry = ModelRegistry::new(profile(
            "claude=true\nopenai=false\nzai=false\nlocal=true",
            "\"claude\", \"openai\", \"zai\", \"local\"",
        ));
        let mut requested = agent(Provider::Claude, "sonnet");
        requested.context_window = None;
        let selected = registry.resolve(&requested).unwrap();
        assert_eq!(selected.context_window, Some(200_000));
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
    fn claude_is_available_as_a_fallback_now_that_its_bridge_exists() {
        let registry = ModelRegistry::new(profile(
            "claude=true\nopenai=false\nzai=false\nlocal=true",
            "\"claude\", \"openai\", \"zai\", \"local\"",
        ));
        let selected = registry.resolve(&agent(Provider::ZAi, "glm-4.6")).unwrap();
        assert_eq!(selected.provider, Provider::Claude);
        assert_eq!(selected.model, "sonnet");
    }

    #[test]
    fn profile_unknown_fields_are_refused() {
        let source = "version=1\nfallback=[\"claude\",\"openai\",\"zai\",\"local\"]\n\
                      surprise=true\n[providers]\nclaude=false\nopenai=false\nzai=false\nlocal=true\n";
        assert!(MachineProfile::parse(source).is_err());
    }
}
