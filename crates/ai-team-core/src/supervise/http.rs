//! A small HTTP/1.1 client for talking to a supervised eve process.
//!
//! Hand-written rather than pulled in, and the reason is scope: every request this crate
//! makes goes to `127.0.0.1` over plaintext, to a process ai-team started itself. A
//! general client would bring a TLS stack, a connection pool and a middleware tower to
//! solve problems loopback does not have, and every one of those is a crate
//! `cargo deny` has to keep vetting.
//!
//! What it does need to get right is framing. eve streams NDJSON with
//! `Transfer-Encoding: chunked`, and a chunk boundary lands in the middle of a line
//! often enough that a naive reader looks correct locally and drops events under load.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

/// What to do after a streamed line. Returned by stream consumers so a caller can stop
/// reading without waiting for the body to end - which is how a parked turn releases its
/// connection instead of burning the idle timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: &'static str,
    pub path: String,
    /// The value for `Authorization`, already encoded.
    pub auth: Option<String>,
    pub json: Option<String>,
}

impl Request {
    pub(super) fn get(path: impl Into<String>) -> Request {
        Request {
            method: "GET",
            path: path.into(),
            auth: None,
            json: None,
        }
    }

    pub(super) fn post(path: impl Into<String>, json: Option<String>) -> Request {
        Request {
            method: "POST",
            path: path.into(),
            auth: None,
            json,
        }
    }

    #[must_use]
    pub(super) fn with_auth(mut self, auth: Option<String>) -> Request {
        self.auth = auth;
        self
    }

    fn head(&self, port: u16) -> String {
        use std::fmt::Write as _;

        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json, \
             application/x-ndjson\r\n",
            self.method, self.path
        );
        if let Some(auth) = &self.auth {
            let _ = write!(head, "Authorization: {auth}\r\n");
        }
        match &self.json {
            Some(body) => {
                let _ = write!(
                    head,
                    "Content-Type: application/json\r\nContent-Length: {}\r\n",
                    body.len()
                );
            }
            // A POST with no body still needs to say so, or a server waits for one.
            None if self.method == "POST" => head.push_str("Content-Length: 0\r\n"),
            None => {}
        }
        // No keep-alive: one request per connection. Pooling is what a real client is
        // for, and a supervised local process is not where that pays.
        head.push_str("Connection: close\r\n\r\n");
        head
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub(super) fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Fail with the server's own message. eve answers errors as JSON with an `error`
    /// field, and surfacing it beats "request failed with 409".
    pub(super) fn error(&self, what: &str) -> Error {
        let detail = serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| self.body.chars().take(200).collect());
        Error::invalid(format!("{what}: HTTP {} - {detail}", self.status))
    }

    pub(super) fn json(&self) -> Result<serde_json::Value> {
        Ok(serde_json::from_str(&self.body)?)
    }
}

/// Send a request and read the whole response.
pub(crate) async fn send(port: u16, request: &Request, timeout: Duration) -> Result<Response> {
    tokio::time::timeout(timeout, async {
        let mut reader = connect(port, request).await?;
        let (status, framing) = read_head(&mut reader).await?;
        let mut body = String::new();
        read_body(&mut reader, framing, |line| {
            body.push_str(line);
            body.push('\n');
            Flow::Continue
        })
        .await?;
        Ok(Response { status, body })
    })
    .await
    .map_err(|_| Error::invalid(format!("{} {} timed out", request.method, request.path)))?
}

/// Send a request and hand each line of the body to `on_line` as it arrives.
///
/// Used for the NDJSON event stream, where waiting for the body to finish would mean
/// waiting for the whole turn and showing the user nothing until it ended.
pub(crate) async fn stream<F>(
    port: u16,
    request: &Request,
    idle_timeout: Duration,
    mut on_line: F,
) -> Result<u16>
where
    F: FnMut(&str) -> Flow,
{
    let mut reader = connect(port, request).await?;
    let (status, framing) = read_head(&mut reader).await?;
    if !(200..300).contains(&status) {
        // Drain enough to report why, then stop.
        let mut body = String::new();
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            read_body(&mut reader, framing, |line| {
                body.push_str(line);
                Flow::Continue
            }),
        )
        .await;
        return Err(Response { status, body }.error(&format!("streaming {}", request.path)));
    }

    // The timeout is per line, not for the whole stream: a turn can legitimately think
    // for minutes, but a connection that has gone quiet for longer than this is wedged.
    let result = read_body_with_idle(&mut reader, framing, idle_timeout, &mut on_line).await;
    result.map(|()| status)
}

