//! `ait stats` - which agent-model pairing is earning its seat, in the terminal.
//!
//! The same rollup the window renders. There are no costs here on purpose: D8 makes every
//! provider a flat subscription, so the scarce resource is rate limit rather than money.

use ai_team_core::{By, Result, Row, Store};

pub(crate) fn run(args: crate::cli::StatsArgs) -> Result<()> {
    let store = Store::open_default()?;
    let project = match &args.project {
        Some(slug) => Some(store.find_project(slug)?.id),
        None => None,
    };
    let rows = ai_team_core::rollup(&store, args.by, project)?;

    if rows.is_empty() {
        println!("No finished runs yet - there is nothing to compare.");
        return Ok(());
    }

    // The group column is sized to its contents so the numbers line up in one block
    // rather than drifting right as a role name gets longer.
    let width = rows
        .iter()
        .map(|row| row.group.chars().count())
        .max()
        .unwrap_or(8)
        .max(8);

    println!(
        "{:<width$}  {:>9}  {:>6}  {:>6}  {:>7}  {:>8}  {:>10}  {:>9}  {:>6}",
        head(args.by),
        "accepted",
        "rework",
        "gates",
        "cycle",
        "input",
        "per change",
        "cache hit",
        "yield"
    );
    for row in &rows {
        println!(
            "{:<width$}  {:>9}  {:>6}  {:>6}  {:>7}  {:>8}  {:>10}  {:>9}  {:>6}",
            row.group,
            format!(
                "{} {}/{}",
                pct(row.accepted_rate()),
                row.accepted,
                row.accepted + row.rejected
            ),
            num(row.rework(), 2),
            pct(row.gate_pass_rate()),
            duration(row.cycle_time()),
            tokens(Some(approx(row.total_input()))),
            tokens(row.input_per_accepted()),
            pct(row.cache_hit_rate()),
            num(row.yield_per_k(), 1)
        );
    }

    // Said out loud rather than left in a column, because it is the finding somebody
    // opened this page for and it is easy to scan past.
    let cold: Vec<&Row> = rows
        .iter()
        .filter(|row| row.cache_hit_rate().is_some_and(|rate| rate < 0.25))
        .collect();
    if !cold.is_empty() {
        println!();
        for row in cold {
            println!(
                "note: {} reads almost none of its context back from cache - {} of rate limit spent on prefix",
                row.group,
                tokens(Some(approx(row.cache_write)))
            );
        }
    }
    Ok(())
}

fn head(by: By) -> &'static str {
    match by {
        By::Agent => "agent",
        By::Model => "model",
        By::Team => "team",
        By::Project => "project",
    }
}

/// The one place a count becomes a float, so the formatter below takes a single type.
///
/// These are token counts: a run would have to spend nine quadrillion of them before the
/// mantissa mattered, and the result is rendered to the nearest thousand.
#[allow(clippy::cast_precision_loss)]
fn approx(value: i64) -> f64 {
    value as f64
}

/// A ratio nobody has earned yet is a dash, never a zero.
fn pct(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{:.0}%", value * 100.0))
}

fn num(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{value:.digits$}"))
}

fn tokens(value: Option<f64>) -> String {
    match value {
        None => "-".into(),
        Some(value) if value >= 1_000_000.0 => format!("{:.1}M", value / 1_000_000.0),
        Some(value) if value >= 1_000.0 => format!("{:.0}k", value / 1_000.0),
        Some(value) => format!("{value:.0}"),
    }
}

fn duration(seconds: Option<f64>) -> String {
    match seconds {
        None => "-".into(),
        Some(s) if s >= 3_600.0 => format!("{:.1}h", s / 3_600.0),
        Some(s) if s >= 60.0 => format!("{:.0}m", s / 60.0),
        Some(s) => format!("{s:.0}s"),
    }
}
