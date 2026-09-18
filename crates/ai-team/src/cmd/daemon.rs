//! `ait daemon` - the clock, without the window.
//!
//! The same loop `ait ui` runs. Two of them at once is the expected arrangement, not a
//! mistake to guard against: claiming a due reminder is one guarded update, so exactly
//! one can win it.

use ai_team_core::Result;

pub(crate) async fn run() -> Result<()> {
    println!("ai-team scheduler running - ctrl-c to stop");
    ai_team_core::serve_schedule(|fired| {
        let what = match (fired.started, fired.run_id, &fired.problem) {
            (true, Some(id), _) => format!("started run #{id}"),
            (true, None, _) => "started a run".into(),
            (false, _, Some(problem)) => format!("could not start: {problem}"),
            (false, _, None) => "announced".into(),
        };
        println!("{}  {} - {what}", ai_team_core::now(), fired.reminder.title);
    })
    .await;
    Ok(())
}
