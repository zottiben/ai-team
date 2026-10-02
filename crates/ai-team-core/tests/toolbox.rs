use std::{fs, path::Path};

use ai_team_core::{
    toolbox::{self, Selection},
    RoleModelDefault, Store,
};

fn registered(root: &Path) -> (Store, i64) {
    let mut store = Store::memory().unwrap();
    let project = ai_team_core::register_project(
        &mut store,
        root,
        Some("toolbox fixture"),
        None,
        RoleModelDefault::local_floor,
    )
    .unwrap();
    assert!(
        project.toolbox_scan.problem.is_none(),
        "{:?}",
        project.toolbox_scan
    );
    (store, project.project.id)
}

fn install(skill: &str) -> Selection {
    serde_json::from_value(serde_json::json!({"operation":"install","harnesses":["claude","pi"],"hooks":[],"mcp":[],"skills":[skill],"scaffold":true})).unwrap()
}

#[test]
fn a_stale_skill_preview_preserves_the_concurrent_edit() {
    let repo = tempfile::tempdir().unwrap();
    let skill = repo.path().join(".agents/skills/pre-pr");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("SKILL.md"), "installed v1").unwrap();
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("pre-pr"),
    )
    .unwrap();
    fs::write(skill.join("SKILL.md"), "concurrent local edit").unwrap();
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "refused");
    assert!(done.outcome.unwrap().problem.unwrap().contains("stale"));
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        "concurrent local edit"
    );
    assert!(!repo.path().join("AGENTS.md").exists());
    assert!(toolbox::apply(&mut store, project, preview.id).is_err());
}

#[test]
fn packaged_setup_is_exact_idempotent_and_project_scoped() {
    let repo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::write(repo.path().join("AGENTS.md"), "Keep this knowledge").unwrap();
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("typesafe-ai"),
    )
    .unwrap();
    assert!(!preview.effects.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(preview.effects.iter().all(|e| !e.path.starts_with('/')));
    assert!(toolbox::apply(&mut store, project + 1, preview.id).is_err());
    assert!(toolbox::preview(
        &mut store,
        project,
        other.path().to_str().unwrap(),
        install("pre-pr")
    )
    .is_err());
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "applied", "{done:?}");
    assert_eq!(
        fs::read_to_string(repo.path().join("AGENTS.md")).unwrap(),
        "Keep this knowledge"
    );
    assert!(repo
        .path()
        .join(".agents/skills/typesafe-ai/LICENSE")
        .is_file());
    assert_eq!(
        fs::read_link(repo.path().join(".claude/skills")).unwrap(),
        Path::new("../.agents/skills")
    );
    let again = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("typesafe-ai"),
    )
    .unwrap();
    assert!(again.effects.is_empty(), "{again:?}");
    assert!(other.path().read_dir().unwrap().next().is_none());
}

#[test]
fn parent_symlink_retargeting_cannot_write_outside_the_project() {
    let repo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("pre-pr"),
    )
    .unwrap();
    std::os::unix::fs::symlink(other.path(), repo.path().join(".agents")).unwrap();
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "refused");
    assert!(other.path().read_dir().unwrap().next().is_none());
    assert!(!repo.path().join("AGENTS.md").exists());
}

#[test]
fn current_pi_overrides_preserve_old_configs_and_unrelated_servers() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir(repo.path().join(".pi")).unwrap();
    let old = r#"{"mcpServers":{"old-only":{"command":"keep-old"}}}"#;
    fs::write(repo.path().join(".pi/mcp.json"), old).unwrap();
    fs::write(
        repo.path().join(".pi/mcp-adapter.json"),
        r#"{"settings":{"scriptMode":true},"mcpServers":{"own":{"command":"keep-own"}}}"#,
    )
    .unwrap();
    let (mut store, project) = registered(repo.path());
    let selection: Selection = serde_json::from_value(serde_json::json!({"operation":"install","harnesses":["pi"],"hooks":[],"mcp":["pixellab"],"skills":[],"scaffold":false})).unwrap();
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection,
    )
    .unwrap();
    assert!(preview
        .effects
        .iter()
        .any(|e| e.path == ".pi/mcp-adapter.json"));
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "applied", "{done:?}");
    assert_eq!(
        fs::read_to_string(repo.path().join(".pi/mcp.json")).unwrap(),
        old
    );
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(repo.path().join(".pi/mcp-adapter.json")).unwrap())
            .unwrap();
    assert_eq!(config["settings"]["scriptMode"], true);
    assert_eq!(config["mcpServers"]["own"]["command"], "keep-own");
    assert!(config["mcpServers"].get("old-only").is_none());
    assert!(config["mcpServers"]["pixellab"]["headers"]["Authorization"]
        .as_str()
        .unwrap()
        .starts_with('!'));
}

