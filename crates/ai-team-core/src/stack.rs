//! How a plan's pull requests stack (PW3, PW9).
//!
//! A slice is one PR. Most are based on the default branch and build side by side; one
//! that needs an earlier one's code is based on that one's branch, and builds once it is
//! built. ai-planner already records a slice's `branch` and `base_branch`, so a stack is
//! plan structure it holds - this reads it, and never keeps a copy (D4).
//!
//! What ai-planner cannot do through its MCP server is *set* a base: `add_slice` and
//! `update_slice` take none. So a planner seat says it in the scope, the way it says what
//! a slice touches - a `Stacks on: PR1` line - and ai-team writes the base through the
//! CLI, which can. Branch names are settled at the same time, because a base is a branch
//! and a slice with no name has nothing to stack on.

use crate::neighbours::Slice;

/// A change to make to the plan so its stack is explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    Branch { key: String, branch: String },
    Base { key: String, base: String },
}

/// The branch a slice without one is given: `<plan>/<key>`.
///
/// Named for the plan as well as the slice, because every plan has a PR1: a name shared
/// across plans is a branch one plan's run resets while another's review is open on it.
pub(crate) fn default_branch(plan: &str, key: &str) -> String {
    format!("{plan}/{}", key.to_lowercase())
}

/// The key a slice says it stacks on, from its `Stacks on:` line.
pub(crate) fn declared_parent(slice: &Slice) -> Option<String> {
    slice
        .scope_md
        .as_deref()?
        .lines()
        .rev()
        .find_map(|line| {
            let line = line.trim();
            let (label, value) = line.split_once(':')?;
            label
                .trim()
                .eq_ignore_ascii_case("stacks on")
                .then(|| value.trim().trim_matches('`').trim().to_string())
        })
        .filter(|key| !key.is_empty() && !key.eq_ignore_ascii_case("none"))
}

/// Whose branch names stand when a plan's stack is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Names {
    /// A plan just written: nothing is built yet, so every branch is put under the plan's
    /// name - `<plan>/<the planner's name>`, or `<plan>/<key>` when it gave none. Plan slugs
    /// are unique, so no two plans' PRs can then share a branch.
    Own,
    /// A plan already in play: a branch it has is kept, because work may be on it. Only a
    /// missing one is filled in.
    Keep,
}

/// The branch `slice` should have, and whether that differs from what it has.
fn settled_branch(plan: &str, slice: &Slice, names: Names) -> (String, bool) {
    let current = slice
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty());
    let prefix = format!("{plan}/");
    match (current, names) {
        (None, _) => (default_branch(plan, &slice.key), true),
        (Some(branch), Names::Own) if !branch.starts_with(&prefix) => {
            (format!("{prefix}{branch}"), true)
        }
        (Some(branch), _) => (branch.to_string(), false),
    }
}

/// What to write to the plan so every slice has a branch and every declared stack a base.
///
/// Idempotent: a plan already explicit needs nothing, so this is safe to ask before every
/// build. A `Stacks on:` naming a slice the plan does not have changes nothing - that is
/// a problem to report, not a base to guess.
pub(crate) fn edits(plan: &str, slices: &[Slice], names: Names) -> Vec<Edit> {
    let mut edits = Vec::new();
    for slice in slices {
        let (branch, changed) = settled_branch(plan, slice, names);
        if changed {
            edits.push(Edit::Branch {
                key: slice.key.clone(),
                branch,
            });
        }
        let Some(parent) = declared_parent(slice) else {
            continue;
        };
        let Some(parent) = slices
            .iter()
            .find(|other| other.key.eq_ignore_ascii_case(&parent) && other.key != slice.key)
        else {
            continue;
        };
        let (base, _) = settled_branch(plan, parent, names);
        if slice.base_branch.as_deref().map(str::trim) != Some(base.as_str()) {
            edits.push(Edit::Base {
                key: slice.key.clone(),
                base,
            });
        }
    }
    edits
}

/// The slice this one is based on, when it is based on another slice of the plan.
pub fn parent<'a>(slice: &Slice, slices: &'a [Slice]) -> Option<&'a Slice> {
    let base = slice.base_branch.as_deref().map(str::trim)?;
    slices.iter().find(|other| {
        other.key != slice.key
            && other
                .branch
                .as_deref()
                .map(str::trim)
                .is_some_and(|branch| branch == base)
    })
}

/// Why a slice cannot be built yet, when it stacks on one that is not built.
///
/// Built means built and verified - `in_review` or `done` on the board (PW9) - not merged:
/// a stack is worked on while its root is still in review, which is the point of one.
pub(crate) fn waiting_on(slice: &Slice, slices: &[Slice]) -> Option<String> {
    let parent = parent(slice, slices)?;
    if matches!(parent.status.as_str(), "in_review" | "done") {
        return None;
    }
    Some(format!(
        "stacks on {}, which is {} - it is built once {} is",
        parent.key,
        parent.status.replace('_', " "),
        parent.key
    ))
}

