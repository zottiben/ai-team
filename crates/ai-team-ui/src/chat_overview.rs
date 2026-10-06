//! One chat's Overview: the repository it works in, the seats working in it, and the
//! files those seats have actually touched.
//!
//! Chat-exact by construction. Every row here is reached from one chat id - its turns,
//! their nodes, their events - so there is no path by which the project's latest run can
//! be rendered under another chat's heading. Nothing here reads the board, the crew or a
//! run list.
//!
//! Two routes rather than one, because the two halves move at different speeds. Walking a
//! checkout is a few hundred `read_dir` calls, and the shape of a repository does not
//! change because a token arrived, so the window reads the map once per chat. The live
//! half is SQL and is re-read on every tick - which is what makes the picture move.
//!
//! What a seat touched is read from its own stream, structurally: Pi's path tools name
//! their argument `path` (`pi/assets/guard.ts` holds the same four tools to the lease),
//! so the evidence is `payload.args.path` on that seat's `tool_call` rows. Never the
//! rendered summary - a display string parsed back into data is the mistake rule 8
//! records about verdicts, in a different column.

use std::collections::BTreeMap;
use std::path::Path;

use axum::extract::{Path as Route, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use ai_team_core::{
    Chat, ChatMode, Event, EventKind, NodeRun, NodeStatus, Owner, Provider, RepoMap, Store, Usage,
};

use crate::error::Result;
use crate::state::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/chats/{id}/overview", get(overview))
        .route("/chats/{id}/overview/map", get(map))
}

/// How many of a chat's newest node runs the file evidence is read from.
///
/// Bounded rather than complete: this is a panel that re-reads on every tick, and a chat
/// a hundred turns deep would otherwise re-scan its whole history to draw one card. The
/// response says what was read, so the window can say so too rather than implying the
/// list is everything.
const NODES_READ: usize = 24;

/// How many paths one seat reports. Newest first, so a cut loses the oldest.
const TOUCHES_PER_SEAT: usize = 60;

/// What a solo chat dispatches: one seat, over the whole checkout.
///
/// Mirrors `Chat::agent()` in ai-team-core, which is crate-private. The map is coloured by
/// the seats that would actually work in *this* chat, so a solo chat is one zone and not
/// the project roster's several - the roster is not working here.
const SOLO_SEAT: (&str, &str, &str) = ("assistant", "Assistant", "**");

/// The checkout, and which of this chat's seats claims each path.
#[derive(Debug, Serialize)]
struct ChatMapView {
    workspace: String,
    map: RepoMap,
}

/// The repository, walked once for this chat.
///
/// The seats are read and the lock released *before* the walk: holding the store open
/// across a few hundred `read_dir` calls would stall every other request on the window
/// for as long as the disk takes.
async fn map(State(state): State<AppState>, Route(id): Route<i64>) -> Result<Json<ChatMapView>> {
    let (workspace, owners) = {
        let store = state.store()?;
        let store = store.lock();
        let chat = store.chat(id)?;
        let owners = seat_owners(&store, &chat)?;
        (chat.workspace_path, owners)
    };
    let walked = ai_team_core::repo_map(Path::new(&workspace), &owners)?;
    Ok(Json(ChatMapView {
        workspace,
        map: walked,
    }))
}

/// Which seats claim which paths in this chat.
///
/// A team chat dispatches the project's enabled seats, so their zones are what routes its
/// work. A solo chat dispatches one seat over everything. A disabled seat is not given
/// work, so its zone owns nothing - the map has to agree with dispatch or it is
/// describing a different program.
fn seat_owners(store: &Store, chat: &Chat) -> Result<Vec<Owner>> {
    if chat.mode == ChatMode::Single {
        let (role, name, zone) = SOLO_SEAT;
        return Ok(vec![Owner {
            role: role.to_string(),
            name: name.to_string(),
            zone: zone.to_string(),
        }]);
    }
    let project = store.project(chat.project_id)?;
    let Some(team) = project.team_id else {
        return Ok(Vec::new());
    };
    Ok(store
        .agents(team)?
        .into_iter()
        .filter(|agent| agent.enabled)
        .map(|agent| Owner {
            role: agent.role,
            name: agent.name,
            zone: agent.zone,
        })
        .collect())
}

#[derive(Debug, Serialize)]
struct ChatOverview {
    chat_id: i64,
    /// The project this chat belongs to, named rather than numbered.
    project: String,
    workspace: String,
    mode: ChatMode,
    seats: Vec<Seat>,
    totals: Totals,
    /// Every node run this chat has, and how many of them the evidence was read from.
    nodes_total: usize,
    nodes_read: usize,
}

