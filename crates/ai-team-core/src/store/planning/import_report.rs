//! What an import would carry, what it would not, and the record it leaves behind.
//!
//! The counts and the sentences come from the same extracted plan the write uses, so the
//! preview cannot promise one thing and the import write another. Anything that is not
//! carried over verbatim is said here - an import that quietly dropped a progress log's
//! dates, or re-created another machine's claim, would be worse than one that refused.

use std::fmt::Write as _;

use ai_planner_core::Status;

use super::import_source::{SourcePlan, SourceSlice};
use crate::plan_library::{PlanImportCounts, PlanImportEvidence, PlanSource};

pub(super) struct Report {
    pub counts: PlanImportCounts,
    pub preserved: Vec<String>,
    pub warnings: Vec<String>,
    pub evidence: Vec<PlanImportEvidence>,
}

pub(super) fn report(plan: &SourcePlan) -> Report {
    let counts = counts(plan);
    Report {
        preserved: preserved(plan, &counts),
        warnings: warnings(plan, &counts),
        evidence: plan.held().into_iter().map(entry).collect(),
        counts,
    }
}

fn counts(plan: &SourcePlan) -> PlanImportCounts {
    PlanImportCounts {
        sections: count(plan.sections.len()),
        slices: count(plan.slices.len()),
        slice_deps: count(plan.deps.len()),
        decisions: count(plan.decisions.len()),
        questions: count(plan.questions.len()),
        gotchas: count(plan.gotchas.len()),
        log: count(plan.log.len()),
        sources: count(plan.sources.len()),
        handoffs: count(plan.handoffs.len()),
        raw_bytes: plan.raw_md.as_ref().map_or(0, |raw| count(raw.len())),
        file_imports: count(plan.file_imports.len()),
        affinities: plan.affinities,
        embeddings: plan.embeddings,
    }
}

/// A count that cannot be negative and will never realistically saturate, but is also
/// never allowed to wrap into one that lies.
pub(super) fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn preserved(plan: &SourcePlan, counts: &PlanImportCounts) -> Vec<String> {
    let mut lines = Vec::new();
    let mut say = |count: i64, text: &str| {
        if count > 0 {
            lines.push(format!("{count} {text}"));
        }
    };
    say(counts.sections, "sections, in their original order");
    say(
        counts.slices,
        "delivery slices with their status, scope, verification criteria and blocking reasons",
    );
    say(counts.slice_deps, "dependency edges between slices");
    say(
        counts.decisions,
        "decisions, superseded ones included, with their original dates",
    );
    say(
        counts.questions,
        "questions with their answers and the dates they were asked and answered",
    );
    say(counts.gotchas, "gotchas");
    say(
        counts.log,
        "progress notes with their original dates, actors, branches and worktrees",
    );
    say(counts.sources, "source references");
    say(
        counts.handoffs,
        "handoff checkpoints with their gates, branch and commit, kept as history",
    );
    if counts.raw_bytes > 0 {
        lines.push(format!(
            "the original markdown document the plan was written from ({})",
            size(counts.raw_bytes)
        ));
    }
    if plan.ticket_key.is_some() || plan.ticket_url.is_some() {
        lines.push("the plan's ticket reference".into());
    }
    if plan.owner.is_some() {
        lines.push("the plan's recorded owner".into());
    }
    if plan.base_branch.is_some() {
        lines.push("the plan's base branch".into());
    }
    lines
}

