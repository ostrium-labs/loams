//! `loams.meter.v1.EntitlementService` over [`Entitlements`].

use std::sync::Arc;

use buffa::enumeration::EnumValue;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use loams_meter_core::{
    Amount, Entitlement, EntitlementKind, EntitlementSource, Expiry, ExpiryUnit, Grant, GrantKind,
    Interval, Mode, Rollover, UsagePeriod,
};
use loams_proto::loams::meter::v1 as pb;
use loams_proto::loams::meter::v1::__buffa::oneof::entitlement::Kind as PbKind;
use loams_proto::loams::meter::v1::{EntitlementService, EntitlementServiceExt};

use super::{
    IntoConnect, amount_in, caller, money_in, money_out, nonempty, page_size, timestamp,
    to_connect, utc,
};
use crate::entitlements::{EntitlementValue, Entitlements, NewGrant};
use crate::error::MeterError;

#[derive(Debug)]
struct Adapter(Entitlements);

pub fn register(router: connectrpc::Router, entitlements: Entitlements) -> connectrpc::Router {
    EntitlementServiceExt::register(Arc::new(Adapter(entitlements)), router)
}

fn interval_out(i: Interval) -> pb::Interval {
    match i {
        Interval::Hour => pb::Interval::INTERVAL_HOUR,
        Interval::Day => pb::Interval::INTERVAL_DAY,
        Interval::Week => pb::Interval::INTERVAL_WEEK,
        Interval::Month => pb::Interval::INTERVAL_MONTH,
        Interval::Year => pb::Interval::INTERVAL_YEAR,
    }
}

fn interval_in(i: &EnumValue<pb::Interval>, field: &str) -> Result<Interval, MeterError> {
    match i.as_known() {
        Some(pb::Interval::INTERVAL_HOUR) => Ok(Interval::Hour),
        Some(pb::Interval::INTERVAL_DAY) => Ok(Interval::Day),
        Some(pb::Interval::INTERVAL_WEEK) => Ok(Interval::Week),
        Some(pb::Interval::INTERVAL_MONTH) => Ok(Interval::Month),
        Some(pb::Interval::INTERVAL_YEAR) => Ok(Interval::Year),
        _ => Err(MeterError::invalid(field, "an interval is required")),
    }
}

fn period_out(p: &UsagePeriod) -> pb::UsagePeriod {
    pb::UsagePeriod {
        interval: EnumValue::Known(interval_out(p.interval)),
        anchor: timestamp(p.anchor).into(),
        ..Default::default()
    }
}

fn period_in(p: &pb::UsagePeriod, field: &str) -> Result<UsagePeriod, MeterError> {
    Ok(UsagePeriod {
        interval: interval_in(&p.interval, field)?,
        anchor: p.anchor.as_option().map(utc).unwrap_or_default(),
    })
}

/// An entitlement as the API answers it.
pub fn entitlement_out(e: &Entitlement) -> pb::Entitlement {
    let kind = match &e.kind {
        EntitlementKind::Metered {
            usage_period,
            mode,
            issue_after_reset,
            preserve_overage_at_reset,
        } => {
            let (m, price, cap) = match mode {
                Mode::HardLimit => (pb::EntitlementMode::ENTITLEMENT_MODE_HARD_LIMIT, None, None),
                Mode::SoftLimit => (pb::EntitlementMode::ENTITLEMENT_MODE_SOFT_LIMIT, None, None),
                Mode::OverageAllowed {
                    overage_price,
                    spend_cap,
                } => (
                    pb::EntitlementMode::ENTITLEMENT_MODE_OVERAGE_ALLOWED,
                    Some(money_out(overage_price)),
                    Some(money_out(spend_cap)),
                ),
            };
            PbKind::Metered(Box::new(pb::MeteredEntitlement {
                usage_period: period_out(usage_period).into(),
                mode: EnumValue::Known(m),
                issue_after_reset: issue_after_reset.map(|a| a.to_string()).unwrap_or_default(),
                preserve_overage_at_reset: *preserve_overage_at_reset,
                overage_price: price.into(),
                spend_cap: cap.into(),
                ..Default::default()
            }))
        }
        EntitlementKind::Boolean { enabled } => PbKind::Toggle(Box::new(pb::BooleanEntitlement {
            enabled: *enabled,
            ..Default::default()
        })),
        EntitlementKind::Static { config } => {
            PbKind::StaticConfig(Box::new(pb::StaticEntitlement {
                config_json: config.to_string(),
                ..Default::default()
            }))
        }
    };
    let (plan, plan_version, created_by) = match &e.source {
        EntitlementSource::Plan { plan, version } => {
            (plan.as_str().to_string(), *version, String::new())
        }
        EntitlementSource::Manual { by } => (String::new(), 0, by.clone()),
    };
    pb::Entitlement {
        id: e.id.to_string(),
        subject: e.subject.as_str().to_string(),
        feature: e.feature.as_str().to_string(),
        kind: Some(kind),
        create_time: timestamp(e.created_at).into(),
        plan,
        plan_version,
        created_by,
        ..Default::default()
    }
}