/// One seat of this chat: its newest turn, and what every turn it took amounts to.
///
/// Grouped by role rather than listed per node run, because a role is what the operator
/// recognises and a retry is a new row (rule 8). The newest run supplies the operational
/// facts - status, command, model - and the older ones are folded into the totals and the
/// touched paths, which is the half that is a fact about the whole conversation.
#[derive(Debug, Serialize)]
struct Seat {
    role: String,
    node_id: i64,
    run_id: i64,
    provider: Provider,
    model: String,
    status: NodeStatus,
    attempt: i64,
    slice_key: Option<String>,
    /// The lease this seat worked in: a team build's draft worktree, or the chat's own
    /// checkout for a solo turn.
    worktree: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    blocked_reason: Option<String>,
    /// Whether this seat holds an unfinished turn.
    live: bool,
    /// Whether a process is actually behind that turn, for a live one. `None` when the
    /// seat is not live, because asking the operating system about a finished turn costs
    /// a `ps` and answers nothing. A live seat with `Some(false)` is interrupted, which
    /// is a different thing from working and must not animate as though it were.
    supervised: Option<bool>,
    /// The newest durable thing this seat did: the command it is running, or its last
    /// observable action.
    activity: Option<Activity>,
    /// The last thing it said, in words.
    said: Option<String>,
    /// How many turns this role took in the window that was read.
    runs: usize,
    /// Pi steps, summed over those turns.
    steps: i64,
    usage: Usage,
    /// Latest provider-reported context occupancy, from the newest turn's session.
    context_tokens: Option<i64>,
    /// Files this seat read, wrote or edited, newest first.
    touches: Vec<Touch>,
    /// Only the newest attempt may light files as current work.
    live_touches: Vec<Touch>,
    /// Paths it touched that resolve outside the checkout this chat maps, so the window
    /// can say so rather than quietly dropping them.
    outside: usize,
}

/// The newest durable event for one seat, reduced to what an operational card needs.
#[derive(Debug, Serialize)]
struct Activity {
    kind: EventKind,
    /// The tool, or the summary of whatever else it was.
    summary: String,
    /// Where it is reading or writing, what command it is running, what it searched for.
    /// Bounded, and never the whole payload or a tool result.
    detail: Option<String>,
    /// The file it named, relative to the checkout, when the tool named one that lands
    /// inside it. What the map lights as the path being worked on right now.
    file: Option<String>,
    at: String,
}

/// One path a seat touched, and how.
#[derive(Debug, Clone, Serialize)]
struct Touch {
    /// Relative to the checkout, which is what the map's nodes are keyed on.
    path: String,
    /// The tool that touched it last.
    tool: String,
    reads: i64,
    writes: i64,
    at: String,
}

#[derive(Debug, Default, Serialize)]
struct Totals {
    /// Turns in this conversation - messages that were answered, not Pi steps.
    turns: usize,
    steps: i64,
    usage: Usage,
    files_touched: usize,
    files_written: usize,
}

