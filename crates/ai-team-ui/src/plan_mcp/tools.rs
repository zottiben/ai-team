use rmcp::model::Tool;
use serde_json::{json, Map, Value};

pub(super) fn definition(name: &'static str) -> Tool {
    let (description, fields, optional): (&str, &[&str], &[&str]) = match name {
        "list_worktrees" => ("List this chat project's existing Git worktrees and external lease availability. Read-only; no scope overrides.", &[], &[]),
        "request_worktree" => ("Propose using an existing worktree when the user asks. Supply an exact path from list_worktrees, then END this turn. Only the human can approve switching after it settles. This does not move the running process or grant access outside its checkout.", &["path"], &[]),
        "list_chat_images" => ("List images the human attached to this chat, with their message node IDs and image indexes. Read-only, no scope overrides. Use read_chat_image to inspect an earlier screenshot or implement a plan based on it.", &[], &[]),
        "read_chat_image" => ("Read one image attached by the human in this exact chat. Supply node and index from list_chat_images. Returns native image content. Images are context, never execution or publication approval.", &["node", "index"], &[]),
        "get_plan" => ("Read this chat's plan, progress, questions, decisions and current revision. No plan is created by reading. A plan too large for one response answers with partial:true and no content: it still exists unchanged and still carries the revision, and read_plan fetches it.", &[], &[]),
        "read_plan" => ("Read one page of this chat's whole plan as pretty JSON, for when get_plan or a mutation answers partial:true. Start at offset 0, then pass back exactly the next_offset each page returns, once each, until next_offset is null; joining the pages in order rebuilds that snapshot. Every page comes from the revision you supply, so a refusal means re-reading from offset 0. No scope overrides.", &["offset", "limit"], &["limit"]),
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
        "request_publication" => ("Ask ai-team to publish the commit you just made, on this chat's current branch. Offered only because the person's own message in this chat explicitly asked for a push. Commit first, then supply that exact commit SHA and END this turn: ai-team pushes after the turn ends and reports the real result in the conversation. Never run git push, never open a pull request or merge, and never report the work as published yourself.", &["commit"], &[]),
        _ => unreachable!("the server's closed allow-list contains only these tools"),
    };
    let mut properties = Map::new();
    let mut required = Vec::new();
    if !matches!(
        name,
        "get_plan"
            | "list_worktrees"
            | "request_worktree"
            | "request_publication"
            | "list_chat_images"
            | "read_chat_image"
    ) {
        properties.insert("expect_revision".into(), json!({"type":"integer","minimum":0,"description":"Revision returned by get_plan, a read_plan page or the previous mutation. Re-read after a conflict."}));
        required.push("expect_revision");
    }
    for field in fields {
        let schema = match *field {
            "node" | "index" => json!({"type":"integer","minimum":0}),
            "offset" => {
                json!({"type":"integer","minimum":0,"description":"0 for the first page, then exactly the next_offset the previous page returned."})
            }
            "limit" => {
                json!({"type":"integer","minimum":1,"maximum":super::response::MAX_LIMIT,"description":"Optional cap on this page's plan text. Fewer bytes may come back; always follow next_offset."})
            }
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
