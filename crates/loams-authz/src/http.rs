//! Deny-by-default enforcement for HTTP surfaces (design §76 §9.2, D1611).
//!
//! The authentication layer runs first: it verifies the credential and puts a [`Subject`] in
//! the request's extensions (and refuses a disabled principal, IA0 report F5). Then
//! [`enforce`] asks the route table what the route needs:
//!
//! - [`Declared::Check`]: one `can_*` relation on one object, checked through the
//!   [`Authorizer`]. Deny answers 403; an authorizer failure answers 503, never an allow.
//! - [`Declared::Authenticated`]: any admitted subject (for example "who am I").
//! - [`Declared::Public`]: no subject needed (health, discovery).
//! - nothing declared: **403**, whatever the subject. A new route is closed until someone
//!   declares what it needs.
//!
//! Every check, allowed or denied, goes to a [`DecisionLog`] (D1616); the default writes a
//! `tracing` event on target `loams_authz::decision`.
//!
//! A route table should split the path with [`segments`], which decodes it the way axum's
//! `Path` extractor does, so the check names the same object the handler acts on.
//!
//! ```ignore
//! let enforcer = Enforcer::new(authz, |m: &Method, p: &str| {
//!     let seg = segments(p)?;
//!     match (m, seg.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
//!         (&Method::GET, ["deployments", id]) => Need::new("can_view", ObjectRef::new("deployment", *id)).ok().map(Declared::Check),
//!         _ => None,
//!     }
//! });
//! let app = Router::new().route(...).layer(axum::middleware::from_fn_with_state(enforcer, enforce));
//! ```

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::{Authorizer, AuthzError, CheckRequest, Consistency, Decision, ObjectRef, Subject};

/// What a route needs: one `can_*` relation on one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    relation: String,
    object: ObjectRef,
    consistency: Consistency,
}

impl Need {
    /// `relation` must be a `can_*` relation: code never names a role (D1603). The object's
    /// type and id must be non-empty, without whitespace or `#`, and the type without `:`, so
    /// a path segment can never smuggle in a userset (`team:x#member`) or a second object.
    pub fn new(relation: impl Into<String>, object: ObjectRef) -> Result<Self, AuthzError> {
        let relation = relation.into();
        let well_formed = |r: &str| {
            r.strip_prefix("can_").is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
        };
        if !well_formed(&relation) {
            return Err(AuthzError::Invalid(format!(
                "routes check can_* relations only, not {relation:?}"
            )));
        }
        let bad = |s: &str| {
            s.is_empty()
                || s.chars()
                    .any(|c| c == '#' || c.is_whitespace() || c.is_control())
        };
        if bad(&object.ty) || object.ty.contains(':') || bad(&object.id) {
            return Err(AuthzError::Invalid(format!(
                "{object} is not a checkable object"
            )));
        }
        Ok(Self {
            relation,
            object,
            consistency: Consistency::MinimizeLatency,
        })
    }

    /// For destructive, admin and credential-issuing routes: `HigherConsistency`, which also
    /// bypasses every cache (D1611).
    #[must_use]
    pub fn strict(mut self) -> Self {
        self.consistency = Consistency::HigherConsistency;
        self
    }

    pub fn relation(&self) -> &str {
        &self.relation
    }

    pub fn object(&self) -> &ObjectRef {
        &self.object
    }

    pub fn consistency(&self) -> Consistency {
        self.consistency
    }
}

/// A route's declaration. A route the table does not declare is denied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declared {
    Check(Need),
    Authenticated,
    Public,
}

/// Maps a request to what it needs. `None` means undeclared, which is denied.
pub trait RouteTable: Send + Sync + 'static {
    fn declare(&self, method: &Method, path: &str) -> Option<Declared>;
}

impl<F> RouteTable for F
where
    F: Fn(&Method, &str) -> Option<Declared> + Send + Sync + 'static,
{
    fn declare(&self, method: &Method, path: &str) -> Option<Declared> {
        self(method, path)
    }
}

/// The path's segments after the leading `/`, each percent-decoded the way axum's `Path`
/// extractor decodes a parameter, so a route table checks the object the handler will act on.
///
/// `None` (the route is then undeclared, so 403) when a segment does not decode to UTF-8 or is
/// `.` or `..`. Empty segments are kept (`/a//b` is `["a", "", "b"]`, `/a/` is `["a", ""]`),
/// so a table that matches exact shapes does not match them.
pub fn segments(path: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix('/')?;
    rest.split('/')
        .map(|raw| {
            let seg = percent_encoding::percent_decode_str(raw)
                .decode_utf8()
                .ok()?;
            (seg != "." && seg != "..").then(|| seg.into_owned())
        })
        .collect()
}