async fn overview(
    State(state): State<AppState>,
    Route(id): Route<i64>,
) -> Result<Json<ChatOverview>> {
    let store = state.store()?;
    let store = store.lock();
    let chat = store.chat(id)?;
    let project = store.project(chat.project_id)?;
    let turns = store.chat_turns(id)?;

    // One list of every node run this chat has, newest first: the turn's own seat and,
    // for a team turn, the members it dispatched.
    let mut nodes: Vec<Dispatched> = Vec::new();
    for turn in &turns {
        nodes.push(Dispatched {
            supervised: live(turn.node.status).then(|| chat.supervisor_alive(&turn.node)),
            node: turn.node.clone(),
        });
        for member in &turn.members {
            nodes.push(Dispatched {
                supervised: live(member.node.status).then(|| member.pi_alive()),
                node: member.node.clone(),
            });
        }
    }
    nodes.sort_by_key(|dispatched| std::cmp::Reverse(dispatched.node.id));
    // A team controller also appears in its member journal. Count it once.
    nodes.dedup_by_key(|dispatched| dispatched.node.id);
    let nodes_total = nodes.len();
    nodes.truncate(NODES_READ);

    let mut seats: Vec<Seat> = Vec::new();
    let mut at_role: BTreeMap<String, usize> = BTreeMap::new();
    let mut totals = Totals {
        turns: turns.len(),
        ..Totals::default()
    };
    for dispatched in &nodes {
        let roots = roots(&dispatched.node, &chat);
        let (touched, outside) = touches(&store, &dispatched.node, &roots)?;
        totals.steps += dispatched.node.turns;
        totals.usage += dispatched.node.usage;
        // An older turn by a role already seen keeps its evidence on that seat, but none
        // of its operational facts: the newest turn is what is current.
        if let Some(&at) = at_role.get(&dispatched.node.role) {
            fold(&mut seats[at], &dispatched.node, touched, outside);
        } else {
            at_role.insert(dispatched.node.role.clone(), seats.len());
            seats.push(seat(&store, dispatched, touched, outside, &roots)?);
        }
    }
    let mut distinct: BTreeMap<&str, i64> = BTreeMap::new();
    for touch in seats.iter().flat_map(|seat| &seat.touches) {
        *distinct.entry(touch.path.as_str()).or_default() += touch.writes;
    }
    totals.files_touched = distinct.len();
    totals.files_written = distinct.values().filter(|writes| **writes > 0).count();
    for seat in &mut seats {
        seat.touches.sort_by(|a, b| b.at.cmp(&a.at));
        seat.touches.truncate(TOUCHES_PER_SEAT);
        seat.live_touches.sort_by(|a, b| b.at.cmp(&a.at));
        seat.live_touches.truncate(TOUCHES_PER_SEAT);
    }

    Ok(Json(ChatOverview {
        chat_id: chat.id,
        project: project.name,
        workspace: chat.workspace_path.clone(),
        mode: chat.mode,
        seats,
        totals,
        nodes_total,
        nodes_read: nodes.len(),
    }))
}

struct Dispatched {
    node: NodeRun,
    supervised: Option<bool>,
}

/// Whether a node run holds an unfinished turn. Not whether anything is behind it.
fn live(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::Running | NodeStatus::Queued)
}

fn seat(
    store: &Store,
    dispatched: &Dispatched,
    touched: Vec<Touch>,
    outside: usize,
    roots: &[Option<&str>],
) -> Result<Seat> {
    let node = &dispatched.node;
    Ok(Seat {
        role: node.role.clone(),
        node_id: node.id,
        run_id: node.run_id,
        provider: node.provider,
        model: node.model.clone(),
        status: node.status,
        attempt: node.attempt,
        slice_key: node.slice_key.clone(),
        worktree: node.worktree_path.clone(),
        started_at: node.started_at.clone(),
        ended_at: node.ended_at.clone(),
        blocked_reason: node.blocked_reason.clone(),
        live: live(node.status),
        supervised: dispatched.supervised,
        activity: store
            .latest_node_event(node.id)?
            .map(|event| activity(event, roots)),
        said: store
            .latest_node_assistant_event(node.id)?
            .and_then(|event| event.payload)
            .and_then(|data| {
                ai_team_core::PiEvent {
                    kind: "message_end".into(),
                    data,
                }
                .assistant_message()
            })
            .map(|text| text.chars().take(2_000).collect()),
        runs: 1,
        steps: node.turns,
        usage: node.usage,
        context_tokens: node.context_tokens,
        live_touches: touched.clone(),
        touches: touched,
        outside,
    })
}

/// Add an older turn by the same role to the seat it belongs to.
fn fold(seat: &mut Seat, node: &NodeRun, touched: Vec<Touch>, outside: usize) {
    seat.runs += 1;
    seat.steps += node.turns;
    seat.usage += node.usage;
    seat.outside += outside;
    for touch in touched {
        match seat.touches.iter().position(|seen| seen.path == touch.path) {
            Some(at) => {
                let seen = &mut seat.touches[at];
                seen.reads += touch.reads;
                seen.writes += touch.writes;
                if touch.at > seen.at {
                    seen.at = touch.at;
                    seen.tool = touch.tool;
                }
            }
            None => seat.touches.push(touch),
        }
    }
}

