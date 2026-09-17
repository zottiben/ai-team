//! The generated eve project.
//!
//! These assert on contents rather than on "it wrote some files", because the whole
//! bargain of D2 is that this directory is derived from the rows. A generator that
//! produces a plausible-looking project with the wrong tools wired in is exactly the
//! failure nobody notices until an agent does something it should not have been able to.

use ai_team_core::{
    Guardrails, ModelRegistry, NewProject, Provider, Store, ToolEffect, DEFAULT_ROSTER, ROOT_ROLE,
};

fn seeded() -> (Store, i64, i64) {
    let mut store = Store::memory().unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget Service".into(),
            ..Default::default()
        })
        .unwrap();
    let team = store.seed_default_team(project.id).unwrap();
    (store, project.id, team.id)
}

#[test]
fn the_roster_becomes_a_root_agent_and_a_subagent_per_seat() {
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    assert!(generated.file("agent/agent.ts").is_some(), "the root agent");
    assert!(generated.file("agent/instructions.md").is_some());

    // Five subagents: the roster of six, minus the orchestrator that became the root.
    for preset in DEFAULT_ROSTER.iter().filter(|p| p.role != ROOT_ROLE) {
        let path = format!("agent/subagents/{}/agent.ts", preset.role);
        let agent_ts = generated
            .file(&path)
            .unwrap_or_else(|| panic!("{path} must exist"));
        // A declared subagent without a description cannot be routed to.
        assert!(agent_ts.contents.contains("description:"), "{path}");
        assert!(generated
            .file(&format!("agent/subagents/{}/instructions.md", preset.role))
            .is_some());
    }
    assert!(
        generated
            .file(&format!("agent/subagents/{ROOT_ROLE}/agent.ts"))
            .is_none(),
        "the orchestrator is the root, not a subagent of itself"
    );
}

#[test]
fn the_authored_tools_replace_eves_sandbox_defaults_at_the_same_slots() {
    // D3: agents edit the leased worktree. Occupying eve's own slot names is what makes
    // the model use them without being told to prefer a parallel set.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    for slot in ["bash", "read_file"] {
        let file = generated.file(&format!("agent/tools/{slot}.ts")).unwrap();
        assert!(
            file.contents.contains("defineTool"),
            "{slot} must be authored"
        );
        assert!(
            file.contents.contains("lib/tools.js"),
            "{slot} must call the worktree implementation"
        );
        assert!(
            file.contents.contains("export const bridgedTool"),
            "{slot} must expose the same implementation to Claude's MCP bridge"
        );
        assert!(
            !file.contents.contains("eve/tools/bash"),
            "{slot} must not re-export eve's sandbox tool"
        );
    }

    // The sandbox defaults are not disabled file-by-file: `defaultTools: false` removes
    // the whole optional set, and `disableTool()` at a slot with no framework default
    // underneath it is a build error.
    for slot in ["web_fetch", "web_search", "todo"] {
        assert!(
            generated.file(&format!("agent/tools/{slot}.ts")).is_none(),
            "{slot} must simply be absent"
        );
    }

    // The root agent turns the optional defaults off wholesale as well.
    let agent_ts = &generated.file("agent/agent.ts").unwrap().contents;
    assert!(agent_ts.contains("defaultTools: false"));
}

#[test]
fn a_read_only_seat_does_not_get_write_tools_at_all() {
    // The verifier and the reviewer are checkers. This is the difference between an
    // instruction not to write and an inability to.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    for role in ["verifier", "reviewer", "planner"] {
        for slot in ["write_file", "edit_file"] {
            let path = format!("agent/subagents/{role}/tools/{slot}.ts");
            assert!(
                generated.file(&path).is_none(),
                "{path} must not exist at all"
            );
        }
        // Reading is still allowed - a reviewer that cannot read is useless.
        let read = format!("agent/subagents/{role}/tools/read_file.ts");
        assert!(generated
            .file(&read)
            .unwrap()
            .contents
            .contains("defineTool"));
    }

    for role in ["backend", "frontend"] {
        for slot in ["write_file", "edit_file"] {
            let path = format!("agent/subagents/{role}/tools/{slot}.ts");
            let file = generated.file(&path).unwrap();
            assert!(
                file.contents.contains("defineTool"),
                "{path} must be able to write"
            );
        }
    }
}

