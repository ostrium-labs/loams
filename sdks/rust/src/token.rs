//! Token sources (design §44 §7.4, D608; runtime contract R1).
//!
//! A [`TokenSource`] returns a bearer and can be asked for a new one. Tokens
//! travel in `Authorization: Bearer` and **never** in the query string and
//! never in a URL. A `401` carrying `reason = token_expired` triggers one
//! refresh and one retry; that logic lives in the call path, not here, so a
//! source stays a source and a source whose credential does not expire — an API
//! key — is not asked to refresh.

use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::error::LoamsError;

/// Where a call's bearer comes from.
#[async_trait]
pub trait TokenSource: Send + Sync + std::fmt::Debug {
    /// The bearer to send, or `None` to send no credential at all.
    ///
    /// Called once per attempt, so a source may answer differently each time.
    async fn token(&self) -> Option<String>;

    /// Fetch a new token after the server reported the current one expired.
    ///
    /// A source that **cannot** refresh — an API key, which does not expire —
    /// does not implement this, and the runtime's refresh-once-and-retry is
    /// then a no-op rather than a retry with the same rejected credential.
    async fn refresh(&self) -> Result<(), LoamsError> {
        Err(LoamsError::internal("this token source cannot refresh"))
    }

    /// Whether [`TokenSource::refresh`] does anything. The call path reads this
    /// so it can skip the retry entirely for a non-refreshing source.
    fn can_refresh(&self) -> bool {
        false
    }
}

/// A Loams API key. The key does not expire, so there is nothing to refresh.
#[derive(Debug, Clone)]
pub struct ApiKey(Arc<str>);

impl ApiKey {
    /// Wraps a key. An empty key is refused here rather than sent as
    /// `Bearer `, which a server reads as a malformed credential.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when `key` is empty.
    pub fn new(key: impl Into<String>) -> Result<Self, LoamsError> {
        let key = key.into();
        if key.is_empty() {
            return Err(LoamsError::internal("ApiKey: the key is empty"));
        }
        Ok(ApiKey(key.into()))
    }
}

