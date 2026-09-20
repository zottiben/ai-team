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
fn every_seat_gets_the_draft_commit_gate() {
    // The gate lives in one module that `bash` calls, so no seat can be generated with
    // a shell that publishes. Every seat has `bash`, including the read-only ones.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    assert!(generated.file("agent/lib/irreversible.ts").is_some());
    let tools = &generated.file("agent/lib/tools.ts").unwrap().contents;
    assert!(
        tools.contains("judge(command)"),
        "bash must ask before running"
    );
    assert!(tools.contains("irreversible.js"), "{tools}");

    for dir in [
        "agent",
        "agent/subagents/backend",
        "agent/subagents/verifier",
    ] {
        let bash = generated
            .file(&format!("{dir}/tools/bash.ts"))
            .unwrap_or_else(|| panic!("{dir} has a shell"));
        assert!(bash.contents.contains("lib/tools.js"), "{dir}");
    }
}

#[test]
fn only_the_seats_that_plan_can_shape_the_plan() {
    // A maker that can add slices can give itself work, and the board a human reads
    // stops being a plan. Everyone reads it; two seats write it.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    for (dir, writes_plan) in [
        ("agent".to_string(), true),
        ("agent/subagents/planner".to_string(), true),
        ("agent/subagents/backend".to_string(), false),
        ("agent/subagents/frontend".to_string(), false),
        ("agent/subagents/verifier".to_string(), false),
        ("agent/subagents/reviewer".to_string(), false),
    ] {
        let reads = generated.file(&format!("{dir}/tools/plan_read.ts"));
        assert!(reads.is_some(), "{dir} must be able to read the board");

        for slot in ["plan_add_slice", "plan_note"] {
            let path = format!("{dir}/tools/{slot}.ts");
            assert_eq!(
                generated.file(&path).is_some(),
                writes_plan,
                "{path} present should be {writes_plan}"
            );
        }
    }

    // The shared implementation is emitted once, and the slots re-export from it.
    assert!(generated.file("agent/lib/plan.ts").is_some());
    let root = &generated
        .file("agent/tools/plan_add_slice.ts")
        .unwrap()
        .contents;
    assert!(root.contains(r#"from "../lib/plan.js""#), "{root}");
    let sub = &generated
        .file("agent/subagents/planner/tools/plan_add_slice.ts")
        .unwrap()
        .contents;
    assert!(sub.contains(r#"from "../../../lib/plan.js""#), "{sub}");
}

#[test]
fn the_orchestrator_is_told_it_plans_rather_than_builds() {
    // D14: it writes the plan and ai-team dispatches. An orchestrator that thinks it
    // should build the slices itself does the work in the wrong worktree.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let instructions = &generated.file("agent/instructions.md").unwrap().contents;

    assert!(instructions.contains("You do not build the slices yourself"));
    assert!(
        instructions.contains("name the paths it touches"),
        "a slice with no paths cannot be routed to a seat"
    );
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
fn context_sources_reach_only_the_seats_that_need_them() {
    use ai_team_core::ContextSource;

    let (store, _, team) = seeded();
    let generated = store
        .generate_project_with_context(
            team,
            "/tmp/unused",
            &[ContextSource::ClickUp, ContextSource::Figma],
        )
        .unwrap();

    // The ticket goes to the seats that decide what the work is.
    assert!(generated.file("agent/connections/clickup.ts").is_some());
    assert!(generated
        .file("agent/subagents/planner/connections/clickup.ts")
        .is_some());

    // The designs go to the seat that owns the UI, read off its zone rather than its
    // name - a team is configurable, and `ui/**` is what actually identifies it.
    assert!(generated
        .file("agent/subagents/frontend/connections/figma.ts")
        .is_some());
    for role in ["backend", "verifier", "reviewer", "planner"] {
        assert!(
            generated
                .file(&format!("agent/subagents/{role}/connections/figma.ts"))
                .is_none(),
            "{role} does not own the look of the thing"
        );
    }
    // A maker that is not the frontend has no business reading the ticket board either.
    assert!(generated
        .file("agent/subagents/backend/connections/clickup.ts")
        .is_none());

    assert!(generated.required_env.contains(&"AI_TEAM_CLICKUP_TOKEN"));
    assert!(generated.required_env.contains(&"AI_TEAM_FIGMA_TOKEN"));
}

#[test]
fn a_context_connection_can_only_read() {
    use ai_team_core::ContextSource;

    // D9 is enforced by naming what may be called. Both servers grew write tools, and
    // `use_figma` is the one that reads like a read while creating, editing and deleting.
    let (store, _, team) = seeded();
    let generated = store
        .generate_project_with_context(
            team,
            "/tmp/unused",
            &[ContextSource::ClickUp, ContextSource::Figma],
        )
        .unwrap();

    let clickup = &generated
        .file("agent/connections/clickup.ts")
        .unwrap()
        .contents;
    assert!(clickup.contains("tools: { allow: READ_ONLY }"), "{clickup}");
    assert!(clickup.contains("\"get_task\""));
    for write in ["create_task", "update_task", "delete_task"] {
        assert!(!clickup.contains(write), "{write} must not be reachable");
    }

    let figma = &generated
        .file("agent/subagents/frontend/connections/figma.ts")
        .unwrap()
        .contents;
    assert!(figma.contains("\"get_design_context\""));
    for write in [
        "use_figma",
        "generate_figma_design",
        "upload_assets",
        "weave_",
    ] {
        assert!(!figma.contains(write), "{write} must not be reachable");
    }
}

#[test]
fn admitting_the_repository_s_tools_does_not_widen_a_context_source() {
    use ai_team_core::{ContextSource, Provider};

    // D19 admits the tools a repository declares, which are `mcp__<server>__*` - the same
    // shape a context source's tools have. A blanket "any MCP tool" would therefore hand
    // back exactly the writes D15 spent an allow-list refusing: `clickup_create_task` is
    // reachable the moment the rule stops excluding the servers that govern themselves.
    let (mut store, _, team) = seeded();
    for role in ["orchestrator", "frontend"] {
        let seat = store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == role)
            .unwrap();
        store
            .set_agent_model(seat.id, Provider::Claude, "sonnet")
            .unwrap();
    }
    let generated = store
        .generate_project_with_context(
            team,
            "/tmp/unused",
            &[ContextSource::ClickUp, ContextSource::Figma],
        )
        .unwrap();

    // A seat governs the namespaces it actually has, which is the same scoping D15 gives
    // the connections themselves: the ticket reaches the seats that decide what the work
    // is, the designs reach the seat whose zone owns the UI.
    let root = &generated.file("agent/agent.ts").unwrap().contents;
    assert!(root.contains("governedServers"), "{root}");
    assert!(root.contains("\"eve\""), "{root}");
    assert!(root.contains("\"clickup\""), "{root}");

    let frontend = &generated
        .file("agent/subagents/frontend/agent.ts")
        .unwrap()
        .contents;
    assert!(
        frontend.contains("\"figma\""),
        "the seat holding figma must govern it\n{frontend}"
    );
}

#[test]
fn a_machine_that_allows_no_context_source_generates_no_connection() {
    // The default: both are off until the machine profile says otherwise (D9), so a
    // work laptop never reaches an account nobody authorised.
    let (store, _, team) = seeded();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let connections: Vec<_> = generated
        .files
        .iter()
        .filter(|file| file.path.to_string_lossy().contains("/connections/"))
        .collect();
    assert!(connections.is_empty(), "{connections:?}");
}

#[test]
fn no_connections_are_generated_because_none_can_be() {
    // eve's `defineMcpClientConnection` requires an HTTP url and has no stdio variant,
    // but ai-planner (`aip serve`) and file-sql both speak MCP over stdio. A generated
    // `agent/connections/ai_planner.ts` would be fiction that happens to typecheck.
    // The route that works for those two is a generated tool shelling out to `aip`
    // (agent/lib/plan.ts). ClickUp and Figma are different: both are hosted over HTTP,
    // so they are real connections - see the tests above.
    use ai_team_core::ContextSource;

    let (store, _, team) = seeded();
    let generated = store
        .generate_project_with_context(team, "/tmp/unused", &[ContextSource::ClickUp])
        .unwrap();

    let stdio: Vec<_> = generated
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .filter(|path| path.contains("ai_planner") || path.contains("file_sql"))
        .collect();
    assert!(stdio.is_empty(), "generated {stdio:?}");
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
    // The repository's own declared tooling reaches the seat, and the human's does not
    // (D19). `project` alone: `user` or `local` would be the home config D7 excluded.
    assert!(agent.contains("settingSources: [\"project\"]"), "{agent}");
    assert!(!agent.contains("\"user\""), "{agent}");
    assert!(!agent.contains("\"local\""), "{agent}");
    assert!(agent.contains("skills: \"all\""), "{agent}");
    // Project settings are discovered relative to cwd, so without the lease none of it
    // resolves - it would look in the generated project directory instead.
    assert!(agent.contains("cwd: worktree"), "{agent}");
    assert!(agent.contains("process.env.AI_TEAM_WORKTREE"), "{agent}");
    // `Skill` is a built-in tool, so an empty base set leaves a repository's skills
    // unreachable however `skills` is configured. Host Bash/Read/Write/Edit stay off.
    assert!(agent.contains("tools: [\"Skill\"]"), "{agent}");
    assert!(agent.contains("canUseTool:"), "{agent}");
    assert!(agent.contains("behavior: \"deny\""), "{agent}");
    // A seat with no context source still governs its own bridge namespace, or the
    // project-tool rule would re-admit the bridged tools it deliberately withheld.
    assert!(agent.contains("governedServers"), "{agent}");
    assert!(agent.contains("isProjectTool"), "{agent}");
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
fn a_custom_prompt_is_rendered_into_the_seats_instructions() {
    let (mut store, _, team) = seeded();
    let backend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "backend")
        .unwrap();
    let mut config = ai_team_core::NewAgent::from(&backend);
    config.prompt_preset = None;
    config.prompt_md = Some("Preserve wire compatibility, even when the schema changes.".into());
    store.update_agent(backend.id, config).unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let instructions = &generated
        .file("agent/subagents/backend/instructions.md")
        .unwrap()
        .contents;
    assert!(instructions.contains("## Custom instructions"));
    assert!(instructions.contains("Preserve wire compatibility"));
}

#[test]
fn a_fallback_prompt_is_not_printed_twice() {
    // A seat added without a preset gets its purpose as its prompt so it is never left
    // with no instructions. Rendering it under its own heading as well would just spend
    // the model's attention on a second copy of the sentence above it.
    let (mut store, _, team) = seeded();
    let backend = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "backend")
        .unwrap();
    let mut config = ai_team_core::NewAgent::from(&backend);
    config.purpose = "Owns the server.".into();
    config.prompt_preset = None;
    config.prompt_md = Some("Owns the server.".into());
    store.update_agent(backend.id, config).unwrap();

    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let instructions = &generated
        .file("agent/subagents/backend/instructions.md")
        .unwrap()
        .contents;
    assert!(
        !instructions.contains("## Custom instructions"),
        "{instructions}"
    );
    assert_eq!(instructions.matches("Owns the server.").count(), 1);
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

/// A checkout carrying one skill, attached to the project so generation can find it.
fn seeded_with_a_skill() -> (Store, tempfile::TempDir, i64) {
    use ai_team_core::NewRepo;

    let (mut store, project_id, team) = seeded();
    let repo = tempfile::tempdir().unwrap();
    let skill = repo.path().join(".agents/skills/house-style");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: house-style\ndescription: How this repo names things.\n---\nFull words only.",
    )
    .unwrap();
    store
        .attach_repo(
            project_id,
            NewRepo {
                main_path: Some(repo.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    (store, repo, team)
}

#[test]
fn a_repo_skill_is_generated_for_every_seat() {
    // Not scoped by zone the way a connection is: a skill is the repository telling
    // whoever edits it how to do so, and which seat needs it is not knowable from the
    // roster. eve shows the model only the name and description until one is loaded.
    let (store, _repo, team) = seeded_with_a_skill();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    assert!(generated
        .file("agent/skills/house-style/SKILL.md")
        .is_some());
    for role in ["backend", "frontend", "planner", "reviewer", "verifier"] {
        assert!(
            generated
                .file(&format!(
                    "agent/subagents/{role}/skills/house-style/SKILL.md"
                ))
                .is_some(),
            "{role} has no skills"
        );
    }
}

#[test]
fn a_generated_skill_keeps_its_frontmatter_and_says_where_the_real_one_is() {
    // eve reads the name and description out of the frontmatter, so the file passes
    // through unchanged - and the note is needed because the skill's own scripts and
    // references were deliberately left in the repository.
    let (store, _repo, team) = seeded_with_a_skill();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();
    let skill = &generated
        .file("agent/skills/house-style/SKILL.md")
        .unwrap()
        .contents;

    assert!(skill.contains("description: How this repo names things."));
    assert!(skill.contains("Full words only."));
    assert!(skill.contains(".agents/skills/house-style"));
}

#[test]
fn a_repo_with_skills_turns_the_framework_defaults_on() {
    // `load_skill` is a framework default, so `defaultTools: false` leaves a seat shown
    // a list of skills it has no way to open. That was the whole bug.
    let (store, _repo, team) = seeded_with_a_skill();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    let root = &generated.file("agent/agent.ts").unwrap().contents;
    assert!(root.contains("defaultTools: true"), "{root}");
    assert!(root.contains("load_skill"), "{root}");

    let backend = &generated
        .file("agent/subagents/backend/agent.ts")
        .unwrap()
        .contents;
    assert!(backend.contains("defaultTools: true"), "{backend}");
}

#[test]
fn a_read_only_seat_does_not_get_eve_s_write_file_back() {
    // Turning the defaults on restores eve's own `write_file`. It writes to the sandbox
    // rather than the worktree, so it cannot edit source - but a read-only seat holding
    // a tool by that name will use it and report work that never landed.
    let (store, _repo, team) = seeded_with_a_skill();
    let generated = store.generate_project(team, "/tmp/unused").unwrap();

    let disabled = &generated
        .file("agent/subagents/verifier/tools/write_file.ts")
        .unwrap()
        .contents;
    assert!(disabled.contains("disableTool()"), "{disabled}");

    // A seat that writes still gets the real one.
    let backend = &generated
        .file("agent/subagents/backend/tools/write_file.ts")
        .unwrap()
        .contents;
    assert!(!backend.contains("disableTool()"), "{backend}");
}

#[test]
fn the_sandbox_backend_is_installed_only_when_there_are_skills() {
    // eve initialises its sandbox templates as soon as a node has a skill and refuses to
    // serve when that fails, and only `just-bash` needs neither Docker nor Vercel. A repo
    // without skills should not pay 22MB for a sandbox ai-team never uses (D3).
    let (store, _repo, team) = seeded_with_a_skill();
    let with = store.generate_project(team, "/tmp/unused").unwrap();
    assert!(with
        .file("package.json")
        .unwrap()
        .contents
        .contains("just-bash"));

    let (plain, _, plain_team) = seeded();
    let without = plain.generate_project(plain_team, "/tmp/unused").unwrap();
    assert!(!without
        .file("package.json")
        .unwrap()
        .contents
        .contains("just-bash"));
}
