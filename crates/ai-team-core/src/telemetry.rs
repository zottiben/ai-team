//! Bounded provider-reported measurements, not execution authority or cumulative spend.
//! Their receive clock and immutable stream scope live on the containing event.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Telemetry {
    Usage {
        phase: UsagePhase,
        sequence: Option<u64>,
        tokens: TokenObservation,
    },
    Compaction {
        phase: CompactionPhase,
        sequence: Option<u64>,
        reason: Option<CompactionReason>,
        tokens_before: Option<u64>,
        estimated_tokens_after: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePhase {
    Live,
    MessageEnd,
    Step,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenObservation {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub context: Option<u64>,
    pub context_source: Option<ContextSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    ReportedTotal,
    CompleteParts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionPhase {
    Started,
    Completed,
    Aborted,
    Retrying,
    Failed,
    Unknown,
}
