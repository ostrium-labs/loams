//! `PlanService` (design §63 §9.5, §17.1, §22; ME1 Tasks 10 and 11): plans
//! applied from YAML by the GitOps reconciler, and assigned to customers.
//!
//! Rules of an apply:
//! - a plan version is immutable: the same content again is a no-op, other
//!   content under an existing version is refused (`plan_version_immutable`);
//! - a meter's semantics are immutable once it has recorded events, as
//!   Stripe's meters are (`meter_semantics_immutable`; a new slug is
//!   needed). An unused meter may still be corrected.
//!
//! Assigning a plan creates, for each of the customer's subjects, the
//! entitlement of each template (anchored at the customer's billing anchor,
//! a whole UTC hour, §63 §16), updates the entitlements an earlier version
//! created, and removes those the new version no longer has. Entitlements
//! created by hand are never touched.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use loams_meter_core::{
    Entitlement, EntitlementId, EntitlementKind, EntitlementSource, PlanId, Slug, UtcNanos,
};

use crate::audit::{AuditEvent, AuditSink};
use crate::authz::{Caller, MeterAuthz};
use crate::customers::{Clock, new_id};
use crate::error::{MeterError, Result};
use crate::plan_yaml::{self, semantics_differ};
use crate::store::{Customer, MeterStore, PlanRecord};

/// Whether a meter has recorded events (the House's `_meter.events` has
/// rows for its event type since `event_from`). The role answers it from
/// the House; without a House (enforcement-only mode) every known meter
/// counts as used, the safe answer.
#[async_trait]
pub trait MeterUsage: Send + Sync + fmt::Debug {
    async fn has_events(&self, slug: &Slug) -> Result<bool>;
}

/// What `ApplyPlan` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub plan: PlanRecord,
    /// False when this version was already applied with the same content.
    pub changed: bool,
}

/// The service.
#[derive(Clone)]
pub struct Plans {
    store: Arc<dyn MeterStore>,
    authz: MeterAuthz,
    audit: Arc<dyn AuditSink>,
    usage: Arc<dyn MeterUsage>,
    clock: Clock,
}

impl fmt::Debug for Plans {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plans").finish_non_exhaustive()
    }
}

/// The customer's billing anchor: its creation time, floored to the hour,
/// so no billed hour straddles a period start (§63 §16).
pub fn billing_anchor(c: &Customer) -> UtcNanos {
    UtcNanos(c.created_at.0 - c.created_at.0.rem_euclid(UtcNanos::HOUR))
}

fn plan_id(raw: &str) -> Result<PlanId> {
    PlanId::new(raw).map_err(|e| MeterError::invalid("plan", e.to_string()))
}

impl Plans {
    pub fn new(
        store: Arc<dyn MeterStore>,
        authz: MeterAuthz,
        audit: Arc<dyn AuditSink>,
        usage: Arc<dyn MeterUsage>,
        clock: Clock,
    ) -> Self {
        Plans {
            store,
            authz,
            audit,
            usage,
            clock,
        }
    }

