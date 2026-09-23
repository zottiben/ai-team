//! Registering a project, wherever the request comes from.
//!
//! This is what `ait init` does, minus the printing. It moved here when the window needed
//! to do the same thing, for the reason `run_workflow` moved: two implementations of
//! "register a project" drift, and the one that drifts is the one nobody demoed today.
//!
//! Idempotent throughout, because the commonest way this gets called is a second time -
//! by accident from the CLI, or by somebody pressing a button twice in a window that
//! polls.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::{NewProject, NewRepo, Project, ProjectKind};
use crate::store::Store;

/// One directory, as something to point at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub name: String,
    /// Absolute, so a surface can hand it straight back without joining anything - the
    /// joining is where a picker gets a path subtly wrong.
    pub path: String,
    /// True when this directory is the top of a git checkout.
    ///
    /// Only the top: a subdirectory of a repository registers the root anyway, but marking
    /// every directory inside one as a repository would make the whole tree look like a
    /// wall of equally good answers.
    pub repo: bool,
}

/// What is in a directory, and where it sits.
#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    /// Where this listing is, absolute and resolved.
    pub path: String,
    /// The directory above, or `None` at the root of the filesystem.
    pub parent: Option<String>,
    /// Subdirectories, by name.
    pub entries: Vec<Candidate>,
}

/// List the directories inside one, so a person can find a checkout by looking.
///
/// Directories only, and their names only. This exists so somebody can pick a project
/// without typing an absolute path from memory, which is not a reason to hand a window the
/// ability to enumerate somebody's documents - so there is nothing here about files, sizes
/// or contents.
///
/// An empty path means home, because that is where a person's checkouts are and `/` is a
/// list of things none of them is.
pub fn browse(path: &str) -> Result<Listing> {
    let path = path.trim();
    let dir = if path.is_empty() {
        crate::paths::home_dir()?
    } else {
        crate::paths::expand_user(Path::new(path))?
    };
    // Resolved rather than taken as given, so `..` in a request becomes a real directory
    // and the `parent` this hands back is the one the operating system agrees with. On
    // macOS this is also what turns `/tmp` into `/private/tmp` (D12).
    let dir = dir.canonicalize().map_err(|error| Error::UnusablePath {
        path: dir.clone(),
        reason: error.to_string(),
    })?;

    let mut entries: Vec<Candidate> = std::fs::read_dir(&dir)
        .map_err(|error| Error::UnusablePath {
            path: dir.clone(),
            reason: error.to_string(),
        })?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Dotfiles are noise here - nobody keeps a checkout in `.cache` - and `.git`
            // itself is a directory that would otherwise look like something to enter.
            if name.starts_with('.') {
                return None;
            }
            Some(Candidate {
                repo: entry.path().join(".git").exists(),
                path: entry.path().to_string_lossy().into_owned(),
                name,
            })
        })
        .collect();

    // Checkouts first, then by name folded to lowercase: this is a list somebody is
    // scanning for one thing they already know the name of, and the answer should be near
    // the top when it is a repository.
    entries.sort_by(|a, b| {
        b.repo
            .cmp(&a.repo)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Listing {
        parent: dir
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned()),
        path: dir.to_string_lossy().into_owned(),
        entries,
    })
}

/// What a directory turned out to be.
#[derive(Debug, Clone, Serialize)]
pub struct Registered {
    pub project: Project,
    /// The checkout attached, if the directory was one.
    pub repo_path: Option<String>,
    /// True when this call created the project rather than finding it.
    pub created: bool,
    /// True when this call seeded the team.
    pub seeded_team: bool,
    /// The seats, so a caller can show what it got without a second round trip.
    pub roster: Vec<(String, String, String)>,
}

