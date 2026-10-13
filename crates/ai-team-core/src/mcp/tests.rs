//! What an app-owned MCP server may be, and what reaches a seat because of it.
//!
//! Every test here drives the store and the composer. None starts a server, runs a
//! command or resolves a credential, and the import tests read a fixture whose paths
//! are handed in rather than the operator's own definitions.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::*;
use crate::model::NewProject;

fn store_with_project() -> (Store, i64) {
    let mut store = Store::memory().unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    (store, project.id)
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn stdio(name: &str, command: &str) -> NewMcpServer {
    NewMcpServer {
        project_id: None,
        name: name.into(),
        enabled: true,
        transport: McpTransport::Stdio,
        command: Some(command.into()),
        args: Vec::new(),
        url: None,
        env: BTreeMap::new(),
        headers: BTreeMap::new(),
        include_tools: None,
    }
}

#[test]
fn aliases_cannot_bypass_the_managed_context_source_policy() {
    let (mut store, _) = store_with_project();
    for source in ["https://mcp.figma.com/mcp", "https://mcp.clickup.com/mcp"] {
        let added = store.add_mcp_server(NewMcpServer {
            transport: McpTransport::Http,
            command: None,
            url: Some(source.into()),
            ..stdio("alias", "unused")
        });
        assert!(
            added.is_err(),
            "a managed source alias must not expose unrestricted tools"
        );
        assert!(store
            .add_mcp_server(NewMcpServer {
                args: vec!["mcp-remote".into(), source.into()],
                ..stdio("proxy-alias", "npx")
            })
            .is_err());
    }
    assert!(store.mcp_servers(None).unwrap().is_empty());
}

#[test]
fn an_enabled_server_reaches_a_seats_generated_config_with_its_tools_in_front_of_the_model() {
    // The complaint this surface answers: a server the operator uses in an ordinary Pi
    // session is absent from a chat, because a chat runs exclusive and inherits nothing.
    let (mut store, project) = store_with_project();
    store
        .add_mcp_server(NewMcpServer {
            args: vec!["chrome-devtools-mcp@latest".into()],
            ..stdio("chrome-devtools", "npx")
        })
        .unwrap();

    let composed = compose(&store, project, false).unwrap();
    let mut config = json!({ "mcpServers": {} });
    apply(&mut config, &composed).unwrap();

    let server = &config["mcpServers"]["chrome-devtools"];
    assert_eq!(server["command"], "npx");
    assert_eq!(server["args"], json!(["chrome-devtools-mcp@latest"]));
    assert_eq!(server["transport"], "stdio");
    // Behind the proxy the model is not shown the tools, decides the server is
    // unavailable and works around it - which is the whole failure being fixed.
    assert_eq!(server["directTools"], json!(true));
}

#[test]
fn a_server_is_off_until_the_operator_enables_it() {
    // Enabling is what starts a local command or reaches an endpoint, so it is never the
    // side effect of adding one.
    let (mut store, project) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            enabled: false,
            ..stdio("file-sql", "file-sql")
        })
        .unwrap();
    assert!(!added.enabled);
    assert!(compose(&store, project, false).unwrap().servers.is_empty());

    let on = store
        .edit_mcp_server(
            added.id,
            McpServerEdit {
                rev: added.rev,
                enabled: Some(true),
                command: None,
                args: None,
                url: None,
                env: None,
                headers: None,
                include_tools: None,
            },
        )
        .unwrap();
    assert!(on.enabled);
    assert_eq!(compose(&store, project, false).unwrap().servers.len(), 1);
}

#[test]
fn a_credential_travels_in_the_turns_environment_not_in_the_file_beside_the_guard() {
    // The same split the context sources use. A value written into the generated config
    // would outlive the turn on disk.
    let (mut store, project) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            project_id: None,
            name: "datadog".into(),
            enabled: true,
            transport: McpTransport::Http,
            command: None,
            args: Vec::new(),
            url: Some("https://mcp.example.invalid/api".into()),
            env: BTreeMap::new(),
            headers: values(&[("Authorization", "Bearer super-secret-value")]),
            include_tools: None,
        })
        .unwrap();

    let composed = compose(&store, project, false).unwrap();
    let text = serde_json::to_string(&composed.servers).unwrap();
    assert!(
        !text.contains("super-secret-value"),
        "a credential reached the generated config: {text}"
    );
    let name = placeholder(added.id, "HDR", "Authorization");
    assert!(text.contains(&format!("${{{name}}}")), "{text}");
    assert!(composed
        .environment
        .contains(&(name, "Bearer super-secret-value".to_string())));
}

