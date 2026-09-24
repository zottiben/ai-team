//! The concrete models Pi can run on this machine.
//!
//! Pi owns the model catalogue (D20), including subscription extensions and custom local
//! providers. Copying those IDs into ai-team is how a picker ends up saying `sonnet`
//! while the runtime calls it `claude-opus-5`, so this reads Pi's own list instead.

use std::collections::HashSet;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::Provider;

use super::{ModelRegistry, ProviderState, ProviderStatus};

/// One exact model choice as Pi names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelChoice {
    /// ai-team's policy/provider enum (`claude`, `openai`, `zai`, `local`).
    pub provider: Provider,
    /// Pi's provider id, which makes the subscription route explicit in the page.
    pub runtime_provider: String,
    /// The exact id passed to `pi --model`.
    pub model: String,
    pub context: String,
    /// Same catalogue value, parsed once for progress math rather than guessed in UI.
    pub context_tokens: i64,
    pub max_output: String,
    pub thinking: bool,
    pub images: bool,
}

/// The deterministic model a role gets when a team is created or explicitly reset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleModelDefault {
    pub role: String,
    pub provider: Provider,
    pub model: String,
}

/// A seat that cannot think on this machine.
///
/// Judged by Pi's own catalogue, because that is what a turn fails on: a model Pi does not
/// list is a turn that dies before any model is reached - `Unknown provider "llama.cpp"`,
/// two seconds into a run. A provider whose own check passes is no help when Pi has no
/// model for it, and one whose check fails only gives a better reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stranded {
    pub agent_id: i64,
    pub role: String,
    /// What dispatch would run it as: its own, or the fallback for a denied provider.
    pub provider: Provider,
    pub model: String,
    /// Why, in a line.
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedModel {
    provider: String,
    model: String,
    context: String,
    max_output: String,
    thinking: bool,
    images: bool,
}

impl ModelRegistry {
    /// Resolve a fresh exact model after one or more subscription accounts reported a
    /// known quota boundary. Reachability is re-probed here so a disabled local gateway
    /// is not selected merely because it appears later in the configured ranking.
    pub(crate) fn quota_fallback(
        &self,
        agent: &crate::model::Agent,
        exhausted: &[Provider],
        reason: &str,
    ) -> Result<Option<super::ModelResolution>> {
        let reachable: Vec<_> = self
            .statuses()
            .into_iter()
            .filter(|status| {
                status.state == ProviderState::Allowed && !exhausted.contains(&status.provider)
            })
            .map(|status| status.provider)
            .collect();
        let choices = self.models()?;
        let Some((provider, model, context_tokens)) =
            quota_fallback_choice(&agent.role, &choices, &reachable, self.profile().fallback())
        else {
            return Ok(None);
        };
        Ok(Some(super::ModelResolution {
            role: agent.role.clone(),
            requested_provider: agent.provider,
            requested_model: agent.model.clone(),
            provider,
            model,
            context_window: (context_tokens > 0).then_some(context_tokens),
            fallback_reason: Some(reason.to_string()),
        }))
    }

    /// The switched-on seats among `seats` that cannot run here, given Pi's catalogue.
    ///
    /// `status` answers for a provider when its own check has something to say; it is asked
    /// only about a seat already found stranded, so a caller can probe lazily.
    pub fn stranded(
        &self,
        seats: &[crate::model::Agent],
        catalog: &[ModelChoice],
        status: impl Fn(Provider) -> Option<ProviderStatus>,
    ) -> Vec<Stranded> {
        seats
            .iter()
            .filter(|seat| seat.enabled)
            .filter_map(|seat| {
                let (provider, model, why) = match self.resolve(seat) {
                    Ok(resolved) => (resolved.provider, resolved.model, None),
                    // Denied, with nothing allowed to fall back to.
                    Err(error) => (seat.provider, seat.model.clone(), Some(error.to_string())),
                };
                if why.is_none()
                    && catalog
                        .iter()
                        .any(|choice| choice.provider == provider && choice.model == model)
                {
                    return None;
                }
                let why = why.unwrap_or_else(|| {
                    status(provider)
                        .filter(|status| status.state == ProviderState::Unreachable)
                        .map_or_else(
                            || {
                                format!(
                                    "Pi has no {}/{model} on this machine",
                                    crate::pi_provider(provider)
                                )
                            },
                            |status| status.detail,
                        )
                });
                Some(Stranded {
                    agent_id: seat.id,
                    role: seat.role.clone(),
                    provider,
                    model,
                    why,
                })
            })
            .collect()
    }