#[test]
fn a_subagents_tool_imports_reach_the_shared_lib() {
    // Subagents inherit none of the parent's authored slots, so each has its own tool
    // files. They are re-exports, and the relative depth differs - a wrong `../` count
    // is a project that only fails at build time.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    let root = &generated.file("agent/tools/bash.ts").unwrap().contents;
    assert!(root.contains(r#"from "../lib/tools.js""#), "{root}");

    let sub = &generated
        .file("agent/subagents/backend/tools/bash.ts")
        .unwrap()
        .contents;
    assert!(sub.contains(r#"from "../../../lib/tools.js""#), "{sub}");
    assert!(
        !sub.contains(r#"from "../lib/"#),
        "the root's depth must not survive"
    );
}

#[test]
fn every_generated_file_says_it_is_generated() {
    // D2's whole point. A file that does not say so is a file someone will hand-edit.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    for file in &generated.files {
        let name = file.path.to_string_lossy();
        if name.ends_with(".json") || name == ".gitignore" {
            continue; // no comment syntax that is worth the noise
        }
        assert!(
            file.contents.to_lowercase().contains("generated"),
            "{name} must announce that it is generated"
        );
    }
}

#[test]
fn no_connections_are_generated_because_none_can_be() {
    // eve's `defineMcpClientConnection` requires an HTTP url and has no stdio variant,
    // but ai-planner (`aip serve`) and file-sql both speak MCP over stdio. A generated
    // `agent/connections/ai_planner.ts` would be fiction that happens to typecheck.
    // M2-S8 picks the route that actually works; until then the directory does not exist.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    let connections: Vec<_> = generated
        .files
        .iter()
        .filter(|f| f.path.starts_with("agent/connections"))
        .map(|f| f.path.display().to_string())
        .collect();
    assert!(connections.is_empty(), "generated {connections:?}");
}

#[test]
fn a_claude_seat_bridges_only_its_authored_tools_into_the_subscription() {
    let (mut store, _, team) = seeded();
    let backend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "backend")
        .unwrap();
    store
        .set_agent_model(backend.id, Provider::Claude, "sonnet")
        .unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let agent = &generated
        .file("agent/subagents/backend/agent.ts")
        .unwrap()
        .contents;
    assert!(
        agent.contains("claudeCode(\"sonnet\", claudeSettings)"),
        "{agent}"
    );
    assert!(
        agent.contains("modelContextWindowTokens: 200000"),
        "{agent}"
    );
    assert!(
        agent.contains("createAiSdkMcpServer(\"eve\", bridgedTools)"),
        "{agent}"
    );
    assert!(agent.contains("settingSources: []"), "{agent}");
    assert!(agent.contains("tools: []"), "{agent}");
    assert!(agent.contains("canUseTool:"), "{agent}");
    assert!(agent.contains("behavior: \"deny\""), "{agent}");
    for tool in ["bash", "read_file", "write_file", "edit_file"] {
        assert!(agent.contains(&format!("mcp__eve__{tool}")), "{agent}");
        assert!(agent.contains(&format!("bridgedTool as {tool}")), "{agent}");
    }
    assert!(!agent.contains("ANTHROPIC_API_KEY"), "{agent}");

    let package = &generated.file("package.json").unwrap().contents;
    assert!(package.contains("ai-sdk-provider-claude-code"), "{package}");
    assert!(!generated.required_env.contains(&"ANTHROPIC_API_KEY"));
}

#[test]
fn a_read_only_claude_seat_cannot_recover_write_tools_through_the_bridge() {
    let (mut store, _, team) = seeded();
    let reviewer = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "reviewer")
        .unwrap();
    store
        .set_agent_model(reviewer.id, Provider::Claude, "sonnet")
        .unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let agent = &generated
        .file("agent/subagents/reviewer/agent.ts")
        .unwrap()
        .contents;
    for tool in ["bash", "read_file"] {
        assert!(agent.contains(&format!("mcp__eve__{tool}")), "{agent}");
    }
    for tool in ["write_file", "edit_file"] {
        assert!(!agent.contains(&format!("mcp__eve__{tool}")), "{agent}");
        assert!(
            !agent.contains(&format!("bridgedTool as {tool}")),
            "{agent}"
        );
    }
}

#[test]
fn a_team_without_claude_does_not_install_the_bridge_provider() {
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let package = &generated.file("package.json").unwrap().contents;
    assert!(
        !package.contains("ai-sdk-provider-claude-code"),
        "{package}"
    );
}

#[test]
fn openai_uses_eves_chatgpt_subscription_helper() {
    let (mut store, _, team) = seeded();
    let orchestrator = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "orchestrator")
        .unwrap();
    store
        .set_agent_model(orchestrator.id, Provider::OpenAi, "gpt-5.6-luna-fast")
        .unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let root = &generated.file("agent/agent.ts").unwrap().contents;
    assert!(root.contains("import { chatgpt } from \"eve/models/openai\""));
    assert!(root.contains("chatgpt(\"gpt-5.6-luna-fast\")"));
    assert!(
        !root.contains("openai("),
        "the metered helper must never be emitted"
    );
    assert!(!generated.required_env.contains(&"OPENAI_API_KEY"));
}

#[test]
fn machine_generation_applies_the_same_visible_fallback_as_dispatch() {
    let (mut store, _, team) = seeded();
    let backend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "backend")
        .unwrap();
    store
        .set_agent_model(backend.id, Provider::ZAi, "glm-4.6")
        .unwrap();

    let generated = store
        .generate_project_for_machine(team, "/tmp/unused", &ModelRegistry::local_only())
        .unwrap();
    let resolution = generated
        .resolutions
        .iter()
        .find(|resolution| resolution.role == "backend")
        .unwrap();
    assert_eq!(resolution.provider, Provider::Local);
    assert_eq!(resolution.model, "auto");
    assert!(resolution.fell_back());

    let backend_ts = &generated
        .file("agent/subagents/backend/agent.ts")
        .unwrap()
        .contents;
    assert!(backend_ts.contains(")(\"auto\")"), "{backend_ts}");
    assert!(!backend_ts.contains("api.z.ai"), "{backend_ts}");
}

