//! A minimal, single-purpose fake HTTP server for exercising the real
//! `cursdel` binary's `license enroll`/`refresh` network code paths in a
//! CLI integration test, without depending on a real license server being
//! reachable. Parses just enough HTTP/1.1 (request line + a
//! `Content-Length` body) to hand the path and JSON body to a caller-
//! supplied handler, and writes back whatever status/JSON the handler
//! returns.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct FakeServer {
    pub base_url: String,
    handle: Option<JoinHandle<()>>,
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        // Best-effort: the listener thread is blocked in `accept()`, which
        // has no clean cross-platform interrupt from here without extra
        // machinery: an OS-assigned ephemeral port dying with the test
        // process is an acceptable trade-off for a test-only helper.
        if let Some(handle) = self.handle.take() {
            drop(handle);
        }
    }
}

/// Starts a background thread serving exactly `expected_requests` requests
/// (then the thread exits), each answered by `handler(path, body) ->
/// (status, json_body)`.
pub fn spawn(
    expected_requests: usize,
    handler: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
) -> FakeServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("local_addr");
    let handler = Arc::new(handler);

    let handle = std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let (stream, _) = match listener.accept() {
                Ok(s) => s,
                Err(_) => return,
            };
            handle_one(stream, handler.as_ref());
        }
    });

    FakeServer {
        base_url: format!("http://{addr}"),
        handle: Some(handle),
    }
}

fn handle_one(mut stream: TcpStream, handler: &(impl Fn(&str, &str) -> (u16, String) + ?Sized)) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];

    // Read until we've seen the blank line terminating the headers.
    let headers_end = loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let header_text = String::from_utf8_lossy(&buf[..headers_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    let content_length: usize = lines
        .find_map(|l| {
            l.to_ascii_lowercase()
                .starts_with("content-length:")
                .then(|| l.to_string())
        })
        .and_then(|l| l.split(':').nth(1).map(|v| v.trim().to_string()))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    while buf.len() < headers_end + content_length {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[headers_end..headers_end + content_length]).to_string();

    let (status, json_body) = handler(&path, &body);
    let status_text = match status {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        409 => "409 Conflict",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    let content_type = if status == 200 {
        "application/json"
    } else {
        "application/problem+json"
    };
    let response = format!(
        "HTTP/1.1 {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
        json_body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
