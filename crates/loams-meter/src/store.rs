//! The meter's records: customers, entitlements and grants (design §63
//! §9.1), behind [`MeterStore`]. The role keeps them in the `ControlStore`
//! (§41); [`MemStore`] keeps them in memory, for tests and `loams dev`.
//!
//! Lists are paged by id (ids are ULIDs, so pages are in creation order):
//! a page token is the last id of the previous page.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use loams_meter_core::{
    Currency, Entitlement, EntitlementId, FeatureKey, Grant, GrantId, Money, PlanId, SubjectKey,
    UtcNanos,
};
use serde::{Deserialize, Serialize};

use crate::error::{MeterError, Result};

/// How a customer pays (§63 §13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentState {
    /// No payment method: hard-stopped at its limits.
    #[default]
    None,
    /// A card on file with the billing provider.
    Card,
    /// A contract, invoiced; set by an operator.
    Invoice,
}

/// Overage and its cap (§63 §13).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendControls {
    pub overage_enabled: bool,
    pub spend_cap: Option<Money>,
    pub auto_top_up: bool,
    pub top_up_amount: Option<Money>,
    pub top_up_threshold: Option<Money>,
}

/// Who pays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    pub id: String,
    pub org: String,
    pub display_name: String,
    pub subjects: Vec<SubjectKey>,
    pub payment_state: PaymentState,
    pub spend_controls: SpendControls,
    pub stripe_customer_id: Option<String>,
    pub currency: Currency,
    /// The plan assigned and its version.
    pub plan: Option<(PlanId, u32)>,
    pub created_at: UtcNanos,
    pub updated_at: UtcNanos,
    /// Bumped by every update; an update names the version it read.
    pub version: u64,
}