#[test]
fn no_generated_file_ever_mentions_a_metered_api_key() {
    // D8, asserted against the actual output rather than against intent.
    let (mut store, _, team) = seeded();
    let backend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|a| a.role == "backend")
        .unwrap();
    store
        .set_agent_model(backend.id, Provider::ZAi, "glm-4.6")
        .unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    for file in &generated.files {
        for forbidden in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "AI_GATEWAY_API_KEY"] {
            assert!(
                !file.contents.contains(forbidden),
                "{} names {forbidden}",
                file.path.display()
            );
        }
    }
    // The two the process always needs, plus one key per provider in the roster.
    assert_eq!(
        generated.required_env,
        [
            "AI_TEAM_AILOCAL_KEY",
            "AI_TEAM_EVE_TOKEN",
            "AI_TEAM_WORKTREE",
            "AI_TEAM_ZAI_KEY"
        ]
    );
}

#[test]
fn a_disabled_seat_is_not_generated() {
    let (mut store, _, team) = seeded();
    let frontend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|a| a.role == "frontend")
        .unwrap();
    store.set_agent_enabled(frontend.id, false).unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    assert!(generated
        .file("agent/subagents/frontend/agent.ts")
        .is_none());
    assert!(generated.file("agent/subagents/backend/agent.ts").is_some());
    // And the roster in the root's instructions no longer advertises it.
    let instructions = &generated.file("agent/instructions.md").unwrap().contents;
    assert!(!instructions.contains("**frontend**"));
}

