//! A seat, as a Pi invocation (D20).
//!
//! The whole of what used to be a generated TypeScript project is a handful of flags.
//! That is the practical shape of the pivot: changing a seat's model is an argument, not
//! a regeneration and a rebuild, and nothing about a seat is written to disk except the
//! MCP config that carries its allow-lists.
//!
//! Two properties are carried over from the eve seats rather than re-derived:
//!
//! - **Read-only means no writing tools.** `bash` is still there, so the guarantee is
//!   "cannot edit source through its tools", not "cannot write a byte" - the same wording
//!   the eve seats carried, for the same reason.
//! - **Context sources are read-only by allow-list, never a block-list** (D15). A wrong
//!   name on an allow-list costs a capability; a missed name on a block-list hands over a
//!   write.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::context::{CLICKUP_READ_TOOLS, FIGMA_READ_TOOLS};
use crate::error::{Error, Result};
use crate::model::{Agent, Provider, Reasoning};
use crate::ContextSource;

use super::process::PiTurn;

/// What Pi calls each provider ai-team knows about (D8).
///
/// Three of the four are configured by the operator's own Pi, which is the point of D20 -
/// ai-team does not carry credentials, it names a provider the harness already has.
pub fn provider_name(provider: Provider) -> &'static str {
    match provider {
        // The `pi-claude-subscription` extension, not the metered `anthropic` provider
        // that sits beside it in the same catalogue. Naming the wrong one is the one way
        // to spend money here, which is why this is a match and not a string.
        Provider::Claude => "claude-subscription",
        Provider::OpenAi => "openai-codex",
        Provider::ZAi => "zai",
        // The ailocal gateway, as Pi has it configured.
        Provider::Local => "llama.cpp",
    }
}

/// A seat's effort, in Pi's vocabulary.
pub fn thinking(reasoning: Reasoning) -> &'static str {
    match reasoning {
        Reasoning::None => "off",
        Reasoning::Low => "low",
        Reasoning::Medium => "medium",
        Reasoning::High => "high",
    }
}

/// The tools a read-only seat does not get.
///
/// `bash` is deliberately not here. A seat that cannot run the project's own checks
/// cannot verify anything, and the verifier is read-only.
pub(super) fn withheld(agent: &Agent) -> Vec<String> {
    if agent.read_only {
        vec!["write".to_string(), "edit".to_string()]
    } else {
        Vec::new()
    }
}

/// The MCP config for one seat, or `None` when it has no context sources.
///
/// Returning `None` rather than an empty config matters: `--mcp-config` pointing at a
/// file with no servers is a flag that looks deliberate and does nothing, and the repo's
/// own `.mcp.json` is discovered either way - Pi's adapter merges what it finds with what
/// it is given rather than replacing one with the other, which is what makes D19 free
/// here.
pub(super) fn mcp_config(sources: &[ContextSource]) -> Option<Value> {
    if sources.is_empty() {
        return None;
    }
    let mut servers = Map::new();
    for source in sources {
        let (allow, env) = match source {
            ContextSource::ClickUp => (CLICKUP_READ_TOOLS, "AI_TEAM_CLICKUP_TOKEN"),
            ContextSource::Figma => (FIGMA_READ_TOOLS, "AI_TEAM_FIGMA_TOKEN"),
        };
        servers.insert(
            source.as_str().to_string(),
            json!({
                "url": source.url(),
                "headers": { "Authorization": format!("Bearer ${{{env}}}") },
                // D15, in the adapter's spelling. An allow-list, so a name nobody thought
                // of is absent rather than permitted - and it filters discovery too, so
                // an excluded tool is not merely refused, it is never offered.
                "includeTools": allow,
            }),
        );
    }
    Some(json!({ "mcpServers": servers }))
}

