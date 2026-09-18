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

    let root_model = model_expression(root_agent, ailocal_base_url);
    required_env.extend(root_model.env.iter().copied());
    files.push(file(
        "agent/agent.ts",
        root_agent_ts(root_agent, &root_model, &subagents),
    ));
    files.push(file(
        "agent/instructions.md",
        instructions(team, root_agent, &subagents),
    ));
    files.extend(tool_files(Path::new("agent"), root_agent, 1));

    for agent in &subagents {
        let dir = PathBuf::from("agent/subagents").join(&agent.role);
        let model = model_expression(agent, ailocal_base_url);
        required_env.extend(model.env.iter().copied());

        files.push(file(
            dir.join("agent.ts").to_string_lossy().as_ref(),
            subagent_ts(agent, &model),
        ));
        files.push(file(
            dir.join("instructions.md").to_string_lossy().as_ref(),
            subagent_instructions(agent),
        ));
        // A declared subagent inherits none of its parent's authored slots, so each one
        // gets its own tool files. They are thin re-exports, so this costs bytes rather
        // than duplication.
        files.extend(tool_files(&dir, agent, 3));
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
fn tool_files(dir: &Path, agent: &Agent, depth: usize) -> Vec<GeneratedFile> {
    let tools = dir.join("tools");
    let up = "../".repeat(depth);
    agent_tools(agent)
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
fn agent_tools(agent: &Agent) -> Vec<(&'static str, &'static str)> {
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

fn model_setup(agent: &Agent, model: &crate::generate::ModelExpression) -> (String, String) {
    let mut imports = vec!["import { defineAgent } from \"eve\";".to_string()];
    imports.extend(model.imports.iter().cloned());
    if !model.bridge_tools {
        return (imports.join("\n"), String::new());
    }

    let tool_names: Vec<_> = agent_tools(agent)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    imports.extend(
        tool_names
            .iter()
            .map(|name| format!("import {{ bridgedTool as {name} }} from \"./tools/{name}.js\";")),
    );
    let tools = tool_names.join(", ");
    let allowed = tool_names
        .iter()
        .map(|name| format!("  \"mcp__eve__{name}\","))
        .collect::<Vec<_>>()
        .join("\n");
    let setup = format!(
        "// Claude Code owns an inner agent loop, so eve's normal AI SDK tool path does\n\
         // not reach it. Bridge only this seat's authored worktree tools in-process.\n\
         const bridgedTools = {{ {tools} }};\n\
         const allowedClaudeTools = new Set([\n{allowed}\n]);\n\
         const claudeSettings = {{\n  \
         mcpServers: {{ eve: createAiSdkMcpServer(\"eve\", bridgedTools) }},\n  \
         allowedTools: [...allowedClaudeTools],\n  \
         // Disable Claude Code's host Bash/Read/Write/Edit. The MCP tools above are\n  \
         // the only route to the leased worktree.\n  \
         tools: [],\n  \
         // An omitted value inherits the human's CLAUDE.md, settings and MCPs.\n  \
         settingSources: [],\n  \
         // The callback is the Agent SDK approval boundary. Current generated tools\n  \
         // are pre-approved above; anything else still fails closed here.\n  \
         canUseTool: async (toolName: string) =>\n    \
         allowedClaudeTools.has(toolName)\n      \
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
) -> String {
    let (imports, model_setup) = model_setup(agent, model);

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
         // eve's optional sandbox defaults are off: this agent acts on a leased\n  \
         // worktree, not on eve's sandbox (D3). The tools under `tools/` replace the\n  \
         // ones the model would otherwise expect.\n  \
         defaultTools: false,\n\
         }});\n",
        role = agent.role,
        roster = roster,
        imports = imports,
        model_setup = model_setup,
        model_expr = model.expression,
        reasoning = reasoning_literal(agent),
        context = context_window(agent),
    )
}

fn subagent_ts(agent: &Agent, model: &crate::generate::ModelExpression) -> String {
    let (imports, model_setup) = model_setup(agent, model);

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
         defaultTools: false,\n\
         }});\n",
        imports = imports,
        model_setup = model_setup,
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
