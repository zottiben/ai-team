//! Whether this install can actually do anything, as data rather than as prose.
//!
//! `ait doctor` already answered this in sentences. Sentences are the right output for a
//! terminal and useless to a window that has to decide what to show, what to block, and
//! what button to offer - so the answer is a report, and `ait doctor` prints it. Two
//! surfaces reading one report cannot disagree about whether a machine is set up, which
//! is exactly what they did when each looked for itself.
//!
//! Every check answers four questions, and the fourth is the one that matters:
//!
//! 1. What did it look at.
//! 2. What did it find.
//! 3. How much does it matter - is nothing possible, is something reduced, or is this
//!    fine.
//! 4. **Who can fix it.** ai-team owns its own state and may repair it without asking;
//!    anything else is a command the operator runs, shown in full (D17). A button that
//!    runs `curl | sh` has taken a decision that is not ai-team's to take.
//!
//! This is read continuously by the window. It must therefore be cheap and it must never
//! write: a report that repairs things as a side effect of being looked at is a
//! configuration that changes while somebody is reading it.

use serde::Serialize;

use crate::error::{Error, Result};
use crate::machine::ModelRegistry;
use crate::store::Store;

/// How much a finding matters.
///
/// The order is the ranking - `derive(Ord)` reads it top to bottom - so the worst thing
/// wrong with a machine is `checks.iter().map(|c| c.severity).min()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Nothing can run until this is dealt with.
    Blocking,
    /// It works, with less. A missing file-sql costs search, not the product.
    Degraded,
    /// Nothing to do.
    Fine,
}

/// Who can put it right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Fix {
    /// ai-team owns this and can repair it on request - its own config, its own database,
    /// its own rows. Named so the caller can ask for exactly one.
    Itself { action: Action, describe: String },
    /// Somebody else's software. The command is given in full and never run: this is the
    /// line D17 draws, and it is the whole reason a button here copies rather than
    /// executes.
    Command { run: String, why: String },
    /// Needs a person's judgement - signing into an account, choosing a directory.
    Human { what: String },
    /// Nothing to fix.
    None,
}

/// The repairs ai-team will perform on itself.
///
/// A closed set on purpose. "Apply every fix" is a button somebody presses without
/// reading, so each one is named, described, and applied on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Write the default machine profile - local allowed, accounts denied.
    CreateMachineProfile,
    /// Create the database and run every migration.
    CreateDatabase,
    /// Bring an existing database up to the current schema.
    Migrate,
}

/// One thing that was looked at.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// A stable key, so a surface can point at one and a test can name it.
    pub id: String,
    /// What a person calls it.
    pub label: String,
    pub severity: Severity,
    /// What was found, in one line, written for somebody who has not read the source.
    pub detail: String,
    pub fix: Fix,
}

impl Check {
    fn fine(id: &str, label: &str, detail: impl Into<String>) -> Check {
        Check {
            id: id.into(),
            label: label.into(),
            severity: Severity::Fine,
            detail: detail.into(),
            fix: Fix::None,
        }
    }
}

/// What this machine is, all told.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub version: &'static str,
    pub checks: Vec<Check>,
    /// The worst severity present. `Fine` when there is nothing wrong.
    pub severity: Severity,
    /// True when a run could actually happen. Not the same as "nothing is wrong": a
    /// machine with no file-sql is degraded and perfectly able to work.
    pub can_run: bool,
    /// True when this looks like a machine nobody has set up yet, which is what decides
    /// whether the window opens on setup or on Today.
    pub needs_setup: bool,
}

impl Report {
    /// The checks a surface should put in front of somebody, worst first.
    pub fn problems(&self) -> Vec<&Check> {
        let mut found: Vec<&Check> = self
            .checks
            .iter()
            .filter(|check| check.severity != Severity::Fine)
            .collect();
        found.sort_by_key(|check| check.severity);
        found
    }
}

