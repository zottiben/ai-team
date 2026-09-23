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

/// The ai-planner tools a seat that only reads the board may call.
///
/// A maker reads the slice it is building and records what happened; it does not shape
/// the plan. A maker that can add slices can give itself work, and the board a human
/// reads stops being a plan and becomes a log of whatever the agents felt like doing.
pub(super) const PLAN_READ_TOOLS: &[&str] = &[
    "get_plan",
    "get_slice",
    "get_resume",
    "list_slices",
    "list_plans",
    "list_questions",
    "search_plans",
    "locate",
    "append_log",
];

/// The ai-planner tools only the planner may call.
///
/// Still an allow-list rather than "everything": `delete_plan` is on the server and is
/// not something a turn should reach for, and neither is `import_markdown`. Nor is
/// `create_plan`: a run's plan is created by ai-team before the planner's turn, and the
/// planner shapes that one.
pub(super) const PLAN_WRITE_TOOLS: &[&str] = &[
    "get_plan",
    "get_slice",
    "get_resume",
    "list_slices",
    "list_plans",
    "list_questions",
    "search_plans",
    "locate",
    "append_log",
    "add_slice",
    "update_slice",
    "set_slice_status",
    "claim_slice",
    "release_slice",
    "add_decision",
    "add_gotcha",
    "open_question",
    "update_section",
];

/// How a seat reaches ai-planner.
///
/// D4 said the neighbours are used over their own interfaces and never vendored, and on
/// eve that meant a generated tool shelling out to `aip`, because eve could not run a
/// stdio MCP server. Pi can, so the neighbour is reached through the interface it
/// actually publishes - `aip serve` - and the generated wrapper is gone.
///
/// `--root` is the checkout, deliberately not the lease. A lease is a copy, and a plan
/// written inside one is a plan nobody finds again.
///
/// The plan is named in the server's environment, `AI_PLANNER_PLAN`, which ai-planner
/// treats as naming it on every call. Left to infer one, it answers with whichever plan
/// the checkout has resolved to most - a previous run's, on a checkout that has had one -
/// so a seat reading "the plan" read that one, and a planner once deferred a slice in it.
fn planner_server(access: PlanAccess<'_>) -> Value {
    json!({
        "command": "aip",
        "args": ["serve", "--root", access.root.to_string_lossy()],
        "env": { "AI_PLANNER_PLAN": access.plan },
        "transport": "stdio",
        // Connected up front and registered as real tools rather than reached through
        // the adapter's proxy. Both matter and the first run proved it: lazily-proxied
        // tools are not in the model's tool list, so a seat told to call `add_slice`
        // sees no such tool, decides the board is unavailable, and writes the code
        // itself. The plan is this seat's entire job - it should not have to go
        // looking for the way to do it.
        "lifecycle": "eager",
        "directTools": true,
        "includeTools": if access.may_write { PLAN_WRITE_TOOLS } else { PLAN_READ_TOOLS },
    })
}

/// Which plan a seat works from, and whether it may shape it.
#[derive(Debug, Clone, Copy)]
pub struct PlanAccess<'a> {
    /// The checkout whose plan it is. Never a lease.
    pub root: &'a Path,
    /// The plan, by slug. Named on every call the seat makes.
    pub plan: &'a str,
    pub may_write: bool,
}

/// The MCP config for one seat, or `None` when it has no context sources.
///
/// Returning `None` rather than an empty config matters: `--mcp-config` pointing at a
/// file with no servers is a flag that looks deliberate and does nothing. Ordinary seats
/// let Pi merge repository servers. Planning seats run exclusive and copy safe unrelated
/// repository servers into this config so generated context definitions have precedence.
pub(super) fn mcp_config(sources: &[ContextSource], plan: Option<PlanAccess<'_>>) -> Option<Value> {
    if sources.is_empty() && plan.is_none() {
        return None;
    }
    let mut servers = Map::new();
    if let Some(access) = plan {
        servers.insert("ai-planner".to_string(), planner_server(access));
    }
    for source in sources {
        servers.insert(source.as_str().to_string(), context_server(*source, true));
    }
    Some(json!({ "mcpServers": servers }))
}

