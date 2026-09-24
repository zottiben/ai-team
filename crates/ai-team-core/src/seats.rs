//! Seats that cannot run on this machine, and moving them to ones that can.
//!
//! A team is seeded with the models this machine can run when it is created, and keeps
//! them: a later catalogue change must not silently reroute it (D13). The trap is the
//! first project on a machine - `ait init` writes a profile that allows only the local
//! gateway, seeds the team against it, and Claude and ChatGPT are allowed afterwards. On a
//! machine without the gateway, every run that team starts dies at its first turn.
//!
//! So a seat that cannot run is found, said, and moved on request - to what a new team
//! here would be seeded with today, and only if Pi can run that. Nothing moves at dispatch;
//! that would be D13's silent rerouting by another name.

use crate::error::Result;
use crate::machine::{ModelChoice, ModelRegistry, ProviderStatus, RoleModelDefault, Stranded};
use crate::model::Provider;
use crate::store::Store;

/// What this machine can run, read once: Pi's catalogue, each provider's status, and the
/// model each role would be seeded with.
#[derive(Debug, Clone)]
pub struct Survey {
    pub catalog: Vec<ModelChoice>,
    pub statuses: Vec<ProviderStatus>,
    pub defaults: Vec<RoleModelDefault>,
}

impl Survey {
    /// Ask Pi and probe every provider. Blocking, and a second or two.
    pub fn read(registry: &ModelRegistry) -> Result<Survey> {
        let catalog = registry.models()?;
        let statuses = registry.statuses();
        let defaults = registry.role_defaults_from(&statuses, &catalog);
        Ok(Survey {
            catalog,
            statuses,
            defaults,
        })
    }

    /// The switched-on seats of `seats` that cannot run here.
    pub fn stranded(&self, registry: &ModelRegistry, seats: &[crate::Agent]) -> Vec<Stranded> {
        registry.stranded(seats, &self.catalog, |provider| {
            self.statuses
                .iter()
                .find(|status| status.provider == provider)
                .cloned()
        })
    }

    /// Where a stranded seat would go: its role's default here, when Pi can run that.
    pub fn replacement(&self, stranded: &Stranded) -> Option<(Provider, String)> {
        replacement(stranded, &self.defaults, &self.catalog)
    }
}

/// A stranded seat's role default, when it is one Pi can run - and not the model it is
/// already stranded on, which is where a machine with nothing runnable seeds its floor.
pub(crate) fn replacement(
    stranded: &Stranded,
    defaults: &[RoleModelDefault],
    catalog: &[ModelChoice],
) -> Option<(Provider, String)> {
    let default = defaults
        .iter()
        .find(|choice| choice.role == stranded.role)?;
    catalog
        .iter()
        .any(|choice| choice.provider == default.provider && choice.model == default.model)
        .then(|| (default.provider, default.model.clone()))
}