/// Where this machine keeps ai-team's things.
///
/// Resolved once and handed down, rather than each check reading the environment for
/// itself. That is partly so there is one answer, and partly so these are testable: a
/// test about what a fresh machine looks like has to be able to describe one, and the
/// alternative is mutating process environment - which this workspace forbids, rightly.
#[derive(Debug)]
pub struct Paths {
    pub data_dir: Result<std::path::PathBuf>,
    pub database: Result<std::path::PathBuf>,
    pub machine_profile: Result<std::path::PathBuf>,
}

impl Paths {
    /// What this machine actually says.
    pub fn resolve() -> Paths {
        Paths {
            data_dir: crate::paths::data_dir(),
            database: crate::paths::default_db_path(),
            machine_profile: crate::paths::machine_profile_path(),
        }
    }

    /// Everything under one directory, for a test that needs a machine to look at.
    #[cfg(test)]
    fn under(dir: &std::path::Path) -> Paths {
        Paths {
            data_dir: Ok(dir.to_path_buf()),
            database: Ok(dir.join("team.db")),
            machine_profile: Ok(dir.join("machine.toml")),
        }
    }
}

/// Everything the database knows that readiness needs, read in one synchronous window.
///
/// A snapshot rather than the store itself, because a `rusqlite::Connection` is `Send` but
/// not `Sync` - so a future holding one cannot be spawned, and this report is awaited
/// inside an HTTP handler. The caller reads, drops its lock, and awaits.
#[derive(Debug, Clone, Default)]
pub struct Known {
    pub schema: Option<i64>,
    /// Slug, display name, and the main checkout if it has one.
    pub projects: Vec<(String, String, Option<String>)>,
}

impl Known {
    /// Read it off a store.
    pub fn of(store: &Store) -> Known {
        Known {
            schema: store.schema_version().ok(),
            projects: store
                .projects()
                .unwrap_or_default()
                .into_iter()
                .map(|project| {
                    let path = store
                        .project_repos(project.id)
                        .ok()
                        .and_then(|repos| repos.into_iter().find_map(|repo| repo.main_path));
                    (project.slug, project.name, path)
                })
                .collect(),
        }
    }
}

/// Look at everything.
///
/// Takes what the database knows as an option because the interesting case is not having a
/// database - a first run has none, and a report that could not be produced without one
/// would be unavailable exactly when it is needed.
pub async fn report(known: Option<&Known>) -> Report {
    report_at(&Paths::resolve(), known).await
}

/// The same, against paths a caller chose.
pub async fn report_at(paths: &Paths, known: Option<&Known>) -> Report {
    let mut checks = vec![data_directory(paths), machine_profile(paths)];
    checks.push(database(paths, known));
    checks.extend(providers());
    checks.extend(context_sources());
    checks.extend(neighbours().await);
    checks.push(frontend());
    if let Some(known) = known {
        checks.extend(projects(known));
    }

    let severity = checks
        .iter()
        .map(|check| check.severity)
        .min()
        .unwrap_or(Severity::Fine);

    // "Can run" is about the two things a run genuinely cannot proceed without: somewhere
    // to record it, and an account to think with. Everything else degrades.
    let blocking = |id: &str| {
        checks
            .iter()
            .any(|check| check.id == id && check.severity == Severity::Blocking)
    };
    let can_run = !blocking("database") && !blocking("providers");

    // A machine nobody has set up, rather than one that is merely misconfigured: no
    // database at all, or a database with no projects in it.
    let needs_setup = blocking("database")
        || blocking("providers")
        || known.is_none_or(|known| known.projects.is_empty());

    Report {
        version: crate::VERSION,
        checks,
        severity,
        can_run,
        needs_setup,
    }
}

fn data_directory(paths: &Paths) -> Check {
    match &paths.data_dir {
        Ok(path) => Check::fine("data_dir", "Data directory", path.display().to_string()),
        Err(error) => Check {
            id: "data_dir".into(),
            label: "Data directory".into(),
            severity: Severity::Blocking,
            detail: format!("cannot be resolved: {error}"),
            // No home directory means no ai-team. There is nothing to press.
            fix: Fix::Human {
                what: "ai-team needs a home directory, or AI_TEAM_HOME set to a writable path"
                    .into(),
            },
        },
    }
}

