use super::fixture::{git, Fixture};
use ai_team_core::{
    drive_chat, drive_chat_team_planning,
    planning::{PlanAction, PlanActor, PlanStatus},
    Chat, ChatBuildApproval, ChatMode, ChatSubmission, ChatTeamPhase, ModelRegistry, NewChat,
    NodeStatus, PiProcess, PiTurn, Provider, Reasoning, RunStatus, Store,
};
use anyhow::Context;
use serde_json::Value;

pub(super) async fn run(f: &Fixture) -> anyhow::Result<()> {
    discovery_control(f).await?;
    super::auth_check::run(f).await?;
    let (mut store, chat) = f.store()?;
    let other = independent_plan(&mut store, &chat, f)?;
    let other_before = serde_json::to_value(store.chat_plan(other, PlanActor::Human)?)?;
    super::context_failure::run(f, &mut store, &chat).await?;
    let registry = ModelRegistry::local_only();
    let base = git(&f.repo, &["rev-parse", "HEAD"]);
    let first = solo(&mut store, chat.id, "first", "SOLO_FIRST", &registry).await?;
    let first_session = store.node_run(first.node_id)?.session_id.unwrap();
    let plan = store.chat_plan(chat.id, PlanActor::Human)?;
    let plan_id = plan.bundle.as_ref().unwrap().plan.id;
    let question = plan.bundle.unwrap().questions.remove(0);
    store.change_chat_plan(
        chat.id,
        PlanActor::Human,
        PlanAction::AnswerQuestion {
            expect_revision: plan.revision,
            question_id: question.id,
            answer: "Yes; draft only.".into(),
        },
    )?;
    assert!(
        !f.root.join("pool-calls").exists(),
        "answering is not approval"
    );
    let follow = solo(&mut store, chat.id, "follow", "SOLO_FOLLOW", &registry).await?;
    assert_eq!(
        store.node_run(follow.node_id)?.session_id.as_ref(),
        Some(&first_session)
    );
    store.set_chat_mode(chat.id, ChatMode::Team, store.chat(chat.id)?.rev)?;
    let team = store.begin_chat_turn(chat.id, "TEAM_REQUEST", "team", &registry)?;
    drive_chat_team_planning(&f.db, chat.id, team.node_id).await?;
    assert_eq!(
        store.chat_team_run(team.run_id)?.unwrap().phase,
        ChatTeamPhase::AwaitingApproval
    );
    assert!(
        !f.root.join("pool-calls").exists(),
        "planning is not approval"
    );
    let review = store.chat_build_review(chat.id, team.node_id).await?;
    assert_eq!(review.plan.bundle.as_ref().unwrap().plan.id, plan_id);
    let start = store
        .approve_chat_build(
            chat.id,
            team.node_id,
            &ChatBuildApproval {
                expect_control_revision: review.execution.rev,
                expect_plan_revision: review.plan.revision,
                expect_roster_revision: review.roster_revision,
                expect_head: review.head,
            },
        )
        .await?;
    super::interruption::build(f, &mut store, start).await?;
    prove_build(f, &store, &team, &base)?;
    store.set_chat_mode(chat.id, ChatMode::Single, store.chat(chat.id)?.rev)?;
    let after = solo(&mut store, chat.id, "after", "SOLO_AFTER_TEAM", &registry).await?;
    assert_eq!(
        store.node_run(after.node_id)?.session_id.as_ref(),
        Some(&first_session)
    );
    assert_eq!(
        store
            .chat_plan(chat.id, PlanActor::Human)?
            .bundle
            .unwrap()
            .plan
            .id,
        plan_id
    );
    assert_eq!(
        serde_json::to_value(store.chat_plan(other, PlanActor::Human)?)?,
        other_before
    );
    prove_transcript(f)?;
    Ok(())
}

fn prove_build(
    f: &Fixture,
    store: &Store,
    team: &ChatSubmission,
    base: &str,
) -> anyhow::Result<()> {
    let execution = store.run(team.run_id)?;
    assert_eq!(execution.status, RunStatus::Done, "{execution:?}");
    let slice = store.chat_build_slices(team.run_id)?.remove(0);
    assert_eq!(slice.build_status, "verified");
    assert_eq!(slice.lease_state, "released");
    let sha = slice.commit_sha.as_ref().unwrap();
    assert_eq!(
        git(&f.repo, &["show", &format!("{sha}:crates/answer.txt")]),
        "GOOD"
    );
    assert_eq!(git(&f.repo, &["rev-parse", &format!("{sha}^")]), base);
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), base);
    assert!(git(&f.repo, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        std::fs::read_to_string(f.root.join("outside.txt"))?,
        "KEEP\n"
    );
    let chat = store.chat_team_run(team.run_id)?.unwrap().chat_id;
    let plan = store.chat_plan(chat, PlanActor::Human)?;
    assert_eq!(
        plan.bundle.as_ref().unwrap().slices[0].status,
        PlanStatus::InReview
    );
    assert!(plan.bundle.as_ref().unwrap().slices[0].claimed_by.is_none());
    let maker = store.node_run(slice.maker_node_id.unwrap())?;
    assert!(store
        .planning_access(chat, PlanActor::Agent(maker.id))
        .is_err());
    assert!(store.chat_team_run(team.run_id)?.unwrap().quiescent);
    Ok(())
}