fn warnings(plan: &SourcePlan, counts: &PlanImportCounts) -> Vec<String> {
    let mut lines = Vec::new();
    let held = plan.held().len();
    if held > 0 {
        lines.push(format!(
            "{held} slices arrive as history: their claim, worktree, branch, base and pull \
             request are recorded in the plan's import record rather than re-created. This \
             import leases nothing, builds nothing and publishes nothing."
        ));
    }
    let running = plan
        .slices
        .iter()
        .filter(|slice| matches!(slice.status, Status::Active | Status::InReview))
        .count();
    if running > 0 {
        lines.push(format!(
            "{running} slices keep a status of active or in review. AI Team only dispatches \
             ready slices, so nothing here starts work; move one to ready when it is genuinely \
             yours to build."
        ));
    }
    if counts.affinities > 0 {
        lines.push(format!(
            "{} learned branch and worktree associations are not imported: they resolve plans \
             in the other tool's checkouts, and this plan belongs to one exact chat.",
            counts.affinities
        ));
    }
    if counts.embeddings > 0 {
        lines.push(format!(
            "{} search embeddings are not copied. They are derived from the text that is, and \
             are rebuilt on demand.",
            counts.embeddings
        ));
    }
    if counts.file_imports > 0 {
        lines.push(format!(
            "{} records of the markdown files the source plan was itself imported from are \
             preserved as text in the import record, not as live import records.",
            counts.file_imports
        ));
    }
    lines.push(
        "The plan row created here is new, so it carries today's created and updated dates. \
         The original dates are written into the import record."
            .into(),
    );
    lines.push(
        "The source database is not changed, registered or watched. Nothing written here goes \
         back to it, and nothing there updates this copy."
            .into(),
    );
    lines
}

fn entry(slice: &SourceSlice) -> PlanImportEvidence {
    PlanImportEvidence {
        key: slice.key.clone(),
        title: slice.title.clone(),
        status: slice.status,
        claimed_by: slice.claimed_by.clone(),
        claimed_at: slice.claimed_at.clone(),
        worktree_path: slice.worktree_path.clone(),
        branch: slice.branch.clone(),
        base_branch: slice.base_branch.clone(),
        pr_url: slice.pr_url.clone(),
    }
}

/// The section the imported plan leads with: where it came from, what came with it, and
/// what did not. It is part of the plan rather than a row in ai-team's own tables so the
/// provenance travels with the document a person actually reads.
pub(super) fn provenance(
    plan: &SourcePlan,
    source: &PlanSource,
    report: &Report,
    at: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Imported into AI Team on {at} from the standalone ai-planner database at `{}` \
         ({}, sha256 `{}`, schema {}).\n",
        source.path,
        size(i64::try_from(source.bytes).unwrap_or(i64::MAX)),
        source.digest,
        source.schema_version
    );
    let _ = writeln!(
        out,
        "AI Team read a private read-only copy of that file. It does not write to it, \
         register it, or keep the two in step: from here on this plan and the original are \
         separate documents.\n"
    );
    header(&mut out, plan);
    evidence(&mut out, report);
    handoffs(&mut out, plan);
    files(&mut out, plan);

    if !report.preserved.is_empty() {
        let _ = writeln!(out, "### Carried over\n");
        for line in &report.preserved {
            let _ = writeln!(out, "- {line}");
        }
        out.push('\n');
    }
    let _ = writeln!(out, "### Not carried over\n");
    for line in &report.warnings {
        let _ = writeln!(out, "- {line}");
    }
    out
}

fn header(out: &mut String, plan: &SourcePlan) {
    let _ = writeln!(out, "| Original | Value |");
    let _ = writeln!(out, "| --- | --- |");
    let mut row = |name: &str, value: String| {
        let _ = writeln!(out, "| {name} | {} |", cell(&value));
    };
    row("Plan", format!("`{}` (id {})", plan.slug, plan.id));
    row(
        "Repository",
        format!("{} (key `{}`)", plan.repo_name, plan.repo_key),
    );
    if let Some(path) = &plan.repo_main_path {
        row("Checkout", format!("`{path}`"));
    }
    row("Status", plan.status.to_string());
    row("Created", plan.created_at.clone());
    row("Last updated", plan.updated_at.clone());
    if let Some(owner) = &plan.owner {
        row("Owner", owner.clone());
    }
    if let Some(ticket) = &plan.ticket_key {
        row("Ticket", ticket.clone());
    }
    if let Some(url) = &plan.ticket_url {
        row("Ticket URL", url.clone());
    }
    if let Some(branch) = &plan.base_branch {
        row("Base branch", format!("`{branch}`"));
    }
    if let Some(path) = &plan.source_path {
        row("Written from", format!("`{path}`"));
    }
    out.push('\n');
}

