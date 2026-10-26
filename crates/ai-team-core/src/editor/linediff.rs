//! A line diff for the save preview.
//!
//! Small on purpose. The preview answers one question - what will this write change in
//! the file that is there now - so it needs hunks over two in-memory line lists and
//! nothing else: no rename detection, no index, no subprocess, and no copy of the
//! operator's buffer written to a temporary file outside the checkout.
//!
//! Bounded rather than clever. Common head and tail are trimmed first, which makes the
//! usual edit tiny; what is left is matched with a longest-common-subsequence table, and
//! a table too large to build honestly degrades to one replacement hunk that says so.

use serde::Serialize;

/// The most table cells an exact diff may build: about 4M `u32`s, 16MiB, which a preview
/// of a file somebody is editing by hand will never reach.
const MAX_CELLS: usize = 4_000_000;

const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub kind: LineKind,
    /// Line number in the file as it is now, or `None` for an added line.
    pub before: Option<usize>,
    /// Line number in the file as it would be written, or `None` for a removed line.
    pub after: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    pub before_start: usize,
    pub before_lines: usize,
    pub after_start: usize,
    pub after_lines: usize,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
    /// True when the two sides were too far apart to diff exactly and the single hunk is
    /// "all of this becomes all of that". Never silently: the window says so.
    pub coarse: bool,
    /// True when hunks were dropped to keep the preview bounded.
    pub truncated: bool,
}

/// Split text into lines, keeping the information a trailing newline carries.
///
/// `split('\n')` on "a\n" yields `["a", ""]`, and that empty last element is exactly how
/// a file that ends with a newline differs from one that does not - so it is kept.
fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// The hunks that turn `before` into `after`, with up to `MAX_HUNKS` of them.
pub(super) fn diff(before: &str, after: &str, max_hunks: usize) -> Change {
    let old = lines(before);
    let new = lines(after);

    let head = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_middle = &old[head..old.len() - tail];
    let new_middle = &new[head..new.len() - tail];

    if old_middle.is_empty() && new_middle.is_empty() {
        return Change {
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            coarse: false,
            truncated: false,
        };
    }

    let script = if old_middle.len().saturating_mul(new_middle.len()) > MAX_CELLS {
        coarse_script(old_middle, new_middle)
    } else {
        exact_script(old_middle, new_middle)
    };
    let coarse = old_middle.len().saturating_mul(new_middle.len()) > MAX_CELLS;

    let mut added = 0;
    let mut removed = 0;
    for entry in &script {
        match entry.0 {
            LineKind::Added => added += 1,
            LineKind::Removed => removed += 1,
            LineKind::Context => {}
        }
    }

    let (hunks, truncated) = group(&old, &new, head, &script, max_hunks);
    Change {
        hunks,
        added,
        removed,
        coarse,
        truncated,
    }
}

/// One step of the edit script: what happened, and which side's line it came from.
type Step = (LineKind, usize);

fn coarse_script(old: &[&str], new: &[&str]) -> Vec<Step> {
    (0..old.len())
        .map(|index| (LineKind::Removed, index))
        .chain((0..new.len()).map(|index| (LineKind::Added, index)))
        .collect()
}

fn exact_script(old: &[&str], new: &[&str]) -> Vec<Step> {
    // Standard LCS table. `table[i][j]` is the length of the longest common subsequence
    // of `old[i..]` and `new[j..]`, built from the end so the walk forward below reads
    // in file order.
    let rows = old.len() + 1;
    let columns = new.len() + 1;
    let mut table = vec![0u32; rows * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * columns + j] = if old[i] == new[j] {
                table[(i + 1) * columns + j + 1] + 1
            } else {
                table[(i + 1) * columns + j].max(table[i * columns + j + 1])
            };
        }
    }

    let mut script = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            script.push((LineKind::Context, i));
            i += 1;
            j += 1;
        } else if table[(i + 1) * columns + j] >= table[i * columns + j + 1] {
            script.push((LineKind::Removed, i));
            i += 1;
        } else {
            script.push((LineKind::Added, j));
            j += 1;
        }
    }
    while i < old.len() {
        script.push((LineKind::Removed, i));
        i += 1;
    }
    while j < new.len() {
        script.push((LineKind::Added, j));
        j += 1;
    }
    script
}

