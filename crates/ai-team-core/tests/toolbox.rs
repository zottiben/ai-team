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
fn layout_migration_preserves_conflicting_hooks_and_skills_without_any_live_writes() {
    let repo = tempfile::tempdir().unwrap();
    for (path, text) in [
        (".claude/hooks/custom.sh", "legacy edit"),
        (".agents/hooks/custom.sh", "canonical edit"),
    ] {
        fs::create_dir_all(repo.path().join(path).parent().unwrap()).unwrap();
        fs::write(repo.path().join(path), text).unwrap();
    }
    // Confirm the pinned engine's hazardous behavior, then require the adapter to refuse it.
    let mut unsafe_plan = ai_toolbox_core::Plan::default();
    ai_toolbox_core::migrate::plan(&mut unsafe_plan, repo.path()).unwrap();
    assert!(unsafe_plan
        .actions
        .iter()
        .any(|a| a.path == repo.path().join(".claude/hooks/custom.sh")
            && matches!(a.kind, ai_toolbox_core::action::Kind::Remove)));
    let (mut store, project) = registered(repo.path());
    let selection: Selection =
        serde_json::from_value(serde_json::json!({"operation":"migrate"})).unwrap();
    let error = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection,
    )
    .unwrap_err();
    assert!(error.to_string().contains("conflict"), "{error}");
    assert_eq!(
        fs::read_to_string(repo.path().join(".claude/hooks/custom.sh")).unwrap(),
        "legacy edit"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join(".agents/hooks/custom.sh")).unwrap(),
        "canonical edit"
    );
}

#[test]
fn layout_migration_preserves_bytes_modes_extra_skill_files_and_pi_overrides() {
    use std::os::unix::fs::PermissionsExt;
    let repo = tempfile::tempdir().unwrap();
    for (path, text) in [
        (".claude/hooks/custom.sh", "#!/bin/sh\\necho local\\n"),
        (".codex/hooks/custom.sh", "#!/bin/sh\\necho local\\n"),
        (".pi/mcp/local.sh", "#!/bin/sh\\necho local MCP\\n"),
        (".claude/skills/own/SKILL.md", "own skill"),
        (".claude/skills/README", "not disposable"),
        (
            ".claude/settings.json",
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":".claude/hooks/custom.sh"}]}]},"custom":"keep"}"#,
        ),
        (
            ".pi/mcp-adapter.json",
            r#"{"settings":{"scriptMode":true},"mcpServers":{"custom":{"command":".pi/mcp/local.sh"}}}"#,
        ),
        (
            ".pi/mcp.json",
            r#"{"other":"preserve obsolete config","mcpServers":{"old":{"command":"old-command"}}}"#,
        ),
    ] {
        fs::create_dir_all(repo.path().join(path).parent().unwrap()).unwrap();
        fs::write(repo.path().join(path), text).unwrap();
    }
    for path in [".claude/hooks/custom.sh", ".codex/hooks/custom.sh"] {
        fs::set_permissions(repo.path().join(path), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let old_pi = fs::read(repo.path().join(".pi/mcp.json")).unwrap();
    let (mut store, project) = registered(repo.path());
    let scan = toolbox::scan(repo.path());
    assert!(!scan.survey.unwrap()["findings"]
        .to_string()
        .contains("ai-toolbox migrate"));
    let selection = || serde_json::from_value(serde_json::json!({"operation":"migrate"})).unwrap();
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    assert!(repo.path().join(".claude/hooks/custom.sh").is_file());
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "applied", "{done:?}");
    assert!(!repo.path().join(".claude/hooks").exists());
    assert!(!repo.path().join(".codex/hooks").exists());
    assert!(!repo.path().join(".pi/mcp").exists());
    assert_eq!(
        fs::metadata(repo.path().join(".agents/hooks/custom.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::read_to_string(repo.path().join(".agents/skills/README")).unwrap(),
        "not disposable"
    );
    assert_eq!(
        fs::read_link(repo.path().join(".claude/skills")).unwrap(),
        Path::new("../.agents/skills")
    );
    let settings = fs::read_to_string(repo.path().join(".claude/settings.json")).unwrap();
    assert!(settings.contains(".agents/hooks/custom.sh"));
    assert!(settings.contains("keep"));
    let pi = fs::read_to_string(repo.path().join(".pi/mcp-adapter.json")).unwrap();
    assert!(pi.contains(".agents/mcp/local.sh"));
    assert!(pi.contains("scriptMode"));
    assert_eq!(fs::read(repo.path().join(".pi/mcp.json")).unwrap(), old_pi);
    assert!(!repo.path().join(".mcp.json").exists());
    let again = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    assert!(again.effects.is_empty(), "{again:?}");
}

#[test]
fn migration_repoints_pi_launchers_without_rewriting_unrelated_strings() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".pi/mcp")).unwrap();
    fs::write(repo.path().join(".pi/mcp/local.sh"), "helper").unwrap();
    let config = r#"{"notes":"my .pi/mcp/local.sh notes","mcpServers":{"own":{"command":".pi/mcp/local.sh","args":[".pi/mcp/arg.sh"],"custom":"my .pi/mcp/local.sh notes","headers":{"Authorization":"!.pi/mcp/local.sh TOKEN","X-Note":"my .pi/mcp/local.sh notes"}}}}"#;
    for path in [".pi/mcp.json", ".pi/mcp-adapter.json"] {
        fs::write(repo.path().join(path), config).unwrap();
    }
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        Selection::Migrate,
    )
    .unwrap();
    assert_eq!(
        toolbox::apply(&mut store, project, preview.id)
            .unwrap()
            .state,
        "applied"
    );
    for path in [".pi/mcp.json", ".pi/mcp-adapter.json"] {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(repo.path().join(path)).unwrap()).unwrap();
        assert_eq!(value["notes"], "my .pi/mcp/local.sh notes");
        assert_eq!(
            value["mcpServers"]["own"]["custom"],
            "my .pi/mcp/local.sh notes"
        );
        assert_eq!(
            value["mcpServers"]["own"]["headers"]["X-Note"],
            "my .pi/mcp/local.sh notes"
        );
        assert_eq!(
            value["mcpServers"]["own"]["headers"]["Authorization"],
            "!.agents/mcp/local.sh TOKEN"
        );
        assert_eq!(
            value["mcpServers"]["own"]["command"],
            ".agents/mcp/local.sh"
        );
    }
}

