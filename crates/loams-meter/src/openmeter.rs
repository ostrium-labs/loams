//! The OpenMeter-compatible entitlement routes (design §63 §17.2; ME1 Task
//! 15), so OpenMeter SDKs and integrations read balances and manage grants
//! unchanged:
//!
//! | Path | OpenMeter operation |
//! |---|---|
//! | `GET /api/v1/subjects/{subject}/entitlements/{idOrFeature}/value` | `getEntitlementValue` |
//! | `GET /api/v1/subjects/{subject}/entitlements/{idOrFeature}/grants` | `listEntitlementGrants` |
//! | `POST /api/v1/subjects/{subject}/entitlements/{idOrFeature}/grants` | `createGrant` |
//!
//! Events and meter queries are ME-house's and the ingest's routes on the
//! same listener. The shapes are checked against OpenMeter's
//! `api/openapi.yaml` at a pinned commit by `tests/openmeter_conformance.rs`.
//!
//! What differs from OpenMeter, by design:
//! - OpenMeter's amounts are JSON numbers (doubles); Loams' are fixed-point
//!   decimals. This face converts at the edge (D2200); the Connect API keeps
//!   the exact decimal strings.
//! - OpenMeter grants always expire; a Loams grant may not. Such a grant is
//!   shown with OpenMeter's longest expiration, 1,000 years (D2200).
//! - A grant created here is `included` and records the reason
//!   `metadata.reason` (or "OpenMeter API").
//!
//! The role mounts this router behind its authentication layer, which puts
//! the [`Caller`] in the request's extensions.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use loams_meter_core::{
    Amount, Expiry, ExpiryUnit, GrantKind, Interval, Rollover, UsagePeriod, UtcNanos,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::authz::Caller;
use crate::entitlements::{EntitlementValue, Entitlements, NewGrant};
use crate::error::MeterError;

/// The base of OpenMeter's problem `type` URIs for Loams' reasons.
pub const PROBLEM_TYPE: &str = "https://loams.dev/problems/meter/";

/// The routes, over `entitlements`.
pub fn router(entitlements: Entitlements) -> Router {
    Router::new()
        .route(
            "/api/v1/subjects/{subject}/entitlements/{key}/value",
            get(value),
        )
        .route(
            "/api/v1/subjects/{subject}/entitlements/{key}/grants",
            get(list_grants).post(create_grant),
        )
        .with_state(entitlements)
}

/// An RFC 7807 problem in OpenMeter's shape (`UnexpectedProblemResponse`).
pub(crate) fn problem(e: &MeterError, instance: &str) -> Response {
    let status = match e {
        MeterError::Unauthenticated => StatusCode::UNAUTHORIZED,
        MeterError::PermissionDenied { .. } => StatusCode::FORBIDDEN,
        MeterError::InvalidArgument { .. } => StatusCode::BAD_REQUEST,
        MeterError::NotFound { .. } => StatusCode::NOT_FOUND,
        MeterError::AlreadyExists { .. } => StatusCode::CONFLICT,
        MeterError::FailedPrecondition { .. } => StatusCode::PRECONDITION_FAILED,
        MeterError::Refused { .. } => StatusCode::PAYMENT_REQUIRED,
        MeterError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        MeterError::Unimplemented(_) => StatusCode::NOT_IMPLEMENTED,
    };
    let body = json!({
        "type": format!("{PROBLEM_TYPE}{}", e.reason()),
        "title": status.canonical_reason().unwrap_or("Error"),
        "status": status.as_u16(),
        "detail": e.to_string(),
        "instance": instance,
        "extensions": { "reason": e.reason() },
    });
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
        body.to_string(),
    )
        .into_response()
}

fn caller(c: Option<Extension<Caller>>) -> Result<Caller, MeterError> {
    c.map(|Extension(c)| c).ok_or(MeterError::Unauthenticated)
}

/// A decimal as OpenMeter's JSON number.
pub fn number(a: Amount) -> Value {
    let f: f64 = a.to_string().parse().unwrap_or(0.0);
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}

fn rfc3339(t: UtcNanos) -> String {
    t.to_rfc3339()
}

