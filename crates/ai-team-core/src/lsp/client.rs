//! One language server, from spawn to shutdown.
//!
//! LSP is bidirectional: a client sends requests and gets responses back *out of order*,
//! and the server sends notifications nobody asked for - which is how diagnostics arrive.
//! So a reader task owns stdout permanently, matches responses to waiting callers by id,
//! and files everything else where the surface can find it.
//!
//! Two things this deliberately does not do. It does not synthesise diagnostics when the
//! server has not sent any: "no diagnostics yet" and "this file is clean" look identical
//! in a gutter and mean opposite things, so the caller is told which it has. And it does
//! not restart a server that dies - a crashed rust-analyzer usually means the project
//! does not build, and respawning it in a loop turns that into a fork bomb.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};

use crate::error::{Error, Result};
use crate::lsp::{wire, Language};

/// Zero-based, as LSP counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: i64,
    pub character: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// A problem the server found, as the gutter needs it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint - LSP's own numbering, kept rather than
    /// renamed so a server's meaning survives the trip.
    #[serde(default)]
    pub severity: Option<i64>,
    #[serde(default)]
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hover {
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    /// Relative to the workspace root, because that is what the editor opens.
    pub path: String,
    pub range: Range,
}

/// What the reader task has collected.
#[derive(Debug, Default)]
struct Inbox {
    /// Callers waiting on a response, by request id.
    waiting: HashMap<i64, oneshot::Sender<Value>>,
    /// The latest diagnostics per document uri. Replaced wholesale, because a publish is
    /// the complete set for that file - merging would leave fixed errors on screen.
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    /// Which documents the server has ever published for, so "no diagnostics yet" can be
    /// told apart from "this file is clean".
    published: std::collections::HashSet<String>,
}

/// A running language server.
#[derive(Debug)]
pub struct Client {
    language: Language,
    root: PathBuf,
    stdin: Mutex<ChildStdin>,
    inbox: Arc<Mutex<Inbox>>,
    next_id: AtomicI64,
    /// Kept so dropping the client kills the server rather than orphaning it.
    child: Mutex<Child>,
    /// What the server wrote to stderr, bounded.
    complaint: Arc<Mutex<String>>,
    /// Its exit status, once it has one.
    exited: Mutex<Option<i32>>,
    /// Documents currently open on the server, with the version last sent.
    open: Mutex<HashMap<String, i64>>,
}