#[test]
fn a_team_with_no_orchestrator_refuses_rather_than_writing_a_rootless_project() {
    let (mut store, project, _) = seeded();
    let template = store
        .create_team(Some(project), "No root", Guardrails::default())
        .unwrap();
    store
        .add_agent(
            template.id,
            ai_team_core::preset("backend").unwrap().to_new_agent(0),
        )
        .unwrap();

    let err = store
        .generate_project(template.id, "/tmp/unused")
        .unwrap_err()
        .to_string();
    assert!(err.contains(ROOT_ROLE), "{err}");
}

#[test]
fn the_instructions_tell_each_seat_what_it_owns() {
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    let backend = &generated
        .file("agent/subagents/backend/instructions.md")
        .unwrap()
        .contents;
    assert!(
        backend.contains("crates/**"),
        "the zone must reach the prompt"
    );
    assert!(!backend.contains("You do not write"));

    let verifier = &generated
        .file("agent/subagents/verifier/instructions.md")
        .unwrap()
        .contents;
    assert!(verifier.contains("You do not write"));

    // The root is told the whole roster, because dispatching is its job.
    let root = &generated.file("agent/instructions.md").unwrap().contents;
    for role in ["backend", "frontend", "verifier", "reviewer"] {
        assert!(
            root.contains(&format!("**{role}**")),
            "root must know about {role}"
        );
    }
}

#[test]
fn writing_replaces_the_tree_but_keeps_what_npm_and_eve_own() {
    // Regenerating must not cost a reinstall, and must not leave a stale seat behind.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("agents");
    let (mut store, _, team) = seeded();

    store
        .generate_project(team, &root)
        .unwrap()
        .write()
        .unwrap();
    assert!(root.join("agent/subagents/frontend/agent.ts").exists());

    // Expensive, not generated, must survive.
    std::fs::create_dir_all(root.join("node_modules/eve")).unwrap();
    std::fs::write(root.join("node_modules/eve/index.js"), "// installed").unwrap();
    std::fs::create_dir_all(root.join(".eve")).unwrap();
    std::fs::write(root.join(".eve/state"), "durable").unwrap();
    // Generated, and about to become stale.
    std::fs::write(root.join("agent/tools/leftover.ts"), "// hand-added").unwrap();

    let frontend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|a| a.role == "frontend")
        .unwrap();
    store.set_agent_enabled(frontend.id, false).unwrap();
    store
        .generate_project(team, &root)
        .unwrap()
        .write()
        .unwrap();

    assert!(
        root.join("node_modules/eve/index.js").exists(),
        "npm's work survives"
    );
    assert!(
        root.join(".eve/state").exists(),
        "eve's durable state survives"
    );
    assert!(
        !root.join("agent/tools/leftover.ts").exists(),
        "stale files do not"
    );
    assert!(
        !root.join("agent/subagents/frontend").exists(),
        "a seat removed from the team is removed from the project"
    );
}

#[test]
fn generation_is_deterministic() {
    // The same rows must produce byte-identical output, or every regeneration looks
    // like a change and nobody reads the diff.
    let (store, _, team) = seeded();
    let first = store.generate_project(team, "/tmp/unused").unwrap();
    let second = store.generate_project(team, "/tmp/unused").unwrap();

    assert_eq!(first.files.len(), second.files.len());
    for (a, b) in first.files.iter().zip(second.files.iter()) {
        assert_eq!(a.path, b.path);
        assert_eq!(
            a.contents,
            b.contents,
            "{} differs between runs",
            a.path.display()
        );
    }
}

#[test]
fn tool_policy_rows_do_not_silently_widen_a_read_only_seat() {
    // A policy that allowed `write_file` on a reviewer would contradict the generated
    // project, which disables the slot outright. The generated tree wins, and this
    // records that it does.
    let (mut store, _, team) = seeded();
    let reviewer = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|a| a.role == "reviewer")
        .unwrap();
    store
        .set_tool_policy(reviewer.id, "write_file", ToolEffect::Allow, Some("oops"))
        .unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    assert!(
        generated
            .file("agent/subagents/reviewer/tools/write_file.ts")
            .is_none(),
        "read_only is stronger than a tool policy"
    );
}
