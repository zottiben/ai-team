//! One error type for the whole crate.
//!
//! Every fallible call returns `Result<T>`. Callers upstream - the CLI, the UI server,
//! the desktop shell - map it to their own presentation; none of them should have to
//! match on a `rusqlite` or `std::io` type to find out what went wrong.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// Raised when neither `$HOME` nor the platform's home directory is readable. Rare,
    /// but it happens inside sandboxes and stripped-down CI images, and a bare "not
    /// found" there sends people hunting in the wrong place.
    #[error("cannot locate a home directory - set $HOME, or pass an explicit path")]
    NoHome,

    #[error("{path} is not usable: {reason}")]
    UnusablePath { path: PathBuf, reason: String },

    #[error("no database at {0} - run `ait init` first")]
    NoDatabase(PathBuf),

    #[error("no project matching {0:?}")]
    NoSuchProject(String),

    #[error("{0:?} matches {1} projects: {2} - name one exactly")]
    AmbiguousProject(String, usize, String),

    #[error("a project {0:?} already exists")]
    DuplicateProject(String),

    #[error("no team {0:?}")]
    NoSuchTeam(String),

    #[error("a team {0:?} already exists on this project")]
    DuplicateTeam(String),

    #[error("project {0:?} has no team - run `ait team seed` first")]
    NoTeam(String),

    #[error("no agent {0:?}")]
    NoSuchAgent(String),

    #[error("this team already has a {0:?} seat")]
    DuplicateAgent(String),

    #[error("no run {0:?}")]
    NoSuchRun(String),

    #[error("no node run {0:?}")]
    NoSuchNodeRun(String),

    #[error("no review {0:?}")]
    NoSuchReview(String),

    #[error("{0}")]
    Invalid(String),
}

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}
