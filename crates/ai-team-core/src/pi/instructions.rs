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
/// who else is on the team, because it is not deciding who does what. `guide` is the
/// planning method the planner works to (PW13), when it is installed.
pub(super) fn for_seat(
    agent: &Agent,
    team: &Team,
    roster: &[Agent],
    guide: Option<&str>,
) -> String {
    let mut out = preamble(agent, team);

    if coordinates(agent) {
        coordinating_section(&mut out, roster);
    } else if plans(agent) {
        planning_section(&mut out, roster, guide);
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

/// The system prompt for the coordinating seat restacking a pull request (PW11).
///
/// Instead of its coordinating rules, not beside them: those say it does not build code,
/// and a conflict is resolved by editing the file it is in. Told to ask rather than guess,
/// because a wrong resolution merges cleanly and reads like the right one.
pub(super) fn for_restack(agent: &Agent, team: &Team) -> String {
    let mut out = preamble(agent, team);
    out.push_str(
        "## This turn: restacking a pull request\n\n\
         This turn is not coordination. A pull request in this plan is stacked on another, \
         and the one it stacks on has moved - merged, or changed since this one was built on \
         it - so this one has to be rebased onto where its parent is now. ai-team chose the \
         commits, and the message says exactly which rebase to run. You can write this turn, \
         because a conflict is resolved by editing the files it is in.\n\n\
         - Run the rebase you are given. When git stops on a conflict, read both sides and \
         resolve it so both survive: what the parent changed, and what this pull request set \
         out to do. Stage the files and `git rebase --continue`.\n\
         - Keep this pull request's commits: do not squash, reorder or reword them, and do \
         not change what a conflict does not touch.\n\
         - When the rebase is done, run the project's own checks. If the new base broke \
         something this pull request relies on, fix it in a commit of its own on this \
         branch.\n\
         - If you cannot tell how a conflict should be resolved, or the checks fail in a way \
         you cannot fix without guessing what somebody meant, run `git rebase --abort` and \
         stop. Name the files and what each side wanted: a person decides.\n\n\
         Leave the pull request on GitHub alone and do not push: ai-team publishes the \
         result, under the team's delivery policy, once it has checked it.\n\n\
         ## When you are done\n\n\
         Say what you did: the rebase, each conflict and how you resolved it, and the checks \
         you ran with what they said. If you stopped, say why and what needs deciding.\n",
    );
    out
}

/// Who the seat is, anything its operator told it, and where it is working.
fn preamble(agent: &Agent, team: &Team) -> String {
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
         Your working directory is a git worktree leased for this work, and while you are \
         in it nobody else is writing to it. It is real: build \
         it, test it, and run the project's own checks in it. Your tools are held to it - \
         reaching outside it is refused, and so is anything that publishes.\n\n\
         Your work is a draft. ai-team commits what you leave behind to a branch and a \
         human decides whether it goes anywhere, so do not push, tag, merge or release. \
         If something genuinely needs one of those, say so in your answer.\n\n",
    );
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

fn planning_section(out: &mut String, roster: &[Agent], guide: Option<&str>) {
    out.push_str(
        "## How the work gets done\n\n\
         **You do not build the slices yourself, and you do not dispatch anybody.** Turn \
         the request into a plan. ai-team reads the plan back, gives each pull request its \
         own worktree, and hands it to the seats its tasks name, one task at a time. \
         Writing the code yourself is the single most common way this goes wrong: it \
         leaves the board empty, so nothing is dispatched and nothing is reviewed.\n\n\
         The plan lives in ai-planner, which you reach through its MCP server. Read it \
         first with `get_plan` and `list_slices`: it is a board that outlives this \
         conversation, so add only what is genuinely missing. A slice that repeats one \
         already there gives somebody the same work twice.\n\n\
         Each slice is one pull request. Add it with `add_slice`; its scope carries the \
         user story, its acceptance criteria and its tasks. **Every task is one line, in \
         exactly this form, under a `## Tasks` heading:**\n\n\
         ```\n\
         ## Tasks\n\
         - T1 [backend] Add the range column and its migration - Touches: migrations/**, src/db/**\n\
         - T2 [frontend] Show the range picker on Summary - Touches: ui/src/Summary.tsx\n\
         ```\n\n\
         `T1`, `T2` and on number the tasks in the order they are built. The word in \
         brackets is the role of the seat that builds it - one of the writing seats under \
         Your team - and `Touches:` names the paths it changes, which belong inside that \
         seat's zone. Those lines are what ai-team reads to decide who builds what: a task \
         whose owner is not a writing seat on this team, or that names no paths, leaves its \
         pull request unbuilt and is reported back undone. A task two seats need is two \
         tasks.\n\n\
         **The last line of every slice's scope is a `Touches:` line naming every path its \
         tasks touch**, comma-separated:\n\n\
         ```\n\
         Touches: migrations/**, src/db/**, ui/src/Summary.tsx\n\
         ```\n\n\
         **A pull request that needs an earlier one's code stacks on it.** Say so with a \
         line in its scope, above the `Touches:` line:\n\n\
         ```\n\
         Stacks on: PR1\n\
         ```\n\n\
         It is then built on PR1's branch, in a worktree of its own, once PR1 is built. \
         Stack only for a real code dependency: every other pull request is built side by \
         side from the default branch, and a fix to the bottom of a stack has to be carried \
         up through every pull request above it.\n\n",
    );

    roster_section(out, roster);

    if let Some(guide) = guide {
        let _ = write!(
            out,
            "## How to shape the plan\n\n\
             Plan the way this guide says. Where it and the instructions above differ, the \
             instructions win: the task format is what the dispatcher reads, and this run's \
             plan already exists - skip any step that starts one.\n\n{}\n\n",
            guide.trim()
        );
    }
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
        out.push_str("You have no owned paths, so work only on what your task names.\n\n");
    } else {
        let _ = write!(
            out,
            "You own: {zone}. Work on what your task names, and stay inside what you own \
             - the rest of a pull request is built by the seats that own it, in their turn.\n\n"
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
        let prompt = for_seat(&agent, &team, &roster, None);

        assert!(prompt.contains("You coordinate"), "{prompt}");
        assert!(prompt.contains("delegation brief"), "{prompt}");
        assert!(prompt.contains("do not shape the ai-planner"), "{prompt}");
        assert!(!prompt.contains("add_slice"), "{prompt}");
        assert!(prompt.contains("never substitute Chrome"), "{prompt}");
    }

    #[test]
    fn a_restacking_orchestrator_is_told_to_rebase_keep_both_sides_and_ask_rather_than_guess() {
        let (agent, team, _) = seat("orchestrator");
        let prompt = for_restack(&agent, &team);

        assert!(prompt.contains("restacking a pull request"), "{prompt}");
        assert!(prompt.contains("git rebase --continue"), "{prompt}");
        assert!(prompt.contains("git rebase --abort"), "{prompt}");
        assert!(prompt.contains("project's own checks"), "{prompt}");
        // Publishing is ai-team's, after it has checked the result.
        assert!(prompt.contains("do not push"), "{prompt}");
        // Not the coordinating seat's rules, which forbid the very edit a conflict needs.
        assert!(!prompt.contains("You coordinate"), "{prompt}");
        assert!(!prompt.contains("delegation brief"), "{prompt}");
    }

    #[test]
    fn the_planner_is_told_who_owns_paths_and_how_to_shape_the_board() {
        // Routing is by zone, so a plan written without knowing the zones is a plan
        // whose slices nobody owns.
        let (agent, team, roster) = seat("planner");
        let prompt = for_seat(&agent, &team, &roster, None);
        assert!(prompt.contains("## Your team"), "{prompt}");
        assert!(prompt.contains("backend"), "{prompt}");
        assert!(prompt.contains("Owns:"), "{prompt}");
        assert!(prompt.contains("add_slice"), "{prompt}");
        assert!(prompt.contains("`Touches:` line"), "{prompt}");
        assert!(prompt.contains("## Tasks"), "{prompt}");
        // No guide installed, no guide section: an empty heading tells a model nothing.
        assert!(!prompt.contains("How to shape the plan"), "{prompt}");

        let guided = for_seat(&agent, &team, &roster, Some("Write user stories."));
        assert!(guided.contains("## How to shape the plan"), "{guided}");
        assert!(guided.contains("Write user stories."), "{guided}");
        // The guide may say to start a plan; ai-team already has.
        assert!(guided.contains("skip any step that starts one"), "{guided}");
    }

    #[test]
    fn the_stack_line_the_planner_is_shown_is_the_one_the_dispatcher_reads() {
        let (agent, team, roster) = seat("planner");
        let prompt = for_seat(&agent, &team, &roster, None);
        let slice = crate::neighbours::Slice {
            key: "PR2".into(),
            scope_md: Some(prompt),
            ..Default::default()
        };
        assert_eq!(
            crate::stack::declared_parent(&slice).as_deref(),
            Some("PR1")
        );
    }

    #[test]
    fn the_task_lines_the_planner_is_shown_are_the_ones_the_dispatcher_reads() {
        // The example is the contract. If the format shown and the format parsed drift
        // apart, every plan written from these instructions is reported back unbuilt.
        let (agent, team, roster) = seat("planner");
        let prompt = for_seat(&agent, &team, &roster, None);
        let listed = crate::tasks::parse(&prompt);
        assert!(listed.problems.is_empty(), "{:?}", listed.problems);
        assert_eq!(
            listed
                .tasks
                .iter()
                .map(|task| (task.key.as_str(), task.owner.as_str()))
                .collect::<Vec<_>>(),
            [("T1", "backend"), ("T2", "frontend")]
        );
        assert!(crate::tasks::check(&listed.tasks, &roster)
            .iter()
            .all(|finding| !finding.blocking));
    }

    #[test]
    fn a_maker_is_not_told_the_roster_and_cannot_shape_the_plan() {
        let (agent, team, roster) = seat("backend");
        let prompt = for_seat(&agent, &team, &roster, None);

        assert!(!prompt.contains("## Your team"), "{prompt}");
        assert!(prompt.contains("do not shape it"), "{prompt}");
        assert!(!prompt.contains("add_slice"), "{prompt}");
    }

    #[test]
    fn a_read_only_seat_is_told_not_to_route_around_it() {
        // `bash` is still there, so this is the half of the guarantee that has to be
        // asked for rather than enforced.
        let (agent, team, roster) = seat("verifier");
        let prompt = for_seat(&agent, &team, &roster, None);
        assert!(prompt.contains("You do not write"), "{prompt}");
        assert!(prompt.contains("Do not work around this with"), "{prompt}");
    }

    #[test]
    fn every_seat_is_told_its_work_is_a_draft() {
        // The publish rule is enforced by the guard, and said here too: a refusal a seat
        // was not expecting reads as a broken tool.
        let (team, agents) = team();
        for agent in &agents {
            let prompt = for_seat(agent, &team, &agents, None);
            assert!(prompt.contains("draft"), "{}: {prompt}", agent.role);
            assert!(prompt.contains("do not push"), "{}", agent.role);
        }
    }

    #[test]
    fn a_custom_prompt_is_carried_through() {
        let (mut agent, team, roster) = seat("backend");
        agent.prompt_md = Some("Always use tabs.".into());
        let prompt = for_seat(&agent, &team, &roster, None);
        assert!(prompt.contains("Custom instructions"), "{prompt}");
        assert!(prompt.contains("Always use tabs."), "{prompt}");
    }
}
