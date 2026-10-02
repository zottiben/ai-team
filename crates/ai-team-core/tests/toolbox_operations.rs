use ai_team_core::{
    toolbox::{self, Authority, RegistrySelection, Selection, UserSelection},
    ProjectStatus, RoleModelDefault, Store,
};
use std::{fs, path::Path, process::Command};
fn register(store: &mut Store, root: &Path) -> i64 {
    ai_team_core::register_project(store, root, None, None, RoleModelDefault::local_floor)
        .unwrap()
        .project
        .id
}
fn write(root: &Path, path: &str, text: &str) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}
fn user(root: &Path) -> Authority {
    Authority::User {
        home: root.canonicalize().unwrap(),
        pi_agent: root.canonicalize().unwrap().join(".pi/agent"),
        charter_targets: vec![],
    }
}
fn selection() -> UserSelection {
    serde_json::from_value(serde_json::json!({"harnesses":["claude","codex","pi"],"skills":["pre-pr"],"no_symlink":false,"charter":true,"charter_path":null})).unwrap()
}
#[test]
fn user_setup_is_separate_exact_stale_safe_and_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let mut s = Store::memory().unwrap();
    let p = register(&mut s, repo.path());
    write(home.path(), ".claude/CLAUDE.md", "My rules\n");
    write(home.path(), ".pi/agent/settings.json", "keep settings");
    let a = user(home.path());
    let proposal = toolbox::preview_user(&mut s, a.clone(), selection()).unwrap();
    for effect in proposal
        .changes
        .iter()
        .flat_map(|c| &c.effects)
        .filter(|e| e.path.ends_with("AGENTS.md") || e.path.ends_with("CLAUDE.md"))
    {
        assert!(
            effect
                .summary
                .contains(home.path().canonicalize().unwrap().to_str().unwrap()),
            "charter summary must name the actual destination, not its private staging path: {}",
            effect.summary
        );
    }
    assert!(s.toolbox_operation(proposal.id, "converge").is_err());
    assert!(toolbox::preview(&mut s, p, home.path().to_str().unwrap(), Selection::Repair).is_err());
    write(home.path(), ".claude/CLAUDE.md", "newer rules\n");
    let refused = toolbox::apply_operation(&mut s, proposal.id, "user", Some(&a)).unwrap();
    assert_eq!(refused.state, "refused");
    assert!(!home.path().join(".agents").exists());
    let proposal = toolbox::preview_user(&mut s, a.clone(), selection()).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, proposal.id, "user", Some(&a))
            .unwrap()
            .state,
        "applied"
    );
    assert!(fs::read_to_string(home.path().join(".claude/CLAUDE.md"))
        .unwrap()
        .starts_with("newer rules\n"));
    for path in [
        ".codex/AGENTS.md",
        ".pi/agent/AGENTS.md",
        ".agents/skills/pre-pr/SKILL.md",
    ] {
        assert!(home.path().join(path).is_file());
    }
    assert_eq!(
        fs::read_to_string(home.path().join(".pi/agent/settings.json")).unwrap(),
        "keep settings"
    );
    let again = toolbox::preview_user(&mut s, a, selection()).unwrap();
    assert!(again.changes.iter().all(|c| c.effects.is_empty()));
    assert!(repo.path().read_dir().unwrap().next().is_none());
}
#[test]
fn user_charter_refuses_non_text_and_a_changed_user_identity() {
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut s = Store::memory().unwrap();
    fs::create_dir(home.path().join(".claude")).unwrap();
    fs::write(home.path().join(".claude/CLAUDE.md"), [255]).unwrap();
    assert!(
        toolbox::preview_user(&mut s, user(home.path()), selection())
            .unwrap_err()
            .to_string()
            .contains("UTF-8")
    );
    assert_eq!(
        fs::read(home.path().join(".claude/CLAUDE.md")).unwrap(),
        [255]
    );
    fs::remove_file(home.path().join(".claude/CLAUDE.md")).unwrap();
    let saved = toolbox::preview_user(&mut s, user(home.path()), selection()).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "user", Some(&user(other.path())))
            .unwrap()
            .state,
        "refused"
    );
    assert!(!home.path().join(".agents").exists());
}
#[test]
fn user_custom_charter_preserves_bytes_and_refuses_alias_changes() {
    let home = tempfile::tempdir().unwrap();
    let outer = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut s = Store::memory().unwrap();
    let alias = home.path().join("selected");
    std::os::unix::fs::symlink(outer.path(), &alias).unwrap();
    let custom = || UserSelection {
        harnesses: vec![ai_toolbox_core::Harness::Pi],
        skills: vec![],
        no_symlink: false,
        charter: true,
        charter_path: Some(alias.join("rules.md").to_string_lossy().into_owned()),
    };
    let a = user(home.path());
    let saved = toolbox::preview_user(&mut s, a.clone(), custom()).unwrap();
    assert!(
        matches!(&saved.authority,Authority::User{charter_targets,..} if charter_targets==&vec![outer.path().canonicalize().unwrap().join("rules.md")])
    );
    fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(other.path(), &alias).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "user", Some(&a))
            .unwrap()
            .state,
        "refused"
    );
    assert!(!outer.path().join("rules.md").exists());
    assert!(!other.path().join("rules.md").exists());
    let saved = toolbox::preview_user(&mut s, a.clone(), custom()).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "user", Some(&a))
            .unwrap()
            .state,
        "applied"
    );
    assert!(other.path().join("rules.md").is_file());
}
fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn linked_fixture() -> (tempfile::TempDir, tempfile::TempDir, Store, i64) {
    let root = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["commit", "--allow-empty", "-qm", "base"]);
    git(
        root.path(),
        &[
            "worktree",
            "add",
            "--detach",
            target.path().to_str().unwrap(),
        ],
    );
    write(root.path(), "AGENTS.md", "source");
    write(root.path(), ".agents/skills/shared/SKILL.md", "reference");
    write(target.path(), ".agents/skills/own/SKILL.md", "keep");
    write(target.path(), "CLAUDE.md", "target only");
    let mut s = Store::memory().unwrap();
    let p = register(&mut s, root.path());
    (root, target, s, p)
}
#[test]
fn convergence_checks_both_roots_and_membership_and_keeps_target_extras() {
    let (root, target, mut s, p) = linked_fixture();
    let foreign = tempfile::tempdir().unwrap();
    let preview = |s: &mut Store| {
        toolbox::preview_convergence(
            s,
            p,
            root.path().to_str().unwrap(),
            target.path().to_str().unwrap(),
        )
        .unwrap()
    };
    assert!(toolbox::preview_convergence(
        &mut s,
        p,
        root.path().to_str().unwrap(),
        foreign.path().to_str().unwrap()
    )
    .is_err());
    let saved = preview(&mut s);
    write(root.path(), "AGENTS.md", "new source");
    let done = toolbox::apply_operation(&mut s, saved.id, "converge", None).unwrap();
    assert_eq!(done.state, "refused");
    assert!(!target.path().join("AGENTS.md").exists());
    let saved = preview(&mut s);
    write(target.path(), ".agents/skills/own/SKILL.md", "newer own");
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "converge", None)
            .unwrap()
            .state,
        "refused"
    );
    let saved = preview(&mut s);
    let chat = s
        .create_chat(ai_team_core::NewChat {
            project_id: p,
            workspace: target.path().into(),
            provider: ai_team_core::Provider::Local,
            model: "offline".into(),
            reasoning: ai_team_core::Reasoning::High,
        })
        .unwrap();
    let turn = s
        .begin_chat_turn(
            chat.id,
            "hold checkout",
            "fixture",
            &ai_team_core::ModelRegistry::local_only(),
        )
        .unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "converge", None)
            .unwrap()
            .state,
        "refused",
        "active chat owns the target"
    );
    s.finish_chat_turn(chat.id, turn.node_id, ai_team_core::NodeStatus::Done, None)
        .unwrap();
    let saved = preview(&mut s);
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "converge", None)
            .unwrap()
            .state,
        "applied"
    );
    assert_eq!(
        fs::read_to_string(target.path().join(".agents/skills/own/SKILL.md")).unwrap(),
        "newer own"
    );
    assert_eq!(
        fs::read_to_string(target.path().join("AGENTS.md")).unwrap(),
        "new source"
    );
    assert_eq!(
        fs::read_to_string(target.path().join("CLAUDE.md")).unwrap(),
        "target only"
    );
    assert!(preview(&mut s).changes.iter().all(|c| c.effects.is_empty()));
}
#[test]
fn convergence_keeps_layouts_but_replaces_config_files_and_is_bounded() {
    let (root, target, mut s, p) = linked_fixture();
    let preview = |s: &mut Store| {
        toolbox::preview_convergence(
            s,
            p,
            root.path().to_str().unwrap(),
            target.path().to_str().unwrap(),
        )
        .unwrap()
    };
    write(
        root.path(),
        ".claude/skills/copy/SKILL.md",
        "independent reference",
    );
    fs::create_dir_all(target.path().join(".claude")).unwrap();
    std::os::unix::fs::symlink("../.agents/skills", target.path().join(".claude/skills")).unwrap();
    let layout = preview(&mut s);
    assert!(layout.warnings.iter().any(|w| w.contains("layout differs")));
    write(
        root.path(),
        ".mcp.json",
        r#"{"mcpServers":{"reference":{"command":"never-execute"}}}"#,
    );
    write(
        target.path(),
        ".mcp.json",
        r#"{"mcpServers":{"target-only-field":{"command":"never-execute-either"}}}"#,
    );
    let config = preview(&mut s);
    assert!(config.warnings.iter().any(|w| w.contains("not merged")));
    let effect = config
        .changes
        .iter()
        .flat_map(|c| &c.effects)
        .find(|e| e.path == ".mcp.json")
        .unwrap();
    assert!(
        matches!(&effect.after,toolbox::Node::File{contents,..} if String::from_utf8_lossy(contents).contains("reference") && !String::from_utf8_lossy(contents).contains("target-only-field"))
    );
    fs::write(
        target.path().join(".agents/oversized"),
        vec![0; 16 * 1024 * 1024 + 1],
    )
    .unwrap();
    assert!(toolbox::preview_convergence(
        &mut s,
        p,
        root.path().to_str().unwrap(),
        target.path().to_str().unwrap()
    )
    .is_err());
}
#[test]
fn legacy_pi_import_normalizes_known_shapes_and_preserves_original_and_unknown_fields() {
    let root = tempfile::tempdir().unwrap();
    let mut s = Store::memory().unwrap();
    let p = register(&mut s, root.path());
    let old = r#"{"settings":{"scriptMode":true},"custom":"keep","mcpServers":{"remote":{"url":"https://example.invalid/mcp","transport":"sse","auth":{"type":"oauth"},"headers":{"Authorization":"!.pi/mcp/helper.sh TOKEN"}}}}"#;
    write(root.path(), ".pi/mcp.json", old);
    write(
        root.path(),
        ".pi/mcp-adapter.json",
        r#"{"mcpServers":{"own":{"command":"never-execute"}}}"#,
    );
    let proposal = toolbox::preview(
        &mut s,
        p,
        root.path().to_str().unwrap(),
        Selection::ImportPi,
    )
    .unwrap();
    assert_eq!(
        toolbox::apply(&mut s, p, proposal.id).unwrap().state,
        "applied"
    );
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.path().join(".pi/mcp-adapter.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(value["custom"], "keep");
    assert_eq!(value["mcpServers"]["remote"]["httpTransport"], "sse");
    assert_eq!(value["mcpServers"]["remote"]["auth"], "oauth");
    assert!(value["mcpServers"]["remote"].get("transport").is_none());
    assert_eq!(
        fs::read_to_string(root.path().join(".pi/mcp.json")).unwrap(),
        old
    );
    write(
        root.path(),
        ".pi/mcp-adapter.json",
        r#"{"mcpServers":{"remote":{"command":"keep local transport"}}}"#,
    );
    assert!(
        toolbox::preview(
            &mut s,
            p,
            root.path().to_str().unwrap(),
            Selection::ImportPi
        )
        .is_err(),
        "destination transport conflict must not create command+url"
    );
    write(root.path(), ".pi/mcp-adapter.json", "{}");
    write(
        root.path(),
        ".mcp.json",
        r#"{"mcpServers":{"remote":{"command":"keep local transport"}}}"#,
    );
    assert!(toolbox::preview(
        &mut s,
        p,
        root.path().to_str().unwrap(),
        Selection::ImportPi
    )
    .unwrap_err()
    .to_string()
    .contains("conflict"));
}
#[test]
fn discovery_reports_entry_limit_even_when_no_child_directory_is_queued() {
    let root = tempfile::tempdir().unwrap();
    let s = Store::memory().unwrap();
    for i in 0..4100 {
        fs::write(root.path().join(format!("file-{i}")), "").unwrap();
    }
    let result = toolbox::discover(&s, vec![root.path().to_string_lossy().into_owned()]).unwrap();
    assert!(
        result.warnings.iter().any(|w| w.contains("limit")),
        "truncated discovery must never look complete"
    );
}
#[test]
fn discovery_is_bounded_read_only_and_registry_forgetting_is_reversible() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("repository");
    fs::create_dir_all(root.join(".git")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), parent.path().join("external")).unwrap();
    let mut s = Store::memory().unwrap();
    let p = register(&mut s, &root);
    let found = toolbox::discover(&s, vec![parent.path().to_string_lossy().into_owned()]).unwrap();
    assert_eq!(found.repositories.len(), 1);
    assert!(s.toolbox_scan_roots().unwrap().is_empty());
    let saved =
        toolbox::preview_registry(&mut s, RegistrySelection::ScanRoots { roots: found.roots })
            .unwrap();
    toolbox::apply_operation(&mut s, saved.id, "registry", None).unwrap();
    assert_eq!(s.toolbox_scan_roots().unwrap().len(), 1);
    let saved =
        toolbox::preview_registry(&mut s, RegistrySelection::Forget { projects: vec![p] }).unwrap();
    s.set_project_status(p, ProjectStatus::Paused).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "registry", None)
            .unwrap()
            .state,
        "refused"
    );
    let saved =
        toolbox::preview_registry(&mut s, RegistrySelection::Forget { projects: vec![p] }).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "registry", None)
            .unwrap()
            .state,
        "applied"
    );
    assert_eq!(s.project(p).unwrap().status, ProjectStatus::Archived);
    assert!(root.is_dir());
    let saved = toolbox::preview_registry(&mut s, RegistrySelection::Restore { projects: vec![p] })
        .unwrap();
    toolbox::apply_operation(&mut s, saved.id, "registry", None).unwrap();
    assert_eq!(s.project(p).unwrap().status, ProjectStatus::Paused);
    fs::remove_dir(root.join(".git")).unwrap();
    fs::remove_dir(&root).unwrap();
    let saved = toolbox::preview_registry(&mut s, RegistrySelection::Prune).unwrap();
    assert_eq!(saved.registrations.len(), 1);
    fs::create_dir(&root).unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "registry", None)
            .unwrap()
            .state,
        "refused"
    );
}
