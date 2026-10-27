//! The read-only op allowlist (D660), ported from `factory/ops.ts` together with the
//! adapter calls each op makes. Every op is a GET; the parity suite
//! (`tests/parity.rs`) replays the TypeScript ops' recorded upstream and checks the
//! same requests go out and the same values come back.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::apps::AppId;
use crate::error::OpError;
use crate::params::Params;
use crate::upstream::Upstream;

mod forgejo;
mod glitchtip;
mod langfuse;
mod matomo;
mod openpanel;
mod plane;
mod signoz;
mod zulip;

/// One configured app, as the ops see it.
#[derive(Clone)]
pub struct Connection {
    /// The configured URL.
    pub url: String,
    /// Every field, secrets included; never logged.
    pub values: BTreeMap<String, String>,
    pub upstream: Arc<dyn Upstream>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("url", &self.url)
            .field("values", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Connection {
    /// A field's value, or the empty string.
    pub fn value(&self, key: &str) -> String {
        self.values.get(key).cloned().unwrap_or_default()
    }

    /// A non-empty field.
    pub fn field(&self, key: &str) -> Option<String> {
        self.values.get(key).filter(|v| !v.is_empty()).cloned()
    }
}

/// What an op runs with besides its params.
#[derive(Debug, Clone, Copy)]
pub struct OpContext {
    /// Milliseconds since the Unix epoch (Langfuse's daily window is relative to now).
    pub now_ms: i64,
}

/// A missing non-secret field: `bad_params`, as `need()` throws.
pub(crate) fn need(value: Option<String>, name: &str) -> Result<String, OpError> {
    value.ok_or_else(|| OpError::param(format!("{name} is required (set it in the app settings)")))
}

/// The ops an app serves (`health` included).
pub fn ops(app: AppId) -> &'static [&'static str] {
    match app {
        AppId::Forgejo => &["repos", "issues", "version", "health"],
        AppId::Zulip => &["streams", "messages", "server", "health"],
        AppId::Plane => &["stats", "issues", "health"],
        AppId::Glitchtip => &["organizations", "issues", "latestEvent", "health"],
        AppId::Openpanel => &["insights", "health"],
        AppId::Matomo => &["visits", "pages", "health"],
        AppId::Langfuse => &["traces", "daily", "health"],
        AppId::Signoz => &[],
    }
}

/// Check `params` against `op`'s schema.
///
/// # Errors
///
/// `bad_params`.
pub fn check(app: AppId, op: &str, params: &Value) -> Result<Params, OpError> {
    match app {
        AppId::Forgejo => forgejo::check(op, params),
        AppId::Zulip => zulip::check(op, params),
        AppId::Plane => plane::check(op, params),
        AppId::Glitchtip => glitchtip::check(op, params),
        AppId::Openpanel => openpanel::check(op, params),
        AppId::Matomo => matomo::check(op, params),
        AppId::Langfuse => langfuse::check(op, params),
        AppId::Signoz => Err(OpError::unsupported("SigNoz has no ops")),
    }
}

/// Run a checked op.
///
/// # Errors
///
/// What the app answered, classified by [`OpError::code`].
pub async fn run(
    app: AppId,
    op: &str,
    params: &Params,
    conn: &Connection,
    ctx: OpContext,
) -> Result<Value, OpError> {
    match app {
        AppId::Forgejo => forgejo::run(op, params, conn).await,
        AppId::Zulip => zulip::run(op, params, conn).await,
        AppId::Plane => plane::run(op, params, conn).await,
        AppId::Glitchtip => glitchtip::run(op, params, conn).await,
        AppId::Openpanel => openpanel::run(op, params, conn).await,
        AppId::Matomo => matomo::run(op, params, conn).await,
        AppId::Langfuse => langfuse::run(op, params, conn, ctx).await,
        AppId::Signoz => Err(OpError::unsupported("SigNoz has no ops")),
    }
}

/// The app's health check: `health` for an app with ops, SigNoz's `/api/v2/healthz`.
///
/// # Errors
///
/// The check failed.
pub async fn health(app: AppId, conn: &Connection, ctx: OpContext) -> Result<(), OpError> {
    if app == AppId::Signoz {
        return signoz::probe(conn).await;
    }
    let params = check(app, "health", &Value::Null)?;
    run(app, "health", &params, conn, ctx).await.map(|_| ())
}

/// `{status: "ok"}`, every health op's value.
pub(crate) fn ok_status() -> Value {
    serde_json::json!({ "status": "ok" })
}
