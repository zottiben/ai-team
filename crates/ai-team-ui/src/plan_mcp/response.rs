//! Bounded planning answers. Pi measures a tool result against its own output guard and
//! removes the whole thing when it is over, so a board that outgrew one response reached
//! the seat as nothing at all: no plan, no revision, and a turn that could only stop.
//!
//! Two properties keep that from being traded away. The budget is on the **serialized**
//! response, because escaping is what pushed a payload that looked safe over the line.
//! And nothing is ever cut: what does not fit is reported as partial, with the exact calls
//! that fetch the rest, rather than silently losing a section, a slice or a log entry.

use ai_team_core::planning::ChatPlan;
use ai_team_core::{Error, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Pi refuses a tool result over 50KB. Stay well inside it: the answer is only part of
/// what a turn carries, and a response that only just fits is one the next slice loses.
const BUDGET: usize = 24_000;

/// Plan text in one page when the caller does not say. Escaping shrinks it further.
const DEFAULT_LIMIT: usize = 16_000;

/// A larger ask cannot be honoured, so it is refused rather than quietly trimmed.
pub(super) const MAX_LIMIT: usize = BUDGET;

/// What `read_plan` takes. Unknown fields are rejected, so a chat, project or plan
/// override in an agent's arguments is a refusal and never a wider read.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    expect_revision: i64,
    offset: usize,
    limit: Option<usize>,
}

/// The answer to `get_plan` and to every mutation: the whole snapshot while it fits, and
/// otherwise a descriptor that still carries the revision, the scope and the plan's
/// identity. An existing plan is never reported as absent - `bundle` is left out rather
/// than set to null, and `plan_present` says what is really there.
pub(super) fn snapshot(plan: &ChatPlan) -> Result<Value> {
    let mut whole = serde_json::to_value(plan)?;
    if let Some(object) = whole.as_object_mut() {
        object.insert("partial".into(), false.into());
    }
    if within_budget(&whole)? {
        return Ok(whole);
    }
    Ok(descriptor(plan, document(plan)?.len()))
}

/// One page of the plan, cut from a fixed document by UTF-8 byte boundaries. A page is
/// bounded at construction; it is never re-compacted into a descriptor, which would leave
/// the caller paging a response that is itself paged.
pub(super) fn page(plan: &ChatPlan, request: Page) -> Result<Value> {
    if request.expect_revision != plan.revision {
        return Err(Error::invalid(
            "this plan changed since you read it; call get_plan again and page from offset 0",
        ));
    }
    let limit = match request.limit {
        None => DEFAULT_LIMIT,
        Some(limit) if limit == 0 || limit > MAX_LIMIT => {
            return Err(Error::invalid(format!(
                "limit reads 1-{MAX_LIMIT} bytes of plan text"
            )))
        }
        Some(limit) => limit,
    };
    let text = document(plan)?;
    if request.offset > text.len() {
        return Err(Error::invalid(format!(
            "this plan is {} bytes; offset must be one this tool returned",
            text.len()
        )));
    }
    if !text.is_char_boundary(request.offset) {
        return Err(Error::invalid(
            "offset fell inside a character; use exactly the next_offset a page returned",
        ));
    }
    fit(plan, &text, request.offset, limit)
}

/// The one stable representation pages are cut from. Pretty JSON of the exact snapshot,
/// so concatenating every page in order reproduces it byte for byte.
fn document(plan: &ChatPlan) -> Result<String> {
    Ok(serde_json::to_string_pretty(plan)?)
}

fn within_budget(value: &Value) -> Result<bool> {
    Ok(serde_json::to_string(value)?.len() <= BUDGET)
}

/// The largest page that still serializes inside the budget. Searching rather than
/// estimating, because a character's escaped cost ranges from one byte to six and a guess
/// either wastes most of the page or overshoots the guard that dropped the snapshot.
fn fit(plan: &ChatPlan, text: &str, offset: usize, limit: usize) -> Result<Value> {
    let (mut fits, mut over) = (offset, floor(text, offset.saturating_add(limit)));
    while fits < over {
        let middle = ceiling(text, fits + (over - fits).div_ceil(2)).min(over);
        if within_budget(&paged(plan, text, offset, middle))? {
            fits = middle;
        } else {
            over = floor(text, middle - 1);
        }
    }
    if fits == offset && offset < text.len() {
        return Err(Error::invalid(
            "a single character of this plan does not fit one response",
        ));
    }
    Ok(paged(plan, text, offset, fits))
}

