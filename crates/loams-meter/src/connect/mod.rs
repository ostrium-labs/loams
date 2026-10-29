//! The Connect face of the meter's services (`loams.meter.v1`, design §63
//! §17.1): each adapter turns the generated request into the domain call
//! and the domain answer (or [`MeterError`]) into the generated response.
//!
//! The caller is a [`Caller`] in the request's extensions. The role's
//! transport layer puts it there from the authenticated principal (or the
//! mTLS service identity); a request without one is `unauthenticated`.

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, as `crates/loams/src/api/connect.rs` does.
#![allow(refining_impl_trait)]

pub mod customers;
pub mod entitlements;
pub mod leases;
pub mod meters;
pub mod notifications;
pub mod plans;

use connectrpc::{ConnectError, ErrorCode, ErrorDetail, RequestContext};
use loams_meter_core::{Amount, Currency, Money, UtcNanos};
use loams_proto::google::protobuf::Timestamp;
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::meter::v1 as pb;

use crate::authz::Caller;
use crate::error::MeterError;

/// The caller of `ctx`'s request.
pub fn caller(ctx: &RequestContext) -> Result<Caller, ConnectError> {
    ctx.extensions()
        .get::<Caller>()
        .cloned()
        .ok_or_else(|| to_connect(MeterError::Unauthenticated))
}

/// A [`MeterError`] as a Connect error with one `loams.errors.v1.ErrorInfo`.
pub fn to_connect(e: MeterError) -> ConnectError {
    let code = match &e {
        MeterError::Unauthenticated => ErrorCode::Unauthenticated,
        MeterError::PermissionDenied { .. } => ErrorCode::PermissionDenied,
        MeterError::InvalidArgument { .. } => ErrorCode::InvalidArgument,
        MeterError::NotFound { .. } => ErrorCode::NotFound,
        MeterError::AlreadyExists { .. } => ErrorCode::AlreadyExists,
        MeterError::FailedPrecondition { .. } => ErrorCode::FailedPrecondition,
        MeterError::Refused { reason, .. } => match reason {
            loams_meter_core::RefusalReason::PaymentMethodRequired
            | loams_meter_core::RefusalReason::NotEntitled => ErrorCode::FailedPrecondition,
            loams_meter_core::RefusalReason::MeterUnavailable => ErrorCode::Unavailable,
            _ => ErrorCode::ResourceExhausted,
        },
        MeterError::Unavailable(_) => ErrorCode::Unavailable,
        MeterError::Unimplemented(_) => ErrorCode::Unimplemented,
    };
    let mut metadata = std::collections::HashMap::new();
    if let MeterError::InvalidArgument { field, .. } = &e {
        metadata.insert("field".to_string(), field.clone());
    }
    let info = ErrorInfo {
        reason: e.reason().to_string(),
        metadata: metadata.into_iter().collect(),
        ..Default::default()
    };
    ConnectError::new(code, e.to_string()).with_detail(ErrorDetail::from_message(
        "loams.errors.v1.ErrorInfo",
        &info,
    ))
}

/// `Result<T, MeterError>` into a Connect result.
pub trait IntoConnect<T> {
    fn connect(self) -> Result<T, ConnectError>;
}

impl<T> IntoConnect<T> for Result<T, MeterError> {
    fn connect(self) -> Result<T, ConnectError> {
        self.map_err(to_connect)
    }
}

pub fn timestamp(t: UtcNanos) -> Timestamp {
    Timestamp {
        seconds: t.0.div_euclid(1_000_000_000),
        nanos: i32::try_from(t.0.rem_euclid(1_000_000_000)).unwrap_or(0),
        ..Default::default()
    }
}

pub fn utc(t: &Timestamp) -> UtcNanos {
    UtcNanos(
        t.seconds
            .saturating_mul(1_000_000_000)
            .saturating_add(i64::from(t.nanos)),
    )
}

pub fn money_out(m: &Money) -> pb::Money {
    pb::Money {
        micros: m.micros,
        currency: m.currency.as_str().to_string(),
        ..Default::default()
    }
}

pub fn money_in(m: &pb::Money, field: &str) -> Result<Money, MeterError> {
    let currency =
        Currency::parse(&m.currency).map_err(|e| MeterError::invalid(field, e.to_string()))?;
    Ok(Money::new(m.micros, currency))
}

/// A decimal string field; empty is `None`.
pub fn amount_in(s: &str, field: &str) -> Result<Option<Amount>, MeterError> {
    if s.is_empty() {
        return Ok(None);
    }
    Amount::parse(s)
        .map(Some)
        .map_err(|e| MeterError::invalid(field, e.to_string()))
}

/// A page size: 0 asks for the default.
pub fn page_size(n: u32) -> usize {
    if n == 0 { 100 } else { n as usize }
}

/// An empty string as `None`.
pub fn nonempty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

/// Registers every meter service of this module on `router`.
pub fn register(
    router: connectrpc::Router,
    customers: crate::customers::Customers,
    entitlements: crate::entitlements::Entitlements,
    plans: crate::plans::Plans,
) -> connectrpc::Router {
    let router = customers::register(router, customers);
    let router = entitlements::register(router, entitlements);
    plans::register(router, plans)
}

/// Registers the internal `LeaseService` (service identities only), on the
/// listener product roles reach.
pub fn register_internal(
    router: connectrpc::Router,
    leases: std::sync::Arc<crate::lease_service::LeaseServer>,
) -> connectrpc::Router {
    leases::register(router, leases)
}
