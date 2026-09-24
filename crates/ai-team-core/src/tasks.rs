//! The tasks inside a pull request (PW4).
//!
//! A slice on the board is one PR, and a PR a person can see working usually needs more
//! than one seat: the data model, then the API, then the screen. The planner writes those
//! steps into the slice's scope, one line each, in the shape the `agile-plan` skill
//! teaches:
//!
//! ```text
//! ## Tasks
//! - T1 [backend] Add the range column and its migration - Touches: migrations/**, src/db/**
//! - T2 [frontend] Show the range picker on Summary - Touches: ui/src/Summary.tsx
//! ```
//!
//! They live on the board rather than here, because ai-planner owns the plan (D4) and has
//! no level below a slice - so this reads them back out of the scope the same way the
//! `Touches:` trailer is read, and never stores a copy. What ai-team keeps is what it
//! did: each task's turns are node runs carrying the task's key as a reference.

use crate::model::Agent;

/// One step of a PR: who builds it, and what it touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// `T1`, as the planner numbered it.
    pub key: String,
    /// The role of the seat that builds it.
    pub owner: String,
    pub title: String,
    pub touches: Vec<String>,
}

/// What a scope's task lines amount to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskList {
    pub tasks: Vec<Task>,
    /// Lines that look like a task but could not be read. Reported, never guessed at: a
    /// task read wrong is work given to the wrong seat.
    pub problems: Vec<String>,
}

/// Read the task lines out of a slice's scope, in the order the planner wrote them.
///
/// A task line is a bullet whose text starts with `T` and a number. Everything else in
/// the scope - the story, its acceptance criteria, the `Touches:` trailer - is prose for
/// the seats to read, not structure for this to parse.
pub(crate) fn parse(scope: &str) -> TaskList {
    let mut list = TaskList::default();
    for (index, raw) in scope.lines().enumerate() {
        let Some(text) = bullet(raw) else {
            continue;
        };
        let Some((key, rest)) = task_key(text) else {
            continue;
        };
        match task_line(&key, rest) {
            Ok(task) => {
                if list.tasks.iter().any(|seen| seen.key == task.key) {
                    list.problems.push(format!(
                        "{} is listed twice (line {})",
                        task.key,
                        index + 1
                    ));
                } else {
                    list.tasks.push(task);
                }
            }
            Err(problem) => list
                .problems
                .push(format!("{key} on line {}: {problem}", index + 1)),
        }
    }
    list
}

/// The text of a bullet line, without its marker.
fn bullet(line: &str) -> Option<&str> {
    let line = line.trim_start();
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
        .map(str::trim)
}

/// `T12 ...` -> (`T12`, `...`). A capital or lower-case `t` followed by digits and then
/// a space, `:` or `.` - so a bullet that merely starts with "Tests" is prose.
fn task_key(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix('T').or_else(|| text.strip_prefix('t'))?;
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = &rest[digits..];
    let after = after
        .strip_prefix(':')
        .or_else(|| after.strip_prefix('.'))
        .unwrap_or(after);
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    Some((format!("T{}", &rest[..digits]), after.trim_start()))
}

fn task_line(key: &str, rest: &str) -> Result<Task, String> {
    let Some(after_bracket) = rest.strip_prefix('[') else {
        return Err("names no owner - write the seat's role in brackets, like [backend]".into());
    };
    let Some((owner, rest)) = after_bracket.split_once(']') else {
        return Err("the owner's bracket is never closed".into());
    };
    let owner = owner.trim().trim_matches('`').to_lowercase();
    if owner.is_empty() {
        return Err("names no owner - the brackets are empty".into());
    }

    // The last `Touches:` on the line, so a title that mentions the word still parses.
    let lower = rest.to_lowercase();
    let Some(at) = lower.rfind("touches:") else {
        return Err("names no paths - end the line with `Touches: <paths>`".into());
    };
    let title = rest[..at]
        .trim()
        .trim_end_matches(['-', '–', '—', ':', '|'])
        .trim()
        .to_string();
    let touches: Vec<String> = rest[at + "touches:".len()..]
        .split(',')
        .map(|path| path.trim().trim_matches('`').to_string())
        .filter(|path| !path.is_empty())
        .collect();
    if title.is_empty() {
        return Err("has no title".into());
    }
    if touches.is_empty() {
        return Err("names no paths after `Touches:`".into());
    }
    Ok(Task {
        key: key.to_string(),
        owner,
        title,
        touches,
    })
}

