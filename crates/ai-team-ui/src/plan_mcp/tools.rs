use rmcp::model::Tool;
use serde_json::{json, Map, Value};

pub(super) fn definition(name: &'static str) -> Tool {
    let (description, fields, optional): (&str, &[&str], &[&str]) = match name {
        "list_worktrees" => ("List this chat project's existing Git worktrees and external lease availability. Read-only; no scope overrides.", &[], &[]),
        "request_worktree" => ("Propose using an existing worktree when the user asks. Supply an exact path from list_worktrees, then END this turn. Only the human can approve switching after it settles. This does not move the running process or grant access outside its checkout.", &["path"], &[]),
        "get_plan" => ("Read this chat's plan, progress, questions, decisions and current revision. No plan is created by reading.", &[], &[]),
        "create_plan" => ("Create a plan for this chat when the user needs one; ordinary conversations need no plan.", &["title", "summary"], &["summary"]),
        "set_plan_status" => ("Update the status of this chat's plan. A planning status is not a verification verdict.", &["status"], &[]),
        "write_section" => ("Write a plan section, preserving concurrent edits through expect_revision.", &["key", "title", "body"], &[]),
        "add_slice" => ("Add a bounded work slice with explicit repository paths and verification criteria.", &["key", "title", "scope", "touches", "demo"], &[]),
        "update_slice" => ("Revise an existing slice's title, scope, touched paths and verification criteria.", &["key", "title", "scope", "touches", "demo"], &[]),
        "set_slice_status" => ("Update a slice's reported progress; blocked requires a reason. Makers may update only their own slice.", &["key", "status", "reason"], &["reason"]),
        "add_decision" => ("Record a design choice and its reasoning in this chat's plan.", &["title", "body"], &[]),
        "open_question" => ("Ask for a human decision. The user answers it in this chat's Overview. Do not answer it yourself.", &["body", "slice"], &["slice"]),
        "append_log" => ("Append progress or verification evidence without rewriting earlier notes.", &["body", "slice"], &["slice"]),
        "add_gotcha" => ("Record a durable planning gotcha for this chat.", &["title", "body"], &[]),
        _ => unreachable!("the server's closed allow-list contains only these tools"),
    };
    let mut properties = Map::new();
    let mut required = Vec::new();
    if !matches!(name, "get_plan" | "list_worktrees" | "request_worktree") {
        properties.insert("expect_revision".into(), json!({"type":"integer","minimum":0,"description":"Revision returned by get_plan or the previous mutation. Re-read after a conflict."}));
        required.push("expect_revision");
    }
    for field in fields {
        let schema = match *field {
            "touches" => {
                json!({"type":"array","items":{"type":"string"},"minItems":1,"description":"Repository-relative paths/globs, for example src/**. No absolute or parent paths."})
            }
            "status" => {
                json!({"type":"string","enum":["draft","ready","active","in_review","blocked","done","deferred"]})
            }
            _ => json!({"type":"string"}),
        };
        properties.insert((*field).into(), schema);
        if !optional.contains(field) {
            required.push(field);
        }
    }
    let schema: Map<String, Value> = serde_json::from_value(json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})).expect("a schema object");
    Tool::new(name, description.to_string(), schema)
}
