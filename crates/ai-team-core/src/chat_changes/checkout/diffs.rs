//! Per-file presentation identity includes Git metadata omitted by the line renderer:
//! full blob IDs, modes, binary patches and the missing-final-newline marker. It is not
//! command authority; approval still uses the complete checkout fingerprint.
use crate::FileDiff;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn read(text: &str) -> (Vec<FileDiff>, BTreeMap<String, String>) {
    let mut files = Vec::new();
    let mut revisions = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    let mut start = 0;
    // Source lines carry a context/add/remove prefix. Only a column-zero Git header
    // starts a file; a source string containing "diff --git" cannot split one.
    for end in text
        .match_indices("\ndiff --git ")
        .map(|(offset, _)| offset + 1)
        .chain(std::iter::once(text.len()))
    {
        let raw = &text[start..end];
        for file in crate::parse_diff(raw) {
            let revision = format!("{:x}", Sha256::digest(raw.as_bytes()));
            if revisions.insert(file.path.clone(), revision).is_some() {
                duplicates.insert(file.path.clone());
            }
            files.push(file);
        }
        start = end;
    }
    for path in duplicates {
        revisions.remove(&path);
    }
    (files, revisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_headers_do_not_split_files_and_ambiguous_paths_have_no_identity() {
        let patch = "diff --git a/a.rs b/a.rs\nindex aa..bb 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+diff --git a/fake b/fake\n";
        let (files, revisions) = read(patch);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks[0].lines[1].text, "diff --git a/fake b/fake");
        assert_eq!(revisions["a.rs"].len(), 64);
        let (_, duplicate) = read(&format!("{patch}{patch}"));
        assert!(duplicate.is_empty());
    }
}
