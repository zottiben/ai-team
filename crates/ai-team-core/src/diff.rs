//! Unified diff, parsed into something a review can anchor comments to.
//!
//! The whole point of this module is the line numbers. A review comment says "line 42 of
//! the new side", and if the parser is off by one the comment lands on the wrong line -
//! which is worse than no comment, because it reads as a confident remark about code
//! that says something else. So every line carries the number it has on each side,
//! computed while walking the hunk rather than guessed at afterwards.
//!
//! git's own output is the input. Parsing it is unglamorous, and the traps are all in
//! the shapes real repositories produce rather than the ones a fixture does: a hunk
//! header with no count, a file with no trailing newline, a rename with no content
//! change at all, and content lines that begin with `--` or `+++`.

use serde::Serialize;

/// What happened to a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of one hunk, carrying the number it has on each side.
///
/// A context line has both. An added line has only a new number, a removed line only an
/// old one - which is exactly why a comment has to name a side as well as a number.
#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub kind: LineKind,
    pub old: Option<i64>,
    pub new: Option<i64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    /// The `@@ ... @@` line, with the trailing section heading git sometimes adds.
    pub header: String,
    pub old_start: i64,
    pub new_start: i64,
    pub lines: Vec<Line>,
}

/// What happened to a file between the two commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Removed,
    Renamed,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    /// The path as it is after the change. A review talks about the code that now
    /// exists, so this is the name a comment refers to.
    pub path: String,
    /// Where it came from, when that differs.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// True when git declined to show the content. Rendering a binary file as text is
    /// how a review turns into a screenful of noise.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub additions: usize,
    pub deletions: usize,
}

/// Parse `git diff` output.
///
/// Anything that is not recognised is skipped rather than guessed at: a diff this cannot
/// read should render as fewer files, never as files with wrong line numbers.
pub fn parse(input: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut at = Position { old: 0, new: 0 };

    for raw in input.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(FileDiff {
                path: paths_from_header(rest),
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: Vec::new(),
                additions: 0,
                deletions: 0,
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };

        if metadata(file, raw) {
            continue;
        }

        if raw.starts_with("@@") {
            if let Some((old, new)) = hunk_range(raw) {
                at = Position { old, new };
                file.hunks.push(Hunk {
                    header: raw.to_string(),
                    old_start: old,
                    new_start: new,
                    lines: Vec::new(),
                });
            }
            continue;
        }

        content(file, raw, &mut at);
    }

    if let Some(file) = current.take() {
        files.push(file);
    }
    files
}

/// Where the next line falls on each side, as the walk goes down a hunk.
struct Position {
    old: i64,
    new: i64,
}

/// Everything git writes between a `diff --git` line and the first hunk.
///
/// Returns whether the line was one of them. `rename to` and `+++ b/...` both override
/// the path taken from the header, which cannot be parsed unambiguously when a path
/// contains a space.
fn metadata(file: &mut FileDiff, raw: &str) -> bool {
    if let Some(from) = raw.strip_prefix("rename from ") {
        file.old_path = Some(from.to_string());
        file.status = FileStatus::Renamed;
        return true;
    }
    if let Some(to) = raw.strip_prefix("rename to ") {
        file.path = to.to_string();
        file.status = FileStatus::Renamed;
        return true;
    }
    if raw.starts_with("new file mode") {
        file.status = FileStatus::Added;
        return true;
    }
    if raw.starts_with("deleted file mode") {
        file.status = FileStatus::Removed;
        return true;
    }
    if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
        file.binary = true;
        return true;
    }

    // Only believe the rest before any hunk has started. Inside a hunk, a line beginning
    // `+++` is somebody's code, and reading it as a header would silently retarget every
    // later comment at another file.
    if !file.hunks.is_empty() {
        return false;
    }
    if let Some(path) = raw.strip_prefix("+++ b/") {
        file.path = path.to_string();
        return true;
    }
    if let Some(path) = raw.strip_prefix("--- a/") {
        if file.status == FileStatus::Renamed {
            file.old_path = Some(path.to_string());
        }
        return true;
    }
    if raw == "+++ /dev/null" {
        file.status = FileStatus::Removed;
        return true;
    }
    if raw == "--- /dev/null" {
        file.status = FileStatus::Added;
        return true;
    }
    raw.starts_with("index ") || raw.starts_with("similarity index ")
}