/// Write a seat's MCP config beside the guard and return its path.
///
/// Outside the lease, like the guard: a config a node can edit is a node that can widen
/// its own allow-list.
pub(super) fn write_mcp_config(
    dir: &Path,
    role: &str,
    sources: &[ContextSource],
) -> Result<Option<PathBuf>> {
    let Some(config) = mcp_config(sources) else {
        return Ok(None);
    };
    std::fs::create_dir_all(dir).map_err(|e| Error::UnusablePath {
        path: dir.to_path_buf(),
        reason: e.to_string(),
    })?;
    let path = dir.join(format!("mcp-{role}.json"));
    let body = serde_json::to_string_pretty(&config)?;
    std::fs::write(&path, body).map_err(|e| Error::UnusablePath {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    Ok(Some(path))
}

/// Everything a seat needs, assembled into one invocation.
///
/// `model` is passed in rather than read off the agent because the registry may have
/// resolved a fallback (D13), and what a run actually used is a fact about the run.
#[derive(Debug)]
pub struct Seat<'a> {
    pub agent: &'a Agent,
    pub provider: Provider,
    pub model: &'a str,
    pub worktree: &'a Path,
    /// Where the guard and any MCP config live. Never the lease.
    pub support: &'a Path,
    pub sources: &'a [ContextSource],
}

impl Seat<'_> {
    /// Build the turn this seat would take, given a prompt.
    pub fn turn(&self, prompt: impl Into<String>) -> Result<PiTurn> {
        let mut turn = PiTurn::new(self.worktree, prompt);
        turn.provider = Some(provider_name(self.provider).to_string());
        turn.model = Some(self.model.to_string());
        turn.thinking = Some(thinking(self.agent.reasoning).to_string());
        turn.exclude_tools = withheld(self.agent);
        turn.guard = Some(super::guard::install_at(self.support)?);
        turn.mcp_config = write_mcp_config(self.support, &self.agent.role, self.sources)?;
        Ok(turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewProject;
    use crate::store::Store;

    fn seat_of(role: &str) -> Agent {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == role)
            .unwrap()
    }

    #[test]
    fn the_claude_seat_names_the_subscription_not_the_metered_provider() {
        // Both sit in Pi's catalogue under names one letter apart in meaning, and only
        // one of them is free. D8 exists to stop exactly this.
        assert_eq!(provider_name(Provider::Claude), "claude-subscription");
        assert_ne!(provider_name(Provider::Claude), "anthropic");
        assert_eq!(provider_name(Provider::OpenAi), "openai-codex");
        assert_eq!(provider_name(Provider::Local), "llama.cpp");
    }

    #[test]
    fn a_read_only_seat_withholds_writing_but_keeps_bash() {
        // The verifier is read-only and has to run the project's own checks. A seat that
        // cannot run them cannot verify anything.
        let verifier = seat_of("verifier");
        assert!(verifier.read_only);
        let withheld = withheld(&verifier);
        assert!(withheld.contains(&"write".to_string()));
        assert!(withheld.contains(&"edit".to_string()));
        assert!(!withheld.contains(&"bash".to_string()));
    }

    #[test]
    fn a_maker_keeps_its_writing_tools() {
        let backend = seat_of("backend");
        assert!(!backend.read_only);
        assert!(withheld(&backend).is_empty());
    }

    #[test]
    fn a_seat_with_no_context_source_gets_no_config_file() {
        // An empty config is a flag that looks deliberate and does nothing. The repo's
        // own `.mcp.json` is discovered regardless.
        let dir = tempfile::tempdir().unwrap();
        assert!(write_mcp_config(dir.path(), "backend", &[])
            .unwrap()
            .is_none());
        assert!(mcp_config(&[]).is_none());
    }

    #[test]
    fn a_context_source_is_read_only_by_allow_list() {
        // D15. Every write ClickUp exposes must be absent, and absent because it was
        // never named rather than because it was listed as forbidden.
        let config = mcp_config(&[ContextSource::ClickUp]).expect("a config");
        let text = serde_json::to_string(&config).unwrap();

        assert!(text.contains("includeTools"), "{text}");
        assert!(
            !text.contains("excludeTools"),
            "a block-list crept in: {text}"
        );
        assert!(text.contains("get_task"), "{text}");
        for write in ["create_task", "update_task", "delete_task"] {
            assert!(!text.contains(write), "{write} is reachable: {text}");
        }
    }

    #[test]
    fn figma_s_write_that_reads_like_a_read_is_absent() {
        // `use_figma` creates, edits and deletes despite its name, which is why the rule
        // is an allow-list and not a judgement call about what sounds safe.
        let config = mcp_config(&[ContextSource::Figma]).expect("a config");
        let text = serde_json::to_string(&config).unwrap();
        assert!(text.contains("get_design_context"), "{text}");
        assert!(!text.contains("use_figma"), "{text}");
    }

    #[test]
    fn a_seat_s_config_is_written_outside_the_lease() {
        // A config a node can edit is a node that can widen its own allow-list.
        let support = tempfile::tempdir().unwrap();
        let path = write_mcp_config(support.path(), "planner", &[ContextSource::ClickUp])
            .unwrap()
            .expect("a path");
        assert!(path.starts_with(support.path()));
        assert!(path.to_string_lossy().contains("planner"), "{path:?}");
    }

    #[test]
    fn two_seats_do_not_share_one_config() {
        // Scoping is the point of D15: the ticket reaches the seats that decide what the
        // work is, and one shared file would give it to everybody.
        let support = tempfile::tempdir().unwrap();
        let planner = write_mcp_config(support.path(), "planner", &[ContextSource::ClickUp])
            .unwrap()
            .unwrap();
        let frontend = write_mcp_config(support.path(), "frontend", &[ContextSource::Figma])
            .unwrap()
            .unwrap();
        assert_ne!(planner, frontend);

        let frontend_text = std::fs::read_to_string(&frontend).unwrap();
        assert!(!frontend_text.contains("clickup"), "{frontend_text}");
    }

    #[test]
    fn a_seat_becomes_a_complete_invocation() {
        let support = tempfile::tempdir().unwrap();
        let lease = tempfile::tempdir().unwrap();
        let agent = seat_of("verifier");
        let seat = Seat {
            agent: &agent,
            provider: Provider::Claude,
            model: "claude-sonnet-5",
            worktree: lease.path(),
            support: support.path(),
            sources: &[],
        };

        let turn = seat.turn("check it").unwrap();
        assert_eq!(turn.provider.as_deref(), Some("claude-subscription"));
        assert_eq!(turn.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(turn.thinking.as_deref(), Some("medium"));
        assert_eq!(turn.exclude_tools, ["write", "edit"]);
        // The guard is always loaded. A turn without it is a seat with unrestricted
        // filesystem access, which is the one thing Pi gives away (M9-S39).
        assert!(turn.guard.is_some());
        assert!(turn.mcp_config.is_none());
    }

    #[test]
    fn the_model_comes_from_the_resolution_not_the_row() {
        // A denied preference falls back (D13), and what a run actually used is a fact
        // about that run rather than about the team as it stands now.
        let support = tempfile::tempdir().unwrap();
        let lease = tempfile::tempdir().unwrap();
        let agent = seat_of("backend");
        let seat = Seat {
            agent: &agent,
            provider: Provider::Local,
            model: "gemma4-12b",
            worktree: lease.path(),
            support: support.path(),
            sources: &[],
        };

        let turn = seat.turn("build it").unwrap();
        assert_eq!(turn.provider.as_deref(), Some("llama.cpp"));
        assert_eq!(turn.model.as_deref(), Some("gemma4-12b"));
        assert_ne!(turn.model.as_deref(), Some(agent.model.as_str()));
    }
}
