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
