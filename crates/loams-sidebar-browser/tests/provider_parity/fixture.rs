//! The fixture web server: a tiny HTTP/1.1 server on 127.0.0.1, in the test
//! process, so no scenario touches the public internet.
//!
//! Hand-rolled on `tokio::net` rather than a framework so the suite adds no
//! dependency. It speaks just enough HTTP for an engine: one request per
//! connection (`Connection: close`), a `Content-Length` body, and no chunked
//! requests. Every request is recorded, headers and body included, so the
//! credential-boundary scenario can assert that no Loams token ever reached a
//! page's origin.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The bytes `/download.bin` serves.
pub const DOWNLOAD_BYTES: &[u8] = b"loams-parity-download-0123456789abcdef";

/// The text the content-extraction scenario looks for.
pub const EXTRACT_SENTINEL: &str = "Parity fixture content: the quick brown fox";

/// One request the fixture saw.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    /// The request as one string, for a substring search.
    pub fn flattened(&self) -> String {
        let mut out = format!("{} {}\n", self.method, self.path);
        for (name, value) in &self.headers {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        }
        out.push_str(&self.body);
        out
    }
}

/// A running fixture origin.
#[derive(Debug)]
pub struct Fixture {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}

impl Fixture {
    /// Bind 127.0.0.1 on a free port and start serving.
    ///
    /// `peer` is the other fixture's base URL, which `/cross` fetches. It is
    /// a parameter so origin A can name origin B without either knowing the
    /// other's port at compile time.
    pub async fn start(peer: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a fixture port on 127.0.0.1");
        let port = listener.local_addr().expect("a local address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let task_seen = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let seen = Arc::clone(&task_seen);
                let peer = peer.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, seen, peer.as_deref()).await;
                });
            }
        });
        Self { port, seen, task }
    }

    /// The origin, e.g. `http://127.0.0.1:41234`.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// A URL on this origin.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin())
    }

    /// Every request seen so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Requests whose path starts with `prefix`.
    pub fn seen_path(&self, prefix: &str) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|seen| seen.path.starts_with(prefix))
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    seen: Arc<Mutex<Vec<Seen>>>,
    peer: Option<&str>,
) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read"))??;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end;
        }
        if buffer.len() > 64 * 1024 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0)
        .min(1024 * 1024);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let request = Seen {
        method,
        path,
        headers,
        body,
    };
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request.clone());

    let response = route(&request, peer).await;
    stream.write_all(&response.encode()).await?;
    stream.shutdown().await
}

struct Response {
    status: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn html(body: String) -> Self {
        Self {
            status: "200 OK",
            headers: vec![(
                "Content-Type".to_string(),
                "text/html; charset=utf-8".to_string(),
            )],
            body: body.into_bytes(),
        }
    }

    fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {}\r\n", self.status);
        for (name, value) in &self.headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
            self.body.len()
        ));
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

async fn route(request: &Seen, peer: Option<&str>) -> Response {
    let (path, query) = request
        .path
        .split_once('?')
        .unwrap_or((request.path.as_str(), ""));
    let query = parse_form(query);
    match path {
        "/" => Response::html(index_page()),
        "/submit" => {
            let form = parse_form(&request.body);
            let email = form.get("email").cloned().unwrap_or_default();
            let note = form.get("note").cloned().unwrap_or_default();
            Response::html(page(
                "Submitted",
                &format!(
                    "<h1>Submitted</h1><p id=\"result\">Thanks, {} / {}</p>",
                    escape(&email),
                    escape(&note)
                ),
            ))
        }
        "/set-cookie" => {
            let value = query
                .get("v")
                .cloned()
                .unwrap_or_else(|| "unset".to_string());
            Response::html(page("Cookie set", "<p id=\"result\">cookie set</p>")).header(
                "Set-Cookie",
                format!("parity_session={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400"),
            )
        }
        "/echo-cookie" => {
            let cookie = request
                .headers
                .iter()
                .find(|(name, _)| name == "cookie")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            Response::html(page(
                "Echo",
                &format!("<p id=\"result\">cookie=[{}]</p>", escape(&cookie)),
            ))
        }
        "/download.bin" => Response {
            status: "200 OK",
            headers: vec![
                (
                    "Content-Type".to_string(),
                    "application/octet-stream".to_string(),
                ),
                (
                    "Content-Disposition".to_string(),
                    "attachment; filename=\"report.bin\"".to_string(),
                ),
            ],
            body: DOWNLOAD_BYTES.to_vec(),
        },
        "/inline.txt" => Response {
            status: "200 OK",
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: b"inline".to_vec(),
        },
        "/ping" => Response {
            status: "200 OK",
            headers: vec![
                ("Content-Type".to_string(), "text/plain".to_string()),
                ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
            ],
            body: b"pong".to_vec(),
        },
        "/slow" => {
            // Longer than any timeout or budget a scenario sets, and short
            // enough that a leaked connection does not outlive the job.
            tokio::time::sleep(Duration::from_secs(45)).await;
            Response::html(page("Slow", "<p>finally</p>"))
        }
        "/animate" => Response::html(animate_page()),
        "/cross" => Response::html(page(
            "Cross",
            &format!(
                "<p id=\"peer\">{}</p>",
                escape(peer.unwrap_or("no peer configured"))
            ),
        )),
        _ => Response {
            status: "404 Not Found",
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: b"not found".to_vec(),
        },
    }
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{margin:0;font:16px sans-serif;background:#fff;color:#111}}</style>\
         </head><body>{body}</body></html>"
    )
}

/// The main fixture: a heading, a link, a labelled form, a block of colour for
/// the screenshot's non-blank check, and a sentence for extraction.
fn index_page() -> String {
    page(
        "Parity fixture",
        &format!(
            "<h1>Fixture login</h1>\
             <nav><a id=\"docs\" href=\"/inline.txt\">Docs</a></nav>\
             <form id=\"login\" method=\"post\" action=\"/submit\">\
               <label for=\"email\">Email</label>\
               <input id=\"email\" name=\"email\" type=\"text\" \
                 style=\"display:block;width:300px;height:32px;margin:8px 0\">\
               <label for=\"note\">Note</label>\
               <input id=\"note\" name=\"note\" type=\"text\" \
                 style=\"display:block;width:300px;height:32px;margin:8px 0\">\
               <button id=\"submit\" type=\"submit\" \
                 style=\"display:block;width:160px;height:40px;margin:8px 0\">Sign in</button>\
             </form>\
             <div id=\"block\" style=\"width:320px;height:160px;background:#d01c1c\"></div>\
             <p id=\"content\">{EXTRACT_SENTINEL}.</p>"
        ),
    )
}

/// A page that keeps changing, so a screencast has frames to send.
fn animate_page() -> String {
    page(
        "Animate",
        "<div id=\"box\" style=\"width:240px;height:120px;background:#1c64d0\"></div>\
         <p id=\"tick\">0</p>\
         <script>\
           let n = 0;\
           setInterval(() => {\
             n += 1;\
             document.getElementById('tick').textContent = String(n);\
             document.getElementById('box').style.background = n % 2 ? '#1c64d0' : '#1cd064';\
           }, 100);\
         </script>",
    )
}

fn parse_form(raw: &str) -> BTreeMap<String, String> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_decoding() {
        let form = parse_form("email=agent%40example.test&note=hello+world");
        assert_eq!(form["email"], "agent@example.test");
        assert_eq!(form["note"], "hello world");
    }
}