/// One line of the decision log (§11).
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionRecord {
    pub subject: Subject,
    pub relation: String,
    pub object: ObjectRef,
    /// `None` when the authorizer failed (the request was refused with 503).
    pub decision: Option<Decision>,
    pub method: Method,
    pub path: String,
    pub request_id: Option<String>,
}

pub trait DecisionLog: Send + Sync + 'static {
    fn record(&self, record: &DecisionRecord);
}

/// The default log: one `tracing` event per decision on target `loams_authz::decision`
/// (denials at `info`, allows at `debug`, failures at `warn`).
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingLog;

impl DecisionLog for TracingLog {
    fn record(&self, r: &DecisionRecord) {
        let act: Vec<String> = r.subject.act.iter().map(Subject::fga_user).collect();
        let subject = r.subject.fga_user();
        let object = r.object.to_string();
        let request_id = r.request_id.as_deref().unwrap_or_default();
        match &r.decision {
            Some(d) if d.allowed => tracing::debug!(
                target: "loams_authz::decision", subject, ?act, relation = %r.relation, object,
                allowed = true, model_id = d.model_id.as_deref().unwrap_or_default(), cached = d.cached, request_id,
            ),
            Some(d) => tracing::info!(
                target: "loams_authz::decision", subject, ?act, relation = %r.relation, object,
                allowed = false, model_id = d.model_id.as_deref().unwrap_or_default(), cached = d.cached, request_id,
            ),
            None => tracing::warn!(
                target: "loams_authz::decision", subject, ?act, relation = %r.relation, object,
                allowed = false, error = true, request_id,
            ),
        }
    }
}

/// The middleware's state: the authorizer, the route table and the decision log.
#[derive(Clone)]
pub struct Enforcer {
    authz: Arc<dyn Authorizer>,
    routes: Arc<dyn RouteTable>,
    log: Arc<dyn DecisionLog>,
}

impl std::fmt::Debug for Enforcer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Enforcer").finish_non_exhaustive()
    }
}

impl Enforcer {
    pub fn new(authz: Arc<dyn Authorizer>, routes: impl RouteTable) -> Self {
        Self {
            authz,
            routes: Arc::new(routes),
            log: Arc::new(TracingLog),
        }
    }

    #[must_use]
    pub fn with_log(mut self, log: Arc<dyn DecisionLog>) -> Self {
        self.log = log;
        self
    }

    /// What `(method, path)` needs, for tests that every served route is declared.
    pub fn declared(&self, method: &Method, path: &str) -> Option<Declared> {
        self.routes.declare(method, path)
    }
}

fn refuse(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": msg }))).into_response()
}

/// The middleware: `axum::middleware::from_fn_with_state(enforcer, enforce)`.
pub async fn enforce(State(e): State<Enforcer>, req: Request, next: Next) -> Response {
    let declared = e.routes.declare(req.method(), req.uri().path());
    let need = match declared {
        Some(Declared::Public) => return next.run(req).await,
        None => {
            return refuse(
                StatusCode::FORBIDDEN,
                "no authorization is declared for this route",
            );
        }
        Some(Declared::Authenticated) => None,
        Some(Declared::Check(need)) => Some(need),
    };
    let Some(subject) = req.extensions().get::<Subject>().cloned() else {
        return refuse(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    let Some(need) = need else {
        return next.run(req).await;
    };
    let check = CheckRequest::new(subject.clone(), need.relation.clone(), need.object.clone())
        .consistency(need.consistency);
    let result = e.authz.check(check).await;
    e.log.record(&DecisionRecord {
        subject,
        relation: need.relation,
        object: need.object,
        decision: result.as_ref().ok().cloned(),
        method: req.method().clone(),
        path: req.uri().path().to_owned(),
        request_id: req
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    });
    match result {
        Ok(d) if d.allowed => next.run(req).await,
        Ok(_) => refuse(StatusCode::FORBIDDEN, "denied"),
        Err(_) => refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "authorization is unavailable",
        ),
    }
}
