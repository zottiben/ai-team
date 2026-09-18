//! Where work comes from: ClickUp tickets, and the Figma frames they point at.
//!
//! Both are **read-only, permanently** (D9). ai-team never writes back: the human answers
//! on those platforms, and an agent that moved a ticket or edited a design would be
//! acting for them somewhere they cannot see it happen.
//!
//! That is enforced rather than asked for. Both MCP servers expose write tools -
//! ClickUp has create/update/delete task, Figma has `use_figma`, which creates, edits and
//! deletes - so the generated connections carry an **allow-list**. An allow-list fails
//! closed: a name that turns out to be wrong loses a capability, where a block-list that
//! misses a name hands over a write tool.

use crate::error::{Error, Result};

/// A ClickUp task or a Figma file, as a URL identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A ClickUp task. The id is what its API and its MCP call a task id.
    ClickUpTask { id: String },
    /// A ClickUp list, folder or space - an "epic" in the everyday sense.
    ClickUpList { id: String },
    /// A Figma file, and the node inside it when the link points at one.
    Figma { key: String, node: Option<String> },
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Source::ClickUpTask { .. } | Source::ClickUpList { .. } => "clickup",
            Source::Figma { .. } => "figma",
        }
    }

    /// What to store in `project.source_key`: enough to find it again, and nothing that
    /// needs a second lookup to interpret.
    pub fn key(&self) -> String {
        match self {
            Source::ClickUpTask { id } => format!("task:{id}"),
            Source::ClickUpList { id } => format!("list:{id}"),
            Source::Figma { key, node: None } => format!("file:{key}"),
            Source::Figma {
                key,
                node: Some(node),
            } => format!("file:{key}#{node}"),
        }
    }
}

/// Read a pasted URL.
///
/// Deliberately strict about what it recognises. A URL it half-understands would create
/// a project pointing at the wrong thing, and a clear refusal costs a retype.
pub fn parse_url(url: &str) -> Result<Source> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.trim_start_matches("www.");

    if host.ends_with("clickup.com") {
        return parse_clickup(path);
    }
    if host.ends_with("figma.com") {
        return parse_figma(path);
    }
    Err(Error::invalid(format!(
        "{trimmed} is not a ClickUp or Figma URL"
    )))
}

fn parse_clickup(path: &str) -> Result<Source> {
    let (path, _query) = path.split_once('?').unwrap_or((path, ""));
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // https://app.clickup.com/t/86abc123  and  .../<team>/t/86abc123
    if let Some(index) = segments.iter().position(|s| *s == "t") {
        if let Some(id) = segments.get(index + 1) {
            return Ok(Source::ClickUpTask {
                id: (*id).to_string(),
            });
        }
    }
    // .../v/l/<id>, .../v/li/<id> - a List; .../v/f/<id> - a Folder.
    if let Some(index) = segments.iter().position(|s| *s == "v") {
        if let Some(id) = segments.get(index + 2) {
            return Ok(Source::ClickUpList {
                id: (*id).to_string(),
            });
        }
    }
    Err(Error::invalid(
        "that ClickUp URL names no task or list - open the task and copy its link",
    ))
}

fn parse_figma(path: &str) -> Result<Source> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // figma.com/{file,design,board,proto,slides}/<key>/<name>
    let key = segments
        .iter()
        .position(|s| matches!(*s, "file" | "design" | "board" | "proto" | "slides"))
        .and_then(|index| segments.get(index + 1))
        .ok_or_else(|| Error::invalid("that Figma URL names no file"))?;

    let node = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("node-id="))
        // Figma writes `1-23` in links and `1:23` in its API; one spelling downstream.
        .map(|node| node.replace('-', ":"))
        .filter(|node| !node.is_empty());

    Ok(Source::Figma {
        key: (*key).to_string(),
        node,
    })
}

/// Every Figma link in a body of text, in the order they appear, without duplicates.
///
/// Used on a ticket's description: the designs a ticket points at are the context the
/// frontend seat needs, and they arrive as links in prose rather than as a field.
pub fn figma_links(text: &str) -> Vec<Source> {
    let mut found: Vec<Source> = Vec::new();
    for token in
        text.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '<' || c == '>')
    {
        // Markdown and prose punctuation cling to the end of a pasted URL.
        let token = token.trim_end_matches([',', '.', ';', ':', ']', '"', '\'']);
        if !token.contains("figma.com") {
            continue;
        }
        if let Ok(source @ Source::Figma { .. }) = parse_url(token) {
            if !found.contains(&source) {
                found.push(source);
            }
        }
    }
    found
}

/// The ClickUp MCP tools a seat may call.
///
/// Read-only by D9. These are the names ClickUp's own MCP documents; an allow-list is
/// used precisely because that list cannot be verified without authenticating, and a
/// wrong name here costs a capability rather than handing over a write.
pub const CLICKUP_READ_TOOLS: &[&str] = &[
    "search_workspace",
    "search_tasks_by_task_type",
    "search_tasks_by_tag",
    "get_task",
    "list_tasks",
    "list_spaces",
    "search_docs",
    "get_doc",
    "list_docs",
    "get_comments",
    "get_workspace_hierarchy",
];