#[test]
fn no_surface_is_ever_handed_a_stored_value() {
    // Keys say what a server is configured with, which is what a decision needs. The
    // values are the part that may be a credential.
    let (mut store, _) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            env: values(&[("DD_API_KEY", "secret-key")]),
            ..stdio("datadog", "datadog-mcp")
        })
        .unwrap();
    assert_eq!(added.env_keys, vec!["DD_API_KEY".to_string()]);
    let listed = serde_json::to_string(&store.mcp_servers(None).unwrap()).unwrap();
    assert!(!listed.contains("secret-key"), "{listed}");
    assert!(listed.contains("DD_API_KEY"), "{listed}");
}

#[test]
fn an_omitted_replacement_retains_the_stored_values() {
    // The page was never given them, so it cannot send them back; treating silence as
    // "clear it" would empty a credential every time somebody ticked a box.
    let (mut store, project) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            env: values(&[("DD_API_KEY", "secret-key")]),
            ..stdio("datadog", "datadog-mcp")
        })
        .unwrap();
    let edited = store
        .edit_mcp_server(
            added.id,
            McpServerEdit {
                rev: added.rev,
                enabled: Some(true),
                command: None,
                args: None,
                url: None,
                env: None,
                headers: None,
                include_tools: Some(vec!["query_metrics".into()]),
            },
        )
        .unwrap();
    assert_eq!(edited.env_keys, vec!["DD_API_KEY".to_string()]);
    let composed = compose(&store, project, false).unwrap();
    assert!(composed
        .environment
        .iter()
        .any(|(_, value)| value == "secret-key"));

    let replaced = store
        .edit_mcp_server(
            edited.id,
            McpServerEdit {
                rev: edited.rev,
                enabled: None,
                command: None,
                args: None,
                url: None,
                env: Some(values(&[("DD_SITE", "example.invalid")])),
                headers: None,
                include_tools: None,
            },
        )
        .unwrap();
    assert_eq!(replaced.env_keys, vec!["DD_SITE".to_string()]);
    let composed = compose(&store, project, false).unwrap();
    assert!(!composed
        .environment
        .iter()
        .any(|(_, value)| value == "secret-key"));
}

#[test]
fn an_edit_against_an_old_revision_is_refused() {
    let (mut store, _) = store_with_project();
    let added = store
        .add_mcp_server(stdio("context7", "context7-mcp"))
        .unwrap();
    let edit = |rev: i64| McpServerEdit {
        rev,
        enabled: Some(false),
        command: None,
        args: None,
        url: None,
        env: None,
        headers: None,
        include_tools: None,
    };
    let moved = store.edit_mcp_server(added.id, edit(added.rev)).unwrap();
    assert!(moved.rev > added.rev);
    let stale = store.edit_mcp_server(added.id, edit(added.rev));
    assert!(stale.is_err(), "a stale edit overwrote a newer one");
    assert!(store.remove_mcp_server(added.id, added.rev).is_err());
    assert!(store.remove_mcp_server(moved.id, moved.rev).is_ok());
}

#[test]
fn the_generated_servers_names_cannot_be_taken() {
    // ai-team's planner, the standalone one, and the two read-only context sources whose
    // allow-lists are D15. A row that could take one of these could widen a seat's reach.
    let (mut store, _) = store_with_project();
    for name in RESERVED {
        let refused = store.add_mcp_server(stdio(name, "anything"));
        assert!(refused.is_err(), "{name} was accepted");
    }
}

#[test]
fn a_standalone_planner_cannot_be_configured_as_a_server() {
    // Under any name: the refusal is on the program, because the name is the operator's
    // to choose and `aip serve` beside the scoped tools is the standalone planner back.
    let (mut store, _) = store_with_project();
    for command in ["aip", "/usr/local/bin/ai-planner", "ait", "AI-Team"] {
        let refused = store.add_mcp_server(stdio("board", command));
        assert!(refused.is_err(), "{command} was accepted");
    }
}

#[test]
fn a_reserved_definition_in_the_table_is_still_dropped_at_dispatch() {
    // Defence in depth: this is the last place before a row becomes a capability, so it
    // does not trust that the table refused what the table refuses.
    let composed = build(
        vec![Definition {
            id: 1,
            name: "clickup".into(),
            enabled: true,
            transport: McpTransport::Http,
            command: None,
            args: Vec::new(),
            url: Some("https://wrong.invalid".into()),
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            include_tools: None,
        }],
        false,
    );
    assert!(composed.servers.is_empty());
}