/// One read-only context server, in the adapter's own format.
///
/// OAuth is explicit rather than left to auto-detection. Pi deliberately gives custom
/// headers precedence over implicit OAuth, so a saved manual token would otherwise make
/// `/mcp-auth clickup` say the server does not support OAuth. Explicit `auth: "oauth"`
/// leaves browser auth available while the header remains a fallback for machines that
/// cannot complete it.
fn context_server(source: ContextSource, manual_token: bool) -> Value {
    let allow = match source {
        ContextSource::ClickUp => CLICKUP_READ_TOOLS,
        ContextSource::Figma => FIGMA_READ_TOOLS,
    };
    let mut server = json!({
        "url": source.url(),
        "auth": "oauth",
        // D15, in the adapter's spelling. An allow-list, so a name nobody thought of is
        // absent rather than permitted - and it filters discovery too, so an excluded
        // tool is not merely refused, it is never offered.
        "includeTools": allow,
    });

    if source == ContextSource::Figma {
        // Figma's registration endpoint rejects Pi's default client name with HTTP 403.
        // Claude Code is one of Figma's supported clients and is the subscription-backed
        // Claude runtime ai-team already relies on, so identify this compatibility flow
        // with the name Figma accepts. pi-mcp-adapter deliberately exposes this override
        // for OAuth servers whose dynamic-registration policy restricts client names.
        server["oauth"] = json!({ "clientName": "Claude Code" });
    }

    // The header is written only when there is a token to put in it. Pi throws while
    // resolving `${VAR}` if it is unset, so an empty placeholder is worse than none.
    if manual_token && crate::secrets::has_token(source) {
        let env = crate::secrets::token_env(source);
        server["headers"] = json!({ "Authorization": format!("Bearer ${{{env}}}") });
    }
    server
}

/// Write the tiny config an interactive Pi needs to authenticate one context source.
///
/// No manual-token header, even if one exists: this Pi exists specifically to run the
/// OAuth flow. The same read allow-list as a seat is retained so successful reconnect is
/// proving the exact capability seats receive, not a broader test server.
pub fn write_context_oauth_config(source: ContextSource) -> Result<PathBuf> {
    let dir = crate::data_dir()?.join("oauth");
    std::fs::create_dir_all(&dir).map_err(|error| Error::UnusablePath {
        path: dir.clone(),
        reason: error.to_string(),
    })?;
    let path = dir.join(format!("{}.json", source.as_str()));
    let mut servers = Map::new();
    servers.insert(source.as_str().to_string(), context_server(source, false));
    let config = json!({ "mcpServers": servers });
    std::fs::write(&path, serde_json::to_string_pretty(&config)?).map_err(|error| {
        Error::UnusablePath {
            path: path.clone(),
            reason: error.to_string(),
        }
    })?;
    Ok(path)
}

/// The credentials a seat's own context sources need, as environment for its process.
///
/// Read here rather than left to the operator's shell (D23): the window is started from
/// Finder, which has never read a `.zshrc`, so the value comes from wherever this machine
/// keeps it. A source with no token contributes nothing, and its server was written
/// without a header to match.
pub(super) fn context_environment(sources: &[ContextSource]) -> Vec<(String, String)> {
    sources
        .iter()
        .filter_map(|source| {
            crate::secrets::token(*source).map(|value| (crate::secrets::token_env(*source), value))
        })
        .collect()
}

/// Write a seat's MCP config beside the guard and return its path.
///
/// Outside the lease, like the guard: a config a node can edit is a node that can widen
/// its own allow-list.
pub(super) fn write_mcp_config(
    dir: &Path,
    role: &str,
    worktree: &Path,
    sources: &[ContextSource],
    plan: Option<PlanAccess<'_>>,
) -> Result<Option<PathBuf>> {
    let Some(mut config) = mcp_config(sources, plan) else {
        return Ok(None);
    };
    if role == crate::ROOT_ROLE && plan.is_some() {
        grant_handoff_tool(&mut config)?;
    }
    if is_planning_role(role) {
        merge_safe_project_servers(worktree, &mut config)?;
    }
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

fn grant_handoff_tool(config: &mut Value) -> Result<()> {
    let tools = config
        .pointer_mut("/mcpServers/ai-planner/includeTools")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::invalid("orchestrator MCP config has no ai-planner allow-list"))?;
    tools.push(Value::String("write_handoff".into()));
    Ok(())
}

