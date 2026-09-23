//! The default roster of six.
//!
//! A node earns a seat only if it needs a different model, a different tool surface, or
//! is a read-only reviewer. These six each clear that bar and nothing else does yet:
//! software-teams' 34-agent roster is the failure mode this list exists to avoid.
//!
//! The portable presets still have no machine-specific model. Team creation combines them
//! with exact role defaults from Pi's catalogue and writes those choices into the rows.
//! Existing teams change only through an explicit edit or reset; dispatch resolves machine
//! policy without rewriting the roster.

use crate::model::{NewAgent, Provider, Reasoning};

/// The seat that plans. Every other seat builds or checks.
///
/// Named rather than inferred: "the one that is read-only and first" was true of the
/// eve roster by accident and would stop being true the moment somebody reordered it.
pub const ROOT_ROLE: &str = "orchestrator";

/// The seat that checks the makers' work (M2-S9). Named rather than inferred from
/// `read_only`, because the reviewer is read-only too and is asked a different question.
pub const VERIFIER_ROLE: &str = "verifier";

#[derive(Debug)]
pub struct RolePreset {
    pub role: &'static str,
    pub name: &'static str,
    pub purpose: &'static str,
    pub zone: &'static str,
    pub reasoning: Reasoning,
    /// Read-only seats cannot write to the worktree at all. The verifier and the
    /// reviewer are checkers, and a checker that can edit the thing it is checking is
    /// just a second maker (pillar 3: maker != checker).
    pub read_only: bool,
}

/// The six. Order is dispatch order, not importance.
pub const DEFAULT_ROSTER: &[RolePreset] = &[
    RolePreset {
        role: "orchestrator",
        name: "Orchestrator",
        purpose: "Grounds one prompt in the repository and required external context, then \
                  delegates a precise brief to the planner. Coordinates the agent graph but \
                  never shapes the board, dispatches processes, or edits code.",
        zone: "",
        reasoning: Reasoning::High,
        read_only: true,
    },
    RolePreset {
        role: "planner",
        name: "Planner",
        purpose: "Turns the orchestrator's grounded brief into an ai-planner plan: one user story \
                  per pull request, with acceptance criteria, a demo, and tasks each owned by a \
                  seat. Owns the shape of the work, not the code or dispatch.",
        zone: "",
        reasoning: Reasoning::High,
        read_only: true,
    },
    RolePreset {
        role: "backend",
        name: "Backend",
        purpose: "Server, data model, migrations and the CLI. Writes the tests that prove its own \
                  changes.",
        zone: "crates/**\nmigrations/**\n*.toml",
        reasoning: Reasoning::Medium,
        read_only: false,
    },
    RolePreset {
        role: "frontend",
        name: "Frontend",
        purpose: "The app: components, state, styling and the design tokens. Holds the line on \
                  how it actually looks.",
        zone: "ui/**\n*.css",
        reasoning: Reasoning::Medium,
        read_only: false,
    },
    RolePreset {
        role: "verifier",
        name: "Verifier",
        purpose: "Runs this project's own gates against a diff and rejects work that does not \
                  pass, with the failing output attached. Reads everything, writes nothing, and \
                  runs on a different model from the maker.",
        zone: "",
        reasoning: Reasoning::Medium,
        read_only: true,
    },
    RolePreset {
        role: "reviewer",
        name: "Reviewer",
        purpose: "Reads a passing diff the way a senior colleague would: naming, structure, \
                  whether it matches the surrounding code, and whether it actually did what was \
                  asked. Comments; never commits.",
        zone: "",
        reasoning: Reasoning::High,
        read_only: true,
    },
];

impl RolePreset {
    /// The agent this preset would create. `ord` follows roster order so the generated
    /// eve project and the UI list agents in the same sequence every time.
    pub fn to_new_agent(&self, ord: i64) -> NewAgent {
        self.to_new_agent_on(Provider::Local, DEFAULT_LOCAL_MODEL, ord)
    }

    /// The same, on a provider the caller chose.
    ///
    /// Used when seeding or resetting a team with the exact model chosen for this role.
    pub fn to_new_agent_on(&self, provider: Provider, model: &str, ord: i64) -> NewAgent {
        NewAgent {
            role: self.role.to_string(),
            name: self.name.to_string(),
            purpose: self.purpose.to_string(),
            provider,
            model: model.to_string(),
            reasoning: self.reasoning,
            zone: self.zone.to_string(),
            prompt_preset: Some(self.role.to_string()),
            prompt_md: None,
            context_window: None,
            read_only: self.read_only,
            enabled: true,
            ord,
        }
    }
}

/// What a seeded agent points at until a real model is chosen.
///
/// Not a model name: ailocal picks its own default per machine (whatever is installed
/// and fits the VRAM budget), so naming one here would be a guess that is wrong on every
/// machine but this one. `auto` means "ask the gateway".
pub const DEFAULT_LOCAL_MODEL: &str = "auto";

pub fn preset(role: &str) -> Option<&'static RolePreset> {
    DEFAULT_ROSTER.iter().find(|p| p.role == role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_roster_is_six() {
        // Not a style preference. "Default team is 6" is the discipline that keeps this
        // from becoming a 34-agent roster, so it is asserted rather than remembered.
        assert_eq!(DEFAULT_ROSTER.len(), 6);
    }

    #[test]
    fn every_role_is_unique_and_findable() {
        let mut roles: Vec<_> = DEFAULT_ROSTER.iter().map(|p| p.role).collect();
        roles.sort_unstable();
        let count = roles.len();
        roles.dedup();
        assert_eq!(
            roles.len(),
            count,
            "roles must be unique - they are the key"
        );
        assert!(preset("verifier").is_some());
        assert!(preset("devops").is_none());
    }

    #[test]
    fn the_checkers_are_read_only() {
        for role in ["verifier", "reviewer", "orchestrator", "planner"] {
            assert!(
                preset(role).unwrap().read_only,
                "{role} must not write code"
            );
        }
        for role in ["backend", "frontend"] {
            assert!(
                !preset(role).unwrap().read_only,
                "{role} must be able to write"
            );
        }
    }

    #[test]
    fn the_makers_own_disjoint_zones() {
        // Two makers owning the same path is how two agents edit one file at once.
        let backend = preset("backend").unwrap();
        let frontend = preset("frontend").unwrap();
        for path in ["crates/ai-team-core/src/db.rs", "Cargo.toml"] {
            assert!(crate::util::zone_matches(backend.zone, path), "{path}");
            assert!(!crate::util::zone_matches(frontend.zone, path), "{path}");
        }
        for path in ["ui/src/App.tsx", "ui/src/styles.css"] {
            assert!(crate::util::zone_matches(frontend.zone, path), "{path}");
            assert!(!crate::util::zone_matches(backend.zone, path), "{path}");
        }
    }

    #[test]
    fn a_seeded_agent_points_at_a_provider_every_machine_allows() {
        // A fresh install must run. Seeding a provider the work machine denies would
        // make the first launch fail for a reason that has nothing to do with the user.
        for (ord, p) in (0i64..).zip(DEFAULT_ROSTER) {
            let agent = p.to_new_agent(ord);
            assert_eq!(agent.provider, Provider::Local);
            assert!(agent.prompt_preset.is_some());
        }
    }
}