fn activity(event: Event, roots: &[Option<&str>]) -> Activity {
    let file = event
        .payload
        .as_ref()
        .and_then(|payload| payload.get("args"))
        .and_then(|args| args.get("path"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| inside(raw, roots));
    Activity {
        detail: detail(&event),
        file,
        kind: event.kind,
        summary: event.summary,
        at: event.at,
    }
}

/// The useful, bounded part of a tool call: where it is reading or writing, what command
/// it is running, what it searched for. Never the whole payload, and never a result.
fn detail(event: &Event) -> Option<String> {
    let args = event.payload.as_ref()?.get("args")?;
    let found = ["path", "filePath", "command", "query", "pattern", "url"]
        .into_iter()
        .find_map(|key| args.get(key).and_then(serde_json::Value::as_str))?;
    Some(
        found
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect(),
    )
}

/// Which tools write. `edit` and `write` change the checkout; everything else that names
/// a path is reading it, and is reported as such rather than as work.
fn writes(tool: &str) -> bool {
    matches!(tool, "write" | "edit")
}

/// Where a seat's own paths are resolved from: the lease it worked in, and failing that
/// the chat's checkout. Both are stored resolved, which is what makes stripping them off
/// an absolute path a comparison rather than a guess (rule 11).
fn roots<'a>(node: &'a NodeRun, chat: &'a Chat) -> [Option<&'a str>; 2] {
    [
        node.worktree_path.as_deref(),
        Some(chat.workspace_path.as_str()),
    ]
}

/// The paths one seat touched, from its own stream.
///
/// `node_events_marked` filters on the payload in SQL, so only the rows that actually
/// carry a path argument are read back - the scan does not grow with how much a seat
/// said, only with how many files it opened.
fn touches(store: &Store, node: &NodeRun, roots: &[Option<&str>]) -> Result<(Vec<Touch>, usize)> {
    let mut found: Vec<Touch> = Vec::new();
    let mut outside = 0;
    for event in store.node_events_marked(node.id, "args.path")? {
        // The start of a call, not its end: both carry the arguments, and counting both
        // would report every file twice.
        if event.kind != EventKind::ToolCall {
            continue;
        }
        let Some(payload) = event.payload.as_ref() else {
            continue;
        };
        let Some(raw) = payload
            .get("args")
            .and_then(|args| args.get("path"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let tool = payload
            .get("toolName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(event.summary.as_str())
            .to_string();
        let Some(path) = inside(raw, roots) else {
            outside += 1;
            continue;
        };
        let wrote = i64::from(writes(&tool));
        match found.iter().position(|seen| seen.path == path) {
            Some(at) => {
                let seen = &mut found[at];
                seen.reads += 1 - wrote;
                seen.writes += wrote;
                seen.at = event.at;
                seen.tool = tool;
            }
            None => found.push(Touch {
                path,
                tool,
                reads: 1 - wrote,
                writes: wrote,
                at: event.at,
            }),
        }
    }
    Ok((found, outside))
}

/// Where a tool's path argument lands inside the checkout, or nothing.
///
/// A seat runs with its `cwd` on its lease (rule 4), so a relative path is already
/// relative to the checkout the map is drawn from. An absolute one is only usable if it
/// resolves beneath a known root. Resolve real aliases on macOS, but never guess an
/// alias or let a symlink outside the checkout light an unrelated file (D12).
fn inside(raw: &str, roots: &[Option<&str>]) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    let relative = if path.is_absolute() {
        roots.iter().copied().flatten().find_map(|root| {
            match (path.canonicalize(), Path::new(root).canonicalize()) {
                (Ok(path), Ok(root)) => path.strip_prefix(root).ok().map(Path::to_path_buf),
                _ => path.strip_prefix(root).ok().map(Path::to_path_buf),
            }
        })?
    } else {
        path.strip_prefix("./").unwrap_or(path).to_path_buf()
    };
    if relative
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    let cleaned = relative.to_str()?.trim_matches('/');
    if cleaned.is_empty() || cleaned.starts_with("..") {
        return None;
    }
    Some(cleaned.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_relative_to_the_lease_or_it_is_not_drawn() {
        let roots = [Some("/tmp/widget"), None];
        assert_eq!(inside("src/lib.rs", &roots).as_deref(), Some("src/lib.rs"));
        assert_eq!(
            inside("./src/lib.rs", &roots).as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(
            inside("/tmp/widget/src/lib.rs", &roots).as_deref(),
            Some("src/lib.rs")
        );
        // macOS resolves /tmp through /private, so an absolute path from a seat need not
        // spell its lease the way the chat row does. Guessing it is the same checkout is
        // how the wrong node lights up (D12).
        assert_eq!(inside("/private/tmp/widget/src/lib.rs", &roots), None);
        assert_eq!(inside("/etc/passwd", &roots), None);
        assert_eq!(inside("../outside.rs", &roots), None);
        assert_eq!(inside("   ", &roots), None);
    }

    #[test]
    fn only_the_two_editing_tools_count_as_work() {
        assert!(writes("write"));
        assert!(writes("edit"));
        assert!(!writes("read"));
        assert!(!writes("bash"));
    }
}