#[test]
fn setup_errors_name_the_real_project_not_a_discarded_staging_directory() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    let (mut store, project) = registered(repo.path());
    fs::create_dir(root.join(".pi")).unwrap();
    fs::write(root.join(".pi/mcp-adapter.json"), "broken").unwrap();
    let scan = toolbox::scan(&root);
    assert!(scan
        .problem
        .unwrap()
        .contains(root.join(".pi/mcp-adapter.json").to_str().unwrap()));
    fs::remove_file(root.join(".pi/mcp-adapter.json")).unwrap();
    fs::write(
        root.join(".mcp.json"),
        "{ // keep my comment\\n\"mcpServers\": {} }",
    )
    .unwrap();
    let selection = serde_json::from_value(serde_json::json!({"operation":"install","harnesses":["pi"],"hooks":[],"mcp":["context7"],"skills":[],"scaffold":false})).unwrap();
    let error =
        toolbox::preview(&mut store, project, root.to_str().unwrap(), selection).unwrap_err();
    assert!(
        error
            .to_string()
            .contains(root.join(".mcp.json").to_str().unwrap()),
        "{error}"
    );
}

#[test]
fn detection_only_directories_are_not_copied_or_used_as_write_authority() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(repo.path().join(".github")).unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.path().join(".github/external")).unwrap();
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("pre-pr"),
    )
    .unwrap();
    assert!(!preview
        .effects
        .iter()
        .any(|e| e.path.starts_with(".github")));
    fs::remove_file(repo.path().join(".github/external")).unwrap();
    fs::remove_dir(repo.path().join(".github")).unwrap();
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "refused");
    assert!(!repo.path().join(".agents").exists());
    assert!(outside.path().read_dir().unwrap().next().is_none());
}

