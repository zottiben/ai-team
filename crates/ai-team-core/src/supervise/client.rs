//! The `/eve/v1` surface, typed.
//!
//! One client per supervised process. It holds the port and the shared secret ai-team
//! minted for that process, so no call site has to remember either.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::eve::StreamEvent;
use crate::supervise::http::{self, Flow, Request};

/// Long enough for a cold local model to answer, short enough that a wedged process is
/// noticed. Applied per line of the stream, not to the turn.
const STREAM_IDLE: Duration = Duration::from_secs(300);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct EveClient {
    port: u16,
    auth: String,
}

/// What a parked turn is waiting for, lifted out of an `input.requested` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub request_id: String,
    pub prompt: String,
    /// The choices eve offered. Empty means free text.
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalOption {
    pub id: String,
    pub label: String,
}

impl EveClient {
    pub fn new(port: u16, token: &str) -> EveClient {
        EveClient {
            port,
            // The username is fixed; the generated channel checks it alongside the
            // secret.
            auth: http::basic_auth("ai-team", token),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Is the process up? The one route that needs no credential.
    pub async fn healthy(&self) -> bool {
        matches!(
            http::send(self.port, &Request::get("/eve/v1/health"), Duration::from_secs(5)).await,
            Ok(response) if response.ok() && response.body.contains("\"ok\":true")
        )
    }

    /// Wait for the process to answer, or give up.
    ///
    /// Polls rather than retrying once: `eve start` binds its port before the workflow
    /// runtime is ready, and a single early probe would declare a healthy process dead.
    pub async fn wait_until_healthy(&self, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if self.healthy().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(Error::invalid(format!(
            "the eve process on port {} never became healthy",
            self.port
        )))
    }

    /// What eve compiled: the tools and subagents it actually discovered.
    pub async fn info(&self) -> Result<AgentInfo> {
        let response = http::send(
            self.port,
            &Request::get("/eve/v1/info").with_auth(Some(self.auth.clone())),
            CALL_TIMEOUT,
        )
        .await?;
        if !response.ok() {
            return Err(response.error("inspecting the agent"));
        }
        let json = response.json()?;
        Ok(AgentInfo {
            tools: names(&json, "tools", "static"),
            dynamic_tools: names(&json, "tools", "dynamic"),
            subagents: names(&json, "subagents", "local"),
            discovery_errors: json
                .get("diagnostics")
                .and_then(|d| d.get("discoveryErrors"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        })
    }

    /// Start a session and send its first message. Returns the durable session id.
    pub async fn start_session(&self, message: &str) -> Result<String> {
        let body = serde_json::json!({ "message": message }).to_string();
        let response = http::send(
            self.port,
            &Request::post("/eve/v1/session", Some(body)).with_auth(Some(self.auth.clone())),
            CALL_TIMEOUT,
        )
        .await?;
        if !response.ok() {
            return Err(response.error("starting a session"));
        }
        response
            .json()?
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::invalid(format!("no sessionId in {:?}", response.body)))
    }

    /// Send a follow-up. Steers the turn in flight by default, which is eve's behaviour
    /// and the one a human correcting an agent expects.
    pub async fn follow_up(&self, session: &str, message: &str) -> Result<()> {
        let body = serde_json::json!({ "message": message }).to_string();
        self.post(
            &format!("/eve/v1/session/{session}"),
            Some(body),
            "sending a follow-up",
        )
        .await
    }

    /// Answer what a parked turn asked for.
    pub async fn respond(&self, session: &str, request_id: &str, option_id: &str) -> Result<()> {
        let body = serde_json::json!({
            "inputResponses": [{ "requestId": request_id, "optionId": option_id }]
        })
        .to_string();
        self.post(
            &format!("/eve/v1/session/{session}"),
            Some(body),
            "answering a request",
        )
        .await
    }

    /// Ask for the in-flight turn to stop. Asynchronous: the stream reports
    /// `turn.cancelled` when it actually has.
    pub async fn cancel(&self, session: &str, tasks: bool) -> Result<()> {
        let body = serde_json::json!({ "tasks": tasks }).to_string();
        self.post(
            &format!("/eve/v1/session/{session}/cancel"),
            Some(body),
            "cancelling the turn",
        )
        .await
    }

    /// Summarise the context without sending a message.
    pub async fn compact(&self, session: &str) -> Result<()> {
        self.post(
            &format!("/eve/v1/session/{session}/compact"),
            None,
            "compacting",
        )
        .await
    }

    /// Read the session's stream from `from_index`, handing each event to `on_event`.
    ///
    /// `from_index` is eve's absolute event count, which is why `node_run.stream_cursor`
    /// stores exactly that and nothing derived from it.
    pub async fn stream<F>(&self, session: &str, from_index: i64, mut on_event: F) -> Result<()>
    where
        F: FnMut(StreamEvent) -> Flow,
    {
        let path = format!("/eve/v1/session/{session}/stream?startIndex={from_index}");
        let request = Request::get(path).with_auth(Some(self.auth.clone()));
        http::stream(self.port, &request, STREAM_IDLE, |line| {
            match StreamEvent::parse(line) {
                Some(event) => on_event(event),
                // A blank keep-alive line, or half a line at a reconnect boundary.
                // Neither is a reason to abandon the turn.
                None => Flow::Continue,
            }
        })
        .await?;
        Ok(())
    }

    async fn post(&self, path: &str, body: Option<String>, what: &str) -> Result<()> {
        let response = http::send(
            self.port,
            &Request::post(path, body).with_auth(Some(self.auth.clone())),
            CALL_TIMEOUT,
        )
        .await?;
        if response.ok() {
            return Ok(());
        }
        Err(response.error(what))
    }
}

/// What eve reports it compiled. Used by `ait doctor` and by the supervisor to refuse a
/// process whose discovery failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub tools: Vec<String>,
    pub dynamic_tools: Vec<String>,
    pub subagents: Vec<String>,
    pub discovery_errors: i64,
}

/// eve's info payload nests each capability under `static` / `dynamic` / `local`, and
/// names them by `logicalPath`.
fn names(json: &serde_json::Value, section: &str, bucket: &str) -> Vec<String> {
    json.get(section)
        .and_then(|s| s.get(bucket))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            let mut out: Vec<String> = items
                .iter()
                .filter_map(|item| {
                    item.get("logicalPath")
                        .or_else(|| item.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            out.sort();
            out
        })
        .unwrap_or_default()
}

/// Lift the pending requests out of an `input.requested` event.
pub fn approvals_in(event: &StreamEvent) -> Vec<Approval> {
    let Some(requests) = event
        .data
        .get("requests")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    requests
        .iter()
        .filter_map(|request| {
            let request_id = request
                .get("requestId")
                .or_else(|| request.get("id"))
                .and_then(serde_json::Value::as_str)?
                .to_string();
            let prompt = request
                .get("prompt")
                .or_else(|| request.get("question"))
                .or_else(|| request.get("title"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(no prompt)")
                .to_string();
            let options = request
                .get("options")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|option| {
                            let id = option
                                .get("optionId")
                                .or_else(|| option.get("id"))
                                .and_then(serde_json::Value::as_str)?
                                .to_string();
                            let label = option
                                .get("label")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(&id)
                                .to_string();
                            Some(ApprovalOption { id, label })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Approval {
                request_id,
                prompt,
                options,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> StreamEvent {
        StreamEvent::parse(line).unwrap()
    }

    #[test]
    fn approvals_are_lifted_with_their_options() {
        let e = event(
            r#"{"type":"input.requested","data":{"requests":[
                 {"requestId":"req_A","prompt":"Commit to slice/PR1?","options":[
                   {"optionId":"approve","label":"Approve"},
                   {"optionId":"deny","label":"Deny"}]}]}}"#,
        );
        let approvals = approvals_in(&e);
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].request_id, "req_A");
        assert_eq!(approvals[0].prompt, "Commit to slice/PR1?");
        assert_eq!(
            approvals[0].options,
            [
                ApprovalOption {
                    id: "approve".into(),
                    label: "Approve".into()
                },
                ApprovalOption {
                    id: "deny".into(),
                    label: "Deny".into()
                },
            ]
        );
    }

    #[test]
    fn an_ask_question_without_options_is_free_text() {
        let e = event(
            r#"{"type":"input.requested","data":{"requests":[
                 {"requestId":"req_B","question":"Which database?"}]}}"#,
        );
        let approvals = approvals_in(&e);
        assert_eq!(approvals[0].prompt, "Which database?");
        assert!(approvals[0].options.is_empty());
    }

    #[test]
    fn several_requests_in_one_event_all_come_through() {
        // eve batches them, and answering only the first would park the turn forever.
        let e = event(
            r#"{"type":"input.requested","data":{"requests":[
                 {"requestId":"a","prompt":"one"},{"requestId":"b","prompt":"two"}]}}"#,
        );
        assert_eq!(approvals_in(&e).len(), 2);
    }

    #[test]
    fn an_event_that_is_not_a_request_yields_nothing() {
        assert!(approvals_in(&event(r#"{"type":"turn.completed","data":{}}"#)).is_empty());
        // A malformed request without an id is skipped rather than panicking.
        assert!(approvals_in(&event(
            r#"{"type":"input.requested","data":{"requests":[{"prompt":"no id"}]}}"#
        ))
        .is_empty());
    }

    #[test]
    fn info_reads_eves_nested_capability_shape() {
        // The shape a real `GET /eve/v1/info` returned, trimmed.
        let json: serde_json::Value = serde_json::from_str(
            r#"{"tools":{"static":[{"logicalPath":"tools/bash.ts"},
                                   {"logicalPath":"tools/read_file.ts"}],
                        "dynamic":[{"logicalPath":"tools/connection_search.ts"}]},
                "subagents":{"local":[{"logicalPath":"subagents/backend"}]},
                "diagnostics":{"discoveryErrors":0}}"#,
        )
        .unwrap();
        assert_eq!(
            names(&json, "tools", "static"),
            ["tools/bash.ts", "tools/read_file.ts"]
        );
        assert_eq!(names(&json, "subagents", "local"), ["subagents/backend"]);
        // A section that is not there is empty, not an error.
        assert!(names(&json, "channels", "static").is_empty());
    }
}