impl Client {
    /// Spawn a server and complete the handshake.
    pub async fn start(language: Language, root: &Path) -> Result<Client> {
        let mut child = tokio::process::Command::new(resolve(language.command, root))
            .args(language.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Drained, not discarded. A server that refuses to run says why here and
            // nowhere else - `rust-analyzer` on PATH is often a rustup shim, and outside
            // a project pinned to a toolchain that has the component it prints "Unknown
            // binary" and exits. Discarding that turns an actionable message into twenty
            // seconds of silence. It must be *drained* rather than merely piped:
            // rust-analyzer writes progress constantly and a full pipe nobody reads
            // deadlocks the server.
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                Error::invalid(format!(
                    "could not start {}: {error} - language features need it on PATH",
                    language.command
                ))
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::invalid("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::invalid("no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::invalid("no stderr"))?;

        // Kept bounded: this is for a failure message, not a log, and a server that runs
        // for a week must not accumulate its whole progress stream in memory.
        let complaint = Arc::new(Mutex::new(String::new()));
        tokio::spawn(drain_stderr(stderr, Arc::clone(&complaint)));

        let inbox = Arc::new(Mutex::new(Inbox::default()));
        tokio::spawn(read_loop(stdout, Arc::clone(&inbox)));

        let client = Client {
            language,
            root: root.to_path_buf(),
            stdin: Mutex::new(stdin),
            inbox,
            next_id: AtomicI64::new(1),
            child: Mutex::new(child),
            complaint,
            exited: Mutex::new(None),
            open: Mutex::new(HashMap::new()),
        };

        // Reported with what the server said, and how it ended. "no answer in 20s" is
        // true and useless; "exited immediately: Unknown binary 'rust-analyzer'" is the
        // thing somebody can act on.
        match client.handshake().await {
            Ok(()) => Ok(client),
            Err(error) => Err(Error::invalid(client.explain(error).await)),
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Turn a handshake failure into something worth reading.
    async fn explain(&self, error: Error) -> String {
        let said = self.complaint.lock().await.trim().to_string();
        let ended = self.exited.lock().await.map_or_else(
            || "it is still running".to_string(),
            |code| format!("it exited with status {code}"),
        );

        let mut out = format!("{} did not start: {error}; {ended}", self.language.command);
        if !said.is_empty() {
            // The last line, because a server that fails usually says why on its way out
            // and everything before it is progress.
            let last = said.lines().last().unwrap_or_default();
            let _ = write!(out, " - it said: {last}");
        }
        out
    }

    async fn handshake(&self) -> Result<()> {
        let root_uri = uri_for(&self.root);
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }],
                "capabilities": {
                    "textDocument": {
                        "synchronization": { "didSave": true, "dynamicRegistration": false },
                        "publishDiagnostics": { "relatedInformation": false },
                        "hover": { "contentFormat": ["markdown", "plaintext"] },
                        "definition": { "linkSupport": false },
                        "completion": {
                            "completionItem": { "snippetSupport": false },
                        },
                        "rename": { "prepareSupport": false },
                    },
                    "workspace": { "workspaceFolders": true, "applyEdit": false },
                },
            }),
        )
        .await?;
        self.notify("initialized", json!({})).await
    }

    /// Tell the server about a document, or that it changed.
    ///
    /// The full text every time rather than incremental edits: the window already has the
    /// whole buffer, and an incremental sync that drifts by one character produces
    /// diagnostics pointing at the wrong line, which is worse than none.
    pub async fn sync(&self, path: &str, text: &str) -> Result<()> {
        let uri = uri_for(&self.root.join(path));
        let mut open = self.open.lock().await;
        match open.get(&uri).copied() {
            None => {
                open.insert(uri.clone(), 1);
                drop(open);
                self.notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": self.language.id,
                            "version": 1,
                            "text": text,
                        }
                    }),
                )
                .await
            }
            Some(version) => {
                let next = version + 1;
                open.insert(uri.clone(), next);
                drop(open);
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": next },
                        "contentChanges": [{ "text": text }],
                    }),
                )
                .await
            }
        }
    }

    /// What the server has said about a file.
    ///
    /// `None` means it has not published for this document yet, which is not the same as
    /// an empty list and must not be drawn as a clean gutter.
    pub async fn diagnostics(&self, path: &str) -> Option<Vec<Diagnostic>> {
        let uri = uri_for(&self.root.join(path));
        let inbox = self.inbox.lock().await;
        inbox
            .published
            .contains(&uri)
            .then(|| inbox.diagnostics.get(&uri).cloned().unwrap_or_default())
    }

    pub async fn hover(&self, path: &str, at: Position) -> Result<Option<Hover>> {
        let value = self
            .request("textDocument/hover", self.at(path, at))
            .await?;
        Ok(hover_text(&value).map(|text| Hover { text }))
    }

    pub async fn definition(&self, path: &str, at: Position) -> Result<Vec<Location>> {
        let value = self
            .request("textDocument/definition", self.at(path, at))
            .await?;
        Ok(self.locations(&value))
    }

    pub async fn completion(&self, path: &str, at: Position) -> Result<Vec<String>> {
        let value = self
            .request("textDocument/completion", self.at(path, at))
            .await?;
        // A completion response is either a list or an object wrapping one.
        let items = value
            .get("items")
            .or(Some(&value))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items
            .iter()
            .filter_map(|item| item.get("label")?.as_str().map(ToString::to_string))
            .collect())
    }

    /// The edits a rename would make, as paths and replacements.
    pub async fn rename(
        &self,
        path: &str,
        at: Position,
        new_name: &str,
    ) -> Result<Vec<(String, Range, String)>> {
        let mut params = self.at(path, at);
        params["newName"] = json!(new_name);
        let value = self.request("textDocument/rename", params).await?;
        Ok(edits_from(&self.root, &value))
    }

    fn at(&self, path: &str, at: Position) -> Value {
        json!({
            "textDocument": { "uri": uri_for(&self.root.join(path)) },
            "position": { "line": at.line, "character": at.character },
        })
    }

    /// A `file://` uri back to a path the editor can open.
    fn relative(&self, uri: &str) -> Option<String> {
        relative_to(&self.root, uri)
    }

    fn locations(&self, value: &Value) -> Vec<Location> {
        // `definition` may answer with one location, a list, or link objects.
        let list = match value {
            Value::Array(items) => items.clone(),
            Value::Null => Vec::new(),
            other => vec![other.clone()],
        };
        list.iter()
            .filter_map(|entry| {
                let uri = entry
                    .get("uri")
                    .or_else(|| entry.get("targetUri"))?
                    .as_str()?;
                let range = entry
                    .get("range")
                    .or_else(|| entry.get("targetSelectionRange"))
                    .or_else(|| entry.get("targetRange"))
                    .and_then(range_of)?;
                Some(Location {
                    path: self.relative(uri)?,
                    range,
                })
            })
            .collect()
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inbox.lock().await.waiting.insert(id, tx);

        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        // Bounded, because a server that never answers must not hold a request open
        // forever - the window is polling and would stack them up. Racing the process
        // itself means a server that exits is noticed at once rather than at the timeout:
        // the common failure is a wrapper script that refuses to run, and waiting the full
        // twenty seconds to say nothing helps nobody.
        let outcome = tokio::select! {
            answer = tokio::time::timeout(std::time::Duration::from_secs(20), rx) => match answer {
                Ok(Ok(value)) => Outcome::Answered(value),
                Ok(Err(_)) => Outcome::Stopped,
                Err(_) => Outcome::Silent,
            },
            status = async {
                let mut child = self.child.lock().await;
                child.wait().await
            } => {
                if let Ok(status) = status {
                    *self.exited.lock().await = status.code();
                }
                Outcome::Stopped
            }
        };

        match outcome {
            Outcome::Answered(value) => Ok(value),
            Outcome::Stopped => Err(Error::invalid(format!("{method}: the server stopped"))),
            Outcome::Silent => {
                self.inbox.lock().await.waiting.remove(&id);
                Err(Error::invalid(format!("{method}: no answer in 20s")))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn send(&self, message: &Value) -> Result<()> {
        let bytes = wire::frame(&message.to_string());
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&bytes)
            .await
            .map_err(|error| Error::invalid(format!("writing to the language server: {error}")))?;
        stdin
            .flush()
            .await
            .map_err(|error| Error::invalid(format!("flushing to the language server: {error}")))
    }
}

/// Read a `WorkspaceEdit` in either of the two shapes the specification allows.
///
/// `changes` is a map of uri to edits; `documentChanges` is a list of documents each
/// carrying its own. Servers genuinely differ - rust-analyzer answers rename with
/// `documentChanges` even when the client advertises nothing - and reading only one
/// returns an empty rename that looks like "nothing to do" rather than like a bug.
///
/// A free function rather than a method because it needs nothing but the root, which is
/// also what lets it be tested without spawning a language server.
fn edits_from(root: &Path, value: &Value) -> Vec<(String, Range, String)> {
    let mut edits = Vec::new();

    let mut take = |uri: &str, list: &Value| {
        let Some(relative) = relative_to(root, uri) else {
            return;
        };
        for edit in list.as_array().into_iter().flatten() {
            if let (Some(range), Some(text)) = (
                edit.get("range").and_then(range_of),
                edit.get("newText").and_then(Value::as_str),
            ) {
                edits.push((relative.clone(), range, text.to_string()));
            }
        }
    };

    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, list) in changes {
            take(uri, list);
        }
    }

    for change in value
        .get("documentChanges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // A `documentChanges` entry may also be a create, rename or delete operation,
        // which has no `textDocument` and is not an edit to apply here.
        let Some(uri) = change
            .get("textDocument")
            .and_then(|doc| doc.get("uri"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(list) = change.get("edits") {
            take(uri, list);
        }
    }

    edits
}

/// A `file://` uri back to a path relative to a root.
fn relative_to(root: &Path, uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    let decoded = percent_decode(path);
    Path::new(&decoded)
        .strip_prefix(root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

/// How a request ended.
enum Outcome {
    Answered(Value),
    /// The server exited, or dropped the channel.
    Stopped,
    /// Still running, still saying nothing.
    Silent,
}

/// Keep stderr drained, and keep the tail of it.
///
/// Draining matters as much as keeping: rust-analyzer writes progress continuously, and a
/// pipe nobody reads fills and blocks the server mid-session.
async fn drain_stderr(mut stderr: tokio::process::ChildStderr, into: Arc<Mutex<String>>) {
    const KEEP: usize = 4 * 1024;
    let mut chunk = vec![0u8; 8 * 1024];
    loop {
        let Ok(read) = stderr.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        let mut text = into.lock().await;
        text.push_str(&String::from_utf8_lossy(&chunk[..read]));
        if text.len() > KEEP {
            // Keep the end: a server that fails says why on its way out.
            let cut = text.len() - KEEP;
            let boundary = (cut..text.len())
                .find(|index| text.is_char_boundary(*index))
                .unwrap_or(text.len());
            *text = text[boundary..].to_string();
        }
    }
}

/// Own stdout for the life of the process.
///
/// Responses and notifications share one stream, so there is no way to read "just my
/// answer" - everything goes through here and is routed by id or by method.
async fn read_loop(mut stdout: tokio::process::ChildStdout, inbox: Arc<Mutex<Inbox>>) {
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];

    loop {
        let Ok(read) = stdout.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);

        loop {
            match wire::take_message(&mut buffer) {
                Ok(Some(text)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    route(&value, &inbox).await;
                }
                Ok(None) => break,
                // A frame this cannot read means the stream is desynchronised and every
                // later message would be garbage. Stopping is honest; guessing is not.
                Err(_) => return,
            }
        }
    }
}

async fn route(value: &Value, inbox: &Arc<Mutex<Inbox>>) {
    if let Some(id) = value.get("id").and_then(Value::as_i64) {
        // A response. An error is delivered as the error object, so the caller sees what
        // the server said rather than a timeout twenty seconds later.
        let payload = value
            .get("result")
            .or_else(|| value.get("error"))
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(waiting) = inbox.lock().await.waiting.remove(&id) {
            let _ = waiting.send(payload);
        }
        return;
    }

    if value.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics") {
        let Some(params) = value.get("params") else {
            return;
        };
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return;
        };
        let list: Vec<Diagnostic> = params
            .get("diagnostics")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();

        let mut inbox = inbox.lock().await;
        // Replaced wholesale: a publish is the complete set for that file, and merging
        // would leave errors on screen after they were fixed.
        inbox.diagnostics.insert(uri.to_string(), list);
        inbox.published.insert(uri.to_string());
    }
}

