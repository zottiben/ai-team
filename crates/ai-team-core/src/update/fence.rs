//! Never unlink this inode: all processes sharing the database use it for admission.
use crate::{Error, Result};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub(crate) fn open(root: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    Ok(options.open(root.join(".app-update.lock"))?)
}

/// Close alone is insufficient: a concurrent fork may still hold a duplicate of
/// this open file description before exec. Ownership ends at this guard, not there.
#[derive(Debug)]
pub(crate) struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            eprintln!("ait: could not release the application update fence: {error}");
        }
    }
}

pub(crate) fn exclusive(root: &Path) -> Result<Lock> {
    let file = open(root)?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lock(file)),
            Err(error) if started.elapsed() >= std::time::Duration::from_secs(5) => {
                return Err(Error::invalid(format!(
                    "AI Team state is busy; finish active work and close app terminals or sign-in sessions before updating; no work was stopped ({error})"
                )));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
}

pub(crate) fn shared(db: &Path) -> Result<Option<Lock>> {
    if db == Path::new(":memory:") {
        return Ok(None);
    }
    let path = if db.exists() {
        db.canonicalize()?
    } else {
        db.to_path_buf()
    };
    let root = path
        .parent()
        .ok_or_else(|| Error::invalid("the database has no directory"))?;
    let file = open(root)?;
    file.try_lock_shared().map_err(|_| {
        Error::invalid("AI Team is updating; wait for it to finish before changing state")
    })?;
    let lock = Lock(file);
    if root.join(".app-update-pending.json").exists() {
        return Err(Error::invalid(
            "an application update needs inspection before changing state",
        ));
    }
    Ok(Some(lock))
}

pub(crate) fn active(root: &Path) -> Result<bool> {
    match File::open(root.join(".app-update.lock")) {
        Ok(file) => {
            if file.try_lock_shared().is_err() {
                Ok(true)
            } else {
                drop(Lock(file));
                Ok(false)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_update_unlocks_even_if_a_spawn_inherited_its_file_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let held = exclusive(root.path()).unwrap();
        let inherited = held.0.try_clone().unwrap();
        assert!(active(root.path()).unwrap());
        drop(held);
        assert!(
            !active(root.path()).unwrap(),
            "a duplicate descriptor must not extend the completed update's ownership"
        );
        drop(inherited);
    }
}
