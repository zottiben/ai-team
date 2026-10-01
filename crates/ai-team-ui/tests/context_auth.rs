//! The real OAuth HTTP -> PTY path, with a fake Pi and isolated credentials/home.
#![cfg(unix)]
use ai_team_ui::{ServeOptions, Server};
use std::{os::unix::fs::PermissionsExt, time::Duration};

async fn post(address: std::net::SocketAddr, token: Option<&str>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let body = r#"{"source":"clickup"}"#;
    let auth = token
        .map(|token| format!("{}: {token}\r\n", ai_team_ui::TOKEN_HEADER))
        .unwrap_or_default();
    socket.write_all(format!("POST /api/settings/context-auth HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    response
}

struct AbortServer(tokio::task::AbortHandle);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn context_sign_in_scopes_only_its_requested_server_without_inherited_metered_auth() {
    let dir = tempfile::Builder::new()
        .prefix("auth scope ' ;")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = std::env::var_os("PATH").unwrap();
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    for key in ["HOME", "AI_TEAM_HOME", "XDG_CONFIG_HOME", "AUTH_FIXTURE"] {
        std::env::set_var(key, &root);
    }
    std::env::set_var(
        "PATH",
        format!("{}:{}", root.display(), path.to_string_lossy()),
    );
    std::env::set_var("ANTHROPIC_API_KEY", "DECOY");
    std::env::set_var("OPENAI_API_KEY", "DECOY");
    let pi = root.join("pi");
    std::fs::write(&pi, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AUTH_FIXTURE/args\"\nprintf '%s\\n' \"$PI_MCP_CONFIG_MODE\" \"$ANTHROPIC_API_KEY$OPENAI_API_KEY\" > \"$AUTH_FIXTURE/env\"\nIFS= read -r line\nprintf '%s' \"$line\" > \"$AUTH_FIXTURE/input\"\n").unwrap();
    std::fs::set_permissions(pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = Server::bind(ServeOptions {
        db_path: Some(root.join("absent.sqlite")),
        credentials: ai_team_core::CredentialStore::isolated(),
        ..Default::default()
    })
    .await
    .unwrap();
    let address = server.addr();
    let token = server.token().to_owned();
    let task = tokio::spawn(server.serve());
    let _abort = AbortServer(task.abort_handle());
    assert!(post(address, None).await.starts_with("HTTP/1.1 401"));
    assert!(!root.join("args").exists());
    let response = post(address, Some(&token)).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.join("input").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("env")).unwrap(),
        "exclusive\n\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("input")).unwrap(),
        "/mcp-auth clickup"
    );
    let args = std::fs::read_to_string(root.join("args")).unwrap();
    let args: Vec<_> = args.lines().collect();
    let index = args.iter().position(|arg| *arg == "--mcp-config").unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(args[index + 1]).unwrap()).unwrap();
    assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
    let source = &config["mcpServers"]["clickup"];
    assert_eq!(source["lifecycle"], "lazy-keep-alive");
    assert_eq!(source["auth"], "oauth");
    assert!(!source["includeTools"].as_array().unwrap().is_empty());
    assert!(!root.join("absent.sqlite").exists());
}
