//! Built-in project tooling. Scan-only registration and exact, single-use approvals.
//! Never discovers ai-toolbox's registry/root or executes its CLI.

mod catalogue;
mod effects;
mod files;
mod install;
mod migrate;
mod pi;
mod survey;
mod worktrees;

use std::path::Path;

use ai_toolbox_core::{Harness, Plan};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, Store};

pub use catalogue::{catalogue, Catalogue, Item, NOTICE, REVISION};
pub use effects::{Effect, Outcome};
pub use files::Node;
pub use worktrees::Worktree;

#[derive(Debug, Clone, Serialize)]
pub struct Scan {
    pub root: String,
    pub catalogue_revision: String,
    pub survey: Option<serde_json::Value>,
    pub problem: Option<String>,
    pub worktrees: Vec<Worktree>,
    pub worktree_problem: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    Install {
        harnesses: Vec<Harness>,
        hooks: Vec<String>,
        mcp: Vec<String>,
        skills: Vec<String>,
        scaffold: bool,
        #[serde(default)]
        no_symlink: bool,
        #[serde(default)]
        with_dotenv: bool,
    },
    Repair,
    Migrate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub id: i64,
    pub project_id: i64,
    pub root: String,
    pub catalogue_revision: String,
    pub effects: Vec<Effect>,
    pub warnings: Vec<String>,
    pub state: String,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: i64,
    pub root: String,
    pub state: String,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub frozen: effects::Frozen,
    pub catalogue_revision: String,
    pub warnings: Vec<String>,
}

/// A malformed or unsafe configuration is a scan result, not a failed registration.
/// No project writes, package installs or auth probes. Only bounded local Git
/// metadata is executed for linked-worktree inventory; no standalone discovery.
pub fn scan(root: &Path) -> Scan {
    let inspected = (|| -> Result<serde_json::Value> {
        let package = catalogue::Packaged::load()?;
        let snapshot = files::Snapshot::capture(root)?;
        let staged = snapshot.stage()?;
        let mut survey = serde_json::to_value(project_error(
            survey::read(staged.path(), &package.catalogue),
            staged.path(),
            &snapshot.root,
        )?)?;
        project_error(
            pi::survey(staged.path(), &mut survey),
            staged.path(),
            &snapshot.root,
        )?;
        remap_paths(&mut survey, staged.path(), &snapshot.root);
        snapshot.validate()?;
        Ok(survey)
    })();
    let (survey, problem) = match inspected {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let (worktrees, worktree_problem) = match worktrees::inspect(root) {
        Ok(trees) => (trees, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    Scan {
        worktrees,
        worktree_problem,
        root: root.to_string_lossy().into_owned(),
        catalogue_revision: REVISION.trim().into(),
        survey,
        problem,
    }
}

pub fn preview(
    store: &mut Store,
    project: i64,
    root: &str,
    selection: Selection,
) -> Result<Preview> {
    let root = store.toolbox_root(project, root)?;
    let package = catalogue::Packaged::load()?;
    let inputs = files::Snapshot::capture(&root)?;
    let stage = inputs.stage()?;
    let planned = (|| -> Result<Plan> {
        Ok(match selection {
            Selection::Install {
                harnesses,
                hooks,
                mcp,
                skills,
                scaffold,
                no_symlink,
                with_dotenv,
            } => {
                if harnesses.is_empty() {
                    return Err(Error::invalid("select at least one harness"));
                }
                // The engine predates the current adapter filename. Never import obsolete
                // overrides into a newly approved config simply because they are present.
                let old = stage.path().join(".pi/mcp.json");
                if old.is_file() {
                    std::fs::remove_file(old)?;
                }
                let mut plan = ai_toolbox_core::install::everything(
                    stage.path(),
                    &package.catalogue,
                    &harnesses,
                    &hooks,
                    &mcp,
                    &[],
                    scaffold,
                )?;
                install::skills(
                    &mut plan,
                    stage.path(),
                    &package.catalogue,
                    &skills,
                    &harnesses,
                    no_symlink,
                )?;
                if with_dotenv {
                    ai_toolbox_core::install::with_dotenv(
                        &mut plan,
                        stage.path(),
                        &package.catalogue,
                    )?;
                }
                pi::adapt(stage.path(), &mut plan)?;
                plan
            }
            Selection::Migrate => migrate::plan(&inputs, stage.path())?,
            Selection::Repair => {
                let survey = survey::read(stage.path(), &package.catalogue)?;
                let mut plan = Plan::default();
                ai_toolbox_core::doctor::repair(
                    &mut plan,
                    stage.path(),
                    &survey.inventory,
                    &survey.report,
                    &package.catalogue,
                    &survey.findings,
                )?;
                preserve_repaired_scripts(stage.path(), &mut plan)?;
                plan
            }
        })
    })();
    let mut plan = project_error(planned, stage.path(), &root)?;
    for secret in plan.secrets {
        plan.warnings.push(format!(
            "Manual credential setup: {}",
            serde_json::to_string(&secret)?
        ));
    }
    plan.warnings.push("Only these project files are approved. No software installation, credential entry, global configuration, commit or publication is performed. Filesystem changes are not a multi-file transaction; interrupted or partial outcomes require inspection.".into());
    let frozen = effects::Frozen::capture(inputs, stage.path(), plan.actions)?;
    store.save_toolbox_preview(
        project,
        &Saved {
            frozen,
            catalogue_revision: REVISION.trim().into(),
            warnings: plan.warnings,
        },
    )
}

pub fn apply(store: &mut Store, project: i64, id: i64) -> Result<Preview> {
    let saved = store.claim_toolbox_preview(project, id)?;
    let outcome = saved.frozen.apply();
    store.finish_toolbox_preview(project, id, &outcome)?;
    store.toolbox_preview(project, id)
}

fn preserve_repaired_scripts(root: &Path, plan: &mut Plan) -> Result<()> {
    for action in &mut plan.actions {
        if !action.path.starts_with(root.join(".agents/hooks"))
            && !action.path.starts_with(root.join(".agents/mcp"))
        {
            continue;
        }
        let ai_toolbox_core::action::Kind::Write { contents, .. } = &mut action.kind else {
            continue;
        };
        if !action.path.is_file() {
            continue;
        }
        let original = std::fs::read(&action.path)?;
        if *contents != original {
            contents.clone_from(&original);
            let path = files::relative(root, &action.path)?;
            action.summary =
                format!("repair wiring/permissions; preserve existing {path} contents");
            plan.warnings.push(format!("Kept local script contents in {path}. Replacing them requires selecting an explicit install, not repair."));
        }
    }
    Ok(())
}

fn project_error<T>(result: Result<T>, staged: &Path, root: &Path) -> Result<T> {
    result.map_err(|error| {
        Error::invalid(error.to_string().replace(
            staged.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
        ))
    })
}

fn remap_paths(value: &mut serde_json::Value, staged: &Path, root: &Path) {
    match value {
        serde_json::Value::String(s) => {
            if let Ok(rest) = Path::new(s).strip_prefix(staged) {
                *s = root.join(rest).to_string_lossy().into_owned();
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                remap_paths(item, staged, root);
            }
        }
        serde_json::Value::Object(items) => {
            for item in items.values_mut() {
                remap_paths(item, staged, root);
            }
        }
        _ => {}
    }
}
