//! What a seat is told about itself, as a system prompt.
//!
//! eve took this as a generated `instructions.md` per node. Pi takes it as
//! `--append-system-prompt`, which is the same thing with one fewer file: the text is
//! built from the team rows at dispatch and handed straight to the process.
//!
//! The first run on Pi went wrong exactly here. With no instructions, the orchestrator
//! was a general-purpose coding assistant holding a `bash` tool, so it did what a coding
//! assistant does with "add a subtract function": it added one. It never wrote a plan,
//! nothing was dispatched, and the run failed looking for a plan that did not exist.
//!
//! A seat is not its model and it is not its tools. It is what it has been told it is.

use std::fmt::Write as _;

use crate::model::{Agent, Team};
use crate::roles::ROOT_ROLE;

/// The system prompt for one seat.
///
/// `team` and `roster` are only used by a planning seat - a maker does not need to know
/// who else is on the team, because it is not deciding who does what.
pub(super) fn for_seat(agent: &Agent, team: &Team, roster: &[Agent]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "# {name}\n\nYou are the {role} seat on the {team} team, working through ai-team.\n\n{purpose}\n\n",
        name = agent.name,
        role = agent.role,
        team = team.name,
        purpose = agent.purpose,
    );

    if let Some(custom) = agent
        .prompt_md
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let _ = write!(out, "## Custom instructions\n\n{custom}\n\n");
    }

    out.push_str(
        "## Where you are\n\n\
         Your working directory is a git worktree leased to you alone. It is real: build \
         it, test it, and run the project's own checks in it. Your tools are held to it - \
         reaching outside it is refused, and so is anything that publishes.\n\n\
         Your work is a draft. ai-team commits what you leave behind to a branch and a \
         human decides whether it goes anywhere, so do not push, tag, merge or release. \
         If something genuinely needs one of those, say so in your answer.\n\n",
    );

    if coordinates(agent) {
        coordinating_section(&mut out, roster);
    } else if plans(agent) {
        planning_section(&mut out, roster);
    } else {
        building_section(&mut out, agent);
    }

    if coordinates(agent) || plans(agent) {
        out.push_str(
            "## When you are done\n\n\
             State the grounded brief or plan you produced and any context you could not \
             verify. Do not build the work yourself.\n",
        );
    } else {
        out.push_str(
            "## When you are done\n\n\
             Say what you changed and what you ran, in plain prose. A turn that reports done \
             but changed no file is recorded as failed, so if you could not do the work, say \
             that instead - it is a useful answer and a false one is not.\n",
        );
    }
    out
}

/// Which seats may shape the plan.
///
/// Only the planner shapes the board. The orchestrator coordinates the graph and hands
/// the planner a grounded brief; combining those jobs is how the planner seat went unused.
fn plans(agent: &Agent) -> bool {
    agent.role == "planner"
}

fn coordinates(agent: &Agent) -> bool {
    agent.role == ROOT_ROLE
}

fn coordinating_section(out: &mut String, roster: &[Agent]) {
    out.push_str(
        "## How the work gets done\n\n\
         **You coordinate; you do not build code and you do not shape the ai-planner \
         board.** Ground the operator's request in the repository and in every required \
         ClickUp or Figma source. Produce a concise delegation brief for the planner: \
         outcome, constraints, relevant paths, acceptance evidence, and unresolved \
         questions. ai-team passes that brief to the planner seat and Rust later leases \
         and dispatches the approved slices.\n\n\
         Required external context is mandatory. If its MCP tools are unavailable, say \
         exactly which source could not be read and stop; never substitute Chrome, \
         Playwright, a browser profile, or generic web search.\n\n",
    );
    roster_section(out, roster);
}

fn planning_section(out: &mut String, roster: &[Agent]) {
    out.push_str(
        "## How the work gets done\n\n\
         **You do not build the slices yourself, and you do not dispatch anybody.** Turn \
         the request into a plan. ai-team reads the plan back, gives each ready slice its \
         own worktree, and starts the seat whose zone owns it. Writing the code yourself \
         is the single most common way this goes wrong: it leaves the board empty, so \
         nothing is dispatched and nothing is reviewed.\n\n\
         The plan lives in ai-planner, which you reach through its MCP server. Read it \
         first with `get_plan` and `list_slices`: it is a board that outlives this \
         conversation, so add only what is genuinely missing. A slice that repeats one \
         already there gives somebody the same work twice.\n\n\
         Add work with `add_slice`. **The last line of every slice's scope must be a \
         `Touches:` line naming the paths it touches**, comma-separated, like this:\n\n\
         ```\n\
         Touches: src/lib.rs, crates/**\n\
         ```\n\n\
         That exact line is what routes the slice - ai-team reads it, finds the seat \
         whose zone owns those paths, and gives it the work. Describing the paths in \
         prose instead does not route anything: the slice is reported back to the human \
         undone, which is the single most common way a plan produces no work. Keep each \
         slice small enough to demo on its own.\n\n",
    );

    roster_section(out, roster);
}

