//! Teams and the seats on them.
//!
//! The team is the database (D2). Nothing here writes a file; the generated eve project
//! is derived from these rows in M1-S3, and a change made by hand under
//! `.ai-team/agents/` is a change that the next regeneration silently throws away.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::model::{
    Agent, DeliveryPolicy, DeliverySettings, Guardrails, NewAgent, Provider, Team, ToolEffect,
    ToolPolicy,
};
use crate::roles::DEFAULT_ROSTER;
use crate::store::{non_empty, Store};
use crate::util::{now, slugify, zone_matches};

impl Store {
    /// Create a team, optionally bound to a project. A team with no project is a
    /// template, which is what `clone_team` copies from.
    pub fn create_team(
        &mut self,
        project_id: Option<i64>,
        name: &str,
        guardrails: Guardrails,
    ) -> Result<Team> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(Error::invalid("a team needs a name"));
        }
        let slug = slugify(&name);
        if slug.is_empty() {
            return Err(Error::invalid(format!(
                "{name:?} does not reduce to a slug"
            )));
        }
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO team
                   (project_id, slug, name, parallel_width, budget_tokens_run, budget_tokens_node,
                    budget_seconds_run, budget_seconds_node, max_turns_node, max_repairs,
                    on_failure, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                params![
                    project_id,
                    slug,
                    name,
                    guardrails.parallel_width,
                    guardrails.budget_tokens_run,
                    guardrails.budget_tokens_node,
                    guardrails.budget_seconds_run,
                    guardrails.budget_seconds_node,
                    guardrails.max_turns_node,
                    guardrails.max_repairs,
                    guardrails.on_failure,
                    at
                ],
            )
            .map_err(|e| duplicate_team(e, &slug))?;
            Ok(tx.last_insert_rowid())
        })?;

        self.team(id)
    }

    /// Create the default team of six and point the project at it.
    ///
    /// This is what makes a fresh database explain itself: `ait init` leaves behind a
    /// project with a real roster rather than empty tables.
    pub fn seed_default_team(&mut self, project_id: i64) -> Result<Team> {
        let project = self.project(project_id)?;
        let team = self.create_team(
            Some(project_id),
            &format!("{} team", project.name),
            Guardrails::default(),
        )?;
        // Counted as i64 from the start rather than casting an index: `ord` is a column
        // type, and the cast is the kind of thing that is correct until it is not.
        // When Pi is available, the defaults are exact ids chosen for each job. Project
        // creation still has a local floor when a fresh machine cannot list models yet.
        // Either way the rows are persisted: a later catalogue change cannot silently
        // reroute an existing team.
        let local = || {
            DEFAULT_ROSTER
                .iter()
                .map(|preset| crate::machine::RoleModelDefault {
                    role: preset.role.to_string(),
                    provider: crate::model::Provider::Local,
                    model: "auto".to_string(),
                })
                .collect()
        };
        let defaults = crate::machine::ModelRegistry::load()
            .ok()
            .and_then(|registry| registry.role_defaults().ok())
            .unwrap_or_else(local);
        for ((ord, preset), choice) in (0i64..).zip(DEFAULT_ROSTER).zip(defaults) {
            debug_assert_eq!(preset.role, choice.role);
            self.add_agent(
                team.id,
                preset.to_new_agent_on(choice.provider, &choice.model, ord),
            )?;
        }
        self.set_project_team(project_id, Some(team.id))?;
        self.team(team.id)
    }

    pub fn team(&self, id: i64) -> Result<Team> {
        self.db()
            .conn()
            .query_row(
                &format!("{TEAM_SELECT} WHERE id = ?1"),
                params![id],
                team_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchTeam(id.to_string()))
    }

    pub fn teams(&self, project_id: Option<i64>) -> Result<Vec<Team>> {
        let (sql, args): (String, Vec<Box<dyn rusqlite::ToSql>>) = match project_id {
            Some(id) => (
                format!("{TEAM_SELECT} WHERE project_id = ?1 ORDER BY slug"),
                vec![Box::new(id)],
            ),
            None => (format!("{TEAM_SELECT} ORDER BY slug"), vec![]),
        };
        let mut stmt = self.db().conn().prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), team_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// A team name whose slug nothing is using yet, by appending a counter.
    fn free_team_name(&self, wanted: &str) -> Result<String> {
        let wanted = wanted.trim();
        if slugify(wanted).is_empty() {
            return Err(Error::invalid(format!(
                "{wanted:?} does not reduce to a slug"
            )));
        }
        if self.find_team(&slugify(wanted)).is_err() {
            return Ok(wanted.to_string());
        }
        for suffix in 2..1000 {
            let candidate = format!("{wanted} {suffix}");
            if self.find_team(&slugify(&candidate)).is_err() {
                return Ok(candidate);
            }
        }
        Err(Error::invalid(format!(
            "could not find a free name near {wanted:?}"
        )))
    }

    /// Resolve a team by slug, then by id. A template has no project, so naming the
    /// team itself is the only way to reach one.
    pub fn find_team(&self, needle: &str) -> Result<Team> {
        let needle = needle.trim();
        if needle.is_empty() {
            return Err(Error::invalid("which team?"));
        }
        if let Some(found) = self
            .db()
            .conn()
            .query_row(
                &format!("{TEAM_SELECT} WHERE slug = ?1"),
                params![needle],
                team_from_row,
            )
            .optional()?
        {
            return Ok(found);
        }
        if let Ok(id) = needle.parse::<i64>() {
            if let Ok(found) = self.team(id) {
                return Ok(found);
            }
        }
        Err(Error::NoSuchTeam(needle.to_string()))
    }

    /// What a human means by "that team": the team a project is running, or a team named
    /// directly. Projects win, because that is what almost every invocation means and a
    /// project slug and a team slug can legitimately collide.
    pub fn find_team_for(&self, needle: &str) -> Result<Team> {
        if let Ok(project) = self.find_project(needle) {
            return match project.team_id {
                Some(team_id) => self.team(team_id),
                None => Err(Error::invalid(format!(
                    "{} has no team - `ait init` seeds one, or clone one onto it",
                    project.slug
                ))),
            };
        }
        self.find_team(needle)
    }

    pub fn update_team(
        &mut self,
        team_id: i64,
        name: &str,
        description: &str,
        guardrails: Guardrails,
    ) -> Result<Team> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("a team needs a name"));
        }
        let slug = slugify(name);
        if slug.is_empty() {
            return Err(Error::invalid(format!(
                "{name:?} does not reduce to a slug"
            )));
        }
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx
                .execute(
                    "UPDATE team
                        SET slug = ?2, name = ?3, description = ?4, parallel_width = ?5,
                            budget_tokens_run = ?6, budget_tokens_node = ?7,
                            budget_seconds_run = ?8, budget_seconds_node = ?9,
                            max_turns_node = ?10, max_repairs = ?11, on_failure = ?12,
                            rev = rev + 1, updated_at = ?13
                      WHERE id = ?1",
                    params![
                        team_id,
                        slug,
                        name,
                        description.trim(),
                        guardrails.parallel_width,
                        guardrails.budget_tokens_run,
                        guardrails.budget_tokens_node,
                        guardrails.budget_seconds_run,
                        guardrails.budget_seconds_node,
                        guardrails.max_turns_node,
                        guardrails.max_repairs,
                        guardrails.on_failure,
                        at,
                    ],
                )
                .map_err(|error| duplicate_team(error, &slug))?;
            if changed == 0 {
                return Err(Error::NoSuchTeam(team_id.to_string()));
            }
            Ok(())
        })?;
        self.team(team_id)
    }

    pub fn update_delivery(&mut self, team_id: i64, delivery: DeliverySettings) -> Result<Team> {
        let at = now();
        let changed = self.db_mut().write(|tx| {
            Ok(tx.execute(
                "UPDATE team
                    SET push_policy = ?2, pr_policy = ?3, merge_policy = ?4,
                        rev = rev + 1, updated_at = ?5
                  WHERE id = ?1",
                params![team_id, delivery.push, delivery.pr, delivery.merge, at],
            )?)
        })?;
        if changed == 0 {
            return Err(Error::NoSuchTeam(team_id.to_string()));
        }
        self.team(team_id)
    }

    pub fn delete_team(&mut self, team_id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            // project.team_id deliberately has no FK because it forms a creation-time
            // cycle with team.project_id, so clear it explicitly before deleting.
            tx.execute(
                "UPDATE project SET team_id = NULL, rev = rev + 1, updated_at = ?2
                  WHERE team_id = ?1",
                params![team_id, now()],
            )?;
            let changed = tx.execute("DELETE FROM team WHERE id = ?1", params![team_id])?;
            if changed == 0 {
                return Err(Error::NoSuchTeam(team_id.to_string()));
            }
            Ok(())
        })
    }

    pub fn add_agent(&mut self, team_id: i64, mut new: NewAgent) -> Result<Agent> {
        normalise_agent(&mut new)?;
        let role = new.role.clone();
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO agent
                   (team_id, ord, role, name, purpose, provider, model, reasoning, zone,
                    prompt_preset, prompt_md, context_window, read_only, enabled,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?15)",
                params![
                    team_id,
                    new.ord,
                    role,
                    new.name,
                    new.purpose,
                    new.provider,
                    new.model,
                    new.reasoning,
                    new.zone,
                    new.prompt_preset,
                    new.prompt_md,
                    new.context_window,
                    i64::from(new.read_only),
                    i64::from(new.enabled),
                    at
                ],
            )
            .map_err(|e| duplicate_agent(e, &role))?;
            Ok(tx.last_insert_rowid())
        })?;

        self.agent(id)
    }

    pub fn agent(&self, id: i64) -> Result<Agent> {
        self.db()
            .conn()
            .query_row(
                &format!("{AGENT_SELECT} WHERE id = ?1"),
                params![id],
                agent_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchAgent(id.to_string()))
    }

    pub fn agents(&self, team_id: i64) -> Result<Vec<Agent>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{AGENT_SELECT} WHERE team_id = ?1 ORDER BY ord, role"
        ))?;
        let rows = stmt
            .query_map(params![team_id], agent_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Replace the configurable part of a seat while keeping its identity and history.
    /// Put a seat on another model and nothing else: its zone, instructions and tools
    /// stay, and the old model's context window goes, because it says nothing about the new
    /// one's.
    pub fn move_seat(
        &mut self,
        agent_id: i64,
        provider: crate::model::Provider,
        model: &str,
    ) -> Result<Agent> {
        let mut update = NewAgent::from(&self.agent(agent_id)?);
        update.provider = provider;
        update.model = model.to_string();
        update.context_window = None;
        self.update_agent(agent_id, update)
    }

    pub fn update_agent(&mut self, agent_id: i64, mut update: NewAgent) -> Result<Agent> {
        normalise_agent(&mut update)?;
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx
                .execute(
                    "UPDATE agent
                        SET ord = ?2, role = ?3, name = ?4, purpose = ?5, provider = ?6,
                            model = ?7, reasoning = ?8, zone = ?9, prompt_preset = ?10,
                            prompt_md = ?11, context_window = ?12, read_only = ?13,
                            enabled = ?14, rev = rev + 1, updated_at = ?15
                      WHERE id = ?1",
                    params![
                        agent_id,
                        update.ord,
                        update.role,
                        update.name,
                        update.purpose,
                        update.provider,
                        update.model,
                        update.reasoning,
                        update.zone,
                        update.prompt_preset,
                        update.prompt_md,
                        update.context_window,
                        i64::from(update.read_only),
                        i64::from(update.enabled),
                        at,
                    ],
                )
                .map_err(|error| duplicate_agent(error, &update.role))?;
            if changed == 0 {
                return Err(Error::NoSuchAgent(agent_id.to_string()));
            }
            Ok(())
        })?;
        self.agent(agent_id)
    }

    pub fn delete_agent(&mut self, agent_id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            let changed = tx.execute("DELETE FROM agent WHERE id = ?1", params![agent_id])?;
            if changed == 0 {
                return Err(Error::NoSuchAgent(agent_id.to_string()));
            }
            Ok(())
        })
    }

    /// Repoint one seat at a different model. The commonest edit there is, and the one
    /// M1-S4's demo triggers a rebuild from.
    pub fn set_agent_model(
        &mut self,
        agent_id: i64,
        provider: Provider,
        model: &str,
    ) -> Result<Agent> {
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err(Error::invalid("a model name cannot be empty"));
        }
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE agent
                    SET provider = ?2, model = ?3,
                        context_window = CASE WHEN provider = ?2 THEN context_window ELSE NULL END,
                        rev = rev + 1, updated_at = ?4
                  WHERE id = ?1",
                params![agent_id, provider, model, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchAgent(agent_id.to_string()));
            }
            Ok(())
        })?;
        self.agent(agent_id)
    }

    /// Record what context window this agent's model has.
    ///
    /// Not cosmetic: eve refuses to compile compaction for a model it cannot size, and
    /// it can only size AI Gateway IDs - which D8 guarantees ai-team never uses.
    pub fn set_agent_context_window(
        &mut self,
        agent_id: i64,
        tokens: Option<i64>,
    ) -> Result<Agent> {
        if let Some(tokens) = tokens {
            if tokens < 1024 {
                return Err(Error::invalid(format!(
                    "a {tokens}-token context window is not usable - did you mean {}?",
                    tokens * 1024
                )));
            }
        }
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE agent SET context_window = ?2, rev = rev + 1, updated_at = ?3
                  WHERE id = ?1",
                params![agent_id, tokens, at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchAgent(agent_id.to_string()));
            }
            Ok(())
        })?;
        self.agent(agent_id)
    }

    pub fn set_agent_enabled(&mut self, agent_id: i64, enabled: bool) -> Result<Agent> {
        let at = now();
        self.db_mut().write(|tx| {
            let changed = tx.execute(
                "UPDATE agent SET enabled = ?2, rev = rev + 1, updated_at = ?3 WHERE id = ?1",
                params![agent_id, i64::from(enabled), at],
            )?;
            if changed == 0 {
                return Err(Error::NoSuchAgent(agent_id.to_string()));
            }
            Ok(())
        })?;
        self.agent(agent_id)
    }

    pub fn set_tool_policy(
        &mut self,
        agent_id: i64,
        tool: &str,
        effect: ToolEffect,
        note: Option<&str>,
    ) -> Result<()> {
        // Produce NoSuchAgent rather than a generic foreign-key error, and do it before
        // validating the rule so callers can distinguish stale identity from bad input.
        self.agent(agent_id)?;
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(Error::invalid("a tool policy needs a tool name or glob"));
        }
        let at = now();
        self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO agent_tool_policy (agent_id, tool, effect, note, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (agent_id, tool)
                 DO UPDATE SET effect = excluded.effect, note = excluded.note",
                params![agent_id, tool, effect, note.map(str::trim), at],
            )?;
            Ok(())
        })
    }

    pub fn tool_policies(&self, agent_id: i64) -> Result<Vec<ToolPolicy>> {
        self.agent(agent_id)?;
        let mut stmt = self.db().conn().prepare(
            "SELECT tool, effect, note FROM agent_tool_policy
              WHERE agent_id = ?1 ORDER BY tool",
        )?;
        let policies = stmt
            .query_map(params![agent_id], |row| {
                Ok(ToolPolicy {
                    tool: row.get(0)?,
                    effect: row.get(1)?,
                    note: non_empty(row.get(2)?),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(policies)
    }

    pub fn remove_tool_policy(&mut self, agent_id: i64, tool: &str) -> Result<bool> {
        self.agent(agent_id)?;
        self.db_mut().write(|tx| {
            Ok(tx.execute(
                "DELETE FROM agent_tool_policy WHERE agent_id = ?1 AND tool = ?2",
                params![agent_id, tool.trim()],
            )? != 0)
        })
    }

    /// The effective answer for one tool: deny beats allow, and an agent with no rule
    /// for a tool falls back to `default_allow` rather than to "anything goes".
    pub fn tool_allowed(&self, agent_id: i64, tool: &str, default_allow: bool) -> Result<bool> {
        let mut stmt = self
            .db()
            .conn()
            .prepare("SELECT tool, effect FROM agent_tool_policy WHERE agent_id = ?1")?;
        let rules: Vec<(String, ToolEffect)> = stmt
            .query_map(params![agent_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let mut decision = None;
        for (pattern, effect) in rules {
            if !zone_matches(&pattern, tool) && pattern != tool {
                continue;
            }
            // A single deny is final, whatever else matched.
            if effect == ToolEffect::Deny {
                return Ok(false);
            }
            decision = Some(true);
        }
        Ok(decision.unwrap_or(default_allow))
    }

    /// Which seat owns this path? The orchestrator's dispatch question (M2-S8).
    ///
    /// Disabled seats and read-only seats are skipped: neither can take the work.
    /// The seat whose zone claims a path most specifically.
    ///
    /// Most specific rather than first: a seat with a catch-all zone would otherwise
    /// starve every other seat purely by having a lower `ord`, and which agent edits a
    /// file would depend on the order the roster was created in. `ord` remains the
    /// tie-break, so two equally specific claims still resolve the same way every time.
    pub fn agent_for_path(&self, team_id: i64, path: &str) -> Result<Option<Agent>> {
        Ok(self
            .agents(team_id)?
            .into_iter()
            .filter(|a| a.enabled && !a.read_only)
            .filter_map(|a| crate::util::zone_specificity(&a.zone, path).map(|rank| (rank, a)))
            .max_by_key(|(rank, a)| (*rank, -a.ord))
            .map(|(_, a)| a))
    }

    /// Copy a team, its seats and their tool policies onto another project (M2-S7).
    pub fn clone_team(&mut self, team_id: i64, onto_project: i64, name: &str) -> Result<Team> {
        let source = self.team(team_id)?;
        let agents = self.agents(team_id)?;
        // Resolve the target first so a typo cannot leave behind an unattached clone.
        self.project(onto_project)?;
        // `ait init` already seeded the destination a team, so the obvious name is
        // usually taken. Suffixing keeps the zero-flag form working and keeps the old
        // team intact - it is somebody's configuration until they say otherwise.
        let name = self.free_team_name(name)?;
        let mut clone = self.create_team(Some(onto_project), &name, source.guardrails)?;
        if !source.description.is_empty() {
            clone = self.update_team(
                clone.id,
                &clone.name,
                &source.description,
                source.guardrails,
            )?;
        }
        clone = self.update_delivery(clone.id, source.delivery)?;

        for agent in agents {
            let created = self.add_agent(clone.id, NewAgent::from(&agent))?;
            // The policies travel with the seat. A cloned team whose tool rules were
            // left behind would be quietly more permissive than the one it came from.
            let mut stmt = self
                .db()
                .conn()
                .prepare("SELECT tool, effect, note FROM agent_tool_policy WHERE agent_id = ?1")?;
            let rules: Vec<(String, ToolEffect, Option<String>)> = stmt
                .query_map(params![agent.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for (tool, effect, note) in rules {
                self.set_tool_policy(created.id, &tool, effect, note.as_deref())?;
            }
        }

        // The clone is immediately the destination project's active team; otherwise
        // `ait agents generate --project <to>` would keep rebuilding the old roster.
        self.set_project_team(onto_project, Some(clone.id))?;
        self.team(clone.id)
    }
}

fn normalise_agent(agent: &mut NewAgent) -> Result<()> {
    agent.role = agent.role.trim().to_string();
    if agent.role.is_empty() {
        return Err(Error::invalid("an agent needs a role"));
    }
    if slugify(&agent.role) != agent.role {
        return Err(Error::invalid(
            "an agent role must be a lowercase slug (for example `release-manager`)",
        ));
    }
    agent.name = agent.name.trim().to_string();
    if agent.name.is_empty() {
        return Err(Error::invalid("an agent needs a display name"));
    }
    agent.model = agent.model.trim().to_string();
    if agent.model.is_empty() {
        return Err(Error::invalid("a model name cannot be empty"));
    }
    if agent.ord < 0 {
        return Err(Error::invalid("an agent's order cannot be negative"));
    }
    if let Some(tokens) = agent.context_window {
        if tokens < 1024 {
            return Err(Error::invalid(format!(
                "a {tokens}-token context window is not usable - did you mean {}?",
                tokens * 1024
            )));
        }
    }

    agent.prompt_preset = agent
        .prompt_preset
        .take()
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty());
    agent.prompt_md = agent
        .prompt_md
        .take()
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty());
    match (&agent.prompt_preset, &agent.prompt_md) {
        (Some(_), Some(_)) => {
            return Err(Error::invalid(
                "an agent must use either a prompt preset or a custom prompt, not both",
            ));
        }
        (None, None) => {
            return Err(Error::invalid(format!(
                "agent {:?} has neither a prompt preset nor a custom prompt",
                agent.role
            )));
        }
        _ => {}
    }
    Ok(())
}

const TEAM_SELECT: &str = "SELECT id, project_id, slug, name, description, parallel_width, \
     budget_tokens_run, budget_tokens_node, budget_seconds_run, budget_seconds_node, \
     max_turns_node, max_repairs, on_failure, push_policy, pr_policy, merge_policy, rev, \
     created_at, updated_at FROM team";

fn team_from_row(r: &Row<'_>) -> rusqlite::Result<Team> {
    Ok(Team {
        id: r.get(0)?,
        project_id: r.get(1)?,
        slug: r.get(2)?,
        name: r.get(3)?,
        description: r.get(4)?,
        guardrails: Guardrails {
            parallel_width: r.get(5)?,
            budget_tokens_run: r.get(6)?,
            budget_tokens_node: r.get(7)?,
            budget_seconds_run: r.get(8)?,
            budget_seconds_node: r.get(9)?,
            max_turns_node: r.get(10)?,
            max_repairs: r.get(11)?,
            on_failure: r.get(12)?,
        },
        delivery: DeliverySettings {
            push: r.get::<_, DeliveryPolicy>(13)?,
            pr: r.get::<_, DeliveryPolicy>(14)?,
            merge: r.get::<_, DeliveryPolicy>(15)?,
        },
        rev: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
    })
}

const AGENT_SELECT: &str = "SELECT id, team_id, ord, role, name, purpose, provider, model, \
     reasoning, zone, prompt_preset, prompt_md, context_window, read_only, enabled, rev, \
     created_at, updated_at FROM agent";

fn agent_from_row(r: &Row<'_>) -> rusqlite::Result<Agent> {
    Ok(Agent {
        id: r.get(0)?,
        team_id: r.get(1)?,
        ord: r.get(2)?,
        role: r.get(3)?,
        name: r.get(4)?,
        purpose: r.get(5)?,
        provider: r.get(6)?,
        model: r.get(7)?,
        reasoning: r.get(8)?,
        zone: r.get(9)?,
        prompt_preset: non_empty(r.get(10)?),
        prompt_md: non_empty(r.get(11)?),
        context_window: r.get(12)?,
        read_only: r.get::<_, i64>(13)? != 0,
        enabled: r.get::<_, i64>(14)? != 0,
        rev: r.get(15)?,
        created_at: r.get(16)?,
        updated_at: r.get(17)?,
    })
}

fn duplicate_team(err: rusqlite::Error, slug: &str) -> Error {
    if is_unique_violation(&err) {
        return Error::DuplicateTeam(slug.to_string());
    }
    err.into()
}

fn duplicate_agent(err: rusqlite::Error, role: &str) -> Error {
    if is_unique_violation(&err) {
        return Error::DuplicateAgent(role.to_string());
    }
    err.into()
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: 2067 | 1555,
            },
            _
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, Reasoning};

    fn project(s: &mut Store, name: &str) -> i64 {
        s.create_project(NewProject {
            name: name.into(),
            ..Default::default()
        })
        .unwrap()
        .id
    }

    #[test]
    fn seeding_gives_a_project_a_team_of_six() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();

        let agents = s.agents(team.id).unwrap();
        assert_eq!(agents.len(), 6);
        assert_eq!(
            agents.iter().map(|a| a.role.as_str()).collect::<Vec<_>>(),
            [
                "orchestrator",
                "planner",
                "backend",
                "frontend",
                "verifier",
                "reviewer"
            ]
        );
        // And the project points back, so the demo needs no query to find it.
        assert_eq!(s.project(p).unwrap().team_id, Some(team.id));
    }

    #[test]
    fn publishing_defaults_to_asking_at_each_boundary_and_is_explicitly_editable() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();
        assert_eq!(team.delivery, DeliverySettings::default());

        let changed = s
            .update_delivery(
                team.id,
                DeliverySettings {
                    push: DeliveryPolicy::Auto,
                    pr: DeliveryPolicy::Ask,
                    merge: DeliveryPolicy::Manual,
                },
            )
            .unwrap();
        assert_eq!(changed.delivery.push, DeliveryPolicy::Auto);
        assert_eq!(changed.delivery.pr, DeliveryPolicy::Ask);
        assert_eq!(changed.delivery.merge, DeliveryPolicy::Manual);
    }

    #[test]
    fn one_role_per_team() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();

        let again = s.add_agent(
            team.id,
            crate::roles::preset("backend").unwrap().to_new_agent(9),
        );
        assert!(matches!(again, Err(Error::DuplicateAgent(_))), "{again:?}");
    }

    #[test]
    fn teams_and_agents_can_be_updated_and_deleted_without_orphans() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();
        let updated_team = s
            .update_team(team.id, "Delivery crew", "Ships widgets.", team.guardrails)
            .unwrap();
        assert_eq!(updated_team.slug, "delivery-crew");
        assert_eq!(updated_team.description, "Ships widgets.");

        let backend = s
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap();
        let mut config = NewAgent::from(&backend);
        config.role = "services".into();
        config.reasoning = Reasoning::High;
        config.prompt_preset = None;
        config.prompt_md = Some("Keep public APIs backwards-compatible.".into());
        config.context_window = Some(128_000);
        let services = s.update_agent(backend.id, config).unwrap();
        assert_eq!(services.role, "services");
        assert_eq!(services.reasoning, Reasoning::High);
        assert_eq!(services.context_window, Some(128_000));
        assert_eq!(
            services.prompt_md.as_deref(),
            Some("Keep public APIs backwards-compatible.")
        );

        s.delete_agent(services.id).unwrap();
        assert!(matches!(s.agent(services.id), Err(Error::NoSuchAgent(_))));
        s.delete_team(team.id).unwrap();
        assert_eq!(s.project(p).unwrap().team_id, None);
        assert!(matches!(s.team(team.id), Err(Error::NoSuchTeam(_))));
    }

    /// Deleting a seat or a whole team must not rewrite what already happened. The
    /// cascades that make this true are declared in the schema and enforced only because
    /// the connection turns `foreign_keys` on, so both halves are asserted here.
    #[test]
    fn deleting_a_team_keeps_the_runs_it_already_did() {
        use crate::model::{NodeStatus, RunTrigger};

        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();
        let backend = s
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap();
        s.set_tool_policy(backend.id, "bash", ToolEffect::Deny, None)
            .unwrap();

        let run = s.create_run(p, "ship it", RunTrigger::Manual).unwrap();
        let node = s
            .dispatch(
                run.id,
                backend.id,
                None,
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        s.set_node_status(node.id, NodeStatus::Done).unwrap();

        s.delete_team(team.id).unwrap();

        // The seat and its rules go; the evidence of what it did does not.
        assert!(matches!(s.agent(backend.id), Err(Error::NoSuchAgent(_))));
        let node = s.node_run(node.id).unwrap();
        assert_eq!(node.role, "backend", "a finished run still says who did it");
        assert_eq!(
            node.agent_id, None,
            "but no longer points at a deleted seat"
        );
        assert_eq!(s.run(run.id).unwrap().prompt, "ship it");
    }

    #[test]
    fn an_agent_needs_instructions_from_somewhere() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.create_team(Some(p), "T", Guardrails::default()).unwrap();

        let bad = s.add_agent(
            team.id,
            NewAgent {
                role: "backend".into(),
                name: "B".into(),
                purpose: String::new(),
                provider: Provider::Local,
                model: "m".into(),
                reasoning: Reasoning::Medium,
                zone: String::new(),
                prompt_preset: None,
                prompt_md: None,
                context_window: None,
                read_only: false,
                enabled: true,
                ord: 0,
            },
        );
        assert!(bad.is_err());
    }

    #[test]
    fn dispatch_finds_the_seat_that_owns_the_path() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();

        let backend = s
            .agent_for_path(team.id, "crates/ai-team-core/src/db.rs")
            .unwrap();
        assert_eq!(backend.unwrap().role, "backend");
        let frontend = s.agent_for_path(team.id, "ui/src/App.tsx").unwrap();
        assert_eq!(frontend.unwrap().role, "frontend");
        // Nobody owns it: the orchestrator has to decide, not guess.
        assert!(s
            .agent_for_path(team.id, "docs/README.md")
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_read_only_seat_is_never_dispatched_work() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();

        // Give the verifier the backend's zone. It still must not be picked: a checker
        // that edits the thing it is checking is just a second maker.
        let verifier = s
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "verifier")
            .unwrap();
        s.db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE agent SET zone = 'crates/**' WHERE id = ?1",
                    params![verifier.id],
                )?;
                Ok(())
            })
            .unwrap();

        let owner = s
            .agent_for_path(team.id, "crates/x/src/lib.rs")
            .unwrap()
            .unwrap();
        assert_eq!(owner.role, "backend");
    }

    #[test]
    fn a_disabled_seat_is_skipped() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();
        let backend = s
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();

        s.set_agent_enabled(backend.id, false).unwrap();
        assert!(s
            .agent_for_path(team.id, "crates/x/src/lib.rs")
            .unwrap()
            .is_none());
    }

    /// The window describes the model, so it cannot outlive a move to another account.
    /// Relies on SQL evaluating every SET expression against the pre-update row.
    #[test]
    fn changing_account_drops_a_context_window_that_described_the_old_model() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();
        let agent = s.agents(team.id).unwrap().into_iter().next().unwrap();
        s.set_agent_context_window(agent.id, Some(32_768)).unwrap();

        // Same account, different model: the operator's number is still theirs to keep.
        let same = s
            .set_agent_model(agent.id, agent.provider, "other")
            .unwrap();
        assert_eq!(same.context_window, Some(32_768));

        // Another account entirely: 32k was a fact about the model left behind.
        let other = if agent.provider == Provider::Claude {
            Provider::OpenAi
        } else {
            Provider::Claude
        };
        let moved = s.set_agent_model(agent.id, other, "other").unwrap();
        assert_eq!(moved.context_window, None);
    }

    #[test]
    fn deny_beats_allow_and_an_unmatched_tool_falls_back() {
        let mut s = Store::memory().unwrap();
        let p = project(&mut s, "Widget");
        let team = s.seed_default_team(p).unwrap();
        let agent = s.agents(team.id).unwrap().into_iter().next().unwrap();

        s.set_tool_policy(agent.id, "bash*", ToolEffect::Allow, None)
            .unwrap();
        s.set_tool_policy(agent.id, "bash_root", ToolEffect::Deny, Some("never"))
            .unwrap();

        assert_eq!(s.tool_policies(agent.id).unwrap().len(), 2);
        assert!(s.tool_allowed(agent.id, "bash_read", false).unwrap());
        assert!(!s.tool_allowed(agent.id, "bash_root", false).unwrap());
        // No rule at all: the caller's default decides, not the absence of a rule.
        assert!(!s.tool_allowed(agent.id, "write", false).unwrap());
        assert!(s.tool_allowed(agent.id, "write", true).unwrap());
        assert!(s.remove_tool_policy(agent.id, "bash_root").unwrap());
        assert!(!s.remove_tool_policy(agent.id, "bash_root").unwrap());
        assert!(s.tool_allowed(agent.id, "bash_root", false).unwrap());
    }

    #[test]
    fn a_cloned_team_carries_its_seats_and_their_tool_rules() {
        let mut s = Store::memory().unwrap();
        let from = project(&mut s, "Widget");
        let onto = project(&mut s, "Gadget");
        let mut source = s.seed_default_team(from).unwrap();
        source = s
            .update_team(
                source.id,
                &source.name,
                "The portable team description.",
                source.guardrails,
            )
            .unwrap();

        let backend = s
            .agents(source.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        s.set_agent_model(backend.id, Provider::ZAi, "glm-4.6")
            .unwrap();
        s.set_agent_context_window(backend.id, Some(128_000))
            .unwrap();
        s.set_agent_enabled(backend.id, false).unwrap();
        s.set_tool_policy(backend.id, "bash_root", ToolEffect::Deny, Some("never"))
            .unwrap();

        let clone = s.clone_team(source.id, onto, "Gadget team").unwrap();
        assert_eq!(clone.project_id, Some(onto));
        assert_eq!(clone.guardrails, source.guardrails);
        assert_eq!(clone.description, source.description);
        assert_eq!(s.project(onto).unwrap().team_id, Some(clone.id));

        let cloned_backend = s
            .agents(clone.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        assert_eq!(cloned_backend.provider, Provider::ZAi);
        assert_eq!(cloned_backend.model, "glm-4.6");
        assert_eq!(cloned_backend.context_window, Some(128_000));
        assert!(!cloned_backend.enabled);
        assert!(!s
            .tool_allowed(cloned_backend.id, "bash_root", true)
            .unwrap());
    }

    #[test]
    fn a_team_may_be_a_template_with_no_project() {
        let mut s = Store::memory().unwrap();
        let template = s
            .create_team(None, "House style", Guardrails::default())
            .unwrap();
        assert!(template.project_id.is_none());

        let onto = project(&mut s, "Widget");
        let clone = s.clone_team(template.id, onto, "Widget team").unwrap();
        assert_eq!(clone.project_id, Some(onto));
    }
}
