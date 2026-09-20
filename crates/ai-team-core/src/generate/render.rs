//! Rendering the files.
//!
//! Static assets are `include_str!`d verbatim; only the parts that actually depend on
//! the team - the model expression, the roster, the instructions, which tool slots a
//! seat gets - are built here.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::generate::model::{context_window, model_expression, reasoning_literal};
use crate::generate::{check_roster, GeneratedFile, GeneratedProject, ROOT_ROLE, VERIFIER_ROLE};
use crate::model::{Agent, Team};

const WORKTREE_LIB: &str = include_str!("assets/lib/worktree.ts");
const TOOLS_LIB: &str = include_str!("assets/lib/tools.ts");
const IRREVERSIBLE_LIB: &str = include_str!("assets/lib/irreversible.ts");
const PLAN_LIB: &str = include_str!("assets/lib/plan.ts");
const TOOL_BASH: &str = include_str!("assets/tools/bash.ts");
const TOOL_READ: &str = include_str!("assets/tools/read_file.ts");
const TOOL_WRITE: &str = include_str!("assets/tools/write_file.ts");
const TOOL_EDIT: &str = include_str!("assets/tools/edit_file.ts");
const TOOL_PLAN_ADD: &str = include_str!("assets/tools/plan_add_slice.ts");
const TOOL_PLAN_NOTE: &str = include_str!("assets/tools/plan_note.ts");
const TOOL_PLAN_READ: &str = include_str!("assets/tools/plan_read.ts");
const TOOL_DISABLED_WEB: &str = include_str!("assets/tools/disabled_web.ts");
const CHANNEL_EVE: &str = include_str!("assets/channels/eve.ts");
const TSCONFIG: &str = include_str!("assets/tsconfig.json");
const GITIGNORE: &str = include_str!("assets/gitignore");
const README: &str = include_str!("assets/README.md");

/// Pinned so a regeneration cannot silently move to a new eve. The plan records that
/// these versions line up; bumping them is a deliberate commit.
const EVE_VERSION: &str = "0.58.1";
const AI_SDK_VERSION: &str = "^7.0.105";
const ZOD_VERSION: &str = "^4.1.12";
const OPENAI_COMPATIBLE_VERSION: &str = "^2.0.0";
// Exact because this package owns the Claude Agent SDK version and its permission
// semantics. A silent minor bump here can change what `tools: []` or canUseTool means.
const CLAUDE_CODE_VERSION: &str = "4.3.1";

