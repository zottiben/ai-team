//! The repository, as the team sees it.
//!
//! Routing is by zone (D14): a seat builds a slice because its zone globs claim the paths
//! that slice touches, and a path nobody claims is work nobody can be given. That is a
//! fact about a checkout, and nothing in the window said it - the roster listed `crates/**`
//! as text and left "does that actually cover this repository" for somebody to work out
//! in their head.
//!
//! So this walks the checkout once and answers it directly: every path, the seat that owns
//! it, and what is left over. It resolves ownership with the same [`zone_specificity`] that
//! dispatch uses, which is the property that matters - a picture that disagrees with where
//! the work would actually go is worse than no picture.
//!
//! Deliberately one walk and one response. The tree route lists a single directory because
//! that is what a file browser opens; a map of the whole checkout is a different question,
//! and asking it a directory at a time would be a few hundred round trips to draw one
//! panel.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::util::zone_specificity;

/// Build output, not source - the same set the tree refuses to walk into.
///
/// Kept here rather than shared with the tree: that list is about what a file browser
/// should show, this one is about what the team owns, and the day they disagree it will be
/// because one of them grew an entry the other should not have.
const SKIP: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "vendor",
    "dist",
    ".output",
    ".eve",
    ".file-sql",
    ".venv",
    "__pycache__",
    // Finder's folder metadata, which macOS writes wherever it has shown a folder.
    ".DS_Store",
];

/// How deep the walk goes before it stops describing and starts listing.
const MAX_DEPTH: usize = 7;

/// How many nodes a map may carry.
///
/// Ownership is still counted across the larger walk below. This is only the visual sample:
/// an alphabetically early tooling directory must not make the page claim the app is unowned.
const MAX_NODES: usize = 700;

/// A filesystem safety bound, separate from the visual bound.
///
/// Repositories commonly carry more than 700 source files. Walking enough to count those
/// truthfully is cheap once dependencies/build output are skipped; shipping all of them to
/// React is not. Hitting this larger bound is still reported as truncation.
const MAX_FILES_WALKED: usize = 50_000;

/// A seat, reduced to what ownership needs.
///
/// Owned rather than borrowed: the caller reads these out of a locked store, and holding
/// that lock across a filesystem walk would block every other request on the window for
/// as long as the disk takes.
#[derive(Debug, Clone)]
pub struct Owner {
    pub role: String,
    pub name: String,
    pub zone: String,
}

/// One path in the checkout.
#[derive(Debug, Clone, Serialize)]
pub struct MapNode {
    pub path: String,
    pub name: String,
    pub dir: bool,
    pub depth: usize,
    /// Files at or under this node; 1 for a file. What sizes a point.
    pub weight: usize,
    /// The role of the seat whose zone claims this path, or `None` when nobody does.
    pub owner: Option<String>,
}

/// Parent to child. Indices into [`RepoMap::nodes`], because a name would be a second
/// spelling of the path and a chance for the two to disagree.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MapEdge {
    pub from: usize,
    pub to: usize,
}

/// What one seat ended up owning.
#[derive(Debug, Clone, Serialize)]
pub struct MapZone {
    pub role: String,
    pub name: String,
    pub zone: String,
    /// Files claimed. The number that makes a zone real rather than aspirational.
    pub owns: usize,
}

/// The checkout, and who owns it.
#[derive(Debug, Clone, Serialize)]
pub struct RepoMap {
    pub root: String,
    pub nodes: Vec<MapNode>,
    pub edges: Vec<MapEdge>,
    /// Every seat, including the ones that own nothing - a zone that claims no file is
    /// the interesting case, not one to hide.
    pub zones: Vec<MapZone>,
    /// Files no zone claims. Reported rather than rounded away: this is the number that
    /// says a run will report work undone instead of giving it to somebody (D14).
    pub unowned: usize,
    pub files: usize,
    /// True when the walk hit [`MAX_NODES`] or [`MAX_DEPTH`] and stopped early.
    pub truncated: bool,
}