/// The kind of an entitlement to create.
pub fn kind_in(kind: Option<&PbKind>) -> Result<EntitlementKind, MeterError> {
    match kind {
        None => Err(MeterError::invalid(
            "entitlement.kind",
            "metered, toggle or static_config is required",
        )),
        Some(PbKind::Toggle(b)) => Ok(EntitlementKind::Boolean { enabled: b.enabled }),
        Some(PbKind::StaticConfig(s)) => {
            let config = serde_json::from_str(&s.config_json).map_err(|e| {
                MeterError::invalid("entitlement.static_config.config_json", e.to_string())
            })?;
            Ok(EntitlementKind::Static { config })
        }
        Some(PbKind::Metered(m)) => {
            let usage_period = m
                .usage_period
                .as_option()
                .map(|p| period_in(p, "entitlement.metered.usage_period"))
                .transpose()?
                .ok_or_else(|| {
                    MeterError::invalid(
                        "entitlement.metered.usage_period",
                        "a usage period is required",
                    )
                })?;
            let mode = match m.mode.as_known() {
                Some(pb::EntitlementMode::ENTITLEMENT_MODE_HARD_LIMIT) => Mode::HardLimit,
                Some(pb::EntitlementMode::ENTITLEMENT_MODE_SOFT_LIMIT) => Mode::SoftLimit,
                Some(pb::EntitlementMode::ENTITLEMENT_MODE_OVERAGE_ALLOWED) => {
                    let price = m.overage_price.as_option().ok_or_else(|| {
                        MeterError::invalid(
                            "entitlement.metered.overage_price",
                            "required with overage",
                        )
                    })?;
                    let cap = m.spend_cap.as_option().ok_or_else(|| {
                        MeterError::invalid(
                            "entitlement.metered.spend_cap",
                            "required with overage",
                        )
                    })?;
                    Mode::OverageAllowed {
                        overage_price: money_in(price, "entitlement.metered.overage_price")?,
                        spend_cap: money_in(cap, "entitlement.metered.spend_cap")?,
                    }
                }
                _ => {
                    return Err(MeterError::invalid(
                        "entitlement.metered.mode",
                        "a mode is required",
                    ));
                }
            };
            Ok(EntitlementKind::Metered {
                usage_period,
                mode,
                issue_after_reset: amount_in(
                    &m.issue_after_reset,
                    "entitlement.metered.issue_after_reset",
                )?,
                preserve_overage_at_reset: m.preserve_overage_at_reset,
            })
        }
    }
}

fn expiry_unit_out(u: ExpiryUnit) -> pb::ExpiryUnit {
    match u {
        ExpiryUnit::Hour => pb::ExpiryUnit::EXPIRY_UNIT_HOUR,
        ExpiryUnit::Day => pb::ExpiryUnit::EXPIRY_UNIT_DAY,
        ExpiryUnit::Week => pb::ExpiryUnit::EXPIRY_UNIT_WEEK,
        ExpiryUnit::Month => pb::ExpiryUnit::EXPIRY_UNIT_MONTH,
        ExpiryUnit::Year => pb::ExpiryUnit::EXPIRY_UNIT_YEAR,
    }
}

fn expiry_in(g: &pb::Grant) -> Result<Option<Expiry>, MeterError> {
    if g.expiry_count == 0 {
        return Ok(None);
    }
    let duration = match g.expiry_unit.as_known() {
        Some(pb::ExpiryUnit::EXPIRY_UNIT_HOUR) => ExpiryUnit::Hour,
        Some(pb::ExpiryUnit::EXPIRY_UNIT_DAY) => ExpiryUnit::Day,
        Some(pb::ExpiryUnit::EXPIRY_UNIT_WEEK) => ExpiryUnit::Week,
        Some(pb::ExpiryUnit::EXPIRY_UNIT_MONTH) => ExpiryUnit::Month,
        Some(pb::ExpiryUnit::EXPIRY_UNIT_YEAR) => ExpiryUnit::Year,
        _ => {
            return Err(MeterError::invalid(
                "grant.expiry_unit",
                "required with expiry_count",
            ));
        }
    };
    let count = u16::try_from(g.expiry_count)
        .ok()
        .filter(|c| (1..=Expiry::MAX_COUNT).contains(c))
        .ok_or_else(|| MeterError::invalid("grant.expiry_count", "must be 1..=1000"))?;
    Ok(Some(Expiry { duration, count }))
}