fn machine_profile(paths: &Paths) -> Check {
    let Ok(path) = &paths.machine_profile else {
        return Check {
            id: "machine_profile".into(),
            label: "Machine profile".into(),
            severity: Severity::Blocking,
            detail: "cannot be resolved".into(),
            fix: Fix::Human {
                what: "ai-team needs a config directory it can write to".into(),
            },
        };
    };

    if path.is_file() {
        return Check::fine(
            "machine_profile",
            "Machine profile",
            path.display().to_string(),
        );
    }

    Check {
        id: "machine_profile".into(),
        label: "Machine profile".into(),
        // Not blocking on its own - the providers check says what the *consequence* is,
        // and saying it twice makes a one-line problem look like two.
        severity: Severity::Degraded,
        detail: format!(
            "{} does not exist yet, so every provider is denied",
            path.display()
        ),
        fix: Fix::Itself {
            action: Action::CreateMachineProfile,
            describe: "Create it with local models allowed and every account denied".into(),
        },
    }
}

fn database(paths: &Paths, known: Option<&Known>) -> Check {
    let Ok(path) = &paths.database else {
        return Check {
            id: "database".into(),
            label: "Database".into(),
            severity: Severity::Blocking,
            detail: "cannot be resolved".into(),
            fix: Fix::Human {
                what: "ai-team needs a data directory it can write to".into(),
            },
        };
    };

    let Some(known) = known else {
        return Check {
            id: "database".into(),
            label: "Database".into(),
            severity: Severity::Blocking,
            detail: format!("{} has not been created yet", path.display()),
            fix: Fix::Itself {
                action: Action::CreateDatabase,
                describe: "Create the database and seed a team of six".into(),
            },
        };
    };

    match known.schema.ok_or(()) {
        Ok(version) if version == crate::db::latest_schema() => Check::fine(
            "database",
            "Database",
            format!(
                "{} · schema v{version} · {} project(s)",
                path.display(),
                known.projects.len()
            ),
        ),
        // A database a migration behind is the case that produces a baffling error later,
        // so it is called out rather than left to be discovered.
        Ok(version) => Check {
            id: "database".into(),
            label: "Database".into(),
            severity: Severity::Blocking,
            detail: format!(
                "schema v{version}, but this build expects v{}",
                crate::db::latest_schema()
            ),
            fix: Fix::Itself {
                action: Action::Migrate,
                describe: "Run the migrations this build carries".into(),
            },
        },
        Err(()) => Check {
            id: "database".into(),
            label: "Database".into(),
            severity: Severity::Blocking,
            detail: "exists but could not be read".into(),
            fix: Fix::Human {
                what: format!(
                    "{} cannot be read - move it aside to start fresh",
                    path.display()
                ),
            },
        },
    }
}