/// Where to find a language server.
///
/// A project's own `node_modules/.bin` first, walking up from the workspace root, then
/// whatever is on PATH. This is what editors do, and it matters: a repository that lists
/// `typescript-language-server` as a devDependency has already installed the right
/// version, and requiring a global one as well means two copies that drift.
fn resolve(command: &str, root: &Path) -> PathBuf {
    let mut dir = Some(root);
    while let Some(here) = dir {
        let candidate = here.join("node_modules").join(".bin").join(command);
        if candidate.is_file() {
            return candidate;
        }
        dir = here.parent();
    }
    PathBuf::from(command)
}

/// A path as a `file://` uri.
///
/// Percent-encoded, because a repository checked out under a path with a space produces a
/// uri servers reject - and "it works unless your folder has a space in it" is the kind of
/// bug that takes an afternoon.
fn uri_for(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn range_of(value: &Value) -> Option<Range> {
    serde_json::from_value(value.clone()).ok()
}

/// Hover comes back as a string, a marked-up object, or a list of either.
fn hover_text(value: &Value) -> Option<String> {
    let contents = value.get("contents")?;
    let text = match contents {
        Value::String(text) => text.clone(),
        Value::Object(map) => map.get("value")?.as_str()?.to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                Value::Object(map) => map.get("value")?.as_str().map(ToString::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_in_the_projects_own_node_modules_wins_over_a_global_one() {
        // A repo that lists the server as a devDependency has already installed the right
        // version; requiring a global one too means two copies that drift.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("typescript-language-server"), "#!/bin/sh\n").unwrap();

        assert_eq!(
            resolve("typescript-language-server", dir.path()),
            bin.join("typescript-language-server")
        );
    }

    #[test]
    fn a_nested_project_finds_the_install_above_it() {
        // `ui/src` is where the file is; `ui/node_modules` is where the server is.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(bin.join("tsls"), "#!/bin/sh\n").unwrap();

        assert_eq!(
            resolve("tsls", &dir.path().join("src/deep")),
            bin.join("tsls")
        );
    }

    #[test]
    fn with_nothing_installed_locally_it_falls_back_to_the_path() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve("rust-analyzer", dir.path()),
            PathBuf::from("rust-analyzer")
        );
    }

    #[test]
    fn a_path_with_a_space_survives_the_trip_to_a_uri_and_back() {
        // "it works unless your folder has a space in it" is an afternoon.
        let path = Path::new("/home/a b/Developer/my repo/src/lib.rs");
        let uri = uri_for(path);
        assert!(uri.contains("%20"), "{uri}");
        assert_eq!(
            percent_decode(uri.strip_prefix("file://").unwrap()),
            path.to_string_lossy()
        );
    }

    #[test]
    fn hover_is_read_from_every_shape_a_server_sends() {
        assert_eq!(
            hover_text(&json!({ "contents": "fn add()" })).as_deref(),
            Some("fn add()")
        );
        assert_eq!(
            hover_text(&json!({ "contents": { "kind": "markdown", "value": "fn add()" } }))
                .as_deref(),
            Some("fn add()")
        );
        assert_eq!(
            hover_text(&json!({ "contents": [{ "value": "one" }, "two"] })).as_deref(),
            Some("one\n\ntwo")
        );
    }

    #[test]
    fn an_empty_hover_is_nothing_rather_than_a_blank_tooltip() {
        assert!(hover_text(&json!({ "contents": "" })).is_none());
        assert!(hover_text(&json!({ "contents": "   " })).is_none());
        assert!(hover_text(&json!({})).is_none());
    }

    #[test]
    fn a_workspace_edit_is_read_in_either_shape() {
        // rust-analyzer answers rename with `documentChanges` even when the client
        // advertises nothing; others send `changes`. Reading one returns an empty rename
        // that looks like "nothing to do" rather than like a bug.
        let with_changes = json!({
            "changes": {
                "file:///r/src/lib.rs": [
                    { "range": { "start": { "line": 0, "character": 7 },
                                 "end": { "line": 0, "character": 10 } },
                      "newText": "plus" }
                ]
            }
        });
        let with_documents = json!({
            "documentChanges": [{
                "textDocument": { "uri": "file:///r/src/lib.rs", "version": 1 },
                "edits": [
                    { "range": { "start": { "line": 0, "character": 7 },
                                 "end": { "line": 0, "character": 10 } },
                      "newText": "plus" }
                ]
            }]
        });

        for value in [with_changes, with_documents] {
            let edits = edits_from(Path::new("/r"), &value);
            assert_eq!(edits.len(), 1, "{value}");
            assert_eq!(edits[0].0, "src/lib.rs");
            assert_eq!(edits[0].2, "plus");
            assert_eq!(edits[0].1.start.character, 7);
        }
    }

    #[test]
    fn a_create_or_delete_operation_is_not_an_edit() {
        // `documentChanges` may carry file operations, which have no textDocument.
        let value = json!({
            "documentChanges": [
                { "kind": "create", "uri": "file:///r/src/new.rs" },
                { "textDocument": { "uri": "file:///r/src/lib.rs" }, "edits": [] }
            ]
        });
        assert!(edits_from(Path::new("/r"), &value).is_empty());
    }

    #[tokio::test]
    async fn a_response_reaches_the_caller_waiting_on_its_id() {
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let (tx, rx) = oneshot::channel();
        inbox.lock().await.waiting.insert(7, tx);

        route(&json!({ "id": 7, "result": { "ok": true } }), &inbox).await;
        assert_eq!(rx.await.unwrap(), json!({ "ok": true }));
    }

    #[tokio::test]
    async fn an_error_response_reaches_the_caller_instead_of_timing_out() {
        // Otherwise the caller waits the full twenty seconds to learn the server already
        // said no.
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let (tx, rx) = oneshot::channel();
        inbox.lock().await.waiting.insert(7, tx);

        route(
            &json!({ "id": 7, "error": { "code": -32601, "message": "nope" } }),
            &inbox,
        )
        .await;
        assert_eq!(rx.await.unwrap()["message"], "nope");
    }

    #[tokio::test]
    async fn diagnostics_are_replaced_wholesale_not_merged() {
        // A publish is the complete set for that file. Merging would leave errors on
        // screen after they had been fixed.
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let uri = "file:///r/src/lib.rs";

        route(
            &json!({
                "method": "textDocument/publishDiagnostics",
                "params": { "uri": uri, "diagnostics": [
                    { "range": { "start": { "line": 1, "character": 0 },
                                 "end": { "line": 1, "character": 4 } },
                      "severity": 1, "message": "mismatched types" }
                ]}
            }),
            &inbox,
        )
        .await;
        assert_eq!(inbox.lock().await.diagnostics[uri].len(), 1);

        route(
            &json!({
                "method": "textDocument/publishDiagnostics",
                "params": { "uri": uri, "diagnostics": [] }
            }),
            &inbox,
        )
        .await;
        assert!(inbox.lock().await.diagnostics[uri].is_empty());
    }

    #[tokio::test]
    async fn a_file_with_no_publish_yet_is_not_a_clean_file() {
        // They look identical in a gutter and mean opposite things.
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        assert!(!inbox.lock().await.published.contains("file:///r/a.rs"));

        route(
            &json!({
                "method": "textDocument/publishDiagnostics",
                "params": { "uri": "file:///r/a.rs", "diagnostics": [] }
            }),
            &inbox,
        )
        .await;
        assert!(inbox.lock().await.published.contains("file:///r/a.rs"));
    }
}
