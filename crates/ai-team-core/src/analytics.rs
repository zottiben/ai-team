//! Which agent-model pairing is actually earning its seat.
//!
//! There are no dollar costs to report. D8 makes every provider a flat subscription, so
//! the scarce resource is **rate limit**, not spend - which changes what is worth
//! measuring. A node that consumed four hundred thousand tokens of context to emit two
//! hundred lines is not expensive, it is *slow to come back to*, and it is the reason the
//! next run waits.
//!
//! Two things this deliberately does not do.
//!
//! It does not read prose. Gate and rejection events carry structured fields, and every
//! rate here comes from those or from `node_run`'s own columns - never from matching
//! English in a summary, which breaks the first time somebody improves the wording
//! (rule 8).
//!
//! And it never totals cached and uncached input. A Claude-bridged seat carries a large
//! prefix that is cached after its first call, so a sum makes every cold node look like a
//! runaway and every warm one look free. Kept apart, the same two numbers answer the
//! question the demo actually asks: which nodes are burning rate limit on prefix alone.

use serde::Serialize;

use crate::error::Result;
use crate::store::Store;

/// How the numbers are grouped. The same metrics, cut four ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    Agent,
    Model,
    Team,
    Project,
}

impl By {
    /// The SQL expression that names a group.
    ///
    /// `role` rather than `agent_id`: an agent can be renamed or deleted and a year-old
    /// run must still say who did the work, which is why the column is denormalised onto
    /// `node_run` in the first place.
    fn column(self) -> &'static str {
        match self {
            By::Agent => "n.role",
            By::Model => "n.provider || '/' || n.model",
            By::Team => "COALESCE(t.name, 'unknown')",
            By::Project => "p.name",
        }
    }
}

/// What one group of node runs cost and returned.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Row {
    pub group: String,

    /// Attempts at changing something. Verifier turns are excluded - a check is not a
    /// change, and counting it would dilute every rate below with work that was never
    /// meant to produce a diff.
    pub attempts: i64,
    /// Attempts that were accepted: gates passed, verifier agreed, work committed.
    pub accepted: i64,
    /// Attempts that were rejected or blocked.
    pub rejected: i64,

    /// Distinct slices this group got accepted.
    pub slices_accepted: i64,

    /// Input tokens that were *not* served from cache.
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cache_read: i64,
    pub cache_write: i64,

    /// Seconds from a node starting to it ending, summed over attempts.
    pub seconds: i64,
    /// Wall-clock seconds from the first attempt at a slice to the accepted one, summed
    /// over slices. Bigger than `seconds` whenever work sat between attempts.
    pub cycle_seconds: i64,

    pub gates_run: i64,
    pub gates_passed: i64,
}

/// `a / b`, or `None` when there is nothing to divide by.
///
/// The one place an `i64` becomes an `f64`. These are counts of turns and tokens: a run
/// would have to spend nine quadrillion of either before the mantissa mattered, and every
/// caller is rendering a percentage or a mean to one decimal place.
#[allow(clippy::cast_precision_loss)]
fn ratio(numerator: i64, denominator: i64) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

impl Row {
    /// Of the attempts that produced a verdict, how many were accepted.
    ///
    /// `None` rather than zero when nothing has finished: a pairing with no history has
    /// not earned a bad score, and rendering 0% would libel it.
    pub fn accepted_rate(&self) -> Option<f64> {
        ratio(self.accepted, self.accepted + self.rejected)
    }

    /// Attempts per accepted slice. 1.0 is first-time-right; 2.0 means one repair each.
    pub fn rework(&self) -> Option<f64> {
        ratio(self.attempts, self.slices_accepted)
    }

    /// Every input token this group asked the provider to process, cached or not.
    ///
    /// This is the rate-limit number. Whether a token was served from cache changes what
    /// it cost the account, but not that it had to be sent.
    pub fn total_input(&self) -> i64 {
        self.tokens_in + self.cache_read + self.cache_write
    }