/// A grant as the API answers it.
pub fn grant_out(g: &Grant) -> pb::Grant {
    let (kind, paid) = match g.kind {
        GrantKind::Promotional => (pb::GrantKind::GRANT_KIND_PROMOTIONAL, None),
        GrantKind::Included => (pb::GrantKind::GRANT_KIND_INCLUDED, None),
        GrantKind::Prepaid { paid } => (pb::GrantKind::GRANT_KIND_PREPAID, Some(money_out(&paid))),
    };
    pb::Grant {
        id: g.id.to_string(),
        entitlement_id: g.entitlement.to_string(),
        amount: g.amount.to_string(),
        priority: u32::from(g.priority),
        effective_time: timestamp(g.effective_at).into(),
        expiry_unit: g
            .expires
            .map(|e| EnumValue::Known(expiry_unit_out(e.duration)))
            .unwrap_or_default(),
        expiry_count: g.expires.map_or(0, |e| u32::from(e.count)),
        rollover_max: g.rollover.max.to_string(),
        rollover_min: g.rollover.min.to_string(),
        recurrence: g.recurrence.as_ref().map(period_out).into(),
        kind: EnumValue::Known(kind),
        paid: paid.into(),
        void_time: g.voided_at.map(timestamp).into(),
        create_time: timestamp(g.created_at).into(),
        ..Default::default()
    }
}

fn grant_in(g: &pb::Grant, reason: String, key: Option<String>) -> Result<NewGrant, MeterError> {
    let entitlement = loams_meter_core::EntitlementId::new(g.entitlement_id.clone())
        .map_err(|e| MeterError::invalid("grant.entitlement_id", e.to_string()))?;
    let amount = amount_in(&g.amount, "grant.amount")?
        .ok_or_else(|| MeterError::invalid("grant.amount", "an amount is required"))?;
    let priority = u8::try_from(if g.priority == 0 { 1 } else { g.priority })
        .map_err(|_| MeterError::invalid("grant.priority", "must be 1..=255"))?;
    let kind = match g.kind.as_known() {
        Some(pb::GrantKind::GRANT_KIND_PROMOTIONAL) => GrantKind::Promotional,
        Some(pb::GrantKind::GRANT_KIND_INCLUDED)
        | None
        | Some(pb::GrantKind::GRANT_KIND_UNSPECIFIED) => GrantKind::Included,
        Some(pb::GrantKind::GRANT_KIND_PREPAID) => {
            let paid = g.paid.as_option().ok_or_else(|| {
                MeterError::invalid("grant.paid", "a prepaid grant records what was paid")
            })?;
            GrantKind::Prepaid {
                paid: money_in(paid, "grant.paid")?,
            }
        }
    };
    // Without a rollover, nothing carries over a reset (OpenMeter's
    // default: max 0, min 0).
    let rollover = Rollover {
        max: amount_in(&g.rollover_max, "grant.rollover_max")?.unwrap_or(Amount::ZERO),
        min: amount_in(&g.rollover_min, "grant.rollover_min")?.unwrap_or(Amount::ZERO),
    };
    Ok(NewGrant {
        entitlement,
        amount,
        priority,
        effective_at: g.effective_time.as_option().map(utc),
        expires: expiry_in(g)?,
        rollover,
        recurrence: g
            .recurrence
            .as_option()
            .map(|p| period_in(p, "grant.recurrence"))
            .transpose()?,
        kind,
        reason,
        idempotency_key: key,
    })
}

fn value_out(v: &EntitlementValue) -> pb::EntitlementValue {
    pb::EntitlementValue {
        has_access: v.has_access,
        balance: v.balance.to_string(),
        usage: v.usage.to_string(),
        overage: v.overage.to_string(),
        total_available: v.total_available.to_string(),
        period_start: v.period.map(|p| timestamp(p.start)).into(),
        period_end: v.period.map(|p| timestamp(p.end)).into(),
        as_of: match v.as_of {
            Some(loams_meter_core::AsOf::Counter { .. }) => "counter".into(),
            Some(loams_meter_core::AsOf::House { .. }) => "house".into(),
            Some(loams_meter_core::AsOf::Replay) => "replay".into(),
            None => String::new(),
        },
        ..Default::default()
    }
}