#[test]
fn a_planning_seat_is_still_refused_a_browser() {
    // Rule 10's companion: a coordinator whose required context failed says so rather
    // than opening a browser and guessing. Matched on what it runs, not only its name.
    let (mut store, project) = store_with_project();
    store
        .add_mcp_server(NewMcpServer {
            args: vec!["chrome-devtools-mcp@latest".into()],
            ..stdio("devtools", "npx")
        })
        .unwrap();
    store
        .add_mcp_server(stdio("context7", "context7-mcp"))
        .unwrap();

    let planning = compose(&store, project, true).unwrap();
    assert_eq!(
        planning
            .servers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["context7"]
    );
    let building = compose(&store, project, false).unwrap();
    assert_eq!(building.servers.len(), 2);
}

#[test]
fn a_projects_own_row_replaces_the_app_default_including_switching_it_off() {
    let (mut store, project) = store_with_project();
    let other = store
        .create_project(NewProject {
            name: "Other".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    store
        .add_mcp_server(stdio("context7", "context7-mcp"))
        .unwrap();
    store
        .add_mcp_server(NewMcpServer {
            project_id: Some(project),
            enabled: false,
            ..stdio("context7", "context7-mcp")
        })
        .unwrap();

    assert!(compose(&store, project, false).unwrap().servers.is_empty());
    assert_eq!(compose(&store, other, false).unwrap().servers.len(), 1);
}

#[test]
fn a_value_that_would_be_resolved_or_run_elsewhere_is_refused() {
    // A leading `!` is a command the adapter runs; `${VAR}` is a reference it resolves,
    // and one that cannot survive the placeholder indirection either.
    let (mut store, _) = store_with_project();
    for value in ["!op read secret", "${DD_API_KEY}", "$env:DD_API_KEY"] {
        let refused = store.add_mcp_server(NewMcpServer {
            env: values(&[("DD_API_KEY", value)]),
            ..stdio("datadog", "datadog-mcp")
        });
        assert!(refused.is_err(), "{value} was accepted");
    }
}

#[test]
fn two_keys_that_cannot_be_told_apart_are_refused_rather_than_silently_merged() {
    // Their placeholders would collide, and one value would quietly become the other's.
    let (mut store, _) = store_with_project();
    let refused = store.add_mcp_server(NewMcpServer {
        env: values(&[("API-KEY", "one"), ("API_KEY", "two")]),
        ..stdio("thing", "thing-mcp")
    });
    assert!(refused.is_err());
}

#[test]
fn an_allow_list_filters_what_the_seat_is_offered_and_an_empty_one_means_everything() {
    let (mut store, project) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            include_tools: Some(vec!["navigate_page".into(), "take_snapshot".into()]),
            ..stdio("devtools", "devtools-mcp")
        })
        .unwrap();
    let composed = compose(&store, project, false).unwrap();
    assert_eq!(
        composed.servers[0].1["includeTools"],
        json!(["navigate_page", "take_snapshot"])
    );

    let cleared = store
        .edit_mcp_server(
            added.id,
            McpServerEdit {
                rev: added.rev,
                enabled: None,
                command: None,
                args: None,
                url: None,
                env: None,
                headers: None,
                include_tools: Some(Vec::new()),
            },
        )
        .unwrap();
    assert!(cleared.include_tools.is_none());
    let composed = compose(&store, project, false).unwrap();
    assert!(composed.servers[0].1.get("includeTools").is_none());
}

/// A Pi agent directory of our own, so reading the operator's definitions is reading a
/// fixture. Returned as the source list rather than installed into the environment: this
/// crate forbids `unsafe`, and `set_var` is unsafe for the good reason that a sibling
/// test would see it.
fn pi_agent_dir(body: &str) -> (tempfile::TempDir, Vec<std::path::PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mcp-adapter.json"), body).unwrap();
    let sources = vec![
        dir.path().join("mcp-adapter.json"),
        dir.path().join("mcp.json"),
    ];
    (dir, sources)
}

/// The fingerprint a preview of these sources answers with - which is what a surface
/// that has just shown the list holds, and what an import has to quote back.
fn approved(sources: &[std::path::PathBuf], store: &Store, project: Option<i64>) -> String {
    preview_in(sources, store, project).unwrap().fingerprint
}