/// One line inside a hunk, numbered as it goes.
fn content(file: &mut FileDiff, raw: &str, at: &mut Position) {
    if file.hunks.last().is_none() {
        return;
    }

    // "\\ No newline at end of file" describes the line before it. It is not a line of
    // the file, and numbering it would shift everything after it.
    if raw.starts_with('\\') {
        return;
    }

    let (kind, text) = match raw.as_bytes().first() {
        Some(b'+') => (LineKind::Added, &raw[1..]),
        Some(b'-') => (LineKind::Removed, &raw[1..]),
        Some(b' ') => (LineKind::Context, &raw[1..]),
        // An empty line inside a hunk is a context line whose single space git dropped.
        // Treating it as anything else desynchronises every number below it.
        None => (LineKind::Context, ""),
        _ => return,
    };

    let line = match kind {
        LineKind::Added => {
            file.additions += 1;
            at.new += 1;
            Line {
                kind,
                old: None,
                new: Some(at.new - 1),
                text: text.to_string(),
            }
        }
        LineKind::Removed => {
            file.deletions += 1;
            at.old += 1;
            Line {
                kind,
                old: Some(at.old - 1),
                new: None,
                text: text.to_string(),
            }
        }
        LineKind::Context => {
            at.old += 1;
            at.new += 1;
            Line {
                kind,
                old: Some(at.old - 1),
                new: Some(at.new - 1),
                text: text.to_string(),
            }
        }
    };

    if let Some(hunk) = file.hunks.last_mut() {
        hunk.lines.push(line);
    }
}

/// `a/src/lib.rs b/src/lib.rs` -> `src/lib.rs`.
///
/// Only a first guess: `+++ b/...` and `rename to ...` both override it, because this
/// line cannot be parsed unambiguously when a path contains a space.
fn paths_from_header(rest: &str) -> String {
    rest.split_once(" b/").map_or_else(
        || rest.trim_start_matches("a/").to_string(),
        |(_, new)| new.to_string(),
    )
}

