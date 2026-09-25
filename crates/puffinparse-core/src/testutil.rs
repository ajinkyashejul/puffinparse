//! Test-only loopback HTTP server for provider wire tests.
//!
//! A hand-rolled HTTP/1.1 server on 127.0.0.1 that answers a scripted sequence of responses (one
//! per connection, in order) and records every request it saw. Response bodies may contain the
//! placeholder `{base}`, replaced with the server's own `http://127.0.0.1:<port>` — that is how a
//! test hands the provider a "presigned URL" that points back at the same server.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One captured request.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    /// Request line, e.g. `GET /job/abc HTTP/1.1`.
    pub line: String,
    /// Header block (lower-cased names and values), without the request line.
    pub headers: String,
    pub body: String,
}

impl Seen {
    /// `"GET /path"` — method and path, for compact assertions.
    pub fn route(&self) -> String {
        self.line.rsplit_once(' ').map(|(r, _)| r.to_string()).unwrap_or_else(|| self.line.clone())
    }

    pub fn header(&self, name: &str) -> Option<String> {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        self.headers.lines().find_map(|l| l.strip_prefix(&prefix).map(|v| v.trim().to_string()))
    }
}

pub(crate) type Captured = Arc<Mutex<Vec<Seen>>>;

/// Serve `responses` (status, body) in order and return the base URL plus the request log.
pub(crate) async fn serve(responses: Vec<(u16, String)>) -> (String, Captured) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let own = base.clone();
    tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut head_end = None;
            let mut content_length = 0usize;
            let mut chunked = false;
            loop {
                let n = socket.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if head_end.is_none() {
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(pos + 4);
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        content_length = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        // Streamed multipart bodies arrive chunked, without a content-length.
                        chunked = head.contains("transfer-encoding: chunked");
                    }
                }
                if let Some(end) = head_end {
                    let done = if chunked {
                        buf[end..].ends_with(b"\r\n0\r\n\r\n")
                    } else {
                        buf.len() >= end + content_length
                    };
                    if done {
                        break;
                    }
                }
            }
            let end = head_end.unwrap_or(buf.len());
            let head = String::from_utf8_lossy(&buf[..end.saturating_sub(4)]).to_string();
            let (line, headers) = head.split_once("\r\n").unwrap_or((head.as_str(), ""));
            let seen = Seen {
                line: line.to_string(),
                headers: headers.to_lowercase(),
                body: String::from_utf8_lossy(&buf[end.min(buf.len())..]).to_string(),
            };
            sink.lock().expect("lock").push(seen);
            let body = body.replace("{base}", &own);
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (base, captured)
}

/// Read a fixture from `tests/fixtures/`.
pub(crate) fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).expect("fixture exists")
}

/// A tiny in-memory PDF request pointed at `base`, with a test key and no retries.
pub(crate) fn pdf_request(base: &str) -> crate::types::DocumentRequest {
    crate::types::DocumentRequest::from_bytes(bytes::Bytes::from_static(b"%PDF-1.4 test"), "doc.pdf")
        .api_key("test-key")
        .base_url(base)
        .timeout_secs(20.0)
        .max_retries(0)
}
