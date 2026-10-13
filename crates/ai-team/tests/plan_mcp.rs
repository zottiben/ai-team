//! Real stdio protocol, fake/no model calls, isolated homes. Standalone aip is never used.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use ai_team_core::planning::PlanActor;
use ai_team_core::{ModelRegistry, NewChat, NewProject, NodeStatus, Provider, Reasoning, Store};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next: u64,
}
impl Client {
    async fn connect(db: &Path, home: &Path, chat: i64, node: i64) -> Self {
        let mut child = command(db, home, chat, node)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut client = Self {
            stdin: child.stdin.take().unwrap(),
            lines: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            next: 0,
        };
        let hello = client.request("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}})).await;
        assert_eq!(hello["serverInfo"]["name"], "ai-team-planner");
        client
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        client
    }

    async fn close(self) {
        let Self {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    }

    async fn send(&mut self, value: Value) {
        self.stdin
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params}))
            .await;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("MCP closed unexpectedly");
                let response: Value =
                    serde_json::from_str(&line).expect("stdout must contain only MCP JSON");
                if response["id"] == self.next {
                    assert!(response.get("error").is_none(), "{response}");
                    return response["result"].clone();
                }
            }
        })
        .await
        .expect("MCP response timed out")
    }
    async fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))
            .await
    }
    async fn plan(&mut self) -> Value {
        let result = self.call("get_plan", json!({})).await;
        assert_ne!(result["isError"], true, "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

fn command(db: &Path, home: &Path, chat: i64, node: i64) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ait"));
    command
        .args(["plan", "serve", "--db"])
        .arg(db)
        .args(["--chat", &chat.to_string(), "--node", &node.to_string()])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("AI_TEAM_HOME", home.join("not-the-bound-database"))
        .env("AI_PLANNER_DB", home.join("standalone.db"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                home.join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .kill_on_drop(true);
    command
}

const SENTINEL: &[u8] = b"standalone planner belongs to the operator";

fn fixture(
    home: &Path,
) -> (
    Store,
    ai_team_core::Chat,
    ai_team_core::Chat,
    ai_team_core::ChatSubmission,
) {
    std::fs::create_dir_all(home.join("bin")).unwrap();
    std::fs::create_dir_all(home.join("config/ai-planner")).unwrap();
    for path in [
        "standalone.db",
        "config/ai-planner/config.toml",
        ".mcp.json",
    ] {
        std::fs::write(home.join(path), SENTINEL).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let aip = home.join("bin/aip");
        std::fs::write(
            &aip,
            "#!/bin/sh\nprintf called > \"$HOME/aip-called\"\nexit 99\n",
        )
        .unwrap();
        std::fs::set_permissions(aip, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let db = home.join("team.db");
    let mut store = Store::init(&db).unwrap();
    let project = store
        .create_project(NewProject {
            name: "MCP".into(),
            ..Default::default()
        })
        .unwrap();
    let mut new_chat = || {
        store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: home.into(),
                provider: Provider::Local,
                model: "fixture".into(),
                reasoning: Reasoning::High,
            })
            .unwrap()
    };
    let chat = new_chat();
    let other = new_chat();
    let turn = store
        .begin_chat_turn(
            chat.id,
            "Plan a change",
            "mcp",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    (store, chat, other, turn)
}

#[tokio::test]
async fn chat_images_are_native_mcp_content_with_exact_chat_and_live_actor_scope() {
    use ai_team_core::chat_images::{self, ImageInput, Prompt};
    let dir = tempfile::tempdir().unwrap();
    let (mut store, chat, other, first) = fixture(dir.path());
    let registry = ModelRegistry::local_only();
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYGAAAAAEAAH2FzhVAAAAAElFTkSuQmCC";
    let images = || {
        chat_images::validate(vec![ImageInput {
            name: "screen.png".into(),
            media_type: "image/png".into(),
            data: data.into(),
        }])
        .unwrap()
    };
    let photo = store
        .begin_chat_prompt(
            chat.id,
            "Inspect screenshot",
            "photo",
            &registry,
            Prompt {
                expected_reasoning: None,
                workspace_epoch: 0,
                images: images(),
                publication: None,
            },
        )
        .unwrap();
    store
        .finish_chat_turn(chat.id, photo.node_id, NodeStatus::Done, None)
        .unwrap();
    let foreign = store
        .begin_chat_prompt(
            other.id,
            "Private screenshot",
            "other-photo",
            &registry,
            Prompt {
                expected_reasoning: None,
                workspace_epoch: 0,
                images: images(),
                publication: None,
            },
        )
        .unwrap();
    store
        .finish_chat_turn(other.id, foreign.node_id, NodeStatus::Done, None)
        .unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "Use the earlier screenshot", "later", &registry)
        .unwrap();
    let mut client = Client::connect(store.path(), dir.path(), chat.id, turn.node_id).await;
    let listed = client.call("list_chat_images", json!({})).await;
    assert_eq!(listed["isError"], false, "{listed}");
    let entries: Value =
        serde_json::from_str(listed["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 1);
    assert_eq!(entries[0]["node"], photo.node_id);
    assert!(!listed.to_string().contains(data));
    let image = client
        .call("read_chat_image", json!({"node":photo.node_id,"index":0}))
        .await;
    assert_eq!(image["isError"], false, "{image}");
    assert_eq!(image["content"][0]["type"], "image");
    assert_eq!(image["content"][0]["data"], data);
    assert_eq!(image["content"][0]["mimeType"], "image/png");
    assert_eq!(
        client
            .call("read_chat_image", json!({"node":foreign.node_id,"index":0}))
            .await["isError"],
        true
    );
    assert_eq!(
        client
            .call("list_chat_images", json!({"chat":other.id}))
            .await["isError"],
        true
    );
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    assert_eq!(
        client
            .call("read_chat_image", json!({"node":photo.node_id,"index":0}))
            .await["isError"],
        true
    );
    client.close().await;
}

#[tokio::test]
async fn embedded_mcp_round_trip_is_scoped_revocable_and_independent_of_aip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (mut store, chat, other, turn) = fixture(home);
    let db = store.path().to_path_buf();
    let mut child = command(&db, home, chat.id, turn.node_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut client = Client {
        stdin: child.stdin.take().unwrap(),
        lines: BufReader::new(child.stdout.take().unwrap()).lines(),
        child,
        next: 0,
    };
    let hello = client.request("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}})).await;
    assert_eq!(hello["serverInfo"]["name"], "ai-team-planner");
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await;
    let tools = client.request("tools/list", json!({})).await;
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"create_plan"));
    assert!(!names.contains(&"answer_question"));
    assert!(!names.contains(&"delete_plan"));
    assert!(client.plan().await["bundle"].is_null());
    assert_eq!(
        client
            .call(
                "create_plan",
                json!({"expect_revision":0,"title":"Scoped work"})
            )
            .await["isError"],
        false
    );
    let before = client.plan().await;
    assert_eq!(
        client
            .call(
                "open_question",
                json!({"expect_revision":before["revision"],"body":"Which option?"})
            )
            .await["isError"],
        false
    );
    let asked = client.plan().await;
    let question = asked["bundle"]["questions"][0]["id"].as_i64().unwrap();
    assert_eq!(client.call("answer_question", json!({"expect_revision":asked["revision"],"question_id":question,"answer":"Self-approval"})).await["isError"], true);
    assert_eq!(
        client.call("get_plan", json!({"chat":other.id})).await["isError"],
        true
    );
    assert_eq!(client.call("add_decision", json!({"expect_revision":asked["revision"],"title":"Escape","body":"No","plan_id":999})).await["isError"], true);
    assert!(store
        .chat_plan(other.id, PlanActor::Human)
        .unwrap()
        .bundle
        .is_none());
    let answer = serde_json::from_value(json!({"action":"answer_question","expect_revision":asked["revision"],"question_id":question,"answer":"Option A"})).unwrap();
    store
        .change_chat_plan(chat.id, PlanActor::Human, answer)
        .unwrap();
    assert_eq!(
        client.plan().await["bundle"]["questions"][0]["answer"],
        "Option A"
    );
    let wrong = tokio::time::timeout(
        Duration::from_secs(10),
        command(&db, home, other.id, turn.node_id).output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!wrong.status.success());
    assert!(wrong.stdout.is_empty());
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    assert_eq!(client.call("get_plan", json!({})).await["isError"], true);
    drop(client.stdin);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), client.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_standalone_untouched(home);
}

#[tokio::test]
async fn team_members_use_distinct_revocable_stdio_scopes_after_the_coordinator_settles() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (mut store, chat, other, solo) = fixture(home);
    store
        .finish_chat_turn(chat.id, solo.node_id, NodeStatus::Done, None)
        .unwrap();
    let team = store
        .seed_default_team(
            chat.project_id,
            &ai_team_core::RoleModelDefault::local_floor(),
        )
        .unwrap();
    let current = store.chat(chat.id).unwrap();
    store
        .set_chat_mode(chat.id, ai_team_core::ChatMode::Team, current.rev)
        .unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "Team plan", "team", &ModelRegistry::local_only())
        .unwrap();
    let db = store.path().to_path_buf();
    let mut coordinator = Client::connect(&db, home, chat.id, turn.node_id).await;
    let tools = coordinator.request("tools/list", json!({})).await;
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"get_plan"));
    assert!(names.contains(&"open_question"));
    assert!(!names.contains(&"create_plan"));
    assert_eq!(
        coordinator
            .call(
                "create_plan",
                json!({"expect_revision":0,"title":"Forbidden"})
            )
            .await["isError"],
        true
    );
    let planner = store
        .agents(team.id)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "planner")
        .unwrap();
    let planning = store
        .dispatch(turn.run_id, planner.id, None, &ModelRegistry::local_only())
        .unwrap();
    store
        .attach_worktree(planning.id, &chat.workspace_path, None, None)
        .unwrap();
    store
        .set_node_status(planning.id, NodeStatus::Running)
        .unwrap();
    let mut writer = Client::connect(&db, home, chat.id, planning.id).await;
    store
        .set_node_status(turn.node_id, NodeStatus::Done)
        .unwrap();
    assert_eq!(
        coordinator.call("get_plan", json!({})).await["isError"],
        true
    );
    assert_eq!(
        writer
            .call(
                "create_plan",
                json!({"expect_revision":0,"title":"Planner-owned work"})
            )
            .await["isError"],
        false
    );
    let plan = writer.plan().await;
    assert_eq!(plan["chat_id"], chat.id);
    assert_eq!(plan["bundle"]["plan"]["title"], "Planner-owned work");
    assert_eq!(
        writer.call("get_plan", json!({"chat":other.id})).await["isError"],
        true
    );
    assert!(store
        .chat_plan(other.id, PlanActor::Human)
        .unwrap()
        .bundle
        .is_none());
    store.request_chat_stop(chat.id, turn.node_id).unwrap();
    assert_eq!(writer.call("get_plan", json!({})).await["isError"], true);
    coordinator.close().await;
    writer.close().await;
    assert_standalone_untouched(home);
}

