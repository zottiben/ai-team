//! The row types, and the enums the schema's CHECK constraints mirror.
//!
//! Every enum here has a `&'static str` spelling that is exactly what goes in the
//! column, and a `FromStr` that refuses anything else. The database and these types are
//! two halves of one statement: if they disagree, a row that SQLite accepted will fail
//! to load, which is a bug you find immediately rather than a silent misread.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Generates the enum, its column spelling, `Display`, `FromStr` and the rusqlite
/// conversions. Hand-writing five impls per enum is how one of them ends up subtly
/// different from the schema.
macro_rules! sql_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self, Error> {
                match s {
                    $($text => Ok($name::$variant),)+
                    other => Err(Error::invalid(format!(
                        "{other:?} is not a valid {}: expected one of {}",
                        stringify!($name),
                        [$($text),+].join(", "),
                    ))),
                }
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                let text = v.as_str()?;
                text.parse().map_err(|e| {
                    rusqlite::types::FromSqlError::Other(Box::new(e))
                })
            }
        }

        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(rusqlite::types::ToSqlOutput::Borrowed(self.as_str().into()))
            }
        }
    };
}

sql_enum! {
    /// What kind of container this project is (D6). A project is not a repo: a triage
    /// session across four services and a one-file chore are both projects.
    ProjectKind {
        Repo => "repo",
        Ticket => "ticket",
        Epic => "epic",
        Quickfix => "quickfix",
        Triage => "triage",
        Chore => "chore",
        Adhoc => "adhoc",
    }
}

sql_enum! {
    ProjectStatus {
        Active => "active",
        Paused => "paused",
        Done => "done",
        Archived => "archived",
    }
}

sql_enum! {
    /// Where a project came from. Read-only in both directions that matter: D9 says
    /// ai-team never writes back to ClickUp or Figma.
    ProjectSource {
        ClickUp => "clickup",
        Figma => "figma",
        Manual => "manual",
    }
}

sql_enum! {
    /// The only providers that exist (D8). There is no metered-API variant, and adding
    /// one means re-opening a decision rather than adding an enum arm.
    Provider {
        /// Claude subscription, through the OAuthed Claude Code CLI, bridged into eve
        /// as a model provider (D7).
        Claude => "claude",
        /// ChatGPT subscription, through the Codex CLI that Pi's `openai-codex` uses.
        #[serde(rename = "openai")]
        OpenAi => "openai",
        /// GLM Coding Plan - flat rate, openai-compatible.
        #[serde(rename = "zai")]
        ZAi => "zai",
        /// The ailocal gateway on loopback. Free, and always allowed.
        Local => "local",
    }
}

