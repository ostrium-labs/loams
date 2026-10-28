//! `EntitlementService` (design §63 §9, §17.1; ME1 Task 10): entitlements,
//! grants and balances.
//!
//! Writes need `meter_admin` (people only; an agent can never write a grant,
//! test `agent_cannot_write_grants`); reads need `billing_viewer` on the org
//! of the subject's customer. Every write is an audit event. Balances come
//! from a [`ValueSource`]: the role's counters and the balance engine.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use loams_meter_core::{
    Amount, AsOf, Balance, Entitlement, EntitlementId, EntitlementKind, EntitlementSource,
    FeatureKey, Grant, GrantId, GrantKind, Mode, Period, RefusalReason, SubjectKey, UtcNanos,
};

use crate::audit::{AuditEvent, AuditSink};
use crate::authz::{Caller, MeterAuthz};
use crate::customers::{Clock, new_id};
use crate::error::{MeterError, Result};
use crate::store::MeterStore;

/// The most items one page answers.
pub const MAX_PAGE: usize = 500;

/// The most (subject, feature) pairs one `CheckAccess` call may ask about.
pub const MAX_CHECKS: usize = 1000;

/// The current balance of a metered entitlement.
#[async_trait]
pub trait ValueSource: Send + Sync + fmt::Debug {
    /// `e`'s balance at `at`, from its live grants and the usage counted so
    /// far in the period (§63 §9.3).
    async fn value(&self, e: &Entitlement, grants: &[Grant], at: UtcNanos) -> Result<Balance>;
}

/// What `GetEntitlementValue` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntitlementValue {
    pub has_access: bool,
    pub balance: Amount,
    pub usage: Amount,
    pub overage: Amount,
    pub total_available: Amount,
    pub period: Option<Period>,
    pub as_of: Option<AsOf>,
}

impl EntitlementValue {
    fn flag(has_access: bool) -> Self {
        EntitlementValue {
            has_access,
            balance: Amount::ZERO,
            usage: Amount::ZERO,
            overage: Amount::ZERO,
            total_available: Amount::ZERO,
            period: None,
            as_of: None,
        }
    }
}

/// One `CheckAccess` answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub subject: String,
    pub feature: String,
    pub has_access: bool,
    pub reason: Option<RefusalReason>,
}

/// A grant to create (ids and times are the service's).
#[derive(Clone, Debug)]
pub struct NewGrant {
    pub entitlement: EntitlementId,
    pub amount: Amount,
    pub priority: u8,
    pub effective_at: Option<UtcNanos>,
    pub expires: Option<loams_meter_core::Expiry>,
    pub rollover: loams_meter_core::Rollover,
    pub recurrence: Option<loams_meter_core::UsagePeriod>,
    pub kind: GrantKind,
    pub reason: String,
    pub idempotency_key: Option<String>,
}

/// The service.
#[derive(Clone)]
pub struct Entitlements {
    store: Arc<dyn MeterStore>,
    authz: MeterAuthz,
    audit: Arc<dyn AuditSink>,
    values: Arc<dyn ValueSource>,
    clock: Clock,
}

impl fmt::Debug for Entitlements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entitlements").finish_non_exhaustive()
    }
}

fn subject(raw: &str) -> Result<SubjectKey> {
    SubjectKey::new(raw).map_err(|e| MeterError::invalid("subject", e.to_string()))
}

fn feature(raw: &str) -> Result<FeatureKey> {
    FeatureKey::new(raw).map_err(|e| MeterError::invalid("feature", e.to_string()))
}

/// §63 §9.2's rules on an entitlement's kind.
pub fn validate_kind(kind: &EntitlementKind) -> Result<()> {
    if let EntitlementKind::Metered {
        mode,
        issue_after_reset,
        ..
    } = kind
    {
        if issue_after_reset.is_some_and(Amount::is_negative) {
            return Err(MeterError::invalid(
                "issue_after_reset",
                "must not be negative",
            ));
        }
        if let Mode::OverageAllowed {
            overage_price,
            spend_cap,
        } = mode
        {
            if overage_price.micros < 0 {
                return Err(MeterError::invalid("overage_price", "must not be negative"));
            }
            if spend_cap.micros <= 0 {
                return Err(MeterError::invalid(
                    "spend_cap",
                    "overage needs a positive spend cap",
                ));
            }
            if overage_price.currency != spend_cap.currency {
                return Err(MeterError::invalid(
                    "spend_cap",
                    "must be in the overage price's currency",
                ));
            }
        }
    }
    Ok(())
}

