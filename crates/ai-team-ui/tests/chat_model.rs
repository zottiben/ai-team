//! One isolated process owns the environment; only an offline fake catalogue may run.
#![cfg(unix)]
use ai_team_core::{
    CredentialStore, ModelRegistry, NewChat, NewProject, Provider, Reasoning, Store,
};
use ai_team_ui::{ServeOptions, Server};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::PermissionsExt,
    time::Duration,
};

struct Fixture {
    _root: tempfile::TempDir,
    _runtime: tokio::runtime::Runtime,
    store: Store,
    chat: i64,
    addr: SocketAddr,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let bin = path.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::env::set_var("HOME", &path);
        std::env::set_var("AI_TEAM_HOME", path.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", path.join("config"));
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        std::fs::create_dir_all(path.join("config/ai-team")).unwrap();
        std::fs::write(
            path.join("config/ai-team/machine.toml"),
            ai_team_core::DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
        )
        .unwrap();
        std::fs::write(bin.join("pi"), "#!/bin/sh\n[ \"$1\" = --offline ] && [ \"$2\" = --list-models ] && [ \"$3\" = --mcp-config ] && [ \"$PI_MCP_CONFIG_MODE\" = exclusive ] || exit 97\nprintf 'provider model context max-out thinking images\\nllama.cpp fixture 32k 8k yes yes\\nllama.cpp alternate 32k 8k no no\\nopenai-codex denied 32k 8k yes yes\\n'\n").unwrap();
        std::fs::set_permissions(bin.join("pi"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = path.join("state/team.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let mut store = Store::init(&db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Fixture".into(),
                ..Default::default()
            })
            .unwrap();
        let chat = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: path,
                provider: Provider::Local,
                model: "fixture".into(),
                reasoning: Reasoning::High,
            })
            .unwrap()
            .id;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::bind(ServeOptions {
                store: Some(Store::open(&db).unwrap()),
                credentials: CredentialStore::isolated(),
                ..Default::default()
            }))
            .unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            server.serve().await.unwrap();
        });
        Self {
            _root: root,
            _runtime: runtime,
            store,
            chat,
            addr,
            token,
        }
    }
    fn post(&self, body: Value, authenticated: bool) -> (u16, Value) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let body = body.to_string();
        let token = if authenticated {
            &self.token
        } else {
            "invalid"
        };
        write!(stream, "POST /api/chats/{}/model HTTP/1.1\r\nHost: {}\r\nx-ai-team-token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", self.chat, self.addr, body.len()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap_or(Value::String(body.into())),
        )
    }
}

#[test]
fn authenticated_model_selection_is_exact_future_only_and_never_runs_a_turn() {
    let mut f = Fixture::new();
    let turn = f
        .store
        .begin_chat_turn(f.chat, "first", "first", &ModelRegistry::local_only())
        .unwrap();
    let rev = f.store.chat(f.chat).unwrap().rev;
    let before = serde_json::to_value(f.store.node_run(turn.node_id).unwrap()).unwrap();
    let request = json!({"provider":"local","model":"alternate","expect_revision":rev});
    assert_eq!(f.post(request.clone(), false).0, 401);
    for bad in [
        json!({"provider":"openai","model":"denied","expect_revision":rev}),
        json!({"provider":"local","model":"missing","expect_revision":rev}),
    ] {
        assert_eq!(f.post(bad, true).0, 400);
    }
    let (status, response) = f.post(request.clone(), true);
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["model"], "alternate");
    assert_eq!(response["reasoning"], "none");
    assert_eq!(response["active_node_id"], turn.node_id);
    assert_eq!(response["stop_requested"], false);
    assert_eq!(f.post(request, true).0, 400);
    assert_eq!(f.post(json!({"provider":"local","model":"fixture","expect_revision":rev+1,"command":"not allowed"}), true).0, 422);
    assert_eq!(f.store.chat_turns(f.chat).unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(f.store.node_run(turn.node_id).unwrap()).unwrap(),
        before
    );
}
