use ai_planner_core as planner;

use crate::planning::PlanAction;
use crate::{Error, Result};

pub(super) fn text(value: &str, name: &str, limit: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > limit {
        return Err(Error::invalid(format!("{name} needs 1–{limit} bytes")));
    }
    Ok(())
}

fn key(value: &str) -> Result<()> {
    text(value, "key", 80)?;
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(Error::invalid(
            "keys contain letters, numbers, hyphens or underscores",
        ));
    }
    Ok(())
}

fn scope(body: &str, touches: &[String]) -> Result<String> {
    text(body, "scope", 32_000)?;
    if touches.is_empty() || touches.len() > 100 {
        return Err(Error::invalid(
            "a slice must name the repository paths it touches",
        ));
    }
    for path in touches {
        if path.trim().is_empty()
            || path.starts_with('/')
            || path.contains(['\n', '\r', '\\', '\0', ','])
            || path.split('/').any(|part| part == "..")
            || path.contains(':')
            || path.len() > 1024
        {
            return Err(Error::invalid(
                "Touches paths must stay relative to this checkout",
            ));
        }
    }
    Ok(format!(
        "{}\n\nTouches: {}",
        body.trim(),
        touches.join(", ")
    ))
}

fn write_slice(store: &mut planner::Store, plan_id: i64, action: PlanAction) -> Result<()> {
    let updating = matches!(action, PlanAction::UpdateSlice { .. });
    let (PlanAction::AddSlice {
        key: name,
        title,
        scope: body,
        touches,
        demo,
        ..
    }
    | PlanAction::UpdateSlice {
        key: name,
        title,
        scope: body,
        touches,
        demo,
        ..
    }) = action
    else {
        return Err(Error::invalid("expected a slice edit"));
    };
    key(&name)?;
    text(&title, "title", 240)?;
    text(&demo, "verification", 16_000)?;
    let scope_md = Some(scope(&body, &touches)?);
    if updating {
        let slice = store.require_slice(plan_id, &name)?;
        store.update_slice(
            &slice,
            planner::SliceUpdate {
                title: Some(title),
                scope_md,
                demo_md: Some(demo),
                ..Default::default()
            },
        )?;
    } else {
        store.add_slice(planner::NewSlice {
            plan_id,
            key: name,
            title,
            scope_md,
            demo_md: Some(demo),
            status: Some(planner::Status::Ready),
            ..Default::default()
        })?;
    }
    Ok(())
}

pub(super) fn apply(
    store: &mut planner::Store,
    plan: &planner::Plan,
    action: PlanAction,
) -> Result<()> {
    match action {
        PlanAction::SetPlanStatus { status, .. } => {
            store.set_plan_status(plan, status)?;
        }
        PlanAction::WriteSection {
            key: name,
            title,
            body,
            ..
        } => {
            key(&name)?;
            text(&title, "title", 240)?;
            if body.len() > 64_000 {
                return Err(Error::invalid("a section must be at most 64000 bytes"));
            }
            store.set_section(
                plan.id,
                planner::SectionWrite {
                    key: &name,
                    title: Some(&title),
                    body: &body,
                    ..Default::default()
                },
            )?;
        }
        action @ (PlanAction::AddSlice { .. } | PlanAction::UpdateSlice { .. }) => {
            write_slice(store, plan.id, action)?;
        }
        PlanAction::SetSliceStatus {
            key,
            status,
            reason,
            ..
        } => {
            if status == planner::Status::Blocked {
                text(reason.as_deref().unwrap_or(""), "blocking reason", 16_000)?;
            }
            let slice = store.require_slice(plan.id, &key)?;
            store.set_slice_status(&slice, status, reason.as_deref())?;
        }
        PlanAction::AddDecision { title, body, .. } => {
            text(&title, "title", 240)?;
            text(&body, "reasoning", 32_000)?;
            store.add_decision(planner::NewDecision {
                plan_id: plan.id,
                title,
                body,
                ..Default::default()
            })?;
        }
        PlanAction::OpenQuestion { body, slice, .. } => {
            text(&body, "question", 16_000)?;
            let slice = slice
                .map(|key| store.require_slice(plan.id, &key))
                .transpose()?;
            store.add_question(plan.id, slice.map(|slice| slice.id), &body)?;
        }
        PlanAction::AnswerQuestion {
            question_id,
            answer,
            ..
        } => {
            text(&answer, "answer", 16_000)?;
            if !store
                .questions(plan.id, true)?
                .iter()
                .any(|question| question.id == question_id)
            {
                return Err(Error::invalid(
                    "that open question does not belong to this chat's plan",
                ));
            }
            store.answer_question(question_id, &answer)?;
        }
        PlanAction::AppendLog { body, slice, .. } => {
            text(&body, "progress note", 32_000)?;
            let slice = slice
                .map(|key| store.require_slice(plan.id, &key))
                .transpose()?;
            store.append_log(planner::NewLog {
                plan_id: plan.id,
                slice_id: slice.map(|slice| slice.id),
                body,
                ..Default::default()
            })?;
        }
        PlanAction::AddGotcha { title, body, .. } => {
            text(&title, "title", 240)?;
            text(&body, "detail", 32_000)?;
            store.add_gotcha(plan.id, &title, &body)?;
        }
        PlanAction::CreatePlan { .. } => {
            return Err(Error::invalid("this chat already has a plan"))
        }
    }
    Ok(())
}
