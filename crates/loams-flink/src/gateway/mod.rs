//! The loams-gateway module for Flink SQL (D1977): the only way to a
//! tenant's SQL Gateway.
//!
//! Clients call `/flink/<tenant>/<SQL Gateway path>` with a Loams API key.
//! The router authenticates ([`auth`]), checks the tenant grant, checks the
//! request against what a tenant may do, and forwards it to that tenant's
//! SQL Gateway (which listens only on the cluster's private network):
//!
//! - **Paths** are an allowlist of the SQL Gateway API (v2, v3 and v4):
//!   `info`, `api_versions`, sessions, `heartbeat`, `configure-session`,
//!   `complete-statement`, statements, operations (status, cancel, close,
//!   results) and `scripts`. Anything else is 404 here, never forwarded.
//! - **Sessions are bound to the tenant** that opened them: a handle
//!   another tenant opened answers 404, as an unknown one would.
//! - **Cluster-owned options** ([`crate::config::PROTECTED_KEYS`]) cannot be
//!   set in session properties, `executionConfig` or a `SET` statement,
//!   except `execution.state-recovery.path` inside the tenant's prefix.
//! - **`scripts`** (application mode) takes a `scriptUri` only inside the
//!   tenant's prefix.
//! - The client's `authorization` header is never forwarded.
//!
//! The session bindings are in this process's memory: run one gateway
//! replica per Flink route until they move to a shared store (follow-up).
//! Wiring into the `loams-gateway` binary (`--module flink`) is a
//! follow-up in that crate.

pub mod auth;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::config::is_protected;
use crate::controller::ClusterRuntime;
use crate::storage::{ObjectStore, TenantId};
use auth::{Action, AuthError, Authenticator, Authorizer, Principal};

/// The longest request body forwarded (statements and scripts).
pub const MAX_BODY: usize = 1024 * 1024;

/// Where each tenant's SQL Gateway is.
pub trait UpstreamResolver: Send + Sync {
    /// The base URL of `tenant`'s SQL Gateway, if it has a cluster.
    fn sql_gateway(&self, tenant: &TenantId) -> Option<String>;
}

/// Fixed upstreams (compose: one tenant, `http://flink-sql-gateway:8083`).
#[derive(Debug, Clone, Default)]
pub struct StaticUpstreams(pub HashMap<TenantId, String>);

impl UpstreamResolver for StaticUpstreams {
    fn sql_gateway(&self, tenant: &TenantId) -> Option<String> {
        self.0.get(tenant).cloned()
    }
}

/// Upstreams from the controller's runtime (one namespace per tenant).
#[derive(Clone)]
pub struct RuntimeUpstreams(pub Arc<dyn ClusterRuntime>);

impl std::fmt::Debug for RuntimeUpstreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeUpstreams")
            .field("platform", &self.0.platform())
            .finish()
    }
}

impl UpstreamResolver for RuntimeUpstreams {
    fn sql_gateway(&self, tenant: &TenantId) -> Option<String> {
        Some(self.0.endpoints(tenant).sql_gateway)
    }
}

/// Who opened a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOwner {
    /// The tenant.
    pub tenant: TenantId,
    /// The principal's id.
    pub principal: String,
    /// When it was opened.
    pub opened: Instant,
}

/// The router's state.
#[derive(Clone)]
pub struct GatewayState {
    authn: Arc<dyn Authenticator>,
    authz: Arc<dyn Authorizer>,
    upstreams: Arc<dyn UpstreamResolver>,
    storage: ObjectStore,
    sessions: Arc<Mutex<HashMap<String, SessionOwner>>>,
    http: reqwest::Client,
}

impl std::fmt::Debug for GatewayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayState")
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

impl GatewayState {
    /// The state over these seams; `storage` is where tenants' prefixes
    /// are (for `scriptUri` and restore paths).
    pub fn new(
        authn: Arc<dyn Authenticator>,
        authz: Arc<dyn Authorizer>,
        upstreams: Arc<dyn UpstreamResolver>,
        storage: ObjectStore,
    ) -> Self {
        Self {
            authn,
            authz,
            upstreams,
            storage,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            http: reqwest::Client::new(),
        }
    }