pub(crate) fn project(
    team: &Team,
    agents: &[Agent],
    root: PathBuf,
    ailocal_base_url: &str,
    resolutions: Vec<crate::ModelResolution>,
    context: &[crate::ContextSource],
) -> Result<GeneratedProject> {
    check_roster(team, agents)?;

    let mut files = Vec::new();
    // Always required: the channel refuses to start without it.
    let mut required_env: Vec<&'static str> = vec!["AI_TEAM_EVE_TOKEN", "AI_TEAM_WORKTREE"];

    // The root agent is the orchestrator; every other seat is a declared subagent.
    let (root_agent, subagents): (Vec<&Agent>, Vec<&Agent>) =
        agents.iter().partition(|a| a.role == ROOT_ROLE);
    let root_agent = root_agent
        .first()
        .copied()
        .expect("check_roster proved one exists");

    files.push(file(
        "package.json",
        package_json(
            team,
            agents
                .iter()
                .any(|agent| agent.provider == crate::Provider::Claude),
        ),
    ));
    files.push(file("tsconfig.json", TSCONFIG.to_string()));
    files.push(file(".gitignore", GITIGNORE.to_string()));
    files.push(file("README.md", README.to_string()));

    // Shared implementations, imported by every tool slot at every level.
    // Channels are root-only, so this is not repeated per subagent.
    files.push(file("agent/channels/eve.ts", CHANNEL_EVE.to_string()));
    files.push(file("agent/lib/worktree.ts", WORKTREE_LIB.to_string()));
    files.push(file("agent/lib/tools.ts", TOOLS_LIB.to_string()));
    files.push(file(
        "agent/lib/irreversible.ts",
        IRREVERSIBLE_LIB.to_string(),
    ));
    files.push(file("agent/lib/plan.ts", PLAN_LIB.to_string()));

    // Connections are per-seat: eve discovers `connections/` under a subagent as well as
    // under the root, so Figma reaches the seat that owns the UI and nobody else.
    for (path, contents) in connection_files("agent", root_agent, context) {
        files.push(file(&path, contents));
    }
    for source in context {
        required_env.push(match source {
            crate::ContextSource::ClickUp => "AI_TEAM_CLICKUP_TOKEN",
            crate::ContextSource::Figma => "AI_TEAM_FIGMA_TOKEN",
        });
    }

    let root_model = model_expression(root_agent, ailocal_base_url);
    required_env.extend(root_model.env.iter().copied());
    files.push(file(
        "agent/agent.ts",
        root_agent_ts(
            root_agent,
            &root_model,
            &subagents,
            !connection_files("agent", root_agent, context).is_empty(),
            context,
        ),
    ));
    files.push(file(
        "agent/instructions.md",
        instructions(team, root_agent, &subagents),
    ));
    files.extend(tool_files(
        Path::new("agent"),
        root_agent,
        1,
        !connection_files("agent", root_agent, context).is_empty(),
    ));

    for agent in &subagents {
        let dir = PathBuf::from("agent/subagents").join(&agent.role);
        let model = model_expression(agent, ailocal_base_url);
        required_env.extend(model.env.iter().copied());

        files.push(file(
            dir.join("agent.ts").to_string_lossy().as_ref(),
            subagent_ts(
                agent,
                &model,
                !connection_files(&dir.to_string_lossy(), agent, context).is_empty(),
                context,
            ),
        ));
        files.push(file(
            dir.join("instructions.md").to_string_lossy().as_ref(),
            subagent_instructions(agent),
        ));
        for (path, contents) in connection_files(&dir.to_string_lossy(), agent, context) {
            files.push(file(&path, contents));
        }
        // A declared subagent inherits none of its parent's authored slots, so each one
        // gets its own tool files. They are thin re-exports, so this costs bytes rather
        // than duplication.
        files.extend(tool_files(
            &dir,
            agent,
            3,
            !connection_files(&dir.to_string_lossy(), agent, context).is_empty(),
        ));
    }

    required_env.sort_unstable();
    required_env.dedup();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(GeneratedProject {
        root,
        files,
        required_env,
        resolutions,
    })
}

fn file(path: &str, contents: String) -> GeneratedFile {
    GeneratedFile {
        path: PathBuf::from(path),
        contents,
    }
}

/// The tool slots for one seat.
///
/// `depth` is how far the agent directory sits below `agent/`, so the re-exports can
/// reach `agent/lib/`: the root is 1 (`agent/tools/x.ts` -> `../lib`), a subagent is 3
/// (`agent/subagents/<role>/tools/x.ts` -> `../../../lib`).
///
/// Nothing is emitted to *disable* anything. `defaultTools: false` on the agent already
/// removes eve's whole optional default set, and `disableTool()` at a slot with no
/// framework default underneath it is a build error - which is what makes an absent file
/// the right way to withhold a tool rather than an explicit disable.
fn tool_files(dir: &Path, agent: &Agent, depth: usize, has_connection: bool) -> Vec<GeneratedFile> {
    let tools = dir.join("tools");
    let up = "../".repeat(depth);
    agent_tools(agent, has_connection)
        .into_iter()
        .map(|(name, asset)| GeneratedFile {
            path: tools.join(format!("{name}.ts")),
            contents: asset.replace("../lib/", &format!("{up}lib/")),
        })
        .collect()
}

/// Every seat reads and runs commands: a verifier that cannot run the project's gates
/// cannot verify anything. A read-only seat gets no source-editing definitions at all.
/// Keep this one list for both eve's discovery files and Claude's explicit MCP bridge so
/// a tool can never exist on one route but not the other.
fn agent_tools(agent: &Agent, has_connection: bool) -> Vec<(&'static str, &'static str)> {
    let mut tools = vec![("bash", TOOL_BASH), ("read_file", TOOL_READ)];
    if !agent.read_only {
        tools.extend([("write_file", TOOL_WRITE), ("edit_file", TOOL_EDIT)]);
    }
    if plans(agent) {
        tools.extend([
            ("plan_add_slice", TOOL_PLAN_ADD),
            ("plan_note", TOOL_PLAN_NOTE),
            ("plan_read", TOOL_PLAN_READ),
        ]);
    } else {
        // A maker still reads the board it is working from; it just does not shape it.
        tools.push(("plan_read", TOOL_PLAN_READ));
    }
    if has_connection {
        // Only meaningful where the framework defaults are on, which is exactly where a
        // connection is - and where `disableTool()` has something underneath it to
        // disable, which is what stops it being a build error.
        tools.extend([
            ("web_fetch", TOOL_DISABLED_WEB),
            ("web_search", TOOL_DISABLED_WEB),
        ]);
    }
    tools
}

