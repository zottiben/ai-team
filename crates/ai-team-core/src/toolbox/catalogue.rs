//! A reproducible catalogue, owned by the binary rather than discovered on the machine.

use std::{fs, os::unix::fs::PermissionsExt};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

include!(concat!(env!("OUT_DIR"), "/toolbox_assets.rs"));

pub const REVISION: &str = include_str!("../../assets/toolbox/REVISION");
pub const NOTICE: &str = include_str!("../../assets/toolbox/NOTICE.md");
const TYPESAFE_LICENSE: &str = include_str!("../../assets/toolbox/skills/typesafe-ai/LICENSE");

/// Public catalogue content, with no temporary or standalone filesystem addresses.
/// Reading these bundled assets neither scans HOME nor connects any server.
#[derive(Debug, serde::Serialize)]
pub struct Catalogue {
    pub revision: String,
    pub hooks: Vec<Item>,
    pub mcp: Vec<Item>,
    pub skills: Vec<Item>,
    pub rules: Vec<Item>,
    pub templates: Vec<Item>,
    pub helpers: Vec<Item>,
    pub charter: Item,
    pub notice: String,
}

#[derive(Debug, serde::Serialize)]
pub struct Item {
    pub key: String,
    pub description: Option<String>,
    pub contents: String,
}

fn item(key: &str, path: &std::path::Path, description: Option<&str>) -> Result<Item> {
    Ok(Item {
        key: key.into(),
        description: description.map(str::to_owned),
        contents: fs::read_to_string(path)?,
    })
}

pub fn catalogue() -> Result<Catalogue> {
    let package = Packaged::load()?;
    let c = &package.catalogue;
    Ok(Catalogue {
        revision: REVISION.trim().into(),
        hooks: c
            .hooks
            .iter()
            .map(|h| item(&h.name, &h.path, h.summary.as_deref()))
            .collect::<Result<_>>()?,
        mcp: c
            .presets
            .iter()
            .map(|p| item(&p.name, &p.path, None))
            .collect::<Result<_>>()?,
        skills: c
            .skills
            .iter()
            .map(|s| item(&s.key, &s.path.join("SKILL.md"), s.description.as_deref()))
            .collect::<Result<_>>()?,
        rules: c
            .rules
            .iter()
            .map(|r| item(&r.name, &r.path, None))
            .collect::<Result<_>>()?,
        templates: ASSETS
            .iter()
            .filter(|(path, ..)| {
                path.starts_with("templates/")
                    || path.starts_with("starters/agents/")
                    || path.starts_with("background/")
                    || *path == "mcp/mcp.json.template"
            })
            .map(|(path, ..)| item(path, &c.root.join(path), None))
            .collect::<Result<_>>()?,
        helpers: c
            .helpers
            .iter()
            .map(|h| item(&h.name, &h.path, None))
            .collect::<Result<_>>()?,
        charter: item(
            "base-charter",
            &c.root.join("starters/base-charter.md"),
            None,
        )?,
        notice: format!("{NOTICE}\n{TYPESAFE_LICENSE}"),
    })
}

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