fn assert_standalone_untouched(home: &Path) {
    assert!(!home.join("aip-called").exists());
    assert!(!home.join("not-the-bound-database").exists());
    for path in [
        "standalone.db",
        "config/ai-planner/config.toml",
        ".mcp.json",
    ] {
        assert_eq!(std::fs::read(home.join(path)).unwrap(), SENTINEL);
    }
}

/// A checkout on a working branch with a bare origin, and a chat that owns it.
fn publication_fixture(
    home: &Path,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    Store,
    ai_team_core::Chat,
) {
    std::fs::create_dir_all(home.join("bin")).unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let repo = home.join("repo");
    let remote = home.join("remote.git");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q", "-b", "work"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@invalid"],
        vec!["config", "core.hooksPath", "/dev/null"],
    ] {
        run_git(&repo, &args);
    }
    std::fs::write(repo.join("README.md"), "base\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-qm", "base"]);
    run_git(
        home,
        &[
            "init",
            "--bare",
            "-q",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );
    run_git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_git(&repo, &["push", "-q", "origin", "work:main"]);
    let db = home.join("team.db");
    let mut store = Store::init(&db).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Publication".into(),
            ..Default::default()
        })
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: repo.clone(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    (db, repo, store, chat)
}

fn head_of(repo: &Path) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned()
}

