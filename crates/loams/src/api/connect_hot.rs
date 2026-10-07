//! The `Loams-Hot` switch on the Connect router (Ruling 11; design §44 §4).
//!
//! [`loams_query::hot::HotLayer`] reads one request header, sets a task-local
//! scope and writes one response header — `loams-hot-used`. On the native REST
//! routes that is the whole of it, and its refusal of an unusable header is the
//! bare `invalid_argument` JSON body the REST routes answer with. On an RPC path
//! that body is **not** a Connect error: it carries no `code` and no
//! `loams.errors.v1.ErrorInfo` detail, so a Connect caller could not tell it from
//! any other 400 and could not branch on the registry reason (D611).
//!
//! So this module is the Connect half of the layer, and it is a **layer over the
//! connect router specifically** rather than over the router the REST routes
//! live in. That is the whole reason it is not simply "merge the connect routes
//! inside `hot_layer`":
//!
//! - The refusal's shape is decided from the request's content type.
//!   `HotLayer`'s own `grpc_content_type` recognises `application/grpc`,
//!   `application/grpc-web` and `application/grpc-web-text`; the Connect
//!   protocol's own two content types are `application/json` and
//!   `application/proto`, and `application/json` is **also** the native REST
//!   content type. One layer over both routers therefore cannot tell a Connect
//!   call from a REST one, and a bad header would answer a REST body to an RPC.
//! - Applied to the connect router alone, there is no ambiguity to resolve: every
//!   request reaching it is an RPC. Connect's content types mean Connect and
//!   answer a Connect error; everything else — gRPC, gRPC-Web, an unrecognised
//!   content type — falls through to [`HotLayer`], which already answers a
//!   trailers-only `grpc-status: 3` to a gRPC caller and the REST JSON body to
//!   anything else. No gRPC behaviour changes.
//!
//! ## What it does to the consistency-in-message guarantee: nothing
//!
//! The note this replaces (`api::router`, lines 132–134) said the connect routes
//! are merged **after** `HotLayer` because "a Connect call carries its own
//! consistency and pinning in the request message, not in the headers the layer
//! reads". That concern is real; it is now **resolved rather than avoided**:
//!
//! - [`HotLayer`] reads exactly one header, `loams-hot`, and so does
//!   [`reject_connect_header`]. Neither reads `loams-consistency-token` and
//!   neither rewrites the request message. Consistency is resolved inside the
//!   handler by [`read_consistency`](super::read_consistency), from the
//!   `consistency` field of the message **and** the
//!   `loams-consistency-token` header, in that order — the same function the
//!   REST route calls with the same precedence. Putting this layer in front of an
//!   RPC cannot make it see a consistency the caller did not state, because it
//!   never looks at consistency at all.
//! - The hot switch is the one thing that genuinely is a header on this surface
//!   rather than a field, and it is a **transport-level** switch: it says "may
//!   this read use a hot structure", not "which state may it read". There is no
//!   message-level meaning to carry, which is the same reason
//!   `loams-backpressure` stays a header on the document RPCs (Task 3's ruling).
//!
//! ## Blast radius, in full
//!
//! The connect routes are now inside `HotLayer`, where they were not. Their
//! answers carry `loams-hot-used`, and an RPC with an unusable `Loams-Hot` is
//! refused where it was not before. The health and reflection services ride the
//! same router and are affected the same way. Nothing else changes:
//! [`HotLayer`] itself is untouched, the REST routes keep the bare JSON refusal,
//! and the other `Loams-Hot` readers (`loams-es`, `loams-qdrant`, Flight, the
//! `loams-query` tests) are unaffected.

use axum::Router;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use loams_query::ServiceError;
use loams_query::hot::{HOT_HEADER, HotLayer, parse_hot_header};