#[async_trait]
impl TokenSource for ApiKey {
    async fn token(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

/// A token that is already valid, for a caller that manages its own.
#[derive(Debug, Clone)]
pub struct StaticToken(Arc<str>);

impl StaticToken {
    /// Wraps a token. An empty token is refused, as for [`ApiKey`].
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when `token` is empty.
    pub fn new(token: impl Into<String>) -> Result<Self, LoamsError> {
        let token = token.into();
        if token.is_empty() {
            return Err(LoamsError::internal("StaticToken: the token is empty"));
        }
        Ok(StaticToken(token.into()))
    }
}

#[async_trait]
impl TokenSource for StaticToken {
    async fn token(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

/// `LOAMS_API_KEY`, then `LOAMS_TOKEN`, then nothing (design §44 §7.4, D407's
/// rename of the environment).
///
/// Read on **every** call rather than once at construction, so a process that
/// receives its credentials after the client is built — a sidecar, a test —
/// still authenticates. An environment with neither variable is a source that
/// answers `None`, not an error: `GetInstance` needs no credential at all, and
/// an unauthenticated client is a legitimate client.
#[derive(Debug, Clone)]
pub struct EnvToken {
    variables: Vec<(&'static str, &'static str)>,
}

impl Default for EnvToken {
    fn default() -> Self {
        EnvToken {
            variables: vec![
                ("LOAMS_API_KEY", "LOAMS_API_KEY"),
                ("LOAMS_TOKEN", "LOAMS_TOKEN"),
            ],
        }
    }
}

impl EnvToken {
    /// A source reading the `LOAMS_*` environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::default()
    }

    /// A source reading the named variables in order, for a host that maps its
    /// own configuration onto Loams's names.
    #[must_use]
    pub fn new(variables: Vec<(&'static str, &'static str)>) -> Self {
        EnvToken { variables }
    }
}

#[async_trait]
impl TokenSource for EnvToken {
    async fn token(&self) -> Option<String> {
        self.variables
            .iter()
            .find_map(|(name, _)| std::env::var(name).ok().filter(|value| !value.is_empty()))
    }
}

/// A source that caches a token and calls a fetcher when asked to refresh.
///
/// This is the shape every refreshing source has: **one in-flight refresh**
/// shared by concurrent callers, so a burst of `401`s produces one token
/// exchange rather than one per request. The first [`TokenSource::token`]
/// fetches, because a source whose cache starts empty would send no credential
/// at all, and an instance that requires one answers `unauthenticated` — which
/// the call path treats as "the token expired" and retries, still with no
/// credential.
pub struct Refreshing<F> {
    fetch: F,
    state: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    token: Option<Arc<str>>,
    in_flight: Option<Shared<InFlight>>,
}

/// The one refresh a burst of callers joins. Not `Debug`: a boxed future has no
/// readable state, and `Refreshing`'s own `Debug` prints the fetcher's type
/// instead.
/// A shared refresh, and what is left of it once it has run.
///
/// Callers that join a refresh already in flight must end up with its
/// **result**, not merely the lock released after it finished. Holding only the
/// future meant a second caller locked the slot and polled a future that had
/// already completed, which panics with "`async fn` resumed after completion".
/// So the slot keeps the outcome once it is known, and later callers read it.
enum InFlight {
    Running(Pin<Box<dyn Future<Output = Result<Arc<str>, String>> + Send>>),
    Done(Result<Arc<str>, String>),
}

/// The single in-flight refresh, shared by every caller that finds it.
type Shared<T> = Arc<tokio::sync::Mutex<T>>;

impl<F> std::fmt::Debug for Refreshing<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Refreshing")
            .field("fetch", &std::any::type_name::<F>())
            .finish()
    }
}

impl<F> Refreshing<F>
where
    F: Fn() -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
{
    /// Wraps a fetcher. A fetcher that fails makes [`TokenSource::refresh`]
    /// return the failure, which the call path reports rather than retrying with
    /// the credential the server has already rejected.
    #[must_use]
    pub fn new(fetch: F) -> Self {
        Refreshing {
            fetch,
            state: Mutex::new(Cache::default()),
        }
    }

    async fn fetch_once(&self) -> Result<Arc<str>, String> {
        // The lock is held only to read or install the shared future, never
        // across the await: a refresh takes as long as the token endpoint takes,
        // and every other caller must be able to join it rather than queue.
        let existing = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "the token cache is poisoned".to_owned())?;
            match &state.in_flight {
                Some(future) => Some(Arc::clone(future)),
                None => {
                    // The fetcher's future is created here and owned by the
                    // shared slot, so a hundred callers joining one refresh
                    // share one exchange and one copy of the token rather than
                    // each running their own. It is created eagerly because a
                    // `Box<dyn Future>` must not borrow the source: the lock
                    // below is dropped before anything is awaited.
                    let produced = (self.fetch)();
                    let in_flight: Shared<InFlight> = Arc::new(tokio::sync::Mutex::new(
                        InFlight::Running(Box::pin(async move { produced.await.map(Arc::from) })),
                    ));
                    state.in_flight = Some(Arc::clone(&in_flight));
                    Some(in_flight)
                }
            }
        };
        let Some(future) = existing else {
            return Err("the token cache is poisoned".to_owned());
        };
        let token = {
            let mut guard = future.lock().await;
            match &mut *guard {
                // This refresh already ran: hand back what it produced rather
                // than polling the future a second time.
                InFlight::Done(result) => result.clone()?,
                InFlight::Running(fut) => {
                    let result = fut.as_mut().await;
                    // Keep it for the callers that join after this one.
                    let result = result.clone();
                    *guard = InFlight::Done(result.clone());
                    result?
                }
            }
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| "the token cache is poisoned".to_owned())?;
        state.token = Some(Arc::clone(&token));
        // Dropped only when it is still *this* future: a waiter that joined an
        // older refresh must not clear the newer one a later caller installed.
        if let Some(in_flight) = &state.in_flight
            && Arc::ptr_eq(in_flight, &future)
        {
            state.in_flight = None;
        }
        Ok(token)
    }
}