/// What is wrong with how a plan's slices stack, slice by slice.
pub(crate) fn problems(slices: &[Slice]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for slice in slices {
        if let Some(declared) = declared_parent(slice) {
            if declared.eq_ignore_ascii_case(&slice.key) {
                found.push((slice.key.clone(), format!("{} stacks on itself", slice.key)));
                continue;
            }
            if !slices
                .iter()
                .any(|other| other.key.eq_ignore_ascii_case(&declared))
            {
                found.push((
                    slice.key.clone(),
                    format!("stacks on {declared}, which this plan does not have"),
                ));
                continue;
            }
        }
        // Walked by base branch, which is what building actually follows: a loop never
        // reaches a slice based on the trunk, so it would never be built.
        let mut seen = vec![slice.key.as_str()];
        let mut at = slice;
        while let Some(next) = parent(at, slices) {
            if seen.contains(&next.key.as_str()) {
                found.push((
                    slice.key.clone(),
                    format!("stacks in a loop: {} -> {}", seen.join(" -> "), next.key),
                ));
                break;
            }
            seen.push(&next.key);
            at = next;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(key: &str, scope: &str, branch: Option<&str>, base: Option<&str>) -> Slice {
        Slice {
            key: key.into(),
            title: format!("{key} title"),
            status: "ready".into(),
            scope_md: Some(scope.into()),
            branch: branch.map(str::to_string),
            base_branch: base.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn every_slice_gets_a_plan_scoped_branch_and_a_declared_stack_gets_its_base() {
        let slices = [
            slice("PR1", "The root.\n\nTouches: crates/**", None, Some("main")),
            slice(
                "PR2",
                "On top.\n\nStacks on: PR1\nTouches: ui/**",
                None,
                Some("main"),
            ),
            slice(
                "PR3",
                "Beside.\n\nStacks on: none",
                Some("mine/pr3"),
                Some("main"),
            ),
        ];

        assert_eq!(
            edits("csv-export", &slices, Names::Keep),
            [
                Edit::Branch {
                    key: "PR1".into(),
                    branch: "csv-export/pr1".into()
                },
                Edit::Branch {
                    key: "PR2".into(),
                    branch: "csv-export/pr2".into()
                },
                Edit::Base {
                    key: "PR2".into(),
                    base: "csv-export/pr1".into()
                },
            ]
        );
    }

    #[test]
    fn a_plan_just_written_has_every_branch_put_under_its_name() {
        // A planner names branches as it likes. Left bare, `pr1-shout-option` from one plan
        // is the branch a later plan's PR1 continues - someone else's commits built on as
        // if they were its own. Nothing is built yet, so the names are ai-team's to set.
        let slices = [
            slice("PR1", "", Some("pr1-shout-option"), Some("main")),
            slice(
                "PR2",
                "Stacks on: PR1",
                Some("pr2-web-command"),
                Some("main"),
            ),
            slice("PR3", "", Some("shout/pr3-docs"), Some("main")),
            slice("PR4", "", None, Some("main")),
        ];

        assert_eq!(
            edits("shout", &slices, Names::Own),
            [
                Edit::Branch {
                    key: "PR1".into(),
                    branch: "shout/pr1-shout-option".into()
                },
                Edit::Branch {
                    key: "PR2".into(),
                    branch: "shout/pr2-web-command".into()
                },
                // Stacked on PR1 as PR1 is now called.
                Edit::Base {
                    key: "PR2".into(),
                    base: "shout/pr1-shout-option".into()
                },
                // Already the plan's.
                Edit::Branch {
                    key: "PR4".into(),
                    branch: "shout/pr4".into()
                },
            ]
        );

        // Once work may be on them, a plan's names stand, whatever they are.
        let named = [slice("PR1", "", Some("pr1-shout-option"), Some("main"))];
        assert!(edits("shout", &named, Names::Keep).is_empty());
    }

    #[test]
    fn a_plan_already_explicit_needs_nothing() {
        let slices = [
            slice("PR1", "", Some("p/pr1"), Some("main")),
            slice("PR2", "Stacks on: `PR1`", Some("p/pr2"), Some("p/pr1")),
        ];
        assert!(edits("p", &slices, Names::Keep).is_empty());
    }

    #[test]
    fn a_child_waits_until_its_parent_is_built_not_merged() {
        let mut slices = vec![
            slice("PR1", "", Some("p/pr1"), Some("main")),
            slice("PR2", "", Some("p/pr2"), Some("p/pr1")),
            slice("PR3", "", Some("p/pr3"), Some("main")),
        ];
        assert_eq!(
            parent(&slices[1], &slices).map(|s| s.key.as_str()),
            Some("PR1")
        );
        assert!(parent(&slices[2], &slices).is_none());

        let waiting = waiting_on(&slices[1], &slices).unwrap();
        assert!(
            waiting.contains("stacks on PR1, which is ready"),
            "{waiting}"
        );
        assert!(waiting_on(&slices[2], &slices).is_none());

        slices[0].status = "in_review".into();
        assert!(waiting_on(&slices[1], &slices).is_none());
        slices[0].status = "blocked".into();
        assert!(waiting_on(&slices[1], &slices).is_some());
    }

    #[test]
    fn a_stack_that_cannot_be_built_is_named() {
        let slices = [
            slice("PR1", "Stacks on: PR9", Some("p/pr1"), Some("main")),
            slice("PR2", "Stacks on: PR2", Some("p/pr2"), Some("main")),
            slice("PR3", "", Some("p/pr3"), Some("p/pr4")),
            slice("PR4", "", Some("p/pr4"), Some("p/pr3")),
        ];
        let found = problems(&slices);
        let text: Vec<String> = found.iter().map(|(k, m)| format!("{k}: {m}")).collect();
        assert!(
            text[0].contains("PR1: stacks on PR9, which this plan does not have"),
            "{text:?}"
        );
        assert!(text[1].contains("PR2: PR2 stacks on itself"), "{text:?}");
        assert!(text[2].contains("PR3: stacks in a loop"), "{text:?}");
        assert!(text[3].contains("PR4: stacks in a loop"), "{text:?}");
    }
}
