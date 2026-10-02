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
    file: File,
    pub(crate) db: PathBuf,
    pub(crate) run: i64,
    pub(crate) delivery: Option<i64>,
    epoch: AtomicI64,
    uncertain: AtomicBool,
}
impl Ownership {
    pub(crate) fn acquire(db: &Path, run: i64) -> Result<Arc<Self>> {
        Self::acquire_scoped(db, run, None)
    }
    pub(crate) fn acquire_delivery(db: &Path, run: i64, delivery: i64) -> Result<Arc<Self>> {
        Self::acquire_scoped(db, run, Some(delivery))
    }
    fn acquire_scoped(db: &Path, run: i64, delivery: Option<i64>) -> Result<Arc<Self>> {
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
        if try_lock_briefly(&file)? {
            // Reject filesystems whose same-process locks are reentrant or where
            // closing a probe would release the owner's lock. SQLite and these
            // receipts require local, independent file-description semantics.
            let probe = OpenOptions::new().read(true).write(true).open(&path)?;
            match probe.try_lock() {
                    Err(TryLockError::WouldBlock) => Ok(Arc::new(Self { file, db: db.canonicalize()?, run, delivery, epoch: AtomicI64::new(0), uncertain: AtomicBool::new(false) })),
                    Err(TryLockError::Error(error)) => Err(error.into()),
                    Ok(()) => Err(Error::invalid("controller storage does not provide independent file locks; use a local data directory")),
                }
        } else {
            Err(Error::invalid(
                "this build still has a controller or draining worker, or a contended lock probe; wait before recovering",
            ))
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

impl Drop for Ownership {
    fn drop(&mut self) {
        // Closing the last Rust receipt is not necessarily the last OS descriptor:
        // a concurrent fork can inherit it until exec, even with CLOEXEC. Unlock
        // explicitly only here, after every controller/worker receipt has gone.
        if let Err(error) = self.file.unlock() {
            eprintln!("could not unlock team controller {}: {error}", self.run);
        }
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
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        // A newly reserved execution may not have opened its lock yet. This is
        // task liveness only, never proof that children are drained.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if try_lock_briefly(&file)? {
        file.unlock()?;
        Ok(false)
    } else {
        Ok(true)
    }
}

// A read-only liveness probe briefly takes the same exclusive lock. Smooth that
// contention (also across windows/processes) before reporting a live controller.
// Lasting contention still fails closed; this is not a wait for task completion.
fn try_lock_briefly(file: &File) -> Result<bool> {
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(true),
            Err(TryLockError::WouldBlock)
                if started.elapsed() < std::time::Duration::from_millis(50) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn an_inherited_descriptor_is_not_a_controller_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("team.sqlite");
        std::fs::write(&db, "fixture").unwrap();
        let owner = Ownership::acquire(&db, 1).unwrap();
        // Make the fork-to-exec inheritance window deterministic: the unrelated
        // child retains this descriptor after exec, but owns no Rust receipt.
        rustix::io::fcntl_setfd(&owner.file, rustix::io::FdFlags::empty()).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("2")
            .spawn()
            .unwrap();
        rustix::io::fcntl_setfd(&owner.file, rustix::io::FdFlags::CLOEXEC).unwrap();
        drop(owner);
        let still_held = held(&lock_path(&db, 1).unwrap()).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            !still_held,
            "an unrelated inherited fd retained a dead controller's lock"
        );
    }

    #[test]
    fn a_short_liveness_probe_does_not_fail_a_claim_or_discovery() {
        for claim in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("team.sqlite");
            std::fs::write(&db, "fixture").unwrap();
            let path = lock_path(&db, 1).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.try_lock().unwrap();
            let (ready, release) = std::sync::mpsc::channel();
            let probe = std::thread::spawn(move || {
                release.recv().unwrap();
                std::thread::sleep(std::time::Duration::from_millis(5));
                file.unlock().unwrap();
            });
            ready.send(()).unwrap();
            let succeeded = if claim {
                Ownership::acquire(&db, 1).is_ok()
            } else {
                !held(&path).unwrap()
            };
            probe.join().unwrap();
            assert!(
                succeeded,
                "a brief liveness probe was mistaken for a controller (claim={claim})"
            );
        }
    }

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
