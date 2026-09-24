//! No two files in this repository may differ only in case.
//!
//! APFS is case-insensitive by default, so `Editor.tsx` and `editor.ts` are two files on
//! the Linux machine this is built on and one file on the Mac it is built for (D12). The
//! symptom is not a missing file - it is TypeScript reporting that a module has no
//! exported member, on one CI leg only, which is a confusing half-hour.
//!
//! This runs on both legs, but the leg that matters is Linux: macOS cannot even check out
//! a tree containing the collision, so by the time CI sees it there the damage is done.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the workspace root is, from this crate.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> is two below the root")
        .to_path_buf()
}

/// Directories that are not ours to police - dependencies and build output.
const SKIP: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    ".output",
    ".eve",
    ".file-sql",
];

fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            walk(&path, found);
        } else {
            found.push(path);
        }
    }
}

#[test]
fn no_two_files_differ_only_in_case() {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(
        files.len() > 50,
        "the walk found almost nothing - wrong root?"
    );

    // Keyed on the lowercased path, which is what a case-insensitive filesystem sees.
    let mut seen: HashMap<String, Vec<String>> = HashMap::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        seen.entry(relative.to_lowercase())
            .or_default()
            .push(relative);
    }

    let collisions: Vec<_> = seen
        .into_values()
        .filter(|paths| paths.len() > 1)
        .map(|mut paths| {
            paths.sort();
            paths.join(" and ")
        })
        .collect();

    assert!(
        collisions.is_empty(),
        "these differ only in case, so macOS sees one file where Linux sees two:\n  {}",
        collisions.join("\n  ")
    );
}
