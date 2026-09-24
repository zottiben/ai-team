//! The clock, kept going.
//!
//! Separate from `schedule`, which is about what a tick does. Both `ait daemon` and
//! `ait ui` call [`serve`]: having the window open should be enough to fire a reminder,
//! and closing it should not stop the clock. Running both at once is safe because
//! claiming is a guarded update, not because of a rule about who may run.
//!
//! Each tick opens its own connection and drops it before awaiting anything. That is
//! forced rather than chosen - a `rusqlite::Connection` is `Send` but not `Sync`, so a
//! future holding one cannot be spawned, and this loop is a spawned task in both
//! processes. Opening SQLite costs about a millisecond every twenty seconds.

use crate::error::Result;
use crate::schedule::{self, Fired};
use crate::store::Store;
use crate::util::now;
use crate::workflow::{self, Request};

/// Loop forever, firing what is due.
pub async fn serve<F>(mut on_fire: F)
where
    F: FnMut(&Fired),
{
    loop {
        match once().await {
            Ok(fired) => {
                for entry in &fired {
                    on_fire(entry);
                }
            }
            // A tick that fails must not stop the clock. The database may be momentarily
            // busy with a run committing, and the next tick is twenty seconds away.
            Err(error) => eprintln!("scheduler: {error}"),
        }
        tokio::time::sleep(schedule::TICK).await;
    }
}

/// One tick: claim in a synchronous window, then act with no database open.
pub async fn once() -> Result<Vec<Fired>> {
    let (claimed, projects, notifications) = {
        let mut store = Store::open_default()?;
        let claimed = schedule::claim_due(&mut store, &now())?;
        // Claimed in the same short synchronous window as reminders. `ait ui` and
        // `ait daemon` may both be alive, so native delivery must have one winner.
        let notifications = store.claim_notification_delivery(25)?;
        // Resolved now because `start` below runs with no store to ask.
        let projects: Vec<(i64, String)> = store
            .projects()?
            .into_iter()
            .map(|project| (project.id, project.slug))
            .collect();
        (claimed, projects, notifications)
    };

    for notification in notifications {
        let delivered = schedule::notify(&notification.title, &notification.body).await;
        if let Ok(mut store) = Store::open_default() {
            let _ = store.complete_notification_delivery(notification.id, delivered);
        }
    }

    Ok(schedule::act(claimed, move |project_id, prompt| {
        let slug = projects
            .iter()
            .find(|(id, _)| *id == project_id)
            .map(|(_, slug)| slug.clone());
        async move {
            let slug =
                slug.ok_or_else(|| crate::error::Error::invalid("that project no longer exists"))?;
            let request = Request {
                project: slug,
                // An empty prompt means "build what is ready", as it does everywhere else.
                prompt: (!prompt.is_empty()).then_some(prompt),
                workspace: None,
                plan: None,
                width: None,
                replan: false,
                plan_only: false,
                approval_required: false,
                // Nobody is there to have asked for the default branch.
                branching: workflow::Branching::Fresh,
            };

            // Detached: a run takes minutes and the next tick is twenty seconds away, so
            // the clock must not wait on it. The run records itself in the database every
            // surface is already watching.
            tokio::spawn(async move {
                if let Err(error) = workflow::run(&request, |_| {}).await {
                    eprintln!("scheduled run: {error}");
                }
            });
            // No id: the run is being created on another task, and a number invented
            // here would read as a real one.
            Ok(None)
        }
    })
    .await)
}