    /// Another HTTP client for the upstream calls.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.http = client;
        self
    }

    /// The tenant's session bindings (for tests and admin).
    pub async fn sessions_of(&self, tenant: &TenantId) -> Vec<String> {
        let mut v: Vec<String> = self
            .sessions
            .lock()
            .await
            .iter()
            .filter(|(_, o)| o.tenant == *tenant)
            .map(|(h, _)| h.clone())
            .collect();
        v.sort();
        v
    }
}

/// The routes: `/flink/{tenant}/{*path}`, every method.
pub fn router(state: GatewayState) -> Router {
    Router::new()
        .route("/flink/{tenant}/{*path}", any(handle))
        .with_state(state)
}

/// A refusal, as Flink-shaped JSON (`{"errors": [...]}`).
fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    let body = json!({ "errors": [message.into()] });
    (status, axum::Json(body)).into_response()
}

fn auth_refusal(e: &AuthError) -> Response {
    match e {
        AuthError::Unauthenticated(_) => {
            let mut r = refuse(StatusCode::UNAUTHORIZED, e.to_string());
            r.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            r
        }
        AuthError::Forbidden(_) => refuse(StatusCode::FORBIDDEN, e.to_string()),
    }
}

/// What a request is, after matching the allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `GET api_versions`, `GET vN/info`.
    Meta,
    /// `POST vN/sessions`.
    OpenSession,
    /// `GET|DELETE vN/sessions/:h`, `POST …/heartbeat`.
    Session {
        /// The handle.
        handle: String,
        /// `DELETE`: the binding goes on success.
        closes: bool,
    },
    /// `POST …/configure-session`.
    Configure {
        /// The handle.
        handle: String,
    },
    /// `GET …/complete-statement`.
    Complete {
        /// The handle.
        handle: String,
    },
    /// `POST …/statements`.
    Execute {
        /// The handle.
        handle: String,
    },
    /// `…/operations/:o/(status|cancel|close|result/:token)`.
    Operation {
        /// The handle.
        handle: String,
    },
    /// `POST …/scripts` (v4).
    Script {
        /// The handle.
        handle: String,
    },
}

impl Call {
    fn handle(&self) -> Option<&str> {
        match self {
            Call::Meta | Call::OpenSession => None,
            Call::Session { handle, .. }
            | Call::Configure { handle }
            | Call::Complete { handle }
            | Call::Execute { handle }
            | Call::Operation { handle }
            | Call::Script { handle } => Some(handle),
        }
    }

    fn action(&self) -> Action {
        match self {
            Call::Meta | Call::Complete { .. } => Action::Read,
            Call::Script { .. } => Action::Deploy,
            _ => Action::Query,
        }
    }
}