sql_enum! {
    Reasoning {
        None => "none",
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}

sql_enum! {
    /// What a team does when a node fails past its repair budget (M2-S10).
    OnFailure {
        /// Try again, up to `max_repairs`.
        Retry => "retry",
        /// Park it and ask the human.
        Escalate => "escalate",
        /// Give up on this branch, and let its siblings finish.
        AbortBranch => "abort_branch",
    }
}

sql_enum! {
    DeliveryAction {
        Push => "push",
        Pr => "pr",
        Merge => "merge",
    }
}

sql_enum! {
    /// Who crosses one publishing boundary after verified work lands locally.
    DeliveryPolicy {
        /// ai-team records the branch and leaves the command to the operator.
        Manual => "manual",
        /// ai-team offers an explicit approval control. The safe default.
        Ask => "ask",
        /// ai-team proceeds after verification without another click.
        Auto => "auto",
    }
}

sql_enum! {
    RunStatus {
        Queued => "queued",
        Planning => "planning",
        Running => "running",
        Blocked => "blocked",
        Done => "done",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

sql_enum! {
    RunTrigger {
        Manual => "manual",
        Scheduled => "scheduled",
        Reminder => "reminder",
        Review => "review",
    }
}

sql_enum! {
    NodeStatus {
        Queued => "queued",
        Running => "running",
        /// Waiting on a human: an approval, or review comments.
        Parked => "parked",
        Blocked => "blocked",
        Done => "done",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

sql_enum! {
    /// The event stream's vocabulary, fed from eve's NDJSON (M1-S3).
    EventKind {
        Step => "step",
        ToolCall => "tool_call",
        ToolResult => "tool_result",
        Cost => "cost",
        ApprovalRequest => "approval_request",
        ApprovalResolved => "approval_resolved",
        Build => "build",
        Note => "note",
        Done => "done",
        Failed => "failed",
    }
}

sql_enum! {
    ReviewStatus {
        Open => "open",
        ChangesRequested => "changes_requested",
        Approved => "approved",
        Dismissed => "dismissed",
    }
}

sql_enum! {
    CommentStatus {
        Open => "open",
        Resolved => "resolved",
        /// The line it was anchored to no longer exists.
        Outdated => "outdated",
    }
}

sql_enum! {
    /// Which side of the diff a comment is anchored to. Without this, a comment on a
    /// removed line and one on an added line at the same number are indistinguishable.
    DiffSide {
        Old => "old",
        New => "new",
    }
}

sql_enum! {
    ReminderKind {
        Reminder => "reminder",
        Idea => "idea",
        ScheduledRun => "scheduled_run",
    }
}

sql_enum! {
    ReminderStatus {
        Pending => "pending",
        Fired => "fired",
        Done => "done",
        Cancelled => "cancelled",
    }
}

sql_enum! {
    Recur {
        Daily => "daily",
        Weekdays => "weekdays",
        Weekly => "weekly",
        Monthly => "monthly",
    }
}

sql_enum! {
    ToolEffect {
        Allow => "allow",
        Deny => "deny",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub kind: ProjectKind,
    pub status: ProjectStatus,
    pub summary: Option<String>,
    pub brief_md: String,
    pub source: Option<ProjectSource>,
    pub source_key: Option<String>,
    pub source_url: Option<String>,
    pub team_id: Option<i64>,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewProject {
    pub slug: Option<String>,
    pub name: String,
    pub kind: Option<ProjectKind>,
    pub summary: Option<String>,
    pub brief_md: String,
    pub source: Option<ProjectSource>,
    pub source_key: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRepo {
    pub id: i64,
    pub project_id: i64,
    pub ord: i64,
    pub key: String,
    pub name: String,
    pub remote_url: Option<String>,
    pub main_path: Option<String>,
    pub default_branch: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewRepo {
    pub remote_url: Option<String>,
    pub name: Option<String>,
    pub main_path: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: i64,
    pub project_id: Option<i64>,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub guardrails: Guardrails,
    pub delivery: DeliverySettings,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySettings {
    pub push: DeliveryPolicy,
    pub pr: DeliveryPolicy,
    pub merge: DeliveryPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDeliveryStatus {
    pub pr_state: String,
    pub checks: String,
}

impl Default for DeliverySettings {
    fn default() -> Self {
        Self {
            push: DeliveryPolicy::Ask,
            pr: DeliveryPolicy::Ask,
            merge: DeliveryPolicy::Ask,
        }
    }
}

/// A team's policy, and the shape a run snapshots at dispatch (M2-S10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guardrails {
    pub parallel_width: i64,
    pub budget_tokens_run: Option<i64>,
    pub budget_tokens_node: Option<i64>,
    pub budget_seconds_run: Option<i64>,
    pub budget_seconds_node: Option<i64>,
    pub max_turns_node: Option<i64>,
    pub max_repairs: i64,
    pub on_failure: OnFailure,
}

impl Default for Guardrails {
    fn default() -> Self {
        // Deliberately finite. An unbounded default is the setting nobody changes until
        // the night it costs them a rate limit.
        Guardrails {
            parallel_width: 2,
            budget_tokens_run: Some(2_000_000),
            budget_tokens_node: Some(400_000),
            budget_seconds_run: Some(3_600),
            budget_seconds_node: Some(900),
            max_turns_node: Some(40),
            max_repairs: 2,
            on_failure: OnFailure::Retry,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: i64,
    pub team_id: i64,
    pub ord: i64,
    pub role: String,
    pub name: String,
    pub purpose: String,
    pub provider: Provider,
    pub model: String,
    pub reasoning: Reasoning,
    pub zone: String,
    pub prompt_preset: Option<String>,
    pub prompt_md: Option<String>,
    /// The model's context window in tokens. eve cannot infer it for a non-Gateway
    /// model and refuses to compile compaction without it.
    pub context_window: Option<i64>,
    pub read_only: bool,
    pub enabled: bool,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewAgent {
    pub role: String,
    pub name: String,
    pub purpose: String,
    pub provider: Provider,
    pub model: String,
    pub reasoning: Reasoning,
    pub zone: String,
    pub prompt_preset: Option<String>,
    pub prompt_md: Option<String>,
    pub context_window: Option<i64>,
    pub read_only: bool,
    pub enabled: bool,
    pub ord: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolPolicy {
    pub tool: String,
    pub effect: ToolEffect,
    pub note: Option<String>,
}

impl From<&Agent> for NewAgent {
    fn from(agent: &Agent) -> Self {
        NewAgent {
            role: agent.role.clone(),
            name: agent.name.clone(),
            purpose: agent.purpose.clone(),
            provider: agent.provider,
            model: agent.model.clone(),
            reasoning: agent.reasoning,
            zone: agent.zone.clone(),
            prompt_preset: agent.prompt_preset.clone(),
            prompt_md: agent.prompt_md.clone(),
            context_window: agent.context_window,
            read_only: agent.read_only,
            enabled: agent.enabled,
            ord: agent.ord,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub project_id: i64,
    pub team_id: Option<i64>,
    pub prompt: String,
    pub status: RunStatus,
    pub trigger: RunTrigger,
    pub plan_slug: Option<String>,
    /// The checkout whose planner and team own this run. Maker nodes may lease sibling
    /// worktrees without moving the run out of its initiating workspace.
    pub workspace_path: Option<String>,
    pub parallel_width: i64,
    pub budget_tokens: Option<i64>,
    pub budget_seconds: Option<i64>,
    /// How many times a failing node may be repaired before `on_failure` applies.
    pub max_repairs: i64,
    /// What one node may spend, snapshotted like the rest.
    pub budget_tokens_node: Option<i64>,
    pub budget_seconds_node: Option<i64>,
    pub max_turns_node: Option<i64>,
    pub on_failure: OnFailure,
    pub blocked_reason: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRun {
    pub id: i64,
    pub run_id: i64,
    pub agent_id: Option<i64>,
    pub role: String,
    pub provider: Provider,
    pub model: String,
    pub status: NodeStatus,
    pub attempt: i64,
    pub slice_key: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub lease_id: Option<String>,
    pub pushed_at: Option<String>,
    pub pr_url: Option<String>,
    pub merge_requested_at: Option<String>,
    pub delivery_claim: Option<String>,
    pub delivery_claimed_at: Option<String>,
    pub delivery_error: Option<String>,
    pub session_id: Option<String>,
    /// Set when future turns must start fresh. The id stays as transcript evidence.
    pub session_retired_at: Option<String>,
    /// Set while an orchestrator is writing its required handoff before retirement.
    pub session_resetting_at: Option<String>,
    /// The local process supervising this node. Not rendered to the browser; the API
    /// reduces it to the useful question: can this interrupted turn be resumed?
    #[serde(skip_serializing)]
    pub supervisor_pid: Option<i64>,
    /// Latest provider-reported context occupancy for this session, including caches.
    /// Unlike `usage`, this is a snapshot rather than a cumulative spend counter.
    pub context_tokens: Option<i64>,
    /// The loopback port this node's supervised `eve start` was given (D10), and the
    /// secret its channel checks. Recorded so the window can answer a question the
    /// terminal's agent asked - see migration 005 for why that is not a wider boundary.
    pub eve_port: Option<i64>,
    pub eve_token: Option<String>,
    /// How many of eve's stream events have been consumed. This is eve's own
    /// `startIndex`, an absolute count - resuming asks for `?startIndex=<this>`.
    pub stream_cursor: i64,
    pub usage: Usage,
    pub turns: i64,
    pub blocked_reason: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Token accounting, with cache reads kept apart from fresh input.
///
/// A Claude node's ~64k system prefix is cached after the first call, so folding cache
/// reads into `input` makes every cold node look like a runaway and hides the thing
/// analytics actually wants to show: which nodes are paying the prefix repeatedly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

impl Usage {
    /// What this node actually put through the model, cache reads excluded.
    pub fn billable(&self) -> i64 {
        self.tokens_in + self.tokens_out + self.cache_write
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, rhs: Usage) {
        self.tokens_in += rhs.tokens_in;
        self.tokens_out += rhs.tokens_out;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub run_id: i64,
    pub node_run_id: Option<i64>,
    pub at: String,
    pub kind: EventKind,
    pub actor: Option<String>,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub node_run_id: Option<i64>,
    pub kind: EventKind,
    pub actor: Option<String>,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
}

impl NewEvent {
    pub fn new(kind: EventKind, summary: impl Into<String>) -> Self {
        NewEvent {
            node_run_id: None,
            kind,
            actor: None,
            summary: summary.into(),
            payload: None,
        }
    }

    #[must_use]
    pub fn on_node(mut self, node_run_id: i64) -> Self {
        self.node_run_id = Some(node_run_id);
        self
    }

    #[must_use]
    pub fn by(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    #[must_use]
    pub fn with(mut self, payload: serde_json::Value) -> Self {
        self.payload = Some(payload);
        self
    }
}

/// One durable request to bring an authoritative state transition to the operator's
/// attention. It is delivery state, not a copy of the run or event that caused it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub project_id: i64,
    pub workspace_path: Option<String>,
    pub run_id: Option<i64>,
    pub node_run_id: Option<i64>,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub action_path: Option<String>,
    pub read_at: Option<String>,
    pub delivered_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct NewNotification {
    pub dedupe_key: String,
    pub project_id: i64,
    pub workspace_path: Option<String>,
    pub run_id: Option<i64>,
    pub node_run_id: Option<i64>,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub action_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: i64,
    pub project_id: i64,
    pub run_id: Option<i64>,
    pub node_run_id: Option<i64>,
    pub title: String,
    pub status: ReviewStatus,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub submitted_at: Option<String>,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: i64,
    pub review_id: i64,
    pub parent_id: Option<i64>,
    pub file_path: Option<String>,
    pub side: Option<DiffSide>,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub author: String,
    pub body: String,
    pub status: CommentStatus,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewComment {
    pub parent_id: Option<i64>,
    pub file_path: Option<String>,
    pub side: Option<DiffSide>,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub author: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: i64,
    pub project_id: Option<i64>,
    pub team_id: Option<i64>,
    pub kind: ReminderKind,
    pub title: String,
    pub body: String,
    pub prompt: Option<String>,
    pub due_at: Option<String>,
    pub recur: Option<Recur>,
    pub status: ReminderStatus,
    pub last_fired_at: Option<String>,
    pub rev: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewReminder {
    pub project_id: Option<i64>,
    pub team_id: Option<i64>,
    pub kind: Option<ReminderKind>,
    pub title: String,
    pub body: String,
    pub prompt: Option<String>,
    pub due_at: Option<String>,
    pub recur: Option<Recur>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_their_column_spelling() {
        for kind in ProjectKind::ALL {
            assert_eq!(kind.as_str().parse::<ProjectKind>().unwrap(), *kind);
        }
        for provider in Provider::ALL {
            assert_eq!(provider.as_str().parse::<Provider>().unwrap(), *provider);
        }
        // The one that is easy to get wrong: snake_case in the column, CamelCase in Rust.
        assert_eq!(OnFailure::AbortBranch.as_str(), "abort_branch");
        assert_eq!(EventKind::ToolResult.as_str(), "tool_result");
        assert_eq!(ReviewStatus::ChangesRequested.as_str(), "changes_requested");
    }

    #[test]
    fn an_unknown_value_is_refused_with_the_alternatives() {
        let err = "sprint".parse::<ProjectKind>().unwrap_err().to_string();
        assert!(err.contains("sprint"), "{err}");
        assert!(err.contains("quickfix"), "{err}");
    }

    #[test]
    fn a_metered_provider_does_not_parse() {
        // D8 has no API-key path, so there is nothing for this to deserialise into.
        assert!("anthropic-api".parse::<Provider>().is_err());
        assert!("openrouter".parse::<Provider>().is_err());
    }

    #[test]
    fn cache_reads_are_not_billable() {
        let usage = Usage {
            tokens_in: 1_000,
            tokens_out: 500,
            cache_read: 61_416,
            cache_write: 2_686,
        };
        // The measured warm Claude run: 61k of that is a cached prefix, and counting it
        // would make the node look 20x more expensive than it is.
        assert_eq!(usage.billable(), 4_186);
    }
}

/// How a turn ended.
///
/// A fact about the turn rather than about the runtime that took it, which is why it
/// outlived the module it was born in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalState {
    Completed,
    Failed,
    Cancelled,
}