/// `EntitlementValue`.
pub fn value_json(v: &EntitlementValue, static_config: Option<&Value>) -> Value {
    let mut out = json!({ "hasAccess": v.has_access });
    if v.period.is_some() {
        out["balance"] = number(v.balance);
        out["usage"] = number(v.usage);
        out["overage"] = number(v.overage);
        out["totalAvailableGrantAmount"] = number(v.total_available);
    }
    if let Some(c) = static_config {
        out["config"] = Value::String(c.to_string());
    }
    out
}

fn interval_out(i: Interval) -> &'static str {
    match i {
        // OpenMeter's enum has no hour; its ISO 8601 form does.
        Interval::Hour => "PT1H",
        Interval::Day => "DAY",
        Interval::Week => "WEEK",
        Interval::Month => "MONTH",
        Interval::Year => "YEAR",
    }
}

fn iso_of(i: Interval) -> &'static str {
    match i {
        Interval::Hour => "PT1H",
        Interval::Day => "P1D",
        Interval::Week => "P1W",
        Interval::Month => "P1M",
        Interval::Year => "P1Y",
    }
}

fn duration_out(u: ExpiryUnit) -> &'static str {
    match u {
        ExpiryUnit::Hour => "HOUR",
        ExpiryUnit::Day => "DAY",
        ExpiryUnit::Week => "WEEK",
        ExpiryUnit::Month => "MONTH",
        ExpiryUnit::Year => "YEAR",
    }
}

/// `EntitlementGrant`.
pub fn grant_json(g: &loams_meter_core::Grant) -> Value {
    let expiry = g.expires.unwrap_or(Expiry {
        duration: ExpiryUnit::Year,
        count: Expiry::MAX_COUNT,
    });
    let mut out = json!({
        "id": g.id.to_string(),
        "entitlementId": g.entitlement.to_string(),
        "amount": number(g.amount),
        "priority": g.priority,
        "effectiveAt": rfc3339(g.effective_at),
        "expiration": { "duration": duration_out(expiry.duration), "count": expiry.count },
        "expiresAt": rfc3339(expiry.after(g.effective_at)),
        "maxRolloverAmount": number(g.rollover.max),
        "minRolloverAmount": number(g.rollover.min),
        "createdAt": rfc3339(g.created_at),
        "updatedAt": rfc3339(g.voided_at.unwrap_or(g.created_at)),
    });
    if let Some(v) = g.voided_at {
        out["voidedAt"] = Value::String(rfc3339(v));
    }
    if let Some(r) = &g.recurrence {
        out["recurrence"] = json!({
            "interval": interval_out(r.interval),
            "intervalISO": iso_of(r.interval),
            "anchor": rfc3339(r.anchor),
        });
    }
    out
}