#[test]
fn the_operators_own_pi_definitions_are_where_pi_keeps_them() {
    // Named rather than guessed, and read-only: these two files are the whole source.
    let sources = user_sources().unwrap();
    let names: Vec<String> = sources
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["mcp-adapter.json", "mcp.json"]);
    assert!(sources.iter().all(|path| path.is_absolute()));
    assert_eq!(sources[0].parent(), sources[1].parent());
}

#[test]
fn importing_is_a_pinned_copy_and_never_ongoing_inheritance() {
    let (dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{
             "chrome-devtools":{"command":"npx","args":["chrome-devtools-mcp@latest"]},
             "datadog":{"url":"https://mcp.example.invalid","headers":{"Authorization":"Bearer pinned"}}
           }}"#,
    );
    let (mut store, project) = store_with_project();

    let preview = preview_in(&sources, &store, None).unwrap();
    let candidates = &preview.candidates;
    assert!(candidates.iter().all(|candidate| candidate.importable));
    // The listing shows the keys, never the values.
    let text = serde_json::to_string(&preview).unwrap();
    assert!(!text.contains("Bearer pinned"), "{text}");
    assert!(text.contains("Authorization"), "{text}");
    let imported = import_from(
        &sources,
        &mut store,
        None,
        &["chrome-devtools".into()],
        &preview.fingerprint,
    )
    .unwrap();
    assert_eq!(imported.len(), 1);
    // Reviewed, then enabled. Importing is not agreeing to run it.
    assert!(!imported[0].enabled);
    assert_eq!(imported[0].origin, "import");
    assert!(imported[0].origin_detail.contains("mcp-adapter.json"));

    // Only what was chosen, and nothing follows the source afterwards.
    std::fs::write(
        dir.path().join("mcp-adapter.json"),
        r#"{"mcpServers":{"something-else":{"command":"else"}}}"#,
    )
    .unwrap();
    let names: Vec<String> = store
        .mcp_servers(None)
        .unwrap()
        .into_iter()
        .map(|server| server.name)
        .collect();
    assert_eq!(names, ["chrome-devtools".to_string()]);
    assert!(compose(&store, project, false).unwrap().servers.is_empty());
}

#[test]
fn a_definition_ai_team_cannot_carry_across_says_so_instead_of_arriving_changed() {
    let (_dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{
             "signed":{"url":"https://x.invalid","requestHeadersCommand":{"command":"sign"}},
             "clickup":{"url":"https://mcp.clickup.com/mcp"},
             "socketed":{"socket":"/tmp/mux.sock"},
             "blocked":{"command":"thing","excludeTools":["danger"]}
           }}"#,
    );
    let (mut store, _) = store_with_project();
    let candidates = candidates_in(&sources, &store, None).unwrap();
    assert_eq!(candidates.len(), 4);
    for candidate in &candidates {
        assert!(!candidate.importable, "{} was offered", candidate.name);
        assert!(
            !candidate.note.is_empty(),
            "{} said nothing",
            candidate.name
        );
    }
    let pinned = approved(&sources, &store, None);
    let refused = import_from(&sources, &mut store, None, &["clickup".into()], &pinned);
    assert!(refused.is_err(), "a reserved name was imported");
}

#[test]
fn an_already_configured_name_is_reported_and_not_overwritten() {
    let (_dir, sources) = pi_agent_dir(r#"{"mcpServers":{"context7":{"command":"context7-mcp"}}}"#);
    let (mut store, _) = store_with_project();
    store
        .add_mcp_server(NewMcpServer {
            args: vec!["--mine".into()],
            ..stdio("context7", "context7-mcp")
        })
        .unwrap();
    let candidates = candidates_in(&sources, &store, None).unwrap();
    assert!(candidates[0].already);
    let pinned = approved(&sources, &store, None);
    assert!(import_from(&sources, &mut store, None, &["context7".into()], &pinned).is_err());
    let kept = store.mcp_servers(None).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].args, ["--mine"]);
}

#[test]
fn nothing_about_reading_the_operators_definitions_runs_a_command() {
    // A GET that executed something would be a shell on loopback by another name. The
    // fixture names a program that does not exist; reading it must still succeed.
    let (_dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{"thing":{"command":"/definitely/not/here/binary","args":["--go"]}}}"#,
    );
    let (store, _) = store_with_project();
    let candidates = candidates_in(&sources, &store, None).unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].importable);
}