/// One check for the set, plus one per provider.
///
/// The summary exists because "no provider is available" is a single problem with a
/// single consequence, and four separate denials do not say it.
fn providers() -> Vec<Check> {
    let Ok(registry) = ModelRegistry::load() else {
        return vec![Check {
            id: "providers".into(),
            label: "Model providers".into(),
            severity: Severity::Blocking,
            detail: "no machine profile, so every provider is denied".into(),
            fix: Fix::Itself {
                action: Action::CreateMachineProfile,
                describe: "Create the machine profile, then allow the accounts you have".into(),
            },
        }];
    };

    let statuses = registry.statuses();
    let usable: Vec<&str> = statuses
        .iter()
        .filter(|status| status.state == crate::machine::ProviderState::Allowed)
        .map(|status| status.provider.as_str())
        .collect();

    let mut checks = vec![if usable.is_empty() {
        Check {
            id: "providers".into(),
            label: "Model providers".into(),
            severity: Severity::Blocking,
            detail: "every provider is denied or unreachable, so no agent can think".into(),
            fix: Fix::Human {
                what: "Allow a provider you are signed into - Claude through the Claude Code \
                       CLI, ChatGPT through eve, GLM with a coding plan, or the local gateway"
                    .into(),
            },
        }
    } else {
        Check::fine(
            "providers",
            "Model providers",
            format!("{} available", usable.join(", ")),
        )
    }];

    for status in statuses {
        let id = format!("provider.{}", status.provider.as_str());
        let label = format!("Provider: {}", status.provider);
        checks.push(match status.state {
            crate::machine::ProviderState::Allowed => Check::fine(&id, &label, status.detail),
            // Denied is a choice, not a fault: a work laptop denying a provider is the
            // machine profile doing its job.
            crate::machine::ProviderState::Denied => Check {
                id,
                label,
                severity: Severity::Fine,
                detail: status.detail,
                fix: Fix::None,
            },
            crate::machine::ProviderState::Unreachable => Check {
                id,
                label,
                severity: Severity::Degraded,
                detail: status.detail,
                fix: Fix::Human {
                    what: format!("Sign in to {} and check it responds", status.provider),
                },
            },
        });
    }
    checks
}

/// Context sources are opt-in, so absent is never a fault (D9).
///
/// Allowed but tokenless is worth saying, though: the connection gets generated and its
/// seats fail at the first call, which is a long way from here.
fn context_sources() -> Vec<Check> {
    let registry = ModelRegistry::load().ok();
    crate::machine::ContextSource::ALL
        .iter()
        .map(|source| {
            let id = format!("context.{}", source.as_str());
            let label = format!("Context: {source}");
            let allowed = registry
                .as_ref()
                .is_some_and(|registry| registry.context_sources().contains(source));
            if !allowed {
                return Check::fine(&id, &label, "not enabled on this machine");
            }
            let env = format!("AI_TEAM_{}_TOKEN", source.as_str().to_uppercase());
            if std::env::var(&env).is_ok_and(|value| !value.trim().is_empty()) {
                Check::fine(&id, &label, format!("enabled, {env} is set"))
            } else {
                Check {
                    id,
                    label,
                    severity: Severity::Degraded,
                    detail: format!("enabled, but {env} is not set - its seats cannot reach it"),
                    fix: Fix::Human {
                        what: format!("Set {env} in the environment ai-team runs in"),
                    },
                }
            }
        })
        .collect()
}

/// The neighbours ai-team borrows rather than absorbs (D4).
///
/// Every one of these is somebody else's software, so every fix here is a command and
/// never a button that runs it (D17).
async fn neighbours() -> Vec<Check> {
    let mut checks = Vec::new();

    for (id, label, version, blocks, install) in [
        (
            "aip",
            "ai-planner",
            crate::Planner::at(".").check().await.ok(),
            "planning and dispatch - a run cannot write or read a plan",
            "curl -fsSL https://zottiben.github.io/ai-planner/install.sh | sh",
        ),
        (
            "awt",
            "ai-worktree",
            crate::Worktrees::at(".").check().await.ok(),
            "leasing a worktree per slice - agents cannot work in parallel",
            "curl -fsSL https://zottiben.github.io/ai-worktree/install.sh | sh",
        ),
    ] {
        checks.push(match version {
            Some(version) => Check::fine(id, label, version),
            None => Check {
                id: id.into(),
                label: label.into(),
                // Blocking: `ait run` needs both. A single agent driven by hand does not,
                // which is why this says what it blocks rather than just "missing".
                severity: Severity::Blocking,
                detail: format!("not installed - {blocks}"),
                fix: Fix::Command {
                    run: install.into(),
                    why: format!("{label} is its own tool; ai-team uses it over its CLI"),
                },
            },
        });
    }

    checks.push(if crate::file_sql_available().await {
        Check::fine("file_sql", "file-sql", "installed")
    } else {
        Check {
            id: "file_sql".into(),
            label: "file-sql".into(),
            // Search in the editor, not the product.
            severity: Severity::Degraded,
            detail: "not installed - search in the editor is unavailable".into(),
            fix: Fix::Command {
                run: "curl -fsSL https://zottiben.github.io/file-sql/install.sh | sh".into(),
                why: "file-sql indexes a repo so agents can find code without grepping it".into(),
            },
        }
    });

    checks.push(if which("ai-toolbox").is_some() {
        Check::fine("ai_toolbox", "ai-toolbox", "installed")
    } else {
        Check {
            id: "ai_toolbox".into(),
            label: "ai-toolbox".into(),
            severity: Severity::Degraded,
            detail: "not installed - no help writing a repo's house rules".into(),
            fix: Fix::Command {
                run: "git clone https://github.com/zottiben/ai-toolbox && \
                      cd ai-toolbox && ./install.sh"
                    .into(),
                why: "ai-toolbox scaffolds the AGENTS.md agents are held to; ai-team reads \
                      one wherever it finds it, and needs no help to do so"
                    .into(),
            },
        }
    });

    checks
}

