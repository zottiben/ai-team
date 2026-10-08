use crate::{Error, Result};
use std::{fs, path::Path};

pub(super) fn plist(bundle: &Path, key: &str) -> Result<String> {
    let body = fs::read_to_string(bundle.join("Contents/Info.plist"))?;
    body.split_once(&format!("<key>{key}</key>"))
        .and_then(|(_, tail)| tail.trim_start().strip_prefix("<string>"))
        .and_then(|tail| tail.split_once("</string>"))
        .map(|(value, _)| value.to_string())
        .ok_or_else(|| Error::invalid(format!("{} has no {key}", bundle.display())))
}

pub(super) fn fingerprint(path: &Path) -> Result<String> {
    if !path.try_exists()? {
        return Ok("missing".into());
    }
    let mut bytes = Vec::new();
    hash_tree(path, Path::new(""), &mut bytes)?;
    Ok(super::super::sha256(&bytes))
}

fn hash_tree(path: &Path, relative: &Path, bytes: &mut Vec<u8>) -> Result<()> {
    let name = relative.to_string_lossy();
    bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    let meta = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        bytes.extend_from_slice(&meta.permissions().mode().to_le_bytes());
    }
    if meta.file_type().is_symlink() {
        bytes.push(b'L');
        let target = fs::read_link(path)?;
        let target = target.to_string_lossy();
        bytes.extend_from_slice(&(target.len() as u64).to_le_bytes());
        bytes.extend_from_slice(target.as_bytes());
    } else if meta.is_dir() {
        bytes.push(b'D');
        let mut children = fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        bytes.extend_from_slice(&(children.len() as u64).to_le_bytes());
        for name in children {
            hash_tree(&path.join(&name), &relative.join(&name), bytes)?;
        }
    } else if meta.is_file() {
        bytes.push(b'F');
        bytes.extend_from_slice(super::super::sha256(&fs::read(path)?).as_bytes());
    } else {
        return Err(Error::invalid("an update target contains a special file"));
    }
    bytes.push(0);
    Ok(())
}

/// Relocatable program bytes only. Framework symlinks may stay inside their bundle,
/// but neither a root link nor a link to another installation grants authority.
pub(super) fn program(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(Error::invalid("a release program must not be a symlink"));
    }
    program_tree(path, &path.canonicalize()?)
}

fn program_tree(path: &Path, root: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        if fs::read_link(path)?.is_absolute() || !path.canonicalize()?.starts_with(root) {
            return Err(Error::invalid(
                "a release symlink points outside its program",
            ));
        }
    } else if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            program_tree(&entry?.path(), root)?;
        }
    } else if !meta.is_file() {
        return Err(Error::invalid("a release contains a special file"));
    }
    Ok(())
}

pub(super) fn copy(source: &Path, destination: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
        #[cfg(not(unix))]
        return Err(Error::invalid("symlink backups are unsupported here"));
    } else if meta.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, meta.permissions())?;
    } else if meta.is_file() {
        fs::copy(source, destination)?;
    } else {
        return Err(Error::invalid("a backup contains a special file"));
    }
    Ok(())
}

pub(super) fn resolved(path: &Path) -> Result<std::path::PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.symlink_metadata().is_ok() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid("cannot resolve an update path"))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::invalid("an update path has no name"))?;
    Ok(resolved(parent)?.join(name))
}

pub(super) fn record(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