fn evidence(out: &mut String, report: &Report) {
    if report.evidence.is_empty() {
        return;
    }
    let _ = writeln!(out, "### Execution evidence, recorded not re-created\n");
    let _ = writeln!(
        out,
        "These slices were claimed, branched or delivered somewhere else. The details are kept \
         here as history: no claim, lease, branch or pull request was carried into AI Team, and \
         nothing below is authority to build or publish.\n"
    );
    let _ = writeln!(
        out,
        "| Slice | Status | Claimed by | At | Worktree | Branch | Base | Pull request |"
    );
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- | --- |");
    for slice in &report.evidence {
        let _ = writeln!(
            out,
            "| {} {} | {} | {} | {} | {} | {} | {} | {} |",
            cell(&slice.key),
            cell(&slice.title),
            slice.status,
            cell(slice.claimed_by.as_deref().unwrap_or("")),
            cell(slice.claimed_at.as_deref().unwrap_or("")),
            cell(slice.worktree_path.as_deref().unwrap_or("")),
            cell(slice.branch.as_deref().unwrap_or("")),
            cell(slice.base_branch.as_deref().unwrap_or("")),
            cell(slice.pr_url.as_deref().unwrap_or(""))
        );
    }
    out.push('\n');
}

fn handoffs(out: &mut String, plan: &SourcePlan) {
    if plan.handoffs.is_empty() {
        return;
    }
    let _ = writeln!(out, "### Handoffs carried over as history\n");
    let _ = writeln!(
        out,
        "Each one is the state of a checkout at the moment somebody stopped working in it, in \
         the other tool. The gates it records were green there and then, not here and now.\n"
    );
    let _ = writeln!(out, "| When | Who | Worktree | Branch | Commit | Gates |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- |");
    for handoff in &plan.handoffs {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            cell(&handoff.at),
            cell(handoff.actor.as_deref().unwrap_or("")),
            cell(&handoff.worktree_path),
            cell(handoff.branch.as_deref().unwrap_or("")),
            cell(handoff.head_sha.as_deref().unwrap_or("")),
            cell(handoff.gates_json.as_deref().unwrap_or(""))
        );
    }
    out.push('\n');
}

fn files(out: &mut String, plan: &SourcePlan) {
    if plan.file_imports.is_empty() {
        return;
    }
    let _ = writeln!(out, "### Files the original was imported from\n");
    let _ = writeln!(out, "| File | sha256 | Bytes | Imported |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    for file in &plan.file_imports {
        let _ = writeln!(
            out,
            "| `{}` | `{}` | {} | {} |",
            cell(&file.source_path),
            cell(&file.sha256),
            file.bytes,
            cell(&file.imported_at)
        );
    }
    out.push('\n');
}

/// A table cell cannot contain a bar or a newline and still be a table cell.
fn cell(value: &str) -> String {
    value.replace('|', r"\|").replace(['\n', '\r'], " ")
}

/// Integer arithmetic on purpose: a float here would be a rounding question in a sentence
/// whose whole job is to be exact about what was read.
fn size(bytes: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = 1024 * KIB;
    let tenths = |unit: i64| format!("{}.{}", bytes / unit, (bytes % unit) * 10 / unit);
    match bytes {
        ..KIB => format!("{bytes} bytes"),
        KIB..MIB => format!("{} KiB", tenths(KIB)),
        _ => format!("{} MiB", tenths(MIB)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_sizes_without_a_rounding_question() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1023), "1023 bytes");
        assert_eq!(size(1024), "1.0 KiB");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(12 * 1024 * 1024 + 512 * 1024), "12.5 MiB");
    }

    #[test]
    fn a_table_cell_survives_a_path_with_a_bar_in_it() {
        assert_eq!(cell("a|b"), r"a\|b");
        assert_eq!(cell("two\nlines"), "two lines");
    }
}
