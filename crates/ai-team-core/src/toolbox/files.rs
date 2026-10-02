//! Bounded, no-follow snapshots. Planning runs on a private copy of these inputs.
//! The upstream reader may follow links; it never gets the operator's live tree.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const INPUTS: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".mcp.json",
    ".agents",
    ".claude/settings.json",
    ".claude/skills",
    ".claude/hooks",
    ".claude/mcp",
    ".codex/config.toml",
    ".codex/skills",
    ".codex/hooks",
    ".codex/mcp",
    ".pi/mcp.json",
    ".pi/mcp-adapter.json",
    ".pi/skills",
    ".pi/hooks",
    ".pi/mcp",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "composer.json",
    "fly.toml",
    "app.json",
    "app.config.js",
    "app.config.ts",
    "codegen.yml",
    "codegen.ts",
    "buf.gen.yaml",
    "openapi.yaml",
    ".github",
    "supabase",
    "ProjectSettings/ProjectVersion.txt",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Missing,
    File {
        contents: Vec<u8>,
        mode: u32,
    },
    Directory {
        entries: BTreeMap<String, Node>,
        mode: u32,
    },
    Symlink {
        target: String,
    },
}

#[derive(Debug, Default)]
struct Budget {
    bytes: usize,
    entries: usize,
}

impl Node {
    pub(super) fn read(path: &Path) -> Result<Self> {
        Self::read_with(path, &mut Budget::default(), 0)
    }

    fn read_with(path: &Path, budget: &mut Budget, depth: usize) -> Result<Self> {
        budget.entries += 1;
        if budget.entries > MAX_ENTRIES || depth > 32 {
            return Err(Error::invalid(
                "toolbox input exceeds the file/depth limit; inspect it manually",
            ));
        }
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Missing),
            Err(error) => return Err(error.into()),
        };
        if meta.file_type().is_symlink() {
            return Ok(Self::Symlink {
                target: text(&fs::read_link(path)?)?,
            });
        }
        let mode = meta.permissions().mode() & 0o777;
        if meta.is_file() {
            let size = usize::try_from(meta.len()).unwrap_or(usize::MAX);
            budget.bytes = budget.bytes.saturating_add(size);
            if budget.bytes > MAX_BYTES {
                return Err(Error::invalid(
                    "toolbox input exceeds 16 MiB; inspect it manually",
                ));
            }
            // Cap the read too: a file can grow after metadata was read.
            let mut contents = Vec::new();
            fs::File::open(path)?
                .take((size + 1) as u64)
                .read_to_end(&mut contents)?;
            if contents.len() != size {
                return Err(Error::invalid(
                    "toolbox input changed during inspection; scan again",
                ));
            }
            return Ok(Self::File { contents, mode });
        }
        if !meta.is_dir() {
            return Err(Error::invalid(format!(
                "unsupported toolbox input: {}",
                path.display()
            )));
        }
        let mut entries = BTreeMap::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            entries.insert(
                text(Path::new(&entry.file_name()))?,
                Self::read_with(&entry.path(), budget, depth + 1)?,
            );
        }
        Ok(Self::Directory { entries, mode })
    }

    pub(super) fn materialize(&self, path: &Path) -> Result<()> {
        match self {
            Self::Missing => {}
            Self::File { contents, mode } => {
                fs::write(path, contents)?;
                fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
            }
            Self::Directory { entries, mode } => {
                fs::create_dir(path)?;
                for (name, node) in entries {
                    node.materialize(&path.join(name))?;
                }
                fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
            }
            Self::Symlink { target } => std::os::unix::fs::symlink(target, path)?,
        }
        Ok(())
    }

    fn safe_links(&self, relative: &Path) -> Result<()> {
        match self {
            Self::Symlink { target }
                if relative == Path::new(".claude/skills") && target == "../.agents/skills" =>
            {
                Ok(())
            }
            Self::Symlink { .. } => Err(Error::invalid(format!(
                "{} is a symlink; toolbox setup leaves it for manual inspection",
                relative.display()
            ))),
            Self::Directory { entries, .. } => {
                for (name, node) in entries {
                    node.safe_links(&relative.join(name))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
    device: u64,
    inode: u64,
    mode: u32,
}

pub(super) fn identity(path: &Path) -> Result<Option<Identity>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::invalid(format!(
            "toolbox parent is not a real directory: {}",
            path.display()
        )));
    }
    Ok(Some(Identity {
        device: meta.dev(),
        inode: meta.ino(),
        mode: meta.permissions().mode() & 0o777,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub root: PathBuf,
    pub nodes: BTreeMap<String, Node>,
    pub parents: BTreeMap<String, Option<Identity>>,
}

impl Snapshot {
    pub(super) fn capture(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let mut snapshot = Self {
            root,
            nodes: BTreeMap::new(),
            parents: BTreeMap::new(),
        };
        let mut budget = Budget::default();
        snapshot
            .parents
            .insert(String::new(), identity(&snapshot.root)?);
        for input in INPUTS {
            snapshot.capture_parents(Path::new(input))?;
            let node = Node::read_with(&snapshot.root.join(input), &mut budget, 0)?;
            node.safe_links(Path::new(input))?;
            snapshot.nodes.insert((*input).into(), node);
        }
        Ok(snapshot)
    }

    pub(super) fn capture_parents(&mut self, path: &Path) -> Result<()> {
        let mut chain: Vec<_> = path.ancestors().skip(1).collect();
        chain.reverse();
        for parent in chain {
            let name = text(parent)?;
            if !self.parents.contains_key(&name) {
                self.parents
                    .insert(name, identity(&self.root.join(parent))?);
            }
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.validate_parents()?;
        for (path, before) in &self.nodes {
            if Node::read(&self.root.join(path))? != *before {
                return Err(stale(path));
            }
        }
        Ok(())
    }

    pub(super) fn validate_parents(&self) -> Result<()> {
        for (path, before) in &self.parents {
            if identity(&self.root.join(path))? != *before {
                return Err(stale(path));
            }
        }
        Ok(())
    }

    pub(super) fn stage(&self) -> Result<tempfile::TempDir> {
        let directory = tempfile::tempdir()?;
        for (relative, node) in &self.nodes {
            if *node == Node::Missing {
                continue;
            }
            let path = directory.path().join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            node.materialize(&path)?;
        }
        Ok(directory)
    }

    pub(super) fn before(&self, relative: &str) -> Result<Node> {
        for (input, node) in &self.nodes {
            let Ok(rest) = Path::new(relative).strip_prefix(input) else {
                continue;
            };
            let mut current = node;
            for part in rest.components() {
                let Node::Directory { entries, .. } = current else {
                    if *current == Node::Missing {
                        return Ok(Node::Missing);
                    }
                    return Err(Error::invalid("toolbox action traverses a file or symlink"));
                };
                let name = part
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| Error::invalid("non-UTF8 toolbox path"))?;
                let Some(next) = entries.get(name) else {
                    return Ok(Node::Missing);
                };
                current = next;
            }
            return Ok(current.clone());
        }
        Err(Error::invalid(format!(
            "toolbox action is outside the inspected setup inputs: {relative}"
        )))
    }
}

pub(super) fn relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::invalid("toolbox action escapes its project"))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::invalid(
            "toolbox action needs a normal project-relative path",
        ));
    }
    text(relative)
}

pub(super) fn text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::invalid("toolbox paths must be UTF-8"))
}

pub(super) fn stale(path: &str) -> Error {
    Error::invalid(format!("toolbox preview is stale at {path}; scan and preview again (nothing is re-planned on approval)"))
}