/// Which seats may shape the plan.
///
/// The orchestrator and the planner, and nobody else: a maker that can add slices can
/// give itself work, and the board a human reads stops being a plan and becomes a log of
/// whatever the agents felt like doing.
fn plans(agent: &Agent) -> bool {
    agent.role == ROOT_ROLE || agent.role == "planner"
}

fn package_json(team: &Team, includes_claude: bool) -> String {
    let claude = if includes_claude {
        format!("    \"ai-sdk-provider-claude-code\": \"{CLAUDE_CODE_VERSION}\",\n    ")
    } else {
        String::new()
    };
    format!(
        "{{\n  \
         \"name\": \"ai-team-{}\",\n  \
         \"private\": true,\n  \
         \"type\": \"module\",\n  \
         \"description\": \"Generated by ai-team from the {} team. Do not edit.\",\n  \
         \"engines\": {{ \"node\": \">=24\" }},\n  \
         \"scripts\": {{\n    \
         \"build\": \"eve build\",\n    \
         \"start\": \"eve start\",\n    \
         \"typecheck\": \"tsc --noEmit\"\n  \
         }},\n  \
         \"dependencies\": {{\n    \
         \"@ai-sdk/openai-compatible\": \"{OPENAI_COMPATIBLE_VERSION}\",\n    \
         {claude}\"ai\": \"{AI_SDK_VERSION}\",\n    \
         \"eve\": \"{EVE_VERSION}\",\n    \
         \"zod\": \"{ZOD_VERSION}\"\n  \
         }},\n  \
         \"devDependencies\": {{\n    \
         \"@types/node\": \"^24.0.0\",\n    \
         \"typescript\": \"^5.9.0\"\n  \
         }}\n\
         }}\n",
        team.slug, team.name,
    )
}

