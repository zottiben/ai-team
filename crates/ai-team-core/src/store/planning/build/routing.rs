use crate::planning::PlanAccess;
use crate::{Agent, Error, Result};

/// Every touched path must have the same unambiguous maker, not merely the first
/// path with an owner. Broad globs must not hide a more-specific seat underneath.
pub(super) fn maker<'a>(roster: &'a [Agent], slice: &ai_planner_core::Slice) -> Result<&'a Agent> {
    let paths: Vec<_> = slice
        .scope_md
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("Touches:"))
        .map(|paths| {
            paths
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        return Err(Error::invalid(format!(
            "{} names no touched paths",
            slice.key
        )));
    }
    let makers: Vec<_> = roster
        .iter()
        .filter(|agent| agent.enabled && PlanAccess::for_team_agent(agent) == PlanAccess::Maker)
        .collect();
    let mut assigned = None;
    for path in paths {
        let mut matches: Vec<_> = makers
            .iter()
            .filter_map(|agent| {
                crate::util::zone_specificity(&agent.zone, path).map(|rank| (*agent, rank))
            })
            .collect();
        matches.sort_by_key(|(_, rank)| std::cmp::Reverse(*rank));
        let Some(&(owner, rank)) = matches.first() else {
            return Err(Error::invalid(format!(
                "{}: no maker owns {path}",
                slice.key
            )));
        };
        if matches.get(1).is_some_and(|(_, second)| *second == rank) {
            return Err(Error::invalid(format!(
                "{}: ambiguous ownership of {path}",
                slice.key
            )));
        }
        let prefix = path.split(['*', '?']).next().unwrap_or(path);
        if (prefix != path || path.ends_with('/'))
            && makers.iter().any(|agent| {
                agent.id != owner.id
                    && agent.zone.lines().any(|zone| {
                        let zone = zone.trim();
                        let literal = zone.split(['*', '?']).next().unwrap_or(zone);
                        !zone.starts_with('#')
                            && literal.starts_with(prefix)
                            && zone.chars().filter(|c| !matches!(c, '*' | '?')).count() > rank
                    })
            })
        {
            return Err(Error::invalid(format!(
                "{} spans another maker's zone; narrow or split {path}",
                slice.key
            )));
        }
        if assigned.is_some_and(|id| id != owner.id) {
            return Err(Error::invalid(format!(
                "{} spans multiple maker zones; split the slice before approval",
                slice.key
            )));
        }
        assigned = Some(owner.id);
    }
    roster
        .iter()
        .find(|agent| Some(agent.id) == assigned)
        .ok_or_else(|| Error::invalid("the routed maker disappeared"))
}