/// Turn the edit script over the middle into hunks with context, in file coordinates.
fn group(
    old: &[&str],
    new: &[&str],
    head: usize,
    script: &[Step],
    max_hunks: usize,
) -> (Vec<Hunk>, bool) {
    // Expand the script back to whole-file coordinates: the trimmed head is context on
    // both sides, and so is the tail.
    let mut rows: Vec<(LineKind, Option<usize>, Option<usize>, &str)> = Vec::new();
    for (index, text) in old.iter().enumerate().take(head) {
        rows.push((LineKind::Context, Some(index), Some(index), *text));
    }
    let (mut before, mut after) = (head, head);
    for (kind, index) in script {
        // The script is indexed within the trimmed middle; the rows are whole-file.
        match kind {
            LineKind::Context => {
                rows.push((
                    LineKind::Context,
                    Some(before),
                    Some(after),
                    old[head + *index],
                ));
                before += 1;
                after += 1;
            }
            LineKind::Removed => {
                rows.push((LineKind::Removed, Some(before), None, old[head + *index]));
                before += 1;
            }
            LineKind::Added => {
                rows.push((LineKind::Added, None, Some(after), new[head + *index]));
                after += 1;
            }
        }
    }
    while before < old.len() {
        rows.push((LineKind::Context, Some(before), Some(after), old[before]));
        before += 1;
        after += 1;
    }

    let changed: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.0 != LineKind::Context)
        .map(|(index, _)| index)
        .collect();
    if changed.is_empty() {
        return (Vec::new(), false);
    }

    let mut spans: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        let from = index.saturating_sub(CONTEXT);
        let to = (index + CONTEXT).min(rows.len() - 1);
        match spans.last_mut() {
            // Touching spans merge, so three one-line edits in a row read as one hunk.
            Some(last) if from <= last.1 + 1 => last.1 = last.1.max(to),
            _ => spans.push((from, to)),
        }
    }

    let truncated = spans.len() > max_hunks;
    let hunks = spans
        .into_iter()
        .take(max_hunks)
        .map(|(from, to)| {
            let lines: Vec<Line> = rows[from..=to]
                .iter()
                .map(|(kind, before, after, text)| Line {
                    kind: *kind,
                    // One-based, the way a file is read and the way the gutter numbers it.
                    before: before.map(|value| value + 1),
                    after: after.map(|value| value + 1),
                    text: (*text).to_string(),
                })
                .collect();
            Hunk {
                before_start: lines
                    .iter()
                    .find_map(|line| line.before)
                    .unwrap_or_default(),
                before_lines: lines.iter().filter(|line| line.before.is_some()).count(),
                after_start: lines.iter().find_map(|line| line.after).unwrap_or_default(),
                after_lines: lines.iter().filter(|line| line.after.is_some()).count(),
                lines,
            }
        })
        .collect();
    (hunks, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(change: &Change) -> Vec<(LineKind, String)> {
        change
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .map(|line| (line.kind, line.text.clone()))
            .collect()
    }

    #[test]
    fn an_unchanged_file_has_no_hunks() {
        let change = diff("a\nb\n", "a\nb\n", 64);
        assert!(change.hunks.is_empty());
        assert_eq!((change.added, change.removed), (0, 0));
    }

    #[test]
    fn one_edited_line_is_one_hunk_with_both_sides() {
        let change = diff("a\nb\nc\n", "a\nB\nc\n", 64);
        assert_eq!(change.hunks.len(), 1);
        assert_eq!((change.added, change.removed), (1, 1));
        assert_eq!(
            texts(&change),
            vec![
                (LineKind::Context, "a".into()),
                (LineKind::Removed, "b".into()),
                (LineKind::Added, "B".into()),
                (LineKind::Context, "c".into()),
                (LineKind::Context, String::new()),
            ]
        );
        let hunk = &change.hunks[0];
        assert_eq!((hunk.before_start, hunk.after_start), (1, 1));
    }

    #[test]
    fn a_dropped_trailing_newline_is_a_change_the_preview_shows() {
        // "a\n" and "a" differ by the empty last element, and a preview that trimmed it
        // would promise to write bytes it was not going to write.
        let change = diff("a\n", "a", 64);
        assert_eq!((change.added, change.removed), (0, 1));
        assert_eq!(
            texts(&change),
            vec![
                (LineKind::Context, "a".into()),
                (LineKind::Removed, String::new()),
            ]
        );
    }

    #[test]
    fn distant_edits_stay_separate_hunks_and_nearby_ones_merge() {
        let before = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>();
        let mut after = before.clone();
        after[2] = "line 3 edited".into();
        after[3] = "line 4 edited".into();
        after[30] = "line 31 edited".into();
        let change = diff(&before.join("\n"), &after.join("\n"), 64);
        assert_eq!(change.hunks.len(), 2, "{:?}", change.hunks);
        assert_eq!((change.added, change.removed), (3, 3));
    }

    #[test]
    fn a_hunk_cap_is_reported_rather_than_silently_applied() {
        let before = (1..=60).map(|n| format!("line {n}")).collect::<Vec<_>>();
        let after: Vec<String> = before
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if index % 10 == 0 {
                    format!("{line} edited")
                } else {
                    line.clone()
                }
            })
            .collect();
        let change = diff(&before.join("\n"), &after.join("\n"), 2);
        assert_eq!(change.hunks.len(), 2);
        assert!(change.truncated);
    }

    #[test]
    fn line_numbers_are_one_based_and_count_each_side_separately() {
        let change = diff("a\nb\nc\n", "a\nc\n", 64);
        let line = change.hunks[0]
            .lines
            .iter()
            .find(|line| line.kind == LineKind::Removed)
            .unwrap();
        assert_eq!((line.before, line.after), (Some(2), None));
    }

    #[test]
    fn an_empty_file_gaining_content_is_all_additions() {
        let change = diff("", "a\nb\n", 64);
        assert_eq!(change.removed, 0);
        assert!(change.added >= 2);
        assert!(!change.coarse);
    }
}