impl Entitlements {
    pub fn new(
        store: Arc<dyn MeterStore>,
        authz: MeterAuthz,
        audit: Arc<dyn AuditSink>,
        values: Arc<dyn ValueSource>,
        clock: Clock,
    ) -> Self {
        Entitlements {
            store,
            authz,
            audit,
            values,
            clock,
        }
    }

    /// The org that pays for `subject`.
    async fn org_of(&self, subject: &SubjectKey) -> Result<String> {
        self.store
            .customer_by_subject(subject)
            .await?
            .map(|c| c.org)
            .ok_or_else(|| {
                MeterError::precondition(
                    "customer_required",
                    format!("subject {subject} belongs to no customer"),
                )
            })
    }

    async fn require_view(&self, caller: &Caller, subject: &SubjectKey) -> Result<()> {
        if self.authz.is_meter_admin(caller).await? {
            return Ok(());
        }
        let org = self.org_of(subject).await?;
        self.authz.require_billing_view(caller, &org).await
    }

    async fn audit(
        &self,
        caller: &Caller,
        action: &str,
        object: String,
        subject: &SubjectKey,
        reason: &str,
        details: serde_json::Value,
    ) -> Result<()> {
        let org = self
            .store
            .customer_by_subject(subject)
            .await?
            .map(|c| c.org);
        self.audit
            .record(AuditEvent {
                action: action.into(),
                actor: caller.actor(),
                object,
                org,
                reason: reason.into(),
                details,
                time: (self.clock)(),
            })
            .await
    }

    async fn load(&self, id: &EntitlementId) -> Result<Entitlement> {
        self.store
            .entitlement(id)
            .await?
            .ok_or_else(|| MeterError::not_found("entitlement", id.as_str()))
    }

    /// `CreateEntitlement` (`meter_admin`): one per (subject, feature), for a
    /// subject that belongs to a customer.
    pub async fn create(
        &self,
        caller: &Caller,
        subject_key: &str,
        feature_key: &str,
        kind: EntitlementKind,
        reason: &str,
        idempotency_key: Option<&str>,
    ) -> Result<Entitlement> {
        self.authz.require_meter_admin(caller).await?;
        let subject = subject(subject_key)?;
        let feature = feature(feature_key)?;
        validate_kind(&kind)?;
        self.org_of(&subject).await?;
        let id = EntitlementId::new(new_id())
            .map_err(|e| MeterError::Unavailable(format!("entitlement id: {e}")))?;
        let e = Entitlement {
            id,
            subject: subject.clone(),
            feature,
            kind,
            created_at: (self.clock)(),
            source: EntitlementSource::Manual { by: caller.actor() },
        };
        let created = self
            .store
            .create_entitlement(e.clone(), idempotency_key)
            .await?;
        if created.id == e.id {
            self.audit(
                caller,
                "meter.entitlement.create",
                format!("entitlement:{}", created.id),
                &subject,
                reason,
                serde_json::json!({ "feature": created.feature, "kind": created.kind }),
            )
            .await?;
        }
        Ok(created)
    }

    /// `ListEntitlements`: one subject's (its billing viewers) or all
    /// (meter admins).
    pub async fn list(
        &self,
        caller: &Caller,
        subject_key: Option<&str>,
        feature_key: Option<&str>,
        page_token: Option<&str>,
        page_size: usize,
    ) -> Result<(Vec<Entitlement>, Option<String>)> {
        let subject = subject_key.map(subject).transpose()?;
        let feature = feature_key.map(feature).transpose()?;
        match &subject {
            Some(s) => self.require_view(caller, s).await?,
            None => self.authz.require_meter_admin(caller).await?,
        }
        let limit = page_size.clamp(1, MAX_PAGE);
        let page = self
            .store
            .list_entitlements(subject.as_ref(), feature.as_ref(), page_token, limit)
            .await?;
        let next = (page.len() == limit)
            .then(|| page.last().map(|e| e.id.to_string()))
            .flatten();
        Ok((page, next))
    }

