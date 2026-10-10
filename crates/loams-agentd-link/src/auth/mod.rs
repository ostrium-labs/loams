//! Authentik OIDC sign-in for the desktop (design 37 section 6.5, D431):
//! authorization code with PKCE in the system browser, a loopback redirect,
//! the refresh token in the OS keychain.
//!
//! The fork's own WorkOS login, which belonged to its private sync backend,
//! is gone (plan DD1 Tasks 2 and 3).
//!
//! Not here yet (plan AP1n Task 4): the RFC 8693 exchange of the Authentik
//! token for Loams tokens at the Loams gateway, which waits for the unified
//! auth plan (Q438). Until then the Authentik access token is the bearer.

pub mod loopback;
pub mod oidc;
pub mod pkce;
pub mod store;

use std::time::Duration;

pub use oidc::{Endpoints, OidcClient, Tokens};
pub use store::{KeyringStore, MemoryStore, TokenStore};

/// How long the user has to finish signing in at the browser.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Everything that can go wrong signing in.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// A network failure reaching the provider.
    #[error("network error: {0}")]
    Network(String),
    /// A malformed provider response.
    #[error("unexpected provider response: {0}")]
    Protocol(String),
    /// The token endpoint refused the grant (RFC 6749 section 5.2 `error`).
    #[error("token endpoint refused the request: {0}")]
    TokenEndpoint(String),
    /// The provider redirected back with an `error`.
    #[error("sign-in refused by the provider: {0}")]
    Provider(String),
    /// The callback's `state` did not match: a stray or forged redirect.
    #[error("sign-in callback state mismatch")]
    StateMismatch,
    /// The user did not finish in time.
    #[error("timed out waiting for the browser sign-in")]
    Timeout,
    /// The loopback listener failed.
    #[error("loopback listener: {0}")]
    Loopback(String),
    /// The browser could not be opened.
    #[error("could not open the system browser: {0}")]
    Browser(String),
    /// The credential store failed.
    #[error("credential store: {0}")]
    Store(String),
}

/// Opens a URL in the user's browser. A trait so tests need no browser.
pub trait BrowserOpener: Send + Sync {
    /// # Errors
    ///
    /// If no browser can be launched.
    fn open(&self, url: &str) -> Result<(), AuthError>;
}

/// The operating system's default browser.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemBrowser;

impl BrowserOpener for SystemBrowser {
    fn open(&self, url: &str) -> Result<(), AuthError> {
        webbrowser::open(url).map_err(|e| AuthError::Browser(e.to_string()))
    }
}

/// Runs the whole interactive sign-in and stores the refresh token under
/// `store_key`.
///
/// # Errors
///
/// Any [`AuthError`].
pub async fn sign_in(
    oidc: &OidcClient,
    store: &dyn TokenStore,
    store_key: &str,
    browser: &dyn BrowserOpener,
    timeout: Duration,
) -> Result<Tokens, AuthError> {
    let pkce = pkce::Pkce::generate();
    let state = pkce::random_token(24);
    let nonce = pkce::random_token(24);
    let loopback = loopback::Loopback::bind().await?;
    let redirect_uri = loopback.redirect_uri.clone();
    let url = oidc.authorize_url(&redirect_uri, &state, &nonce, &pkce);
    browser.open(url.as_str())?;
    let code = loopback.wait_for_code(&state, timeout).await?;
    let tokens = oidc
        .exchange_code(&code, &redirect_uri, &pkce.verifier)
        .await?;
    if let Some(refresh) = &tokens.refresh_token {
        store.save(store_key, refresh)?;
    }
    Ok(tokens)
}

/// Silent sign-in from a stored refresh token. The refresh token rotates, so
/// the new one replaces the old. A refused grant (`invalid_grant`) deletes the
/// stale entry and returns `Ok(None)`: the user must sign in again.
///
/// # Errors
///
/// Network and store failures. A refusal is not an error.
pub async fn restore(
    oidc: &OidcClient,
    store: &dyn TokenStore,
    store_key: &str,
) -> Result<Option<Tokens>, AuthError> {
    let Some(refresh) = store.load(store_key)? else {
        return Ok(None);
    };
    match oidc.refresh(&refresh).await {
        Ok(tokens) => {
            if let Some(next) = &tokens.refresh_token {
                store.save(store_key, next)?;
            }
            Ok(Some(tokens))
        }
        Err(AuthError::TokenEndpoint(reason)) if reason == "invalid_grant" => {
            store.delete(store_key)?;
            Ok(None)
        }
        Err(other) => Err(other),
    }
}