impl EntitlementService for Adapter {
    async fn create_entitlement(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateEntitlementRequest>,
    ) -> ServiceResult<pb::CreateEntitlementResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let e = r.entitlement.as_option().cloned().unwrap_or_default();
        let kind = kind_in(e.kind.as_ref()).connect()?;
        let created = self
            .0
            .create(
                &caller,
                &e.subject,
                &e.feature,
                kind,
                &r.reason,
                nonempty(&r.idempotency_key),
            )
            .await
            .connect()?;
        Response::ok(pb::CreateEntitlementResponse {
            entitlement: entitlement_out(&created).into(),
            ..Default::default()
        })
    }

    async fn list_entitlements(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListEntitlementsRequest>,
    ) -> ServiceResult<pb::ListEntitlementsResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let (page, next) = self
            .0
            .list(
                &caller,
                nonempty(&r.subject),
                nonempty(&r.feature),
                nonempty(&r.page_token),
                page_size(r.page_size),
            )
            .await
            .connect()?;
        Response::ok(pb::ListEntitlementsResponse {
            entitlements: page.iter().map(entitlement_out).collect(),
            next_page_token: next.unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn get_entitlement_value(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetEntitlementValueRequest>,
    ) -> ServiceResult<pb::GetEntitlementValueResponse> {
        let caller = caller(&ctx)?;
        let v = self
            .0
            .value(&caller, request.subject, request.feature)
            .await
            .connect()?;
        Response::ok(pb::GetEntitlementValueResponse {
            value: value_out(&v).into(),
            ..Default::default()
        })
    }

    async fn check_access(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CheckAccessRequest>,
    ) -> ServiceResult<pb::CheckAccessResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let checks: Vec<(String, String)> = r
            .checks
            .iter()
            .map(|c| (c.subject.clone(), c.feature.clone()))
            .collect();
        let results = self.0.check_access(&caller, &checks).await.connect()?;
        Response::ok(pb::CheckAccessResponse {
            results: results
                .into_iter()
                .map(|a| pb::AccessResult {
                    subject: a.subject,
                    feature: a.feature,
                    has_access: a.has_access,
                    reason: a.reason.map(|r| r.reason().to_string()).unwrap_or_default(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn create_grant(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateGrantRequest>,
    ) -> ServiceResult<pb::CreateGrantResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let g = r.grant.as_option().cloned().unwrap_or_default();
        let new = grant_in(
            &g,
            r.reason.clone(),
            nonempty(&r.idempotency_key).map(str::to_string),
        )
        .connect()?;
        let created = self.0.create_grant(&caller, new).await.connect()?;
        Response::ok(pb::CreateGrantResponse {
            grant: grant_out(&created).into(),
            ..Default::default()
        })
    }

    async fn void_grant(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::VoidGrantRequest>,
    ) -> ServiceResult<pb::VoidGrantResponse> {
        let caller = caller(&ctx)?;
        let g = self
            .0
            .void_grant(&caller, request.id, request.reason)
            .await
            .connect()?;
        Response::ok(pb::VoidGrantResponse {
            grant: grant_out(&g).into(),
            ..Default::default()
        })
    }

    async fn list_grants(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListGrantsRequest>,
    ) -> ServiceResult<pb::ListGrantsResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let (page, next) = self
            .0
            .list_grants(
                &caller,
                &r.entitlement_id,
                r.include_voided,
                nonempty(&r.page_token),
                page_size(r.page_size),
            )
            .await
            .connect()?;
        Response::ok(pb::ListGrantsResponse {
            grants: page.iter().map(grant_out).collect(),
            next_page_token: next.unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn reset_entitlement(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ResetEntitlementRequest>,
    ) -> ServiceResult<pb::ResetEntitlementResponse> {
        let caller = caller(&ctx)?;
        let r = request.to_owned_message();
        let e = self
            .0
            .reset(
                &caller,
                &r.id,
                r.effective_time.as_option().map(utc),
                &r.reason,
            )
            .await
            .connect()?;
        Response::ok(pb::ResetEntitlementResponse {
            entitlement: entitlement_out(&e).into(),
            ..Default::default()
        })
    }

    async fn get_balance_history(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetBalanceHistoryRequest>,
    ) -> ServiceResult<pb::GetBalanceHistoryResponse> {
        let caller = caller(&ctx)?;
        let points = self
            .0
            .balance_history(&caller, request.id)
            .await
            .map_err(to_connect)?;
        Response::ok(pb::GetBalanceHistoryResponse {
            points: points
                .into_iter()
                .map(|(t, usage, balance)| pb::BalancePoint {
                    time: timestamp(t).into(),
                    usage: usage.to_string(),
                    balance: balance.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }
}