/// Register a directory as a project.
///
/// A directory that is not a git repository is a project of a different kind rather than
/// an error (D6): a triage session across four services and a one-file chore are both
/// projects, and refusing the ones that are not checkouts would rule out half of what
/// ai-team is for.
pub fn register(
    store: &mut Store,
    dir: &Path,
    name: Option<&str>,
    kind: Option<ProjectKind>,
) -> Result<Registered> {
    let dir = crate::paths::expand_user(dir)?;
    let dir = dir.canonicalize().map_err(|error| Error::UnusablePath {
        path: dir.clone(),
        reason: error.to_string(),
    })?;
    if !dir.is_dir() {
        return Err(Error::invalid(format!(
            "{} is not a directory",
            dir.display()
        )));
    }

    let git = GitContext::discover(&dir);

    // The name defaults to the directory, because that is what somebody standing in it
    // would call the project.
    let name = match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => git
            .as_ref()
            .and_then(|found| found.name.clone())
            .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "project".to_string()),
    };

    // Given wins over inferred: a checkout can legitimately be a `ticket` or a `chore`,
    // and guessing `repo` because there happens to be a `.git` would override somebody who
    // said otherwise.
    let kind = kind.unwrap_or(if git.is_some() {
        ProjectKind::Repo
    } else {
        ProjectKind::Adhoc
    });

    // An existing project is the expected case, not an error.
    let slug = crate::util::slugify(&name);
    let (project, created) = match store.find_project(&slug) {
        Ok(existing) => (existing, false),
        Err(_) => (
            store.create_project(NewProject {
                name: name.clone(),
                kind: Some(kind),
                ..Default::default()
            })?,
            true,
        ),
    };

    let repo_path = match &git {
        Some(git) => {
            let repo = store.attach_repo(
                project.id,
                NewRepo {
                    remote_url: git.remote.clone(),
                    name: git.name.clone(),
                    main_path: Some(git.root.to_string_lossy().into_owned()),
                    default_branch: git.branch.clone(),
                },
            )?;
            repo.main_path
        }
        None => None,
    };

    let (team_id, seeded_team) = if let Some(id) = project.team_id {
        (id, false)
    } else {
        {
            let team = store.seed_default_team(project.id)?;
            // Zones from the repository rather than from the presets, which name *this*
            // project's directories. Only on a fresh team: a roster somebody has edited is
            // their configuration, and overwriting it because a directory appeared would be
            // ai-team deciding something that is not its to decide.
            if let Some(root) = &repo_path {
                let (backend, frontend) = zones_for(Path::new(root));
                for agent in store.agents(team.id)? {
                    let zone = match agent.role.as_str() {
                        "backend" => Some(&backend),
                        "frontend" => Some(&frontend),
                        _ => None,
                    };
                    // Applied even when empty: a seat with nothing to own should say so
                    // rather than keep a preset zone naming a directory this repository does
                    // not have, which would route a slice to somebody who then finds nothing.
                    if let Some(zone) = zone {
                        let mut update = crate::model::NewAgent::from(&agent);
                        update.zone.clone_from(zone);
                        store.update_agent(agent.id, update)?;
                    }
                }
            }
            (team.id, true)
        }
    };

    let roster = store
        .agents(team_id)?
        .into_iter()
        .map(|agent| {
            (
                agent.role,
                format!("{}/{}", agent.provider, agent.model),
                if agent.read_only { "reads" } else { "writes" }.to_string(),
            )
        })
        .collect();

    // Re-read, because attaching a repo and seeding a team both change the row.
    Ok(Registered {
        project: store.project(project.id)?,
        repo_path,
        created,
        seeded_team,
        roster,
    })
}

/// Attach another checkout to an existing project.
///
/// A project is a container, so more than one repo is normal - a change spanning a service
/// and its client is one piece of work.
pub fn attach(store: &mut Store, project_id: i64, dir: &Path) -> Result<String> {
    let dir = crate::paths::expand_user(dir)?;
    let dir = dir.canonicalize().map_err(|error| Error::UnusablePath {
        path: dir.clone(),
        reason: error.to_string(),
    })?;
    let git = GitContext::discover(&dir).ok_or_else(|| {
        Error::invalid(format!(
            "{} is not inside a git repository, so there is nothing to attach",
            dir.display()
        ))
    })?;

    let repo = store.attach_repo(
        project_id,
        NewRepo {
            remote_url: git.remote.clone(),
            name: git.name.clone(),
            main_path: Some(git.root.to_string_lossy().into_owned()),
            default_branch: git.branch.clone(),
        },
    )?;
    Ok(repo
        .main_path
        .unwrap_or_else(|| git.root.to_string_lossy().into_owned()))
}