fn paged(plan: &ChatPlan, text: &str, offset: usize, end: usize) -> Value {
    json!({
        "chat_id": plan.chat_id,
        "project_id": plan.project_id,
        "revision": plan.revision,
        "partial": end < text.len(),
        "format": "json-pretty",
        "encoding": "utf-8",
        "total_bytes": text.len(),
        "offset": offset,
        "next_offset": (end < text.len()).then_some(end),
        "content": &text[offset..end],
    })
}

/// The identity and shape of a plan too large to send, with the calls that fetch it.
fn descriptor(plan: &ChatPlan, total_bytes: usize) -> Value {
    let mut out = Map::new();
    out.insert("chat_id".into(), plan.chat_id.into());
    out.insert("project_id".into(), plan.project_id.into());
    out.insert("revision".into(), plan.revision.into());
    out.insert("partial".into(), true.into());
    out.insert("plan_present".into(), plan.bundle.is_some().into());
    out.insert("total_bytes".into(), total_bytes.into());
    if let Some(bundle) = &plan.bundle {
        out.insert(
            "plan".into(),
            json!({
                "id": bundle.plan.id,
                "slug": bundle.plan.slug,
                "title": bundle.plan.title.chars().take(240).collect::<String>(),
                "status": bundle.plan.status.as_str(),
                "updated_at": bundle.plan.updated_at,
            }),
        );
        out.insert(
            "counts".into(),
            json!({
                "sources": bundle.sources.len(),
                "sections": bundle.sections.len(),
                "decisions": bundle.decisions.len(),
                "slices": bundle.slices.len(),
                "questions": bundle.questions.len(),
                "gotchas": bundle.gotchas.len(),
                "log": bundle.log.len(),
            }),
        );
    }
    out.insert(
        "read_plan".into(),
        json!({"expect_revision": plan.revision, "offset": 0}),
    );
    out.insert(
        "guidance".into(),
        Value::String(format!(
            "This plan is whole and unchanged; it is only too large to send in one response, \
         so none of it is included here. Call read_plan with expect_revision={revision} \
         and offset=0, then call it again with the next_offset each page returns, once \
         each, until next_offset is null. Concatenating those pages in order gives the \
         {total_bytes} bytes of pretty JSON for this exact snapshot. Writing still works \
         now: pass expect_revision={revision} to any mutation.",
            revision = plan.revision
        )),
    );
    Value::Object(out)
}

fn floor(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceiling(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_team_core::planning::PlanBundle;

    fn plan(bundle: Option<PlanBundle>, revision: i64) -> ChatPlan {
        ChatPlan {
            chat_id: 4,
            project_id: 2,
            revision,
            bundle,
        }
    }

    #[test]
    fn an_empty_chat_still_answers_whole_and_says_it_is_not_partial() {
        let answer = snapshot(&plan(None, 0)).unwrap();
        assert_eq!(answer["partial"], false);
        assert!(answer["bundle"].is_null());
        assert_eq!(answer["revision"], 0);
    }

    #[test]
    fn a_page_is_bounded_by_the_serialized_response_not_the_raw_slice() {
        // Every byte of this text escapes to six, so a page budgeted on the raw slice
        // would serialize far over the guard that removed the snapshot.
        let text = "\u{7}".repeat(40_000);
        let value = fit(&plan(None, 3), &text, 0, MAX_LIMIT).unwrap();
        assert!(serde_json::to_string(&value).unwrap().len() <= BUDGET);
        let content = value["content"].as_str().unwrap();
        assert!(!content.is_empty() && content.len() < MAX_LIMIT);
        assert_eq!(value["next_offset"].as_u64().unwrap(), content.len() as u64);
    }

    #[test]
    fn pages_split_on_character_boundaries_and_rejoin_exactly() {
        let text = "é漢字🛠 ".repeat(4_000);
        let mut rebuilt = String::new();
        let mut offset = Some(0usize);
        while let Some(at) = offset {
            let value = fit(&plan(None, 1), &text, at, 300).unwrap();
            let content = value["content"].as_str().unwrap();
            assert!(!content.is_empty());
            rebuilt.push_str(content);
            offset = value["next_offset"]
                .as_u64()
                .map(|next| usize::try_from(next).unwrap());
        }
        assert_eq!(rebuilt, text);
    }
}
