//! The spike's Axum service (plan IA0 Tasks 3 and 4).
//!
//! * Every route but the Zitadel webhook needs a Zitadel JWT. The route table below declares the
//!   `can_*` relation and object of each route; a request that matches no declaration answers
//!   403 (deny by default, D1611).
//! * `POST /zitadel/events` verifies `ZITADEL-Signature` and applies Actions v2 events to
//!   OpenFGA: the `identity-sync` mapping of design 76 §8.1 in miniature.
//! * `/admin/*` (bearer `IA0_ADMIN_TOKEN`) lets the drivers seed tuples and read the event log.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::fga::{self, Consistency, Fga};
use crate::jwt::{Claims, Verifier};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    User,
    Agent,
    ServiceAccount,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Agent => "agent",
            Kind::ServiceAccount => "service_account",
        }
    }
}

/// The principal store's link `(issuer, sub) -> principal` (design 76 §5), in memory. Unknown
/// subjects are people (`user:<sub>`); machine users are linked as agents by the bootstrap and by
/// `user.machine.added`. A wrong kind fails closed: the other type holds no tuples.
#[derive(Clone, Debug, Serialize)]
pub struct Principal {
    pub kind: Kind,
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventRecord {
    pub event_type: String,
    pub aggregate_id: String,
    pub sequence: u64,
    pub created_at: String,
    pub received_unix_ms: u128,
    pub applied: bool,
    pub stale: bool,
    pub status: u16,
}

pub struct Inner {
    pub verifier: Verifier,
    pub fga: Fga,
    pub admin_token: String,
    pub event_key_file: Option<std::path::PathBuf>,
    pub principals: Mutex<HashMap<String, Principal>>,
    pub events: Mutex<Vec<EventRecord>>,
    /// Highest sequence applied per (aggregate, user), for out-of-order arrivals.
    pub applied_seq: Mutex<HashMap<(String, String), u64>>,
    /// When > 0, the next N webhook calls answer 500 (redelivery test).
    pub fail_next: Mutex<u32>,
}

pub type AppState = Arc<Inner>;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/zitadel/events", post(events))
        .route("/admin/tuples", post(admin_tuples))
        .route(
            "/admin/events",
            axum::routing::get(admin_events).delete(admin_events_clear),
        )
        .route("/admin/fail-next", post(admin_fail_next))
        .route("/admin/principals", post(admin_principals))
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .fallback(guarded)
        .with_state(state)
}

/// What a route needs. `None` means "any authenticated principal" (only `/whoami`).
struct Need {
    relation: Option<&'static str>,
    object: Option<String>,
    consistency: Consistency,
    list: Option<(&'static str, &'static str)>,
}

/// The declared route table. Anything absent is denied.
fn declared(method: &Method, path: &str) -> Option<Need> {
    let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
    let fast = Consistency::MinimizeLatency;
    let strong = Consistency::HigherConsistency;
    let n = |relation, object: String, consistency| {
        Some(Need {
            relation: Some(relation),
            object: Some(object),
            consistency,
            list: None,
        })
    };
    match (method, seg.as_slice()) {
        (&Method::GET, ["whoami"]) => Some(Need {
            relation: None,
            object: None,
            consistency: fast,
            list: None,
        }),
        (&Method::GET, ["deployments"]) => Some(Need {
            relation: None,
            object: None,
            consistency: fast,
            list: Some(("deployment", "can_view")),
        }),
        (&Method::GET, ["deployments", id]) => n("can_view", format!("deployment:{id}"), fast),
        (&Method::POST, ["deployments", id, "restart"]) => {
            n("can_operate", format!("deployment:{id}"), fast)
        }
        (&Method::DELETE, ["deployments", id]) => {
            n("can_destroy", format!("deployment:{id}"), strong)
        }
        (&Method::GET, ["databases", id]) => n("can_read", format!("database:{id}"), fast),
        (&Method::POST, ["databases", id, "connect"]) => {
            n("can_connect", format!("database:{id}"), fast)
        }
        _ => None,
    }
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, axum::Json(json!({ "error": msg.into() }))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn subject_of(state: &Inner, claims: &Claims) -> Result<String, Response> {
    let principals = state.principals.lock().await;
    let p = principals.get(&claims.sub).cloned().unwrap_or(Principal {
        kind: Kind::User,
        disabled: false,
    });
    if p.disabled {
        // The JWT is still cryptographically valid until `exp`; the principal store refuses it
        // (design 76 §10: "stops at once").
        return Err(err(StatusCode::UNAUTHORIZED, "principal is disabled"));
    }
    Ok(format!("{}:{}", p.kind.prefix(), claims.sub))
}

async fn guarded(State(state): State<AppState>, req: Request) -> Response {
    let started = Instant::now();
    let Some(token) = bearer(req.headers()) else {
        return err(StatusCode::UNAUTHORIZED, "missing bearer token");
    };
    let claims = match state.verifier.verify(token).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::UNAUTHORIZED, format!("invalid token: {e}")),
    };
    let verified = started.elapsed();
    let subject = match subject_of(&state, &claims).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(need) = declared(req.method(), req.uri().path()) else {
        return err(
            StatusCode::FORBIDDEN,
            "no authorization is declared for this route",
        );
    };
    let timing = |d: fga::Decision| {
        [
            ("x-ia0-cached", d.cached.to_string()),
            ("x-ia0-verify-us", verified.as_micros().to_string()),
            ("x-ia0-check-us", d.elapsed.as_micros().to_string()),
        ]
    };
    if let Some((ty, relation)) = need.list {
        return match state.fga.list_objects(&subject, relation, ty).await {
            Ok(mut objects) => {
                objects.sort();
                axum::Json(json!({ "subject": subject, "objects": objects })).into_response()
            }
            Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, format!("authorizer: {e}")),
        };
    }
    let (Some(relation), Some(object)) = (need.relation, need.object) else {
        return axum::Json(json!({ "subject": subject, "org": claims.org, "aud": claims.aud }))
            .into_response();
    };
    match state.fga.check(&subject, relation, &object, need.consistency, None).await {
        Ok(d) if d.allowed => {
            (StatusCode::OK, timing(d), axum::Json(json!({ "subject": subject, "allowed": relation, "object": object }))).into_response()
        }
        Ok(d) => (StatusCode::FORBIDDEN, timing(d), axum::Json(json!({ "error": "denied", "subject": subject, "relation": relation, "object": object }))).into_response(),
        // The authorizer being down is a deny, never an allow.
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, format!("authorizer: {e}")),
    }
}