    async fn value_of(&self, e: &Entitlement) -> Result<EntitlementValue> {
        match &e.kind {
            EntitlementKind::Boolean { enabled } => Ok(EntitlementValue::flag(*enabled)),
            EntitlementKind::Static { .. } => Ok(EntitlementValue::flag(true)),
            EntitlementKind::Metered { .. } => {
                let grants = self
                    .store
                    .list_grants(&e.id, true, None, usize::MAX)
                    .await?;
                let b = self.values.value(e, &grants, (self.clock)()).await?;
                Ok(EntitlementValue {
                    has_access: b.has_access,
                    balance: b.balance,
                    usage: b.usage,
                    overage: b.overage,
                    total_available: b.total_available,
                    period: Some(b.period),
                    as_of: Some(b.as_of),
                })
            }
        }
    }

    /// `GetEntitlementValue`.
    pub async fn value(
        &self,
        caller: &Caller,
        subject_key: &str,
        feature_key: &str,
    ) -> Result<EntitlementValue> {
        let subject = subject(subject_key)?;
        let feature = feature(feature_key)?;
        self.require_view(caller, &subject).await?;
        let e = self
            .store
            .entitlement_for(&subject, &feature)
            .await?
            .ok_or_else(|| MeterError::Refused {
                reason: RefusalReason::NotEntitled,
                message: format!("{subject} has no entitlement to {feature}"),
            })?;
        self.value_of(&e).await
    }

    /// The entitlement an OpenMeter path names (`entitlementIdOrFeatureKey`):
    /// `subject`'s entitlement to that feature, or the entitlement with that
    /// id if it is `subject`'s.
    pub async fn resolve(
        &self,
        caller: &Caller,
        subject_key: &str,
        id_or_feature: &str,
    ) -> Result<Entitlement> {
        let subject = subject(subject_key)?;
        self.require_view(caller, &subject).await?;
        if let Ok(f) = FeatureKey::new(id_or_feature)
            && let Some(e) = self.store.entitlement_for(&subject, &f).await?
        {
            return Ok(e);
        }
        let id = EntitlementId::new(id_or_feature)
            .map_err(|e| MeterError::invalid("entitlementIdOrFeatureKey", e.to_string()))?;
        match self.store.entitlement(&id).await? {
            Some(e) if e.subject == subject => Ok(e),
            _ => Err(MeterError::not_found("entitlement", id_or_feature)),
        }
    }

    /// `CheckAccess` (batch). Each subject is authorized once.
    pub async fn check_access(
        &self,
        caller: &Caller,
        checks: &[(String, String)],
    ) -> Result<Vec<Access>> {
        if checks.len() > MAX_CHECKS {
            return Err(MeterError::invalid(
                "checks",
                format!("at most {MAX_CHECKS} per call"),
            ));
        }
        let mut allowed: HashMap<SubjectKey, ()> = HashMap::new();
        let mut out = Vec::with_capacity(checks.len());
        for (s, f) in checks {
            let subject = subject(s)?;
            let feature = feature(f)?;
            if !allowed.contains_key(&subject) {
                self.require_view(caller, &subject).await?;
                allowed.insert(subject.clone(), ());
            }
            let access = match self.store.entitlement_for(&subject, &feature).await? {
                None => Access {
                    subject: s.clone(),
                    feature: f.clone(),
                    has_access: false,
                    reason: Some(RefusalReason::NotEntitled),
                },
                Some(e) => {
                    let v = self.value_of(&e).await?;
                    Access {
                        subject: s.clone(),
                        feature: f.clone(),
                        has_access: v.has_access,
                        reason: (!v.has_access).then_some(match &e.kind {
                            EntitlementKind::Metered { .. } => RefusalReason::EntitlementExhausted,
                            _ => RefusalReason::NotEntitled,
                        }),
                    }
                }
            };
            out.push(access);
        }
        Ok(out)
    }

