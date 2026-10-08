use super::{files, UpdateInstallation, UpdateManager};
use crate::{Error, Result};
use std::time::SystemTime;

/// Held across ordinary mutations, and across authentication terminal lifetimes.
/// The updater waits briefly for ordinary writes, but never stops work to acquire it.
#[derive(Debug)]
pub struct UpdatePermit {
    _file: Option<super::super::fence::Lock>,
}

pub(super) fn stamp(installation: &UpdateInstallation) -> Result<Option<SystemTime>> {
    match std::fs::metadata(installation.data_dir.join("install-method")) {
        Ok(metadata) => Ok(Some(metadata.modified()?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl UpdatePermit {
    /// Explicit CLI configuration writes use the same state fence without discovering
    /// programs or probing anything on the operator's PATH.
    pub fn acquire(db: &std::path::Path) -> Result<Self> {
        let resolved = files::resolved(db)?;
        let parent = resolved
            .parent()
            .ok_or_else(|| Error::invalid("application state has no parent"))?;
        if !parent.exists() {
            files::private_dir(parent)?;
        }
        let permit = super::super::fence::shared(db)?;
        if db.exists()
            && crate::Store::installed_application_version(db)?
                .is_some_and(|v| super::super::is_newer(&v, crate::current_version()))
        {
            return Err(Error::invalid(
                "AI Team was updated; restart this process before continuing",
            ));
        }
        Ok(Self { _file: permit })
    }
}

impl UpdateManager {
    pub fn admit(&self) -> Result<UpdatePermit> {
        let Some(installation) = &self.0.installation else {
            return Ok(UpdatePermit { _file: None });
        };
        let fallback = installation.data_dir.join("team.db");
        let db = installation.database.as_deref().unwrap_or(&fallback);
        let permit = UpdatePermit::acquire(db)?;
        let initial = self.0.initial_update.as_ref().map_err(Error::invalid)?;
        if stamp(installation)? != *initial {
            return Err(Error::invalid(
                "the installed programs changed; restart this process before continuing",
            ));
        }
        Ok(permit)
    }
}
