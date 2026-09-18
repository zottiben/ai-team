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
pub(crate) async fn run() {
    println!("ai-team {}", core::VERSION);

    line(
        "data directory",
        core::data_dir().as_deref().map(|p| (p, "")),
    );

    line(
        "database",
        core::default_db_path()
            .as_deref()
            .map(|p| (p, exists(p, "present", "not created yet - run `ait init`"))),
    );

    // Reported separately from the file's existence: a database that is there but a
    // migration behind is the case that produces a confusing error later.
    if let Ok(path) = core::default_db_path() {
        if path.exists() {
            match core::Store::open(&path) {
                Ok(store) => println!(
                    "  {:<18} v{} · {} project(s) · {} view(s)",
                    "schema",
                    store.schema_version().unwrap_or(0),
                    store.projects().map_or(0, |p| p.len()),
                    store.views().map_or(0, |v| v.len()),
                ),
                Err(e) => println!("  {:<18} unreadable: {e}", "schema"),
            }
        }
    }

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

    let loaded = core::ModelRegistry::load();
    match &loaded {
        Ok(registry) => {
            for status in registry.statuses() {
                println!(
                    "  {:<18} {:<11} {}",
                    format!("provider {}", status.provider),
                    status.state.as_str(),
                    status.detail
                );
            }
        }
        Err(error) => {
            for provider in core::Provider::ALL {
                println!(
                    "  {:<18} {:<11} {error}",
                    format!("provider {provider}"),
                    "denied"
                );
            }
        }
    }

    report_context(loaded.as_ref().ok());

    // The neighbours ai-team borrows rather than absorbs (D4). Absent is not fatal - a
    // single-node `ait run --worktree` needs neither - so each says what it blocks.
    for (label, outcome, blocks) in [
        (
            "ai-planner",
            core::Planner::at(".").check().await,
            "planning and dispatch",
        ),
        (
            "ai-worktree",
            core::Worktrees::at(".").check().await,
            "leasing a worktree per slice",
        ),
    ] {
        match outcome {
            Ok(version) => println!("  {label:<18} {version}"),
            Err(_) => println!("  {label:<18} not installed - {blocks} is unavailable"),
        }
    }

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

/// Read-only context sources (D9).
///
/// Denied is the default, so a source missing here means this machine has not opted in
/// rather than something being broken. Allowed but tokenless is worth saying too: the
/// connection is generated and its seats will fail at the first call.
fn report_context(registry: Option<&core::ModelRegistry>) {
    for source in core::ContextSource::ALL {
        let allowed = registry.is_some_and(|registry| registry.context_sources().contains(source));
        let env = format!("AI_TEAM_{}_TOKEN", source.as_str().to_uppercase());
        let state = if allowed {
            if std::env::var(&env).is_ok_and(|value| !value.trim().is_empty()) {
                format!("allowed     read-only, {env} is set")
            } else {
                format!("allowed     but {env} is not set, so its seats cannot reach it")
            }
        } else {
            "denied      blocked by machine.toml".to_string()
        };
        println!("  {:<18} {state}", format!("context {source}"));
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
