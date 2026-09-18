//! LSP's framing, which is the part that goes wrong.
//!
//! A message is `Content-Length: N\r\n\r\n` followed by exactly N **bytes** of JSON. That
//! sounds trivial and is the third time this project has had to get a framing right, so
//! the same three rules apply as in `supervise/http.rs` and the review stub:
//!
//! - **A read is not a message.** One `read()` returns whatever the kernel has; a header
//!   and its body routinely arrive separately, and two small messages routinely arrive
//!   together. Nothing may be handed on until a whole one is in the buffer.
//! - **N is bytes, not characters.** rust-analyzer sends hover text full of `→` and `…`,
//!   and slicing a UTF-8 string by a byte count taken as characters truncates mid-message
//!   for the rest of the session.
//! - **The tail is the next message.** Whatever is left after N bytes belongs to the
//!   message after this one, and discarding it loses a response nobody will resend.

use crate::error::{Error, Result};

/// Frame a payload for sending.
///
/// The length is `body.len()`, which is bytes because `str::len` is bytes. That is the
/// correct thing here and the one place the distinction is free.
pub(crate) fn frame(body: &str) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Take one complete message off the front of a buffer.
///
/// Returns `None` when the buffer does not hold a whole message yet, which is the normal
/// state between reads rather than an error. On success the message is removed from
/// `buffer` and whatever followed it is left in place.
pub(crate) fn take_message(buffer: &mut Vec<u8>) -> Result<Option<String>> {
    const SEPARATOR: &[u8] = b"\r\n\r\n";

    let Some(head_end) = find(buffer, SEPARATOR) else {
        return Ok(None);
    };

    // Headers are ASCII by specification, so a lossy read here cannot corrupt a length.
    let headers = String::from_utf8_lossy(&buffer[..head_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .ok_or_else(|| {
            Error::invalid(format!(
                "an LSP message arrived with no readable Content-Length: {headers:?}"
            ))
        })?;

    let body_start = head_end + SEPARATOR.len();
    if buffer.len() < body_start + length {
        // The body is still arriving. Leave everything where it is.
        return Ok(None);
    }

    let body = buffer[body_start..body_start + length].to_vec();
    // Drain rather than clear: what follows is the next message, and dropping it loses a
    // response the server will never send again.
    buffer.drain(..body_start + length);

    String::from_utf8(body)
        .map(Some)
        .map_err(|error| Error::invalid(format!("an LSP message was not UTF-8: {error}")))
}

/// First index of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    #[test]
    fn a_whole_message_comes_out_and_the_buffer_empties() {
        let mut buf = frame(r#"{"id":1}"#);
        assert_eq!(
            take_message(&mut buf).unwrap().as_deref(),
            Some(r#"{"id":1}"#)
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn a_header_with_no_body_yet_is_not_a_message() {
        // The ordinary state between two reads, not an error.
        let mut buf = buffer("Content-Length: 8\r\n\r\n{\"id\"");
        assert_eq!(take_message(&mut buf).unwrap(), None);
        // And nothing was consumed, so the next read completes it.
        buf.extend_from_slice(b":1}");
        assert_eq!(
            take_message(&mut buf).unwrap().as_deref(),
            Some(r#"{"id":1}"#)
        );
    }

    #[test]
    fn half_a_header_is_not_a_message_either() {
        let mut buf = buffer("Content-Len");
        assert_eq!(take_message(&mut buf).unwrap(), None);
        assert_eq!(buf.len(), 11, "nothing should have been consumed");
    }

    #[test]
    fn two_messages_in_one_read_both_come_out() {
        // Servers batch constantly - a response and a progress notification arrive
        // together - and clearing the buffer after the first would lose the second.
        let mut buf = frame(r#"{"id":1}"#);
        buf.extend_from_slice(&frame(r#"{"id":2}"#));

        assert_eq!(
            take_message(&mut buf).unwrap().as_deref(),
            Some(r#"{"id":1}"#)
        );
        assert_eq!(
            take_message(&mut buf).unwrap().as_deref(),
            Some(r#"{"id":2}"#)
        );
        assert_eq!(take_message(&mut buf).unwrap(), None);
    }

    #[test]
    fn the_length_is_bytes_not_characters() {
        // rust-analyzer's hover text is full of `→` and `…`. Treating the count as
        // characters truncates this message and desynchronises every one after it.
        let body = r#"{"hover":"fn a() → …"}"#;
        assert!(
            body.chars().count() < body.len(),
            "the fixture needs multi-byte text"
        );

        let mut buf = frame(body);
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some(body));
        assert!(buf.is_empty());
    }

    #[test]
    fn a_body_split_mid_character_waits_for_the_rest() {
        // A read boundary lands wherever the kernel puts it, including inside a `→`.
        let body = r#"{"hover":"→"}"#;
        let framed = frame(body);
        let split = framed.len() - 1;

        let mut buf = framed[..split].to_vec();
        assert_eq!(take_message(&mut buf).unwrap(), None);

        buf.extend_from_slice(&framed[split..]);
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some(body));
    }

    #[test]
    fn extra_headers_are_tolerated() {
        // The specification allows Content-Type, and some servers send it.
        let mut buf = buffer(
            "Content-Length: 8\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{\"id\":1}",
        );
        assert_eq!(
            take_message(&mut buf).unwrap().as_deref(),
            Some(r#"{"id":1}"#)
        );
    }

    #[test]
    fn the_header_name_is_matched_without_regard_to_case() {
        let mut buf = buffer("content-length: 8\r\n\r\n{\"id\":1}");
        assert_eq!(
            take_message(&mut buf).unwrap().as_deref(),
            Some(r#"{"id":1}"#)
        );
    }

    #[test]
    fn a_message_with_no_length_is_an_error_rather_than_a_silent_stall() {
        // Otherwise the reader waits forever for a body whose size it never learned.
        let mut buf = buffer("Content-Type: application/json\r\n\r\n{}");
        assert!(take_message(&mut buf).is_err());
    }

    #[test]
    fn an_empty_body_is_a_message() {
        let mut buf = buffer("Content-Length: 0\r\n\r\n");
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some(""));
    }

    #[test]
    fn framing_round_trips() {
        for body in ["{}", r#"{"a":"→…"}"#, &"x".repeat(10_000)] {
            let mut buf = frame(body);
            assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some(body));
        }
    }
}
