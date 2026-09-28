use ai_team_core::planning::{ChatPlan, PlanAccess, PlanAction, PlanActor};
use ai_team_core::{ModelRegistry, NewChat, NewProject, NodeStatus, Provider, Reasoning, Store};
use serde_json::json;

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    chats: [i64; 2],
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::init(&dir.path().join("team.db")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Planning".into(),
                ..Default::default()
            })
            .unwrap();
        let mut create = || {
            store
                .create_chat(NewChat {
                    project_id: project.id,
                    workspace: dir.path().into(),
                    provider: Provider::Local,
                    model: "fixture".into(),
                    reasoning: Reasoning::High,
                })
                .unwrap()
                .id
        };
        let chats = [create(), create()];
        Self { dir, store, chats }
    }
    fn get(&self, id: i64) -> ChatPlan {
        self.store.chat_plan(id, PlanActor::Human).unwrap()
    }
    fn apply(&mut self, id: i64, mut action: serde_json::Value) -> ChatPlan {
        action["expect_revision"] = self.get(id).revision.into();
        self.store
            .change_chat_plan(
                id,
                PlanActor::Human,
                serde_json::from_value(action).unwrap(),
            )
            .unwrap()
    }
    fn plan(&mut self, id: i64) -> ChatPlan {
        self.apply(
            id,
            json!({"action":"create_plan","title":"Build it","summary":"A scoped plan"}),
        )
    }
}

#[test]
fn two_chats_in_one_checkout_own_separate_durable_plans_and_questions() {
    let mut f = Fixture::new();
    let [first, other] = f.chats;
    assert!(f.get(first).bundle.is_none());
    assert!(
        !f.store.planning_path().unwrap().exists(),
        "GET is not initialization"
    );
    f.plan(first);
    f.plan(other);
    assert_ne!(
        f.get(first).bundle.unwrap().plan.id,
        f.get(other).bundle.unwrap().plan.id
    );
    f.apply(first, json!({"action":"add_slice","key":"S1","title":"First step","scope":"Implement","touches":["src/**"],"demo":"Run tests"}));
    let asked = f.apply(
        first,
        json!({"action":"open_question","body":"Which variant?","slice":"S1"}),
    );
    let question = asked.bundle.unwrap().questions[0].id;
    let wrong = serde_json::from_value(json!({"action":"answer_question","expect_revision": f.get(other).revision,"question_id":question,"answer":"Wrong chat"})).unwrap();
    assert!(f
        .store
        .change_chat_plan(other, PlanActor::Human, wrong)
        .is_err());
    f.apply(
        first,
        json!({"action":"answer_question","question_id":question,"answer":"Use the small variant"}),
    );
    let reloaded = Store::open(f.store.path())
        .unwrap()
        .chat_plan(first, PlanActor::Human)
        .unwrap();
    let bundle = reloaded.bundle.unwrap();
    assert_eq!(
        bundle.questions[0].answer.as_deref(),
        Some("Use the small variant")
    );
    assert!(bundle.slices[0].scope_md.ends_with("Touches: src/**"));
    assert!(f.get(other).bundle.unwrap().slices.is_empty());
    assert!(f.store.planning_revision().unwrap() > reloaded.revision);
}

#[test]
fn similar_chat_ids_never_resolve_by_fuzzy_slug_or_cross_project_fallback() {
    let mut f = Fixture::new();
    let first = f.chats[0];
    f.plan(f.chats[1]); // Ensure the first project's engine repo already exists.
    let project = f
        .store
        .create_project(NewProject {
            name: "Other project".into(),
            ..Default::default()
        })
        .unwrap();
    let mut last = 0;
    for _ in 0..9 {
        last = f
            .store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: f.dir.path().into(),
                provider: Provider::Local,
                model: "fixture".into(),
                reasoning: Reasoning::High,
            })
            .unwrap()
            .id;
    }
    assert_eq!(last, 11);
    f.plan(last);
    assert!(
        f.get(first).bundle.is_none(),
        "chat-1 must not discover chat-11, even in another project"
    );
    f.plan(first);
    assert_ne!(
        f.get(first).bundle.unwrap().plan.id,
        f.get(last).bundle.unwrap().plan.id
    );
}

#[test]
fn stale_writes_are_rejected_but_other_chats_do_not_invalidate_this_plan() {
    let mut f = Fixture::new();
    let [first, other] = f.chats;
    let initial = f.plan(first);
    f.plan(other);
    let action: PlanAction = serde_json::from_value(json!({"action":"write_section","expect_revision":initial.revision,"key":"outcome","title":"Outcome","body":"Keep this"})).unwrap();
    f.store
        .change_chat_plan(first, PlanActor::Human, action.clone())
        .unwrap();
    let mut second_connection = Store::open(f.store.path()).unwrap();
    assert!(second_connection
        .change_chat_plan(first, PlanActor::Human, action)
        .unwrap_err()
        .to_string()
        .contains("changed since"));
    assert_eq!(f.get(first).bundle.unwrap().sections[0].body, "Keep this");
}