/// What git says about a directory.
struct GitContext {
    root: PathBuf,
    name: Option<String>,
    remote: Option<String>,
    branch: Option<String>,
}

impl GitContext {
    /// Best effort, and deliberately shells out rather than linking a git library: this
    /// asks the same git the operator's own commands use, so worktrees, includes and
    /// conditional config all behave the way they already expect.
    fn discover(from: &Path) -> Option<GitContext> {
        let root = PathBuf::from(git(from, &["rev-parse", "--show-toplevel"])?);
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

/// What the maker seats should own, read off the repository itself.
///
/// The presets name this project's own directories - `crates/**`, `ui/**` - which is right
/// for ai-team and useless for anything else: a repository with a `src/` and a `web/` had
/// no seat owning either, so every slice came back undone and nothing said why.
///
/// A catch-all would be the easy fix and the wrong one. D14 says a slice nobody owns is
/// reported rather than handed to somebody, and a seat claiming `**` deletes that safety
/// by making every slice owned. So the zones are *derived* and stay disjoint: each
/// top-level directory goes to exactly one seat.
///
/// Returns `(backend, frontend)`. Either can be empty, which is honest - a repository with
/// no frontend has no frontend zone, and a slice touching one would be reported undone.
pub fn zones_for(repo: &Path) -> (String, String) {
    /// Directories that are somebody else's, or output rather than source.
    const SKIP: &[&str] = &[
        "target",
        "node_modules",
        "dist",
        "build",
        "out",
        ".output",
        "vendor",
        "coverage",
        "__pycache__",
    ];
    /// What a web project is usually called.
    const WEB: &[&str] = &["ui", "web", "frontend", "client", "www", "site", "webapp"];

    let Ok(entries) = std::fs::read_dir(repo) else {
        return (String::new(), String::new());
    };
    let root_javascript = repo.join("package.json").is_file();
    let package = std::fs::read_to_string(repo.join("package.json"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let browser_stack = [
        "\"react\"",
        "\"next\"",
        "\"vue\"",
        "\"svelte\"",
        "\"@angular/",
        "\"vite\"",
    ]
    .iter()
    .any(|dependency| package.contains(dependency));
    let root_backend = ["composer.json", "Cargo.toml", "pyproject.toml", "go.mod"]
        .iter()
        .any(|manifest| repo.join(manifest).is_file());

    let mut backend: Vec<String> = Vec::new();
    let mut frontend: Vec<String> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP.contains(&name.as_str()) {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }

        // A `package.json` inside is stronger evidence than the name: plenty of projects
        // call their frontend something unexpected, and almost none put a package.json in
        // a directory that is not one.
        let lower = name.to_lowercase();
        let web = WEB.contains(&lower.as_str())
            || entry.path().join("package.json").is_file()
            // `app/` is a frontend convention in Next, and the backend namespace in
            // Laravel. The root manifests disambiguate it; the name alone did not.
            || (lower == "app" && root_javascript && !root_backend)
            // A root browser framework usually keeps its code directly in `src/`, while
            // a Node service does too. Dependencies distinguish those without guessing
            // that every package.json means frontend.
            || (browser_stack
                && ["src", "pages", "components", "public", "assets"]
                    .contains(&lower.as_str()))
            // Mixed backends commonly keep their browser source here (Laravel/Vite).
            || (lower == "resources" && root_javascript);
        if web {
            frontend.push(format!("{name}/**"));
        } else {
            backend.push(format!("{name}/**"));
        }
    }

    backend.sort();
    frontend.sort();

    // Root manifests follow the stack that owns them. Broad `*.lock` used to hand a
    // JavaScript lockfile to Backend in the same repository where Frontend owned the app.
    if !backend.is_empty() {
        backend.extend(
            [
                "Cargo.toml",
                "Cargo.lock",
                "composer.json",
                "composer.lock",
                "pyproject.toml",
                "uv.lock",
                "go.mod",
                "go.sum",
                "*.toml",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    let javascript_files = [
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock*",
        "tsconfig*.json",
        "vite.config.*",
    ];
    if !frontend.is_empty() {
        frontend.extend(javascript_files.into_iter().map(str::to_string));
        frontend.push("*.css".to_string());
    } else if root_javascript && !backend.is_empty() {
        // A Node service owns its own package manifest and lockfile. Leaving them unowned
        // merely because it has no browser directory makes ordinary dependency work
        // impossible to route.
        backend.extend(javascript_files.into_iter().map(str::to_string));
    }

    (backend.join("\n"), frontend.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main", "."],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap();
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "base"]] {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap();
        }
    }

    #[test]
    fn a_checkout_becomes_a_project_with_its_repo_attached() {
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), None, None).unwrap();
        assert!(done.created);
        assert!(done.seeded_team);
        assert_eq!(done.project.kind, ProjectKind::Repo);
        // Named after the directory, which is what somebody standing in it would call it.
        assert_eq!(
            done.project.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
        assert!(done.repo_path.is_some());
        assert_eq!(done.roster.len(), 6, "a team of six");
    }

    #[test]
    fn a_directory_that_is_not_a_repo_is_a_project_of_another_kind() {
        // D6: a triage session across four services and a one-file chore are both
        // projects. Refusing the ones that are not checkouts would rule out half of what
        // ai-team is for.
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), Some("Triage"), None).unwrap();
        assert_eq!(done.project.kind, ProjectKind::Adhoc);
        assert!(done.repo_path.is_none());
        assert!(done.seeded_team, "it still gets a team");
    }

    #[test]
    fn registering_the_same_directory_twice_finds_it_rather_than_duplicating() {
        // The commonest way this is called is a second time - by accident from the CLI, or
        // by somebody pressing a button twice in a window that polls.
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        let mut store = Store::memory().unwrap();

        let first = register(&mut store, dir.path(), None, None).unwrap();
        let again = register(&mut store, dir.path(), None, None).unwrap();

        assert_eq!(first.project.id, again.project.id);
        assert!(!again.created);
        assert!(!again.seeded_team, "the team is not seeded twice");
        assert_eq!(store.projects().unwrap().len(), 1);
    }

    #[test]
    fn a_name_given_wins_over_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), Some("Widget Service"), None).unwrap();
        assert_eq!(done.project.name, "Widget Service");
        assert_eq!(done.project.slug, "widget-service");
    }

    #[test]
    fn a_blank_name_falls_back_rather_than_creating_a_nameless_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::memory().unwrap();
        let done = register(&mut store, dir.path(), Some("   "), None).unwrap();
        assert!(!done.project.name.trim().is_empty());
    }

    #[test]
    fn a_path_that_is_not_there_says_so() {
        let mut store = Store::memory().unwrap();
        let error = register(&mut store, Path::new("/definitely/not/here"), None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("/definitely/not/here"), "{error}");
    }

    #[test]
    fn browsing_marks_the_checkouts_and_puts_them_first() {
        // The whole point of the picker: a person is scanning for one directory they
        // already know the name of, and the ones that are repositories are the answers.
        let dir = tempfile::tempdir().unwrap();
        for name in ["zebra-service", "notes", "alpha-app"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        repo(&dir.path().join("zebra-service"));
        repo(&dir.path().join("alpha-app"));

        let listing = browse(&dir.path().to_string_lossy()).unwrap();
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["alpha-app", "zebra-service", "notes"]);
        assert!(listing.entries[0].repo);
        assert!(!listing.entries[2].repo);

        // And each path is absolute, so a surface hands it straight back rather than
        // joining it to something - which is where a picker gets a path subtly wrong.
        for entry in &listing.entries {
            assert!(Path::new(&entry.path).is_absolute(), "{}", entry.path);
            assert!(Path::new(&entry.path).is_dir(), "{}", entry.path);
        }
    }

    #[test]
    fn browsing_shows_directories_and_nothing_else() {
        // It exists so somebody can find a checkout by looking, which is not a reason to
        // let a window enumerate their documents. Dotfiles go too: nobody keeps a checkout
        // in `.cache`, and `.git` would otherwise look like somewhere to go into.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("secrets.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join(".cache")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();

        let names: Vec<String> = browse(&dir.path().to_string_lossy())
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, ["src"]);
    }

    #[test]
    fn browsing_resolves_where_it_actually_is() {
        // `..` in a request has to become a real directory, and the parent handed back has
        // to be the one the operating system agrees with - on macOS that also means `/tmp`
        // resolving to `/private/tmp` (D12), which is exactly the kind of thing a picker
        // gets wrong by string-joining instead of asking.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        let resolved = dir.path().canonicalize().unwrap();

        let listing = browse(&format!("{}/src/deep/..", dir.path().display())).unwrap();
        assert_eq!(listing.path, resolved.join("src").to_string_lossy());
        assert_eq!(
            listing.parent.as_deref(),
            Some(resolved.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn browsing_nowhere_starts_at_home() {
        // Where a person's checkouts are. `/` is a list of things none of them is.
        let listing = browse("  ").unwrap();
        assert_eq!(
            listing.path,
            crate::paths::home_dir()
                .unwrap()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
    }

    #[test]
    fn a_subdirectory_registers_the_repository_root() {
        // Somebody pointing at `src/` means the project, not the subdirectory - and a repo
        // rooted at `src/` could not be leased.
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, &dir.path().join("src/deep"), None, None).unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(
            done.repo_path.as_deref(),
            Some(root.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn a_kind_given_wins_over_the_one_inferred() {
        // A checkout can legitimately be a ticket or a chore, and inferring `repo` from the
        // presence of `.git` would override somebody who said otherwise.
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), None, Some(ProjectKind::Chore)).unwrap();
        assert_eq!(done.project.kind, ProjectKind::Chore);
    }

    #[test]
    fn zones_come_from_the_repository_rather_than_from_this_projects_shape() {
        // The presets name `crates/**` and `ui/**`, which is right for ai-team and useless
        // for anything else: a repo with `src/` and `web/` had no seat owning either, so
        // every slice came back undone and nothing said why.
        let dir = tempfile::tempdir().unwrap();
        for name in ["src", "web", "target", "node_modules", ".git", "docs"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }

        let (backend, frontend) = zones_for(dir.path());

        assert!(backend.contains("src/**"), "{backend}");
        assert!(backend.contains("docs/**"), "{backend}");
        assert!(frontend.contains("web/**"), "{frontend}");

        // Output and other people's code are not anybody's zone.
        for junk in ["target", "node_modules", ".git"] {
            assert!(!backend.contains(junk), "{backend}");
            assert!(!frontend.contains(junk), "{frontend}");
        }
    }

    #[test]
    fn the_zones_stay_disjoint() {
        // A catch-all would be the easy fix and the wrong one: D14 says a slice nobody owns
        // is reported rather than handed to somebody, and a seat claiming `**` deletes that
        // safety by making every slice owned.
        let dir = tempfile::tempdir().unwrap();
        for name in ["src", "ui"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        let (backend, frontend) = zones_for(dir.path());

        assert!(
            !backend.contains("**\n") || !backend.starts_with("**"),
            "{backend}"
        );
        for path in ["src/lib.rs", "src/deep/thing.rs"] {
            assert!(crate::util::zone_matches(&backend, path), "{path}");
            assert!(!crate::util::zone_matches(&frontend, path), "{path}");
        }
        for path in ["ui/App.tsx", "ui/src/main.ts"] {
            assert!(crate::util::zone_matches(&frontend, path), "{path}");
            assert!(!crate::util::zone_matches(&backend, path), "{path}");
        }
    }

    #[test]
    fn a_mixed_php_javascript_repo_keeps_laravel_app_on_the_backend() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["app", "database", "resources"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join("composer.json"), "{}").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();

        let (backend, frontend) = zones_for(dir.path());
        assert!(backend.contains("app/**"), "{backend}");
        assert!(backend.contains("database/**"), "{backend}");
        assert!(!frontend.contains("app/**"), "{frontend}");
        assert!(frontend.contains("resources/**"), "{frontend}");
        assert!(frontend.contains("package.json"), "{frontend}");
        assert!(backend.contains("composer.json"), "{backend}");
    }

    #[test]
    fn a_javascript_app_directory_still_belongs_to_frontend() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();

        let (backend, frontend) = zones_for(dir.path());
        assert!(!backend.contains("app/**"), "{backend}");
        assert!(frontend.contains("app/**"), "{frontend}");
    }

    #[test]
    fn root_javascript_dependencies_distinguish_a_browser_app_from_a_node_service() {
        let browser = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(browser.path().join("src")).unwrap();
        std::fs::write(
            browser.path().join("package.json"),
            r#"{"dependencies":{"react":"latest"}}"#,
        )
        .unwrap();
        let (backend, frontend) = zones_for(browser.path());
        assert!(!backend.contains("src/**"), "{backend}");
        assert!(frontend.contains("src/**"), "{frontend}");

        let service = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(service.path().join("src")).unwrap();
        std::fs::write(
            service.path().join("package.json"),
            r#"{"dependencies":{"express":"latest"}}"#,
        )
        .unwrap();
        let (backend, frontend) = zones_for(service.path());
        assert!(backend.contains("src/**"), "{backend}");
        assert!(backend.contains("package.json"), "{backend}");
        assert!(!frontend.contains("src/**"), "{frontend}");
    }

    #[test]
    fn a_package_json_is_stronger_evidence_than_a_directory_name() {
        // Plenty of projects call their frontend something unexpected, and almost none put a
        // package.json in a directory that is not one.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("dashboard")).unwrap();
        std::fs::write(dir.path().join("dashboard/package.json"), "{}").unwrap();
        std::fs::create_dir_all(dir.path().join("server")).unwrap();

        let (backend, frontend) = zones_for(dir.path());
        assert!(frontend.contains("dashboard/**"), "{frontend}");
        assert!(backend.contains("server/**"), "{backend}");
    }

    #[test]
    fn a_repo_with_no_frontend_has_no_frontend_zone() {
        // Honest rather than convenient: a slice touching a frontend that does not exist is
        // reported undone, which is the right answer.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let (backend, frontend) = zones_for(dir.path());
        assert!(!backend.is_empty());
        assert!(frontend.is_empty(), "{frontend}");
    }

    #[test]
    fn registering_a_repo_gives_its_seats_zones_that_route() {
        // End to end: the thing that was broken was a fresh project on a differently-shaped
        // repo having no seat that owned anything.
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), None, None).unwrap();
        let team = done.project.team_id.unwrap();

        let owner = store
            .agent_for_path(team, "src/lib.rs")
            .unwrap()
            .expect("some seat should own src/");
        assert_eq!(owner.role, "backend");
    }

    #[test]
    fn a_seat_with_nothing_to_own_is_given_nothing() {
        // Rather than keeping a preset zone naming a directory this repository does not
        // have, which routes a slice to somebody who then finds nothing there.
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), None, None).unwrap();
        let frontend = store
            .agents(done.project.team_id.unwrap())
            .unwrap()
            .into_iter()
            .find(|a| a.role == "frontend")
            .unwrap();
        assert_eq!(frontend.zone, "", "there is no frontend in this repo");
    }

    #[test]
    fn a_roster_somebody_has_edited_is_left_alone() {
        // Registering twice must not overwrite zones a person chose - that would be ai-team
        // deciding something that is not its to decide.
        let dir = tempfile::tempdir().unwrap();
        repo(dir.path());
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, dir.path(), None, None).unwrap();
        let team = done.project.team_id.unwrap();
        let backend = store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();

        let mut edited = crate::model::NewAgent::from(&backend);
        edited.zone = "chosen/**".into();
        store.update_agent(backend.id, edited).unwrap();

        register(&mut store, dir.path(), None, None).unwrap();
        let after = store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|a| a.role == "backend")
            .unwrap();
        assert_eq!(after.zone, "chosen/**");
    }

    #[test]
    fn a_second_repo_can_be_attached_to_one_project() {
        // A project is a container, so a change spanning a service and its client is one
        // piece of work.
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        repo(one.path());
        repo(two.path());
        let mut store = Store::memory().unwrap();

        let done = register(&mut store, one.path(), Some("Both"), None).unwrap();
        attach(&mut store, done.project.id, two.path()).unwrap();

        assert_eq!(store.project_repos(done.project.id).unwrap().len(), 2);
    }

    #[test]
    fn attaching_something_that_is_not_a_repo_is_refused_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let plain = tempfile::tempdir().unwrap();
        repo(dir.path());
        let mut store = Store::memory().unwrap();
        let done = register(&mut store, dir.path(), None, None).unwrap();

        let error = attach(&mut store, done.project.id, plain.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("not inside a git repository"), "{error}");
    }
}