fn model_setup(
    agent: &Agent,
    model: &crate::generate::ModelExpression,
    context: &[crate::ContextSource],
) -> (String, String) {
    let mut imports = vec!["import { defineAgent } from \"eve\";".to_string()];
    imports.extend(model.imports.iter().cloned());
    if !model.bridge_tools {
        return (imports.join("\n"), String::new());
    }

    // `false`: the bridge wants the tools this seat actually implements. The disable
    // sentinels a connection-bearing seat also gets export `disableTool()` rather than a
    // `bridgedTool`, and importing one would not compile.
    let tool_names: Vec<_> = agent_tools(agent, false)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    imports.extend(
        tool_names
            .iter()
            .map(|name| format!("import {{ bridgedTool as {name} }} from \"./tools/{name}.js\";")),
    );
    let tools = tool_names.join(", ");
    // A bridged seat reaches an MCP server through the Agent SDK's own `mcpServers`,
    // not through eve: the bridge only exposes the tools handed to
    // `createAiSdkMcpServer`, and eve's `connection_search` is a framework tool with no
    // importable `execute`. So the connection eve registers is invisible here, and the
    // HTTP endpoint is given to Claude Code directly instead.
    let sources: Vec<crate::ContextSource> = context
        .iter()
        .copied()
        .filter(|source| match source {
            crate::ContextSource::ClickUp => plans(agent),
            crate::ContextSource::Figma => owns_design(agent),
        })
        .collect();

    let mut allowed: Vec<String> = tool_names
        .iter()
        .map(|name| format!("  \"mcp__eve__{name}\","))
        .collect();
    for source in &sources {
        let read_tools = match source {
            crate::ContextSource::ClickUp => crate::CLICKUP_READ_TOOLS,
            crate::ContextSource::Figma => crate::FIGMA_READ_TOOLS,
        };
        for tool in read_tools {
            allowed.push(format!("  \"mcp__{}__{tool}\",", source.as_str()));
        }
    }
    let allowed = allowed.join("\n");

    let context_servers = sources
        .iter()
        .map(|source| {
            let env = format!("AI_TEAM_{}_TOKEN", source.as_str().to_uppercase());
            format!(
                "    {}: {{\n      type: \"http\" as const,\n      url: {:?},\n      \
                 headers: {{ Authorization: `Bearer ${{process.env.{env} ?? \"\"}}` }},\n    }},",
                source.as_str(),
                source.url()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let context_servers = if context_servers.is_empty() {
        String::new()
    } else {
        format!("\n{context_servers}")
    };
    // The namespaces that already carry their own allow-list and must not be widened by
    // the project-tool rule below: `eve` is the bridge, and every context source is
    // read-only by explicit name (D15). ClickUp and Figma both expose writes, so a blanket
    // "any MCP tool" would hand over `clickup_create_task`.
    let governed: Vec<String> = std::iter::once("eve".to_string())
        .chain(sources.iter().map(|source| source.as_str().to_string()))
        .map(|name| format!("{name:?}"))
        .collect();
    let governed = governed.join(", ");

    let setup = format!(
        "// Claude Code owns an inner agent loop, so eve's normal AI SDK tool path does\n\
         // not reach it. Bridge only this seat's authored worktree tools in-process.\n\
         const bridgedTools = {{ {tools} }};\n\
         const allowedClaudeTools = new Set([\n{allowed}\n]);\n\
         \n\
         // Namespaces that carry their own allow-list, so the rule below must not widen\n\
         // them: `eve` is the bridge above, and each context source is read-only by\n\
         // explicit name (D15) - both ClickUp and Figma expose writes.\n\
         const governedServers = new Set([{governed}]);\n\
         \n\
         // A tool the repository itself declares (D19). `Skill` is how a repo's own\n\
         // skills are invoked; `mcp__<server>__*` is a server from its .mcp.json.\n\
         const isProjectTool = (name: string) => {{\n  \
         if (name === \"Skill\") return true;\n  \
         const parts = name.split(\"__\");\n  \
         if (parts.length < 3 || parts[0] !== \"mcp\") return false;\n  \
         return !governedServers.has(parts[1]);\n\
         }};\n\
         \n\
         // The lease this seat is bound to (D10). Everything below is rooted here.\n\
         const worktree = process.env.AI_TEAM_WORKTREE;\n\
         if (!worktree) {{\n  \
         throw new Error(\"AI_TEAM_WORKTREE is not set - ai-team sets it per lease\");\n\
         }}\n\
         \n\
         const claudeSettings = {{\n  \
         mcpServers: {{\n    eve: createAiSdkMcpServer(\"eve\", bridgedTools),{context_servers}\n  }},\n  \
         // The leased worktree, and the reason everything else here works: project\n  \
         // settings, .mcp.json, skills and CLAUDE.md are all discovered relative to\n  \
         // cwd. Without it they resolved against the generated project directory,\n  \
         // where none of them exist - so none of it applied even in principle.\n  \
         cwd: worktree,\n  \
         // `project` only (D19). That is the repository's own declared tooling:\n  \
         // .claude/settings.json, .mcp.json, its skills, and every CLAUDE.md in it -\n  \
         // checked in, reviewed, and already running in the operator's own harness.\n  \
         // Never `user` or `local`: the human's home config staying out is what D7 was\n  \
         // actually protecting, and this does not reopen it.\n  \
         settingSources: [\"project\"],\n  \
         // Every skill the repository ships. A repo whose AGENTS.md refers to a skill\n  \
         // the agent cannot load is a repo the agent cannot follow.\n  \
         skills: \"all\",\n  \
         // The base set of built-in tools, and `Skill` is one of them - an empty array\n  \
         // here is what silently left a repository's skills unreachable even with\n  \
         // `skills: \"all\"` set. Naming it explicitly keeps Claude Code's host\n  \
         // Bash/Read/Write/Edit disabled: the MCP tools above stay the only route to\n  \
         // the leased worktree (D3).\n  \
         tools: [\"Skill\"],\n  \
         // The approval boundary. The generated tools are pre-approved above, and so\n  \
         // are the tools the repository itself declares - an MCP server in .mcp.json is\n  \
         // the operator asking for that capability in that repo (D19). Everything else\n  \
         // still fails closed.\n  \
         //\n  \
         // The boundary that matters is not this list, it is the lease: one process per\n  \
         // worktree, and `irreversible.ts` still refusing to publish.\n  \
         canUseTool: async (toolName: string) =>\n    \
         allowedClaudeTools.has(toolName) || isProjectTool(toolName)\n      \
         ? {{ behavior: \"allow\" as const }}\n      \
         : {{\n          behavior: \"deny\" as const,\n          message: `Tool ${{toolName}} is not enabled for this ai-team seat`,\n        }},\n\
         }};\n"
    );
    (imports.join("\n"), setup)
}

fn root_agent_ts(
    agent: &Agent,
    model: &crate::generate::ModelExpression,
    subagents: &[&Agent],
    has_connection: bool,
    context: &[crate::ContextSource],
) -> String {
    let (imports, model_setup) = model_setup(agent, model, context);

    let roster = subagents
        .iter()
        .map(|a| format!("//   {:<14} {}", a.role, one_line(&a.purpose)))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "// GENERATED by ai-team. Do not edit: change the team rows and regenerate.\n\
         //\n\
         // The root agent: {role}.\n\
         //\n\
         // Its subagents, one per seat on the team:\n\
         {roster}\n\n\
         {imports}\n\n\
         {model_setup}\
         export default defineAgent({{\n  \
         model: {model_expr},\n  \
         reasoning: {reasoning:?},\n  \
         // eve sizes a model's context window from AI Gateway metadata, which a direct\n  \
         // openai-compatible model has none of - without this it refuses to compile\n  \
         // compaction at all.\n  \
         modelContextWindowTokens: {context},\n  \
         {defaults}\n\
         }});\n",
        role = agent.role,
        roster = roster,
        imports = imports,
        model_setup = model_setup,
        model_expr = model.expression,
        reasoning = reasoning_literal(agent),
        context = context_window(agent),
        defaults = default_tools_line(has_connection),
    )
}

fn subagent_ts(
    agent: &Agent,
    model: &crate::generate::ModelExpression,
    has_connection: bool,
    context: &[crate::ContextSource],
) -> String {
    let (imports, model_setup) = model_setup(agent, model, context);

    format!(
        "// GENERATED by ai-team. Do not edit: change the team rows and regenerate.\n\n\
         {imports}\n\n\
         {model_setup}\
         export default defineAgent({{\n  \
         // Required on a declared subagent: this is what the parent routes on.\n  \
         description: {description:?},\n  \
         model: {model_expr},\n  \
         reasoning: {reasoning:?},\n  \
         modelContextWindowTokens: {context},\n  \
         {defaults}\n\
         }});\n",
        imports = imports,
        model_setup = model_setup,
        defaults = default_tools_line(has_connection),
        description = one_line(&agent.purpose),
        model_expr = model.expression,
        reasoning = reasoning_literal(agent),
        context = context_window(agent),
    )
}

fn instructions(team: &Team, root: &Agent, subagents: &[&Agent]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = write!(
        out,
        "# {role}\n\n\
         <!-- GENERATED by ai-team from the {team} team. Do not edit. -->\n\n\
         {purpose}\n\n",
        role = root.name,
        team = team.name,
        purpose = root.purpose,
    );

    if let Some(prompt) = custom_prompt(root) {
        let _ = write!(out, "## Custom instructions\n\n{prompt}\n\n");
    }

    out.push_str(
        "## Where you are\n\n\
         You are working in a git checkout. Your tools act on it and cannot reach outside \
         it. It is real: build it, test it, and run the project's own checks in it.\n\n\
         ## How the work gets done\n\n\
         You do not build the slices yourself and you do not dispatch anybody. Turn the \
         request into a plan, and ai-team gives each ready slice its own worktree and the \
         seat whose zone owns it.\n\n\
         Read the plan first with `plan_read`. It is a board that outlives this \
         conversation, so add only what is genuinely missing - a slice that repeats one \
         already there gives somebody the same work twice.\n\n\
         Every slice you add must name the paths it touches. That is what routes it: a \
         slice naming no path this team owns cannot be given to anybody, and is reported \
         back to the human undone. Keep each slice small enough to demo on its own.\n\n",
    );

    out.push_str("## Your team\n\n");
    for agent in subagents {
        let zone = if agent.zone.trim().is_empty() {
            "no owned paths".to_string()
        } else {
            agent
                .zone
                .lines()
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            out,
            "- **{}** ({}) - {} Owns: {}.",
            agent.role,
            if agent.read_only {
                "reads only"
            } else {
                "writes"
            },
            one_line(&agent.purpose),
            zone,
        );
    }

    out.push_str(
        "\nDelegate by zone: the seat that owns a path is the seat that changes it. Two \
         agents editing one file is the failure this roster exists to prevent.\n\n\
         ## How you finish\n\n\
         Work is done when the project's own checks pass, not when the code looks right. \
         If you cannot run them, say so rather than claiming success.\n",
    );
    out
}

fn subagent_instructions(agent: &Agent) -> String {
    let zone = if agent.zone.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n## Your zone\n\nYou own these paths:\n\n{}\n\nWork outside them belongs to \
             another seat - say so rather than reaching across.\n",
            agent
                .zone
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
                .map(|l| format!("- `{}`", l.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };

    let access = if agent.read_only {
        "\n## You do not write\n\nYour `write_file` and `edit_file` tools are disabled, \
         deliberately. Report what you found and let the seat that owns the code change \
         it. A checker that edits what it is checking is just a second maker.\n"
    } else {
        ""
    };
    let custom = custom_prompt(agent).map_or_else(String::new, |prompt| {
        format!("## Custom instructions\n\n{prompt}\n\n")
    });

    format!(
        "# {name}\n\n\
         <!-- GENERATED by ai-team. Do not edit. -->\n\n\
         {purpose}\n\n\
         {custom}\
         ## Where you are\n\n\
         You are in a git worktree leased for this run. Your tools act on it and cannot \
         reach outside it.\n\
         {zone}{access}{verifying}",
        name = agent.name,
        purpose = agent.purpose,
        custom = custom,
        verifying = if agent.role == VERIFIER_ROLE {
            VERIFIER_INSTRUCTIONS
        } else {
            ""
        },
    )
}

/// What the verifier is actually looking for.
///
/// The gates have already run by the time it reads a diff, so "the tests pass" is not
/// news and is not what it is for. The three questions below are the ones a green test
/// run does not answer, and they are the failures that actually get shipped: a function
/// that exists but returns a constant, or one that is perfect and never called.
const VERIFIER_INSTRUCTIONS: &str = "\n## What you are checking\n\n\
     The project's own gates have already been run and their output is in the request. \
     Do not re-run them to decide; read them. Your job is the three things a passing \
     test run does not tell anybody:\n\n\
     1. **Existence** - is the thing that was asked for actually there? Read the files. A \
     summary saying it was added is not evidence that it was.\n\
     2. **Substantive** - does it do the work, or does it only look like it? A function \
     that returns a constant, a branch that swallows the error, a test asserting \
     `true == true`, a TODO where the logic should be.\n\
     3. **Wired** - is it reachable? Exported, registered, called, routed. Code nothing \
     refers to is not a feature, however correct it is.\n\n\
     Answer with `VERDICT: pass` or `VERDICT: reject` on its own line. A rejection must \
     say which of the three failed and quote the file and line that shows it, because \
     the maker is given your words and nothing else. If the gates failed, it is a \
     rejection - say which gate.\n\n\
     Reject work that is not done. Passing something through because it is close is how \
     a stub reaches the human who trusted this ran.\n";

/// What to say about eve's own tool set.
///
/// Off by default: this agent acts on a leased worktree, not eve's sandbox (D3), and the
/// authored tools replace the ones the model would otherwise expect.
///
/// A seat with a connection needs them back, because `connection_search` - the only way
/// a model can reach a connection at all - is one of them, and the flag is all-or-nothing.
/// The authored `bash`, `read_file` and `write_file` still win at their own slots, so the
/// worktree boundary is unchanged; `web_fetch` and `web_search` are disabled separately,
/// since a page fetched off the internet is untrusted text landing in the context.
fn default_tools_line(has_connection: bool) -> &'static str {
    if has_connection {
        "// On because `connection_search` is a framework default and it is the only\n  \
         // route to this seat's connections. The authored tools still override the\n  \
         // sandbox ones at their own slots, and web_fetch/web_search are disabled.\n  \
         defaultTools: true,"
    } else {
        "// eve's optional defaults are off: this agent acts on a leased worktree, not\n  \
         // on eve's sandbox (D3), and the tools under `tools/` replace the ones the\n  \
         // model would otherwise expect.\n  \
         defaultTools: false,"
    }
}

/// Which context sources this seat gets, and where their files go.
///
/// Scoped by what the seat is for, not handed to everybody. The orchestrator and the
/// planner read the ticket because they decide what the work is; the seat that owns the
/// UI reads the designs because it is the one building them. A backend seat needs
/// neither, and a connection it never calls is prompt it pays for on every turn.
fn connection_files(
    dir: &str,
    agent: &Agent,
    context: &[crate::ContextSource],
) -> Vec<(String, String)> {
    let plans = plans(agent);
    let designs = owns_design(agent);

    context
        .iter()
        .filter(|source| match source {
            crate::ContextSource::ClickUp => plans,
            crate::ContextSource::Figma => designs,
        })
        .map(|source| {
            let tools = match source {
                crate::ContextSource::ClickUp => crate::CLICKUP_READ_TOOLS,
                crate::ContextSource::Figma => crate::FIGMA_READ_TOOLS,
            };
            (
                format!("{dir}/connections/{}.ts", source.as_str()),
                connection_ts(*source, tools),
            )
        })
        .collect()
}

/// Does this seat own the look of the thing?
///
/// Read off its zone rather than its role name, because a team is configurable: the seat
/// that owns `ui/**` is the one that needs the designs, whatever it is called.
fn owns_design(agent: &Agent) -> bool {
    const DESIGN_PATHS: &[&str] = &[
        "ui/",
        "web/",
        "frontend/",
        "app/",
        "src/components",
        ".css",
        ".tsx",
        ".vue",
        ".svelte",
    ];
    agent.zone.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && DESIGN_PATHS.iter().any(|hint| line.contains(hint))
    })
}