#[test]
fn an_unreadable_or_malformed_pi_file_is_not_an_error() {
    // It is the operator's file, not ours. It may be absent, half-written, or something
    // else entirely, and none of that should stop the page loading.
    let dir = tempfile::tempdir().unwrap();
    let sources = vec![dir.path().join("mcp-adapter.json")];
    let (store, _) = store_with_project();
    assert!(candidates_in(&sources, &store, None).unwrap().is_empty());
    std::fs::write(dir.path().join("mcp-adapter.json"), "not json").unwrap();
    assert!(candidates_in(&sources, &store, None).unwrap().is_empty());
    std::fs::write(dir.path().join("mcp-adapter.json"), r#"{"servers":{}}"#).unwrap();
    assert!(candidates_in(&sources, &store, None).unwrap().is_empty());
}

#[test]
fn a_generated_config_is_readable_only_by_its_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    write_private(&path, "{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
}

#[test]
fn pointing_a_server_somewhere_else_will_not_carry_its_stored_credential_across() {
    // The destination is what a credential was given to. An edit that moves the command,
    // its arguments or the URL while `env` is omitted would hand the stored value to
    // whatever is there now - so it is refused until the save says what the values are.
    let (mut store, _) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            env: values(&[("DD_API_KEY", "secret-key")]),
            ..stdio("datadog", "datadog-mcp")
        })
        .unwrap();
    let edit = |rev: i64, command: &str, env: Option<BTreeMap<String, String>>| McpServerEdit {
        rev,
        enabled: None,
        command: Some(command.into()),
        args: None,
        url: None,
        env,
        headers: None,
        include_tools: None,
    };

    let moved = store.edit_mcp_server(added.id, edit(added.rev, "somebody-elses-mcp", None));
    assert!(
        moved.is_err(),
        "a stored credential followed the command somewhere else"
    );
    let said = moved.unwrap_err().to_string();
    assert!(
        said.contains("empty object") || said.contains("replacement"),
        "the refusal did not name the remedy: {said}"
    );

    // The same change with the values said is allowed, and so is clearing them.
    let replaced = store
        .edit_mcp_server(
            added.id,
            edit(
                added.rev,
                "somebody-elses-mcp",
                Some(values(&[("OTHER_KEY", "fresh")])),
            ),
        )
        .unwrap();
    assert_eq!(replaced.env_keys, vec!["OTHER_KEY".to_string()]);
    let cleared = store
        .edit_mcp_server(
            replaced.id,
            edit(replaced.rev, "third-mcp", Some(BTreeMap::new())),
        )
        .unwrap();
    assert!(cleared.env_keys.is_empty());

    // Arguments are part of the destination: the same program pointed at another endpoint
    // is another endpoint.
    let keyed = store
        .edit_mcp_server(
            cleared.id,
            McpServerEdit {
                rev: cleared.rev,
                enabled: None,
                command: None,
                args: None,
                url: None,
                env: Some(values(&[("DD_API_KEY", "secret-key")])),
                headers: None,
                include_tools: None,
            },
        )
        .unwrap();
    assert!(store
        .edit_mcp_server(
            keyed.id,
            McpServerEdit {
                rev: keyed.rev,
                enabled: None,
                command: None,
                args: Some(vec!["--to".into(), "https://elsewhere.invalid".into()]),
                url: None,
                env: None,
                headers: None,
                include_tools: None,
            },
        )
        .is_err());
}

#[test]
fn an_http_destination_change_will_not_carry_its_stored_header_across() {
    let (mut store, _) = store_with_project();
    let added = store
        .add_mcp_server(NewMcpServer {
            project_id: None,
            name: "datadog".into(),
            enabled: true,
            transport: McpTransport::Http,
            command: None,
            args: Vec::new(),
            url: Some("https://mcp.example.invalid/api".into()),
            env: BTreeMap::new(),
            headers: values(&[("Authorization", "Bearer secret")]),
            include_tools: None,
        })
        .unwrap();
    let moved = store.edit_mcp_server(
        added.id,
        McpServerEdit {
            rev: added.rev,
            enabled: None,
            command: None,
            args: None,
            url: Some("https://attacker.invalid/api".into()),
            env: None,
            headers: None,
            include_tools: None,
        },
    );
    assert!(moved.is_err(), "a bearer token followed the URL elsewhere");

    // Everything that is not the destination still saves without resending the values.
    let on = store
        .edit_mcp_server(
            added.id,
            McpServerEdit {
                rev: added.rev,
                enabled: Some(false),
                command: None,
                args: None,
                url: None,
                env: None,
                headers: None,
                include_tools: Some(vec!["query_metrics".into()]),
            },
        )
        .unwrap();
    assert_eq!(on.header_keys, vec!["Authorization".to_string()]);
}

