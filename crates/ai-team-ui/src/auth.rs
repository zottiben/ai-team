//! A per-process bearer token on the API.
//!
//! The server binds loopback only, which is necessary but not sufficient: on a shared
//! machine every local process can reach 127.0.0.1, and this API will eventually be
//! able to dispatch agents against real worktrees. So the token exists from the first
//! commit rather than being retrofitted once there is something worth stealing.
//!
//! It is minted per process and never written to disk. A browser tab gets it from the
//! URL that opened the tab, which is the only channel a fresh tab has.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::error::{Error, Result};
use crate::state::AppState;

pub const TOKEN_HEADER: &str = "x-ai-team-token";
pub const TOKEN_QUERY: &str = "token";

/// 32 bytes of OS randomness, hex-encoded. Not a UUID: a v4 UUID is 122 bits spread
/// over a shape that invites people to parse it, and this is opaque by design.
pub(crate) fn mint_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        Error::Io(std::io::Error::other(format!(
            "no OS randomness for the session token: {e}"
        )))
    })?;
    Ok(bytes.iter().fold(String::with_capacity(64), |mut out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
        out
    }))
}

pub(crate) async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> std::result::Result<Response, Error> {
    let presented = from_header(&request).or_else(|| from_query(&request));
    match presented {
        Some(token) if constant_time_eq(&token, state.token()) => Ok(next.run(request).await),
        _ => Err(Error::Unauthorized),
    }
}

fn from_header(request: &Request) -> Option<String> {
    let value = request.headers().get(TOKEN_HEADER)?;
    value.to_str().ok().map(str::to_owned)
}

/// The query fallback is for `EventSource`, which cannot set a header. Everything else
/// uses the header.
fn from_query(request: &Request) -> Option<String> {
    let query = request.uri().query()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == TOKEN_QUERY).then(|| value.to_owned())
    })
}

/// Compared in constant time so a local process cannot recover the token one byte at a
/// time from how long the rejection took.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_is_64_hex_characters() {
        let token = mint_token().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, mint_token().unwrap());
    }

    #[test]
    fn comparison_rejects_the_near_misses() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
    }
}