    /// `CreateGrant` (`meter_admin`; agents never). A manual grant needs a
    /// reason.
    pub async fn create_grant(&self, caller: &Caller, new: NewGrant) -> Result<Grant> {
        self.authz.require_meter_admin(caller).await?;
        if new.reason.trim().is_empty() {
            return Err(MeterError::invalid("reason", "a grant needs a reason"));
        }
        let e = self.load(&new.entitlement).await?;
        if !matches!(e.kind, EntitlementKind::Metered { .. }) {
            return Err(MeterError::precondition(
                "not_metered",
                "grants apply to metered entitlements only",
            ));
        }
        if let GrantKind::Prepaid { paid } = new.kind
            && paid.micros <= 0
        {
            return Err(MeterError::invalid(
                "paid",
                "a prepaid grant records what was paid",
            ));
        }
        let now = (self.clock)();
        let id = GrantId::new(new_id())
            .map_err(|e| MeterError::Unavailable(format!("grant id: {e}")))?;
        let g = Grant {
            id,
            entitlement: e.id.clone(),
            amount: new.amount,
            priority: new.priority,
            effective_at: new.effective_at.unwrap_or(now),
            expires: new.expires,
            rollover: new.rollover,
            recurrence: new.recurrence,
            kind: new.kind,
            voided_at: None,
            created_at: now,
        }
        .normalized();
        g.validate()
            .map_err(|err| MeterError::invalid("grant", err.to_string()))?;
        let created = self
            .store
            .create_grant(g.clone(), new.idempotency_key.as_deref())
            .await?;
        if created.id == g.id {
            self.audit(
                caller,
                "meter.grant.create",
                format!("grant:{}", created.id),
                &e.subject,
                &new.reason,
                serde_json::json!({
                    "entitlement": e.id,
                    "amount": created.amount,
                    "kind": created.kind,
                    "priority": created.priority,
                }),
            )
            .await?;
        }
        Ok(created)
    }

    /// `VoidGrant` (`meter_admin`): what is left is forfeited from now.
    pub async fn void_grant(&self, caller: &Caller, id: &str, reason: &str) -> Result<Grant> {
        self.authz.require_meter_admin(caller).await?;
        let gid = GrantId::new(id).map_err(|e| MeterError::invalid("id", e.to_string()))?;
        let mut g = self
            .store
            .grant(&gid)
            .await?
            .ok_or_else(|| MeterError::not_found("grant", id))?;
        if g.voided_at.is_some() {
            return Ok(g);
        }
        g.voided_at = Some((self.clock)());
        let voided = self.store.update_grant(g).await?;
        let e = self.load(&voided.entitlement).await?;
        self.audit(
            caller,
            "meter.grant.void",
            format!("grant:{}", voided.id),
            &e.subject,
            reason,
            serde_json::json!({ "entitlement": e.id }),
        )
        .await?;
        Ok(voided)
    }

    /// `ListGrants`.
    pub async fn list_grants(
        &self,
        caller: &Caller,
        entitlement: &str,
        include_voided: bool,
        page_token: Option<&str>,
        page_size: usize,
    ) -> Result<(Vec<Grant>, Option<String>)> {
        let eid = EntitlementId::new(entitlement)
            .map_err(|e| MeterError::invalid("entitlement_id", e.to_string()))?;
        let e = self.load(&eid).await?;
        self.require_view(caller, &e.subject).await?;
        let limit = page_size.clamp(1, MAX_PAGE);
        let page = self
            .store
            .list_grants(&eid, include_voided, page_token, limit)
            .await?;
        let next = (page.len() == limit)
            .then(|| page.last().map(|g| g.id.to_string()))
            .flatten();
        Ok((page, next))
    }

    /// `ResetEntitlement` (`meter_admin`): a new usage period starts at
    /// `effective` (now by default), anchored there.
    pub async fn reset(
        &self,
        caller: &Caller,
        id: &str,
        effective: Option<UtcNanos>,
        reason: &str,
    ) -> Result<Entitlement> {
        self.authz.require_meter_admin(caller).await?;
        let eid = EntitlementId::new(id).map_err(|e| MeterError::invalid("id", e.to_string()))?;
        let mut e = self.load(&eid).await?;
        let at = effective.unwrap_or_else(|| (self.clock)());
        match &mut e.kind {
            EntitlementKind::Metered { usage_period, .. } => usage_period.anchor = at,
            _ => {
                return Err(MeterError::precondition(
                    "not_metered",
                    "only a metered entitlement has a usage period",
                ));
            }
        }
        let updated = self.store.update_entitlement(e).await?;
        self.audit(
            caller,
            "meter.entitlement.reset",
            format!("entitlement:{}", updated.id),
            &updated.subject,
            reason,
            serde_json::json!({ "at": at }),
        )
        .await?;
        Ok(updated)
    }

    /// `GetBalanceHistory` reads the House's closed windows, which the role
    /// wires with ME-house's query (ME1 Task 8). Until then it is refused as
    /// not implemented, rather than answering an empty history.
    pub async fn balance_history(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<Vec<(UtcNanos, Amount, Amount)>> {
        let eid = EntitlementId::new(id).map_err(|e| MeterError::invalid("id", e.to_string()))?;
        let e = self.load(&eid).await?;
        self.require_view(caller, &e.subject).await?;
        Err(MeterError::Unimplemented("GetBalanceHistory"))
    }
}
