//! `ait doctor` - what this install actually is.
//!
//! A view of the readiness report, not a second opinion. It used to look at the machine
//! for itself, which meant the terminal and the window could disagree about whether a
//! machine was set up - and they did. Everything here is formatting.

use ai_team_core as core;
use core::{Fix, Severity};

/// Infallible on purpose: every line reports what it found, including "unavailable", so
/// there is nothing left for a caller to handle. A doctor that can itself fail is a
/// doctor you cannot run when things are broken.
pub(crate) async fn run() {
    // Opened rather than required: the whole point of running this is often that there is
    // no database yet.
    let known = core::default_db_path()
        .ok()
        .and_then(|path| core::Store::open(&path).ok())
        .map(|store| core::Known::of(&store));
    let report = core::readiness_report(known.as_ref()).await;

    println!("ai-team {}", report.version);
    for check in &report.checks {
        println!(
            "  {:<20} {:<9} {}",
            check.label,
            mark(check.severity),
            check.detail
        );
    }

    let problems = report.problems();
    if problems.is_empty() {
        println!("\nEverything is set up.");
        return;
    }

    // The fixes are listed after the findings rather than beside them, because a command
    // buried in a column is a command nobody can copy.
    println!("\nWhat to do:");
    for check in problems {
        match &check.fix {
            Fix::Itself { describe, .. } => {
                // Named as something ai-team will do, and `ait init` is how to ask for it
                // from here - the window has a button.
                println!("  {}: {describe}", check.label);
                println!("    run `ait init`");
            }
            Fix::Command { run, why } => {
                println!("  {}: {why}", check.label);
                println!("    {run}");
            }
            Fix::Human { what } => println!("  {}: {what}", check.label),
            Fix::None => {}
        }
    }

    if !report.can_run {
        println!("\nNothing can run yet.");
    }
}

/// A word, not a colour: this is read over ssh and piped into files.
fn mark(severity: Severity) -> &'static str {
    match severity {
        Severity::Blocking => "blocking",
        Severity::Degraded => "degraded",
        Severity::Fine => "ok",
    }
}