/// The Connect protocol's own content types (`connectrpc::codec::content_type`),
/// which `HotLayer`'s own `grpc_content_type` does not recognise — deliberately,
/// because it cannot: `application/json` is the native REST content type too.
///
/// `application/connect+json` and `application/connect+proto` are Connect's
/// streaming variants and are recognised for the same reason.
fn is_connect_content_type(value: &str) -> bool {
    let media = value.split(';').next().unwrap_or("").trim();
    media.eq_ignore_ascii_case("application/json")
        || media.eq_ignore_ascii_case("application/proto")
        || media.eq_ignore_ascii_case("application/connect+json")
        || media.eq_ignore_ascii_case("application/connect+proto")
}

/// The message of a `parse_hot_header` refusal, which is the REST route's own
/// sentence and so is the same on both surfaces.
///
/// `parse_hot_header` raises nothing but `InvalidArgument`; the `other` arm is
/// unreachable today and reads its own `Display` rather than panicking, so a
/// future variant is answered rather than taken as a bug.
fn message_of(err: &ServiceError) -> String {
    match err {
        ServiceError::InvalidArgument(message) => message.clone(),
        other => other.to_string(),
    }
}

/// The Connect answer to an unusable `Loams-Hot`.
///
/// Built by [`super::connect_errors::invalid`], the one mapping every
/// `loams.*.v1` failure goes through, so the reason is `invalid_argument`
/// because that is the registry row and not because it was written here. The
/// error is serialised by connect-rust's own `to_json()` and its status taken
/// from its own `http_status()`, so this is byte-for-byte the body and status a
/// handler returning `Err(invalid(…))` produces — including the
/// `loams.errors.v1.ErrorInfo` detail and the registry reason a caller branches
/// on.
///
/// The prose is the REST route's, from `parse_hot_header`, and the REST route
/// re-words it as `invalid Loams-Hot header: {value}` in its own JSON body; the
/// RPC repeats that prefix so a caller reading either surface's message sees the
/// header name in it.
async fn reject_connect_header(request: Request, next: Next) -> Response {
    // Only the Connect protocol's own content types. A gRPC or gRPC-Web call on
    // this router is refused by `HotLayer` instead, which answers the
    // trailers-only `grpc-status: 3` response a gRPC caller parses — so this
    // middleware changing that would be a regression, not a fix.
    if !request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_connect_content_type)
    {
        return next.run(request).await;
    }
    let Some(value) = request
        .headers()
        .get(HOT_HEADER)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
    else {
        // No header: `HotLayer`'s `default_enabled` applies, and it is the only
        // thing that reads the service's default.
        return next.run(request).await;
    };
    if let Err(err) = parse_hot_header(&value) {
        let error = super::connect_errors::invalid(
            HOT_HEADER,
            format!("invalid Loams-Hot header: {}", message_of(&err)),
        );
        let mut response = (
            StatusCode::from_u16(error.http_status().as_u16()).unwrap_or(StatusCode::BAD_REQUEST),
            [(http::header::CONTENT_TYPE, "application/json")],
            error.to_json(),
        )
            .into_response();
        response.extensions_mut().insert(error);
        return response;
    }
    next.run(request).await
}

/// The connect routes with `HotLayer` on them, and in front of it the refusal
/// that layer owes an RPC in the Connect protocol's own shape.
///
/// `default_enabled` is the service's `hot_default`, the same value the REST
/// routes' layer is built with, so a request that names no `Loams-Hot` behaves
/// the same whichever surface it arrives on.
///
/// The order is the point: [`Router::layer`] wraps outward, so the rejection is
/// the outer layer and `HotLayer` the inner one — a refused header never reaches
/// `HotLayer`, which is what stops the bare REST JSON body from being the answer
/// to an RPC. A valid header, and every gRPC or gRPC-Web request, passes
/// straight through to `HotLayer` unchanged.
pub(crate) fn layer(routes: Router, default_enabled: bool) -> Router {
    let hot = HotLayer::new(default_enabled);
    routes
        .layer(hot)
        .layer(axum::middleware::from_fn(reject_connect_header))
}