/// Walk a checkout and say who owns what.
pub fn repo_map(worktree: &Path, owners: &[Owner]) -> Result<RepoMap> {
    let root = worktree.file_name().map_or_else(
        || worktree.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );

    let mut walked: Vec<(String, usize)> = Vec::new();
    let mut truncated = false;
    walk(worktree, "", 0, &mut walked, &mut truncated)?;

    // Sorted so the same checkout always produces the same map. The walk takes whatever
    // order the filesystem hands back, which differs between APFS and ext4 - and a panel
    // that reshuffles itself between two machines is a panel nobody trusts (D12).
    walked.sort_by(|a, b| a.0.cmp(&b.0));

    // Count ownership over everything walked, not merely over the points that fit in the
    // picture. The old depth-first 700-file cutoff let `.agents/` consume the entire map
    // before `app/` was visited, reporting 0% owned for a repository with valid zones.
    let mut zones: Vec<MapZone> = owners
        .iter()
        .map(|owner| MapZone {
            role: owner.role.clone(),
            name: owner.name.clone(),
            zone: owner.zone.clone(),
            owns: 0,
        })
        .collect();
    let mut unowned = 0usize;
    for (path, _) in &walked {
        match owner_of(path, owners).as_deref() {
            Some(role) => {
                if let Some(zone) = zones.iter_mut().find(|zone| zone.role == role) {
                    zone.owns += 1;
                }
            }
            None => unowned += 1,
        }
    }

    let file_count = walked.len();
    let files = if walked.len() > MAX_NODES {
        truncated = true;
        // Include both ends and evenly spaced points between them. Sampling the first 700
        // would reproduce the same alphabetical starvation as the old walk bound.
        (0..MAX_NODES)
            .map(|at| {
                let index = at * (walked.len() - 1) / (MAX_NODES - 1);
                walked[index].clone()
            })
            .collect()
    } else {
        walked
    };

    let mut nodes: Vec<MapNode> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut edges: Vec<MapEdge> = Vec::new();

    nodes.push(MapNode {
        path: String::new(),
        name: root.clone(),
        dir: true,
        depth: 0,
        weight: 0,
        owner: None,
    });
    index.insert(String::new(), 0);

    // Every directory on the way to a file becomes a node, so the picture has the spine
    // the repository actually has rather than a cloud of leaves.
    for (path, depth) in &files {
        let mut parent = 0usize;
        let parts: Vec<&str> = path.split('/').collect();
        for (step, part) in parts.iter().enumerate() {
            let so_far = parts[..=step].join("/");
            let last = step + 1 == parts.len();
            let at = *index.entry(so_far.clone()).or_insert_with(|| {
                nodes.push(MapNode {
                    path: so_far.clone(),
                    name: (*part).to_string(),
                    dir: !last,
                    depth: step + 1,
                    weight: 0,
                    owner: None,
                });
                edges.push(MapEdge {
                    from: parent,
                    to: nodes.len() - 1,
                });
                nodes.len() - 1
            });
            if last {
                nodes[at].weight = 1;
                nodes[at].owner = owner_of(path, owners);
                let _ = depth;
            }
            parent = at;
        }
    }

    // A directory weighs what is under it, and takes the ownership its contents voted
    // for. Asking the zones directly would not work: `crates/**` claims the files in
    // `crates`, not the word `crates`, so every directory would come back unowned.
    roll_up(&mut nodes, &edges);

    Ok(RepoMap {
        root,
        files: file_count,
        nodes,
        edges,
        zones,
        unowned,
        truncated,
    })
}

/// Which seat claims a path, most specific claim winning.
///
/// The tie-break is the whole reason this defers to [`zone_specificity`] rather than
/// taking the first match: two zones that overlap resolve by what they said, not by which
/// seat happens to be first in the roster.
fn owner_of(path: &str, owners: &[Owner]) -> Option<String> {
    owners
        .iter()
        .filter_map(|owner| zone_specificity(&owner.zone, path).map(|how| (how, owner)))
        .max_by_key(|(how, _)| *how)
        .map(|(_, owner)| owner.role.clone())
}

/// Give every directory the weight and the owner of what it contains.
fn roll_up(nodes: &mut [MapNode], edges: &[MapEdge]) {
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for edge in edges {
        children.entry(edge.from).or_default().push(edge.to);
    }

    // Deepest first, so a directory is summed only once everything below it already has
    // been. Doing it the other way round gives every directory above the second level a
    // weight of zero, and the picture loses its trunk.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|at| std::cmp::Reverse(nodes[*at].depth));

    for at in order {
        if !nodes[at].dir {
            continue;
        }
        let Some(kids) = children.get(&at) else {
            continue;
        };
        nodes[at].weight = kids.iter().map(|kid| nodes[*kid].weight).sum();

        let mut votes: HashMap<String, usize> = HashMap::new();
        for kid in kids {
            if let Some(role) = nodes[*kid].owner.clone() {
                *votes.entry(role).or_default() += nodes[*kid].weight;
            }
        }
        // Ties broken by name so the answer does not depend on hash order.
        nodes[at].owner = votes
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(role, _)| role);
    }
}

