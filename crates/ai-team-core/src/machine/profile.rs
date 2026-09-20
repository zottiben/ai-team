//! Loading and validating `~/.config/ai-team/machine.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::model::Provider;
use crate::paths::machine_profile_path;

/// The profile written for a new machine. It is intentionally useful but conservative:
/// local inference is free and loopback-only; every account-backed provider needs a
/// deliberate opt-in on that machine.
pub const DEFAULT_MACHINE_PROFILE: &str = r#"version = 1

# When a seat's preferred provider is denied, the first allowed, implemented provider
# in this list is used. The model comes from ai-team's subscription-only registry.
fallback = ["claude", "openai", "zai", "local"]

[providers]
claude = false
openai = false
zai = false
local = true

# Read-only context sources (D9). Both reach an account, so both are off until this
# machine says otherwise - a work laptop can leave them off entirely.
[context]
clickup = false
figma = false
"#;

/// The allow-list and fallback order for one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineProfile {
    allowed: HashMap<Provider, bool>,
    fallback: Vec<Provider>,
    context: HashMap<ContextSource, bool>,
}

/// A read-only source of work and design context (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextSource {
    ClickUp,
    Figma,
}

impl ContextSource {
    pub const ALL: &'static [ContextSource] = &[ContextSource::ClickUp, ContextSource::Figma];

    pub fn as_str(self) -> &'static str {
        match self {
            ContextSource::ClickUp => "clickup",
            ContextSource::Figma => "figma",
        }
    }

    /// The MCP endpoint, as the ai-toolbox preset spells it. Both are HTTP, which is why
    /// they can be eve connections at all while ai-planner and file-sql cannot (D4).
    pub fn url(self) -> &'static str {
        match self {
            ContextSource::ClickUp => "https://mcp.clickup.com/mcp",
            ContextSource::Figma => "https://mcp.figma.com/mcp",
        }
    }
}

impl std::fmt::Display for ContextSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    version: u32,
    fallback: Vec<Provider>,
    providers: HashMap<Provider, bool>,
    /// Absent on a profile written before context sources existed, which reads as
    /// "none allowed" - the fail-closed direction.
    #[serde(default)]
    context: HashMap<ContextSource, bool>,
}

impl MachineProfile {
    /// Parse and validate a profile. Every provider must be named explicitly: a newly
    /// added provider becoming allowed because an old file omitted it is fail-open.
    pub fn parse(source: &str) -> Result<Self> {
        let raw: ProfileFile = toml::from_str(source)
            .map_err(|e| Error::invalid(format!("invalid machine profile: {e}")))?;
        if raw.version != 1 {
            return Err(Error::invalid(format!(
                "machine profile version {} is not supported (expected 1)",
                raw.version
            )));
        }
        for provider in Provider::ALL {
            if !raw.providers.contains_key(provider) {
                return Err(Error::invalid(format!(
                    "machine profile does not say whether {provider} is allowed"
                )));
            }
        }
        if raw.providers.len() != Provider::ALL.len() {
            return Err(Error::invalid(
                "machine profile names a provider outside the subscription-only registry",
            ));
        }

        let mut seen = Vec::new();
        for provider in &raw.fallback {
            if seen.contains(provider) {
                return Err(Error::invalid(format!(
                    "machine profile lists {provider} twice in fallback"
                )));
            }
            seen.push(*provider);
        }
        if seen.len() != Provider::ALL.len()
            || Provider::ALL
                .iter()
                .any(|provider| !seen.contains(provider))
        {
            return Err(Error::invalid(
                "machine profile fallback must rank claude, openai, zai and local exactly once",
            ));
        }

        Ok(MachineProfile {
            allowed: raw.providers,
            fallback: raw.fallback,
            context: raw.context,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path).map_err(|e| Error::UnusablePath {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        Self::parse(&source)
    }

    pub fn load_default() -> Result<Self> {
        let path = machine_profile_path()?;
        if !path.exists() {
            return Err(Error::invalid(format!(
                "no machine profile at {} - run `ait init` to create the local-only default",
                path.display()
            )));
        }
        Self::load(&path)
    }

    /// The safe profile a new machine starts with.
    pub fn local_only() -> Self {
        Self::parse(DEFAULT_MACHINE_PROFILE).expect("the compiled default profile is valid")
    }

    pub fn allowed(&self, provider: Provider) -> bool {
        self.allowed.get(&provider).copied().unwrap_or(false)
    }

    /// May this machine reach that context source? Unstated means no: a source that
    /// became available because an older profile did not mention it is fail-open.
    pub fn context_allowed(&self, source: ContextSource) -> bool {
        self.context.get(&source).copied().unwrap_or(false)
    }

    pub fn fallback(&self) -> &[Provider] {
        &self.fallback
    }
}

/// Create the local-only profile if this machine has never had one. Existing policy is
/// never rewritten by init.
pub fn ensure_machine_profile() -> Result<(PathBuf, bool)> {
    ensure_machine_profile_at(&machine_profile_path()?)
}

/// The same, at a path the caller chose.
///
/// Split out so readiness can repair a machine it was handed rather than the one the
/// process happens to be running on - which is also what makes the repair testable
/// without mutating the environment.
pub(crate) fn ensure_machine_profile_at(path: &Path) -> Result<(PathBuf, bool)> {
    let path = path.to_path_buf();
    if path.exists() {
        return Ok((path, false));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid("machine profile has no parent directory"))?;
    std::fs::create_dir_all(parent).map_err(|e| Error::UnusablePath {
        path: parent.to_path_buf(),
        reason: e.to_string(),
    })?;
    std::fs::write(&path, DEFAULT_MACHINE_PROFILE).map_err(|e| Error::UnusablePath {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    Ok((path, true))
}
