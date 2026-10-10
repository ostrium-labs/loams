//! The RFC 8252 section 7.3 loopback redirect: one listener on
//! `127.0.0.1:<ephemeral>`, one accepted callback, `state` checked.

use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::AuthError;

const PAGE: &str = "<!doctype html><meta charset=utf-8><title>Loams</title>\
<body style=\"font:16px system-ui;margin:3rem\"><h1>Signed in</h1>\
<p>You can close this tab and return to Loams Desktop.</p></body>";

/// A bound loopback listener awaiting the authorization callback.
#[derive(Debug)]
pub struct Loopback {
    listener: TcpListener,
    /// The `redirect_uri` to register in the authorization request.
    pub redirect_uri: String,
}

impl Loopback {
    /// Binds `127.0.0.1` on an ephemeral port.
    ///
    /// # Errors
    ///
    /// Fails if no loopback port can be bound.
    pub async fn bind() -> Result<Self, AuthError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| AuthError::Loopback(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| AuthError::Loopback(e.to_string()))?
            .port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
        })
    }

    /// Waits for the callback and returns the authorization code.
    ///
    /// Requests for other paths (a browser asking for `/favicon.ico`) get a
    /// 404 and are ignored. The first `/callback` is the answer: a wrong
    /// `state` or an `error` parameter fails the sign-in.
    ///
    /// # Errors
    ///
    /// [`AuthError::Timeout`], [`AuthError::StateMismatch`], or the provider's
    /// error.
    pub async fn wait_for_code(
        self,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<String, AuthError> {
        tokio::time::timeout(timeout, self.accept_callback(expected_state))
            .await
            .map_err(|_| AuthError::Timeout)?
    }

    async fn accept_callback(&self, expected_state: &str) -> Result<String, AuthError> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| AuthError::Loopback(e.to_string()))?;
            let Some(target) = read_request_target(&mut stream).await else {
                continue;
            };
            let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
                respond(&mut stream, "400 Bad Request", "bad request").await;
                continue;
            };
            if url.path() != "/callback" {
                respond(&mut stream, "404 Not Found", "not found").await;
                continue;
            }
            let params: std::collections::HashMap<String, String> =
                url.query_pairs().into_owned().collect();
            if let Some(error) = params.get("error") {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    "Sign-in failed. Return to Loams Desktop.",
                )
                .await;
                return Err(AuthError::Provider(error.clone()));
            }
            if params.get("state").map(String::as_str) != Some(expected_state) {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    "Sign-in failed. Return to Loams Desktop.",
                )
                .await;
                return Err(AuthError::StateMismatch);
            }
            let Some(code) = params.get("code") else {
                respond(&mut stream, "400 Bad Request", "missing code").await;
                return Err(AuthError::Protocol("callback without a code".into()));
            };
            respond(&mut stream, "200 OK", PAGE).await;
            return Ok(code.clone());
        }
    }
}

/// Reads the request line's target (`/callback?code=...`) from a GET.
async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = vec![0_u8; 8192];
    let mut filled = 0;
    // Only the request line matters; stop at the first newline or a full buffer.
    while filled < buf.len() {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf[filled..]))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        filled += n;
        if buf[..filled].contains(&b'\n') {
            break;
        }
    }
    let head = std::str::from_utf8(&buf[..filled]).ok()?;
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_owned))?
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let message = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\n\
         cache-control: no-store\r\nreferrer-policy: no-referrer\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(message.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(addr: &str, target: &str) -> String {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(format!("GET {target} HTTP/1.1\r\nhost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    fn addr_of(l: &Loopback) -> String {
        l.redirect_uri
            .trim_start_matches("http://")
            .trim_end_matches("/callback")
            .to_owned()
    }

    #[tokio::test]
    async fn returns_the_code_and_ignores_stray_requests() {
        let l = Loopback::bind().await.unwrap();
        let addr = addr_of(&l);
        let waiter = tokio::spawn(l.wait_for_code("good", Duration::from_secs(5)));
        assert!(get(&addr, "/favicon.ico").await.starts_with("HTTP/1.1 404"));
        let page = get(&addr, "/callback?code=abc&state=good").await;
        assert!(page.starts_with("HTTP/1.1 200"));
        assert_eq!(waiter.await.unwrap().unwrap(), "abc");
    }

    #[tokio::test]
    async fn wrong_state_fails_the_sign_in() {
        let l = Loopback::bind().await.unwrap();
        let addr = addr_of(&l);
        let waiter = tokio::spawn(l.wait_for_code("good", Duration::from_secs(5)));
        get(&addr, "/callback?code=abc&state=evil").await;
        assert!(matches!(
            waiter.await.unwrap(),
            Err(AuthError::StateMismatch)
        ));
    }

    #[tokio::test]
    async fn provider_errors_surface() {
        let l = Loopback::bind().await.unwrap();
        let addr = addr_of(&l);
        let waiter = tokio::spawn(l.wait_for_code("s", Duration::from_secs(5)));
        get(&addr, "/callback?error=access_denied&state=s").await;
        assert!(
            matches!(waiter.await.unwrap(), Err(AuthError::Provider(e)) if e == "access_denied")
        );
    }

    #[tokio::test]
    async fn times_out_when_nobody_calls_back() {
        let l = Loopback::bind().await.unwrap();
        let r = l.wait_for_code("s", Duration::from_millis(50)).await;
        assert!(matches!(r, Err(AuthError::Timeout)));
    }
}