async fn connect(port: u16, request: &Request) -> Result<BufReader<TcpStream>> {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!("connecting to 127.0.0.1:{port}: {e}"),
        ))
    })?;
    socket.write_all(request.head(port).as_bytes()).await?;
    if let Some(body) = &request.json {
        socket.write_all(body.as_bytes()).await?;
    }
    socket.flush().await?;
    Ok(BufReader::new(socket))
}

/// How the body is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    Chunked,
    Length(usize),
    ToEof,
}

async fn read_head(reader: &mut BufReader<TcpStream>) -> Result<(u16, Framing)> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| Error::invalid(format!("not an HTTP response: {line:?}")))?;

    let mut framing = Framing::ToEof;
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header).await?;
        if read == 0 || header.trim().is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        match name.as_str() {
            // Chunked wins: when both are present the framing is chunked, and trusting
            // the length instead truncates the body.
            "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => {
                framing = Framing::Chunked;
            }
            "content-length" if framing != Framing::Chunked => {
                if let Ok(len) = value.parse() {
                    framing = Framing::Length(len);
                }
            }
            _ => {}
        }
    }
    Ok((status, framing))
}

async fn read_body<F>(reader: &mut BufReader<TcpStream>, framing: Framing, on_line: F) -> Result<()>
where
    F: FnMut(&str) -> Flow,
{
    read_body_with_idle(reader, framing, Duration::from_secs(300), on_line).await
}

async fn read_body_with_idle<F>(
    reader: &mut BufReader<TcpStream>,
    framing: Framing,
    idle: Duration,
    mut on_line: F,
) -> Result<()>
where
    F: FnMut(&str) -> Flow,
{
    let mut pending = String::new();
    let mut remaining = match framing {
        Framing::Length(len) => len,
        _ => usize::MAX,
    };

    loop {
        let chunk = if framing == Framing::Chunked {
            match next_chunk(reader, idle).await? {
                Some(bytes) => bytes,
                None => break,
            }
        } else {
            {
                if remaining == 0 {
                    break;
                }
                let mut buf = vec![0u8; remaining.min(16 * 1024)];
                let read = tokio::time::timeout(idle, reader.read(&mut buf))
                    .await
                    .map_err(|_| Error::invalid("the response went quiet"))??;
                if read == 0 {
                    break;
                }
                buf.truncate(read);
                if remaining != usize::MAX {
                    remaining -= read;
                }
                buf
            }
        };

        pending.push_str(&String::from_utf8_lossy(&chunk));
        // A chunk boundary lands mid-line often enough that this is the part that has to
        // be right: only complete lines are handed on, and the remainder stays pending.
        while let Some(at) = pending.find('\n') {
            let line: String = pending.drain(..=at).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if on_line(line) == Flow::Stop {
                return Ok(());
            }
        }
    }

    // A body that did not end with a newline still has a last line.
    if !pending.trim().is_empty() {
        on_line(pending.trim_end_matches(['\r', '\n']));
    }
    Ok(())
}

/// One chunk, or `None` at the terminating zero-length chunk.
async fn next_chunk(reader: &mut BufReader<TcpStream>, idle: Duration) -> Result<Option<Vec<u8>>> {
    let mut size_line = String::new();
    let read = tokio::time::timeout(idle, reader.read_line(&mut size_line))
        .await
        .map_err(|_| Error::invalid("the stream went quiet"))??;
    if read == 0 {
        return Ok(None);
    }
    // A chunk size may carry extensions after a `;`, and is hex.
    let size_text = size_line.trim();
    let size_text = size_text.split(';').next().unwrap_or("").trim();
    if size_text.is_empty() {
        // A stray CRLF between chunks; ask again rather than treating it as the end.
        return Box::pin(next_chunk(reader, idle)).await;
    }
    let size = usize::from_str_radix(size_text, 16)
        .map_err(|_| Error::invalid(format!("bad chunk size {size_text:?}")))?;

    if size == 0 {
        // Trailers, then a blank line. Nothing reads them, but they must be consumed.
        loop {
            let mut trailer = String::new();
            let read = reader.read_line(&mut trailer).await?;
            if read == 0 || trailer.trim().is_empty() {
                break;
            }
        }
        return Ok(None);
    }

    let mut buf = vec![0u8; size];
    tokio::time::timeout(idle, reader.read_exact(&mut buf))
        .await
        .map_err(|_| Error::invalid("the stream went quiet mid-chunk"))??;
    // The CRLF that closes the chunk.
    let mut crlf = [0u8; 2];
    let _ = reader.read_exact(&mut crlf).await;
    Ok(Some(buf))
}