/// A read-only MCP connection to a context source (D9).
///
/// ClickUp and Figma are the two neighbours that *can* be eve connections:
/// `defineMcpClientConnection` needs an HTTP url, and unlike `aip serve` and file-sql
/// both of these are hosted over HTTP.
///
/// The allow-list is the whole point. Both servers expose write tools - ClickUp creates
/// and deletes tasks, and Figma's `use_figma` creates, edits and deletes - so read-only
/// is enforced by naming what may be called rather than by asking the model nicely. An
/// allow-list also fails in the safe direction: a name that is wrong loses a capability,
/// where a block-list that misses one hands over a write.
fn connection_ts(source: crate::ContextSource, tools: &[&str]) -> String {
    let allow = tools
        .iter()
        .map(|tool| format!("    {tool:?},"))
        .collect::<Vec<_>>()
        .join("\n");
    let env = format!("AI_TEAM_{}_TOKEN", source.as_str().to_uppercase());

    format!(
        "// GENERATED by ai-team. Do not edit: change the team rows and regenerate.\n\
         //\n\
         // {name} as a read-only context source (D9). ai-team never writes back: the\n\
         // human answers on {name}, and an agent acting for them there does it somewhere\n\
         // they cannot see it happen.\n\n\
         import {{ defineMcpClientConnection }} from \"eve/connections\";\n\n\
         // Only these. Every write tool the server offers is absent on purpose, and an\n\
         // allow-list fails closed if one of these names is wrong.\n\
         const READ_ONLY = [\n{allow}\n];\n\n\
         export default defineMcpClientConnection({{\n  \
         url: {url:?},\n  \
         description:\n    \
         \"Read-only {name} access: look up the work this run is for, and the designs it \\\n     \
         points at. Cannot create, change or delete anything.\",\n  \
         tools: {{ allow: READ_ONLY }},\n  \
         auth: {{\n    \
         // Read per call, not at module scope: `eve build` evaluates every authored\n    \
         // module, so a throw up here would fail the build on any machine that is not\n    \
         // running an agent. Missing means no token, which fails closed.\n    \
         getToken: async () => {{\n      \
         const token = process.env.{env};\n      \
         if (!token) {{\n        \
         throw new Error(\n          \
         \"{env} is not set. ai-team does not do the {name} OAuth dance itself: \\\n           \
         authorise {name} in your MCP client and put the resulting token in that \\\n           \
         variable.\",\n        \
         );\n      \
         }}\n      \
         return {{ token }};\n    \
         }},\n  \
         }},\n\
         }});\n",
        name = match source {
            crate::ContextSource::ClickUp => "ClickUp",
            crate::ContextSource::Figma => "Figma",
        },
        url = source.url(),
        allow = allow,
        env = env,
    )
}

/// A seat's custom prompt, unless it only repeats the purpose printed above it.
///
/// A seat created without a preset falls back to its purpose so it is never left with no
/// instructions at all, and rendering that twice wastes the model's attention on the
/// second copy.
fn custom_prompt(agent: &Agent) -> Option<&str> {
    agent
        .prompt_md
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty() && *prompt != agent.purpose.trim())
}

/// Collapse prose to one line. A newline inside a TypeScript string literal or a table
/// cell breaks the thing that renders it.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
