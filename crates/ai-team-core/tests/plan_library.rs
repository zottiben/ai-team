//! The cross-project board, and importing a standalone plan into one chosen chat (D27).
//!
//! The source database in these tests is a scratch fixture built with the engine, never
//! the operator's own planner: the whole point of the feature is that somebody else's
//! database is read and left alone, and a test that reached for the real one would be
//! writing to the thing it claims not to touch.

use std::path::{Path, PathBuf};

use ai_planner_core as planner;
use ai_team_core::plan_library::{PlanImportRequest, PlanImported, PlanLibraryFilter};
use ai_team_core::planning::{PlanActor, PlanStatus};
use ai_team_core::{ModelRegistry, NewChat, NewProject, NodeStatus, Provider, Reasoning, Store};

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    project: i64,
    chats: [i64; 3],
    source: PathBuf,
    /// The plan in the source database that the tests import.
    plan: i64,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::init(&dir.path().join("team.db")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
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
        let chats = [create(), create(), create()];
        let source = dir.path().join("standalone").join("planner.db");
        let plan = seed_source(&source);
        Self {
            dir,
            store,
            project: project.id,
            chats,
            source,
            plan,
        }
    }

    fn path(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn import_into(&mut self, chat: i64) -> ai_team_core::Result<PlanImported> {
        let preview = self.store.preview_plan_import(&self.path(), self.plan)?;
        self.store.import_plan(&PlanImportRequest {
            path: self.path(),
            plan_id: self.plan,
            chat_id: chat,
            fingerprint: preview.fingerprint,
        })
    }

    /// The owned planning store, read directly, to check what was actually written.
    fn owned(&self) -> planner::Store {
        planner::Store::open(&self.store.planning_path().unwrap()).unwrap()
    }
}

