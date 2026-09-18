//! `ait today` - the same answer the window gives, in the terminal.
//!
//! One ranking, computed in core and tested there. A second one here would be a second
//! answer to the same question, and the two would drift the first time either changed.

use ai_team_core::{Result, Store, Urgency};

/// Wide enough for the longest label, so the titles line up in a column.
const LABEL: usize = 9;

pub(crate) async fn run() -> Result<()> {
    let store = Store::open_default()?;

    let (mut items, checkouts) = {
        let from_store = ai_team_core::today_from_store(&store)?;
        let checkouts: Vec<(String, String)> = store
            .projects()?
            .into_iter()
            .filter_map(|project| {
                let repo = store
                    .project_repos(project.id)
                    .ok()?
                    .into_iter()
                    .find_map(|repo| repo.main_path)?;
                Some((project.slug, repo))
            })
            .collect();
        (from_store, checkouts)
    };

    for (slug, repo) in checkouts {
        // Best effort: a checkout that has moved, or one with no plan yet, must not
        // empty the list for every other project.
        let planner = ai_team_core::Planner::at(repo);
        let Ok(questions) = planner.open_questions().await else {
            continue;
        };
        for question in questions {
            items.push(ai_team_core::from_question(
                &slug,
                &question.body,
                question.asked_at,
            ));
        }
        // Work an agent finished and left `in_review` is the commonest thing waiting
        // after a run, and it lives on the plan rather than in ai-team's own tables.
        for slice in planner.slices().await.unwrap_or_default() {
            if let Some(item) =
                ai_team_core::from_slice(&slug, &slice.key, &slice.title, &slice.status)
            {
                items.push(item);
            }
        }
    }

    let items = ai_team_core::rank_today(items);
    if items.is_empty() {
        println!("Nothing is waiting on you.");
        return Ok(());
    }

    for (index, item) in items.iter().enumerate() {
        // The first line is set apart, because a ranked list whose top item looks like
        // every other line is a list people read top to bottom anyway.
        let mark = if index == 0 { ">" } else { " " };
        println!(
            "{mark} {:<LABEL$}  {:<10}  {}",
            item.urgency.as_str(),
            item.project.as_deref().unwrap_or("-"),
            item.title
        );
        if let Some(detail) = &item.detail {
            println!("  {:<LABEL$}  {:<10}  {detail}", "", "");
        }
    }

    if items[0].urgency == Urgency::InFlight {
        // Worth saying: everything on the list is already moving.
        println!("\nNothing needs you - the team is working.");
    }
    Ok(())
}