/// The Figma MCP tools a seat may call.
///
/// Taken from Figma's published tool list, and deliberately missing every write: not
/// only the obvious `generate_figma_design` and `upload_assets`, but `use_figma`, which
/// the docs file under writes because it creates, edits and deletes - and the `weave_*`
/// tools, which spend credits.
pub const FIGMA_READ_TOOLS: &[&str] = &[
    "get_design_context",
    "get_metadata",
    "get_screenshot",
    "get_variable_defs",
    "get_code_connect_map",
    "get_libraries",
    "search_design_system",
    "download_assets",
    "get_figjam",
    "get_motion_context",
    "whoami",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_clickup_task_url_is_understood() {
        for url in [
            "https://app.clickup.com/t/86abc123",
            "https://app.clickup.com/9014/t/86abc123",
            "http://app.clickup.com/t/86abc123?comment=1",
            "  https://app.clickup.com/t/86abc123  ",
        ] {
            assert_eq!(
                parse_url(url).unwrap(),
                Source::ClickUpTask {
                    id: "86abc123".into()
                },
                "{url}"
            );
        }
        assert_eq!(
            parse_url("https://app.clickup.com/t/86abc123")
                .unwrap()
                .key(),
            "task:86abc123"
        );
    }

    #[test]
    fn a_clickup_list_url_is_an_epic_in_the_everyday_sense() {
        let list = parse_url("https://app.clickup.com/9014/v/l/901400123456").unwrap();
        assert_eq!(
            list,
            Source::ClickUpList {
                id: "901400123456".into()
            }
        );
        assert_eq!(list.key(), "list:901400123456");
    }

    #[test]
    fn a_figma_link_carries_the_frame_it_points_at() {
        let source =
            parse_url("https://www.figma.com/design/AbC123/Widget?node-id=42-7&t=xyz").unwrap();
        assert_eq!(
            source,
            Source::Figma {
                key: "AbC123".into(),
                // Figma writes 42-7 in a link and 42:7 in its API; one spelling from here.
                node: Some("42:7".into()),
            }
        );
        assert_eq!(source.key(), "file:AbC123#42:7");

        // A file link with no frame is still a file.
        let whole = parse_url("https://figma.com/file/AbC123/Widget").unwrap();
        assert_eq!(
            whole,
            Source::Figma {
                key: "AbC123".into(),
                node: None
            }
        );
        assert_eq!(whole.key(), "file:AbC123");
    }

    #[test]
    fn a_url_that_is_only_half_understood_is_refused() {
        // Creating a project pointing at the wrong thing is worse than a retype.
        for url in [
            "https://app.clickup.com/9014",
            "https://www.figma.com/files/recent",
            "https://github.com/zottiben/ai-team",
            "not a url at all",
            "",
        ] {
            assert!(parse_url(url).is_err(), "{url} should be refused");
        }
    }

    #[test]
    fn figma_links_are_pulled_out_of_the_prose_a_ticket_is_written_in() {
        let description = "\
            Build the console per the design.\n\n\
            Main frame: https://www.figma.com/design/AbC123/Console?node-id=10-2\n\
            Empty state: [here](https://www.figma.com/design/AbC123/Console?node-id=10-99).\n\
            The same frame again: https://www.figma.com/design/AbC123/Console?node-id=10-2\n\
            See also https://clickup.com/t/86zzz for the parent.";

        let links = figma_links(description);
        assert_eq!(
            links,
            [
                Source::Figma {
                    key: "AbC123".into(),
                    node: Some("10:2".into())
                },
                Source::Figma {
                    key: "AbC123".into(),
                    node: Some("10:99".into())
                },
            ],
            "in order, deduplicated, and the ClickUp link is not a Figma one"
        );
    }

    #[test]
    fn no_write_tool_is_on_either_allow_list() {
        // The D9 guarantee, asserted against the names rather than the intent. Both
        // servers grew write tools; `use_figma` is the one that reads like a read.
        for forbidden in [
            "create_task",
            "update_task",
            "delete_task",
            "create_bulk_tasks",
            "set_custom_fields",
        ] {
            assert!(!CLICKUP_READ_TOOLS.contains(&forbidden), "{forbidden}");
        }
        for forbidden in [
            "use_figma",
            "generate_figma_design",
            "generate_diagram",
            "create_new_file",
            "upload_assets",
            "add_code_connect_map",
            "send_code_connect_mappings",
            "create_shader",
            "update_shader",
            "create_generative_plugin",
            "update_generative_plugin",
            "weave_run_tool",
            "weave_upload_asset",
            "weave_cancel_tool_run",
        ] {
            assert!(!FIGMA_READ_TOOLS.contains(&forbidden), "{forbidden}");
        }
    }
}
