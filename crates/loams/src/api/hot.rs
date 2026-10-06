//! The hot routes (plan M1.3 Task 8 rules 1, 2 and 4): pinning a
//! collection's structures (`PUT …/hot`), warming it on its owner
//! (`POST …/warm`), and the `"hot"` value of `GET …/collections/{c}`, which
//! the owning node answers.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::{BytesRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{post, put};
use bytes::Bytes;
use loams_common::meta::{Consistency, HotConfig};
use loams_common::{CollectionId, NamespaceId};
use loams_hot::{DetailedHotStatus, HotStateKind, OwnerStatus, disabled_status};
use loams_query::ServiceError;
use loams_query::placement::Owner;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::errors::unavailable;
use super::internal::{HOT_STATUS_PATH, HOT_WARM_PATH};
use super::{ApiError, ApiResult, AppState, parse_json};

/// How long a call to the owner may take to connect, and in all (rule 4).
pub(crate) const OWNER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
pub(crate) const OWNER_TIMEOUT: Duration = Duration::from_secs(5);

/// PUT /v1/namespaces/{ns}/collections/{c}/hot and POST /v1/namespaces/{ns}/collections/{c}/warm.
pub fn routes() -> Router<AppState> {
    let collection = "/v1/namespaces/{ns}/collections/{c}";
    Router::new()
        .route(&format!("{collection}/hot"), put(set_hot))
        .route(&format!("{collection}/warm"), post(warm))
}

/// The body of `PUT …/hot`: absent keys are false.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HotBody {
    #[serde(default)]
    vectors: bool,
    #[serde(default)]
    text: bool,
    #[serde(default)]
    fragments: bool,
}

/// The body of an internal hot call: which collection.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HotTarget {
    pub(crate) ns: u64,
    pub(crate) cid: u64,
}

/// `{c}` of namespace `ns`, by name or alias; `404 not_found` when either
/// is missing.
pub(crate) async fn resolve(
    state: &AppState,
    ns: &str,
    name: &str,
) -> Result<(NamespaceId, CollectionId), ApiError> {
    let not_found = || {
        ApiError::from(ServiceError::NotFound {
            kind: "collection",
            name: name.to_string(),
        })
    };
    let Some(namespace) = state.meta.namespace_by_name(Consistency::Local, ns).await? else {
        return Err(not_found());
    };
    let collection = state
        .meta
        .resolve_collection(Consistency::Local, namespace.id, name)
        .await?
        .filter(|collection| collection.namespace == namespace.id)
        .ok_or_else(not_found)?;
    Ok((namespace.id, collection.id))
}