#[test]
fn a_switched_off_row_suppresses_the_checkouts_server_of_the_same_name() {
    // The checkout's own `.mcp.json` is merged in before these rows are applied, so a row
    // that only declines to add itself leaves the repository's server of that name
    // reachable - and switching a server off on this page has to mean the seat does not
    // get one.
    let (mut store, project) = store_with_project();
    store
        .add_mcp_server(NewMcpServer {
            enabled: false,
            ..stdio("context7", "context7-mcp")
        })
        .unwrap();

    let composed = compose(&store, project, false).unwrap();
    let mut config =
        json!({"mcpServers":{"context7":{"command":"repo-context7","transport":"stdio"}}});
    apply(&mut config, &composed).unwrap();
    assert!(
        config["mcpServers"].get("context7").is_none(),
        "a switched-off server was still reachable through the checkout: {config}"
    );

    // And the generated definitions are never what a row takes down with it.
    let mut generated = json!({"mcpServers":{"clickup":{"url":"https://mcp.clickup.com/mcp"}}});
    apply(&mut generated, &composed).unwrap();
    assert!(
        generated["mcpServers"]["clickup"].is_object(),
        "{generated}"
    );
}

#[test]
fn a_projects_switch_off_suppresses_the_checkouts_server_for_that_project_only() {
    let (mut store, project) = store_with_project();
    let other = store
        .create_project(NewProject {
            name: "Other".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    store
        .add_mcp_server(stdio("context7", "context7-mcp"))
        .unwrap();
    store
        .add_mcp_server(NewMcpServer {
            project_id: Some(project),
            enabled: false,
            ..stdio("context7", "context7-mcp")
        })
        .unwrap();

    let repo = || json!({"mcpServers":{"context7":{"command":"repo-context7"}}});
    let mut off = repo();
    apply(&mut off, &compose(&store, project, false).unwrap()).unwrap();
    assert!(off["mcpServers"].get("context7").is_none(), "{off}");

    let mut on = repo();
    apply(&mut on, &compose(&store, other, false).unwrap()).unwrap();
    assert_eq!(on["mcpServers"]["context7"]["command"], "context7-mcp");
}

#[test]
fn an_import_candidate_whose_shape_is_not_understood_is_refused_rather_than_trimmed() {
    // Every one of these would arrive as a server that looks like the operator's and
    // behaves differently: a filtered argument list, a dropped credential, a working
    // directory nobody carried across, or an allow-list that widened to every tool.
    let (_dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{
             "numbered":{"command":"thing","args":["--go",7]},
             "nested":{"command":"thing","env":{"DD_API_KEY":{"from":"keychain"}}},
             "elsewhere":{"command":"thing","cwd":"/opt/other"},
             "tokened":{"url":"https://x.invalid","auth":"oauth"},
             "notools":{"command":"thing","includeTools":[]},
             "sse":{"url":"https://x.invalid","type":"sse"},
             "unknown":{"command":"thing","alwaysAllow":["rm"]}
           }}"#,
    );
    let (store, _) = store_with_project();
    let candidates = candidates_in(&sources, &store, None).unwrap();
    assert_eq!(candidates.len(), 7);
    for candidate in &candidates {
        assert!(
            !candidate.importable,
            "{} was offered despite {}",
            candidate.name, candidate.note
        );
        assert!(
            !candidate.note.is_empty(),
            "{} said nothing",
            candidate.name
        );
    }
    let named = |name: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.name == name)
            .unwrap()
            .note
            .clone()
    };
    assert!(named("elsewhere").contains("cwd"), "{}", named("elsewhere"));
    assert!(named("tokened").contains("auth"), "{}", named("tokened"));
    assert!(
        named("unknown").contains("alwaysAllow"),
        "{}",
        named("unknown")
    );
    assert!(
        named("notools").contains("every tool"),
        "{}",
        named("notools")
    );
}

#[test]
fn an_allow_list_is_carried_exactly_or_the_candidate_is_not_offered() {
    // `check_tools` drops blanks and duplicates, which is right for somebody typing into
    // the page and wrong for a copy: a copy that quietly widened a list would be a server
    // offering tools the operator's own configuration does not.
    let (_dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{
             "exact":{"command":"thing","includeTools":["navigate_page","take_snapshot"]},
             "padded":{"command":"thing","includeTools":["navigate_page"," "]},
             "repeated":{"command":"thing","includeTools":["a","a"]}
           }}"#,
    );
    let (store, _) = store_with_project();
    let candidates = candidates_in(&sources, &store, None).unwrap();
    let named = |name: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.name == name)
            .unwrap()
    };
    assert!(named("exact").importable);
    assert_eq!(
        named("exact").include_tools,
        Some(vec!["navigate_page".into(), "take_snapshot".into()])
    );
    assert!(!named("padded").importable, "{}", named("padded").note);
    assert!(!named("repeated").importable, "{}", named("repeated").note);
}

