//! A chat's controller and its explicitly enrolled execution attempts.

pub(super) mod build;
mod children;
pub(super) mod closure;
mod continuation;
mod control;
mod interrupted;
pub(super) mod kept;
mod recovery;
mod results;
mod startup;

use rusqlite::{params, OptionalExtension, Transaction};

use super::Store;
use crate::planning::PlanAccess;
use crate::{Agent, ChatTeamMember, ChatTeamRun, Error, Result};

impl Store {
    /// Legacy planning, steering and delivery must not reinterpret owned chat runs.
    pub(crate) fn require_legacy_run(&self, run: i64) -> Result<()> {
        let chat: bool = self.db().conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_turn WHERE run_id = ?1)",
            [run],
            |row| row.get(0),
        )?;
        if chat {
            return Err(Error::invalid(
                "this execution belongs to a chat; use its chat controls",
            ));
        }
        Ok(())
    }

    pub fn chat_team_run(&self, run_id: i64) -> Result<Option<ChatTeamRun>> {
        let found = self.db().conn().query_row(
            "SELECT run_id, chat_id, control_node_id, phase, base_sha, approved_revision,
                    reason, rev, supervisor_pid, supervisor_identity, controller_protocol, quiescent FROM chat_team_run WHERE run_id = ?1",
            [run_id], |row| Ok((ChatTeamRun {
                run_id: row.get(0)?, chat_id: row.get(1)?, control_node_id: row.get(2)?,
                phase: row.get(3)?, base_sha: row.get(4)?, approved_revision: row.get(5)?,
                reason: row.get(6)?, rev: row.get(7)?, supervisor_pid: row.get(8)?,
                supervisor_identity: row.get(9)?, quiescent: row.get(11)?, controller_lock: None,
            }, row.get::<_, bool>(10)?)),
        ).optional()?;
        found
            .map(|(mut execution, protocol)| {
                if protocol {
                    execution.controller_lock = Some(crate::chat::team::ownership::lock_path(
                        self.path(),
                        run_id,
                    )?);
                }
                Ok(execution)
            })
            .transpose()
    }

    pub fn chat_team_members(&self, run_id: i64) -> Result<Vec<ChatTeamMember>> {
        let mut statement = self.db().conn().prepare(
            "SELECT node_id, live_text, pi_identity FROM chat_team_node WHERE run_id = ?1 ORDER BY node_id",
        )?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, live_text, pi_identity)| {
                Ok(ChatTeamMember {
                    node: self.node_run(id)?,
                    live_text,
                    pi_identity,
                })
            })
            .collect()
    }

    pub(super) fn chat_team_coordinator(&self, team_id: Option<i64>) -> Result<Agent> {
        let team = team_id
            .ok_or_else(|| Error::invalid("configure a project team before using Team mode"))?;
        let agents = self.agents(team)?;
        let planning_seat = |role| {
            agents.iter().find(|agent| agent.role == role && agent.enabled && agent.read_only)
            .ok_or_else(|| Error::invalid(format!("Team mode needs an enabled, read-only {role} seat; configure it in Project tools")))
        };
        let coordinator = planning_seat(crate::ROOT_ROLE)?.clone();
        planning_seat("planner")?;
        if !agents
            .iter()
            .any(|agent| agent.enabled && PlanAccess::for_team_agent(agent) == PlanAccess::Maker)
        {
            return Err(Error::invalid(
                "Team mode needs at least one enabled maker seat",
            ));
        }
        Ok(coordinator)
    }
}

/// Called inside dispatch's transaction: failure rolls the node back too. A permission
/// is an attempt snapshot, so a later team edit cannot escalate an already running seat.
pub(super) fn register_member(
    tx: &Transaction<'_>,
    run_id: i64,
    node_id: i64,
    agent: &Agent,
    slice: Option<&str>,
) -> Result<()> {
    let phase: Option<String> = tx
        .query_row(
            "SELECT t.phase FROM chat_team_run t JOIN chat c ON c.id = t.chat_id
         JOIN run r ON r.id = t.run_id
         WHERE t.run_id = ?1 AND c.active_node_id = t.control_node_id
           AND c.stop_requested = 0 AND c.archived = 0 AND r.status = 'running'",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    let is_team: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_team_run WHERE run_id = ?1)",
        [run_id],
        |row| row.get(0),
    )?;
    if !is_team {
        return Ok(());
    }
    closure::require_open(tx, run_id)?;
    let phase =
        phase.ok_or_else(|| Error::invalid("this team execution no longer owns the chat"))?;
    if !matches!(phase.as_str(), "grounding" | "planning" | "building") {
        return Err(Error::invalid(
            "this team execution is not dispatching agents",
        ));
    }
    let unchanged: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent a JOIN run r ON r.team_id = a.team_id
                       WHERE a.id = ?1 AND a.rev = ?2 AND a.enabled = 1 AND r.id = ?3)",
        params![agent.id, agent.rev, run_id],
        |row| row.get(0),
    )?;
    if !unchanged {
        return Err(Error::invalid(
            "this seat changed or does not belong to the executing team; refresh and retry",
        ));
    }
    let access = PlanAccess::for_team_agent(agent);
    if matches!(access, PlanAccess::Coordinator | PlanAccess::Planner) && !agent.read_only {
        return Err(Error::invalid("team planning seats must be read-only"));
    }
    if matches!(access, PlanAccess::Maker | PlanAccess::Reader) {
        let approved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_build_slice WHERE run_id = ?1 AND slice_key = ?2 AND lease_state = 'leased')",
            params![run_id, slice], |row| row.get(0),
        )?;
        if phase != "building" || !approved {
            return Err(Error::invalid(
                "a team worker needs an approved, leased slice in this run",
            ));
        }
        let busy: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM node_run WHERE run_id = ?1 AND id != ?2
            AND status IN ('queued','running') AND (slice_key = ?3 OR agent_id = ?4))",
            params![run_id, node_id, slice, agent.id],
            |row| row.get(0),
        )?;
        if busy {
            return Err(Error::invalid(
                "a team worker already owns this lease or seat; wait for it to settle",
            ));
        }
        if access == PlanAccess::Maker {
            let approval = build::read(tx, run_id, slice.unwrap_or_default())?;
            let assigned = build::approved_agent(tx, &approval)?;
            if assigned.id != agent.id {
                return Err(Error::invalid(
                    "this maker was not approved for the assigned slice",
                ));
            }
        }
    } else if access == PlanAccess::Planner && phase == "building" {
        return Err(Error::invalid(
            "stop the build before replanning this execution",
        ));
    }
    tx.execute(
        "INSERT INTO chat_team_node (node_id, run_id, plan_access) VALUES (?1, ?2, ?3)",
        params![node_id, run_id, access.member_name()?],
    )?;
    Ok(())
}
