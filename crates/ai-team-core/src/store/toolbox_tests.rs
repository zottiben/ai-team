use super::Store;
use crate::{toolbox, RoleModelDefault};

#[test]
fn interrupted_apply_is_visible_and_cannot_be_replayed_or_superseded() {
    let repo = tempfile::tempdir().unwrap();
    let mut store = Store::memory().unwrap();
    let project = crate::register_project(
        &mut store,
        repo.path(),
        None,
        None,
        RoleModelDefault::local_floor,
    )
    .unwrap()
    .project
    .id;
    let selection = || {
        serde_json::from_value(serde_json::json!({"operation":"install","harnesses":["pi"],"hooks":[],"mcp":[],"skills":[],"scaffold":true})).unwrap()
    };
    let first = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    let second = toolbox::preview(
        &mut store,
        project,
        repo.path().to_str().unwrap(),
        selection(),
    )
    .unwrap();
    store.claim_toolbox_preview(project, first.id).unwrap();
    assert!(store.claim_toolbox_preview(project, first.id).is_err());
    assert!(store.claim_toolbox_preview(project, second.id).is_err());
    assert_eq!(store.toolbox_history(project).unwrap()[0].state, "applying");
    assert!(!repo.path().join("AGENTS.md").exists());
    assert!(store
        .db_mut()
        .write(|tx| {
            tx.execute(
                "UPDATE toolbox_preview SET snapshot_json='{}' WHERE id=?1",
                [first.id],
            )?;
            Ok(())
        })
        .is_err());
}