/// `@@ -12,7 +12,9 @@ fn thing()` -> `(12, 12)`.
///
/// The count is optional and means 1 when absent, which is the shape a single-line file
/// produces and the one a fixture never does.
fn hunk_range(header: &str) -> Option<(i64, i64)> {
    let inner = header.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = inner.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |spec: &str| -> Option<i64> {
        let count = spec.split_once(',');
        let (start, len) = match count {
            Some((start, len)) => (start, len.parse::<i64>().ok()?),
            None => (spec, 1),
        };
        let start = start.parse::<i64>().ok()?;
        // An empty range is written as `-0,0` for a new file. The first line git will
        // show is 1, and numbering from 0 would put every comment one line early.
        Some(if len == 0 { start.max(1) } else { start })
    };
    Some((start(old)?, start(new)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(input: &str) -> FileDiff {
        let files = parse(input);
        assert_eq!(files.len(), 1, "expected exactly one file");
        files.into_iter().next().unwrap()
    }

    #[test]
    fn every_line_carries_the_number_it_has_on_each_side() {
        // The whole module exists for this. A comment on "line 42 of the new side" that
        // lands on line 41 reads as a confident remark about different code.
        let file = only(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -10,4 +10,5 @@\n\
             \x20fn one() {}\n\
             -fn two() {}\n\
             +fn two(x: i32) {}\n\
             +fn three() {}\n\
             \x20fn four() {}\n",
        );
        let numbers: Vec<_> = file.hunks[0]
            .lines
            .iter()
            .map(|line| (line.kind, line.old, line.new))
            .collect();
        assert_eq!(
            numbers,
            [
                (LineKind::Context, Some(10), Some(10)),
                (LineKind::Removed, Some(11), None),
                (LineKind::Added, None, Some(11)),
                (LineKind::Added, None, Some(12)),
                (LineKind::Context, Some(12), Some(13)),
            ]
        );
        assert_eq!((file.additions, file.deletions), (2, 1));
    }

    #[test]
    fn a_hunk_header_with_no_count_means_one_line() {
        // What a single-line file produces, and what a hand-written fixture never does.
        let file = only("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n");
        assert_eq!(file.hunks[0].lines[0].old, Some(1));
        assert_eq!(file.hunks[0].lines[1].new, Some(1));
    }

    #[test]
    fn a_new_file_numbers_from_one_not_from_zero() {
        // git writes `-0,0` for the empty side. Numbering the new side from 0 would put
        // every comment in the file one line early.
        let file = only(
            "diff --git a/new.rs b/new.rs\n\
             new file mode 100644\n\
             --- /dev/null\n\
             +++ b/new.rs\n\
             @@ -0,0 +1,2 @@\n\
             +first\n\
             +second\n",
        );
        assert_eq!(file.status, FileStatus::Added);
        assert_eq!(file.hunks[0].lines[0].new, Some(1));
        assert_eq!(file.hunks[0].lines[1].new, Some(2));
    }

    #[test]
    fn a_line_of_code_beginning_with_plus_plus_is_code() {
        // C++ in a string, a markdown rule, a diff inside a test fixture. Treating it as
        // a header mid-hunk would silently retarget every later comment at another file.
        let file = only(
            "diff --git a/x b/x\n\
             --- a/x\n\
             +++ b/x\n\
             @@ -1,2 +1,3 @@\n\
             \x20let a = 1;\n\
             +++counter;\n\
             \x20let b = 2;\n",
        );
        assert_eq!(file.path, "x");
        assert_eq!(file.hunks[0].lines[1].text, "++counter;");
        assert_eq!(file.hunks[0].lines[1].new, Some(2));
    }

    #[test]
    fn no_newline_at_end_of_file_is_not_a_line() {
        // Numbering it would shift everything after it in a multi-hunk file.
        let file = only(
            "diff --git a/x b/x\n\
             --- a/x\n\
             +++ b/x\n\
             @@ -1,2 +1,2 @@\n\
             \x20keep\n\
             -old\n\
             \\ No newline at end of file\n\
             +new\n\
             \\ No newline at end of file\n",
        );
        assert_eq!(file.hunks[0].lines.len(), 3);
        assert_eq!(file.hunks[0].lines[2].new, Some(2));
    }

    #[test]
    fn an_empty_line_inside_a_hunk_is_context() {
        // git drops the leading space on a blank context line. Skipping it would
        // desynchronise every number below it.
        let file = only(
            "diff --git a/x b/x\n\
             --- a/x\n\
             +++ b/x\n\
             @@ -1,3 +1,3 @@\n\
             \x20fn a() {}\n\
             \n\
             -fn b() {}\n\
             +fn b(x: i32) {}\n",
        );
        assert_eq!(file.hunks[0].lines[1].kind, LineKind::Context);
        assert_eq!(file.hunks[0].lines[1].new, Some(2));
        assert_eq!(file.hunks[0].lines[3].new, Some(3));
    }

    #[test]
    fn a_rename_says_where_the_file_came_from() {
        let file = only(
            "diff --git a/old.rs b/new.rs\n\
             similarity index 94%\n\
             rename from old.rs\n\
             rename to new.rs\n",
        );
        assert_eq!(file.status, FileStatus::Renamed);
        assert_eq!(file.path, "new.rs");
        assert_eq!(file.old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn a_binary_file_is_flagged_rather_than_rendered() {
        let file = only(
            "diff --git a/logo.png b/logo.png\n\
             index 1234..5678 100644\n\
             Binary files a/logo.png and b/logo.png differ\n",
        );
        assert!(file.binary);
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn several_files_stay_separate_and_keep_their_own_counts() {
        let files = parse(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1,2 @@\n \x20x\n+y\n\
             diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -5,2 +5 @@\n-gone\n \x20stays\n",
        );
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!((files[0].additions, files[0].deletions), (1, 0));
        assert_eq!(files[1].path, "b.rs");
        assert_eq!((files[1].additions, files[1].deletions), (0, 1));
        assert_eq!(files[1].hunks[0].lines[0].old, Some(5));
    }

    #[test]
    fn a_second_hunk_restarts_from_its_own_header() {
        // Continuing to count from the first hunk is the classic off-by-many.
        let file = only(
            "diff --git a/x b/x\n\
             --- a/x\n\
             +++ b/x\n\
             @@ -1,2 +1,2 @@\n\
             \x20a\n\
             \x20b\n\
             @@ -50,2 +50,3 @@ fn far_below()\n\
             \x20y\n\
             +z\n",
        );
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(file.hunks[1].lines[0].new, Some(50));
        assert_eq!(file.hunks[1].lines[1].new, Some(51));
    }

    #[test]
    fn a_hunk_heading_after_the_at_at_is_kept_for_context() {
        let file =
            only("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@ impl Store {\n-a\n+b\n");
        assert!(file.hunks[0].header.ends_with("impl Store {"));
    }

    #[test]
    fn nothing_at_all_is_no_files_rather_than_a_panic() {
        assert!(parse("").is_empty());
        assert!(parse("not a diff\njust text\n").is_empty());
    }
}