/// Planning runs in adapter-exclusive mode so a checkout cannot replace the generated
/// ClickUp/Figma allow-lists or offer a browser when required context fails. Unrelated
/// repository servers are copied into that exclusive config rather than discarded.
fn merge_safe_project_servers(worktree: &Path, config: &mut Value) -> Result<()> {
    const RESERVED: &[&str] = &[
        "ai-planner",
        "clickup",
        "figma",
        "playwright",
        "chrome-devtools",
    ];
    let target = config
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::invalid("generated MCP config has no server map"))?;

    for path in [worktree.join(".mcp.json"), worktree.join(".pi/mcp.json")] {
        if !path.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&path).map_err(|error| Error::UnusablePath {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        // Repository MCP files are optional neighbours. Planning runs in exclusive mode,
        // so an unrelated malformed shape cannot weaken our allow-lists; it should not
        // prevent the orchestrator from starting either. Pi likewise ignores definitions
        // it cannot discover as an `mcpServers` map.
        let Ok(project) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        let Some(servers) = project.get("mcpServers").and_then(Value::as_object) else {
            continue;
        };
        for (name, server) in servers {
            if !RESERVED.contains(&name.as_str()) {
                target.insert(name.clone(), server.clone());
            }
        }
    }
    Ok(())
}

fn is_planning_role(role: &str) -> bool {
    role == crate::ROOT_ROLE || role == "planner"
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
    /// The plan this seat works from, and whether it may shape it.
    ///
    /// `None` for a turn with no plan behind it - a single-node run, somebody talking to a
    /// seat, or the orchestrator grounding a request before its plan exists - where the
    /// planning tools stay absent rather than pointing at whichever plan the working
    /// directory happens to resolve to.
    pub plan: Option<PlanAccess<'a>>,
    /// The team this seat sits on, and who else is on it. A planning seat is told the
    /// roster because routing is by zone; a maker is not, because it is not deciding who
    /// does what.
    pub team: &'a crate::model::Team,
    pub roster: &'a [Agent],
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
        turn.mcp_config = write_mcp_config(
            self.support,
            &self.agent.role,
            self.worktree,
            self.sources,
            self.plan,
        )?;
        // Only this seat's own sources. The ticket reaches the seats that decide what the
        // work is and the designs reach the seat that owns the UI (D9), so handing every
        // seat every credential would undo the scoping the MCP config just did.
        turn.environment = context_environment(self.sources);
        if is_planning_role(&self.agent.role) {
            // The adapter otherwise merges repository and global MCP definitions after
            // this file. Exclusive mode makes the generated read-only definitions win;
            // unrelated project servers were copied above so precedence costs no context.
            turn.environment
                .push(("PI_MCP_CONFIG_MODE".into(), "exclusive".into()));
            turn.exclude_tools.extend(
                [
                    "web_search",
                    "source_check",
                    "fetch_content",
                    "get_search_content",
                ]
                .into_iter()
                .map(str::to_string),
            );
        }
        turn.instructions = Some(super::instructions::for_seat(
            self.agent,
            self.team,
            self.roster,
        ));
        Ok(turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewProject;
    use crate::store::Store;

    fn team_of() -> (crate::model::Team, Vec<Agent>) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let agents = store.agents(team.id).unwrap();
        (team, agents)
    }

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
        assert!(
            write_mcp_config(dir.path(), "backend", dir.path(), &[], None)
                .unwrap()
                .is_none()
        );
        assert!(mcp_config(&[], None).is_none());
    }

    #[test]
    fn a_context_source_is_read_only_by_allow_list() {
        // D15. Every write ClickUp exposes must be absent, and absent because it was
        // never named rather than because it was listed as forbidden.
        let config = mcp_config(&[ContextSource::ClickUp], None).expect("a config");
        let text = serde_json::to_string(&config).unwrap();

        assert!(text.contains("includeTools"), "{text}");
        assert_eq!(
            config["mcpServers"]["clickup"]["auth"], "oauth",
            "OAuth must stay explicit: a manual header disables auto-detection"
        );
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
        let config = mcp_config(&[ContextSource::Figma], None).expect("a config");
        let text = serde_json::to_string(&config).unwrap();
        assert!(text.contains("get_design_context"), "{text}");
        assert!(!text.contains("use_figma"), "{text}");
    }

    #[test]
    fn figma_uses_an_oauth_client_identity_its_allow_list_accepts() {
        // Figma rejects otherwise-valid dynamic registration from Pi with HTTP 403. Its
        // public MCP endpoint currently admits the Claude Code client identity, and the
        // adapter exposes clientName specifically for servers with this restriction.
        let config = mcp_config(&[ContextSource::Figma], None).expect("a config");
        assert_eq!(
            config["mcpServers"]["figma"]["oauth"]["clientName"],
            "Claude Code"
        );
        let clickup = mcp_config(&[ContextSource::ClickUp], None).expect("a config");
        assert!(
            clickup["mcpServers"]["clickup"].get("oauth").is_none(),
            "an unrelated source must keep Pi's real client identity"
        );
    }

    #[test]
    fn a_source_with_no_token_is_declared_without_a_header() {
        // Not tidiness. Pi's adapter throws while *resolving* headers when a configured
        // `${VAR}` is unset - "Missing environment credential in OAuth HTTP headers" - so
        // an empty placeholder does not degrade to unauthenticated, it stops the server
        // connecting at all. With no header the adapter runs its own OAuth instead, keyed
        // by the server name, which is why these are named `clickup` and `figma`.
        //
        // Asserted on whichever source this machine has no token for, so a developer who
        // has set one up is still testing the branch rather than skipping it.
        let Some(&without) = ContextSource::ALL
            .iter()
            .find(|source| !crate::secrets::has_token(**source))
        else {
            return;
        };

        let config = mcp_config(&[without], None).expect("a config");
        let server = &config["mcpServers"][without.as_str()];
        assert!(
            server.get("headers").is_none(),
            "an unset placeholder is worse than no header: {server}"
        );
        // And the allow-list is still there - no token is not no rules.
        assert!(server.get("includeTools").is_some(), "{server}");
    }

    #[test]
    fn the_browser_flow_never_puts_a_manual_token_header_in_its_config() {
        // The config used to authenticate exists specifically to exercise OAuth. A
        // manual header here can satisfy the endpoint before the adapter challenges and
        // leave the browser button appearing to work without ever creating OAuth.
        let server = context_server(ContextSource::ClickUp, false);
        assert_eq!(server["auth"], "oauth");
        assert!(server.get("headers").is_none(), "{server}");
        assert!(server.get("includeTools").is_some(), "{server}");
    }

    #[test]
    fn a_stored_token_becomes_the_header_and_the_seats_environment() {
        // The other half of the branch above, and the one that needs a token to exist.
        // Under `cfg(test)` the secret store is a map in memory, so this touches no
        // keychain and raises no dialog - but it is shared with the tests in that module,
        // so it still puts back whatever it found. Skipped when the token comes from the
        // environment, because that is not ours to restore.
        let source = ContextSource::ClickUp;
        if crate::secrets::held(source) == crate::secrets::Held::Environment {
            return;
        }
        // The store is global, so a test using it takes its turn - otherwise the secrets
        // tests blank it partway through this one.
        let _turn = crate::secrets::fake::TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = crate::secrets::token(source);
        crate::secrets::set_token(source, "pk_seat_test").unwrap();

        let config = mcp_config(&[source], None).expect("a config");
        let header = config["mcpServers"]["clickup"]["headers"]["Authorization"]
            .as_str()
            .map(str::to_string);
        let environment = context_environment(&[source]);

        // Restored before asserting, so a failing assertion does not leave the store set
        // for whichever test runs next.
        match &before {
            Some(original) => crate::secrets::set_token(source, original).unwrap(),
            None => crate::secrets::clear_token(source).unwrap(),
        }

        // The header names the variable; the value travels in the environment. Never the
        // other way round - the config is a file on disk beside the guard, and a
        // credential written into it would outlive the turn.
        assert_eq!(header.as_deref(), Some("Bearer ${AI_TEAM_CLICKUP_TOKEN}"));
        assert_eq!(
            environment,
            vec![(
                "AI_TEAM_CLICKUP_TOKEN".to_string(),
                "pk_seat_test".to_string()
            )]
        );
    }

    #[test]
    fn a_seat_is_given_only_its_own_sources_credentials() {
        // The ticket reaches the seats that decide what the work is, the designs reach the
        // seat that owns the UI (D9). Handing every seat every credential would undo the
        // scoping the MCP config just did.
        assert!(
            context_environment(&[]).is_empty(),
            "a seat with no sources needs nothing"
        );

        for source in ContextSource::ALL {
            let names: Vec<String> = context_environment(&[*source])
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            let other = ContextSource::ALL
                .iter()
                .find(|candidate| *candidate != source)
                .unwrap();
            assert!(
                !names.contains(&crate::secrets::token_env(*other)),
                "a {source} seat was handed {other}'s credential: {names:?}"
            );
        }
    }

    #[test]
    fn a_seat_s_config_is_written_outside_the_lease() {
        // A config a node can edit is a node that can widen its own allow-list.
        let support = tempfile::tempdir().unwrap();
        let path = write_mcp_config(
            support.path(),
            "planner",
            support.path(),
            &[ContextSource::ClickUp],
            None,
        )
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
        let planner = write_mcp_config(
            support.path(),
            "planner",
            support.path(),
            &[ContextSource::ClickUp],
            None,
        )
        .unwrap()
        .unwrap();
        let frontend = write_mcp_config(
            support.path(),
            "frontend",
            support.path(),
            &[ContextSource::Figma],
            None,
        )
        .unwrap()
        .unwrap();
        assert_ne!(planner, frontend);

        let frontend_text = std::fs::read_to_string(&frontend).unwrap();
        assert!(!frontend_text.contains("clickup"), "{frontend_text}");
    }

    #[test]
    fn planning_config_keeps_unrelated_project_servers_but_owns_context_and_browser_names() {
        let support = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        std::fs::write(
            checkout.path().join(".mcp.json"),
            r#"{"mcpServers":{"context7":{"command":"context7"},"clickup":{"url":"wrong"},"playwright":{"command":"browser"}}}"#,
        )
        .unwrap();
        std::fs::create_dir(checkout.path().join(".pi")).unwrap();
        std::fs::write(
            checkout.path().join(".pi/mcp.json"),
            r#"{"mcpServers":{"file-sql":{"command":"file-sql"},"chrome-devtools":{"command":"browser"}}}"#,
        )
        .unwrap();

        let path = write_mcp_config(
            support.path(),
            "planner",
            checkout.path(),
            &[ContextSource::ClickUp],
            Some(access(checkout.path(), true)),
        )
        .unwrap()
        .unwrap();
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let servers = config["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("context7"));
        assert!(servers.contains_key("file-sql"));
        assert_eq!(servers["clickup"]["auth"], "oauth");
        assert!(servers.contains_key("ai-planner"));
        assert!(!servers.contains_key("playwright"));
        assert!(!servers.contains_key("chrome-devtools"));
    }

    #[test]
    fn an_unrelated_project_mcp_shape_cannot_stop_planning() {
        for body in ["not json", r#"{"servers":{}}"#] {
            let support = tempfile::tempdir().unwrap();
            let checkout = tempfile::tempdir().unwrap();
            std::fs::write(checkout.path().join(".mcp.json"), body).unwrap();

            let path = write_mcp_config(
                support.path(),
                "planner",
                checkout.path(),
                &[],
                Some(access(checkout.path(), true)),
            )
            .expect("an unrelated repository config is optional")
            .unwrap();
            let config: Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert!(config["mcpServers"].get("ai-planner").is_some());
        }
    }

    #[test]
    fn a_seat_becomes_a_complete_invocation() {
        let support = tempfile::tempdir().unwrap();
        let lease = tempfile::tempdir().unwrap();
        let (team, roster) = team_of();
        let agent = roster
            .iter()
            .find(|a| a.role == "verifier")
            .unwrap()
            .clone();
        let seat = Seat {
            agent: &agent,
            provider: Provider::Claude,
            model: "claude-sonnet-5",
            worktree: lease.path(),
            support: support.path(),
            sources: &[],
            plan: None,
            team: &team,
            roster: &roster,
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
    fn a_maker_reads_the_board_and_cannot_shape_it() {
        // A maker that can add slices can give itself work, and the board a human reads
        // stops being a plan and becomes a log of whatever the agents felt like doing.
        let root = tempfile::tempdir().unwrap();
        let config = mcp_config(&[], Some(access(root.path(), false))).expect("a config");
        let text = serde_json::to_string(&config).unwrap();

        assert!(text.contains("get_slice"), "{text}");
        assert!(text.contains("append_log"), "a maker records what happened");
        for write in ["add_slice", "set_slice_status", "delete_plan"] {
            assert!(!text.contains(write), "{write} is reachable: {text}");
        }
    }

    #[test]
    fn a_planning_seat_may_shape_the_board_but_not_destroy_it() {
        let root = tempfile::tempdir().unwrap();
        let config = mcp_config(&[], Some(access(root.path(), true))).expect("a config");
        let text = serde_json::to_string(&config).unwrap();

        assert!(text.contains("add_slice"), "{text}");
        assert!(text.contains("set_slice_status"), "{text}");
        // Still an allow-list rather than everything the server happens to expose.
        assert!(!text.contains("delete_plan"), "{text}");
        assert!(!text.contains("import_markdown"), "{text}");
    }

    #[test]
    fn the_planner_resolves_from_the_checkout_not_the_lease() {
        // A lease is a copy. A plan written inside one is a plan nobody finds again.
        let root = tempfile::tempdir().unwrap();
        let config = mcp_config(&[], Some(access(root.path(), true))).expect("a config");
        let text = serde_json::to_string(&config).unwrap();
        assert!(text.contains("--root"), "{text}");
        assert!(
            text.contains(&root.path().to_string_lossy().to_string()),
            "{text}"
        );
    }

    fn access(root: &Path, may_write: bool) -> PlanAccess<'_> {
        PlanAccess {
            root,
            plan: "csv-export",
            may_write,
        }
    }

    #[test]
    fn every_seat_is_pointed_at_its_runs_plan_and_none_may_start_another() {
        // Left to infer the plan, ai-planner answers with whichever one the checkout has
        // resolved to most: a previous run's. Named, every call acts on this run's.
        let root = tempfile::tempdir().unwrap();
        for may_write in [false, true] {
            let config = mcp_config(&[], Some(access(root.path(), may_write))).expect("a config");
            assert_eq!(
                config["mcpServers"]["ai-planner"]["env"]["AI_PLANNER_PLAN"],
                "csv-export"
            );
            let text = serde_json::to_string(&config).unwrap();
            // ai-team creates a run's plan; a seat that can create one can start a
            // second board beside it.
            assert!(!text.contains("create_plan"), "{text}");
        }
    }

    #[test]
    fn a_turn_with_no_plan_gets_no_planning_tools() {
        // A single-node run or a person talking to a seat has no plan behind it, and
        // tools pointing at whichever plan the cwd resolves to are worse than none.
        assert!(mcp_config(&[], None).is_none());
    }

    #[test]
    fn coordinating_turns_are_exclusive_and_cannot_fall_back_to_the_web() {
        let support = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let (team, roster) = team_of();
        let agent = roster
            .iter()
            .find(|agent| agent.role == crate::ROOT_ROLE)
            .unwrap()
            .clone();
        let seat = Seat {
            agent: &agent,
            provider: Provider::Local,
            model: "local",
            worktree: checkout.path(),
            support: support.path(),
            sources: &[ContextSource::ClickUp],
            plan: Some(access(checkout.path(), false)),
            team: &team,
            roster: &roster,
        };

        let turn = seat.turn("ground it").unwrap();
        assert!(turn
            .environment
            .contains(&("PI_MCP_CONFIG_MODE".into(), "exclusive".into())));
        for tool in ["web_search", "fetch_content", "source_check"] {
            assert!(turn.exclude_tools.contains(&tool.to_string()), "{tool}");
        }
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(turn.mcp_config.expect("a config")).unwrap(),
        )
        .unwrap();
        let planner = serde_json::to_string(&config["mcpServers"]["ai-planner"]).unwrap();
        assert!(!planner.contains("add_slice"), "{planner}");
        assert!(planner.contains("write_handoff"), "{planner}");
    }

    #[test]
    fn the_model_comes_from_the_resolution_not_the_row() {
        // A denied preference falls back (D13), and what a run actually used is a fact
        // about that run rather than about the team as it stands now.
        let support = tempfile::tempdir().unwrap();
        let lease = tempfile::tempdir().unwrap();
        let (team, roster) = team_of();
        let agent = roster.iter().find(|a| a.role == "backend").unwrap().clone();
        let seat = Seat {
            agent: &agent,
            provider: Provider::Local,
            model: "gemma4-12b",
            worktree: lease.path(),
            support: support.path(),
            sources: &[],
            plan: None,
            team: &team,
            roster: &roster,
        };

        let turn = seat.turn("build it").unwrap();
        assert_eq!(turn.provider.as_deref(), Some("llama.cpp"));
        assert_eq!(turn.model.as_deref(), Some("gemma4-12b"));
        assert_ne!(turn.model.as_deref(), Some(agent.model.as_str()));
    }
}