fn is_segment(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Matches `method path` (the part after `/flink/<tenant>/`) against the
/// allowlist.
pub fn classify(method: &Method, path: &str) -> Option<Call> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    if segs.iter().any(|s| s.is_empty() || *s == "." || *s == "..") {
        return None;
    }
    if segs.as_slice() == ["api_versions"] {
        return (method == Method::GET).then_some(Call::Meta);
    }
    let (version, rest) = segs.split_first()?;
    let v = match *version {
        "v2" => 2,
        "v3" => 3,
        "v4" => 4,
        _ => return None,
    };
    let m = method.clone();
    match rest {
        ["info"] if m == Method::GET => Some(Call::Meta),
        ["api_versions"] if m == Method::GET => Some(Call::Meta),
        ["sessions"] if m == Method::POST => Some(Call::OpenSession),
        ["sessions", h] if is_segment(h) && (m == Method::GET || m == Method::DELETE) => {
            Some(Call::Session {
                handle: (*h).to_owned(),
                closes: m == Method::DELETE,
            })
        }
        ["sessions", h, "heartbeat"] if is_segment(h) && m == Method::POST => Some(Call::Session {
            handle: (*h).to_owned(),
            closes: false,
        }),
        ["sessions", h, "configure-session"] if is_segment(h) && m == Method::POST => {
            Some(Call::Configure {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "complete-statement"] if is_segment(h) && m == Method::GET => {
            Some(Call::Complete {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "statements"] if is_segment(h) && m == Method::POST => {
            Some(Call::Execute {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "operations", o, "status"]
            if is_segment(h) && is_segment(o) && m == Method::GET =>
        {
            Some(Call::Operation {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "operations", o, "cancel"]
            if is_segment(h) && is_segment(o) && m == Method::POST =>
        {
            Some(Call::Operation {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "operations", o, "close"]
            if is_segment(h) && is_segment(o) && m == Method::DELETE =>
        {
            Some(Call::Operation {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "operations", o, "result", token]
            if is_segment(h)
                && is_segment(o)
                && token.bytes().all(|b| b.is_ascii_digit())
                && m == Method::GET =>
        {
            Some(Call::Operation {
                handle: (*h).to_owned(),
            })
        }
        ["sessions", h, "scripts"] if v >= 4 && is_segment(h) && m == Method::POST => {
            Some(Call::Script {
                handle: (*h).to_owned(),
            })
        }
        _ => None,
    }
}

/// A `SET` in `sql` of a key the tenant may not set (with its value), if
/// any. Statements are split on `;` outside quotes; `--` and `/* */`
/// comments are skipped.
pub fn forbidden_set(sql: &str) -> Option<(String, String)> {
    for stmt in split_statements(sql) {
        let trimmed = stmt.trim_start();
        let Some(head) = trimmed.get(..3) else {
            continue;
        };
        if !head.eq_ignore_ascii_case("set") {
            continue;
        }
        let after = &trimmed[3..];
        if !after.starts_with(char::is_whitespace) {
            continue;
        }
        // SET 'key' = 'value'  |  SET key = value
        let after = after.trim();
        let (key, rest) = take_token(after);
        let rest = rest.trim_start().strip_prefix('=').unwrap_or(rest).trim();
        let (value, _) = take_token(rest);
        if is_protected(&key) {
            return Some((key, value));
        }
    }
    None
}

/// A quoted (`'…'`, `''` escapes) or bare token, and the rest.
fn take_token(s: &str) -> (String, &str) {
    if let Some(body) = s.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = body.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '\'' {
                if chars.peek().map(|(_, n)| *n) == Some('\'') {
                    out.push('\'');
                    chars.next();
                    continue;
                }
                return (out, &body[i + 1..]);
            }
            out.push(c);
        }
        (out, "")
    } else {
        let end = s
            .find(|c: char| c.is_whitespace() || c == '=' || c == ';')
            .unwrap_or(s.len());
        (s[..end].to_owned(), &s[end..])
    }
}

/// `sql` split into statements, comments removed, quotes respected.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                cur.push(c);
            }
            '-' if chars.peek() == Some(&'-') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
                cur.push(' ');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                cur.push(' ');
            }
            ';' => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Checks option maps (`properties`, `executionConfig`): no protected key,
/// except a restore path inside the tenant's prefix.
fn check_options(
    options: Option<&Value>,
    tenant: &TenantId,
    storage: &ObjectStore,
) -> Result<(), String> {
    let Some(Value::Object(map)) = options else {
        return Ok(());
    };
    for (k, v) in map {
        check_option(k, v.as_str().unwrap_or_default(), tenant, storage)?;
    }
    Ok(())
}

fn check_option(
    key: &str,
    value: &str,
    tenant: &TenantId,
    storage: &ObjectStore,
) -> Result<(), String> {
    if !is_protected(key) {
        return Ok(());
    }
    let restore = key.eq_ignore_ascii_case(crate::controller::STATE_RECOVERY_PATH)
        || key.eq_ignore_ascii_case("execution.savepoint.path");
    if restore && storage.tenant(tenant).contains(value) {
        return Ok(());
    }
    Err(format!("{key} is set by the cluster and cannot be changed"))
}

/// Checks the JSON body of `call`.
fn check_body(
    call: &Call,
    body: &Value,
    tenant: &TenantId,
    storage: &ObjectStore,
) -> Result<(), String> {
    match call {
        Call::OpenSession => check_options(body.get("properties"), tenant, storage),
        Call::Configure { .. } | Call::Execute { .. } => {
            let stmt = body
                .get("statement")
                .and_then(Value::as_str)
                .ok_or("a statement is required")?;
            if let Some((key, value)) = forbidden_set(stmt) {
                check_option(&key, &value, tenant, storage)?;
            }
            check_options(body.get("executionConfig"), tenant, storage)
        }
        Call::Script { .. } => {
            if let Some(uri) = body.get("scriptUri").and_then(Value::as_str)
                && !storage.tenant(tenant).contains(uri)
            {
                return Err(format!(
                    "scriptUri must be under {}",
                    storage.tenant(tenant).root_uri()
                ));
            }
            if let Some(script) = body.get("script").and_then(Value::as_str)
                && let Some((key, value)) = forbidden_set(script)
            {
                check_option(&key, &value, tenant, storage)?;
            }
            check_options(body.get("executionConfig"), tenant, storage)
        }
        _ => Ok(()),
    }
}

async fn handle(
    State(state): State<GatewayState>,
    Path((tenant, path)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(tenant) = TenantId::new(tenant) else {
        return refuse(StatusCode::NOT_FOUND, "no such Flink cluster");
    };
    let principal: Principal = match state.authn.authenticate(&headers).await {
        Ok(p) => p,
        Err(e) => return auth_refusal(&e),
    };
    let Some(call) = classify(&method, &path) else {
        return refuse(
            StatusCode::NOT_FOUND,
            "not a SQL Gateway endpoint this route serves",
        );
    };
    if let Err(e) = state.authz.authorize(&principal, &tenant, call.action()) {
        return auth_refusal(&e);
    }
    if body.len() > MAX_BODY {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the request body is over 1 MiB",
        );
    }
    if let Some(handle) = call.handle() {
        let owned = state
            .sessions
            .lock()
            .await
            .get(handle)
            .is_some_and(|o| o.tenant == tenant);
        if !owned {
            // Indistinguishable from an expired session.
            return refuse(
                StatusCode::NOT_FOUND,
                format!("session {handle} does not exist"),
            );
        }
    }
    if matches!(
        call,
        Call::OpenSession | Call::Configure { .. } | Call::Execute { .. } | Call::Script { .. }
    ) {
        let parsed: Value = if body.is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return refuse(
                        StatusCode::BAD_REQUEST,
                        format!("the body is not JSON: {e}"),
                    );
                }
            }
        };
        if let Err(why) = check_body(&call, &parsed, &tenant, &state.storage) {
            tracing::info!(%tenant, principal = %principal.id, %why, "refused a Flink SQL request");
            return refuse(StatusCode::FORBIDDEN, why);
        }
    }
    let Some(base) = state.upstreams.sql_gateway(&tenant) else {
        return refuse(StatusCode::NOT_FOUND, "no such Flink cluster");
    };
    let mut url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(&q);
    }
    let mut req = state
        .http
        .request(method.clone(), &url)
        .header(header::ACCEPT, "application/json");
    if !body.is_empty() {
        req = req
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_vec());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%tenant, error = %e, "the SQL Gateway did not answer");
            return refuse(
                StatusCode::BAD_GATEWAY,
                "the Flink SQL Gateway did not answer",
            );
        }
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                "the Flink SQL Gateway's answer broke off",
            );
        }
    };
    if status.is_success() {
        match &call {
            Call::OpenSession => {
                if let Some(h) = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| {
                    v.get("sessionHandle")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }) {
                    state.sessions.lock().await.insert(
                        h,
                        SessionOwner {
                            tenant: tenant.clone(),
                            principal: principal.id.clone(),
                            opened: Instant::now(),
                        },
                    );
                }
            }
            Call::Session {
                handle,
                closes: true,
            } => {
                state.sessions.lock().await.remove(handle);
            }
            _ => {}
        }
    } else if status == StatusCode::NOT_FOUND
        && let Some(h) = call.handle()
        && matches!(call, Call::Session { .. })
    {
        // The gateway expired it: forget the binding.
        state.sessions.lock().await.remove(h);
    }
    let mut out = http::Response::builder().status(status);
    if let Some(ct) = content_type {
        out = out.header(header::CONTENT_TYPE, ct);
    }
    out.body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| refuse(StatusCode::BAD_GATEWAY, "bad upstream response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_matches_the_sql_gateway_api() {
        let get = Method::GET;
        let post = Method::POST;
        let del = Method::DELETE;
        assert_eq!(classify(&get, "api_versions"), Some(Call::Meta));
        assert_eq!(classify(&get, "v3/info"), Some(Call::Meta));
        assert_eq!(classify(&post, "v3/sessions"), Some(Call::OpenSession));
        assert_eq!(
            classify(&del, "v3/sessions/abc-1"),
            Some(Call::Session {
                handle: "abc-1".into(),
                closes: true
            })
        );
        assert!(matches!(
            classify(&post, "v2/sessions/a/statements"),
            Some(Call::Execute { .. })
        ));
        assert!(matches!(
            classify(&get, "v3/sessions/a/operations/b/result/12"),
            Some(Call::Operation { .. })
        ));
        assert!(matches!(
            classify(&post, "v4/sessions/a/scripts"),
            Some(Call::Script { .. })
        ));
        // Not served: v1, scripts below v4, materialized tables, odd paths.
        assert_eq!(classify(&post, "v1/sessions"), None);
        assert_eq!(classify(&post, "v3/sessions/a/scripts"), None);
        assert_eq!(
            classify(&post, "v3/sessions/a/materialized-tables/x/refresh"),
            None
        );
        assert_eq!(classify(&get, "v3/sessions/../jobs"), None);
        assert_eq!(classify(&get, "v3/sessions/a/operations/b/result/x"), None);
        assert_eq!(classify(&post, "v3/info"), None);
        assert_eq!(classify(&get, "jobs/overview"), None);
    }

    #[test]
    fn protected_sets_are_found_past_comments_and_quotes() {
        assert_eq!(
            forbidden_set("SET 'state.checkpoints.dir' = 's3://x/y';"),
            Some(("state.checkpoints.dir".into(), "s3://x/y".into()))
        );
        assert!(forbidden_set("-- hi\nset   'S3.SECRET-KEY'='x'").is_some());
        assert!(forbidden_set("/* c */ SET high-availability.type = NONE").is_some());
        assert_eq!(forbidden_set("SET 'table.exec.state.ttl' = '1 h'"), None);
        assert_eq!(forbidden_set("SELECT 'SET state.backend.type = x'"), None);
        assert_eq!(forbidden_set("SETTINGS"), None);
        assert!(forbidden_set("SELECT 1; SET 'rest.port' = '1'").is_some());
    }

    #[test]
    fn restore_paths_must_be_the_tenants() {
        let storage = ObjectStore::default();
        let t1 = TenantId::new("t1").unwrap();
        assert!(
            check_option(
                "execution.state-recovery.path",
                "s3://flink/flink/t1/savepoints/sp",
                &t1,
                &storage
            )
            .is_ok()
        );
        assert!(
            check_option(
                "execution.state-recovery.path",
                "s3://flink/flink/t2/savepoints/sp",
                &t1,
                &storage
            )
            .is_err()
        );
        assert!(check_option("state.backend.type", "hashmap", &t1, &storage).is_err());
        assert!(check_option("pipeline.name", "x", &t1, &storage).is_ok());
        let script = json!({"scriptUri": "s3://flink/flink/t2/scripts/a.sql"});
        assert!(check_body(&Call::Script { handle: "h".into() }, &script, &t1, &storage).is_err());
        let script = json!({"scriptUri": "s3://flink/flink/t1/scripts/a.sql", "executionConfig": {"pipeline.name": "a"}});
        assert!(check_body(&Call::Script { handle: "h".into() }, &script, &t1, &storage).is_ok());
        let open = json!({"properties": {"kubernetes.namespace": "other"}});
        assert!(check_body(&Call::OpenSession, &open, &t1, &storage).is_err());
    }
}
