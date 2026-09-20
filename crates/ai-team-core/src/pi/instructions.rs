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

use crate::generate::ROOT_ROLE;
use crate::model::{Agent, Team};

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

    if plans(agent) {
        planning_section(&mut out, roster);
    } else {
        building_section(&mut out, agent);
    }

    out.push_str(
        "## When you are done\n\n\
         Say what you changed and what you ran, in plain prose. A turn that reports done \
         but changed no file is recorded as failed, so if you could not do the work, say \
         that instead - it is a useful answer and a false one is not.\n",
    );
    out
}

/// Which seats may shape the plan.
///
/// The orchestrator and the planner, and nobody else. A maker that can add slices can
/// give itself work, and the board a human reads stops being a plan and becomes a log of
/// whatever the agents felt like doing.
fn plans(agent: &Agent) -> bool {
    agent.role == ROOT_ROLE || agent.role == "planner"
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
         Add work with `add_slice`. **Every slice must name the paths it touches.** That \
         is what routes it: a slice naming no path this team owns cannot be given to \
         anybody and is reported back to the human undone. Keep each slice small enough \
         to demo on its own.\n\n",
    );

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
         re-statusing slices is the orchestrator's job, not yours.\n\n",
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
    fn the_orchestrator_is_told_not_to_build() {
        // The first run on Pi failed exactly here: with no instructions the orchestrator
        // was a coding assistant with a bash tool, so it wrote the code and left the
        // board empty.
        let (agent, team, roster) = seat("orchestrator");
        let prompt = for_seat(&agent, &team, &roster);

        assert!(
            prompt.contains("do not build the slices yourself"),
            "{prompt}"
        );
        assert!(prompt.contains("add_slice"), "{prompt}");
        assert!(prompt.contains("name the paths it touches"), "{prompt}");
    }

    #[test]
    fn a_planning_seat_is_told_who_is_on_the_team() {
        // Routing is by zone, so a plan written without knowing the zones is a plan
        // whose slices nobody owns.
        let (agent, team, roster) = seat("orchestrator");
        let prompt = for_seat(&agent, &team, &roster);
        assert!(prompt.contains("## Your team"), "{prompt}");
        assert!(prompt.contains("backend"), "{prompt}");
        assert!(prompt.contains("Owns:"), "{prompt}");
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
