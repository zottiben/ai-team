//! A reproducible catalogue, owned by the binary rather than discovered on the machine.

use std::{fs, os::unix::fs::PermissionsExt};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

include!(concat!(env!("OUT_DIR"), "/toolbox_assets.rs"));

pub const REVISION: &str = include_str!("../../assets/toolbox/REVISION");
pub const NOTICE: &str = include_str!("../../assets/toolbox/NOTICE.md");
const TYPESAFE_LICENSE: &str = include_str!("../../assets/toolbox/skills/typesafe-ai/LICENSE");

#[derive(Debug)]
pub(super) struct Packaged {
    pub catalogue: ai_toolbox_core::Catalogue,
    _directory: tempfile::TempDir,
}

impl Packaged {
    pub(super) fn load() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        for (path, bytes, mode, digest) in ASSETS {
            if format!("{:x}", Sha256::digest(bytes)) != *digest {
                return Err(Error::invalid(format!(
                    "bundled toolbox asset failed verification: {path}"
                )));
            }
            let target = directory.path().join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, bytes)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(*mode))?;
        }
        // Notices travel with the extracted catalogue as well as the binary.
        fs::write(directory.path().join("NOTICE.md"), NOTICE)?;
        fs::write(directory.path().join("TYPESAFE-LICENSE"), TYPESAFE_LICENSE)?;
        Ok(Self {
            catalogue: ai_toolbox_core::Catalogue::load(directory.path())?,
            _directory: directory,
        })
    }
}
