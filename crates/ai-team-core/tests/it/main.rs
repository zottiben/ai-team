//! The integration tests, as one binary.
//!
//! Cargo links every file directly under `tests/` into its own executable, each carrying
//! the whole crate and everything under it. Eleven of them were eleven links on every
//! test build and eleven copies in `target/`; as modules of one they are a single link,
//! and their tests run side by side rather than one binary after another.
//!
//! A new integration test is a new module here, not a new file beside this directory.

mod budgets;
mod case_collisions;
mod diff_against_real_git;
mod isolation;
mod lifecycle;
mod lsp_live;
mod pi_live;
mod release_assets;
mod review_range;
mod review_steering;
mod staging;