#[test]
fn agent_tools_cannot_escape_chat_ownership_answer_for_a_human_or_survive_settlement() {
    let mut f = Fixture::new();
    let [first, other] = f.chats;
    let initial = f.plan(first);
    let turn = f
        .store
        .begin_chat_turn(first, "Plan it", "one", &ModelRegistry::local_only())
        .unwrap();
    let agent = PlanActor::Agent(turn.node_id);
    assert_eq!(
        f.store.planning_access(first, agent).unwrap(),
        PlanAccess::Planner
    );
    assert!(f.store.chat_plan(other, agent).is_err());
    let answer = serde_json::from_value(json!({"action":"answer_question","expect_revision":initial.revision,"question_id":1,"answer":"Approve myself"})).unwrap();
    assert!(f.store.change_chat_plan(first, agent, answer).is_err());
    f.store
        .finish_chat_turn(first, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    assert!(f.store.chat_plan(first, agent).is_err());
    assert!(!PlanAccess::Maker.tools().contains(&"add_slice"));
    assert_eq!(PlanAccess::Reader.tools(), ["get_plan"]);
    let forged =
        json!({"action":"create_plan","expect_revision":0,"title":"Escape","project_id":999});
    assert!(serde_json::from_value::<PlanAction>(forged).is_err());
}

#[test]
fn maker_reader_stopping_and_archived_permissions_are_enforced_by_the_service() {
    let mut f = Fixture::new();
    let id = f.chats[0];
    f.plan(id);
    for key in ["S1", "S2"] {
        f.apply(id, json!({"action":"add_slice","key":key,"title":"Work","scope":"Build","touches":["src/**"],"demo":"Test"}));
    }
    let turn = f
        .store
        .begin_chat_turn(id, "Work", "role-test", &ModelRegistry::local_only())
        .unwrap();
    let actor = PlanActor::Agent(turn.node_id);
    // Fixture role assignments model future chat team seats without starting agents.
    let conn = rusqlite::Connection::open(f.store.path()).unwrap();
    conn.execute(
        "UPDATE node_run SET role = 'verifier' WHERE id = ?1",
        [turn.node_id],
    )
    .unwrap();
    assert_eq!(
        f.store.planning_access(id, actor).unwrap(),
        PlanAccess::Reader
    );
    let status = |key: &str, revision| {
        serde_json::from_value(json!({"action":"set_slice_status","key":key,"status":"active","expect_revision":revision})).unwrap()
    };
    assert!(f
        .store
        .change_chat_plan(id, actor, status("S1", f.get(id).revision))
        .unwrap_err()
        .to_string()
        .contains("not allowed"));
    conn.execute(
        "UPDATE node_run SET role = 'frontend', slice_key = 'S1' WHERE id = ?1",
        [turn.node_id],
    )
    .unwrap();
    assert_eq!(
        f.store.planning_access(id, actor).unwrap(),
        PlanAccess::Maker
    );
    assert!(f
        .store
        .change_chat_plan(id, actor, status("S2", f.get(id).revision))
        .unwrap_err()
        .to_string()
        .contains("assigned slice"));
    f.store
        .change_chat_plan(id, actor, status("S1", f.get(id).revision))
        .unwrap();
    f.store.request_chat_stop(id, turn.node_id).unwrap();
    assert!(f.store.chat_plan(id, actor).is_err());
    f.store
        .finish_chat_turn(id, turn.node_id, NodeStatus::Cancelled, None)
        .unwrap();
    f.store.archive_chat(id, true).unwrap();
    let revision = f.get(id).revision;
    assert!(f
        .store
        .change_chat_plan(id, PlanActor::Human, status("S1", revision))
        .unwrap_err()
        .to_string()
        .contains("read-only"));
}

#[test]
fn an_incomplete_revision_schema_is_rejected_rather_than_weakening_concurrency() {
    let mut f = Fixture::new();
    f.plan(f.chats[0]);
    let conn = rusqlite::Connection::open(f.store.planning_path().unwrap()).unwrap();
    conn.execute_batch("DROP TRIGGER ai_team_plan_UPDATE")
        .unwrap();
    assert!(
        f.store.chat_plan(f.chats[0], PlanActor::Human).is_err(),
        "an owned marker alone is not a complete schema"
    );
}

#[test]
fn standalone_files_and_symlink_targets_cannot_be_adopted_as_embedded_storage() {
    let mut f = Fixture::new();
    let path = f.store.planning_path().unwrap();
    let external = f.dir.path().join("standalone.db");
    drop(ai_planner_core::Store::init(&external).unwrap());
    let before = std::fs::read(&external).unwrap();
    assert!(Store::open_planning_host(&external).is_err());
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &path).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(&external, &path).unwrap();
    let action = serde_json::from_value(
        json!({"action":"create_plan","expect_revision":0,"title":"Do not adopt"}),
    )
    .unwrap();
    assert!(f
        .store
        .change_chat_plan(f.chats[0], PlanActor::Human, action)
        .is_err());
    assert_eq!(std::fs::read(external).unwrap(), before);
}
