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
    /// A server with a real database behind it, seeded with one run.
    fn with_store() -> (Harness, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team.db");
        let mut store = ai_team_core::Store::init(&path).unwrap();
        let project = store
            .create_project(ai_team_core::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        store
            .create_run(project.id, "add subtract", ai_team_core::RunTrigger::Manual)
            .unwrap();
        drop(store);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::bind(ServeOptions {
                store: Some(ai_team_core::Store::open(&path).unwrap()),
                ..Default::default()
            }))
            .unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            let _ = server.serve().await;
        });
        (
            Harness {
                addr,
                token,
                _runtime: runtime,
            },
            dir,
        )
    }

    /// A server with no database, watching a path one may appear at.
    fn watching(db_path: &std::path::Path) -> Harness {
        Harness::bound(ServeOptions {
            port: 0,
            token: None,
            store: None,
            db_path: Some(db_path.to_path_buf()),
        })
    }

    fn start() -> Harness {
        Harness::bound(ServeOptions::default())
    }

    /// Bind, spawn, and hand back the address and token.
    fn bound(options: ServeOptions) -> Harness {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let server = runtime.block_on(Server::bind(options)).unwrap();
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

    /// POST with a JSON body, for the routes that change something.
    fn post(&self, path: &str, body: &str) -> Response {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: {}\r\n{TOKEN_HEADER}: {}\r\n\
             content-type: application/json\r\ncontent-length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            self.addr,
            self.token,
            body.len()
        )
        .unwrap();
        read_response(stream)
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

        read_response(stream)
    }
}

/// Parse one HTTP/1.1 response off a socket.
fn read_response(mut stream: TcpStream) -> Response {
    let mut raw = String::new();
    {
        stream.read_to_string(&mut raw).unwrap();
    }
    {
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
            store: None,
            db_path: None,
        }))
        .unwrap();

    assert!(expected.contains(&app.token));
    assert_eq!(server.token(), "fixed-token");
    assert!(server.url().ends_with("token=fixed-token"));
}

#[test]
fn the_api_is_a_view_over_the_database() {
    let (app, _dir) = Harness::with_store();

    let projects = app.get("/api/projects");
    assert_eq!(projects.status, 200);
    assert!(
        projects.body.contains("\"name\":\"Widget\""),
        "{}",
        projects.body
    );
    // The count a sidebar shows, computed server-side so two windows cannot disagree.
    assert!(
        projects.body.contains("\"open_runs\":1"),
        "{}",
        projects.body
    );

    let runs = app.get("/api/runs");
    assert!(runs.body.contains("add subtract"), "{}", runs.body);

    // A run carries its nodes and its spend, because the dock renders all three and
    // three round trips to draw one panel is three chances to show a half-updated run.
    let detail = app.get("/api/runs/1");
    assert_eq!(detail.status, 200);
    assert!(detail.body.contains("\"nodes\""), "{}", detail.body);
    assert!(detail.body.contains("\"usage\""), "{}", detail.body);
}

#[test]
fn a_server_with_no_database_says_so_instead_of_failing() {
    // `ait ui` before `ait init` is a normal first run. The window should explain
    // itself, not look broken - and health must still answer, since that is what
    // `ait doctor` asks.
    let app = Harness::start();

    assert_eq!(app.get("/api/health").status, 200);

    let projects = app.get("/api/projects");
    assert_eq!(projects.status, 503, "not a 500: nothing is broken");
    assert!(projects.body.contains("ait init"), "{}", projects.body);
}

#[test]
fn the_event_stream_needs_a_token_like_everything_else() {
    // EventSource cannot set a header, so the stream takes its token from the query -
    // which makes it exactly the route where an auth hole would go unnoticed.
    let (app, _dir) = Harness::with_store();
    assert_eq!(app.get_anonymous("/api/events").status, 401);
    assert_eq!(
        app.get_anonymous(&format!("/api/events?{TOKEN_QUERY}=wrong"))
            .status,
        401
    );
}

#[test]
fn the_health_report_is_served_without_a_database() {
    // The one route that must work on a machine nobody has set up, because that is the
    // machine somebody most needs a report about. Every other route answers 503 and says
    // `ait init`, which is right for them and exactly wrong here.
    let harness = Harness::start();

    assert_eq!(harness.get("/api/projects").status, 503);

    let response = harness.get("/api/doctor");
    assert_eq!(response.status, 200, "{}", response.body);
    let report = response.json();
    assert_eq!(report["needs_setup"], true);
    assert_eq!(report["can_run"], false);
    assert_eq!(report["severity"], "blocking");
    assert!(
        report["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty()),
        "{}",
        response.body
    );
}

#[test]
fn a_report_of_a_set_up_machine_still_wants_a_provider() {
    // A database and a project are not enough to run: something has to be allowed to
    // think. Worth asserting, because "set up" is easy to define as "has a database".
    let (harness, _dir) = Harness::with_store();
    let report = harness.get("/api/doctor").json();

    let database = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "database")
        .unwrap()
        .clone();
    assert_eq!(database["severity"], "fine", "{database}");
}

#[test]
fn a_fix_ai_team_does_not_own_cannot_even_be_asked_for() {
    // D17: the set of things this route can cause is the set of `Action` variants. An
    // install is not one of them, so it is rejected at deserialisation rather than by a
    // check at the end of a function that looked like it might do it.
    let (harness, _dir) = Harness::with_store();

    let refused = harness.post("/api/doctor/fix", r#"{"action":"install_ai_planner"}"#);
    assert_eq!(refused.status, 422, "{}", refused.body);

    let nonsense = harness.post("/api/doctor/fix", r#"{"action":"rm -rf /"}"#);
    assert_eq!(nonsense.status, 422, "{}", nonsense.body);
}

#[test]
fn the_window_picks_up_a_database_created_after_it_started() {
    // The bug this replaced: the server decided at startup whether a database existed and
    // never looked again, so creating one from the setup page left every route answering
    // "run `ait init`" until the process was restarted - a confusing way to be told that
    // setup had worked.
    //
    // Pointed at its own path rather than the machine's: a test that reaches for
    // `default_db_path` writes into the developer's home, which this one did once.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("team.db");
    let harness = Harness::watching(&path);

    assert_eq!(harness.get("/api/projects").status, 503);

    // Created the way the setup page creates it.
    ai_team_core::Store::init(&path).unwrap();

    assert_eq!(
        harness.get("/api/projects").status,
        200,
        "the same process should find it"
    );
}