/// Stranded seats in a clause each for what they are on and why it cannot run:
/// `orchestrator, planner use local/auto, which cannot run here: ...; reviewer uses ...`.
pub fn describe(stranded: &[Stranded]) -> String {
    let mut groups: Vec<(String, &str, Vec<&str>)> = Vec::new();
    for seat in stranded {
        let on = format!("{}/{}", seat.provider.as_str(), seat.model);
        match groups
            .iter_mut()
            .find(|(model, why, _)| *model == on && *why == seat.why)
        {
            Some((.., roles)) => roles.push(&seat.role),
            None => groups.push((on, &seat.why, vec![&seat.role])),
        }
    }
    groups
        .iter()
        .map(|(on, why, roles)| {
            format!(
                "{} {} {on}, which cannot run here: {why}",
                roles.join(", "),
                if roles.len() == 1 { "uses" } else { "use" }
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One seat moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub project: String,
    pub role: String,
    pub from: (Provider, String),
    pub to: (Provider, String),
}

/// What a reseat did: the seats it moved, and the ones it had nowhere to move.
#[derive(Debug, Clone, Default)]
pub struct Reseated {
    pub moved: Vec<Moved>,
    pub left: Vec<(String, Stranded)>,
}

impl Reseated {
    /// In a sentence, for a button's result and the terminal.
    pub fn summary(&self) -> String {
        if self.moved.is_empty() && self.left.is_empty() {
            return "Every seat can run on this machine - nothing to move".into();
        }
        let mut parts = Vec::new();
        if !self.moved.is_empty() {
            parts.push(format!(
                "Moved {}",
                self.moved
                    .iter()
                    .map(|moved| format!(
                        "{} {} to {}/{}",
                        moved.project,
                        moved.role,
                        moved.to.0.as_str(),
                        moved.to.1
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.left.is_empty() {
            parts.push(format!(
                "{} still cannot run - there is no provider Pi can run to move {} to",
                self.left
                    .iter()
                    .map(|(project, seat)| format!("{project} {}", seat.role))
                    .collect::<Vec<_>>()
                    .join(", "),
                if self.left.len() == 1 { "it" } else { "them" }
            ));
        }
        parts.join("; ")
    }
}

/// Move every stranded seat - of one project, or of all of them - to its role's default
/// here. A seat with no runnable default is left where it is and reported.
pub fn reseat_stranded(
    store: &mut Store,
    registry: &ModelRegistry,
    survey: &Survey,
    project: Option<i64>,
) -> Result<Reseated> {
    let mut reseated = Reseated::default();
    for found in store.projects()? {
        if project.is_some_and(|only| only != found.id) {
            continue;
        }
        let Some(team_id) = found.team_id else {
            continue;
        };
        let seats = store.agents(team_id)?;
        for stranded in survey.stranded(registry, &seats) {
            let Some(to) = survey.replacement(&stranded) else {
                reseated.left.push((found.slug.clone(), stranded));
                continue;
            };
            store.move_seat(stranded.agent_id, to.0, &to.1)?;
            reseated.moved.push(Moved {
                project: found.slug.clone(),
                role: stranded.role,
                from: (stranded.provider, stranded.model),
                to,
            });
        }
    }
    Ok(reseated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stranded(role: &str) -> Stranded {
        Stranded {
            agent_id: 1,
            role: role.into(),
            provider: Provider::Local,
            model: "auto".into(),
            why: "ailocal is not set up".into(),
        }
    }

    fn default(role: &str, provider: Provider, model: &str) -> RoleModelDefault {
        RoleModelDefault {
            role: role.into(),
            provider,
            model: model.into(),
        }
    }

    fn listed(provider: Provider, model: &str) -> ModelChoice {
        ModelChoice {
            provider,
            runtime_provider: crate::pi_provider(provider).to_string(),
            model: model.into(),
            context: "1M".into(),
            context_tokens: 1_000_000,
            max_output: "128K".into(),
            thinking: true,
            images: true,
        }
    }

    #[test]
    fn a_stranded_seat_goes_to_its_roles_default_only_if_pi_can_run_that() {
        let defaults = [
            default("orchestrator", Provider::OpenAi, "gpt-6-astra"),
            default("planner", Provider::Claude, "claude-opus-5"),
            // A machine with nothing runnable seeds its floor, which is where the seat
            // already is: no move at all, rather than a move to the same failure.
            default("backend", Provider::Local, "auto"),
        ];
        let catalog = [
            listed(Provider::OpenAi, "gpt-6-astra"),
            listed(Provider::Claude, "claude-opus-5"),
        ];

        assert_eq!(
            replacement(&stranded("orchestrator"), &defaults, &catalog),
            Some((Provider::OpenAi, "gpt-6-astra".into()))
        );
        assert_eq!(
            replacement(&stranded("planner"), &defaults, &catalog),
            Some((Provider::Claude, "claude-opus-5".into()))
        );
        assert_eq!(replacement(&stranded("backend"), &defaults, &catalog), None);
        assert_eq!(
            replacement(&stranded("reviewer"), &defaults, &catalog),
            None
        );
    }

    #[test]
    fn stranded_seats_are_described_a_clause_per_model_and_reason() {
        let on = |role: &str, model: &str, why: &str| Stranded {
            model: model.into(),
            why: why.into(),
            ..stranded(role)
        };
        assert_eq!(
            describe(&[
                on("orchestrator", "auto", "not set up"),
                on("planner", "auto", "not set up"),
                on(
                    "reviewer",
                    "qwen",
                    "Pi has no llama.cpp/qwen on this machine"
                ),
            ]),
            "orchestrator, planner use local/auto, which cannot run here: not set up; reviewer \
             uses local/qwen, which cannot run here: Pi has no llama.cpp/qwen on this machine"
        );
    }

    #[test]
    fn a_reseat_says_what_moved_and_what_could_not() {
        let reseated = Reseated {
            moved: vec![Moved {
                project: "ai-team".into(),
                role: "planner".into(),
                from: (Provider::Local, "auto".into()),
                to: (Provider::Claude, "claude-opus-5".into()),
            }],
            left: vec![("ai-team".into(), stranded("backend"))],
        };
        assert_eq!(
            reseated.summary(),
            "Moved ai-team planner to claude/claude-opus-5; ai-team backend still cannot \
             run - there is no provider Pi can run to move it to"
        );
        assert_eq!(
            Reseated::default().summary(),
            "Every seat can run on this machine - nothing to move"
        );
    }

    #[test]
    fn a_seat_moved_keeps_everything_but_its_model() {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let before = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap();

        let after = store
            .move_seat(before.id, Provider::Claude, "claude-opus-5")
            .unwrap();

        assert_eq!(
            (after.provider, after.model.as_str()),
            (Provider::Claude, "claude-opus-5")
        );
        // The old model's context window says nothing about the new one.
        assert_eq!(after.context_window, None);
        assert_eq!(
            (
                &after.zone,
                after.read_only,
                after.enabled,
                after.reasoning,
                &after.prompt_md
            ),
            (
                &before.zone,
                before.read_only,
                before.enabled,
                before.reasoning,
                &before.prompt_md
            )
        );
    }
}