    /// Every exact model Pi reports available, narrowed by this machine's policy.
    pub fn models(&self) -> Result<Vec<ModelChoice>> {
        let output = Command::new("pi")
            // Listing a local catalogue does not need update checks, and a settings page
            // must not wait on the network merely to open.
            .args(["--offline", "--list-models"])
            .stdin(Stdio::null())
            .output()
            .map_err(|error| Error::invalid(format!("could not ask Pi for its models: {error}")))?;
        if !output.status.success() {
            return Err(Error::invalid(format!(
                "`pi --list-models` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let listed = parse(&String::from_utf8_lossy(&output.stdout))?;
        let mut choices = Vec::new();
        for provider in Provider::ALL {
            if !self.profile().allowed(*provider) {
                continue;
            }
            let runtime = crate::pi_provider(*provider);
            choices.extend(
                listed
                    .iter()
                    .filter(|model| model.provider == runtime)
                    .map(|model| {
                        Ok(ModelChoice {
                            provider: *provider,
                            runtime_provider: runtime.to_string(),
                            model: model.model.clone(),
                            context: model.context.clone(),
                            context_tokens: capacity(&model.context)?,
                            max_output: model.max_output.clone(),
                            thinking: model.thinking,
                            images: model.images,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(choices)
    }

    /// Concrete, role-appropriate defaults for a new team.
    ///
    /// This is deliberately not dispatch fallback. The choices are written visibly into
    /// the roster, and an existing team changes only through an explicit reset. Exact ids
    /// come from Pi, so an explicit reset fails rather than silently replacing them with
    /// aliases when Pi's catalogue cannot be read. Project creation may choose its own
    /// conservative fallback at the call site.
    pub fn role_defaults(&self) -> Result<Vec<RoleModelDefault>> {
        Ok(self.role_defaults_from(&self.statuses(), &self.models()?))
    }

    /// The same defaults from statuses and a catalogue a caller has already read, so a
    /// report that needs both does not probe every provider twice.
    pub fn role_defaults_from(
        &self,
        statuses: &[ProviderStatus],
        catalog: &[ModelChoice],
    ) -> Vec<RoleModelDefault> {
        let fallback_provider = statuses
            .iter()
            .filter(|status| status.state == ProviderState::Allowed)
            .min_by_key(|status| {
                self.profile()
                    .fallback()
                    .iter()
                    .position(|ranked| *ranked == status.provider)
                    .unwrap_or(usize::MAX)
            })
            .map_or(Provider::Local, |status| status.provider);
        let fallback = (
            fallback_provider,
            ModelRegistry::default_model(fallback_provider).to_string(),
        );
        let reachable: HashSet<_> = statuses
            .iter()
            .filter(|status| status.state == ProviderState::Allowed)
            .map(|status| status.provider)
            .collect();
        let choices: Vec<_> = catalog
            .iter()
            .filter(|choice| reachable.contains(&choice.provider))
            .cloned()
            .collect();

        crate::DEFAULT_ROSTER
            .iter()
            .map(|preset| {
                let (provider, model) = role_choice(preset.role, &choices, &fallback);
                RoleModelDefault {
                    role: preset.role.to_string(),
                    provider,
                    model,
                }
            })
            .collect()
    }
}

/// Ranked by the nature of the job, never by whatever ordering Pi happened to print.
/// Each role has a different first choice so the roster communicates an actual team rather
/// than six aliases for one generic seat.
fn quota_fallback_choice(
    role: &str,
    choices: &[ModelChoice],
    reachable: &[Provider],
    fallback_order: &[Provider],
) -> Option<(Provider, String, i64)> {
    let provider = fallback_order.iter().copied().find(|provider| {
        reachable.contains(provider) && choices.iter().any(|choice| choice.provider == *provider)
    })?;
    let provider_choices: Vec<_> = choices
        .iter()
        .filter(|choice| choice.provider == provider)
        .cloned()
        .collect();
    let fallback = (provider, ModelRegistry::default_model(provider).to_string());
    let (provider, model) = role_choice(role, &provider_choices, &fallback);
    let context_tokens = provider_choices
        .iter()
        .find(|choice| choice.provider == provider && choice.model == model)
        .map_or(0, |choice| choice.context_tokens);
    Some((provider, model, context_tokens))
}

fn role_choice(
    role: &str,
    choices: &[ModelChoice],
    fallback: &(Provider, String),
) -> (Provider, String) {
    let ranked: &[(Provider, &str)] = match role {
        "orchestrator" => &[
            (Provider::OpenAi, "gpt-6-astra"),
            (Provider::Claude, "claude-opus-5"),
        ],
        "planner" | "backend" => &[
            (Provider::Claude, "claude-opus-5"),
            (Provider::OpenAi, "gpt-6-astra"),
        ],
        "frontend" => &[
            (Provider::Claude, "claude-sonnet-5"),
            (Provider::OpenAi, "gpt-5.6-terra"),
        ],
        "verifier" => &[
            (Provider::OpenAi, "gpt-5.6-luna"),
            (Provider::Claude, "claude-sonnet-5"),
        ],
        "reviewer" => &[
            (Provider::OpenAi, "gpt-5.6-sol"),
            (Provider::Claude, "claude-opus-5"),
        ],
        _ => &[],
    };

    for (provider, model) in ranked {
        if choices
            .iter()
            .any(|choice| choice.provider == *provider && choice.model == *model)
        {
            return (*provider, (*model).to_string());
        }
    }
    choices
        .iter()
        .find(|choice| choice.provider == fallback.0)
        .or_else(|| choices.first())
        .map_or_else(
            || fallback.clone(),
            |choice| (choice.provider, choice.model.clone()),
        )
}

fn parse(source: &str) -> Result<Vec<ListedModel>> {
    let mut lines = source.lines();
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    let columns: Vec<_> = header.split_whitespace().collect();
    if columns
        != [
            "provider", "model", "context", "max-out", "thinking", "images",
        ]
    {
        return Err(Error::invalid(format!(
            "Pi returned an unfamiliar model list header: {header}"
        )));
    }

    lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 6 {
                return Err(Error::invalid(format!(
                    "Pi returned an unreadable model row: {line}"
                )));
            }
            Ok(ListedModel {
                provider: fields[0].to_string(),
                model: fields[1].to_string(),
                context: fields[2].to_string(),
                max_output: fields[3].to_string(),
                thinking: yes_no(fields[4], line)?,
                images: yes_no(fields[5], line)?,
            })
        })
        .collect()
}

fn capacity(value: &str) -> Result<i64> {
    let (number, scale) = match value.chars().last() {
        Some('K' | 'k') => (&value[..value.len() - 1], 1_000_i64),
        Some('M' | 'm') => (&value[..value.len() - 1], 1_000_000_i64),
        Some(_) => (value, 1_i64),
        None => return Err(Error::invalid("Pi returned an empty context window")),
    };
    number
        .parse::<i64>()
        .ok()
        .and_then(|number| number.checked_mul(scale))
        .ok_or_else(|| Error::invalid(format!("Pi returned an unreadable context window: {value}")))
}

fn yes_no(value: &str, line: &str) -> Result<bool> {
    match value {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(Error::invalid(format!(
            "Pi returned an unreadable model capability in: {line}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_pi_prints_keeps_exact_ids_and_capabilities() {
        let found = parse(
            "provider             model              context  max-out  thinking  images\n\
             claude-subscription  claude-opus-5      1M       128K     yes       yes\n\
             openai-codex         gpt-5.6-luna       272K     128K     yes       yes\n\
             openai-codex         gpt-5.3-spark      128K     128K     yes       no\n",
        )
        .unwrap();

        assert_eq!(found[0].model, "claude-opus-5");
        assert_eq!(found[0].provider, "claude-subscription");
        assert_eq!(found[0].context, "1M");
        assert_eq!(capacity(&found[0].context).unwrap(), 1_000_000);
        assert_eq!(capacity("272K").unwrap(), 272_000);
        assert!(found[0].thinking);
        assert!(found[0].images);
        assert!(!found[2].images);
    }

    fn choice(provider: Provider, model: &str) -> ModelChoice {
        ModelChoice {
            provider,
            runtime_provider: crate::pi_provider(provider).to_string(),
            model: model.to_string(),
            context: "1M".to_string(),
            context_tokens: 1_000_000,
            max_output: "128K".to_string(),
            thinking: true,
            images: true,
        }
    }

    #[test]
    fn roles_choose_distinct_exact_models_instead_of_one_blanket_default() {
        let choices = vec![
            choice(Provider::Claude, "claude-opus-5"),
            choice(Provider::Claude, "claude-sonnet-5"),
            choice(Provider::OpenAi, "gpt-6-astra"),
            choice(Provider::OpenAi, "gpt-5.6-luna"),
            choice(Provider::OpenAi, "gpt-5.6-sol"),
        ];
        let fallback = (Provider::Claude, "sonnet".to_string());

        assert_eq!(
            role_choice("orchestrator", &choices, &fallback),
            (Provider::OpenAi, "gpt-6-astra".to_string())
        );
        assert_eq!(
            role_choice("planner", &choices, &fallback),
            (Provider::Claude, "claude-opus-5".to_string())
        );
        assert_eq!(
            role_choice("backend", &choices, &fallback),
            (Provider::Claude, "claude-opus-5".to_string())
        );
        assert_eq!(
            role_choice("frontend", &choices, &fallback),
            (Provider::Claude, "claude-sonnet-5".to_string())
        );
        assert_eq!(
            role_choice("verifier", &choices, &fallback),
            (Provider::OpenAi, "gpt-5.6-luna".to_string())
        );
        assert_eq!(
            role_choice("reviewer", &choices, &fallback),
            (Provider::OpenAi, "gpt-5.6-sol".to_string())
        );
    }

    #[test]
    fn quota_fallback_keeps_the_roles_exact_profile_on_the_next_provider() {
        let choices = vec![
            choice(Provider::Claude, "claude-sonnet-5"),
            choice(Provider::OpenAi, "gpt-5.6-terra"),
            choice(Provider::OpenAi, "gpt-6-astra"),
        ];

        assert_eq!(
            quota_fallback_choice(
                "frontend",
                &choices,
                &[Provider::OpenAi],
                &[Provider::Claude, Provider::OpenAi, Provider::Local],
            ),
            Some((Provider::OpenAi, "gpt-5.6-terra".into(), 1_000_000))
        );
        assert_eq!(
            quota_fallback_choice(
                "frontend",
                &choices,
                &[],
                &[Provider::Claude, Provider::OpenAi],
            ),
            None
        );
    }

    #[test]
    fn role_defaults_stay_on_the_reachable_fallback_when_preferred_models_are_absent() {
        let choices = vec![choice(Provider::Local, "Qwen3-Coder-Next")];
        assert_eq!(
            role_choice("planner", &choices, &(Provider::Local, "auto".to_string())),
            (Provider::Local, "Qwen3-Coder-Next".to_string())
        );
    }

    #[test]
    fn a_catalog_shape_change_fails_loudly_instead_of_mislabeling_models() {
        let error = parse("provider model context surprise\nx y z yes")
            .unwrap_err()
            .to_string();
        assert!(error.contains("unfamiliar model list header"), "{error}");
    }

    #[test]
    fn a_partial_row_is_not_silently_dropped() {
        let error = parse(
            "provider model context max-out thinking images\n\
             claude-subscription claude-opus-5 1M 128K yes\n",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unreadable model row"), "{error}");
    }
}