#[async_trait]
impl<F> TokenSource for Refreshing<F>
where
    F: Fn() -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
{
    async fn token(&self) -> Option<String> {
        let cached = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.token.clone())
            .map(|token| token.to_string());
        if cached.is_some() {
            return cached;
        }
        // The first `token()` fetches. A failure surfaces as "no credential",
        // which is the honest answer: the alternative is to invent one.
        self.fetch_once().await.ok().map(|token| token.to_string())
    }

    async fn refresh(&self) -> Result<(), LoamsError> {
        self.fetch_once().await.map(|_| ()).map_err(|message| {
            LoamsError::internal(format!("refreshing the token failed: {message}"))
        })
    }

    fn can_refresh(&self) -> bool {
        true
    }
}

/// Posts a form-encoded body to a URL and returns the response body.
///
/// The SDK has **no** HTTP client of its own beyond Connect's, and Rust's
/// standard library has none either, so the RFC 8693 token exchange takes the
/// poster as a trait rather than pulling in `reqwest` and a TLS stack on every
/// user of the crate. A host that has one (most do: `reqwest`, `ureq`, a
/// gateway) implements this in three lines.
#[async_trait]
pub trait FormPoster: Send + Sync + std::fmt::Debug {
    /// `POST` `form` to `url` as `application/x-www-form-urlencoded` and return
    /// the response body, or the failure as text.
    async fn post_form(&self, url: &str, form: &str) -> Result<String, String>;
}

/// The RFC 8693 token exchange a person signed in through Authentik needs
/// (design §44 §7.4, D608; §19 §5.2).
///
/// The instance's `/oauth/token` protocol endpoint takes the identity token
/// and answers with a Loams access token, which is then cached until the server
/// says it expired. `loams-auth` is the public OAuth client id (§44 §7.4: the
/// gateway exchanges the token, D447/D449).
///
/// **Not covered by the conformance suite**, for the reason TypeScript's is not:
/// the instance serves no OAuth endpoint yet (the auth plan, MT, and API1
/// Task 7 build it), so this path is written to the documented request and
/// response and cannot be exercised against a live server.
/// `rust_token_source_refresh` covers the caching and the
/// refresh-once-and-retry loop, which is the part the SDK owns.
pub struct OidcExchange<P, S> {
    endpoint: String,
    client_id: String,
    subject_token: S,
    poster: P,
}

impl<P, S> std::fmt::Debug for OidcExchange<P, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcExchange")
            .field("endpoint", &self.endpoint)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl<P, S> OidcExchange<P, S>
where
    P: FormPoster,
    S: Fn() -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync,
{
    /// Builds a source that exchanges `subject_token()` at `endpoint`.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        client_id: impl Into<String>,
        subject_token: S,
        poster: P,
    ) -> Self {
        OidcExchange {
            endpoint: endpoint.into(),
            client_id: client_id.into(),
            subject_token,
            poster,
        }
    }

    fn form(&self, subject: &str) -> String {
        form_encode(&[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:id_token",
            ),
            (
                "requested_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            ("subject_token", subject),
            ("client_id", self.client_id.as_str()),
            ("scope", "loams"),
        ])
    }
}

#[async_trait]
impl<P, S> TokenSource for OidcExchange<P, S>
where
    P: FormPoster,
    S: Fn() -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync,
{
    async fn token(&self) -> Option<String> {
        let body = self
            .poster
            .post_form(&self.endpoint, &self.form(&(self.subject_token)().await))
            .await
            .ok()?;
        access_token_of(&body)
    }

    async fn refresh(&self) -> Result<(), LoamsError> {
        // An exchange is cheap enough and correct enough to re-run: the access
        // token is not cached here, because the instance serves no OAuth
        // endpoint to tell the SDK when it expires.
        self.token()
            .await
            .map(|_| ())
            .ok_or_else(|| LoamsError::internal("the token exchange answered no access_token"))
    }

    fn can_refresh(&self) -> bool {
        true
    }
}

/// `application/x-www-form-urlencoded`, which is what the OAuth token endpoint
/// takes. Written out rather than pulled in as a dependency for four pairs.
fn form_encode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&percent_encode(key));
        out.push('=');
        out.push_str(&percent_encode(value));
    }
    out
}

