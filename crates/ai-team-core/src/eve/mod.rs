//! Talking to eve: its event stream, and getting it into the database.
//!
//! Supervision - building, starting and driving a process - is M1-S4. What is here is
//! the part that outlives any particular way of running eve: the shape of its events,
//! and the rules for ingesting them exactly once.

mod event;
mod ingest;

pub use event::{Disposition, StreamEvent, StreamMeta};
pub use ingest::{Ingested, TerminalState};