/// The scoped publication tool exists only while the person's own explicit instruction
/// does, and it pins the exact commit rather than publishing anything itself.
#[tokio::test]
async fn request_publication_is_offered_only_to_an_authorised_turn() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let (db, repo, mut store, chat) = publication_fixture(&home);

    // An ordinary turn is never offered the tool, and cannot reach it by naming it.
    let ordinary = store
        .begin_chat_turn(chat.id, "add a test", "plain", &ModelRegistry::local_only())
        .unwrap();
    let mut client = Client::connect(&db, &home, chat.id, ordinary.node_id).await;
    assert!(!tool_names(&mut client)
        .await
        .contains(&"request_publication".to_string()));
    let head = head_of(&repo);
    assert_eq!(
        client
            .call("request_publication", json!({"commit": head}))
            .await["isError"],
        true
    );
    client.close().await;
    store
        .finish_chat_turn(chat.id, ordinary.node_id, NodeStatus::Done, None)
        .unwrap();

    // The person asks, in their own words, and the same seat gains exactly one tool.
    let message = "Create a new branch from main. Then commit and push and I'll open the PR.";
    let authority = ai_team_core::chat_push::prepare(&mut store, chat.id, message)
        .await
        .unwrap();
    let ai_team_core::chat_push::Authority::Solo(target) = &authority else {
        panic!("expected solo authority, got {authority:?}");
    };
    let turn = store
        .begin_chat_turn(chat.id, message, "asked", &ModelRegistry::local_only())
        .unwrap();
    ai_team_core::chat_push::mint(
        &mut store,
        chat.id,
        "asked",
        message,
        Some(turn.node_id),
        target,
        None,
    )
    .unwrap();
    let mut client = Client::connect(&db, &home, chat.id, turn.node_id).await;
    assert!(tool_names(&mut client)
        .await
        .contains(&"request_publication".to_string()));
    run_git(
        &repo,
        &["checkout", "-qb", "chore/scoped-publication", "origin/main"],
    );
    std::fs::write(repo.join("feature.txt"), "done\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-qm", "the asked-for work"]);
    let made = head_of(&repo);
    // A commit that is not this branch's tip is refused, so a seat cannot pin an earlier
    // state or somebody else's work.
    assert_eq!(
        client
            .call("request_publication", json!({"commit": head}))
            .await["isError"],
        true
    );
    let pinned = client
        .call("request_publication", json!({"commit": made}))
        .await;
    assert_ne!(pinned["isError"], true, "{pinned}");
    assert!(pinned["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("after this turn ends"));
    client.close().await;

    // Pinning published nothing: the host does that, after the turn.
    assert!(!home.join("remote.git/refs/heads/work").exists());
    assert!(!home
        .join("remote.git/refs/heads/chore/scoped-publication")
        .exists());
    assert_eq!(
        store
            .chat_push_grant_for_request(chat.id, "asked")
            .unwrap()
            .unwrap()
            .pinned_branch
            .as_deref(),
        Some("chore/scoped-publication")
    );
    assert_eq!(
        store
            .chat_push_grant_for_request(chat.id, "asked")
            .unwrap()
            .unwrap()
            .commit_sha
            .as_deref(),
        Some(made.as_str())
    );
}

/// Pi drops a tool result whose output exceeds its guard, so a plan that outgrew one
/// response vanished entirely: the seat was handed nothing, could not read the revision
/// and stopped. A board too large for one answer is paged, never dropped and never cut.
#[tokio::test]
async fn a_plan_too_large_for_one_response_is_paged_exactly_rather_than_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (mut store, chat, other, turn) = fixture(home);
    let db = store.path().to_path_buf();
    let mut client = Client::connect(&db, home, chat.id, turn.node_id).await;
    assert!(tool_names(&mut client)
        .await
        .contains(&"read_plan".to_string()));

    // A plan small enough to answer whole still arrives whole, and says so.
    let mut revision = apply(
        &mut store,
        chat.id,
        0,
        json!({"action":"create_plan","title":"Large board","summary":"Paging regression"}),
    );
    let small = client.plan().await;
    assert_eq!(small["partial"], false);
    assert_eq!(small["bundle"]["plan"]["title"], "Large board");

    // Unicode and one very long line, because both are what the output guard measures.
    let unicode = "é漢字🛠 ".repeat(1_000);
    for action in [
        json!({"action":"write_section","key":"context","title":"Context","body":format!("{unicode}\n{unicode}")}),
        json!({"action":"add_slice","key":"S1","title":"Build","scope":&unicode,"touches":["src/**"],"demo":&unicode}),
        json!({"action":"append_log","body":&unicode}),
    ] {
        revision = apply(&mut store, chat.id, revision, action);
    }
    let whole = serde_json::to_value(store.chat_plan(chat.id, PlanActor::Human).unwrap()).unwrap();
    assert!(
        serde_json::to_string(&whole).unwrap().len() > 60_000,
        "the fixture must outgrow one response"
    );

    // get_plan keeps the revision and the plan's identity, and never calls it absent.
    let described = client.call("get_plan", json!({})).await;
    assert_ne!(described["isError"], true, "{described}");
    let text = described["content"][0]["text"].as_str().unwrap();
    assert!(
        text.len() <= BUDGET,
        "get_plan returned {} bytes",
        text.len()
    );
    let descriptor: Value = serde_json::from_str(text).unwrap();
    assert_eq!(descriptor["partial"], true);
    assert_eq!(descriptor["plan_present"], true);
    assert_eq!(descriptor["revision"], revision);
    assert_eq!(descriptor["chat_id"], chat.id);
    assert_eq!(descriptor["project_id"], chat.project_id);
    assert_eq!(descriptor["plan"]["title"], "Large board");
    assert!(descriptor.get("bundle").is_none(), "{descriptor}");
    let total = descriptor["total_bytes"].as_u64().unwrap();

    // Every page once, in order, reconstructing exactly what the store holds.
    let (document, pages) = read_every_page(&mut client, revision, total).await;
    assert!(pages > 1, "a plan this size must take more than one page");
    assert_eq!(serde_json::from_str::<Value>(&document).unwrap(), whole);

    // A page is cut from that same fixed document, and a limit bounds it.
    let page = read_page(
        &mut client,
        json!({"expect_revision":revision,"offset":0,"limit":2_000}),
    )
    .await;
    let head = page["content"].as_str().unwrap();
    assert!(!head.is_empty() && head.len() <= 2_000);
    assert_eq!(head, &document[..head.len()]);

    // Offsets this tool never handed back, and limits outside its range.
    let inside = document
        .char_indices()
        .find(|(_, c)| c.len_utf8() > 1)
        .map(|(at, _)| at + 1)
        .unwrap();
    assert!(!document.is_char_boundary(inside));
    for refused in [
        json!({"expect_revision":revision,"offset":inside}),
        json!({"expect_revision":revision,"offset":total + 1}),
        json!({"expect_revision":revision,"offset":-1}),
        json!({"expect_revision":revision,"offset":0,"limit":0}),
        json!({"expect_revision":revision,"offset":0,"limit":1_000_000}),
        json!({"expect_revision":revision,"offset":0,"chat":other.id}),
        json!({"expect_revision":revision,"offset":0,"plan_id":999}),
        json!({"offset":0}),
    ] {
        assert_eq!(
            client.call("read_plan", refused.clone()).await["isError"],
            true,
            "{refused}"
        );
    }

    // The revision the descriptor carried is still a working write token, and the
    // mutation's own answer is bounded the same way.
    let appended = client
        .call(
            "append_log",
            json!({"expect_revision":revision,"body":"paged the board"}),
        )
        .await;
    assert_ne!(appended["isError"], true, "{appended}");
    let written = appended["content"][0]["text"].as_str().unwrap();
    assert!(
        written.len() <= BUDGET,
        "the mutation returned {} bytes",
        written.len()
    );
    let written: Value = serde_json::from_str(written).unwrap();
    assert_eq!(written["partial"], true);
    let fresh = written["revision"].as_i64().unwrap();
    assert!(fresh > revision);

    // A stale page would stitch two different documents together.
    assert_eq!(
        client
            .call("read_plan", json!({"expect_revision":revision,"offset":0}))
            .await["isError"],
        true
    );
    assert!(store
        .chat_plan(other.id, PlanActor::Human)
        .unwrap()
        .bundle
        .is_none());
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    assert_eq!(
        client
            .call("read_plan", json!({"expect_revision":fresh,"offset":0}))
            .await["isError"],
        true
    );
    client.close().await;
    assert_standalone_untouched(home);
}