#[test]
fn repairing_unwired_or_non_executable_hooks_preserves_local_script_edits() {
    use std::os::unix::fs::PermissionsExt;
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".agents/hooks")).unwrap();
    let hook = repo.path().join(".agents/hooks/format-on-edit.sh");
    fs::write(&hook, "#!/bin/sh\necho my local formatting\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        Selection::Repair,
    )
    .unwrap();
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "applied", "{done:?}");
    assert_eq!(
        fs::read_to_string(&hook).unwrap(),
        "#!/bin/sh\necho my local formatting\n"
    );
    assert_ne!(fs::metadata(&hook).unwrap().permissions().mode() & 0o111, 0);
    assert!(repo.path().join(".claude/settings.json").is_file());
}

#[test]
fn scanning_reports_linked_worktree_configuration_drift_without_converging_it() {
    let repo = tempfile::tempdir().unwrap();
    let linked = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["commit", "--allow-empty", "-qm", "fixture"]);
    git(&[
        "worktree",
        "add",
        "--detach",
        linked.path().to_str().unwrap(),
    ]);
    fs::write(repo.path().join("AGENTS.md"), "reference knowledge").unwrap();
    fs::write(linked.path().join("AGENTS.md"), "branch knowledge").unwrap();
    let scan = toolbox::scan(repo.path());
    assert!(scan.worktree_problem.is_none(), "{scan:?}");
    let path = linked
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let linked_scan = scan.worktrees.iter().find(|w| w.path == path).unwrap();
    assert_eq!(linked_scan.different, vec!["AGENTS.md"]);
    assert_eq!(
        fs::read_to_string(linked.path().join("AGENTS.md")).unwrap(),
        "branch knowledge"
    );
}

#[test]
fn saved_previews_survive_reopening_without_replanning() {
    let repo = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("team.db");
    let mut store = Store::init(&path).unwrap();
    let project = ai_team_core::register_project(
        &mut store,
        repo.path(),
        None,
        None,
        RoleModelDefault::local_floor,
    )
    .unwrap()
    .project
    .id;
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("pre-pr"),
    )
    .unwrap();
    drop(store);
    let mut reopened = Store::open(&path).unwrap();
    let done = toolbox::apply(&mut reopened, project, preview.id).unwrap();
    assert_eq!(done.state, "applied");
    assert_eq!(
        serde_json::to_value(done.effects).unwrap(),
        serde_json::to_value(preview.effects).unwrap()
    );
    assert_eq!(
        reopened.toolbox_history(project).unwrap()[0].state,
        "applied"
    );
}

#[test]
fn modified_catalogue_items_do_not_launch_upstream_git_history() {
    use std::{os::unix::fs::PermissionsExt, process::Command};
    let repo = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("git-was-called");
    fs::write(
        bin.path().join("git"),
        format!(
            "#!/bin/sh\nprintf called > '{}'\nexit 99\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(bin.path().join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(repo.path().join(".agents/skills/pre-pr")).unwrap();
    fs::write(
        repo.path().join(".agents/skills/pre-pr/SKILL.md"),
        "local edit, not the catalogue",
    )
    .unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "scan_without_git_child", "--nocapture"])
        .env("AI_TEAM_TOOLBOX_SCAN_FIXTURE", repo.path())
        .env("PATH", bin.path())
        .env("GIT_DIR", "/unrelated/standalone/git")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !marker.exists(),
        "scan launched an unbounded upstream history command"
    );
}

#[test]
fn scan_without_git_child() {
    let Some(root) = std::env::var_os("AI_TEAM_TOOLBOX_SCAN_FIXTURE") else {
        return;
    };
    let scan = toolbox::scan(Path::new(&root));
    assert!(scan.problem.is_none(), "{scan:?}");
    let survey = scan.survey.unwrap();
    let item = survey["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "pre-pr")
        .unwrap();
    assert_eq!(item["origin"]["state"], "modified");
    assert!(!survey["findings"]
        .to_string()
        .contains("no version the catalogue ever shipped"));
    assert!(!survey["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "item-stale"));
}

#[test]
fn registration_reports_malformed_configuration_without_mutating_it() {
    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join(".mcp.json"), "broken JSON").unwrap();
    let mut store = Store::memory().unwrap();
    let registered = ai_team_core::register_project(
        &mut store,
        repo.path(),
        None,
        None,
        RoleModelDefault::local_floor,
    )
    .unwrap();
    assert!(registered.toolbox_scan.problem.is_some());
    assert_eq!(
        fs::read_to_string(repo.path().join(".mcp.json")).unwrap(),
        "broken JSON"
    );
    assert!(!repo.path().join("AGENTS.md").exists());
}
