//! `ait doctor` - what this install actually is.
//!
//! Every line here answers a question that has otherwise cost someone an afternoon:
//! which database am I looking at, is there a machine profile, and did this binary get
//! a real frontend bundle or the apology page.

use std::path::Path;

use ai_team_core as core;

/// Infallible on purpose: every line reports what it found, including "unavailable", so
/// there is nothing left for a caller to handle. A doctor that can itself fail is a
/// doctor you cannot run when things are broken.
pub(crate) fn run() {
    println!("ai-team {}", core::VERSION);

    line(
        "data directory",
        core::data_dir().as_deref().map(|p| (p, "")),
    );

    // "not created yet" is the expected state until M0-S2 creates the schema, so it is
    // reported as a fact rather than dressed up as a problem.
    line(
        "database",
        core::default_db_path()
            .as_deref()
            .map(|p| (p, exists(p, "present", "not created yet"))),
    );

    line(
        "machine profile",
        core::machine_profile_path().as_deref().map(|p| {
            (
                p,
                exists(
                    p,
                    "present",
                    "absent - every provider is denied until it exists",
                ),
            )
        }),
    );

    let bundle = ai_team_ui::bundle();
    if bundle.embedded {
        println!("  {:<18} {} files compiled in", "frontend", bundle.files);
    } else {
        println!(
            "  {:<18} missing - `ait ui` will serve a placeholder. Rebuild with node \
             available, or run `cd ui && npm ci && npm run build` first.",
            "frontend"
        );
    }
}

fn line(label: &str, value: Result<(&Path, &str), &core::Error>) {
    match value {
        Ok((path, "")) => println!("  {label:<18} {}", path.display()),
        Ok((path, note)) => println!("  {label:<18} {} ({note})", path.display()),
        Err(e) => println!("  {label:<18} unavailable: {e}"),
    }
}

fn exists<'a>(path: &Path, yes: &'a str, no: &'a str) -> &'a str {
    if path.exists() {
        yes
    } else {
        no
    }
}