fn admin_ok(state: &Inner, headers: &HeaderMap) -> bool {
    !state.admin_token.is_empty() && bearer(headers) == Some(state.admin_token.as_str())
}

#[derive(Deserialize)]
struct TupleReq {
    #[serde(default)]
    writes: Vec<(String, String, String)>,
    #[serde(default)]
    deletes: Vec<(String, String, String)>,
}

async fn admin_tuples(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<TupleReq>,
) -> Response {
    if !admin_ok(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let writes = body
        .writes
        .iter()
        .map(|(u, r, o)| fga::tuple(u, r, o))
        .collect();
    let deletes = body
        .deletes
        .iter()
        .map(|(u, r, o)| fga::key(u, r, o))
        .collect();
    match state.fga.write(writes, deletes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn admin_events(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_ok(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    axum::Json(state.events.lock().await.clone()).into_response()
}

async fn admin_events_clear(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_ok(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    state.events.lock().await.clear();
    state.applied_seq.lock().await.clear();
    StatusCode::NO_CONTENT.into_response()
}

async fn admin_fail_next(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !admin_ok(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    *state.fail_next.lock().await = body.trim().parse().unwrap_or(0);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct PrincipalReq {
    sub: String,
    kind: Kind,
}

async fn admin_principals(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<PrincipalReq>,
) -> Response {
    if !admin_ok(&state, &headers) {
        return err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    state.principals.lock().await.insert(
        body.sub,
        Principal {
            kind: body.kind,
            disabled: false,
        },
    );
    StatusCode::NO_CONTENT.into_response()
}

/// `ZITADEL-Signature: t=<unix>,v1=<hex HMAC-SHA256(key, "<t>." + body)>`, five minutes of
/// tolerance (Zitadel's documented scheme for Actions v2 targets).
pub fn verify_signature(header: &str, body: &[u8], key: &str, now_unix: i64) -> bool {
    let mut ts = None;
    let mut sigs = Vec::new();
    for part in header.split(',') {
        match part.split_once('=') {
            Some(("t", v)) => ts = v.parse::<i64>().ok(),
            Some(("v1", v)) => {
                if let Ok(s) = hex::decode(v) {
                    sigs.push(s);
                }
            }
            _ => {}
        }
    }
    let Some(ts) = ts else { return false };
    if (now_unix - ts).abs() > 300 {
        return false;
    }
    sigs.iter().any(|sig| {
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("hmac key");
        mac.update(ts.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        mac.verify_slice(sig).is_ok()
    })
}

#[derive(Deserialize, Debug)]
struct ZEvent {
    #[serde(rename = "aggregateID", default)]
    aggregate_id: String,
    #[serde(rename = "resourceOwner", default)]
    resource_owner: String,
    #[serde(default)]
    sequence: u64,
    #[serde(default)]
    event_type: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    event_payload: Option<Value>,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

async fn events(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let received = now_ms();
    let Some(key_file) = &state.event_key_file else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "no signing key configured");
    };
    let key = std::fs::read_to_string(key_file).unwrap_or_default();
    let sig = headers
        .get("zitadel-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let now = i64::try_from(received / 1000).unwrap_or(i64::MAX);
    if key.trim().is_empty() || !verify_signature(sig, &body, key.trim(), now) {
        return err(StatusCode::UNAUTHORIZED, "bad ZITADEL-Signature");
    }
    let ev: ZEvent = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("event body: {e}")),
    };
    let mut record = EventRecord {
        event_type: ev.event_type.clone(),
        aggregate_id: ev.aggregate_id.clone(),
        sequence: ev.sequence,
        created_at: ev.created_at.clone(),
        received_unix_ms: received,
        applied: false,
        stale: false,
        status: 200,
    };
    {
        let mut fail = state.fail_next.lock().await;
        if *fail > 0 {
            *fail -= 1;
            record.status = 500;
            state.events.lock().await.push(record);
            return err(StatusCode::INTERNAL_SERVER_ERROR, "injected failure");
        }
    }
    let result = apply(&state, &ev).await;
    match result {
        Ok((applied, stale)) => {
            record.applied = applied;
            record.stale = stale;
            state.events.lock().await.push(record);
            StatusCode::OK.into_response()
        }
        Err(e) => {
            record.status = 500;
            state.events.lock().await.push(record);
            err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

/// Applies one event. Returns (applied, stale). Membership events are applied by
/// (aggregate, sequence): an event older than the last one applied for the same membership is
/// skipped as stale, so out-of-order arrivals cannot resurrect a removed member.
async fn apply(state: &Inner, ev: &ZEvent) -> Result<(bool, bool)> {
    let payload = ev.event_payload.clone().unwrap_or(Value::Null);
    let user_in_payload = payload
        .get("userId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let membership = |org: &str| (format!("org:{org}"), format!("user:{user_in_payload}"));
    match ev.event_type.as_str() {
        // Zitadel org "members" are its org managers. User grants on the Loams project are the
        // better source for Loams membership; the spike maps both (report, finding F3).
        "org.member.added"
        | "org.member.removed"
        | "org.member.cascade.removed"
        | "user.grant.added"
        | "user.grant.removed"
        | "user.grant.cascade.removed" => {
            let org = if ev.event_type.starts_with("org.") {
                ev.aggregate_id.as_str()
            } else {
                ev.resource_owner.as_str()
            };
            if user_in_payload.is_empty() {
                return Ok((false, false));
            }
            let (object, user) = membership(org);
            // Sequences are per aggregate (the org for member events, the grant for user
            // grants), so the ordering slot is (aggregate, user).
            let slot = (ev.aggregate_id.clone(), user.clone());
            {
                let mut seq = state.applied_seq.lock().await;
                if seq.get(&slot).is_some_and(|last| *last >= ev.sequence) {
                    return Ok((false, true));
                }
                seq.insert(slot, ev.sequence);
            }
            if ev.event_type.ends_with(".added") {
                state
                    .fga
                    .write(vec![fga::tuple(&user, "member", &object)], Vec::new())
                    .await?;
            } else {
                state
                    .fga
                    .write(Vec::new(), vec![fga::key(&user, "member", &object)])
                    .await?;
            }
            Ok((true, false))
        }
        "user.machine.added" => {
            state
                .principals
                .lock()
                .await
                .entry(ev.aggregate_id.clone())
                .or_insert(Principal {
                    kind: Kind::Agent,
                    disabled: false,
                });
            Ok((true, false))
        }
        // Revocation (D1607, §8.1): disable the principal at once and delete its tuples.
        "user.deactivated" | "user.locked" | "user.removed" => {
            let kind = {
                let mut p = state.principals.lock().await;
                let e = p.entry(ev.aggregate_id.clone()).or_insert(Principal {
                    kind: Kind::User,
                    disabled: false,
                });
                e.disabled = true;
                e.kind
            };
            state
                .fga
                .delete_subject(&format!("{}:{}", kind.prefix(), ev.aggregate_id))
                .await?;
            Ok((true, false))
        }
        "user.reactivated" | "user.unlocked" => {
            if let Some(p) = state.principals.lock().await.get_mut(&ev.aggregate_id) {
                p.disabled = false;
            }
            Ok((true, false))
        }
        _ => Ok((false, false)),
    }
}

pub async fn serve(state: AppState, listen: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("ia0-authz listening on {listen}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

pub fn cache_ttl(ms: &str) -> Duration {
    Duration::from_millis(ms.parse().unwrap_or(5000))
}