#[test]
fn migration_refuses_a_stale_source_before_creating_or_removing_anything() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".claude/skills/own")).unwrap();
    fs::write(repo.path().join(".claude/skills/own/SKILL.md"), "original").unwrap();
    let (mut store, project) = registered(repo.path());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        Selection::Migrate,
    )
    .unwrap();
    fs::write(
        repo.path().join(".claude/skills/own/new-file"),
        "concurrent",
    )
    .unwrap();
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "refused");
    assert!(done.outcome.unwrap().applied.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(repo.path().join(".claude/skills/own/new-file").is_file());
}

#[test]
fn migration_rejects_same_named_skills_with_different_permissions_or_contents() {
    use std::os::unix::fs::PermissionsExt;
    for mode_only in [false, true] {
        let repo = tempfile::tempdir().unwrap();
        for base in [".agents/skills/own", ".claude/skills/own"] {
            fs::create_dir_all(repo.path().join(base)).unwrap();
            fs::write(repo.path().join(base).join("SKILL.md"), "original").unwrap();
        }
        let edited = repo.path().join(".claude/skills/own/SKILL.md");
        if mode_only {
            fs::set_permissions(&edited, fs::Permissions::from_mode(0o700)).unwrap();
        } else {
            fs::write(&edited, "local edit").unwrap();
        }
        let (mut store, project) = registered(repo.path());
        let error = toolbox::preview(
            &mut store,
            project,
            repo.path().to_str().unwrap(),
            Selection::Migrate,
        )
        .unwrap_err();
        assert!(error.to_string().contains("conflict"), "{error}");
        assert!(!fs::symlink_metadata(repo.path().join(".claude/skills"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(repo.path().join(".agents/skills/own/SKILL.md")).unwrap(),
            "original"
        );
    }
}

#[test]
fn the_catalogue_exposes_every_bundled_choice_and_read_only_reference() {
    let catalogue = toolbox::catalogue().unwrap();
    let manifest = include_str!("../assets/toolbox/FILES");
    for line in manifest.lines() {
        let path = line.split_whitespace().last().unwrap();
        let exposed = if path.starts_with("hooks/")
            && Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sh"))
            && !path.ends_with("_lib.sh")
        {
            catalogue
                .hooks
                .iter()
                .any(|i| path == format!("hooks/{}.sh", i.key))
        } else if path.starts_with("mcp/presets/") {
            catalogue
                .mcp
                .iter()
                .any(|i| path == format!("mcp/presets/{}.json", i.key))
        } else if path.starts_with("skills/") && path.ends_with("/SKILL.md") {
            catalogue
                .skills
                .iter()
                .any(|i| path == format!("skills/{}/SKILL.md", i.key))
        } else if path.starts_with("starters/rules/") {
            catalogue
                .rules
                .iter()
                .any(|i| path == format!("starters/rules/{}.md", i.key))
        } else if path.starts_with("templates/")
            || path.starts_with("starters/agents/")
            || path.starts_with("background/")
            || path == "mcp/mcp.json.template"
        {
            catalogue.templates.iter().any(|i| i.key == path)
        } else {
            true
        };
        assert!(exposed, "bundled capability missing from catalogue: {path}");
    }
    assert!(catalogue
        .rules
        .iter()
        .any(|i| i.key == "unity" && !i.contents.is_empty()));
    assert!(catalogue.charter.contents.contains("Do the work"));
    assert!(catalogue.notice.contains("TypeSafe"));
    assert!(!serde_json::to_string(&catalogue)
        .unwrap()
        .contains("/var/folders/"));
}

#[test]
fn every_catalogue_preset_previews_for_all_harnesses_without_connecting() {
    for item in toolbox::catalogue().unwrap().mcp {
        let repo = tempfile::tempdir().unwrap();
        fs::write(
            repo.path().join(".mcp.json"),
            r#"{"mcpServers":{"own":{"command":"never-execute-me"}}}"#,
        )
        .unwrap();
        let (mut store, project) = registered(repo.path());
        let selection = serde_json::from_value(serde_json::json!({
            "operation":"install", "harnesses":["claude","codex","pi"], "hooks":[], "mcp":[item.key], "skills":[], "scaffold":false
        })).unwrap();
        let preview = toolbox::preview(
            &mut store,
            project,
            repo.path().to_str().unwrap(),
            selection,
        )
        .unwrap();
        let done = toolbox::apply(&mut store, project, preview.id).unwrap();
        assert_eq!(done.state, "applied", "{}: {done:?}", item.key);
        let shared: serde_json::Value =
            serde_json::from_slice(&fs::read(repo.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(shared["mcpServers"]["own"]["command"], "never-execute-me");
        assert!(repo.path().join(".codex/config.toml").is_file());
    }
}

#[test]
fn copy_mode_replaces_only_the_canonical_link_and_preserves_unselected_skills() {
    let repo = tempfile::tempdir().unwrap();
    let (mut store, project) = registered(repo.path());
    let initial = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        install("pre-pr"),
    )
    .unwrap();
    assert_eq!(
        toolbox::apply(&mut store, project, initial.id)
            .unwrap()
            .state,
        "applied"
    );
    fs::create_dir_all(repo.path().join(".agents/skills/own")).unwrap();
    fs::write(
        repo.path().join(".agents/skills/own/SKILL.md"),
        "keep own skill",
    )
    .unwrap();
    let selection: Selection = serde_json::from_value(serde_json::json!({
        "operation":"install", "harnesses":["claude","codex","pi"], "hooks":[], "mcp":[],
        "skills":["cli/gh"], "scaffold":false, "no_symlink":true, "with_dotenv":true
    }))
    .unwrap();
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection,
    )
    .unwrap();
    assert!(preview.effects.iter().any(|e| e.path == ".claude/skills"));
    assert!(!preview
        .effects
        .iter()
        .any(|e| e.path.starts_with(".claude/skills/")));
    let done = toolbox::apply(&mut store, project, preview.id).unwrap();
    assert_eq!(done.state, "applied", "{done:?}");
    assert!(!fs::symlink_metadata(repo.path().join(".claude/skills"))
        .unwrap()
        .file_type()
        .is_symlink());
    for base in [".agents/skills", ".claude/skills"] {
        assert!(repo.path().join(base).join("pre-pr/SKILL.md").is_file());
        assert!(repo.path().join(base).join("gh/SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(repo.path().join(base).join("own/SKILL.md")).unwrap(),
            "keep own skill"
        );
    }
    assert!(repo.path().join(".agents/mcp/with-dotenv.sh").is_file());
}

#[test]
fn explicit_copy_mode_keeps_unrelated_claude_skills_and_is_stale_safe() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".claude/skills/own")).unwrap();
    fs::write(repo.path().join(".claude/skills/own/SKILL.md"), "keep").unwrap();
    let (mut store, project) = registered(repo.path());
    let selection = || {
        serde_json::from_value(serde_json::json!({
            "operation":"install", "harnesses":["claude"], "hooks":[], "mcp":[],
            "skills":["cli"], "scaffold":false, "no_symlink":true
        }))
        .unwrap()
    };
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    fs::write(repo.path().join(".claude/skills/own/SKILL.md"), "newer").unwrap();
    assert_eq!(
        toolbox::apply(&mut store, project, preview.id)
            .unwrap()
            .state,
        "refused"
    );
    assert!(!repo.path().join(".agents").exists());
    let preview = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    assert_eq!(
        toolbox::apply(&mut store, project, preview.id)
            .unwrap()
            .state,
        "applied"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join(".claude/skills/own/SKILL.md")).unwrap(),
        "newer"
    );
    assert!(repo.path().join(".claude/skills/gh/SKILL.md").is_file());
    let again = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    assert!(again.effects.is_empty(), "{again:?}");
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
