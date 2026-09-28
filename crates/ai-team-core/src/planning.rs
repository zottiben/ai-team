//! The chat-scoped planning contract shared by the window, Pi MCP and future dispatch.

use serde::{Deserialize, Serialize};

pub use ai_planner_core::{PlanBundle, Status as PlanStatus};

#[derive(Debug, Clone, Serialize)]
pub struct ChatPlan {
    pub chat_id: i64,
    pub project_id: i64,
    pub revision: i64,
    pub bundle: Option<PlanBundle>,
}

/// Agent identity comes from an existing execution row, never a client-supplied role.
#[derive(Debug, Clone, Copy)]
pub enum PlanActor {
    Human,
    Agent(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAccess {
    Coordinator,
    Reader,
    Maker,
    Planner,
    Human,
}

impl PlanAccess {
    pub(crate) fn for_team_agent(agent: &crate::Agent) -> Self {
        match agent.role.as_str() {
            crate::ROOT_ROLE => Self::Coordinator,
            "planner" => Self::Planner,
            "verifier" | "reviewer" => Self::Reader,
            _ if agent.read_only => Self::Reader,
            _ => Self::Maker,
        }
    }

    pub(crate) fn member_name(self) -> crate::Result<&'static str> {
        match self {
            Self::Coordinator => Ok("coordinator"),
            Self::Reader => Ok("reader"),
            Self::Maker => Ok("maker"),
            Self::Planner => Ok("planner"),
            Self::Human => Err(crate::Error::invalid(
                "an agent cannot inherit human permissions",
            )),
        }
    }

    pub(crate) fn from_member(value: &str) -> crate::Result<Self> {
        match value {
            "coordinator" => Ok(Self::Coordinator),
            "reader" => Ok(Self::Reader),
            "maker" => Ok(Self::Maker),
            "planner" => Ok(Self::Planner),
            _ => Err(crate::Error::invalid(
                "invalid team member planning permissions",
            )),
        }
    }

    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Coordinator => &["get_plan", "open_question", "append_log"],
            Self::Reader => &["get_plan"],
            Self::Maker => &[
                "get_plan",
                "set_slice_status",
                "open_question",
                "append_log",
            ],
            Self::Planner | Self::Human => &[
                "get_plan",
                "create_plan",
                "set_plan_status",
                "write_section",
                "add_slice",
                "update_slice",
                "set_slice_status",
                "add_decision",
                "open_question",
                "append_log",
                "add_gotcha",
            ],
        }
    }
}

/// Every mutation carries the revision read from this chat's plan. Unknown fields are
/// rejected, including attempted cwd/project/plan overrides in an agent's tool call.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanAction {
    CreatePlan {
        expect_revision: i64,
        title: String,
        summary: Option<String>,
    },
    SetPlanStatus {
        expect_revision: i64,
        status: PlanStatus,
    },
    WriteSection {
        expect_revision: i64,
        key: String,
        title: String,
        body: String,
    },
    AddSlice {
        expect_revision: i64,
        key: String,
        title: String,
        scope: String,
        touches: Vec<String>,
        demo: String,
    },
    UpdateSlice {
        expect_revision: i64,
        key: String,
        title: String,
        scope: String,
        touches: Vec<String>,
        demo: String,
    },
    SetSliceStatus {
        expect_revision: i64,
        key: String,
        status: PlanStatus,
        reason: Option<String>,
    },
    AddDecision {
        expect_revision: i64,
        title: String,
        body: String,
    },
    OpenQuestion {
        expect_revision: i64,
        body: String,
        slice: Option<String>,
    },
    AnswerQuestion {
        expect_revision: i64,
        question_id: i64,
        answer: String,
    },
    AppendLog {
        expect_revision: i64,
        body: String,
        slice: Option<String>,
    },
    AddGotcha {
        expect_revision: i64,
        title: String,
        body: String,
    },
}

impl PlanAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::CreatePlan { .. } => "create_plan",
            Self::SetPlanStatus { .. } => "set_plan_status",
            Self::WriteSection { .. } => "write_section",
            Self::AddSlice { .. } => "add_slice",
            Self::UpdateSlice { .. } => "update_slice",
            Self::SetSliceStatus { .. } => "set_slice_status",
            Self::AddDecision { .. } => "add_decision",
            Self::OpenQuestion { .. } => "open_question",
            Self::AnswerQuestion { .. } => "answer_question",
            Self::AppendLog { .. } => "append_log",
            Self::AddGotcha { .. } => "add_gotcha",
        }
    }

    pub(crate) fn revision(&self) -> i64 {
        match self {
            Self::CreatePlan {
                expect_revision, ..
            }
            | Self::SetPlanStatus {
                expect_revision, ..
            }
            | Self::WriteSection {
                expect_revision, ..
            }
            | Self::AddSlice {
                expect_revision, ..
            }
            | Self::UpdateSlice {
                expect_revision, ..
            }
            | Self::SetSliceStatus {
                expect_revision, ..
            }
            | Self::AddDecision {
                expect_revision, ..
            }
            | Self::OpenQuestion {
                expect_revision, ..
            }
            | Self::AnswerQuestion {
                expect_revision, ..
            }
            | Self::AppendLog {
                expect_revision, ..
            }
            | Self::AddGotcha {
                expect_revision, ..
            } => *expect_revision,
        }
    }

    pub(crate) fn slice(&self) -> Option<&str> {
        match self {
            Self::SetSliceStatus { key, .. } => Some(key),
            Self::OpenQuestion { slice, .. } | Self::AppendLog { slice, .. } => slice.as_deref(),
            _ => None,
        }
    }
}
