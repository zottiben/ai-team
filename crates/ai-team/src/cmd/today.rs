//! `ait today` - the same answer the window gives, in the terminal.
//!
//! One ranking, computed in core and tested there. A second one here would be a second
//! answer to the same question, and the two would drift the first time either changed.

use ai_team_core::{Result, Store, Urgency};

/// Wide enough for the longest label, so the titles line up in a column.
const LABEL: usize = 9;

pub(crate) async fn run() -> Result<()> {
    // Often asked with neither the window nor the daemon keeping the clock, and a run
    // whose process is gone must not be reported as the team working.
    ai_team_core::settle_abandoned_runs(&ai_team_core::default_db_path()?).await?;
    let store = Store::open_default()?;

    let mut items = ai_team_core::today_from_store(&store)?;
    items.extend(ai_team_core::today_from_plans(ai_team_core::today_checkouts(&store)?).await);

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
