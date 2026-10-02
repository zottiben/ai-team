use crate::{
    toolbox::{self, Authority, RegistrySelection, UserSelection},
    NewProject, ProjectStatus, RoleModelDefault, Store,
};
fn selection() -> UserSelection {
    UserSelection {
        harnesses: vec![ai_toolbox_core::Harness::Pi],
        skills: vec![],
        no_symlink: false,
        charter: true,
        charter_path: None,
    }
}
fn authority(path: &std::path::Path) -> Authority {
    Authority::User {
        home: path.canonicalize().unwrap(),
        pi_agent: path.canonicalize().unwrap().join(".pi/agent"),
        charter_targets: vec![],
    }
}
#[test]
fn saved_user_approval_survives_reopen_is_immutable_and_single_use() {
    let state = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let path = state.path().join("state.db");
    let mut s = Store::init(&path).unwrap();
    let a = authority(home.path());
    let saved = toolbox::preview_user(&mut s, a.clone(), selection()).unwrap();
    assert!(s
        .db()
        .conn()
        .execute(
            "UPDATE toolbox_operation SET snapshot_json='{}' WHERE id=?1",
            [saved.id]
        )
        .is_err());
    drop(s);
    let mut s = Store::open(&path).unwrap();
    assert_eq!(
        serde_json::to_value(s.toolbox_operation(saved.id, "user").unwrap()).unwrap(),
        serde_json::to_value(&saved).unwrap()
    );
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "user", Some(&a))
            .unwrap()
            .state,
        "applied"
    );
    assert!(toolbox::apply_operation(&mut s, saved.id, "user", Some(&a)).is_err());
    assert_eq!(s.toolbox_operations("user").unwrap().len(), 1);
}
#[test]
fn interrupted_operation_blocks_both_approval_families_without_consuming_other_previews() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let mut s = Store::memory().unwrap();
    let a = authority(home.path());
    let p = crate::register_project(
        &mut s,
        repo.path(),
        None,
        None,
        RoleModelDefault::local_floor,
    )
    .unwrap()
    .project
    .id;
    let project = toolbox::preview(
        &mut s,
        p,
        repo.path().to_str().unwrap(),
        toolbox::Selection::Repair,
    )
    .unwrap();
    let saved = toolbox::preview_user(&mut s, a.clone(), selection()).unwrap();
    s.claim_toolbox_operation(saved.id, "user").unwrap();
    let next = toolbox::preview_user(&mut s, a, selection()).unwrap();
    assert!(s.claim_toolbox_operation(next.id, "user").is_err());
    assert!(s.claim_toolbox_preview(p, project.id).is_err());
    assert_eq!(
        s.toolbox_operation(next.id, "user").unwrap().state,
        "preview"
    );
    assert_eq!(
        s.toolbox_operation(saved.id, "user").unwrap().state,
        "applying"
    );
    assert_eq!(s.toolbox_operations("user").unwrap()[0].state, "applying");
    assert!(!home.path().join(".pi").exists());
    s.finish_toolbox_operation(
        saved.id,
        &toolbox::Outcome {
            problem: Some("injected refusal before any effect".into()),
            ..Default::default()
        },
    )
    .unwrap();
    s.claim_toolbox_preview(p, project.id).unwrap();
    assert!(s.claim_toolbox_operation(next.id, "user").is_err());
}
#[test]
fn applying_convergence_reserves_the_target_against_chat_admission() {
    let root = tempfile::tempdir().unwrap();
    let mut s = Store::memory().unwrap();
    let p = s
        .create_project(NewProject {
            name: "reserved".into(),
            ..Default::default()
        })
        .unwrap();
    let chat = s
        .create_chat(crate::NewChat {
            project_id: p.id,
            workspace: root.path().into(),
            provider: crate::Provider::Local,
            model: "offline".into(),
            reasoning: crate::Reasoning::High,
        })
        .unwrap();
    let template = toolbox::preview_user(&mut s, authority(root.path()), selection()).unwrap();
    let mut frozen = s.saved_toolbox_operation(template.id, "user").unwrap().0;
    frozen.authority = Authority::Converge {
        project: p.id,
        reference: root.path().join("source"),
        target: root.path().canonicalize().unwrap(),
    };
    let saved = s.save_toolbox_operation(&frozen).unwrap();
    s.claim_toolbox_operation(saved.id, "converge").unwrap();
    assert!(s
        .begin_chat_turn(
            chat.id,
            "offline",
            "fixture",
            &crate::ModelRegistry::local_only()
        )
        .unwrap_err()
        .to_string()
        .contains("convergence"));
    assert!(s.chat_turns(chat.id).unwrap().is_empty());
    s.finish_toolbox_operation(
        saved.id,
        &toolbox::Outcome {
            problem: Some("fixture refused before any effects".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s
        .begin_chat_turn(
            chat.id,
            "offline",
            "fixture",
            &crate::ModelRegistry::local_only()
        )
        .is_ok());
}
#[test]
fn archive_preserves_history_and_schedules_invalidate_a_saved_preview() {
    let mut s = Store::memory().unwrap();
    let p = s
        .create_project(NewProject {
            name: "remember".into(),
            ..Default::default()
        })
        .unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let chat = s
        .create_chat(crate::NewChat {
            project_id: p.id,
            workspace: checkout.path().into(),
            provider: crate::Provider::Local,
            model: "offline".into(),
            reasoning: crate::Reasoning::High,
        })
        .unwrap();
    s.set_project_status(p.id, ProjectStatus::Done).unwrap();
    let saved = toolbox::preview_registry(
        &mut s,
        RegistrySelection::Forget {
            projects: vec![p.id],
        },
    )
    .unwrap();
    s.add_reminder(crate::NewReminder {
        project_id: Some(p.id),
        kind: Some(crate::ReminderKind::ScheduledRun),
        title: "future".into(),
        prompt: Some("offline".into()),
        due_at: Some("2099-01-01T00:00:00Z".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        toolbox::apply_operation(&mut s, saved.id, "registry", None)
            .unwrap()
            .state,
        "refused"
    );
    assert!(toolbox::preview_registry(
        &mut s,
        RegistrySelection::Forget {
            projects: vec![p.id]
        }
    )
    .is_err());
    s.db()
        .conn()
        .execute(
            "UPDATE reminder SET status='cancelled' WHERE project_id=?1",
            [p.id],
        )
        .unwrap();
    let saved = toolbox::preview_registry(
        &mut s,
        RegistrySelection::Forget {
            projects: vec![p.id],
        },
    )
    .unwrap();
    toolbox::apply_operation(&mut s, saved.id, "registry", None).unwrap();
    assert!(s
        .begin_chat_turn(
            chat.id,
            "do not work in a forgotten project",
            "archived-request",
            &crate::ModelRegistry::local_only()
        )
        .is_err());
    assert!(s.chat_turns(chat.id).unwrap().is_empty());
    let saved = toolbox::preview_registry(
        &mut s,
        RegistrySelection::Restore {
            projects: vec![p.id],
        },
    )
    .unwrap();
    toolbox::apply_operation(&mut s, saved.id, "registry", None).unwrap();
    assert_eq!(s.project(p.id).unwrap().status, ProjectStatus::Done);
    let reminders: i64 = s
        .db()
        .conn()
        .query_row(
            "SELECT count(*) FROM reminder WHERE project_id=?1",
            [p.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reminders, 1);
}