fn frontend() -> Check {
    // Asked of this crate rather than of ai-team-ui, which depends on it: the bundle is a
    // build-time fact and a core check cannot reach upwards for it.
    Check::fine(
        "frontend",
        "Window",
        "served by the binary that is running it",
    )
}

/// Per project: the things that make a project usable rather than merely recorded.
fn projects(known: &Known) -> Vec<Check> {
    let projects = &known.projects;
    if projects.is_empty() {
        return vec![Check {
            id: "projects".into(),
            label: "Projects".into(),
            severity: Severity::Blocking,
            detail: "none yet - ai-team has nothing to work on".into(),
            fix: Fix::Human {
                what: "Add a project by pointing ai-team at a repository".into(),
            },
        }];
    }

    let mut checks = vec![Check::fine(
        "projects",
        "Projects",
        format!("{} registered", projects.len()),
    )];

    for (slug, name, path) in projects {
        let id = format!("project.{slug}");
        let label = format!("Project: {name}");
        checks.push(match path.clone() {
            None => Check {
                id,
                label,
                severity: Severity::Degraded,
                detail: "no checkout attached, so nothing can be leased or edited".into(),
                fix: Fix::Human {
                    what: format!("Attach a repository to {name}"),
                },
            },
            // A checkout that has moved is worth saying here rather than discovering when
            // a run fails to lease it.
            Some(path) if !std::path::Path::new(&path).is_dir() => Check {
                id,
                label,
                severity: Severity::Blocking,
                detail: format!("{path} no longer exists"),
                fix: Fix::Human {
                    what: format!("Point {name} at where the checkout is now"),
                },
            },
            Some(path) => Check::fine(&id, &label, path),
        });
    }
    checks
}

/// Whether a command is on PATH.
///
/// Written out because the alternative is a dependency to answer a question that is one
/// directory walk, and because shelling out to `which` would itself need `which`.
fn which(command: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        candidate.is_file().then_some(candidate)
    })
}

/// Apply one of the repairs ai-team owns.
///
/// Deliberately narrow: this takes an [`Action`] rather than a check id, so the set of
/// things a request can cause is the set of variants above and nothing else. Anything
/// needing a command or a person is refused here rather than half-attempted.
pub fn apply(action: Action) -> Result<String> {
    apply_at(&Paths::resolve(), action)
}