#[test]
fn a_preview_does_not_put_a_credential_in_a_url_on_the_page() {
    // A URL is shown so somebody can check by eye what a server reaches, and a URL is
    // also somewhere a credential is written. Such definitions are redacted for
    // review and refused: accepting them would leak tokens in later settings reads.
    let (_dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{
             "inline":{"url":"https://me:hunter2@mcp.example.invalid/api?api_key=sk-live-999&page=2"},
             "proxied":{"command":"npx","args":["mcp-remote","https://me:hunter2@x.invalid/?token=sk-live-999"]},
             "ordinary":{"command":"npx","args":["chrome-devtools-mcp@latest","--headless"]}
           }}"#,
    );
    let (mut store, _) = store_with_project();
    let candidates = candidates_in(&sources, &store, None).unwrap();
    let shown = serde_json::to_string(&candidates).unwrap();
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(!shown.contains("sk-live-999"), "{shown}");
    // Still recognisable: the host and the harmless parameters survive.
    assert!(shown.contains("mcp.example.invalid"), "{shown}");
    assert!(shown.contains("page=2"), "{shown}");
    // And an ordinary command is left alone. `pkg@latest` is a version, not a login, and
    // a destination shown as `***@latest` is one nobody can check by eye.
    let ordinary = candidates
        .iter()
        .find(|candidate| candidate.name == "ordinary")
        .unwrap();
    assert_eq!(ordinary.args, ["chrome-devtools-mcp@latest", "--headless"]);

    for name in ["inline", "proxied"] {
        assert!(
            !candidates
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .importable
        );
        let pinned = approved(&sources, &store, None);
        assert!(import_from(&sources, &mut store, None, &[name.into()], &pinned).is_err());
    }
    assert!(store.mcp_servers(None).unwrap().is_empty());
}

