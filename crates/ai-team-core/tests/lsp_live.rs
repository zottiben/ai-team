//! The LSP client against the real servers.
//!
//! Framing and routing are unit-tested next to the code. What cannot be checked that way
//! is whether a real rust-analyzer or typescript-language-server accepts the handshake
//! this sends and answers the requests it makes - and both have opinions the
//! specification does not record.
//!
//! Skipped when the server is not installed. A machine without rust-analyzer is a normal
//! machine, and failing CI on somebody else's missing binary teaches nothing - the
//! skipped line says so out loud rather than passing silently.

use std::time::Duration;

use ai_team_core::{Client, Position};

fn installed(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// How long to wait for a real language server to have an opinion.
///
/// Generous on purpose. These drive live servers, and rust-analyzer's first answer costs
/// an index build - which on a laptop is seconds and on a loaded CI runner, sharing cores
/// with the rest of this suite compiling, is minutes. A tight deadline here does not
/// catch anything; it just fails on a busy machine, and a test that fails when the runner
/// is busy gets muted.
const PATIENCE: Duration = Duration::from_secs(180);

/// Wait for a server to publish diagnostics for a file.
///
/// Polled rather than awaited once: rust-analyzer builds its index before it says
/// anything, and asking immediately gets "nothing published yet" every time.
async fn wait_for_diagnostics(
    client: &Client,
    path: &str,
    timeout: Duration,
) -> Option<Vec<ai_team_core::Diagnostic>> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Some(found) = client.diagnostics(path).await {
            if !found.is_empty() {
                return Some(found);
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    client.diagnostics(path).await
}

#[tokio::test]
async fn rust_analyzer_attaches_and_reports_a_type_error() {
    if !installed("rust-analyzer") {
        eprintln!("skipped: rust-analyzer is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // The shape an agent actually introduces: a function that says it returns one thing
    // and returns another.
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn broken() -> i32 {\n    \"not a number\"\n}\n",
    )
    .unwrap();

    let language = ai_team_core::language_for("src/lib.rs").unwrap();
    let client = Client::start(language, root)
        .await
        .expect("rust-analyzer should start");
    client
        .sync(
            "src/lib.rs",
            &std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        )
        .await
        .unwrap();

    let found = wait_for_diagnostics(&client, "src/lib.rs", PATIENCE)
        .await
        .expect("rust-analyzer published nothing at all - was the machine this loaded?");

    // Asserted on what is stable rather than on rustc's phrasing: the wording here is
    // "expected i32, found &'static str", not "mismatched types", and pinning a test to a
    // compiler's prose is how it breaks on the next toolchain bump.
    let error = found
        .iter()
        .find(|d| d.severity == Some(1))
        .unwrap_or_else(|| panic!("expected an error among {found:#?}"));
    assert_eq!(error.source.as_deref(), Some("rust-analyzer"));
    assert!(error.message.contains("i32"), "{error:#?}");

    // And it points at the offending line, which is what makes a gutter marker useful
    // rather than decorative. Zero-based: line 5 is the string literal.
    assert_eq!(error.range.start.line, 5, "{error:#?}");
}

#[tokio::test]
async fn rust_analyzer_answers_hover_and_definition() {
    if !installed("rust-analyzer") {
        eprintln!("skipped: rust-analyzer is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let source = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn use_it() -> i32 {\n    add(1, 2)\n}\n";
    std::fs::write(root.join("src/lib.rs"), source).unwrap();

    let language = ai_team_core::language_for("src/lib.rs").unwrap();
    let client = Client::start(language, root).await.unwrap();
    client.sync("src/lib.rs", source).await.unwrap();

    // Give it a moment to index before asking; an answer of None early is not an error.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut hover = None;
    while tokio::time::Instant::now() < deadline {
        // Line 5, on the call to `add`.
        hover = client
            .hover(
                "src/lib.rs",
                Position {
                    line: 5,
                    character: 5,
                },
            )
            .await
            .unwrap();
        if hover.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let hover =
        hover.expect("hover said nothing within the deadline - was the machine this loaded?");
    assert!(hover.text.contains("add"), "{}", hover.text);

    let places = client
        .definition(
            "src/lib.rs",
            Position {
                line: 5,
                character: 5,
            },
        )
        .await
        .unwrap();
    assert!(
        !places.is_empty(),
        "go-to-definition should find the function"
    );
    // Defined on line 0, and reported relative to the workspace root.
    assert_eq!(places[0].path, "src/lib.rs");
    assert_eq!(places[0].range.start.line, 0);
}

#[tokio::test]
async fn typescript_attaches_and_reports_a_type_error() {
    // The server is a devDependency of `ui/`, so the client finds it the way it would in
    // any repository that installs one - through the project's own node_modules/.bin.
    let installed_at = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/node_modules/.bin/typescript-language-server");
    if !installed_at.is_file() {
        eprintln!("skipped: typescript-language-server is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A node_modules/.bin beside the project, as a real checkout has.
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        installed_at.canonicalize().unwrap(),
        bin.join("typescript-language-server"),
    )
    .unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true, "noEmit": true } }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("index.ts"),
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport const wrong: number = \"not a number\";\n",
    )
    .unwrap();

    let language = ai_team_core::language_for("index.ts").unwrap();
    let client = Client::start(language, root)
        .await
        .expect("tsserver should start");
    client
        .sync(
            "index.ts",
            &std::fs::read_to_string(root.join("index.ts")).unwrap(),
        )
        .await
        .unwrap();

    let found = wait_for_diagnostics(&client, "index.ts", PATIENCE)
        .await
        .expect("typescript published nothing at all - was the machine this loaded?");
    let error = found
        .iter()
        .find(|d| d.severity == Some(1))
        .unwrap_or_else(|| panic!("expected an error among {found:#?}"));
    assert!(error.message.contains("not assignable"), "{error:#?}");
    assert_eq!(error.range.start.line, 4, "{error:#?}");
}

#[tokio::test]
async fn a_server_that_is_not_installed_is_an_error_worth_reading() {
    // Plenty of machines have neither. The message has to say what to do about it.
    let language = ai_team_core::Language {
        id: "nonsense",
        command: "definitely-not-a-language-server",
        args: &[],
    };
    let dir = tempfile::tempdir().unwrap();
    let error = Client::start(language, dir.path()).await.unwrap_err();
    let text = error.to_string();
    assert!(text.contains("definitely-not-a-language-server"), "{text}");
    assert!(text.contains("PATH"), "{text}");
}
