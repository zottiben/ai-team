//! The server, driven over a real socket.
//!
//! These go through `Server::bind` and a TCP connection rather than calling handlers
//! directly, because the parts that break are the parts a unit test skips: the token
//! middleware, the status codes, the SPA fallback, and whether the JSON is the shape
//! the client was written against.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use ai_team_ui::{ServeOptions, Server, TOKEN_HEADER, TOKEN_QUERY};

struct Harness {
    addr: SocketAddr,
    token: String,
    _runtime: tokio::runtime::Runtime,
}

impl Harness {
    fn start() -> Harness {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let server = runtime
            .block_on(Server::bind(ServeOptions::default()))
            .unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            let _ = server.serve().await;
        });

        Harness {
            addr,
            token,
            _runtime: runtime,
        }
    }

    fn get(&self, path: &str) -> Response {
        self.request(path, Some(&self.token))
    }

    fn get_anonymous(&self, path: &str) -> Response {
        self.request(path, None)
    }

    fn request(&self, path: &str, token: Option<&str>) -> Response {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        let auth = match token {
            Some(t) => format!("{TOKEN_HEADER}: {t}\r\n"),
            None => String::new(),
        };
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {}\r\n{auth}Connection: close\r\n\r\n",
            self.addr
        )
        .unwrap();

        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").expect("a complete response");
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("a status line");
        Response {
            status,
            head: head.to_lowercase(),
            body: body.to_string(),
        }
    }
}

struct Response {
    status: u16,
    head: String,
    body: String,
}

impl Response {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("expected JSON, got {:?}: {e}", self.body))
    }
}

#[test]
fn health_reports_the_version_and_the_bundle() {
    let app = Harness::start();
    let response = app.get("/api/health");

    assert_eq!(response.status, 200);
    let body = response.json();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    // Not asserted as true: a `cargo test` on a machine without node embeds nothing,
    // and failing here would blame the frontend for an absent toolchain. CI proves the
    // bundle is real and current with `git diff --exit-code -- ui/dist`.
    assert!(body["bundle_embedded"].is_boolean());
    assert!(body["bundle_files"].is_number());
}

#[test]
fn the_api_is_closed_without_the_token() {
    let app = Harness::start();

    let anonymous = app.get_anonymous("/api/health");
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.json()["error"], "unauthorized");

    let wrong = app.request("/api/health", Some("0".repeat(64).as_str()));
    assert_eq!(wrong.status, 401);
}

#[test]
fn the_token_may_arrive_in_the_query() {
    // EventSource cannot set a header, so the query has to work - and it is also the
    // URL the CLI prints and the desktop shell opens.
    let app = Harness::start();
    let path = format!("/api/health?{TOKEN_QUERY}={}", app.token);
    assert_eq!(app.get_anonymous(&path).status, 200);
}

#[test]
fn unknown_paths_fall_back_to_the_app() {
    // A deep link reloaded in the browser must reach the client router, not a 404.
    let app = Harness::start();
    let response = app.get_anonymous("/console/some-run");

    assert_eq!(response.status, 200);
    assert!(response.head.contains("text/html"));
    assert!(response.body.contains("<!doctype html>"));
    // Served from memory and rebuilt with the binary, so a cached copy of the previous
    // version is exactly what we do not want.
    assert!(response.head.contains("cache-control: no-cache"));
}

#[test]
fn the_url_carries_the_token() {
    let app = Harness::start();
    let expected = format!("http://{}/?{TOKEN_QUERY}={}", app.addr, app.token);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime
        .block_on(Server::bind(ServeOptions {
            port: 0,
            token: Some("fixed-token".into()),
        }))
        .unwrap();

    assert!(expected.contains(&app.token));
    assert_eq!(server.token(), "fixed-token");
    assert!(server.url().ends_with("token=fixed-token"));
}