async fn value(
    State(e): State<Entitlements>,
    caller_ext: Option<Extension<Caller>>,
    Path((subject, key)): Path<(String, String)>,
) -> Response {
    let instance = format!("/api/v1/subjects/{subject}/entitlements/{key}/value");
    let run = async {
        let caller = caller(caller_ext)?;
        let ent = e.resolve(&caller, &subject, &key).await?;
        let v = e.value(&caller, &subject, ent.feature.as_str()).await?;
        let config = match &ent.kind {
            loams_meter_core::EntitlementKind::Static { config } => Some(config.clone()),
            _ => None,
        };
        Ok::<_, MeterError>(value_json(&v, config.as_ref()))
    };
    match run.await {
        Ok(v) => Json(v).into_response(),
        Err(err) => problem(&err, &instance),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    #[serde(default)]
    include_deleted: bool,
}

async fn list_grants(
    State(e): State<Entitlements>,
    caller_ext: Option<Extension<Caller>>,
    Path((subject, key)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> Response {
    let instance = format!("/api/v1/subjects/{subject}/entitlements/{key}/grants");
    let run = async {
        let caller = caller(caller_ext)?;
        let ent = e.resolve(&caller, &subject, &key).await?;
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) = e
                .list_grants(
                    &caller,
                    ent.id.as_str(),
                    q.include_deleted,
                    token.as_deref(),
                    500,
                )
                .await?;
            out.extend(page.iter().map(grant_json));
            match next {
                Some(n) => token = Some(n),
                None => break,
            }
        }
        Ok::<_, MeterError>(Value::Array(out))
    };
    match run.await {
        Ok(v) => Json(v).into_response(),
        Err(err) => problem(&err, &instance),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantInput {
    amount: f64,
    #[serde(default)]
    priority: Option<u8>,
    effective_at: String,
    expiration: ExpirationInput,
    #[serde(default)]
    max_rollover_amount: Option<f64>,
    #[serde(default)]
    min_rollover_amount: Option<f64>,
    #[serde(default)]
    recurrence: Option<RecurrenceInput>,
    #[serde(default)]
    metadata: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ExpirationInput {
    duration: String,
    count: u16,
}

#[derive(Debug, Deserialize)]
struct RecurrenceInput {
    interval: String,
    #[serde(default)]
    anchor: Option<String>,
}

fn amount_of(f: f64, field: &str) -> Result<Amount, MeterError> {
    if !f.is_finite() {
        return Err(MeterError::invalid(field, "must be a finite number"));
    }
    Amount::parse(&format!("{f}")).map_err(|e| MeterError::invalid(field, e.to_string()))
}

fn time_of(s: &str, field: &str) -> Result<UtcNanos, MeterError> {
    UtcNanos::from_rfc3339(s).map_err(|e| MeterError::invalid(field, e.to_string()))
}

fn interval_in(s: &str) -> Result<Interval, MeterError> {
    Ok(match s {
        "PT1H" => Interval::Hour,
        "DAY" | "P1D" => Interval::Day,
        "WEEK" | "P1W" => Interval::Week,
        "MONTH" | "P1M" => Interval::Month,
        "YEAR" | "P1Y" => Interval::Year,
        other => {
            return Err(MeterError::invalid(
                "recurrence.interval",
                format!("{other} is not one interval (PT1H, DAY, WEEK, MONTH, YEAR)"),
            ));
        }
    })
}

fn grant_in(
    entitlement: loams_meter_core::EntitlementId,
    g: GrantInput,
) -> Result<NewGrant, MeterError> {
    let duration = match g.expiration.duration.as_str() {
        "HOUR" => ExpiryUnit::Hour,
        "DAY" => ExpiryUnit::Day,
        "WEEK" => ExpiryUnit::Week,
        "MONTH" => ExpiryUnit::Month,
        "YEAR" => ExpiryUnit::Year,
        other => {
            return Err(MeterError::invalid(
                "expiration.duration",
                format!("{other} is not HOUR, DAY, WEEK, MONTH or YEAR"),
            ));
        }
    };
    let effective_at = time_of(&g.effective_at, "effectiveAt")?;
    let recurrence = g
        .recurrence
        .map(|r| {
            Ok::<_, MeterError>(UsagePeriod {
                interval: interval_in(&r.interval)?,
                anchor: match r.anchor {
                    Some(a) => time_of(&a, "recurrence.anchor")?,
                    None => effective_at,
                },
            })
        })
        .transpose()?;
    let reason = g
        .metadata
        .as_ref()
        .and_then(|m| m.get("reason").cloned())
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| "OpenMeter API".to_string());
    Ok(NewGrant {
        entitlement,
        amount: amount_of(g.amount, "amount")?,
        priority: g.priority.unwrap_or(1),
        effective_at: Some(effective_at),
        expires: Some(Expiry {
            duration,
            count: g.expiration.count,
        }),
        rollover: Rollover {
            max: g
                .max_rollover_amount
                .map(|f| amount_of(f, "maxRolloverAmount"))
                .transpose()?
                .unwrap_or(Amount::ZERO),
            min: g
                .min_rollover_amount
                .map(|f| amount_of(f, "minRolloverAmount"))
                .transpose()?
                .unwrap_or(Amount::ZERO),
        },
        recurrence,
        kind: GrantKind::Included,
        reason,
        idempotency_key: None,
    })
}

async fn create_grant(
    State(e): State<Entitlements>,
    caller_ext: Option<Extension<Caller>>,
    Path((subject, key)): Path<(String, String)>,
    body: Result<Json<GrantInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let instance = format!("/api/v1/subjects/{subject}/entitlements/{key}/grants");
    let run = async {
        let caller = caller(caller_ext)?;
        let Json(input) = body.map_err(|r| MeterError::invalid("body", r.body_text()))?;
        let ent = e.resolve(&caller, &subject, &key).await?;
        let new = grant_in(ent.id.clone(), input)?;
        let g = e.create_grant(&caller, new).await?;
        Ok::<_, MeterError>(grant_json(&g))
    };
    match run.await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(err) => problem(&err, &instance),
    }
}
