//! One error type for the whole store.
//!
//! Every fallible call in this crate returns `Result<T>`. Callers upstream - the CLI,
//! the UI server, the desktop shell - map it to their own presentation; none of them
//! should have to match on a `rusqlite` or `std::io` type to find out what went wrong.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    /// Raised when neither `$HOME` nor the platform's home directory is readable.
    /// Rare, but it happens inside sandboxes and stripped-down CI images, and a bare
    /// "not found" there sends people hunting in the wrong place.
    #[error("cannot locate a home directory - set $HOME, or pass an explicit path")]
    NoHome,

    #[error("{path} is not usable: {reason}")]
    UnusablePath { path: PathBuf, reason: String },
}
