//! The MCP configuration routes, over a real socket.
//!
//! Nothing here points at the operator's own Pi definitions: the import route is asked
//! only whether it answers and whether it is authenticated, because what it reads out of
//! a file is settled by the core tests against a fixture whose paths are handed in.

use super::Harness;
use serde_json::json;

#[test]
fn mcp_configuration_is_authenticated_listed_without_values_and_edited_against_a_revision() {
    let (h, _dir) = Harness::with_store();

    for path in ["/api/mcp/servers", "/api/mcp/import"] {
        assert_eq!(h.get_anonymous(path).status, 401, "{path}");
    }
    assert_eq!(h.get("/api/mcp/servers").json(), json!([]));
    // Fixture servers default to no discovery paths, even if the operator has Pi
    // configured. Only production entry points opt into the personal files.
    let preview = h.get("/api/mcp/import").json();
    assert_eq!(preview["candidates"], json!([]));
    assert!(preview["candidates"].is_array());
    assert!(preview["fingerprint"]
        .as_str()
        .is_some_and(|fingerprint| !fingerprint.is_empty()));

    let added = h.post(
        "/api/mcp/servers",
        &json!({
            "name": "chrome-devtools",
            "transport": "stdio",
            "command": "npx",
            "args": ["chrome-devtools-mcp@latest"],
            "env": {"DEVTOOLS_KEY": "secret-value"},
        })
        .to_string(),
    );
    assert_eq!(added.status, 200, "{}", added.body);
    // A server is off until somebody enables it: adding one must not start a command.
    assert_eq!(added.json()["enabled"], json!(false));
    let id = added.json()["id"].as_i64().unwrap();
    let rev = added.json()["rev"].as_i64().unwrap();

    let listed = h.get("/api/mcp/servers");
    assert!(
        !listed.body.contains("secret-value"),
        "a stored value reached the page: {}",
        listed.body
    );
    assert_eq!(listed.json()[0]["env_keys"], json!(["DEVTOOLS_KEY"]));

    // Saving without the values keeps them: the page was never given them to send back.
    let enabled = h.post(
        &format!("/api/mcp/servers/{id}"),
        &json!({"rev": rev, "enabled": true}).to_string(),
    );
    assert_eq!(enabled.status, 200, "{}", enabled.body);
    assert_eq!(enabled.json()["enabled"], json!(true));
    assert_eq!(enabled.json()["env_keys"], json!(["DEVTOOLS_KEY"]));

    // The revision the page was showing has moved, so the same save is refused.
    let stale = h.post(
        &format!("/api/mcp/servers/{id}"),
        &json!({"rev": rev, "enabled": false}).to_string(),
    );
    assert_eq!(stale.status, 400, "{}", stale.body);
    assert_eq!(
        h.post(
            &format!("/api/mcp/servers/{id}/remove"),
            &json!({"rev": rev}).to_string()
        )
        .status,
        400
    );

    let current = enabled.json()["rev"].as_i64().unwrap();
    assert_eq!(
        h.post(
            &format!("/api/mcp/servers/{id}/remove"),
            &json!({"rev": current}).to_string()
        )
        .status,
        200
    );
    assert_eq!(h.get("/api/mcp/servers").json(), json!([]));
}

#[test]
fn the_routes_refuse_what_would_replace_a_generated_definition_or_bring_the_standalone_planner_back(
) {
    let (h, _dir) = Harness::with_store();
    for name in ["ai-team-planner", "ai-planner", "clickup", "figma"] {
        let refused = h.post(
            "/api/mcp/servers",
            &json!({"name": name, "transport": "stdio", "command": "anything"}).to_string(),
        );
        assert_eq!(refused.status, 400, "{name}: {}", refused.body);
    }
    for command in ["aip", "/usr/local/bin/ai-planner", "ait"] {
        let refused = h.post(
            "/api/mcp/servers",
            &json!({"name": "board", "transport": "stdio", "command": command}).to_string(),
        );
        assert_eq!(refused.status, 400, "{command}: {}", refused.body);
    }
    // An adapter setting ai-team does not carry is refused rather than silently ignored.
    // `requestHeadersCommand` runs a program for every request, so a body that smuggled
    // it past a permissive shape would be a command this surface never agreed to run.
    let unknown = h.post(
        "/api/mcp/servers",
        &json!({"name":"thing","transport":"stdio","command":"thing","requestHeadersCommand":"sign"})
            .to_string(),
    );
    assert_eq!(unknown.status, 422, "{}", unknown.body);
    assert_eq!(h.get("/api/mcp/servers").json(), json!([]));
}

