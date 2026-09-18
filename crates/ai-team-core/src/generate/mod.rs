//! Generating the eve project from the team rows.
//!
//! The database is the team (D2); this directory is output. Nothing here reads what is
//! already on disk and merges with it - generation replaces the tree, because a
//! hand-edit that survived a regeneration would be a second source of truth that only
//! shows up when someone wonders why their change vanished.
//!
//! The static TypeScript lives under `assets/` as real `.ts` files rather than string
//! literals in Rust. They are `include_str!`d, so a `cargo install`ed binary carries
//! them, and they can be typechecked and read as code rather than as escaped text.

mod model;
mod render;

use std::path::{Path, PathBuf};

pub use model::ModelExpression;

use crate::error::{Error, Result};
use crate::machine::{ModelRegistry, ModelResolution};
use crate::model::{Agent, Team};
use crate::store::Store;

/// The role that becomes the root agent. Every other seat is a declared subagent.
pub const ROOT_ROLE: &str = "orchestrator";

/// The seat that checks the makers' work (M2-S9). Named rather than inferred from
/// `read_only`, because the reviewer is read-only too and is asked a different question.
pub const VERIFIER_ROLE: &str = "verifier";

/// A file the generator will write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Relative to the project root.
    pub path: PathBuf,
    pub contents: String,
}

/// Everything the generator produced, and where it goes.
#[derive(Debug, Clone)]
pub struct GeneratedProject {
    pub root: PathBuf,
    pub files: Vec<GeneratedFile>,
    /// Environment variables the supervisor must set for this project to resolve its
    /// models. Collected from the agents rather than assumed.
    pub required_env: Vec<&'static str>,
    /// The policy result for every generated seat. A fallback is visible here, in the
    /// terminal, and again as an event when the seat is dispatched.
    pub resolutions: Vec<ModelResolution>,
}

impl GeneratedProject {
    /// Write the tree, replacing whatever was there.
    ///
    /// `node_modules/`, `.eve/` and `.output/` are preserved: they are npm's and eve's,
    /// they cost minutes to rebuild, and nothing in them is generated from the rows.
    pub fn write(&self) -> Result<()> {
        if self.root.exists() {
            clear_generated(&self.root)?;
        }
        for file in &self.files {
            let target = self.root.join(&file.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::UnusablePath {
                    path: parent.to_path_buf(),
                    reason: e.to_string(),
                })?;
            }
            std::fs::write(&target, &file.contents).map_err(|e| Error::UnusablePath {
                path: target.clone(),
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }

    /// Look a generated file up by its relative path. For tests, and for `ait agents
    /// show`.
    pub fn file(&self, path: &str) -> Option<&GeneratedFile> {
        self.files.iter().find(|f| f.path == Path::new(path))
    }
}

/// Everything except the directories npm and eve own.
fn clear_generated(root: &Path) -> Result<()> {
    let keep = ["node_modules", ".eve", ".output"];
    let entries = std::fs::read_dir(root).map_err(|e| Error::UnusablePath {
        path: root.to_path_buf(),
        reason: e.to_string(),
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if keep.iter().any(|k| name == *k) {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        result.map_err(|e| Error::UnusablePath {
            path,
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

impl Store {
    /// Build the eve project for a team, without writing it.
    ///
    /// Split from writing so the CLI can show a diff, and so tests can assert on the
    /// contents without touching a filesystem.
    pub fn generate_project(
        &self,
        team_id: i64,
        root: impl Into<PathBuf>,
    ) -> Result<GeneratedProject> {
        let team = self.team(team_id)?;
        let agents: Vec<Agent> = self
            .agents(team_id)?
            .into_iter()
            .filter(|a| a.enabled)
            .collect();
        let resolutions = agents
            .iter()
            .map(|agent| ModelResolution {
                role: agent.role.clone(),
                requested_provider: agent.provider,
                requested_model: agent.model.clone(),
                provider: agent.provider,
                model: agent.model.clone(),
                context_window: agent.context_window,
                fallback_reason: None,
            })
            .collect();
        render::project(
            &team,
            &agents,
            root.into(),
            "http://127.0.0.1:8081/v1",
            resolutions,
        )
    }

    /// Build for this machine, resolving every seat through its allow-list first.
    ///
    /// This does not replace the dispatch-time check: a generated directory is cached
    /// output, whereas dispatch is the point at which permission has to be enforced.
    pub fn generate_project_for_machine(
        &self,
        team_id: i64,
        root: impl Into<PathBuf>,
        registry: &ModelRegistry,
    ) -> Result<GeneratedProject> {
        let team = self.team(team_id)?;
        let agents: Vec<Agent> = self
            .agents(team_id)?
            .into_iter()
            .filter(|agent| agent.enabled)
            .collect();
        let (effective, resolutions) = registry.resolve_agents(&agents)?;
        render::project(
            &team,
            &effective,
            root.into(),
            registry.ailocal_base_url(),
            resolutions,
        )
    }

    /// The conventional location for a project's generated eve project.
    pub fn agents_dir(&self, project_slug: &str) -> Result<PathBuf> {
        Ok(crate::data_dir()?.join("agents").join(project_slug))
    }
}

/// Validate a roster before rendering, so a broken team fails with one clear message
/// rather than a half-written directory.
pub(crate) fn check_roster(team: &Team, agents: &[Agent]) -> Result<()> {
    if agents.is_empty() {
        return Err(Error::invalid(format!(
            "team {:?} has no enabled agents to generate",
            team.slug
        )));
    }
    if !agents.iter().any(|a| a.role == ROOT_ROLE) {
        return Err(Error::invalid(format!(
            "team {:?} has no {ROOT_ROLE} - eve needs a root agent, and that is the seat \
             that dispatches the others",
            team.slug
        )));
    }
    // Every seat becomes a directory name, so a role that is not a safe slug would put
    // the generator somewhere it did not intend to write.
    for agent in agents {
        if agent.role != crate::util::slugify(&agent.role) {
            return Err(Error::invalid(format!(
                "role {:?} is not a usable directory name - use lowercase and dashes",
                agent.role
            )));
        }
    }
    Ok(())
}