/// Collect every file under `worktree`, relative and slash-separated.
fn walk(
    worktree: &Path,
    relative: &str,
    depth: usize,
    into: &mut Vec<(String, usize)>,
    truncated: &mut bool,
) -> Result<()> {
    if depth >= MAX_DEPTH {
        *truncated = true;
        return Ok(());
    }
    if into.len() >= MAX_FILES_WALKED {
        *truncated = true;
        return Ok(());
    }

    let dir = if relative.is_empty() {
        worktree.to_path_buf()
    } else {
        worktree.join(relative)
    };

    let listing = std::fs::read_dir(&dir)
        .map_err(|error| Error::invalid(format!("could not read {}: {error}", dir.display())))?;

    let mut here: Vec<(String, bool)> = Vec::new();
    for entry in listing.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        // Symlinks are not followed. A checkout with one pointing at its own parent is a
        // walk that does not finish, and `file_type` reports the link rather than what it
        // points at precisely so this can be refused here.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        here.push((path, kind.is_dir()));
    }
    here.sort();

    for (path, is_dir) in here {
        if into.len() >= MAX_FILES_WALKED {
            *truncated = true;
            return Ok(());
        }
        if is_dir {
            walk(worktree, &path, depth + 1, into, truncated)?;
        } else {
            into.push((path, depth + 1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "crates/ai-team-core/src/lib.rs",
            "crates/ai-team-core/src/store.rs",
            "crates/ai-team-ui/src/api.rs",
            "ui/src/App.tsx",
            "ui/src/Overview.tsx",
            "README.md",
        ] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, b"x").unwrap();
        }
        // Build output, which must not appear.
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/ait"), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("vendor/package")).unwrap();
        std::fs::write(dir.path().join("vendor/package/library.php"), b"x").unwrap();
        dir
    }

    fn owners() -> Vec<Owner> {
        vec![
            Owner {
                role: "backend".into(),
                name: "Backend".into(),
                zone: "crates/**".into(),
            },
            Owner {
                role: "frontend".into(),
                name: "Frontend".into(),
                zone: "ui/**".into(),
            },
        ]
    }

    #[test]
    fn every_file_is_placed_under_the_seat_whose_zone_claims_it() {
        let dir = checkout();
        let map = repo_map(dir.path(), &owners()).unwrap();

        let owner = |path: &str| {
            map.nodes
                .iter()
                .find(|node| node.path == path)
                .unwrap_or_else(|| panic!("{path} is not on the map"))
                .owner
                .clone()
        };
        assert_eq!(
            owner("crates/ai-team-core/src/lib.rs").as_deref(),
            Some("backend")
        );
        assert_eq!(owner("ui/src/App.tsx").as_deref(), Some("frontend"));
        // Nobody claimed the root README, and that is the answer rather than a default.
        assert_eq!(owner("README.md"), None);
        assert_eq!(map.unowned, 1);
    }

    #[test]
    fn build_output_is_not_part_of_the_repository() {
        // `target/` holds more files than the rest of this project by two orders of
        // magnitude, so a map that walks into it is a map of a build directory.
        let dir = checkout();
        let map = repo_map(dir.path(), &owners()).unwrap();
        assert!(
            !map.nodes.iter().any(|node| node.path.starts_with("target")),
            "target/ should not be walked"
        );
        assert_eq!(map.files, 6);
    }

    #[test]
    fn finders_folder_metadata_is_not_a_file_nobody_owns() {
        // macOS writes a `.DS_Store` into every folder Finder has shown. Counted, each is
        // a file no zone claims, and the checkout reads as less owned than it is.
        let dir = checkout();
        for folder in ["", "crates", "ui"] {
            std::fs::write(dir.path().join(folder).join(".DS_Store"), b"x").unwrap();
        }
        let map = repo_map(dir.path(), &owners()).unwrap();
        assert_eq!(map.unowned, 1);
        assert_eq!(map.files, 6);
    }

    #[test]
    fn a_directory_weighs_what_is_under_it() {
        let dir = checkout();
        let map = repo_map(dir.path(), &owners()).unwrap();
        let at = |path: &str| map.nodes.iter().find(|node| node.path == path).unwrap();

        assert_eq!(at("crates").weight, 3);
        assert_eq!(at("crates/ai-team-core/src").weight, 2);
        assert_eq!(at("ui").weight, 2);
        // And the root carries the lot, which is what makes the trunk the trunk.
        assert_eq!(map.nodes[0].weight, 6);
    }

    #[test]
    fn a_directory_takes_the_ownership_of_its_contents() {
        // Zones claim files, not directory names: `crates/**` does not match `crates`, so
        // asking the globs directly would leave every directory unowned and the picture
        // with no structure to colour.
        let dir = checkout();
        let map = repo_map(dir.path(), &owners()).unwrap();
        let at = |path: &str| map.nodes.iter().find(|node| node.path == path).unwrap();

        assert_eq!(at("crates").owner.as_deref(), Some("backend"));
        assert_eq!(at("ui/src").owner.as_deref(), Some("frontend"));
    }

    #[test]
    fn overlapping_zones_resolve_by_what_they_said() {
        // The tie-break dispatch uses. A catch-all must not starve a seat that named the
        // directory, whichever order the roster happens to be in.
        let dir = checkout();
        let mut owners = owners();
        owners.insert(
            0,
            Owner {
                role: "orchestrator".into(),
                name: "Orchestrator".into(),
                zone: "**".into(),
            },
        );

        let map = repo_map(dir.path(), &owners).unwrap();
        let at = |path: &str| map.nodes.iter().find(|node| node.path == path).unwrap();

        assert_eq!(at("ui/src/App.tsx").owner.as_deref(), Some("frontend"));
        // And the catch-all still picks up what nobody else named.
        assert_eq!(at("README.md").owner.as_deref(), Some("orchestrator"));
        assert_eq!(map.unowned, 0);
    }

    #[test]
    fn a_zone_that_claims_nothing_is_still_reported() {
        // The interesting case: a seat configured for a directory this repository does
        // not have will never be given work, and the roster's text alone cannot say so.
        let dir = checkout();
        let mut owners = owners();
        owners.push(Owner {
            role: "mobile".into(),
            name: "Mobile".into(),
            zone: "ios/**".into(),
        });

        let map = repo_map(dir.path(), &owners).unwrap();
        let mobile = map.zones.iter().find(|zone| zone.role == "mobile").unwrap();
        assert_eq!(mobile.owns, 0);
        assert_eq!(map.zones.len(), 3);
    }

    #[test]
    fn an_early_large_tooling_directory_does_not_hide_owned_application_code() {
        let dir = tempfile::tempdir().unwrap();
        let noise = dir.path().join(".agents/cache");
        std::fs::create_dir_all(&noise).unwrap();
        for at in 0..900 {
            std::fs::write(noise.join(format!("{at:04}.md")), b"x").unwrap();
        }
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::write(dir.path().join("app/feature.php"), b"x").unwrap();
        let owners = [Owner {
            role: "backend".into(),
            name: "Backend".into(),
            zone: "app/**".into(),
        }];

        let map = repo_map(dir.path(), &owners).unwrap();
        assert_eq!(map.files, 901);
        assert!(map.truncated);
        assert_eq!(map.zones[0].owns, 1);
        assert_eq!(map.unowned, 900);
        assert!(
            map.nodes.iter().any(|node| node.path == "app/feature.php"),
            "the visual sample should span the repository rather than take its first 700 files"
        );
    }

    #[test]
    fn the_same_checkout_maps_the_same_way_twice() {
        // The walk takes whatever order the filesystem gives it, which is not the same on
        // APFS and ext4 - and a panel that reshuffles between two machines is one nobody
        // trusts (D12).
        let dir = checkout();
        let first = repo_map(dir.path(), &owners()).unwrap();
        let again = repo_map(dir.path(), &owners()).unwrap();

        let paths = |map: &RepoMap| {
            map.nodes
                .iter()
                .map(|node| node.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&first), paths(&again));
    }

    #[test]
    fn a_symlink_is_not_followed() {
        // A link pointing at its own parent is a walk that does not finish.
        let dir = checkout();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();

        let map = repo_map(dir.path(), &owners()).unwrap();
        assert!(!map.nodes.iter().any(|node| node.path.starts_with("loop")));
    }
}