#[test]
fn a_projects_scope_is_separate_from_the_app_default() {
    let (h, _dir) = Harness::with_store();
    assert_eq!(
        h.post(
            "/api/mcp/servers",
            &json!({"name":"context7","transport":"stdio","command":"context7-mcp"}).to_string()
        )
        .status,
        200
    );
    let scoped = h.post(
        "/api/mcp/servers",
        &json!({"project_id":1,"name":"context7","transport":"stdio","command":"context7-mcp","enabled":true})
            .to_string(),
    );
    assert_eq!(scoped.status, 200, "{}", scoped.body);

    // The app default alone, and then both - so a page can show why a default is not in
    // force rather than only the winner.
    assert_eq!(
        h.get("/api/mcp/servers").json().as_array().unwrap().len(),
        1
    );
    let both = h.get("/api/mcp/servers?project=1");
    assert_eq!(both.json().as_array().unwrap().len(), 2);

    assert_eq!(h.get("/api/mcp/servers?project=99").status, 400);
}

#[test]
fn an_import_without_the_preview_it_was_approved_against_is_refused() {
    // The route may not accept names alone: that would re-read the operator's files and
    // pin whatever those names point at by then.
    let (h, _dir) = Harness::with_store();
    let nameless = h.post(
        "/api/mcp/import",
        &json!({"project_id": null, "names": ["context7"]}).to_string(),
    );
    assert_eq!(nameless.status, 422, "{}", nameless.body);

    let made_up = h.post(
        "/api/mcp/import",
        &json!({"project_id": null, "names": ["context7"], "fingerprint": "nonsense"}).to_string(),
    );
    assert_eq!(made_up.status, 400, "{}", made_up.body);
    assert_eq!(h.get("/api/mcp/servers").json(), json!([]));
}

#[test]
fn a_destination_change_will_not_take_a_stored_credential_with_it() {
    // The page cannot send back values it was never given, so silence means "keep them" -
    // and keeping them across a change of command would hand the operator's key to
    // whatever is there now. The refusal says the remedy.
    let (h, _dir) = Harness::with_store();
    let added = h.post(
        "/api/mcp/servers",
        &json!({
            "name": "datadog",
            "transport": "stdio",
            "command": "datadog-mcp",
            "env": {"DD_API_KEY": "secret-value"},
        })
        .to_string(),
    );
    assert_eq!(added.status, 200, "{}", added.body);
    let id = added.json()["id"].as_i64().unwrap();
    let rev = added.json()["rev"].as_i64().unwrap();

    let moved = h.post(
        &format!("/api/mcp/servers/{id}"),
        &json!({"rev": rev, "command": "somebody-elses-mcp"}).to_string(),
    );
    assert_eq!(moved.status, 400, "{}", moved.body);
    assert!(moved.body.contains("empty object"), "{}", moved.body);

    // Said explicitly, it saves - and the page still never sees a value.
    let replaced = h.post(
        &format!("/api/mcp/servers/{id}"),
        &json!({"rev": rev, "command": "somebody-elses-mcp", "env": {}}).to_string(),
    );
    assert_eq!(replaced.status, 200, "{}", replaced.body);
    assert_eq!(replaced.json()["env_keys"], json!([]));
    assert!(!replaced.body.contains("secret-value"), "{}", replaced.body);
}

#[test]
fn an_http_server_keeps_its_credential_off_the_page() {
    let (h, _dir) = Harness::with_store();
    let added = h.post(
        "/api/mcp/servers",
        &json!({
            "name": "datadog",
            "transport": "http",
            "url": "https://mcp.example.invalid/api",
            "headers": {"Authorization": "Bearer never-shown"},
        })
        .to_string(),
    );
    assert_eq!(added.status, 200, "{}", added.body);
    assert!(!added.body.contains("never-shown"), "{}", added.body);
    assert_eq!(added.json()["header_keys"], json!(["Authorization"]));
    let listed = h.get("/api/mcp/servers");
    assert!(!listed.body.contains("never-shown"), "{}", listed.body);
}