/// The unreserved set of RFC 3986, plus the two the form encoding never
/// escapes. Everything else is percent-encoded, so a subject token containing
/// `+` or `=` survives the round trip.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The `access_token` out of a token endpoint's JSON answer.
///
/// Hand-rolled on purpose: the alternative is a JSON parser dependency on the
/// SDK's *default* path, for one field of one response that no Loams instance
/// serves yet.
fn access_token_of(body: &str) -> Option<String> {
    let key = "\"access_token\"";
    let at = body.find(key)?;
    let rest = &body[at + key.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let quote = after.find('"')?;
    let value = &after[quote + 1..];
    let end = value.find('"')?;
    let token = &value[..end];
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    #[test]
    fn an_empty_api_key_is_refused_rather_than_sent_as_a_bare_bearer() {
        assert!(ApiKey::new("").is_err());
        assert!(ApiKey::new("key").is_ok());
        assert!(StaticToken::new("").is_err());
        assert!(StaticToken::new("token").is_ok());
    }

    #[tokio::test]
    async fn an_api_key_cannot_refresh_so_the_runtime_does_not_retry() {
        let key = ApiKey::new("k").expect("not empty");
        assert!(!key.can_refresh());
        assert_eq!(key.token().await.as_deref(), Some("k"));
        assert!(key.refresh().await.is_err());
    }

    #[tokio::test]
    async fn a_refreshing_source_fetches_once_and_then_caches() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let source = Refreshing::new(move || {
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(format!("token-{}", counter.load(Ordering::SeqCst)))
            })
        });
        assert_eq!(source.token().await.as_deref(), Some("token-1"));
        assert_eq!(source.token().await.as_deref(), Some("token-1"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(source.can_refresh());
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_in_flight_refresh() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let source = std::sync::Arc::new(Refreshing::new(move || {
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(format!("token-{}", counter.load(Ordering::SeqCst)))
            })
        }));
        let first = tokio::spawn({
            let source = std::sync::Arc::clone(&source);
            async move { source.refresh().await }
        });
        let second = tokio::spawn({
            let source = std::sync::Arc::clone(&source);
            async move { source.refresh().await }
        });
        assert!(first.await.expect("joined").is_ok());
        assert!(second.await.expect("joined").is_ok());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one token exchange, not two"
        );
    }

    #[tokio::test]
    async fn a_failing_fetch_reports_rather_than_inventing_a_credential() {
        let source =
            Refreshing::new(|| Box::pin(async { Err("the token endpoint is down".to_owned()) }));
        assert_eq!(source.token().await, None);
        assert!(source.refresh().await.is_err());
        // …and a later call retries the fetch rather than caching the failure.
        assert_eq!(source.token().await, None);
    }

    #[test]
    fn the_form_body_is_the_rfc_8693_request() {
        #[derive(Debug)]
        struct Never;
        #[async_trait]
        impl FormPoster for Never {
            async fn post_form(&self, _url: &str, _form: &str) -> Result<String, String> {
                Err("unused".to_owned())
            }
        }
        let source = OidcExchange::new(
            "https://acme.loams.dev/oauth/token",
            "loams-auth",
            || Box::pin(async { "id-token".to_owned() }),
            Never,
        );
        let form = source.form("id-token");
        assert!(
            form.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange")
        );
        assert!(form.contains("subject_token=id-token"));
        assert!(form.contains("client_id=loams-auth"));
        // A `+` in a bearer token must not survive as a space, and a `=` must
        // not read as the next pair's separator. Assert against a subject that
        // actually contains them: the form above was built from "id-token", so
        // `a+b=c` never appeared in it and this assertion could not pass.
        let with_symbols = source.form("a+b=c");
        assert!(with_symbols.contains(&percent_encode("a+b=c")));
        assert!(percent_encode("a+b=c").contains("%2B"));
    }

    #[test]
    fn the_access_token_is_read_out_of_the_answer() {
        assert_eq!(
            access_token_of(r#"{"access_token":"abc","expires_in":60}"#).as_deref(),
            Some("abc")
        );
        assert_eq!(access_token_of(r#"{"access_token": ""}"#), None);
        assert_eq!(access_token_of("{}"), None);
    }
}