fn roster_section(out: &mut String, roster: &[Agent]) {
    if roster.is_empty() {
        return;
    }
    out.push_str("## Your team\n\n");
    for agent in roster {
        let zone = agent
            .zone
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        let zone = if zone.is_empty() {
            "no owned paths".to_string()
        } else {
            zone
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
    out.push('\n');
}

fn building_section(out: &mut String, agent: &Agent) {
    let zone = agent
        .zone
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    out.push_str("## What you build\n\n");
    if zone.is_empty() {
        out.push_str("You have no owned paths, so work only on what the slice names.\n\n");
    } else {
        let _ = write!(
            out,
            "You own: {zone}. Work on what the slice names, and stay inside what you own \
             - another seat may be editing the rest of this repository right now.\n\n"
        );
    }

    out.push_str(
        "You read the board but you do not shape it. `get_slice` tells you what you are \
         building and `append_log` records anything worth knowing later; adding or \
         re-statusing slices belongs to the planner and Rust control plane, not you.\n\n",
    );

    if agent.read_only {
        out.push_str(
            "## You do not write\n\n\
             Your `write` and `edit` tools are withheld on purpose. Read, run the \
             project's checks, and report what you found. Do not work around this with \
             `bash` - a seat that edits source it was not given the tools to edit is \
             doing somebody else's job without their review.\n\n",
        );
    }
}

/// One line, bounded, for a roster entry.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(120).collect();
    if flat.chars().count() > 120 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewProject;
    use crate::store::Store;

    fn team() -> (Team, Vec<Agent>) {
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

    fn seat(role: &str) -> (Agent, Team, Vec<Agent>) {
        let (team, agents) = team();
        let agent = agents.iter().find(|a| a.role == role).unwrap().clone();
        (agent, team, agents)
    }

    #[test]
    fn the_orchestrator_is_told_to_ground_and_delegate_without_shaping_the_board() {
        let (agent, team, roster) = seat("orchestrator");
        let prompt = for_seat(&agent, &team, &roster);

        assert!(prompt.contains("You coordinate"), "{prompt}");
        assert!(prompt.contains("delegation brief"), "{prompt}");
        assert!(prompt.contains("do not shape the ai-planner"), "{prompt}");
        assert!(!prompt.contains("add_slice"), "{prompt}");
        assert!(prompt.contains("never substitute Chrome"), "{prompt}");
    }

    #[test]
    fn the_planner_is_told_who_owns_paths_and_how_to_shape_the_board() {
        // Routing is by zone, so a plan written without knowing the zones is a plan
        // whose slices nobody owns.
        let (agent, team, roster) = seat("planner");
        let prompt = for_seat(&agent, &team, &roster);
        assert!(prompt.contains("## Your team"), "{prompt}");
        assert!(prompt.contains("backend"), "{prompt}");
        assert!(prompt.contains("Owns:"), "{prompt}");
        assert!(prompt.contains("add_slice"), "{prompt}");
        assert!(prompt.contains("`Touches:` line"), "{prompt}");
        assert!(prompt.contains("Touches: src/lib.rs"), "{prompt}");
    }

    #[test]
    fn a_maker_is_not_told_the_roster_and_cannot_shape_the_plan() {
        let (agent, team, roster) = seat("backend");
        let prompt = for_seat(&agent, &team, &roster);

        assert!(!prompt.contains("## Your team"), "{prompt}");
        assert!(prompt.contains("do not shape it"), "{prompt}");
        assert!(!prompt.contains("add_slice"), "{prompt}");
    }

    #[test]
    fn a_read_only_seat_is_told_not_to_route_around_it() {
        // `bash` is still there, so this is the half of the guarantee that has to be
        // asked for rather than enforced.
        let (agent, team, roster) = seat("verifier");
        let prompt = for_seat(&agent, &team, &roster);
        assert!(prompt.contains("You do not write"), "{prompt}");
        assert!(prompt.contains("Do not work around this with"), "{prompt}");
    }

    #[test]
    fn every_seat_is_told_its_work_is_a_draft() {
        // The publish rule is enforced by the guard, and said here too: a refusal a seat
        // was not expecting reads as a broken tool.
        let (team, agents) = team();
        for agent in &agents {
            let prompt = for_seat(agent, &team, &agents);
            assert!(prompt.contains("draft"), "{}: {prompt}", agent.role);
            assert!(prompt.contains("do not push"), "{}", agent.role);
        }
    }

    #[test]
    fn a_custom_prompt_is_carried_through() {
        let (mut agent, team, roster) = seat("backend");
        agent.prompt_md = Some("Always use tabs.".into());
        let prompt = for_seat(&agent, &team, &roster);
        assert!(prompt.contains("Custom instructions"), "{prompt}");
        assert!(prompt.contains("Always use tabs."), "{prompt}");
    }
}
