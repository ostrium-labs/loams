//! How every `loams.*.v1` failure becomes a Connect error (design §44 §7.4,
//! D611).
//!
//! There is **one** mapping, here, and the RPC modules
//! ([`super::connect_collections`], [`super::connect_documents`]) call it rather
//! than each building their own: a caller branches on the Connect `code` and
//! the registry `reason`, and both must be the same whichever service raised
//! them. [`super::connect::refuse`] builds the error; this module decides what
//! goes in it.
//!
//! `ApiError`'s own `code` becomes the `reason` unchanged (plan ruling 1.6), so
//! there is no second error hierarchy to keep in step with `docs/api/reasons.md`
//! — the REST route's `error` value and the RPC's `reason` are literally the
//! same string. `ApiError`'s structured fields become `ErrorInfo.metadata`:
//! `kind` and `name` for a missing collection, `field` for a schema violation,
//! `index` for a rejected write op, `matched` and `limit` for a filter write
//! over its budget, and `retry_after_ms` for a throttled write. A caller tells
//! a missing collection from a missing namespace, or refuses on the op that
//! failed, without parsing prose.
//!
//! The response headers the REST body would have set (`Retry-After`) are set
//! here too, through [`ApiError::headers`], because a Connect error carries its
//! headers rather than a response.

use connectrpc::{ConnectError, ErrorCode};
use loams_common::meta::MetaError;
use loams_query::ServiceError;

use super::ApiError;
use super::connect::refuse;

/// A malformed request: `invalid_argument` naming the field that is missing or
/// unusable. A request that names no resource is a malformed request, not a
/// missing one: `not_found` would put a name in `metadata` that no caller sent.
pub(super) fn invalid(field: &str, message: impl Into<String>) -> ConnectError {
    refuse(
        ErrorCode::InvalidArgument,
        "invalid_argument",
        message,
        &[("field", field.to_owned())],
    )
}

/// An `ApiError` as a Connect error, headers and all.
pub(super) fn refused(err: ApiError) -> ConnectError {
    refused_inner(err, axum::http::HeaderMap::new())
}

/// An `ApiError` as a Connect error with extra response headers — the backlog a
/// refused write was admitted at, which the REST route sets on the body it
/// returns for the same refusal (M1.3 Task 15 rule 5).
pub(super) fn refused_with(err: ApiError, headers: axum::http::HeaderMap) -> ConnectError {
    refused_inner(err, headers)
}

/// `ApiError` to a Connect error, plus whatever response headers the REST
/// route would have set on the same failure.
fn refused_inner(err: ApiError, extra: axum::http::HeaderMap) -> ConnectError {
    let mut headers = err.headers();
    for (name, value) in extra.iter() {
        headers.insert(name, value.clone());
    }
    let (code, reason) = classify(err.code());
    let metadata: Vec<(&str, String)> = err
        .extra()
        .iter()
        .filter_map(|(key, value)| {
            // `ErrorInfo.metadata` is `map<string, string>`, and the REST body
            // puts numbers in it as JSON numbers (`index`, `matched`, `limit`,
            // `retry_after_ms`). They are spelled as their decimal text here,
            // which is the same value a `u64` reaches the wire as elsewhere in
            // proto3 JSON — so a caller reads one number one way everywhere. A
            // structured field this server does not raise (an object, an
            // array) is dropped rather than stringified into something a caller
            // would parse as a number.
            scalar(value).map(|value| (key.as_str(), value))
        })
        .collect();
    let error = refuse(code, reason, err.message(), &metadata);
    if headers.is_empty() {
        error
    } else {
        error.with_headers(headers)
    }
}

/// A JSON scalar as the text an `ErrorInfo.metadata` value carries.
fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        serde_json::Value::Null => None,
        _ => None,
    }
}

/// A service failure as a Connect error, through the same `ApiError` the REST
/// route builds, so the two surfaces classify a failure identically.
pub(super) fn refused_service(err: ServiceError) -> ConnectError {
    refused(ApiError::from(err))
}

/// A metastore failure, the same way.
pub(super) fn refused_meta(err: MetaError) -> ConnectError {
    refused(ApiError::from(err))
}

/// The Connect code and registry reason an `ApiError` code maps to.
///
/// Every code `api::errors` raises is a registry row except three, which fold
/// into the row they mean: a `schema_violation` is an `invalid_argument` (the
/// offending field rides in `metadata`), a REST `conflict` is an `aborted` (a
/// fenced lease or a version mismatch is a concurrent write that won), and a
/// REST `timeout` is a `deadline_exceeded`. A code with no mapping is a layer
/// that grew one behind the registry, so it answers `internal` and is logged
/// rather than inventing a reason.
pub(super) fn classify(code: &str) -> (ErrorCode, &'static str) {
    match code {
        "invalid_argument" | "schema_violation" => (ErrorCode::InvalidArgument, "invalid_argument"),
        "not_found" => (ErrorCode::NotFound, "not_found"),
        "already_exists" => (ErrorCode::AlreadyExists, "already_exists"),
        "conflict" => (ErrorCode::Aborted, "aborted"),
        "resource_exhausted" => (ErrorCode::ResourceExhausted, "resource_exhausted"),
        "unavailable" => (ErrorCode::Unavailable, "unavailable"),
        "timeout" => (ErrorCode::DeadlineExceeded, "deadline_exceeded"),
        "permission_denied" => (ErrorCode::PermissionDenied, "permission_denied"),
        "unauthenticated" => (ErrorCode::Unauthenticated, "unauthenticated"),
        "internal" => (ErrorCode::Internal, "internal"),
        other => {
            tracing::error!(code = other, "an API error code has no Connect mapping");
            (ErrorCode::Internal, "internal")
        }
    }
}