    async fn record(
        &self,
        caller: &Caller,
        action: &str,
        object: String,
        org: Option<String>,
        reason: &str,
        details: serde_json::Value,
    ) -> Result<()> {
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

    /// `ApplyPlan` (`meter_admin`; the GitOps reconciler's identity is a
    /// person-backed service account with that relation, or an operator).
    pub async fn apply(&self, caller: &Caller, yaml: &str, reason: &str) -> Result<Applied> {
        self.authz.require_meter_admin(caller).await?;
        let plan = plan_yaml::parse(yaml)?;
        if let Some(existing) = self.store.plan(&plan.name, plan.version).await? {
            if existing.digest == plan.digest {
                return Ok(Applied {
                    plan: existing,
                    changed: false,
                });
            }
            return Err(MeterError::precondition(
                "plan_version_immutable",
                format!(
                    "plan {} version {} was applied with other content; give the change a new version",
                    plan.name, plan.version
                ),
            ));
        }
        // Every meter first, so a refusal changes nothing.
        let mut changed_meters = Vec::new();
        for m in &plan.meters {
            if let Some(old) = self.store.meter(&m.slug).await? {
                if !semantics_differ(&old, m) {
                    continue;
                }
                if self.usage.has_events(&m.slug).await? {
                    return Err(MeterError::precondition(
                        "meter_semantics_immutable",
                        format!(
                            "meter {} has recorded events, so what it counts cannot change; define a new meter slug",
                            m.slug
                        ),
                    ));
                }
            }
            changed_meters.push(m.clone());
        }
        for f in &plan.features {
            if let Some(slug) = &f.meter {
                let defined = plan.meters.iter().any(|m| &m.slug == slug)
                    || self.store.meter(slug).await?.is_some();
                if !defined {
                    return Err(MeterError::invalid(
                        "features",
                        format!("feature {} names meter {slug}, which is not defined", f.key),
                    ));
                }
            }
        }
        for m in changed_meters {
            self.store.put_meter(m).await?;
        }
        for f in &plan.features {
            self.store.put_feature(f.clone()).await?;
        }
        let record = self
            .store
            .create_plan(PlanRecord {
                name: plan.name.clone(),
                version: plan.version,
                yaml: plan.yaml.clone(),
                digest: plan.digest.clone(),
                created_at: (self.clock)(),
            })
            .await?;
        self.record(
            caller,
            "meter.plan.apply",
            format!("plan:{}/{}", record.name, record.version),
            None,
            reason,
            serde_json::json!({
                "digest": record.digest,
                "meters": plan.meters.iter().map(|m| m.slug.to_string()).collect::<Vec<_>>(),
                "features": plan.templates.iter().map(|t| t.feature.to_string()).collect::<Vec<_>>(),
            }),
        )
        .await?;
        Ok(Applied {
            plan: record,
            changed: true,
        })
    }

    /// `ListPlans` (`meter_admin`): the latest version of each plan, or
    /// every version.
    pub async fn list(
        &self,
        caller: &Caller,
        name: Option<&str>,
        all_versions: bool,
    ) -> Result<Vec<PlanRecord>> {
        self.authz.require_meter_admin(caller).await?;
        let name = name.map(plan_id).transpose()?;
        let mut all = self.store.list_plans(name.as_ref()).await?;
        all.sort_by(|a, b| (a.name.as_str(), a.version).cmp(&(b.name.as_str(), b.version)));
        if !all_versions {
            let mut latest: Vec<PlanRecord> = Vec::new();
            for p in all {
                match latest.last_mut() {
                    Some(l) if l.name == p.name => *l = p,
                    _ => latest.push(p),
                }
            }
            return Ok(latest);
        }
        Ok(all)
    }

    async fn version(&self, name: &PlanId, version: u32) -> Result<PlanRecord> {
        if version > 0 {
            return self
                .store
                .plan(name, version)
                .await?
                .ok_or_else(|| MeterError::not_found("plan", format!("{name} v{version}")));
        }
        self.store
            .list_plans(Some(name))
            .await?
            .into_iter()
            .max_by_key(|p| p.version)
            .ok_or_else(|| MeterError::not_found("plan", name.as_str()))
    }

    /// `AssignPlan` (`meter_admin`): answers the customer and how many
    /// entitlements were created, changed or removed.
    pub async fn assign(
        &self,
        caller: &Caller,
        customer_id: &str,
        plan: &str,
        version: u32,
        reason: &str,
    ) -> Result<(Customer, u32)> {
        self.authz.require_meter_admin(caller).await?;
        let mut customer = self
            .store
            .customer(customer_id)
            .await?
            .ok_or_else(|| MeterError::not_found("customer", customer_id))?;
        let name = plan_id(plan)?;
        let record = self.version(&name, version).await?;
        let parsed = plan_yaml::parse(&record.yaml)?;
        let anchor = billing_anchor(&customer);
        let now = (self.clock)();
        let mut changed = 0u32;
        let features: HashSet<_> = parsed.templates.iter().map(|t| t.feature.clone()).collect();
        for subject in &customer.subjects {
            for t in &parsed.templates {
                let mut kind = t.kind.clone();
                if let EntitlementKind::Metered { usage_period, .. } = &mut kind {
                    usage_period.anchor = anchor;
                }
                let source = EntitlementSource::Plan {
                    plan: name.clone(),
                    version: record.version,
                };
                match self.store.entitlement_for(subject, &t.feature).await? {
                    Some(e) if matches!(e.source, EntitlementSource::Manual { .. }) => {
                        // Set by hand: a plan never overrides it.
                    }
                    Some(mut e) => {
                        // Keep a reset's anchor when the interval is the same.
                        if let (
                            EntitlementKind::Metered {
                                usage_period: old, ..
                            },
                            EntitlementKind::Metered {
                                usage_period: new, ..
                            },
                        ) = (&e.kind, &mut kind)
                            && old.interval == new.interval
                        {
                            new.anchor = old.anchor;
                        }
                        if e.kind != kind || e.source != source {
                            e.kind = kind;
                            e.source = source;
                            self.store.update_entitlement(e).await?;
                            changed += 1;
                        }
                    }
                    None => {
                        let id = EntitlementId::new(new_id())
                            .map_err(|e| MeterError::Unavailable(format!("entitlement id: {e}")))?;
                        self.store
                            .create_entitlement(
                                Entitlement {
                                    id,
                                    subject: subject.clone(),
                                    feature: t.feature.clone(),
                                    kind,
                                    created_at: now,
                                    source,
                                },
                                None,
                            )
                            .await?;
                        changed += 1;
                    }
                }
            }
            // What an earlier version of a plan created and this one lacks.
            for e in self
                .store
                .list_entitlements(Some(subject), None, None, usize::MAX)
                .await?
            {
                if matches!(e.source, EntitlementSource::Plan { .. })
                    && !features.contains(&e.feature)
                {
                    self.store.delete_entitlement(&e.id).await?;
                    changed += 1;
                }
            }
        }
        customer.plan = Some((name.clone(), record.version));
        customer.updated_at = now;
        let customer = self.store.update_customer(customer).await?;
        self.record(
            caller,
            "meter.plan.assign",
            format!("customer:{}", customer.id),
            Some(customer.org.clone()),
            reason,
            serde_json::json!({ "plan": name, "version": record.version, "changed": changed }),
        )
        .await?;
        Ok((customer, changed))
    }

    /// `UnassignPlan` (`meter_admin`): removes the plan and the entitlements
    /// it created.
    pub async fn unassign(
        &self,
        caller: &Caller,
        customer_id: &str,
        reason: &str,
    ) -> Result<Customer> {
        self.authz.require_meter_admin(caller).await?;
        let mut customer = self
            .store
            .customer(customer_id)
            .await?
            .ok_or_else(|| MeterError::not_found("customer", customer_id))?;
        let Some((plan, version)) = customer.plan.clone() else {
            return Ok(customer);
        };
        for subject in &customer.subjects {
            for e in self
                .store
                .list_entitlements(Some(subject), None, None, usize::MAX)
                .await?
            {
                if matches!(&e.source, EntitlementSource::Plan { plan: p, .. } if *p == plan) {
                    self.store.delete_entitlement(&e.id).await?;
                }
            }
        }
        customer.plan = None;
        customer.updated_at = (self.clock)();
        let customer = self.store.update_customer(customer).await?;
        self.record(
            caller,
            "meter.plan.unassign",
            format!("customer:{}", customer.id),
            Some(customer.org.clone()),
            reason,
            serde_json::json!({ "plan": plan, "version": version }),
        )
        .await?;
        Ok(customer)
    }
}