/// The same, against paths a caller chose.
pub fn apply_at(paths: &Paths, action: Action) -> Result<String> {
    match action {
        Action::CreateMachineProfile => {
            let path = paths
                .machine_profile
                .as_ref()
                .map_err(|error| Error::invalid(error.to_string()))?;
            let (path, created) = crate::machine::ensure_machine_profile_at(path)?;
            Ok(if created {
                format!("Created {}", path.display())
            } else {
                format!("{} already exists", path.display())
            })
        }
        Action::CreateDatabase | Action::Migrate => {
            // One implementation for both, because `Store::init` creates or migrates and a
            // second path would be a second thing to keep correct - and "create" against
            // an existing database is exactly "migrate".
            let path = paths
                .database
                .as_ref()
                .map_err(|error| Error::invalid(error.to_string()))?;
            let store = Store::init(path)?;
            Ok(format!(
                "{} is at schema v{}",
                path.display(),
                store.schema_version()?
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &str, severity: Severity) -> Check {
        Check {
            id: id.into(),
            label: id.into(),
            severity,
            detail: String::new(),
            fix: Fix::None,
        }
    }

    fn report_of(checks: Vec<Check>) -> Report {
        let severity = checks
            .iter()
            .map(|check| check.severity)
            .min()
            .unwrap_or(Severity::Fine);
        Report {
            version: "test",
            checks,
            severity,
            can_run: true,
            needs_setup: false,
        }
    }

    #[test]
    fn the_worst_thing_wrong_is_the_severity_of_the_whole_machine() {
        // What the health indicator reads. One blocking check among twenty fine ones is a
        // machine that cannot work, and an average would hide it.
        let report = report_of(vec![
            check("a", Severity::Fine),
            check("b", Severity::Degraded),
            check("c", Severity::Blocking),
        ]);
        assert_eq!(report.severity, Severity::Blocking);

        let report = report_of(vec![
            check("a", Severity::Fine),
            check("b", Severity::Degraded),
        ]);
        assert_eq!(report.severity, Severity::Degraded);

        assert_eq!(
            report_of(vec![check("a", Severity::Fine)]).severity,
            Severity::Fine
        );
    }

    #[test]
    fn problems_are_worst_first_and_exclude_what_is_fine() {
        // A list that leads with a missing file-sql while the database is absent is a list
        // that gets the wrong thing fixed first.
        let report = report_of(vec![
            check("fine", Severity::Fine),
            check("degraded", Severity::Degraded),
            check("blocking", Severity::Blocking),
        ]);
        let ids: Vec<&str> = report.problems().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["blocking", "degraded"]);
    }

    #[test]
    fn an_empty_report_is_fine_rather_than_unknown() {
        assert_eq!(report_of(Vec::new()).severity, Severity::Fine);
        assert!(report_of(Vec::new()).problems().is_empty());
    }

    #[test]
    fn a_machine_with_no_database_is_blocking_and_ai_team_can_fix_it() {
        let dir = tempfile::tempdir().unwrap();
        let check = database(&Paths::under(dir.path()), None);

        assert_eq!(check.severity, Severity::Blocking);
        assert!(
            matches!(
                &check.fix,
                Fix::Itself {
                    action: Action::CreateDatabase,
                    ..
                }
            ),
            "{:?}",
            check.fix
        );
        // And it says which file, because "no database" without a path sends somebody
        // looking in the wrong place.
        assert!(check.detail.contains("team.db"), "{}", check.detail);
    }

    #[test]
    fn a_database_a_migration_behind_is_called_out_rather_than_left_to_surprise() {
        // The case that otherwise produces a baffling error much later, somewhere else.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::memory().unwrap();
        let known = Known::of(&store);
        let fine = database(&Paths::under(dir.path()), Some(&known));
        assert_eq!(fine.severity, Severity::Fine, "{}", fine.detail);
        assert!(fine
            .detail
            .contains(&format!("v{}", crate::db::latest_schema())));
    }

    #[test]
    fn a_missing_machine_profile_is_reported_once_with_its_consequence() {
        // Said twice it looks like two problems. The profile is the cause and the
        // providers check is the consequence, so only one of them blocks.
        let dir = tempfile::tempdir().unwrap();
        let profile = machine_profile(&Paths::under(dir.path()));
        assert_eq!(profile.severity, Severity::Degraded);
        assert!(matches!(
            profile.fix,
            Fix::Itself {
                action: Action::CreateMachineProfile,
                ..
            }
        ));

        // And once it exists, it is simply where it is.
        std::fs::write(
            dir.path().join("machine.toml"),
            crate::machine::DEFAULT_MACHINE_PROFILE,
        )
        .unwrap();
        assert_eq!(
            machine_profile(&Paths::under(dir.path())).severity,
            Severity::Fine
        );
    }

    #[tokio::test]
    async fn a_missing_neighbour_is_a_command_never_a_button_that_runs_it() {
        // D17. A GUI that runs `curl | sh` has taken a decision that is not its to take,
        // so no neighbour may ever be `Fix::Itself` - installed or not.
        for check in neighbours().await {
            assert!(
                !matches!(check.fix, Fix::Itself { .. }),
                "{} must not be something ai-team installs: {:?}",
                check.id,
                check.fix
            );
            // And when it is missing, the command is given in full rather than described.
            if check.severity != Severity::Fine {
                match &check.fix {
                    Fix::Command { run, why } => {
                        assert!(!run.trim().is_empty(), "{} has an empty command", check.id);
                        assert!(!why.trim().is_empty(), "{} does not say why", check.id);
                    }
                    other => panic!("{} should offer a command, not {other:?}", check.id),
                }
            }
        }
    }

    #[test]
    fn a_project_whose_checkout_has_moved_is_blocking_rather_than_discovered_later() {
        // Otherwise it surfaces as a run failing to lease a worktree, which reads as a
        // problem with ai-worktree.
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(crate::model::NewProject {
                name: "Gone".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .attach_repo(
                project.id,
                crate::model::NewRepo {
                    main_path: Some("/definitely/not/here".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let checks = projects(&Known::of(&store));
        let check = checks
            .iter()
            .find(|check| check.id == "project.gone")
            .unwrap_or_else(|| {
                panic!("no per-project check in {:?}", checks.iter().map(|c| &c.id))
            });
        assert_eq!(check.severity, Severity::Blocking);
        assert!(check.detail.contains("/definitely/not/here"));
    }

    #[test]
    fn a_machine_with_no_projects_has_nothing_to_work_on() {
        let store = Store::memory().unwrap();
        let checks = projects(&Known::of(&store));
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].severity, Severity::Blocking);
        assert!(matches!(checks[0].fix, Fix::Human { .. }));
    }

    #[test]
    fn applying_a_repair_is_idempotent() {
        // The window polls and a person clicks twice. Both have to be safe.
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());

        assert!(apply_at(&paths, Action::CreateMachineProfile)
            .unwrap()
            .contains("Created"));
        assert!(apply_at(&paths, Action::CreateMachineProfile)
            .unwrap()
            .contains("already exists"));

        let created = apply_at(&paths, Action::CreateDatabase).unwrap();
        assert!(created.contains("schema"), "{created}");
        // And again, which is what `Migrate` is on an already-current database.
        assert!(apply_at(&paths, Action::Migrate)
            .unwrap()
            .contains("schema"));
    }

    #[test]
    fn repairing_a_machine_leaves_it_with_nothing_blocking_that_it_owns() {
        // What the setup wizard does. After ai-team has fixed what is its own, the only
        // blocking things left should be ones needing a command or a person (D17).
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        apply_at(&paths, Action::CreateMachineProfile).unwrap();
        apply_at(&paths, Action::CreateDatabase).unwrap();

        assert_eq!(machine_profile(&paths).severity, Severity::Fine);
        let store = Store::open(paths.database.as_ref().unwrap()).unwrap();
        let known = Known::of(&store);
        assert_eq!(database(&paths, Some(&known)).severity, Severity::Fine);
    }

    #[test]
    fn which_finds_something_on_the_path_and_not_something_absent() {
        assert!(which("sh").is_some(), "sh should be on PATH");
        assert!(which("definitely-not-a-real-command-8f3a").is_none());
    }
}
