//! Process identity is not task identity. The file lock lives as long as any worker or
//! Pi stop future holding its receipt. Never unlink lock files: that would split owners.
use crate::{Error, Result};
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    },
};

tokio::task_local! { static OWNER: Arc<Ownership>; }

#[derive(Debug)]
pub(crate) struct Ownership {
    _file: File,
    pub(crate) db: PathBuf,
    pub(crate) run: i64,
    epoch: AtomicI64,
    uncertain: AtomicBool,
}
impl Ownership {
    pub(crate) fn acquire(db: &Path, run: i64) -> Result<Arc<Self>> {
        let path = lock_path(db, run)?;
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| Error::invalid("the controller lock has no directory"))?,
        )?;
        if path
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err(Error::invalid("a controller lock must not be a symlink"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => {
                // Reject filesystems whose same-process locks are reentrant or where
                // closing a probe would release the owner's lock. SQLite and these
                // receipts require local, independent file-description semantics.
                let probe = OpenOptions::new().read(true).write(true).open(&path)?;
                match probe.try_lock() {
                    Err(TryLockError::WouldBlock) => Ok(Arc::new(Self { _file: file, db: db.canonicalize()?, run, epoch: AtomicI64::new(0), uncertain: AtomicBool::new(false) })),
                    Err(TryLockError::Error(error)) => Err(error.into()),
                    Ok(()) => Err(Error::invalid("controller storage does not provide independent file locks; use a local data directory")),
                }
            }
            Err(TryLockError::WouldBlock) => Err(Error::invalid(
                "this build still has a controller or draining worker; wait before recovering",
            )),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
    pub(crate) fn bind(&self, store: &crate::Store) -> Result<()> {
        self.epoch
            .store(store.chat_child_epoch(self.run)?, Ordering::SeqCst);
        Ok(())
    }
    pub(crate) fn epoch(&self) -> i64 {
        self.epoch.load(Ordering::SeqCst)
    }
    pub(crate) fn quiescent(&self) -> bool {
        !self.uncertain.load(Ordering::SeqCst)
    }
    pub(crate) fn doubt(&self) {
        self.uncertain.store(true, Ordering::SeqCst);
    }
    pub(crate) fn track<T>(
        self: &Arc<Self>,
        future: impl std::future::Future<Output = T>,
    ) -> impl std::future::Future<Output = T> {
        OWNER.scope(self.clone(), Box::pin(future))
    }
}

pub(crate) fn current() -> Option<Arc<Ownership>> {
    OWNER.try_with(Arc::clone).ok()
}

pub(crate) fn lock_path(db: &Path, run: i64) -> Result<PathBuf> {
    let db = db.canonicalize()?;
    let mut name = db
        .file_name()
        .ok_or_else(|| Error::invalid("the chat database has no name"))?
        .to_os_string();
    name.push(".controllers");
    Ok(db.with_file_name(name).join(format!("run-{run}.lock")))
}

pub(crate) fn held(path: &Path) -> Result<bool> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_task_lock_is_not_a_live_pid_and_survives_cloned_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("team.sqlite");
        std::fs::write(&db, "fixture").unwrap();
        let owner = Ownership::acquire(&db, 1).unwrap();
        let worker = owner.clone();
        assert!(Ownership::acquire(&db, 1).is_err());
        assert!(held(&lock_path(&db, 1).unwrap()).unwrap());
        drop(owner);
        assert!(Ownership::acquire(&db, 1).is_err());
        drop(worker);
        assert!(!held(&lock_path(&db, 1).unwrap()).unwrap());
        assert!(Ownership::acquire(&db, 1).is_ok());
    }
}