/// Rule 1: `PUT …/hot` sets the catalog configuration and answers with the
/// status.
async fn set_hot(
    State(state): State<AppState>,
    path: Result<Path<(String, String)>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult {
    let (Path((ns, name)), body) = (path?, body?);
    let value: Value = parse_json(&body)?;
    if !value.is_object() {
        return Err(ApiError::invalid("the hot body must be a JSON object"));
    }
    let request: HotBody = serde_json::from_value(value)
        .map_err(|err| ApiError::invalid(format!("bad request body: {err}")))?;
    let (ns_id, cid) = resolve(&state, &ns, &name).await?;
    let config = HotConfig {
        vectors: request.vectors,
        text: request.text,
        fragments: request.fragments,
    };
    state.meta.set_collection_hot(ns_id, cid, config).await?;
    let status = hot_status_value(&state, ns_id, cid).await;
    Ok(axum::Json(json!({ "hot": status })).into_response())
}

/// Rule 2: `POST …/warm` (an empty body or `{}`) warms the collection on
/// its owner and answers `202` with the status.
async fn warm(
    State(state): State<AppState>,
    path: Result<Path<(String, String)>, PathRejection>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult {
    let (Path((ns, name)), body) = (path?, body?);
    if !body.iter().all(u8::is_ascii_whitespace) {
        let fields: serde_json::Map<String, Value> = parse_json(&body)?;
        if !fields.is_empty() {
            return Err(ApiError::invalid("the warm body takes no fields"));
        }
    }
    let (ns_id, cid) = resolve(&state, &ns, &name).await?;
    match state.placement.owner(ns_id, cid) {
        Owner::Local => {
            let status = warm_local(&state, ns_id, cid).await?;
            Ok((StatusCode::ACCEPTED, axum::Json(json!({ "hot": status }))).into_response())
        }
        Owner::Remote { node_id, addr } => {
            let (status, body) = ask_owner_to_warm(&state, node_id, addr, ns_id, cid).await?;
            let body = match status.is_success() {
                true => json!({ "hot": body }),
                false => body,
            };
            Ok((status, axum::Json(body)).into_response())
        }
    }
}

/// Asks `(ns, cid)`'s owning node to warm it, over the internal route, and
/// answers the owner's `(status, body)` as it was.
///
/// Both surfaces reach the owner this way and neither re-implements the call:
/// the REST route puts the owner's status and body on the wire unchanged, and
/// [`warm_owner`] turns a refusal into an `ApiError` that keeps it.
pub(crate) async fn ask_owner_to_warm(
    state: &AppState,
    node_id: u64,
    addr: SocketAddr,
    ns: NamespaceId,
    cid: CollectionId,
) -> Result<(StatusCode, Value), ApiError> {
    let target = HotTarget {
        ns: ns.0,
        cid: cid.0,
    };
    let url = format!("http://{addr}{HOT_WARM_PATH}");
    let response = state
        .internal
        .post(url)
        .json(&target)
        .send()
        .await
        .map_err(|err| unavailable(format!("the owner node {node_id} is unreachable: {err}")))?;
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body: Value = response
        .json()
        .await
        .map_err(|err| unavailable(format!("the owner node {node_id} answered badly: {err}")))?;
    Ok((status, body))
}

/// The `ApiError` code an HTTP status a remote owner answered with maps to.
/// Only the statuses the owner's warm can answer with are listed; anything
/// else is a proxy or gateway answer and is reported as `internal`.
fn code_of(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "invalid_argument",
        404 => "not_found",
        409 => "already_exists",
        429 => "resource_exhausted",
        503 => "unavailable",
        504 => "timeout",
        _ => "internal",
    }
}

/// Warms `(ns, cid)` on its owner and answers the owner's hot status (rule 4).
///
/// `loams.collection.v1`'s `WarmCollection` calls this. A refusal from a
/// remote owner becomes an `ApiError` carrying the *owner's* status, so the
/// Connect error answers the status the REST route would have answered — the
/// REST route passes that `(status, body)` through untouched, which is why the
/// two do not share one function.
pub(crate) async fn warm_owner(
    state: &AppState,
    ns: NamespaceId,
    cid: CollectionId,
) -> Result<Value, ApiError> {
    match state.placement.owner(ns, cid) {
        Owner::Local => warm_local(state, ns, cid).await,
        Owner::Remote { node_id, addr } => {
            let (status, body) = ask_owner_to_warm(state, node_id, addr, ns, cid).await?;
            if !status.is_success() {
                return Err(ApiError::new(status, code_of(status), body.to_string()));
            }
            Ok(body)
        }
    }
}

/// Warms `(ns, cid)` on this node and returns its status: `400` when this
/// node's hot tier is off.
pub(crate) async fn warm_local(
    state: &AppState,
    ns: NamespaceId,
    cid: CollectionId,
) -> Result<Value, ApiError> {
    let Some(tier) = &state.hot else {
        return Err(ApiError::invalid(format!(
            "the hot tier is off on node {}",
            state.node_id
        )));
    };
    tier.start_warm(ns, cid)
        .map_err(|err| ApiError::invalid(err.to_string()))?;
    Ok(local_status(state, ns, cid).await)
}

/// The status of `(ns, cid)` on this node, as JSON: the tier's detailed
/// status, or [`disabled_status`] without a tier. A failed read answers
/// the disabled shape with every state `building`.
pub(crate) async fn local_status(state: &AppState, ns: NamespaceId, cid: CollectionId) -> Value {
    let owner = OwnerStatus {
        node_id: state.node_id,
        local: true,
    };
    let status = match &state.hot {
        Some(tier) => match tier.detailed_status(ns, cid).await {
            Ok(status) => status,
            Err(err) => {
                tracing::warn!(namespace = %ns, collection = %cid, %err, "reading the hot status failed");
                let mut status = disabled_status(HotConfig::default(), state.hot_pin_all, owner);
                status.enabled = true;
                building(&mut status);
                status
            }
        },
        None => {
            let config = state
                .meta
                .collection_hot(Consistency::Local, ns, cid)
                .await
                .unwrap_or_default();
            disabled_status(config, state.hot_pin_all, owner)
        }
    };
    to_value(&status)
}

fn building(status: &mut DetailedHotStatus) {
    status.vectors.state = HotStateKind::Building;
    status.text.state = HotStateKind::Building;
    status.fragments.state = HotStateKind::Building;
}

fn to_value(status: &DetailedHotStatus) -> Value {
    serde_json::to_value(status).unwrap_or(Value::Null)
}

/// The "hot" value of GET /v1/namespaces/{ns}/collections/{c} (B6), from the owning node (rule 4).
pub async fn hot_status_value(state: &AppState, ns: NamespaceId, cid: CollectionId) -> Value {
    match state.placement.owner(ns, cid) {
        Owner::Local => local_status(state, ns, cid).await,
        Owner::Remote { node_id, addr } => {
            let target = HotTarget {
                ns: ns.0,
                cid: cid.0,
            };
            let url = format!("http://{addr}{HOT_STATUS_PATH}");
            let answer = async {
                let response = state.internal.post(url).json(&target).send().await?;
                response.error_for_status()?.json::<Value>().await
            }
            .await;
            match answer {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!(namespace = %ns, collection = %cid, node_id, %err, "the owner's hot status is unavailable");
                    let config = state
                        .meta
                        .collection_hot(Consistency::Local, ns, cid)
                        .await
                        .unwrap_or_default();
                    let mut status = disabled_status(
                        config,
                        state.hot_pin_all,
                        OwnerStatus {
                            node_id,
                            local: false,
                        },
                    );
                    building(&mut status);
                    to_value(&status)
                }
            }
        }
    }
}