/// Comfortably inside Pi's 50KB output guard, which is what dropped the whole snapshot.
const BUDGET: usize = 24_000;

fn apply(store: &mut Store, chat: i64, revision: i64, mut action: Value) -> i64 {
    action["expect_revision"] = revision.into();
    store
        .change_chat_plan(
            chat,
            PlanActor::Human,
            serde_json::from_value(action).unwrap(),
        )
        .unwrap()
        .revision
}

async fn read_page(client: &mut Client, arguments: Value) -> Value {
    let result = client.call("read_plan", arguments).await;
    assert_ne!(result["isError"], true, "{result}");
    let body = result["content"][0]["text"].as_str().unwrap();
    assert!(body.len() <= BUDGET, "a page of {} bytes", body.len());
    serde_json::from_str(body).unwrap()
}

/// Follow next_offset to the end, refusing to visit an offset twice.
async fn read_every_page(client: &mut Client, revision: i64, total: u64) -> (String, usize) {
    let mut document = String::new();
    let mut seen: Vec<u64> = Vec::new();
    let mut at = Some(0);
    while let Some(offset) = at {
        assert!(!seen.contains(&offset), "offset {offset} was paged twice");
        assert!(seen.len() < 64, "paging never finished");
        seen.push(offset);
        let page = read_page(client, json!({"expect_revision":revision,"offset":offset})).await;
        assert_eq!(page["offset"], offset);
        assert_eq!(page["revision"], revision);
        assert_eq!(page["total_bytes"], total);
        let content = page["content"].as_str().unwrap();
        assert!(!content.is_empty(), "an empty page cannot make progress");
        document.push_str(content);
        at = page["next_offset"].as_u64();
        match at {
            Some(next) => assert_eq!(next, offset + content.len() as u64),
            None => assert!(page["next_offset"].is_null(), "{page}"),
        }
    }
    assert_eq!(document.len() as u64, total);
    (document, seen.len())
}

async fn tool_names(client: &mut Client) -> Vec<String> {
    client.request("tools/list", json!({})).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn run_git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