#[test]
fn an_import_pins_the_list_that_was_approved_rather_than_whatever_is_there_now() {
    // Choosing by name alone means the import re-reads the file, so what arrives is
    // whatever that name points at by then - and the person who approved it saw the
    // definition it used to be.
    let (dir, sources) = pi_agent_dir(r#"{"mcpServers":{"context7":{"command":"context7-mcp"}}}"#);
    let (mut store, _) = store_with_project();
    let preview = preview_in(&sources, &store, None).unwrap();
    assert!(!preview.fingerprint.is_empty());

    std::fs::write(
        dir.path().join("mcp-adapter.json"),
        r#"{"mcpServers":{"context7":{"command":"something-else"}}}"#,
    )
    .unwrap();
    let stale = import_from(
        &sources,
        &mut store,
        None,
        &["context7".into()],
        &preview.fingerprint,
    );
    assert!(stale.is_err(), "a changed definition was imported anyway");
    assert!(
        stale.unwrap_err().to_string().contains("look again"),
        "the refusal did not say what to do"
    );
    assert!(store.mcp_servers(None).unwrap().is_empty());
    // No fingerprint at all is not a way past it either.
    assert!(import_from(&sources, &mut store, None, &["context7".into()], "").is_err());

    // The list as it stands now imports, because it is the list that would be shown.
    let now = approved(&sources, &store, None);
    let imported = import_from(&sources, &mut store, None, &["context7".into()], &now).unwrap();
    assert_eq!(imported[0].command.as_deref(), Some("something-else"));
}

#[test]
fn a_rotated_credential_behind_an_unchanged_name_makes_the_preview_stale() {
    // The value is the part a surface never sees, so a change to it is exactly the change
    // nobody reviewing the list could have noticed.
    let write = |dir: &std::path::Path, token: &str| {
        std::fs::write(
            dir.join("mcp-adapter.json"),
            format!(r#"{{"mcpServers":{{"dd":{{"url":"https://x.invalid","headers":{{"Authorization":"{token}"}}}}}}}}"#),
        )
        .unwrap();
    };
    let (dir, sources) = pi_agent_dir("{}");
    write(dir.path(), "Bearer first");
    let (mut store, _) = store_with_project();
    let pinned = approved(&sources, &store, None);
    write(dir.path(), "Bearer second");
    assert!(import_from(&sources, &mut store, None, &["dd".into()], &pinned).is_err());
}

#[test]
fn a_preview_of_one_scope_does_not_approve_an_import_into_another() {
    let (_dir, sources) = pi_agent_dir(r#"{"mcpServers":{"context7":{"command":"context7-mcp"}}}"#);
    let (mut store, project) = store_with_project();
    let app = approved(&sources, &store, None);
    let scoped = approved(&sources, &store, Some(project));
    assert_ne!(app, scoped);
    assert!(import_from(
        &sources,
        &mut store,
        Some(project),
        &["context7".into()],
        &app
    )
    .is_err());
    assert!(import_from(&sources, &mut store, None, &["context7".into()], &scoped).is_err());
    assert!(store.mcp_servers(Some(project)).unwrap().is_empty());
}

#[test]
fn a_batch_with_one_bad_name_in_it_imports_nothing() {
    let (_dir, sources) = pi_agent_dir(
        r#"{"mcpServers":{
             "good":{"command":"good-mcp"},
             "alsogood":{"command":"also-mcp"},
             "socketed":{"socket":"/tmp/mux.sock"}
           }}"#,
    );
    let (mut store, _) = store_with_project();
    let pinned = approved(&sources, &store, None);
    let refused = import_from(
        &sources,
        &mut store,
        None,
        &["good".into(), "socketed".into(), "alsogood".into()],
        &pinned,
    );
    assert!(refused.is_err());
    assert!(
        store.mcp_servers(None).unwrap().is_empty(),
        "a refused batch left rows behind"
    );

    let imported = import_from(
        &sources,
        &mut store,
        None,
        &["good".into(), "alsogood".into()],
        &pinned,
    )
    .unwrap();
    assert_eq!(imported.len(), 2);
    assert!(imported.iter().all(|server| !server.enabled));
}

#[test]
fn importing_reads_the_operators_configuration_and_writes_nothing_to_it() {
    // It is their file, and ai-team is a reader of it. Nothing here synchronises: a copy
    // is taken, and the file is exactly as it was afterwards.
    let (dir, sources) = pi_agent_dir(r#"{"mcpServers":{"context7":{"command":"context7-mcp"}}}"#);
    let before: Vec<Vec<u8>> = sources
        .iter()
        .map(|path| std::fs::read(path).unwrap_or_default())
        .collect();
    let (mut store, _) = store_with_project();
    let pinned = approved(&sources, &store, None);
    import_from(&sources, &mut store, None, &["context7".into()], &pinned).unwrap();
    let after: Vec<Vec<u8>> = sources
        .iter()
        .map(|path| std::fs::read(path).unwrap_or_default())
        .collect();
    assert_eq!(before, after, "an import wrote to the operator's own files");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_source_that_is_not_a_readable_configuration_is_reported_rather_than_skipped() {
    // Silently showing an empty list reports "you have no servers", which is a different
    // statement from "AI Team did not read this file".
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-adapter.json");
    let sources = vec![path.clone()];
    let (store, _) = store_with_project();

    // Absent is the ordinary case and says nothing.
    assert!(preview_in(&sources, &store, None)
        .unwrap()
        .problems
        .is_empty());

    std::fs::write(&path, "not json").unwrap();
    let preview = preview_in(&sources, &store, None).unwrap();
    assert!(preview.candidates.is_empty());
    assert_eq!(preview.problems.len(), 1);
    assert!(
        preview.problems[0].contains("not JSON"),
        "{:?}",
        preview.problems
    );

    // A directory stands in for the things reading would not return from. What matters is
    // that a source which is not a regular file is named rather than read.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let preview = preview_in(&sources, &store, None).unwrap();
    assert_eq!(preview.problems.len(), 1);
    assert!(
        preview.problems[0].contains("not a regular file"),
        "{:?}",
        preview.problems
    );
}

#[test]
fn applying_needs_a_server_map_and_leaves_what_is_already_there() {
    let composed = build(
        vec![Definition {
            id: 7,
            name: "context7".into(),
            enabled: true,
            transport: McpTransport::Stdio,
            command: Some("context7-mcp".into()),
            args: Vec::new(),
            url: None,
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            include_tools: None,
        }],
        false,
    );
    let mut config: Value = json!({"mcpServers":{"ai-team-planner":{"command":"ait"}}});
    apply(&mut config, &composed).unwrap();
    assert!(config["mcpServers"]["ai-team-planner"].is_object());
    assert!(config["mcpServers"]["context7"].is_object());
    assert!(apply(&mut json!({}), &composed).is_err());
}
