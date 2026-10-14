//! Counts only the native edit tool's complete, numbered display diff. Never forwards
//! source text or infers repository changes from a write's input or a prose receipt.
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(super) struct EditCounts {
    added: usize,
    removed: usize,
    first_changed_line: Option<u64>,
    source: &'static str,
}

pub(super) fn counts(data: &Value) -> Option<EditCounts> {
    let details = data.pointer("/result/details")?;
    if ["truncated", "diffTruncated"]
        .iter()
        .any(|key| details.get(key).and_then(Value::as_bool) == Some(true))
    {
        return None;
    }
    let diff = details.get("diff")?.as_str()?;
    if diff.len() > 256 * 1024 {
        return None;
    }
    let mut counts = EditCounts {
        added: 0,
        removed: 0,
        first_changed_line: details
            .get("firstChangedLine")
            .and_then(Value::as_u64)
            .filter(|line| *line > 0 && *line <= 9_007_199_254_740_991),
        source: "pi_edit_diff",
    };
    for (index, line) in diff.lines().enumerate() {
        if index >= 20_000 {
            return None;
        }
        let prefix = *line.as_bytes().first()?;
        if !matches!(prefix, b' ' | b'+' | b'-') {
            return None;
        }
        let rest = line[1..].trim_start_matches(' ');
        // Pi elides unchanged context, never changed lines, in this display format.
        if prefix == b' ' && rest == "..." {
            continue;
        }
        let (number, _) = rest.split_once(' ')?;
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if number.parse::<u64>().ok()? == 0 {
            return None;
        }
        counts.added += usize::from(prefix == b'+');
        counts.removed += usize::from(prefix == b'-');
    }
    Some(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_only_explicit_numbered_changes_with_bounded_work() {
        let value = counts(&json!({"result":{"details":{"diff":"  9 +not an addition\n-10 old\n+10 new\n+11 second\n    ...", "firstChangedLine":10}}})).unwrap();
        assert_eq!(
            (value.added, value.removed, value.first_changed_line),
            (2, 1, Some(10))
        );
        assert_eq!(
            counts(&json!({"result":{"details":{"diff":""}}}))
                .unwrap()
                .added,
            0
        );
        for diff in [
            "+source",
            "+0 source",
            "+++ b/source",
            "@@ -1 +1 @@",
            "∂1 source",
        ] {
            assert!(
                counts(&json!({"result":{"details":{"diff":diff}}})).is_none(),
                "{diff}"
            );
        }
        for diff in [
            "+1 x\n".repeat(20_001),
            format!("+1 {}", "x".repeat(256 * 1024)),
        ] {
            assert!(counts(&json!({"result":{"details":{"diff":diff}}})).is_none());
        }
        assert!(
            counts(&json!({"result":{"details":{"diff":"+1 partial", "truncated":true}}}))
                .is_none()
        );
        for line in [json!(0), json!(-1), json!("7"), json!(1e30)] {
            assert!(counts(
                &json!({"result":{"details":{"diff":"+1 new", "firstChangedLine":line}}})
            )
            .unwrap()
            .first_changed_line
            .is_none());
        }
    }
}