/// Something a person should see about a PR's tasks before approving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub task: String,
    pub message: String,
    /// Whether it stops the PR being built. A seat that does not exist, or cannot edit,
    /// cannot build anything; a path outside a seat's zone is worth a look but is the
    /// planner's call (PW5).
    pub blocking: bool,
}

/// Hold each task against the team that will build it.
///
/// The owner is the authority (PW5): a task is never quietly given to a different seat.
/// What can go wrong is reported instead - an owner the team does not have, one that
/// cannot write, and paths outside the owner's zone.
pub(crate) fn check(tasks: &[Task], roster: &[Agent]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for task in tasks {
        let Some(owner) = roster
            .iter()
            .find(|agent| agent.role.eq_ignore_ascii_case(&task.owner))
        else {
            findings.push(Finding {
                task: task.key.clone(),
                message: format!(
                    "{} is owned by `{}`, and this team has no such seat",
                    task.key, task.owner
                ),
                blocking: true,
            });
            continue;
        };
        if !owner.enabled {
            findings.push(Finding {
                task: task.key.clone(),
                message: format!(
                    "{} is owned by {}, who is switched off",
                    task.key, owner.role
                ),
                blocking: true,
            });
            continue;
        }
        if owner.read_only {
            findings.push(Finding {
                task: task.key.clone(),
                message: format!(
                    "{} is owned by {}, who reads but cannot edit files",
                    task.key, owner.role
                ),
                blocking: true,
            });
            continue;
        }
        // A seat with no zone is a generalist: nothing it touches is out of bounds.
        if owner.zone.trim().is_empty() {
            continue;
        }
        let outside: Vec<&str> = task
            .touches
            .iter()
            .map(String::as_str)
            .filter(|path| crate::util::zone_specificity(&owner.zone, path).is_none())
            .collect();
        if !outside.is_empty() {
            findings.push(Finding {
                task: task.key.clone(),
                message: format!(
                    "{} gives {} {}, outside its zone ({})",
                    task.key,
                    owner.role,
                    outside.join(", "),
                    owner
                        .zone
                        .lines()
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                blocking: false,
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(role: &str, zone: &str, read_only: bool) -> Agent {
        Agent {
            id: 1,
            team_id: 1,
            ord: 0,
            role: role.into(),
            name: role.into(),
            purpose: String::new(),
            provider: crate::model::Provider::Local,
            model: "auto".into(),
            reasoning: crate::model::Reasoning::Medium,
            zone: zone.into(),
            prompt_preset: None,
            prompt_md: None,
            context_window: None,
            read_only,
            enabled: true,
            rev: 0,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn the_skills_task_lines_are_read_in_order_with_their_owners_and_paths() {
        let scope = "As an operator, I want a range picker.\n\n\
                     ## Tasks\n\
                     - T1 [backend] Add the range column and its migration - Touches: migrations/**, src/db/**\n\
                     - T2 [frontend] Show the range picker on Summary - Touches: `ui/src/Summary.tsx`\n\n\
                     Touches: migrations/**, src/db/**, ui/**";
        let list = parse(scope);
        assert!(list.problems.is_empty(), "{:?}", list.problems);
        assert_eq!(
            list.tasks,
            [
                Task {
                    key: "T1".into(),
                    owner: "backend".into(),
                    title: "Add the range column and its migration".into(),
                    touches: vec!["migrations/**".into(), "src/db/**".into()],
                },
                Task {
                    key: "T2".into(),
                    owner: "frontend".into(),
                    title: "Show the range picker on Summary".into(),
                    touches: vec!["ui/src/Summary.tsx".into()],
                },
            ]
        );
    }

    #[test]
    fn what_a_model_writes_around_the_format_still_reads() {
        let list = parse(
            "* T3: [Backend] Wire the export (touches: nothing yet) — Touches: crates/**\n\
             + t4. [`frontend`] Button | Touches: ui/**",
        );
        assert!(list.problems.is_empty(), "{:?}", list.problems);
        assert_eq!(list.tasks[0].key, "T3");
        assert_eq!(list.tasks[0].owner, "backend");
        assert_eq!(
            list.tasks[0].title,
            "Wire the export (touches: nothing yet)"
        );
        assert_eq!(list.tasks[0].touches, ["crates/**"]);
        assert_eq!(list.tasks[1].key, "T4");
        assert_eq!(list.tasks[1].owner, "frontend");
        assert_eq!(list.tasks[1].title, "Button");
    }

    #[test]
    fn prose_is_not_a_task_and_a_scope_without_tasks_has_none() {
        let list =
            parse("- Tests must pass\n- The T-shirt size is S\n- Tidy up\nTouches: crates/**");
        assert!(list.tasks.is_empty());
        assert!(list.problems.is_empty());
    }

    #[test]
    fn a_task_that_cannot_be_read_is_reported_rather_than_guessed() {
        let list = parse(
            "- T1 Add the thing - Touches: src/**\n\
             - T2 [backend] Add the other thing\n\
             - T3 [] Nobody - Touches: src/**\n\
             - T4 [backend] - Touches: src/**\n\
             - T5 [backend] Paths missing - Touches:\n\
             - T6 [backend] Fine - Touches: src/**\n\
             - T6 [frontend] Again - Touches: ui/**",
        );
        assert_eq!(list.tasks.len(), 1, "{:?}", list.tasks);
        assert_eq!(list.tasks[0].key, "T6");
        let problems = list.problems.join("\n");
        for expected in [
            "T1 on line 1: names no owner",
            "T2 on line 2: names no paths",
            "T3 on line 3: names no owner - the brackets are empty",
            "T4 on line 4: has no title",
            "T5 on line 5: names no paths after",
            "T6 is listed twice (line 7)",
        ] {
            assert!(
                problems.contains(expected),
                "missing {expected:?} in\n{problems}"
            );
        }
    }

    #[test]
    fn an_owner_the_team_cannot_use_stops_the_pr_and_a_stray_path_is_only_flagged() {
        let roster = [
            seat("backend", "crates/**\n*.toml", false),
            seat("frontend", "ui/**", false),
            seat("verifier", "", true),
            seat("generalist", "", false),
        ];
        let tasks = parse(
            "- T1 [backend] Server - Touches: crates/core/src/lib.rs\n\
             - T2 [frontend] Screen and a server tweak - Touches: ui/src/App.tsx, crates/core/src/api.rs\n\
             - T3 [designer] Mockup - Touches: design/**\n\
             - T4 [verifier] Check it - Touches: crates/**\n\
             - T5 [generalist] Anything - Touches: docs/**",
        )
        .tasks;

        let findings = check(&tasks, &roster);

        assert_eq!(
            findings
                .iter()
                .map(|f| (f.task.as_str(), f.blocking))
                .collect::<Vec<_>>(),
            [("T2", false), ("T3", true), ("T4", true)]
        );
        assert!(
            findings[0].message.contains("crates/core/src/api.rs"),
            "{}",
            findings[0].message
        );
        assert!(
            !findings[0].message.contains("App.tsx"),
            "{}",
            findings[0].message
        );
        assert!(findings[1].message.contains("no such seat"));
        assert!(findings[2].message.contains("cannot edit"));

        let mut off = seat("frontend", "ui/**", false);
        off.enabled = false;
        let findings = check(&tasks[1..2], &[off]);
        assert!(findings[0].blocking && findings[0].message.contains("switched off"));
    }
}
