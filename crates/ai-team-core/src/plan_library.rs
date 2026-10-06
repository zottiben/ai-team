//! The cross-project board over AI Team's own plans, and explicit import of a
//! standalone ai-planner plan into it (D27).
//!
//! Two directions over one owned store, and only one of them touches a neighbour.
//! The board is a read of plans ai-team already owns, across every project, resolved
//! back to the exact chat each belongs to. Import is a single, reviewed, one-way copy:
//! the operator names a database, reads a preview of exactly what would be written,
//! and approves it into one idle empty chat.
//!
//! What import is deliberately not: it never opens the standalone database through the
//! writable planner `Store` (that would migrate it), never registers it, never defaults
//! to it, and never looks at it again afterwards. There is no synchronisation pipeline,
//! because a second writable copy of one plan is the problem the planner was built to
//! remove. An imported claim, lease, branch or pull request arrives as *history* - the
//! plan carries the evidence, and nothing in it is authority to build or publish.

use serde::{Deserialize, Serialize};

use crate::planning::PlanStatus;

/// Which plans the board should show. Empty means everything ai-team owns.
#[derive(Debug, Clone, Default)]
pub struct PlanLibraryFilter {
    /// A project slug. `None` is every project.
    pub project: Option<String>,
    /// Statuses to keep. Empty is every status.
    pub status: Vec<PlanStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanLibrary {
    pub entries: Vec<PlanLibraryEntry>,
    /// Every project that owns a plan, filtered or not, so the picker keeps its options
    /// after a filter hides the rows they came from.
    pub projects: Vec<PlanLibraryProject>,
    /// Chats an import may be approved into, right now.
    pub destinations: Vec<PlanImportTarget>,
    /// Owned plans whose chat or project row is gone. Shown rather than dropped: they
    /// are why a chat id cannot be reused for an import.
    pub detached: Vec<DetachedPlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanLibraryProject {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub plans: i64,
}

/// One owned plan, and the exact chat that opens it.
#[derive(Debug, Clone, Serialize)]
pub struct PlanLibraryEntry {
    pub plan_id: i64,
    pub slug: String,
    pub title: String,
    pub status: PlanStatus,
    pub summary: Option<String>,
    pub project_id: i64,
    pub project_slug: String,
    pub project_name: String,
    pub chat_id: i64,
    pub chat_title: String,
    pub chat_archived: bool,
    pub slices: i64,
    pub done: i64,
    pub open_questions: i64,
    pub updated_at: String,
    /// The newest progress note, which is what "active" actually means to a person.
    pub last_activity: Option<String>,
    /// Present only on an imported plan, so the board never implies ai-team planned it.
    pub imported: Option<PlanProvenance>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanProvenance {
    pub source_path: String,
    /// `<repo key>/<slug>` as the source named it.
    pub source_plan: Option<String>,
    pub imported_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetachedPlan {
    pub plan_id: i64,
    pub slug: String,
    pub title: String,
    pub repo_key: String,
    pub why: String,
}

/// A chat an import may be written into: idle, empty, not archived, and with no plan.
#[derive(Debug, Clone, Serialize)]
pub struct PlanImportTarget {
    pub chat_id: i64,
    pub project_id: i64,
    pub project_slug: String,
    pub project_name: String,
    pub title: String,
    pub workspace_path: String,
    pub created_at: String,
}

/// The source database, as read - never as opened for writing.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSource {
    /// Canonical, because `/var` and `/private/var` are different strings (D12).
    pub path: String,
    pub bytes: u64,
    /// sha256 over the database file and any journal beside it, taken twice around the
    /// copy so a database being written during the read is refused rather than torn.
    pub digest: String,
    pub schema_version: i64,
    pub plans: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanSourceSurvey {
    pub source: PlanSource,
    pub plans: Vec<PlanSourcePlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanSourcePlan {
    pub id: i64,
    pub repo_key: String,
    pub repo_name: String,
    pub slug: String,
    pub title: String,
    pub status: PlanStatus,
    pub summary: Option<String>,
    pub slices: i64,
    pub done: i64,
    pub open_questions: i64,
    pub created_at: String,
    pub updated_at: String,
    /// Where this exact plan already lives here. Import refuses a second copy.
    pub already_imported: Option<PlanImportedInto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanImportedInto {
    pub chat_id: i64,
    pub project_slug: Option<String>,
    pub title: String,
    pub imported_at: String,
}

/// Everything the import would write, counted by kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PlanImportCounts {
    pub sections: i64,
    pub slices: i64,
    pub slice_deps: i64,
    pub decisions: i64,
    pub questions: i64,
    pub gotchas: i64,
    pub log: i64,
    pub sources: i64,
    pub handoffs: i64,
    pub raw_bytes: i64,
    /// Records of the markdown file the source plan was itself imported from.
    pub file_imports: i64,
    /// Learned branch/worktree associations. Read to be reported, never written here.
    pub affinities: i64,
    pub embeddings: i64,
}

/// A slice that arrived holding execution state somewhere else.
#[derive(Debug, Clone, Serialize)]
pub struct PlanImportEvidence {
    pub key: String,
    pub title: String,
    pub status: PlanStatus,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub base_branch: Option<String>,
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanImportPreview {
    pub source: PlanSource,
    pub plan: PlanSourcePlan,
    /// What the approval must echo back. It is a digest of the whole extracted plan, so
    /// a source edited between reading and approving is refused rather than imported
    /// as something the operator never saw.
    pub fingerprint: String,
    pub counts: PlanImportCounts,
    /// What is carried over, in sentences, including the parts that keep their original
    /// dates and actors.
    pub preserved: Vec<String>,
    /// What is not carried over, or carried over only as history. Never silent.
    pub warnings: Vec<String>,
    pub evidence: Vec<PlanImportEvidence>,
    /// Why this plan cannot be imported at all, if it cannot. The preview still renders.
    pub refusal: Option<String>,
}

/// The approval. Every field is the operator's explicit choice; nothing is inferred
/// from a working directory, a branch or the newest chat.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanImportRequest {
    pub path: String,
    pub plan_id: i64,
    pub chat_id: i64,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanImported {
    pub chat_id: i64,
    pub project_id: i64,
    pub project_slug: String,
    pub plan_id: i64,
    pub slug: String,
    pub title: String,
    pub revision: i64,
    pub counts: PlanImportCounts,
    pub warnings: Vec<String>,
    pub source: PlanSource,
}

/// The repo key a project's owned plans live under, and the chat a plan slug names.
/// Both are exact identities written by ai-team, never a resolution or a guess.
pub(crate) fn chat_of_slug(slug: &str) -> Option<i64> {
    slug.strip_prefix("chat-")?.parse().ok()
}

pub(crate) fn project_of_repo_key(key: &str) -> Option<i64> {
    key.strip_prefix("ai-team-project-")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_round_trip_exactly_and_refuse_anything_else() {
        assert_eq!(chat_of_slug("chat-12"), Some(12));
        assert_eq!(chat_of_slug("chat-12-old"), None);
        assert_eq!(chat_of_slug("codex-shaped-ai-team"), None);
        assert_eq!(project_of_repo_key("ai-team-project-3"), Some(3));
        assert_eq!(project_of_repo_key("ai-team-project-"), None);
        assert_eq!(project_of_repo_key("git:github.com/x/y"), None);
    }
}
