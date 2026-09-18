//! Performance budgets, as tests rather than as intentions.
//!
//! Two things in ai-team are on a hot path and both degrade quietly. The event stream is
//! polled every second by every open window, so a query that is fine at a hundred events
//! and linear at a hundred thousand turns a long day's work into a warm laptop. And a run
//! detail is re-read on every tick, so it has to stay bounded no matter how talkative the
//! agents were.
//!
//! Written as budgets with headroom rather than as tight timings: a CI runner under load
//! is several times slower than a laptop, and a test that fails when the runner is busy
//! gets muted, which is worse than having no test. What these catch is an algorithm
//! changing shape - a scan appearing where an index was - not a millisecond of drift.

use std::time::{Duration, Instant};

use ai_team_core::{EventKind, NewEvent, NewProject, RunTrigger, Store};

/// A store with `count` events on one run.
fn loaded(count: usize) -> (Store, i64) {
    let mut store = Store::memory().unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    store.seed_default_team(project.id).unwrap();
    let run = store
        .create_run(project.id, "ship it", RunTrigger::Manual)
        .unwrap();

    for index in 0..count {
        store
            .append_event(
                run.id,
                NewEvent::new(EventKind::Step, format!("step {index}")),
            )
            .unwrap();
    }
    (store, run.id)
}

#[test]
fn the_event_stream_poll_does_not_get_slower_as_events_pile_up() {
    // Every open window runs this once a second. If it ever becomes a scan, the cost
    // grows with the length of the day and nothing says so - the window just gets
    // warmer.
    let (small, _) = loaded(100);
    let (large, _) = loaded(50_000);

    let time = |store: &Store| {
        // Warmed first: the first query on a connection pays for preparing it, which is
        // not what is being measured.
        for _ in 0..10 {
            store.latest_event_id().unwrap();
        }
        let started = Instant::now();
        for _ in 0..200 {
            store.latest_event_id().unwrap();
        }
        started.elapsed()
    };

    let quick = time(&small);
    let slow = time(&large);

    // `MAX(id)` on an INTEGER PRIMARY KEY is a lookup, not a scan, so five hundred times
    // the rows must not be measurably more work. Ten times the headroom, because this is
    // about shape rather than milliseconds.
    assert!(
        slow < quick.max(Duration::from_micros(200)) * 10,
        "polling got slower with more events: {quick:?} at 100, {slow:?} at 50k"
    );
}

#[test]
fn reading_a_runs_events_is_bounded_however_talkative_the_agents_were() {
    // The window re-reads this on every tick. A run that emitted fifty thousand events -
    // which a long agent session genuinely does - must not hand fifty thousand rows to a
    // surface that draws the last few dozen.
    let (store, run) = loaded(50_000);

    let started = Instant::now();
    let page = store.events(run, None, 200).unwrap();
    let took = started.elapsed();

    assert_eq!(page.len(), 200, "the limit has to be applied in SQL");
    assert!(
        took < Duration::from_millis(250),
        "reading a page of events took {took:?}"
    );

    // Paging is by cursor, not by offset. An OFFSET deep into a large table re-walks
    // everything before it, so the last page of a long run would be the slowest - which
    // is precisely the page somebody watching a run is on.
    let last_seen = page.last().unwrap().id;
    let deep = Instant::now();
    let further = store.events(run, Some(49_000), 200).unwrap();
    let deep = deep.elapsed();

    assert_eq!(further.len(), 200);
    assert!(further[0].id > last_seen);
    assert!(
        deep < Duration::from_millis(250),
        "reading deep into the run took {deep:?}"
    );
}

#[test]
fn appending_events_stays_flat() {
    // Ingesting eve's stream appends continuously for the length of a turn. A per-insert
    // cost that grows with the table would make a long turn slow down as it went, which
    // reads as the model getting slower.
    let (mut store, run) = loaded(20_000);

    let started = Instant::now();
    for index in 0..1_000 {
        store
            .append_event(run, NewEvent::new(EventKind::Step, format!("late {index}")))
            .unwrap();
    }
    let took = started.elapsed();

    assert!(
        took < Duration::from_millis(2_000),
        "appending a thousand events onto twenty thousand took {took:?}"
    );
}