    /// Input tokens per accepted change - the headline.
    pub fn input_per_accepted(&self) -> Option<f64> {
        ratio(self.total_input(), self.slices_accepted)
    }

    /// How much of the context this group sent was served from cache.
    ///
    /// The "cold node" signal. A seat that runs once per run pays to write its prefix and
    /// never reads it back, so this sits near zero while `total_input` stays large -
    /// rate limit spent on saying hello.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        ratio(self.cache_read, self.cache_read + self.cache_write)
    }

    /// Output tokens per thousand input. Low means a node read a great deal to say
    /// very little.
    pub fn yield_per_k(&self) -> Option<f64> {
        ratio(self.tokens_out * 1000, self.total_input())
    }

    pub fn gate_pass_rate(&self) -> Option<f64> {
        ratio(self.gates_passed, self.gates_run)
    }

    /// Mean seconds from first attempt to acceptance, per slice.
    pub fn cycle_time(&self) -> Option<f64> {
        ratio(self.cycle_seconds, self.slices_accepted)
    }
}

/// The metrics, grouped.
///
/// One query rather than one per metric: these are all counts over the same rows, and
/// running them separately would let two of them disagree about which attempts existed
/// if a run finished in between.
pub fn rollup(store: &Store, by: By, project_id: Option<i64>) -> Result<Vec<Row>> {
    let group = by.column();
    let sql = format!(
        "WITH maker AS (
             SELECT n.*, r.project_id
               FROM node_run n
               JOIN run r ON r.id = n.run_id
              -- A verifier checks; it does not change anything. Including its turns
              -- would dilute every rate below with work that never produced a diff.
              WHERE n.role <> 'verifier'
                AND (?1 IS NULL OR r.project_id = ?1)
         ),
         -- One row per slice a group actually landed, with how long it took from the
         -- first attempt at it. Computed separately because a slice spans attempts and
         -- the per-attempt sum below would count the gap between them as zero.
         -- Aliased `n` here and in `gates` because `{group}` is written in terms of
         -- `n`, `p` and `t`. Alias it anything else and the expression silently refers
         -- to the *outer* query's tables instead, which makes every group's subquery
         -- match every row.
         cycles AS (
             SELECT {group} AS grp,
                    n.slice_key,
                    MIN(n.started_at) AS first_start,
                    MIN(CASE WHEN n.status = 'done' THEN n.ended_at END) AS accepted_at
               FROM maker n
               JOIN run r  ON r.id = n.run_id
               JOIN project p ON p.id = r.project_id
          LEFT JOIN team t ON t.id = r.team_id
              WHERE n.slice_key IS NOT NULL
           GROUP BY grp, n.slice_key
             HAVING accepted_at IS NOT NULL
         ),
         gates AS (
             SELECT {group} AS grp,
                    COUNT(*) AS run_count,
                    SUM(CASE WHEN json_extract(e.payload_json, '$.passed') THEN 1 ELSE 0 END)
                        AS pass_count
               FROM event e
               JOIN maker n ON n.id = e.node_run_id
               JOIN run r  ON r.id = n.run_id
               JOIN project p ON p.id = r.project_id
          LEFT JOIN team t ON t.id = r.team_id
              WHERE json_extract(e.payload_json, '$.passed') IS NOT NULL
           GROUP BY grp
         )
         SELECT {group} AS grp,
                COUNT(*),
                SUM(CASE WHEN n.status = 'done' THEN 1 ELSE 0 END),
                SUM(CASE WHEN n.status IN ('failed','blocked','cancelled') THEN 1 ELSE 0 END),
                COUNT(DISTINCT CASE WHEN n.status = 'done' THEN n.slice_key END),
                SUM(n.tokens_in),
                SUM(n.tokens_out),
                SUM(n.tokens_cache_read),
                SUM(n.tokens_cache_write),
                CAST(COALESCE(SUM(
                    MAX(0, strftime('%s', n.ended_at) - strftime('%s', n.started_at))
                ), 0) AS INTEGER),
                -- `{group}` repeated rather than the `grp` alias: inside a correlated
                -- subquery an unqualified name binds to the subquery's own FROM, so
                -- `c.grp = grp` reads as `c.grp = c.grp` - always true, and every group
                -- silently gets the first row's numbers.
                COALESCE((SELECT CAST(SUM(
                    MAX(0, strftime('%s', c.accepted_at) - strftime('%s', c.first_start))
                ) AS INTEGER) FROM cycles c WHERE c.grp = {group}), 0),
                COALESCE((SELECT g.run_count FROM gates g WHERE g.grp = {group}), 0),
                COALESCE((SELECT g.pass_count FROM gates g WHERE g.grp = {group}), 0)
           FROM maker n
           JOIN run r  ON r.id = n.run_id
           JOIN project p ON p.id = r.project_id
      LEFT JOIN team t ON t.id = r.team_id
       GROUP BY grp
       ORDER BY 2 DESC, grp"
    );

    let conn = store.db().conn();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params![project_id], |row| {
            Ok(Row {
                group: row.get(0)?,
                attempts: row.get(1)?,
                accepted: row.get(2)?,
                rejected: row.get(3)?,
                slices_accepted: row.get(4)?,
                tokens_in: row.get(5)?,
                tokens_out: row.get(6)?,
                cache_read: row.get(7)?,
                cache_write: row.get(8)?,
                seconds: row.get(9)?,
                cycle_seconds: row.get(10)?,
                gates_run: row.get(11)?,
                gates_passed: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewEvent, NewProject, NodeStatus, RunTrigger, Usage};
    use crate::ModelRegistry;

    /// A project, a team and a run to hang node runs off.
    fn seeded() -> (Store, i64, i64, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store.seed_default_team(project.id).unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        (store, project.id, team.id, run.id)
    }

    fn seat(store: &Store, team: i64, role: &str) -> i64 {
        store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == role)
            .unwrap()
            .id
    }

    /// Backdate a node's clock.
    ///
    /// `set_node_status` stamps the real time, which is right for a running system and
    /// useless for asserting that a slice took half an hour. These tests live inside the
    /// crate so they can write the history they are measuring.
    fn clock(store: &mut Store, node: i64, started: &str, ended: &str) {
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE node_run SET started_at = ?2, ended_at = ?3 WHERE id = ?1",
                    rusqlite::params![node, started, ended],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// Dispatch a node and drive it to a finished state, with usage and a clock.
    fn attempt(
        store: &mut Store,
        run: i64,
        agent: i64,
        slice: &str,
        accepted: bool,
        when: (&str, &str),
        usage: (i64, i64, i64, i64),
    ) -> i64 {
        let node = store
            .dispatch(run, agent, Some(slice), &ModelRegistry::local_only())
            .unwrap();
        let (tokens_in, tokens_out, cache_read, cache_write) = usage;
        store
            .record_usage(
                node.id,
                Usage {
                    tokens_in,
                    tokens_out,
                    cache_read,
                    cache_write,
                },
                1,
            )
            .unwrap();
        store
            .set_node_status(
                node.id,
                if accepted {
                    NodeStatus::Done
                } else {
                    NodeStatus::Failed
                },
            )
            .unwrap();
        clock(store, node.id, when.0, when.1);
        node.id
    }

    fn gate(store: &mut Store, run: i64, node: i64, passed: bool) {
        store
            .append_event(
                run,
                NewEvent::new(
                    if passed {
                        crate::model::EventKind::Note
                    } else {
                        crate::model::EventKind::Failed
                    },
                    "gate test `cargo test` ran",
                )
                .on_node(node)
                .with(serde_json::json!({ "gate": "test", "passed": passed })),
            )
            .unwrap();
    }

    fn find<'a>(rows: &'a [Row], group: &str) -> &'a Row {
        rows.iter()
            .find(|row| row.group == group)
            .unwrap_or_else(|| panic!("no group {group} in {:?}", rows.iter().map(|r| &r.group)))
    }

    #[test]
    fn a_verifiers_turns_are_not_counted_as_changes() {
        // A check is not a change. The verifier runs on every attempt, so counting its
        // turns would dilute every rate below by roughly half with work that was never
        // meant to produce a diff.
        let (mut store, project, team, run) = seeded();
        let backend = seat(&store, team, "backend");
        let verifier = seat(&store, team, "verifier");
        attempt(
            &mut store,
            run,
            backend,
            "S1",
            true,
            ("2026-09-18T09:00:00Z", "2026-09-18T09:05:00Z"),
            (100, 50, 0, 0),
        );
        attempt(
            &mut store,
            run,
            verifier,
            "S1",
            true,
            ("2026-09-18T09:05:00Z", "2026-09-18T09:06:00Z"),
            (90_000, 20, 0, 0),
        );

        let rows = rollup(&store, By::Agent, Some(project)).unwrap();
        assert!(rows.iter().all(|row| row.group != "verifier"));
        assert_eq!(find(&rows, "backend").attempts, 1);
    }

    #[test]
    fn a_slice_that_took_three_attempts_is_still_one_accepted_change() {
        // What the headline metric rests on. Counting attempts as changes would reward a
        // seat for failing twice before getting it right.
        let (mut store, project, team, run) = seeded();
        let backend = seat(&store, team, "backend");
        for (accepted, when) in [
            (false, ("2026-09-18T09:00:00Z", "2026-09-18T09:10:00Z")),
            (false, ("2026-09-18T09:10:00Z", "2026-09-18T09:20:00Z")),
            (true, ("2026-09-18T09:20:00Z", "2026-09-18T09:30:00Z")),
        ] {
            attempt(
                &mut store,
                run,
                backend,
                "S1",
                accepted,
                when,
                (100, 50, 0, 0),
            );
        }

        let rows = rollup(&store, By::Agent, Some(project)).unwrap();
        let backend = find(&rows, "backend");
        assert_eq!(backend.attempts, 3);
        assert_eq!(backend.slices_accepted, 1);
        assert_eq!(backend.accepted_rate(), Some(1.0 / 3.0));
        assert_eq!(backend.rework(), Some(3.0));

        // Half an hour from the first attempt to acceptance, not three ten-minute
        // stretches added together.
        assert_eq!(backend.cycle_seconds, 1_800);
    }

    #[test]
    fn each_group_gets_its_own_cycle_time_not_everybodys() {
        // The bug this exists to catch was invisible with one group: every group was
        // handed the sum of *all* cycles, so a single-seat fixture looked perfectly
        // right. Two seats with deliberately different durations is the smallest shape
        // that can tell the difference.
        let (mut store, project, team, run) = seeded();
        let backend = seat(&store, team, "backend");
        let frontend = seat(&store, team, "frontend");

        // Ten minutes.
        attempt(
            &mut store,
            run,
            backend,
            "S1",
            true,
            ("2026-09-18T09:00:00Z", "2026-09-18T09:10:00Z"),
            (10, 10, 0, 0),
        );
        // An hour.
        attempt(
            &mut store,
            run,
            frontend,
            "S2",
            true,
            ("2026-09-18T09:00:00Z", "2026-09-18T10:00:00Z"),
            (10, 10, 0, 0),
        );

        let rows = rollup(&store, By::Agent, Some(project)).unwrap();
        assert_eq!(find(&rows, "backend").cycle_seconds, 600);
        assert_eq!(find(&rows, "frontend").cycle_seconds, 3_600);
    }

    #[test]
    fn cached_and_uncached_input_stay_apart_but_rate_limit_adds_them_up() {
        let (mut store, project, team, run) = seeded();
        let backend = seat(&store, team, "backend");
        attempt(
            &mut store,
            run,
            backend,
            "S1",
            true,
            ("2026-09-18T09:00:00Z", "2026-09-18T09:05:00Z"),
            (1_000, 800, 60_000, 4_000),
        );

        let rows = rollup(&store, By::Agent, Some(project)).unwrap();
        let backend = find(&rows, "backend");
        assert_eq!(
            (backend.tokens_in, backend.cache_read, backend.cache_write),
            (1_000, 60_000, 4_000)
        );
        assert_eq!(backend.total_input(), 65_000);
    }

    #[test]
    fn gates_are_counted_from_structured_fields_and_land_on_the_right_group() {
        // Read from `payload_json.passed`, never by matching English in the summary -
        // which would break the first time somebody improved the wording (rule 8).
        let (mut store, project, team, run) = seeded();
        let backend_seat = seat(&store, team, "backend");
        let frontend_seat = seat(&store, team, "frontend");
        let when = ("2026-09-18T09:00:00Z", "2026-09-18T09:05:00Z");
        let backend = attempt(
            &mut store,
            run,
            backend_seat,
            "S1",
            true,
            when,
            (10, 10, 0, 0),
        );
        let frontend = attempt(
            &mut store,
            run,
            frontend_seat,
            "S2",
            true,
            when,
            (10, 10, 0, 0),
        );

        gate(&mut store, run, backend, true);
        gate(&mut store, run, backend, true);
        gate(&mut store, run, backend, false);
        gate(&mut store, run, frontend, true);

        // An event with no `passed` field is not a gate, and must not be counted as a
        // failing one.
        store
            .append_event(
                run,
                NewEvent::new(crate::model::EventKind::Note, "dispatched")
                    .on_node(backend)
                    .with(serde_json::json!({ "reason": "something else entirely" })),
            )
            .unwrap();

        let rows = rollup(&store, By::Agent, Some(project)).unwrap();
        let back = find(&rows, "backend");
        assert_eq!((back.gates_run, back.gates_passed), (3, 2));
        assert_eq!(back.gate_pass_rate(), Some(2.0 / 3.0));

        let front = find(&rows, "frontend");
        assert_eq!((front.gates_run, front.gates_passed), (1, 1));
    }

    #[test]
    fn the_same_history_can_be_cut_by_model_as_well_as_by_seat() {
        // The point of the page: two seats on one model is a fact about the model, and
        // one seat across two models is the comparison worth making.
        let (mut store, project, team, run) = seeded();
        let when = ("2026-09-18T09:00:00Z", "2026-09-18T09:05:00Z");
        for (role, slice) in [("backend", "S1"), ("frontend", "S2")] {
            let agent = seat(&store, team, role);
            attempt(&mut store, run, agent, slice, true, when, (100, 50, 0, 0));
        }

        assert_eq!(rollup(&store, By::Agent, Some(project)).unwrap().len(), 2);

        // Both seats are on the same local model here, so by model they collapse to one
        // row carrying both attempts.
        let by_model = rollup(&store, By::Model, Some(project)).unwrap();
        assert_eq!(by_model.len(), 1);
        assert_eq!((by_model[0].attempts, by_model[0].slices_accepted), (2, 2));
    }

    #[test]
    fn another_projects_history_does_not_leak_in() {
        let (mut store, project, team, run) = seeded();
        let when = ("2026-09-18T09:00:00Z", "2026-09-18T09:05:00Z");
        let backend = seat(&store, team, "backend");
        attempt(&mut store, run, backend, "S1", true, when, (100, 50, 0, 0));

        let other = store
            .create_project(NewProject {
                name: "Other".into(),
                ..Default::default()
            })
            .unwrap();
        let other_team = store.seed_default_team(other.id).unwrap();
        let other_run = store
            .create_run(other.id, "something else", RunTrigger::Manual)
            .unwrap();
        let other_backend = seat(&store, other_team.id, "backend");
        attempt(
            &mut store,
            other_run.id,
            other_backend,
            "X1",
            false,
            when,
            (999_999, 1, 0, 0),
        );

        let scoped = rollup(&store, By::Agent, Some(project)).unwrap();
        assert_eq!(find(&scoped, "backend").attempts, 1);
        assert_eq!(find(&scoped, "backend").tokens_in, 100);

        // And unscoped sees both, which is what the "everything" view wants.
        let everything = rollup(&store, By::Agent, None).unwrap();
        assert_eq!(find(&everything, "backend").attempts, 2);
    }

    #[test]
    fn a_database_with_no_runs_is_no_rows_rather_than_an_error() {
        let store = Store::memory().unwrap();
        assert!(rollup(&store, By::Agent, None).unwrap().is_empty());
    }

    fn row(accepted: i64, rejected: i64, slices: i64) -> Row {
        Row {
            group: "backend".into(),
            attempts: accepted + rejected,
            accepted,
            rejected,
            slices_accepted: slices,
            tokens_in: 0,
            tokens_out: 0,
            cache_read: 0,
            cache_write: 0,
            seconds: 0,
            cycle_seconds: 0,
            gates_run: 0,
            gates_passed: 0,
        }
    }

    #[test]
    fn a_pairing_with_no_history_has_not_earned_a_bad_score() {
        // Rendering 0% for a seat that has never run would libel it, and the first thing
        // somebody would do is retire a model that had never been tried.
        let fresh = row(0, 0, 0);
        assert_eq!(fresh.accepted_rate(), None);
        assert_eq!(fresh.rework(), None);
        assert_eq!(fresh.input_per_accepted(), None);
        assert_eq!(fresh.cache_hit_rate(), None);
        assert_eq!(fresh.gate_pass_rate(), None);
    }

    #[test]
    fn accepted_rate_is_of_what_was_decided() {
        let r = row(3, 1, 3);
        assert_eq!(r.accepted_rate(), Some(0.75));
    }

    #[test]
    fn rework_counts_attempts_against_slices_landed() {
        // Four attempts to land two slices is one repair each.
        let r = row(2, 2, 2);
        assert_eq!(r.rework(), Some(2.0));
    }

    #[test]
    fn input_is_never_reported_as_one_number_but_rate_limit_is() {
        // Kept apart, because a Claude seat's prefix is cached after its first call and
        // a sum makes every cold node look like a runaway. Added up only where the
        // question is rate limit, which does not care where a token came from.
        let mut r = row(1, 0, 1);
        r.tokens_in = 1_000;
        r.cache_read = 60_000;
        r.cache_write = 4_000;
        assert_eq!(r.total_input(), 65_000);
        assert_eq!(r.input_per_accepted(), Some(65_000.0));
    }

    #[test]
    fn a_cold_node_shows_up_as_a_low_cache_hit_rate() {
        // The demo's second question. A seat that runs once per run pays to write its
        // prefix and never reads it back: rate limit spent on saying hello.
        let mut cold = row(1, 0, 1);
        cold.cache_write = 64_000;
        cold.cache_read = 0;
        assert_eq!(cold.cache_hit_rate(), Some(0.0));

        let mut warm = row(4, 0, 4);
        warm.cache_write = 64_000;
        warm.cache_read = 192_000;
        assert!(warm.cache_hit_rate().unwrap() > 0.7);
    }

    #[test]
    fn yield_says_how_much_was_read_to_say_how_little() {
        let mut r = row(1, 0, 1);
        r.cache_read = 400_000;
        r.tokens_out = 800;
        assert_eq!(r.yield_per_k(), Some(2.0));
    }

    #[test]
    fn cycle_time_is_per_slice_not_per_attempt() {
        // Three attempts over an hour to land one slice is an hour of cycle time, not
        // three separate short ones.
        let mut r = row(1, 2, 1);
        r.cycle_seconds = 3_600;
        assert_eq!(r.cycle_time(), Some(3_600.0));
    }
}