/// `Authorization: Basic <base64(user:pass)>`.
pub(crate) fn basic_auth(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64(format!("{username}:{password}").as_bytes())
    )
}

/// Base64 for one short credential. A dependency for 20 lines used once would be a
/// worse trade than the 20 lines.
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for group in input.chunks(3) {
        let b = [
            group[0],
            *group.get(1).unwrap_or(&0),
            *group.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= group.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that replies with exactly these bytes, so the framing under test is the
    /// framing that arrives - which a real HTTP server would not let us control.
    async fn serve(reply: &'static [u8]) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                // Read the request head so the client's write does not block.
                let mut reader = BufReader::new(&mut socket);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line.trim().is_empty()
                    {
                        break;
                    }
                }
                let _ = socket.write_all(reply).await;
                let _ = socket.flush().await;
            }
        });
        port
    }

    #[tokio::test]
    async fn a_content_length_body_is_read_exactly() {
        let port =
            serve(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"ok\":true,\"a\":1}").await;
        let response = send(port, &Request::get("/x"), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.json().unwrap()["ok"], true);
    }

    #[tokio::test]
    async fn a_chunked_body_is_reassembled() {
        let port = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
              5\r\n{\"a\":\r\n\
              4\r\n1}\n{\r\n\
              7\r\n\"b\":2}\n\r\n\
              0\r\n\r\n",
        )
        .await;

        let mut lines = Vec::new();
        stream(port, &Request::get("/s"), Duration::from_secs(5), |line| {
            lines.push(line.to_string());
            Flow::Continue
        })
        .await
        .unwrap();

        // The point: both chunk boundaries fall mid-line, and the lines still come out
        // whole.
        assert_eq!(lines, [r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[tokio::test]
    async fn a_chunk_size_with_an_extension_is_accepted() {
        let port = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
              3;name=value\r\nhi\n\r\n0\r\n\r\n",
        )
        .await;
        let mut lines = Vec::new();
        stream(port, &Request::get("/s"), Duration::from_secs(5), |line| {
            lines.push(line.to_string());
            Flow::Continue
        })
        .await
        .unwrap();
        assert_eq!(lines, ["hi"]);
    }

    #[tokio::test]
    async fn stopping_early_closes_the_stream() {
        let port = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
              2\r\na\n\r\n2\r\nb\n\r\n2\r\nc\n\r\n0\r\n\r\n",
        )
        .await;
        let mut seen = Vec::new();
        stream(port, &Request::get("/s"), Duration::from_secs(5), |line| {
            seen.push(line.to_string());
            if seen.len() == 2 {
                Flow::Stop
            } else {
                Flow::Continue
            }
        })
        .await
        .unwrap();
        assert_eq!(seen, ["a", "b"]);
    }

    #[tokio::test]
    async fn a_body_with_no_trailing_newline_still_yields_its_last_line() {
        let port = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello").await;
        let mut lines = Vec::new();
        stream(port, &Request::get("/s"), Duration::from_secs(5), |line| {
            lines.push(line.to_string());
            Flow::Continue
        })
        .await
        .unwrap();
        assert_eq!(lines, ["hello"]);
    }

    #[tokio::test]
    async fn an_error_response_reports_the_servers_own_message() {
        let port = serve(
            b"HTTP/1.1 409 Conflict\r\nContent-Length: 62\r\n\r\n\
              {\"code\":\"session_not_active\",\"error\":\"The session is gone.\"}",
        )
        .await;
        let response = send(port, &Request::get("/x"), Duration::from_secs(5))
            .await
            .unwrap();
        assert!(!response.ok());
        let err = response.error("sending a follow-up").to_string();
        assert!(err.contains("409"), "{err}");
        assert!(err.contains("The session is gone."), "{err}");
    }

    #[tokio::test]
    async fn a_refused_connection_names_the_port() {
        // Nothing is listening: the supervisor has to be able to say which process.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let err = send(port, &Request::get("/x"), Duration::from_secs(2))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(&port.to_string()), "{err}");
    }

    #[test]
    fn basic_auth_encodes_the_way_every_server_expects() {
        // Checked against the RFC 7617 example rather than against itself.
        assert_eq!(
            basic_auth("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }
}