/// A standalone planner database with the awkward parts of a real one: a claimed slice,
/// a delivered one, backdated progress notes, an answered question, a handoff, learned
/// affinities and a raw markdown original.
fn seed_source(path: &Path) -> i64 {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut store = planner::Store::init(path).unwrap();
    store.set_actor("someone-else");
    let repo = store
        .ensure_repo(&planner::GitContext {
            repo_key: "git:github.com/acme/widget".into(),
            repo_name: "widget".into(),
            remote_url: Some("git@github.com:acme/widget.git".into()),
            main_path: PathBuf::from("/elsewhere/widget"),
            worktree: PathBuf::from("/elsewhere/widget"),
            branch: Some("main".into()),
            head_sha: None,
        })
        .unwrap();
    let plan = store
        .create_plan(planner::NewPlan {
            repo_id: repo.id,
            title: "Ship the widget".into(),
            slug: Some("ship-the-widget".into()),
            status: Some(planner::Status::Active),
            summary: Some("Three pull requests and a migration.".into()),
            ticket_key: Some("WID-14".into()),
            ticket_url: Some("https://tickets.example/WID-14".into()),
            base_branch: Some("main".into()),
            owner: Some("someone-else".into()),
            raw_md: Some("# Ship the widget\n\nThe original markdown.\n".into()),
            source_path: Some("/elsewhere/widget/BUILD_PLAN.md".into()),
            ..Default::default()
        })
        .unwrap();
    store
        .set_section(
            plan.id,
            planner::SectionWrite {
                key: "outcome",
                title: Some("Outcome"),
                body: "A widget that ships.",
                ..Default::default()
            },
        )
        .unwrap();
    store
        .add_decision(planner::NewDecision {
            plan_id: plan.id,
            key: Some("D1".into()),
            title: "One queue, not two".into(),
            body: "Two queues drifted.".into(),
            ..Default::default()
        })
        .unwrap();

    for (key, title) in [
        ("PR1", "Schema"),
        ("PR2", "Server"),
        ("PR3", "Window"),
        ("PR4", "Docs"),
    ] {
        store
            .add_slice(planner::NewSlice {
                plan_id: plan.id,
                key: key.into(),
                title: title.into(),
                scope_md: Some(format!("{title} work.\n\nTouches: src/**")),
                demo_md: Some("Run the gates.".into()),
                ..Default::default()
            })
            .unwrap();
    }

    // PR1 is finished somewhere else, with a branch, a worktree and a pull request.
    let pr1 = store.require_slice(plan.id, "PR1").unwrap();
    let pr1 = store
        .claim_slice(&pr1, "/elsewhere/trees/pr1", Some("widget/pr1-schema"))
        .unwrap();
    let pr1 = store
        .update_slice(
            &pr1,
            planner::SliceUpdate {
                pr_url: Some("https://github.com/acme/widget/pull/1".into()),
                base_branch: Some("main".into()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .set_slice_status(&pr1, planner::Status::Done, None)
        .unwrap();

    // PR2 is claimed and running there right now.
    let pr2 = store.require_slice(plan.id, "PR2").unwrap();
    let pr2 = store
        .claim_slice(&pr2, "/elsewhere/trees/pr2", Some("widget/pr2-server"))
        .unwrap();
    store
        .set_slice_status(&pr2, planner::Status::Active, None)
        .unwrap();

    // PR4 is blocked, with a reason worth keeping.
    let pr4 = store.require_slice(plan.id, "PR4").unwrap();
    store
        .set_slice_status(&pr4, planner::Status::Blocked, Some("waiting on the API"))
        .unwrap();

    let pr3 = store.require_slice(plan.id, "PR3").unwrap();
    let asked = store
        .add_question(plan.id, Some(pr3.id), "Which variant of the widget?")
        .unwrap();
    store.answer_question(asked, "The small one.").unwrap();
    store
        .add_question(plan.id, None, "Who signs the release off?")
        .unwrap();
    store
        .add_gotcha(plan.id, "The index is case folded", "APFS, again.")
        .unwrap();
    store
        .append_log(planner::NewLog {
            plan_id: plan.id,
            slice_id: Some(pr1.id),
            kind: Some(planner::LogKind::Verification),
            body: "Gates green on PR1".into(),
            branch: Some("widget/pr1-schema".into()),
            worktree_path: Some("/elsewhere/trees/pr1".into()),
            at: Some("2026-01-02T03:04:05Z".into()),
        })
        .unwrap();
    store
        .write_handoff(planner::NewHandoff {
            plan_id: plan.id,
            worktree_path: "/elsewhere/trees/pr2".into(),
            branch: Some("widget/pr2-server".into()),
            head_sha: Some("abc123".into()),
            gates: vec![planner::Gate::parse("test=pass:812 tests")],
            resume_md: "Pick up at the handler.".into(),
            next_md: "Write the handler test first.".into(),
        })
        .unwrap();
    store
        .record_affinity(plan.id, repo.id, Some("main"), "/elsewhere/widget")
        .unwrap();

    // A second plan, so choosing one is a real choice.
    store
        .create_plan(planner::NewPlan {
            repo_id: repo.id,
            title: "Retire the gadget".into(),
            slug: Some("retire-the-gadget".into()),
            ..Default::default()
        })
        .unwrap();

    let id = plan.id;
    // Dropped so the write-ahead log is checkpointed away, which is the state a closed
    // `aip` leaves behind - and the state a read-only connection could not open at all.
    drop(store);
    id
}

fn digest(path: &Path) -> Vec<Vec<u8>> {
    ["", "-wal", "-shm", "-journal"]
        .iter()
        .map(|suffix| {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            std::fs::read(PathBuf::from(name)).unwrap_or_default()
        })
        .collect()
}

#[test]
fn reading_and_importing_a_source_never_writes_a_byte_of_it() {
    let mut f = Fixture::new();
    let before = digest(&f.source);
    assert!(
        before[1].is_empty() && before[2].is_empty(),
        "the fixture must be closed, with no journal beside it"
    );

    let survey = f.store.read_plan_source(&f.path()).unwrap();
    assert_eq!(survey.plans.len(), 2);
    assert_eq!(survey.source.schema_version, 5);
    let preview = f.store.preview_plan_import(&f.path(), f.plan).unwrap();
    assert!(preview.refusal.is_none());
    f.import_into(f.chats[0]).unwrap();

    assert_eq!(
        digest(&f.source),
        before,
        "the source database, and every journal beside it, must be untouched"
    );
}

#[test]
fn an_imported_plan_keeps_every_part_of_its_content_and_its_own_dates() {
    let mut f = Fixture::new();
    let imported = f.import_into(f.chats[0]).unwrap();
    assert_eq!(imported.chat_id, f.chats[0]);
    assert_eq!(imported.slug, format!("chat-{}", f.chats[0]));

    let owned = f.owned();
    let plan = owned
        .find_plan(&format!("chat-{}", f.chats[0]), None)
        .unwrap();
    let bundle = owned.bundle(plan.id).unwrap();
    assert_eq!(bundle.plan.title, "Ship the widget");
    assert_eq!(bundle.plan.status, PlanStatus::Active);
    assert_eq!(bundle.plan.ticket_key.as_deref(), Some("WID-14"));
    assert_eq!(bundle.plan.owner.as_deref(), Some("someone-else"));
    assert_eq!(bundle.plan.base_branch.as_deref(), Some("main"));
    assert_eq!(
        bundle.plan.source_path.as_deref(),
        Some("/elsewhere/widget/BUILD_PLAN.md")
    );
    let raw: Option<String> = owned
        .db()
        .conn()
        .query_row("SELECT raw_md FROM plan WHERE id = ?1", [plan.id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        raw.as_deref(),
        Some("# Ship the widget\n\nThe original markdown.\n"),
        "the markdown the plan was written from is part of the plan"
    );

    assert_eq!(bundle.slices.len(), 4);
    let blocked = bundle.slices.iter().find(|s| s.key == "PR4").unwrap();
    assert_eq!(blocked.status, PlanStatus::Blocked);
    assert_eq!(
        blocked.blocked_reason.as_deref(),
        Some("waiting on the API")
    );
    assert!(bundle
        .slices
        .iter()
        .any(|slice| slice.scope_md.contains("Touches: src/**")));
    assert_eq!(bundle.decisions.len(), 1);
    assert_eq!(bundle.decisions[0].key, "D1");
    assert_eq!(bundle.gotchas.len(), 1);
    assert_eq!(bundle.questions.len(), 2);
    let answered = bundle
        .questions
        .iter()
        .find(|question| question.status == "answered")
        .expect("the answered question keeps its answer");
    assert_eq!(answered.answer.as_deref(), Some("The small one."));
    assert_eq!(answered.slice_key.as_deref(), Some("PR3"));

    let verification = bundle
        .log
        .iter()
        .find(|entry| entry.body == "Gates green on PR1")
        .expect("progress notes come across");
    assert_eq!(
        verification.at, "2026-01-02T03:04:05Z",
        "a note keeps the date it was written, not the date it was imported"
    );
    assert_eq!(verification.actor.as_deref(), Some("someone-else"));
    assert_eq!(verification.slice_key.as_deref(), Some("PR1"));
    assert_eq!(verification.kind, planner::LogKind::Verification);
    assert!(
        bundle.log.len() >= 8,
        "every note the source held, including its status history: {}",
        bundle.log.len()
    );

    let handoffs = owned.handoffs_for(plan.id).unwrap();
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].head_sha.as_deref(), Some("abc123"));
    assert_eq!(handoffs[0].branch.as_deref(), Some("widget/pr2-server"));
    assert_eq!(handoffs[0].worktree_path, "/elsewhere/trees/pr2");
    assert_eq!(handoffs[0].resume_md, "Pick up at the handler.");
    assert_eq!(handoffs[0].next_md, "Write the handler test first.");
    assert_eq!(handoffs[0].gates[0].name, "test");
    assert_eq!(handoffs[0].gates[0].detail.as_deref(), Some("812 tests"));
    assert_eq!(handoffs[0].actor.as_deref(), Some("someone-else"));
    let written_at: String = rusqlite::Connection::open(&f.source)
        .unwrap()
        .query_row("SELECT at FROM handoff LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        handoffs[0].at, written_at,
        "a handoff is history, so it keeps the moment it was taken"
    );

    // An imported plan is an ordinary owned plan: the chat edits it through the same
    // revision-guarded surface, and a stale revision is refused like any other.
    drop(owned);
    let snapshot = f.store.chat_plan(f.chats[0], PlanActor::Human).unwrap();
    assert_eq!(snapshot.revision, imported.revision);
    let stale = serde_json::from_value(serde_json::json!({
        "action": "append_log", "expect_revision": 0, "body": "Mine now"
    }))
    .unwrap();
    assert!(f
        .store
        .change_chat_plan(f.chats[0], PlanActor::Human, stale)
        .is_err());
    let note = serde_json::from_value(serde_json::json!({
        "action": "append_log",
        "expect_revision": snapshot.revision,
        "body": "Picked this up in AI Team"
    }))
    .unwrap();
    let changed = f
        .store
        .change_chat_plan(f.chats[0], PlanActor::Human, note)
        .unwrap();
    assert!(changed
        .bundle
        .unwrap()
        .log
        .iter()
        .any(|entry| entry.body == "Picked this up in AI Team"));

    let owned = f.owned();
    let deps: i64 = owned
        .db()
        .conn()
        .query_row("SELECT COUNT(*) FROM slice_dep", [], |row| row.get(0))
        .unwrap();
    assert_eq!(deps, 0, "the fixture has none, and none were invented");
}

#[test]
fn claims_and_deliveries_arrive_as_evidence_and_never_as_authority() {
    let mut f = Fixture::new();
    let preview = f.store.preview_plan_import(&f.path(), f.plan).unwrap();
    let evidence: Vec<&str> = preview
        .evidence
        .iter()
        .map(|slice| slice.key.as_str())
        .collect();
    assert_eq!(evidence, ["PR1", "PR2"]);
    assert_eq!(
        preview.evidence[0].pr_url.as_deref(),
        Some("https://github.com/acme/widget/pull/1")
    );
    assert!(preview
        .warnings
        .iter()
        .any(|warning| warning.contains("leases nothing, builds nothing and publishes nothing")));
    assert!(preview
        .warnings
        .iter()
        .any(|warning| warning.contains("learned branch and worktree associations")));

    f.import_into(f.chats[0]).unwrap();
    let owned = f.owned();
    let plan = owned
        .find_plan(&format!("chat-{}", f.chats[0]), None)
        .unwrap();
    for slice in owned.slices(plan.id).unwrap() {
        assert_eq!(slice.claimed_by, None, "{} arrived claimed", slice.key);
        assert_eq!(slice.claimed_at, None);
        assert_eq!(slice.worktree_path, None);
        assert_eq!(slice.branch, None);
        assert_eq!(slice.base_branch, None);
        assert_eq!(slice.pr_url, None);
    }
    assert!(
        owned
            .slices_claimed_in("/elsewhere/trees/pr1")
            .unwrap()
            .is_empty(),
        "no lease anywhere answers to this import"
    );

    // The evidence is not lost: it is written into the plan, where it reads as history.
    let record = owned
        .bundle(plan.id)
        .unwrap()
        .sections
        .into_iter()
        .find(|section| section.title == "Imported from ai-planner")
        .expect("an imported plan says where it came from");
    assert!(record.body.contains("/elsewhere/trees/pr1"));
    assert!(record
        .body
        .contains("https://github.com/acme/widget/pull/1"));
    assert!(record.body.contains("widget/pr2-server"));
    assert!(record.body.contains("ship-the-widget"));
    assert!(record.body.contains("Pick up at the handler.") || record.body.contains("abc123"));

    // And no execution state was created on the ai-team side either.
    let conn = rusqlite::Connection::open(f.store.path()).unwrap();
    for table in ["run", "node_run", "chat_turn", "chat_build_slice", "event"] {
        let rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "importing a plan created a row in {table}");
    }
}

#[test]
fn a_preview_read_before_the_source_changed_is_refused() {
    let mut f = Fixture::new();
    let preview = f.store.preview_plan_import(&f.path(), f.plan).unwrap();

    let mut source = planner::Store::open(&f.source).unwrap();
    source.set_actor("someone-else");
    source
        .append_log(planner::NewLog {
            plan_id: f.plan,
            body: "One more thing happened over here".into(),
            ..Default::default()
        })
        .unwrap();
    drop(source);

    let stale = f.store.import_plan(&PlanImportRequest {
        path: f.path(),
        plan_id: f.plan,
        chat_id: f.chats[0],
        fingerprint: preview.fingerprint,
    });
    let message = stale.unwrap_err().to_string();
    assert!(
        message.contains("changed in the source since you previewed it"),
        "{message}"
    );
    assert!(
        f.store
            .chat_plan(f.chats[0], PlanActor::Human)
            .unwrap()
            .bundle
            .is_none(),
        "a refused import writes nothing"
    );

    // Reading it again shows the new state, and that one imports.
    let fresh = f.store.preview_plan_import(&f.path(), f.plan).unwrap();
    assert_eq!(fresh.counts.log, preview.counts.log + 1);
    f.store
        .import_plan(&PlanImportRequest {
            path: f.path(),
            plan_id: f.plan,
            chat_id: f.chats[0],
            fingerprint: fresh.fingerprint,
        })
        .unwrap();
}

#[test]
fn one_plan_is_imported_once_even_from_a_moved_copy_of_its_database() {
    let mut f = Fixture::new();
    f.import_into(f.chats[0]).unwrap();

    let again = f.import_into(f.chats[1]).unwrap_err().to_string();
    assert!(again.contains("already imported"), "{again}");

    // The same database somewhere else is still the same plan.
    let moved = f.dir.path().join("copy.db");
    std::fs::copy(&f.source, &moved).unwrap();
    let preview = f
        .store
        .preview_plan_import(&moved.to_string_lossy(), f.plan)
        .unwrap();
    assert!(
        preview.refusal.is_some(),
        "the preview says so before the operator approves anything"
    );
    let refused = f
        .store
        .import_plan(&PlanImportRequest {
            path: moved.to_string_lossy().into_owned(),
            plan_id: f.plan,
            chat_id: f.chats[1],
            fingerprint: preview.fingerprint,
        })
        .unwrap_err()
        .to_string();
    assert!(refused.contains("already imported"), "{refused}");
    assert!(f
        .store
        .chat_plan(f.chats[1], PlanActor::Human)
        .unwrap()
        .bundle
        .is_none());
}

#[test]
fn only_an_idle_empty_chat_with_no_plan_can_take_an_import() {
    let mut f = Fixture::new();
    let [first, busy, archived] = f.chats;

    f.store.archive_chat(archived, true).unwrap();
    let refused = f.import_into(archived).unwrap_err().to_string();
    assert!(refused.contains("archived"), "{refused}");

    let turn = f
        .store
        .begin_chat_turn(
            busy,
            "do a thing",
            "request-1",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    let refused = f.import_into(busy).unwrap_err().to_string();
    assert!(refused.contains("turn of its own running"), "{refused}");

    // Settled, but no longer empty: the conversation has already run work.
    f.store
        .finish_chat_turn(busy, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    let refused = f.import_into(busy).unwrap_err().to_string();
    assert!(refused.contains("already run work"), "{refused}");

    // A chat that planned for itself is not empty of planning either.
    let create = serde_json::from_value(serde_json::json!({
        "action": "create_plan", "expect_revision": 0, "title": "Mine"
    }))
    .unwrap();
    f.store
        .change_chat_plan(first, PlanActor::Human, create)
        .unwrap();
    let refused = f.import_into(first).unwrap_err().to_string();
    assert!(refused.contains("already holds a plan"), "{refused}");
    assert_eq!(
        f.store
            .chat_plan(first, PlanActor::Human)
            .unwrap()
            .bundle
            .unwrap()
            .plan
            .title,
        "Mine",
        "the chat's own plan is untouched by the refusal"
    );

    let missing = f.import_into(4242).unwrap_err().to_string();
    assert!(missing.contains("no chat 4242"), "{missing}");
}

#[test]
fn a_plan_left_behind_by_a_deleted_chat_keeps_its_reused_id_out_of_reach() {
    let mut f = Fixture::new();
    let orphan = f.chats[2];
    let create = serde_json::from_value(serde_json::json!({
        "action": "create_plan", "expect_revision": 0, "title": "Left behind"
    }))
    .unwrap();
    f.store
        .change_chat_plan(orphan, PlanActor::Human, create)
        .unwrap();

    // The chat row goes; an `INTEGER PRIMARY KEY` is a reused rowid, so a later chat can
    // be handed this number while the plan under it is still there.
    rusqlite::Connection::open(f.store.path())
        .unwrap()
        .execute("DELETE FROM chat WHERE id = ?1", [orphan])
        .unwrap();

    let library = f.store.plan_library(&PlanLibraryFilter::default()).unwrap();
    let detached = library
        .detached
        .iter()
        .find(|plan| plan.slug == format!("chat-{orphan}"))
        .expect("a plan whose chat is gone is reported, not hidden");
    assert!(detached.why.contains("no longer exists"));
    assert!(
        !library
            .destinations
            .iter()
            .any(|target| target.chat_id == orphan),
        "a chat id an owned plan still names is never offered as a destination"
    );

    // SQLite hands the next chat the deleted one's rowid, and that chat looks brand new.
    let reused = f
        .store
        .create_chat(NewChat {
            project_id: f.project,
            workspace: f.dir.path().into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap()
        .id;
    assert_eq!(reused, orphan, "this is the trap the refusal exists for");
    let message = f.import_into(reused).unwrap_err().to_string();
    assert!(message.contains("already holds a plan"), "{message}");
    assert!(
        message.contains("a chat id can be reused"),
        "the refusal has to explain itself: {message}"
    );
    assert_eq!(
        f.store
            .chat_plan(reused, PlanActor::Human)
            .unwrap()
            .bundle
            .unwrap()
            .plan
            .title,
        "Left behind",
        "the orphan is still exactly where it was, and now visible to its new chat"
    );
}

#[test]
fn the_board_lists_owned_plans_across_projects_and_opens_the_exact_chat() {
    let mut f = Fixture::new();
    let other = f
        .store
        .create_project(NewProject {
            name: "Gadget".into(),
            ..Default::default()
        })
        .unwrap();
    let elsewhere = f
        .store
        .create_chat(NewChat {
            project_id: other.id,
            workspace: f.dir.path().into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap()
        .id;
    let create = serde_json::from_value(serde_json::json!({
        "action": "create_plan", "expect_revision": 0, "title": "Gadget work"
    }))
    .unwrap();
    f.store
        .change_chat_plan(elsewhere, PlanActor::Human, create)
        .unwrap();
    f.import_into(f.chats[0]).unwrap();

    let all = f.store.plan_library(&PlanLibraryFilter::default()).unwrap();
    assert_eq!(all.entries.len(), 2);
    assert_eq!(all.projects.len(), 2);
    let imported = all
        .entries
        .iter()
        .find(|entry| entry.chat_id == f.chats[0])
        .unwrap();
    assert_eq!(imported.title, "Ship the widget");
    assert_eq!(imported.project_id, f.project);
    assert_eq!(imported.project_slug, "widget");
    assert_eq!(imported.slices, 4);
    assert_eq!(imported.done, 1);
    assert_eq!(imported.open_questions, 1);
    let provenance = imported
        .imported
        .as_ref()
        .expect("an imported plan is marked as one");
    assert_eq!(
        provenance.source_path,
        f.source.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(
        provenance.source_plan.as_deref(),
        Some("git:github.com/acme/widget/ship-the-widget")
    );
    assert!(all
        .entries
        .iter()
        .find(|entry| entry.chat_id == elsewhere)
        .unwrap()
        .imported
        .is_none());

    let widget = f
        .store
        .plan_library(&PlanLibraryFilter {
            project: Some("widget".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(widget.entries.len(), 1);
    assert_eq!(widget.entries[0].chat_id, f.chats[0]);
    assert_eq!(
        widget.projects.len(),
        2,
        "a filter hides rows, not the other projects to filter by"
    );

    let drafts = f
        .store
        .plan_library(&PlanLibraryFilter {
            status: vec![PlanStatus::Draft],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(drafts.entries.len(), 1);
    assert_eq!(drafts.entries[0].chat_id, elsewhere);

    // Every plan belongs to the chat it names, and to no other.
    assert!(f
        .store
        .chat_plan(f.chats[1], PlanActor::Human)
        .unwrap()
        .bundle
        .is_none());
    assert_eq!(
        f.store
            .chat_plan(f.chats[0], PlanActor::Human)
            .unwrap()
            .bundle
            .unwrap()
            .plan
            .title,
        "Ship the widget"
    );
}

#[test]
fn the_destinations_offered_are_the_chats_an_import_would_be_accepted_into() {
    let mut f = Fixture::new();
    let library = f.store.plan_library(&PlanLibraryFilter::default()).unwrap();
    let offered: Vec<i64> = library
        .destinations
        .iter()
        .map(|target| target.chat_id)
        .collect();
    assert_eq!(offered.len(), 3);
    for chat in f.chats {
        assert!(offered.contains(&chat));
    }

    f.import_into(f.chats[0]).unwrap();
    f.store.archive_chat(f.chats[1], true).unwrap();
    let after = f.store.plan_library(&PlanLibraryFilter::default()).unwrap();
    assert_eq!(
        after
            .destinations
            .iter()
            .map(|target| target.chat_id)
            .collect::<Vec<_>>(),
        vec![f.chats[2]],
        "a chat that now holds a plan, and an archived one, are no longer offered"
    );
    assert_eq!(after.destinations[0].project_slug, "widget");
}

#[test]
fn a_file_that_is_not_a_readable_planner_database_is_refused_before_anything_else() {
    let f = Fixture::new();
    let nonsense = f.dir.path().join("notes.txt");
    std::fs::write(&nonsense, "not a database").unwrap();
    let error = f
        .store
        .read_plan_source(&nonsense.to_string_lossy())
        .unwrap_err()
        .to_string();
    assert!(error.contains("not a"), "{error}");

    let missing = f
        .store
        .read_plan_source(&f.dir.path().join("gone.db").to_string_lossy())
        .unwrap_err()
        .to_string();
    assert!(missing.contains("cannot read"), "{missing}");

    // ai-team's own databases are not an import source.
    for own in [
        f.store.path().to_path_buf(),
        f.store.planning_path().unwrap(),
    ] {
        let create = serde_json::from_value(serde_json::json!({
            "action": "create_plan", "expect_revision": 0, "title": "Ours"
        }))
        .unwrap();
        let mut store = Store::open(f.store.path()).unwrap();
        let _ = store.change_chat_plan(f.chats[0], PlanActor::Human, create);
        let error = store
            .read_plan_source(&own.to_string_lossy())
            .unwrap_err()
            .to_string();
        assert!(error.contains("ai-team's own database"), "{error}");
    }
}

#[test]
fn a_database_from_a_newer_planner_is_refused_rather_than_partly_read() {
    let f = Fixture::new();
    let conn = rusqlite::Connection::open(&f.source).unwrap();
    conn.execute(
        "INSERT INTO schema_migrations (version, name, applied_at)
         VALUES (6, 'something-new', '2026-05-01T00:00:00Z')",
        [],
    )
    .unwrap();
    drop(conn);

    let error = f.store.read_plan_source(&f.path()).unwrap_err().to_string();
    assert!(error.contains("schema 6"), "{error}");
    assert!(error.contains("Update ai-team"), "{error}");
}

#[test]
fn an_approval_without_the_preview_it_came_from_is_refused() {
    let mut f = Fixture::new();
    let empty = f.store.import_plan(&PlanImportRequest {
        path: f.path(),
        plan_id: f.plan,
        chat_id: f.chats[0],
        fingerprint: String::new(),
    });
    assert!(empty
        .unwrap_err()
        .to_string()
        .contains("carries no fingerprint"));

    let invented = f.store.import_plan(&PlanImportRequest {
        path: f.path(),
        plan_id: f.plan,
        chat_id: f.chats[0],
        fingerprint: "0".repeat(64),
    });
    assert!(invented
        .unwrap_err()
        .to_string()
        .contains("changed in the source"));
    assert!(f
        .store
        .chat_plan(f.chats[0], PlanActor::Human)
        .unwrap()
        .bundle
        .is_none());
}