/// Forgets the stored refresh token.
///
/// # Errors
///
/// Store failures.
pub fn sign_out(store: &dyn TokenStore, store_key: &str) -> Result<(), AuthError> {
    store.delete(store_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    /// A token endpoint that records form bodies and replies from a script.
    struct FakeIdp {
        url: url::Url,
        bodies: Arc<Mutex<Vec<String>>>,
    }

    async fn fake_idp(replies: Vec<(u16, &'static str)>) -> FakeIdp {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let recorded = bodies.clone();
        tokio::spawn(async move {
            for (status, json) in replies {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0_u8; 4096];
                let body = loop {
                    let n = s.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let want: usize = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= want || n == 0 {
                            break body.to_owned();
                        }
                    }
                    if n == 0 {
                        break String::new();
                    }
                };
                recorded.lock().unwrap().push(body);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
                    json.len()
                );
                s.write_all(reply.as_bytes()).await.unwrap();
                let _ = s.shutdown().await;
            }
        });
        FakeIdp {
            url: format!("http://127.0.0.1:{port}/token").parse().unwrap(),
            bodies,
        }
    }

    fn client(idp: &FakeIdp) -> OidcClient {
        OidcClient::new(
            "loams-desktop",
            Endpoints {
                authorization_endpoint: "http://127.0.0.1:1/authorize".parse().unwrap(),
                token_endpoint: idp.url.clone(),
            },
        )
    }

    /// Plays the browser: follows the authorize URL straight to its redirect.
    struct FakeBrowser;
    impl BrowserOpener for FakeBrowser {
        fn open(&self, url: &str) -> Result<(), AuthError> {
            let url = url::Url::parse(url).unwrap();
            let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            let callback = format!("{}?code=the-code&state={}", q["redirect_uri"], q["state"]);
            tokio::spawn(async move {
                let u = url::Url::parse(&callback).unwrap();
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", u.port().unwrap()))
                    .await
                    .unwrap();
                let target = format!("{}?{}", u.path(), u.query().unwrap());
                s.write_all(format!("GET {target} HTTP/1.1\r\nhost: x\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut sink = Vec::new();
                let _ = s.read_to_end(&mut sink).await;
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn sign_in_exchanges_the_code_with_the_verifier_and_stores_the_refresh_token() {
        let idp = fake_idp(vec![(
            200,
            r#"{"access_token":"at","refresh_token":"rt1","id_token":"it","expires_in":300}"#,
        )])
        .await;
        let store = MemoryStore::default();
        let tokens = sign_in(
            &client(&idp),
            &store,
            "inst:usr",
            &FakeBrowser,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "at");
        assert_eq!(store.load("inst:usr").unwrap().as_deref(), Some("rt1"));
        let body = idp.bodies.lock().unwrap()[0].clone();
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code=the-code"));
        assert!(body.contains("code_verifier="));
        assert!(body.contains("client_id=loams-desktop"));
        assert!(!body.contains("client_secret"));
    }

    #[tokio::test]
    async fn restore_rotates_the_refresh_token() {
        let idp = fake_idp(vec![(
            200,
            r#"{"access_token":"at2","refresh_token":"rt2"}"#,
        )])
        .await;
        let store = MemoryStore::default();
        store.save("k", "rt1").unwrap();
        let tokens = restore(&client(&idp), &store, "k").await.unwrap().unwrap();
        assert_eq!(tokens.access_token, "at2");
        assert_eq!(store.load("k").unwrap().as_deref(), Some("rt2"));
        assert!(idp.bodies.lock().unwrap()[0].contains("refresh_token=rt1"));
    }

    #[tokio::test]
    async fn restore_with_a_revoked_token_signs_the_user_out() {
        let idp = fake_idp(vec![(400, r#"{"error":"invalid_grant"}"#)]).await;
        let store = MemoryStore::default();
        store.save("k", "rt1").unwrap();
        assert!(restore(&client(&idp), &store, "k").await.unwrap().is_none());
        assert_eq!(store.load("k").unwrap(), None);
    }

    #[tokio::test]
    async fn restore_with_nothing_stored_is_none() {
        let idp = fake_idp(vec![]).await;
        let store = MemoryStore::default();
        assert!(restore(&client(&idp), &store, "k").await.unwrap().is_none());
    }
}
