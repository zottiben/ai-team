//! Teams and the seats on them.
//!
//! The team is the database (D2). Nothing here writes a file; the generated eve project
//! is derived from these rows in M1-S3, and a change made by hand under
//! `.ai-team/agents/` is a change that the next regeneration silently throws away.

use rusqlite::{params, OptionalExtension, Row};

use crate::error::{Error, Result};
use crate::model::{Agent, Guardrails, NewAgent, Provider, Team, ToolEffect};
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
        for (ord, preset) in (0i64..).zip(DEFAULT_ROSTER) {
            self.add_agent(team.id, preset.to_new_agent(ord))?;
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

    pub fn add_agent(&mut self, team_id: i64, new: NewAgent) -> Result<Agent> {
        let role = new.role.trim().to_string();
        if role.is_empty() {
            return Err(Error::invalid("an agent needs a role"));
        }
        if new.prompt_preset.is_none() && new.prompt_md.is_none() {
            return Err(Error::invalid(format!(
                "agent {role:?} has neither a prompt preset nor a prompt"
            )));
        }
        let at = now();

        let id = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO agent
                   (team_id, ord, role, name, purpose, provider, model, reasoning, zone,
                    prompt_preset, prompt_md, read_only, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?13)",
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
                    i64::from(new.read_only),
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
                "UPDATE agent SET provider = ?2, model = ?3, rev = rev + 1, updated_at = ?4
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
        let at = now();
        self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO agent_tool_policy (agent_id, tool, effect, note, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (agent_id, tool)
                 DO UPDATE SET effect = excluded.effect, note = excluded.note",
                params![agent_id, tool.trim(), effect, note, at],
            )?;
            Ok(())
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
    pub fn agent_for_path(&self, team_id: i64, path: &str) -> Result<Option<Agent>> {
        Ok(self
            .agents(team_id)?
            .into_iter()
            .find(|a| a.enabled && !a.read_only && zone_matches(&a.zone, path)))
    }

    /// Copy a team, its seats and their tool policies onto another project (M2-S7).
    pub fn clone_team(&mut self, team_id: i64, onto_project: i64, name: &str) -> Result<Team> {
        let source = self.team(team_id)?;
        let agents = self.agents(team_id)?;
        let clone = self.create_team(Some(onto_project), name, source.guardrails)?;

        for agent in agents {
            let created = self.add_agent(
                clone.id,
                NewAgent {
                    role: agent.role.clone(),
                    name: agent.name.clone(),
                    purpose: agent.purpose.clone(),
                    provider: agent.provider,
                    model: agent.model.clone(),
                    reasoning: agent.reasoning,
                    zone: agent.zone.clone(),
                    prompt_preset: agent.prompt_preset.clone(),
                    prompt_md: agent.prompt_md.clone(),
                    read_only: agent.read_only,
                    ord: agent.ord,
                },
            )?;
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

        self.team(clone.id)
    }
}

const TEAM_SELECT: &str = "SELECT id, project_id, slug, name, description, parallel_width, \
     budget_tokens_run, budget_tokens_node, budget_seconds_run, budget_seconds_node, \
     max_turns_node, max_repairs, on_failure, rev, created_at, updated_at FROM team";

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
        rev: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
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
                read_only: false,
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

        assert!(s.tool_allowed(agent.id, "bash_read", false).unwrap());
        assert!(!s.tool_allowed(agent.id, "bash_root", false).unwrap());
        // No rule at all: the caller's default decides, not the absence of a rule.
        assert!(!s.tool_allowed(agent.id, "write", false).unwrap());
        assert!(s.tool_allowed(agent.id, "write", true).unwrap());
    }

    #[test]
    fn a_cloned_team_carries_its_seats_and_their_tool_rules() {
        let mut s = Store::memory().unwrap();
        let from = project(&mut s, "Widget");
        let onto = project(&mut s, "Gadget");
        let source = s.seed_default_team(from).unwrap();

        let backend = s
            .agents(source.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        s.set_agent_model(backend.id, Provider::ZAi, "glm-4.6")
            .unwrap();
        s.set_tool_policy(backend.id, "bash_root", ToolEffect::Deny, Some("never"))
            .unwrap();

        let clone = s.clone_team(source.id, onto, "Gadget team").unwrap();
        assert_eq!(clone.project_id, Some(onto));
        assert_eq!(clone.guardrails, source.guardrails);

        let cloned_backend = s
            .agents(clone.id)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        assert_eq!(cloned_backend.provider, Provider::ZAi);
        assert_eq!(cloned_backend.model, "glm-4.6");
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