fn independent_plan(store: &mut Store, chat: &Chat, f: &Fixture) -> anyhow::Result<i64> {
    let other = store.create_chat(NewChat {
        project_id: chat.project_id,
        workspace: f.repo.clone(),
        provider: Provider::Local,
        model: "fixture-solo".into(),
        reasoning: Reasoning::High,
    })?;
    store.change_chat_plan(
        other.id,
        PlanActor::Human,
        PlanAction::CreatePlan {
            expect_revision: 0,
            title: "OTHER_PLAN_SECRET".into(),
            summary: None,
        },
    )?;
    Ok(other.id)
}

async fn solo(
    store: &mut Store,
    chat: i64,
    id: &str,
    prompt: &str,
    registry: &ModelRegistry,
) -> anyhow::Result<ChatSubmission> {
    let turn = store.begin_chat_turn(chat, prompt, id, registry)?;
    drive_chat(store.path(), chat, turn.node_id, false).await?;
    let node = store.node_run(turn.node_id)?;
    assert_eq!(node.status, NodeStatus::Done, "{node:?}");
    Ok(turn)
}

async fn discovery_control(f: &Fixture) -> anyhow::Result<()> {
    // Without exclusive mode, the exact same adapter must see our global sentinel.
    let mut turn = PiTurn::new(&f.repo, "Discovery control, no tools.");
    turn.provider = Some("llama.cpp".into());
    turn.model = Some("fixture-probe".into());
    let mut process = PiProcess::start(&turn)?;
    let outcome =
        tokio::time::timeout(std::time::Duration::from_secs(30), process.drive(|_| {})).await??;
    anyhow::ensure!(
        outcome.settled && !outcome.failed,
        "discovery control failed: {outcome:?}"
    );
    let attempts = std::fs::read_to_string(f.root.join("unexpected-tool")).context(
        "unscoped positive control never launched the sentinel; check adapter config discovery",
    )?;
    assert!(!attempts.is_empty() && attempts.lines().all(|line| line == "aip"));
    std::fs::rename(
        f.root.join("unexpected-tool"),
        f.root.join("discovery-control-proof"),
    )?;
    Ok(())
}

fn prove_transcript(f: &Fixture) -> anyhow::Result<()> {
    for path in ["network-denied", "unexpected-tool"] {
        assert!(
            !f.root.join(path).exists(),
            "unexpected external activity: {path}"
        );
    }
    let mut calls = Vec::<Value>::new();
    for entry in std::fs::read_dir(&f.root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("model-") {
            for line in std::fs::read_to_string(entry.path())?.lines() {
                calls.push(serde_json::from_str(line)?);
            }
        }
    }
    calls.sort_by_key(|call| call["time"].as_u64().unwrap());
    assert!(!calls.iter().any(|call| call.get("failure").is_some()));
    let starts: Vec<_> = calls
        .iter()
        .filter(|c| c.get("tools").is_some() && c["role"] != "probe" && c["contextFailure"] != true)
        .collect();
    assert_eq!(
        starts.len(),
        8,
        "three solo + coordinator/planner/two maker attempts/reader"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.get("tools").is_some() && c["contextFailure"] == true)
            .count(),
        1
    );
    assert!(starts
        .iter()
        .all(|s| !s.to_string().contains("OTHER_PLAN_SECRET")));
    assert_eq!(
        std::fs::read_to_string(f.root.join("standalone.sqlite"))?,
        "STANDALONE_KEEP"
    );
    let last = starts.last().unwrap().to_string();
    for marker in [
        "SOLO_FIRST",
        "SOLO_FOLLOW",
        "COORDINATOR_CONTEXT_SENTINEL",
        "PLANNER_CONTEXT_SENTINEL",
        "MAKER_CONTEXT_SENTINEL",
        "NOT merged",
    ] {
        assert!(
            last.contains(marker),
            "missing cross-mode context: {marker}"
        );
    }
    let pool: Vec<Value> = std::fs::read_to_string(f.root.join("pool-calls"))?
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(pool.iter().filter(|c| c[0] == "get").count(), 1);
    assert_eq!(pool.iter().filter(|c| c[0] == "return").count(), 1);
    assert_eq!(
        calls
            .iter()
            .filter(|c| c["role"] != "probe"
                && c["contextFailure"] != true
                && c["result"]["isError"] == true)
            .count(),
        12,
        "three scope overrides + two makers x (escape/publish/self-certification) + reader write"
    );
    Ok(())
}
