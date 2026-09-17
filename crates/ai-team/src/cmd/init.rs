//! `ait init` - create the database, register a project, seed the team of six.
//!
//! Idempotent: running it twice in the same worktree updates the repo row rather than
//! adding a second project, because the commonest way to run it is by accident.

use std::path::Path;

use anyhow::{Context, Result};

use ai_team_core::{NewProject, NewRepo, ProjectKind, Store};

use crate::cli::InitArgs;

pub(crate) fn run(args: InitArgs) -> Result<()> {
    let (profile_path, profile_created) = ai_team_core::ensure_machine_profile()?;
    if profile_created {
        println!(
            "Created {} (local allowed; account providers denied)",
            profile_path.display()
        );
    }

    let db_path = ai_team_core::default_db_path()?;
    let fresh = !db_path.exists();
    let mut store =
        Store::init(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
    if fresh {
        println!("Created {}", db_path.display());
    }

    let cwd = std::env::current_dir().context("reading the current directory")?;
    let git = GitContext::discover(&cwd);

    // The name defaults to the repo's directory, because that is what the person
    // standing in it would call the project.
    let name = match args.name {
        Some(name) => name,
        None => git
            .as_ref()
            .and_then(|g| g.name.clone())
            .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "project".to_string()),
    };

    let kind = args.kind.unwrap_or(if git.is_some() {
        ProjectKind::Repo
    } else {
        // Not a repo, and D6 says that is fine - a project is a container.
        ProjectKind::Adhoc
    });

    // Running this twice in one worktree is the commonest way it gets run, so an
    // existing project is the expected case rather than an error.
    let project = if let Ok(existing) = store.find_project(&ai_team_core::slugify(&name)) {
        println!("Project {} already registered", existing.slug);
        existing
    } else {
        let created = store.create_project(NewProject {
            name: name.clone(),
            kind: Some(kind),
            ..Default::default()
        })?;
        println!("Registered {} ({})", created.slug, created.kind);
        created
    };

    if let Some(git) = &git {
        let repo = store.attach_repo(
            project.id,
            NewRepo {
                remote_url: git.remote.clone(),
                name: git.name.clone(),
                main_path: Some(git.root.to_string_lossy().into_owned()),
                default_branch: git.branch.clone(),
            },
        )?;
        println!("  repo             {} ({})", repo.name, repo.key);
    } else {
        println!("  repo             none - this project is not a checkout");
    }

    let team = if let Some(id) = project.team_id {
        store.team(id)?
    } else {
        let seeded = store.seed_default_team(project.id)?;
        println!("  team             {} seeded", seeded.name);
        seeded
    };

    for agent in store.agents(team.id)? {
        let access = if agent.read_only { "reads" } else { "writes" };
        println!(
            "    {:<14} {}/{}  {access}",
            agent.role, agent.provider, agent.model
        );
    }

    println!("\nNext:\n  ait doctor\n  ait db open      # the whole database, in TablePlus");
    Ok(())
}

struct GitContext {
    root: std::path::PathBuf,
    name: Option<String>,
    remote: Option<String>,
    branch: Option<String>,
}

impl GitContext {
    /// Best effort, and deliberately shells out rather than linking a git library: this
    /// asks the same git the user's own commands use, so worktrees, includes and
    /// conditional config all behave the way they already expect.
    fn discover(from: &Path) -> Option<GitContext> {
        let root = git(from, &["rev-parse", "--show-toplevel"])?;
        let root = std::path::PathBuf::from(root);
        Some(GitContext {
            name: root.file_name().map(|n| n.to_string_lossy().into_owned()),
            remote: git(from, &["remote", "get-url", "origin"]),
            branch: git(from, &["rev-parse", "--abbrev-ref", "HEAD"]),
            root,
        })
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
