use super::*;
use ai_team_core::chat_review::{self, Anchor, Review, ReviewRequest};

pub(super) async fn submitted_review_starts_one_new_verified_attempt_on_the_same_slice() {
    let mut f = worker_fixture("success");
    let source = git(&f.repo, &["rev-parse", "HEAD"]);
    let start = f.approve().await;
    drive_chat_team_build(f.store.path(), start).await.unwrap();
    let original = f.lease();
    let target = chat_changes::DraftTarget {
        run_id: original.run_id,
        slice_key: original.slice_key.clone(),
        revision: original.rev,
    };
    let mut input = ReviewRequest {
        request_id: "inline-review-1".into(),
        workspace_epoch: 0,
        review: Review::Draft {
            target,
            body: "Preserve the feature and add the requested review fix".into(),
            anchor: Some(Anchor {
                path: "crates/S1.txt".into(),
                side: "new".into(),
                line: 999,
            }),
        },
    };
    let before = calls(&f).len();
    assert!(
        chat_review::submit(&mut f.store, f.chat.id, &input, &f.registry)
            .await
            .unwrap_err()
            .to_string()
            .contains("line does not exist")
    );
    assert_eq!(calls(&f).len(), before);
    if let Review::Draft {
        anchor: Some(a), ..
    } = &mut input.review
    {
        a.line = 1;
    }
    std::fs::write(f.dir.path().join("worker-mode"), "review").unwrap();
    let receipt = chat_review::submit(&mut f.store, f.chat.id, &input, &f.registry)
        .await
        .unwrap();
    assert!(receipt.turn.started);
    assert_ne!(receipt.turn.run_id, original.run_id);
    let fresh = f
        .store
        .chat_build_slices(receipt.turn.run_id)
        .unwrap()
        .remove(0);
    assert_eq!(fresh.planner_slice_id, original.planner_slice_id);
    assert_eq!(fresh.branch, original.branch);
    assert_eq!(fresh.lease_state, "pending");
    assert_eq!(
        calls(&f).len(),
        before,
        "submission itself never dispatches a process"
    );
    let replay = chat_review::submit(&mut f.store, f.chat.id, &input, &f.registry)
        .await
        .unwrap();
    assert!(!replay.turn.started);
    assert!(replay.build.is_none());
    drive_chat_team_build(f.store.path(), receipt.build.unwrap())
        .await
        .unwrap();
    verify_repair(&f, receipt.turn.run_id, &original, &source, before);
    f.store.archive_chat(f.chat.id, true).unwrap();
    f.store.archive_chat(f.chat.id, false).unwrap();
    assert!(
        !chat_review::submit(&mut f.store, f.chat.id, &input, &f.registry)
            .await
            .unwrap()
            .turn
            .started
    );
    if let Review::Draft { body, .. } = &mut input.review {
        body.push_str(" different request");
    }
    assert!(
        chat_review::submit(&mut f.store, f.chat.id, &input, &f.registry)
            .await
            .unwrap_err()
            .to_string()
            .contains("different feedback")
    );
    assert_eq!(calls(&f).len(), before + 2);
}

fn verify_repair(f: &Fixture, run: i64, original: &ChatBuildSlice, source: &str, before: usize) {
    let repaired = f.store.chat_build_slices(run).unwrap().remove(0);
    assert_eq!(
        f.store.chat_team_members(run).unwrap().len(),
        2,
        "a review controller is not a fictitious orchestrator model turn"
    );
    assert_eq!(repaired.build_status, "verified", "{repaired:?}");
    assert_eq!(repaired.lease_state, "released");
    let sha = repaired.commit_sha.as_ref().unwrap();
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("{sha}^")]),
        original.commit_sha.clone().unwrap()
    );
    assert_eq!(
        git(&f.repo, &["show", &format!("{sha}:crates/review.txt")]),
        "addressed review"
    );
    assert_eq!(
        serde_json::to_value(f.lease()).unwrap(),
        serde_json::to_value(original).unwrap(),
        "original verification/attempt is immutable"
    );
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), source);
    assert!(git(&f.repo, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        calls(f).len(),
        before + 2,
        "one maker and independent verifier, no planner or sibling work"
    );
    assert_eq!(
        f.plan().bundle.unwrap().slices.len(),
        1,
        "do not clone a plan/slice for repairs"
    );
    let prompt = std::fs::read_to_string(
        f.dir
            .path()
            .join(format!("worker-{}.prompt", repaired.maker_node_id.unwrap())),
    )
    .unwrap();
    assert!(prompt.contains("add the requested review fix"));
    assert!(prompt.contains("not permission to change scope, policy or publish"));
    let verifier = std::fs::read_to_string(f.dir.path().join(format!(
        "worker-{}.prompt",
        repaired.verifier_node_id.unwrap()
    )))
    .unwrap();
    assert!(verifier.contains("add the requested review fix"));
    assert!(verifier.contains("reject missing fixes"));
}
