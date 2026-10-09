//! A raw HTTP/1.1 client for the House HTTP tests: one request per connection,
//! every byte visible. A library client would hide exactly what these tests check —
//! when each header line arrives (`X-ClickHouse-Progress` while the query runs),
//! and whether a chunked body ended with its terminating chunk (Ruling 9: a body
//! that failed mid-stream must not).

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// A response as it came off the wire.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// Header lines in arrival order, with the time each arrived.
    pub headers: Vec<(String, String, Instant)>,
    pub body: Vec<u8>,
    /// Whether the body ended properly: the terminating chunk of a chunked body, or
    /// all of a `Content-Length` body.
    pub complete: bool,
    pub sent_at: Instant,
}

impl Response {
    /// The first value of a header, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v, _)| v.as_str())
    }

    /// Every value of a header, with arrival times.
    pub fn all(&self, name: &str) -> Vec<(&str, Instant)> {
        self.headers
            .iter()
            .filter(|(n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v, t)| (v.as_str(), *t))
            .collect()
    }

    /// The header names in order.
    pub fn header_names(&self) -> Vec<&str> {
        self.headers.iter().map(|(n, _, _)| n.as_str()).collect()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Percent-encodes a query-string value.
pub fn enc(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Builds `/?k=v&…` with every value encoded.
pub fn target(params: &[(&str, &str)]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect();
    format!("/?{}", query.join("&"))
}

/// Sends one request and reads the whole response.
pub fn request(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Response {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("timeout");
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if method != "GET" || !body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let sent_at = Instant::now();
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    read_response(stream, sent_at, method == "HEAD")
}

fn read_response(stream: TcpStream, sent_at: Instant, head_only: bool) -> Response {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("status line");
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("a status line, got {line:?}"));
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("header line");
        let at = Instant::now();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if read == 0 || trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed.split_once(':').expect("name: value");
        headers.push((name.to_string(), value.trim().to_string(), at));
    }
    let get = |name: &str| {
        headers
            .iter()
            .find(|(n, _, _): &&(String, String, Instant)| n.eq_ignore_ascii_case(name))
            .map(|(_, v, _)| v.clone())
    };
    let mut body = Vec::new();
    let complete = if head_only {
        // A HEAD response has no body, whatever its headers say: anything that
        // follows (the connection is `close`) is a bug the caller sees in `body`.
        let _ = reader.read_to_end(&mut body);
        true
    } else if get("Transfer-Encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        read_chunked(&mut reader, &mut body)
    } else if let Some(length) = get("Content-Length") {
        let length: usize = length.parse().expect("length");
        body.resize(length, 0);
        reader.read_exact(&mut body).is_ok()
    } else {
        reader.read_to_end(&mut body).is_ok()
    };
    Response {
        status,
        headers,
        body,
        complete,
        sent_at,
    }
}

/// Reads a chunked body; true only if the terminating `0` chunk arrived.
fn read_chunked(reader: &mut impl BufRead, body: &mut Vec<u8>) -> bool {
    loop {
        let mut size_line = String::new();
        match reader.read_line(&mut size_line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        let size = match usize::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16)
        {
            Ok(size) => size,
            Err(_) => return false,
        };
        if size == 0 {
            let mut end = String::new();
            let _ = reader.read_line(&mut end);
            return true;
        }
        let mut chunk = vec![0u8; size];
        if reader.read_exact(&mut chunk).is_err() {
            body.extend_from_slice(&chunk);
            return false;
        }
        body.extend_from_slice(&chunk);
        let mut crlf = [0u8; 2];
        if reader.read_exact(&mut crlf).is_err() {
            return false;
        }
    }
}

/// Sends raw bytes and reads everything until the server closes (or `wait`).
pub fn raw(addr: SocketAddr, bytes: &[u8], wait: Duration) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream.set_read_timeout(Some(wait)).expect("timeout");
    stream.write_all(bytes).expect("write");
    let mut out = Vec::new();
    let mut piece = [0u8; 65536];
    loop {
        match stream.read(&mut piece) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&piece[..n]),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