/// The meter's record store.
#[async_trait]
pub trait MeterStore: Send + Sync + fmt::Debug {
    /// Creates `c`. With an idempotency key already used, answers the
    /// customer that call created; a subject that belongs to another
    /// customer is `already_exists`.
    async fn create_customer(&self, c: Customer, idempotency_key: Option<&str>)
    -> Result<Customer>;
    async fn customer(&self, id: &str) -> Result<Option<Customer>>;
    async fn customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>>;
    async fn customer_by_subject(&self, subject: &SubjectKey) -> Result<Option<Customer>>;
    /// Customers of `org` (all with `None`), ids after `after`, at most `limit`.
    async fn list_customers(
        &self,
        org: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Customer>>;
    /// Replaces the customer if its stored version is `c.version`; answers it
    /// with the version bumped. A concurrent change is `failed_precondition`
    /// (`version_conflict`): read and retry.
    async fn update_customer(&self, c: Customer) -> Result<Customer>;

    /// Creates `e`; one entitlement per (subject, feature).
    async fn create_entitlement(
        &self,
        e: Entitlement,
        idempotency_key: Option<&str>,
    ) -> Result<Entitlement>;
    async fn entitlement(&self, id: &EntitlementId) -> Result<Option<Entitlement>>;
    async fn entitlement_for(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
    ) -> Result<Option<Entitlement>>;
    async fn list_entitlements(
        &self,
        subject: Option<&SubjectKey>,
        feature: Option<&FeatureKey>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Entitlement>>;
    async fn update_entitlement(&self, e: Entitlement) -> Result<Entitlement>;
    async fn delete_entitlement(&self, id: &EntitlementId) -> Result<()>;

    async fn create_grant(&self, g: Grant, idempotency_key: Option<&str>) -> Result<Grant>;
    async fn grant(&self, id: &GrantId) -> Result<Option<Grant>>;
    async fn list_grants(
        &self,
        entitlement: &EntitlementId,
        include_voided: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Grant>>;
    async fn update_grant(&self, g: Grant) -> Result<Grant>;
}

/// [`MeterStore`] in memory.
#[derive(Debug, Default)]
pub struct MemStore {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    customers: BTreeMap<String, Customer>,
    entitlements: BTreeMap<String, Entitlement>,
    grants: BTreeMap<String, Grant>,
    /// (kind, idempotency key) → the id it created.
    idempotency: HashMap<(&'static str, String), String>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn page<'a, T: 'a>(
    items: impl Iterator<Item = (&'a String, &'a T)>,
    after: Option<&str>,
    limit: usize,
) -> Vec<&'a T> {
    items
        .filter(|(id, _)| after.is_none_or(|a| id.as_str() > a))
        .take(limit)
        .map(|(_, v)| v)
        .collect()
}

#[async_trait]
impl MeterStore for MemStore {
    async fn create_customer(
        &self,
        c: Customer,
        idempotency_key: Option<&str>,
    ) -> Result<Customer> {
        let mut s = self.lock();
        if let Some(k) = idempotency_key
            && let Some(id) = s.idempotency.get(&("customer", k.to_string()))
        {
            return s
                .customers
                .get(id)
                .cloned()
                .ok_or_else(|| MeterError::not_found("customer", id.clone()));
        }
        if s.customers.contains_key(&c.id) {
            return Err(MeterError::exists("customer", c.id));
        }
        for subject in &c.subjects {
            if s.customers.values().any(|o| o.subjects.contains(subject)) {
                return Err(MeterError::exists("subject", subject.as_str()));
            }
        }
        if let Some(stripe) = &c.stripe_customer_id
            && s.customers
                .values()
                .any(|o| o.stripe_customer_id.as_ref() == Some(stripe))
        {
            return Err(MeterError::exists("stripe customer", stripe.clone()));
        }
        if let Some(k) = idempotency_key {
            s.idempotency
                .insert(("customer", k.to_string()), c.id.clone());
        }
        s.customers.insert(c.id.clone(), c.clone());
        Ok(c)
    }

    async fn customer(&self, id: &str) -> Result<Option<Customer>> {
        Ok(self.lock().customers.get(id).cloned())
    }

    async fn customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        Ok(self
            .lock()
            .customers
            .values()
            .find(|c| c.stripe_customer_id.as_deref() == Some(stripe_customer_id))
            .cloned())
    }

    async fn customer_by_subject(&self, subject: &SubjectKey) -> Result<Option<Customer>> {
        Ok(self
            .lock()
            .customers
            .values()
            .find(|c| c.subjects.contains(subject))
            .cloned())
    }

    async fn list_customers(
        &self,
        org: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Customer>> {
        let s = self.lock();
        Ok(page(
            s.customers
                .iter()
                .filter(|(_, c)| org.is_none_or(|o| c.org == o)),
            after,
            limit,
        )
        .into_iter()
        .cloned()
        .collect())
    }

    async fn update_customer(&self, mut c: Customer) -> Result<Customer> {
        let mut s = self.lock();
        let Some(stored) = s.customers.get(&c.id) else {
            return Err(MeterError::not_found("customer", c.id));
        };
        if stored.version != c.version {
            return Err(MeterError::precondition(
                "version_conflict",
                format!("customer {} changed concurrently; read it again", c.id),
            ));
        }
        for subject in &c.subjects {
            if s.customers
                .values()
                .any(|o| o.id != c.id && o.subjects.contains(subject))
            {
                return Err(MeterError::exists("subject", subject.as_str()));
            }
        }
        c.version += 1;
        s.customers.insert(c.id.clone(), c.clone());
        Ok(c)
    }

    async fn create_entitlement(
        &self,
        e: Entitlement,
        idempotency_key: Option<&str>,
    ) -> Result<Entitlement> {
        let mut s = self.lock();
        if let Some(k) = idempotency_key
            && let Some(id) = s.idempotency.get(&("entitlement", k.to_string()))
        {
            return s
                .entitlements
                .get(id)
                .cloned()
                .ok_or_else(|| MeterError::not_found("entitlement", id.clone()));
        }
        if s.entitlements
            .values()
            .any(|o| o.subject == e.subject && o.feature == e.feature)
        {
            return Err(MeterError::exists(
                "entitlement",
                format!("{}/{}", e.subject, e.feature),
            ));
        }
        if let Some(k) = idempotency_key {
            s.idempotency
                .insert(("entitlement", k.to_string()), e.id.to_string());
        }
        s.entitlements.insert(e.id.to_string(), e.clone());
        Ok(e)
    }

    async fn entitlement(&self, id: &EntitlementId) -> Result<Option<Entitlement>> {
        Ok(self.lock().entitlements.get(id.as_str()).cloned())
    }

    async fn entitlement_for(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
    ) -> Result<Option<Entitlement>> {
        Ok(self
            .lock()
            .entitlements
            .values()
            .find(|e| &e.subject == subject && &e.feature == feature)
            .cloned())
    }

    async fn list_entitlements(
        &self,
        subject: Option<&SubjectKey>,
        feature: Option<&FeatureKey>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Entitlement>> {
        let s = self.lock();
        Ok(page(
            s.entitlements.iter().filter(|(_, e)| {
                subject.is_none_or(|x| &e.subject == x) && feature.is_none_or(|f| &e.feature == f)
            }),
            after,
            limit,
        )
        .into_iter()
        .cloned()
        .collect())
    }

    async fn update_entitlement(&self, e: Entitlement) -> Result<Entitlement> {
        let mut s = self.lock();
        if !s.entitlements.contains_key(e.id.as_str()) {
            return Err(MeterError::not_found("entitlement", e.id.as_str()));
        }
        s.entitlements.insert(e.id.to_string(), e.clone());
        Ok(e)
    }

    async fn delete_entitlement(&self, id: &EntitlementId) -> Result<()> {
        let mut s = self.lock();
        s.entitlements.remove(id.as_str());
        s.grants.retain(|_, g| &g.entitlement != id);
        Ok(())
    }

    async fn create_grant(&self, g: Grant, idempotency_key: Option<&str>) -> Result<Grant> {
        let mut s = self.lock();
        if let Some(k) = idempotency_key
            && let Some(id) = s.idempotency.get(&("grant", k.to_string()))
        {
            return s
                .grants
                .get(id)
                .cloned()
                .ok_or_else(|| MeterError::not_found("grant", id.clone()));
        }
        if !s.entitlements.contains_key(g.entitlement.as_str()) {
            return Err(MeterError::not_found("entitlement", g.entitlement.as_str()));
        }
        if s.grants.contains_key(g.id.as_str()) {
            return Err(MeterError::exists("grant", g.id.as_str()));
        }
        if let Some(k) = idempotency_key {
            s.idempotency
                .insert(("grant", k.to_string()), g.id.to_string());
        }
        s.grants.insert(g.id.to_string(), g.clone());
        Ok(g)
    }

    async fn grant(&self, id: &GrantId) -> Result<Option<Grant>> {
        Ok(self.lock().grants.get(id.as_str()).cloned())
    }

    async fn list_grants(
        &self,
        entitlement: &EntitlementId,
        include_voided: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Grant>> {
        let s = self.lock();
        Ok(page(
            s.grants.iter().filter(|(_, g)| {
                &g.entitlement == entitlement && (include_voided || g.voided_at.is_none())
            }),
            after,
            limit,
        )
        .into_iter()
        .cloned()
        .collect())
    }

    async fn update_grant(&self, g: Grant) -> Result<Grant> {
        let mut s = self.lock();
        if !s.grants.contains_key(g.id.as_str()) {
            return Err(MeterError::not_found("grant", g.id.as_str()));
        }
        s.grants.insert(g.id.to_string(), g.clone());
        Ok(g)
    }
}
