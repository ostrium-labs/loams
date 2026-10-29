//! The meter's records: customers, entitlements and grants (design §63
//! §9.1), behind [`MeterStore`]. The role keeps them in the `ControlStore`
//! (§41) through [`DocStore`] on a [`VersionedDoc`]; [`MemStore`] keeps them in
//! memory, for tests.
//!
//! Lists are paged by id (ids are ULIDs, so pages are in creation order):
//! a page token is the last id of the previous page.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use loams_meter_core::{
    Currency, Entitlement, EntitlementId, Feature, FeatureKey, Grant, GrantId, Meter, Money,
    PlanId, Slug, SubjectKey, UtcNanos,
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

/// An applied plan version (ME1 Task 11): the YAML it came from and its
/// digest; its templates are parsed from the YAML when needed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    pub name: PlanId,
    pub version: u32,
    pub yaml: String,
    pub digest: String,
    pub created_at: UtcNanos,
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

    /// Stores a plan version; one that exists is `already_exists`.
    async fn create_plan(&self, p: PlanRecord) -> Result<PlanRecord>;
    async fn plan(&self, name: &PlanId, version: u32) -> Result<Option<PlanRecord>>;
    /// Every version of every plan (`name` set: of that plan), by name then
    /// version.
    async fn list_plans(&self, name: Option<&PlanId>) -> Result<Vec<PlanRecord>>;
    async fn meter(&self, slug: &Slug) -> Result<Option<Meter>>;
    async fn put_meter(&self, m: Meter) -> Result<()>;
    async fn list_meters(&self) -> Result<Vec<Meter>>;
    async fn feature(&self, key: &FeatureKey) -> Result<Option<Feature>>;
    async fn put_feature(&self, f: Feature) -> Result<()>;
    async fn list_features(&self) -> Result<Vec<Feature>>;

    /// Every customer, entitlement, live grant, meter and feature in one
    /// read: what the counter owners and the lease service cache (one load
    /// of the document rather than one per entitlement).
    async fn records(&self) -> Result<Records>;
}

/// The records the meter's runtime caches ([`MeterStore::records`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Records {
    pub customers: Vec<Customer>,
    pub entitlements: Vec<Entitlement>,
    /// Grants that are not voided.
    pub grants: Vec<Grant>,
    pub meters: Vec<Meter>,
    pub features: Vec<Feature>,
}

/// [`MeterStore`] in memory.
#[derive(Debug, Default)]
pub struct MemStore {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default, Clone)]
struct Inner {
    customers: BTreeMap<String, Customer>,
    entitlements: BTreeMap<String, Entitlement>,
    grants: BTreeMap<String, Grant>,
    plans: BTreeMap<(String, u32), PlanRecord>,
    meters: BTreeMap<String, Meter>,
    features: BTreeMap<String, Feature>,
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

/// A versioned document: how the server keeps a control-plane record set
/// (a pointer in the metastore and the bytes in the bucket, as the
/// principal directory is kept). `save` stores only if the version is still
/// `expected`.
#[async_trait]
pub trait VersionedDoc: Send + Sync + fmt::Debug {
    async fn load(&self) -> Result<Option<(u64, Vec<u8>)>>;
    /// `Some(new version)`, or `None` when another writer moved it first.
    async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> Result<Option<u64>>;
}

/// How often a write is redone after losing a race.
const MAX_WRITE_ATTEMPTS: usize = 8;

/// The records as stored (JSON): lists, so no map key needs to be a string.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Snapshot {
    #[serde(default)]
    customers: Vec<Customer>,
    #[serde(default)]
    entitlements: Vec<Entitlement>,
    #[serde(default)]
    grants: Vec<Grant>,
    #[serde(default)]
    plans: Vec<PlanRecord>,
    #[serde(default)]
    meters: Vec<Meter>,
    #[serde(default)]
    features: Vec<Feature>,
    /// (kind, key, id).
    #[serde(default)]
    idempotency: Vec<(String, String, String)>,
}

fn kind_of(k: &str) -> Option<&'static str> {
    match k {
        "customer" => Some("customer"),
        "entitlement" => Some("entitlement"),
        "grant" => Some("grant"),
        _ => None,
    }
}

impl Inner {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            customers: self.customers.values().cloned().collect(),
            entitlements: self.entitlements.values().cloned().collect(),
            grants: self.grants.values().cloned().collect(),
            plans: self.plans.values().cloned().collect(),
            meters: self.meters.values().cloned().collect(),
            features: self.features.values().cloned().collect(),
            idempotency: self
                .idempotency
                .iter()
                .map(|((k, key), id)| ((*k).to_string(), key.clone(), id.clone()))
                .collect(),
        }
    }

    fn from_snapshot(s: Snapshot) -> Inner {
        Inner {
            customers: s.customers.into_iter().map(|c| (c.id.clone(), c)).collect(),
            entitlements: s
                .entitlements
                .into_iter()
                .map(|e| (e.id.to_string(), e))
                .collect(),
            grants: s
                .grants
                .into_iter()
                .map(|g| (g.id.to_string(), g))
                .collect(),
            plans: s
                .plans
                .into_iter()
                .map(|p| ((p.name.to_string(), p.version), p))
                .collect(),
            meters: s
                .meters
                .into_iter()
                .map(|m| (m.slug.to_string(), m))
                .collect(),
            features: s
                .features
                .into_iter()
                .map(|f| (f.key.to_string(), f))
                .collect(),
            idempotency: s
                .idempotency
                .into_iter()
                .filter_map(|(k, key, id)| kind_of(&k).map(|k| ((k, key), id)))
                .collect(),
        }
    }
}

/// [`MeterStore`] on a [`VersionedDoc`]: every write reads the document,
/// applies the change and stores it if nobody moved it meanwhile (redone
/// otherwise), so several `meter` instances share one record set. Meant for
/// control-plane volumes (thousands of customers and grants), which is what
/// the meter's records are; usage never goes here.
pub struct DocStore {
    doc: std::sync::Arc<dyn VersionedDoc>,
    writes: tokio::sync::Mutex<()>,
}

impl fmt::Debug for DocStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocStore").finish_non_exhaustive()
    }
}

impl DocStore {
    pub fn new(doc: std::sync::Arc<dyn VersionedDoc>) -> Self {
        DocStore {
            doc,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    async fn current(&self) -> Result<(Option<u64>, Inner)> {
        match self.doc.load().await? {
            None => Ok((None, Inner::default())),
            Some((version, bytes)) => {
                let snap: Snapshot = serde_json::from_slice(&bytes).map_err(|e| {
                    MeterError::Unavailable(format!("the meter records do not parse: {e}"))
                })?;
                Ok((Some(version), Inner::from_snapshot(snap)))
            }
        }
    }

    async fn read<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R>) -> Result<R> {
        let (_, mut inner) = self.current().await?;
        f(&mut inner)
    }

    async fn write<R>(&self, mut f: impl FnMut(&mut Inner) -> Result<R>) -> Result<R> {
        let _one_writer_per_process = self.writes.lock().await;
        for _ in 0..MAX_WRITE_ATTEMPTS {
            let (version, mut inner) = self.current().await?;
            let out = f(&mut inner)?;
            let bytes = serde_json::to_vec(&inner.snapshot()).map_err(|e| {
                MeterError::Unavailable(format!("the meter records do not serialise: {e}"))
            })?;
            if self.doc.save(version, bytes).await?.is_some() {
                return Ok(out);
            }
        }
        Err(MeterError::Unavailable(
            "the meter records changed concurrently too often; retry".into(),
        ))
    }
}

/// A [`VersionedDoc`] in memory (tests).
#[derive(Debug, Default)]
pub struct MemDoc(Mutex<Option<(u64, Vec<u8>)>>);

#[async_trait]
impl VersionedDoc for MemDoc {
    async fn load(&self) -> Result<Option<(u64, Vec<u8>)>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> Result<Option<u64>> {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().map(|(v, _)| *v) != expected {
            return Ok(None);
        }
        let version = expected.map_or(1, |v| v + 1);
        *slot = Some((version, doc));
        Ok(Some(version))
    }
}

/// The records as one value: what [`MemStore`] keeps behind a lock and
/// [`DocStore`] keeps in a versioned document.
impl Inner {
    fn create_customer(&mut self, c: Customer, idempotency_key: Option<&str>) -> Result<Customer> {
        let s = self;
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

    fn customer(&mut self, id: &str) -> Result<Option<Customer>> {
        Ok(self.customers.get(id).cloned())
    }

    fn customer_by_stripe(&mut self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        Ok(self
            .customers
            .values()
            .find(|c| c.stripe_customer_id.as_deref() == Some(stripe_customer_id))
            .cloned())
    }

    fn customer_by_subject(&mut self, subject: &SubjectKey) -> Result<Option<Customer>> {
        Ok(self
            .customers
            .values()
            .find(|c| c.subjects.contains(subject))
            .cloned())
    }

    fn list_customers(
        &mut self,
        org: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Customer>> {
        let s = &*self;
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

    fn update_customer(&mut self, mut c: Customer) -> Result<Customer> {
        let s = self;
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

    fn create_entitlement(
        &mut self,
        e: Entitlement,
        idempotency_key: Option<&str>,
    ) -> Result<Entitlement> {
        let s = self;
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

    fn entitlement(&mut self, id: &EntitlementId) -> Result<Option<Entitlement>> {
        Ok(self.entitlements.get(id.as_str()).cloned())
    }

    fn entitlement_for(
        &mut self,
        subject: &SubjectKey,
        feature: &FeatureKey,
    ) -> Result<Option<Entitlement>> {
        Ok(self
            .entitlements
            .values()
            .find(|e| &e.subject == subject && &e.feature == feature)
            .cloned())
    }

    fn list_entitlements(
        &mut self,
        subject: Option<&SubjectKey>,
        feature: Option<&FeatureKey>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Entitlement>> {
        let s = &*self;
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

    fn update_entitlement(&mut self, e: Entitlement) -> Result<Entitlement> {
        let s = self;
        if !s.entitlements.contains_key(e.id.as_str()) {
            return Err(MeterError::not_found("entitlement", e.id.as_str()));
        }
        s.entitlements.insert(e.id.to_string(), e.clone());
        Ok(e)
    }

    fn delete_entitlement(&mut self, id: &EntitlementId) -> Result<()> {
        let s = self;
        s.entitlements.remove(id.as_str());
        s.grants.retain(|_, g| &g.entitlement != id);
        Ok(())
    }

    fn create_grant(&mut self, g: Grant, idempotency_key: Option<&str>) -> Result<Grant> {
        let s = self;
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

    fn grant(&mut self, id: &GrantId) -> Result<Option<Grant>> {
        Ok(self.grants.get(id.as_str()).cloned())
    }

    fn list_grants(
        &mut self,
        entitlement: &EntitlementId,
        include_voided: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Grant>> {
        let s = &*self;
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

    fn update_grant(&mut self, g: Grant) -> Result<Grant> {
        let s = self;
        if !s.grants.contains_key(g.id.as_str()) {
            return Err(MeterError::not_found("grant", g.id.as_str()));
        }
        s.grants.insert(g.id.to_string(), g.clone());
        Ok(g)
    }

    fn create_plan(&mut self, p: PlanRecord) -> Result<PlanRecord> {
        let s = self;
        let key = (p.name.to_string(), p.version);
        if s.plans.contains_key(&key) {
            return Err(MeterError::exists(
                "plan",
                format!("{} v{}", p.name, p.version),
            ));
        }
        s.plans.insert(key, p.clone());
        Ok(p)
    }

    fn plan(&mut self, name: &PlanId, version: u32) -> Result<Option<PlanRecord>> {
        Ok(self.plans.get(&(name.to_string(), version)).cloned())
    }

    fn list_plans(&mut self, name: Option<&PlanId>) -> Result<Vec<PlanRecord>> {
        Ok(self
            .plans
            .values()
            .filter(|p| name.is_none_or(|n| &p.name == n))
            .cloned()
            .collect())
    }

    fn meter(&mut self, slug: &Slug) -> Result<Option<Meter>> {
        Ok(self.meters.get(slug.as_str()).cloned())
    }

    fn put_meter(&mut self, m: Meter) -> Result<()> {
        self.meters.insert(m.slug.to_string(), m);
        Ok(())
    }

    fn list_meters(&mut self) -> Result<Vec<Meter>> {
        Ok(self.meters.values().cloned().collect())
    }

    fn feature(&mut self, key: &FeatureKey) -> Result<Option<Feature>> {
        Ok(self.features.get(key.as_str()).cloned())
    }

    fn put_feature(&mut self, f: Feature) -> Result<()> {
        self.features.insert(f.key.to_string(), f);
        Ok(())
    }

    fn list_features(&mut self) -> Result<Vec<Feature>> {
        Ok(self.features.values().cloned().collect())
    }

    fn records(&mut self) -> Result<Records> {
        Ok(Records {
            customers: self.customers.values().cloned().collect(),
            entitlements: self.entitlements.values().cloned().collect(),
            grants: self
                .grants
                .values()
                .filter(|g| g.voided_at.is_none())
                .cloned()
                .collect(),
            meters: self.meters.values().cloned().collect(),
            features: self.features.values().cloned().collect(),
        })
    }
}

#[async_trait]
impl MeterStore for MemStore {
    async fn create_customer(
        &self,
        c: Customer,
        idempotency_key: Option<&str>,
    ) -> Result<Customer> {
        self.lock().create_customer(c, idempotency_key)
    }
    async fn customer(&self, id: &str) -> Result<Option<Customer>> {
        self.lock().customer(id)
    }
    async fn customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        self.lock().customer_by_stripe(stripe_customer_id)
    }
    async fn customer_by_subject(&self, subject: &SubjectKey) -> Result<Option<Customer>> {
        self.lock().customer_by_subject(subject)
    }
    async fn list_customers(
        &self,
        org: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Customer>> {
        self.lock().list_customers(org, after, limit)
    }
    async fn update_customer(&self, c: Customer) -> Result<Customer> {
        self.lock().update_customer(c)
    }
    async fn create_entitlement(
        &self,
        e: Entitlement,
        idempotency_key: Option<&str>,
    ) -> Result<Entitlement> {
        self.lock().create_entitlement(e, idempotency_key)
    }
    async fn entitlement(&self, id: &EntitlementId) -> Result<Option<Entitlement>> {
        self.lock().entitlement(id)
    }
    async fn entitlement_for(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
    ) -> Result<Option<Entitlement>> {
        self.lock().entitlement_for(subject, feature)
    }
    async fn list_entitlements(
        &self,
        subject: Option<&SubjectKey>,
        feature: Option<&FeatureKey>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Entitlement>> {
        self.lock()
            .list_entitlements(subject, feature, after, limit)
    }
    async fn update_entitlement(&self, e: Entitlement) -> Result<Entitlement> {
        self.lock().update_entitlement(e)
    }
    async fn delete_entitlement(&self, id: &EntitlementId) -> Result<()> {
        self.lock().delete_entitlement(id)
    }
    async fn create_grant(&self, g: Grant, idempotency_key: Option<&str>) -> Result<Grant> {
        self.lock().create_grant(g, idempotency_key)
    }
    async fn grant(&self, id: &GrantId) -> Result<Option<Grant>> {
        self.lock().grant(id)
    }
    async fn list_grants(
        &self,
        entitlement: &EntitlementId,
        include_voided: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Grant>> {
        self.lock()
            .list_grants(entitlement, include_voided, after, limit)
    }
    async fn update_grant(&self, g: Grant) -> Result<Grant> {
        self.lock().update_grant(g)
    }
    async fn create_plan(&self, p: PlanRecord) -> Result<PlanRecord> {
        self.lock().create_plan(p)
    }
    async fn plan(&self, name: &PlanId, version: u32) -> Result<Option<PlanRecord>> {
        self.lock().plan(name, version)
    }
    async fn list_plans(&self, name: Option<&PlanId>) -> Result<Vec<PlanRecord>> {
        self.lock().list_plans(name)
    }
    async fn meter(&self, slug: &Slug) -> Result<Option<Meter>> {
        self.lock().meter(slug)
    }
    async fn put_meter(&self, m: Meter) -> Result<()> {
        self.lock().put_meter(m)
    }
    async fn list_meters(&self) -> Result<Vec<Meter>> {
        self.lock().list_meters()
    }
    async fn feature(&self, key: &FeatureKey) -> Result<Option<Feature>> {
        self.lock().feature(key)
    }
    async fn put_feature(&self, f: Feature) -> Result<()> {
        self.lock().put_feature(f)
    }
    async fn list_features(&self) -> Result<Vec<Feature>> {
        self.lock().list_features()
    }
    async fn records(&self) -> Result<Records> {
        self.lock().records()
    }
}

#[async_trait]
impl MeterStore for DocStore {
    async fn create_customer(
        &self,
        c: Customer,
        idempotency_key: Option<&str>,
    ) -> Result<Customer> {
        self.write(|s| s.create_customer(c.clone(), idempotency_key))
            .await
    }
    async fn customer(&self, id: &str) -> Result<Option<Customer>> {
        self.read(|s| s.customer(id)).await
    }
    async fn customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        self.read(|s| s.customer_by_stripe(stripe_customer_id))
            .await
    }
    async fn customer_by_subject(&self, subject: &SubjectKey) -> Result<Option<Customer>> {
        self.read(|s| s.customer_by_subject(subject)).await
    }
    async fn list_customers(
        &self,
        org: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Customer>> {
        self.read(|s| s.list_customers(org, after, limit)).await
    }
    async fn update_customer(&self, c: Customer) -> Result<Customer> {
        self.write(|s| s.update_customer(c.clone())).await
    }
    async fn create_entitlement(
        &self,
        e: Entitlement,
        idempotency_key: Option<&str>,
    ) -> Result<Entitlement> {
        self.write(|s| s.create_entitlement(e.clone(), idempotency_key))
            .await
    }
    async fn entitlement(&self, id: &EntitlementId) -> Result<Option<Entitlement>> {
        self.read(|s| s.entitlement(id)).await
    }
    async fn entitlement_for(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
    ) -> Result<Option<Entitlement>> {
        self.read(|s| s.entitlement_for(subject, feature)).await
    }
    async fn list_entitlements(
        &self,
        subject: Option<&SubjectKey>,
        feature: Option<&FeatureKey>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Entitlement>> {
        self.read(|s| s.list_entitlements(subject, feature, after, limit))
            .await
    }
    async fn update_entitlement(&self, e: Entitlement) -> Result<Entitlement> {
        self.write(|s| s.update_entitlement(e.clone())).await
    }
    async fn delete_entitlement(&self, id: &EntitlementId) -> Result<()> {
        self.write(|s| s.delete_entitlement(id)).await
    }
    async fn create_grant(&self, g: Grant, idempotency_key: Option<&str>) -> Result<Grant> {
        self.write(|s| s.create_grant(g.clone(), idempotency_key))
            .await
    }
    async fn grant(&self, id: &GrantId) -> Result<Option<Grant>> {
        self.read(|s| s.grant(id)).await
    }
    async fn list_grants(
        &self,
        entitlement: &EntitlementId,
        include_voided: bool,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Grant>> {
        self.read(|s| s.list_grants(entitlement, include_voided, after, limit))
            .await
    }
    async fn update_grant(&self, g: Grant) -> Result<Grant> {
        self.write(|s| s.update_grant(g.clone())).await
    }
    async fn create_plan(&self, p: PlanRecord) -> Result<PlanRecord> {
        self.write(|s| s.create_plan(p.clone())).await
    }
    async fn plan(&self, name: &PlanId, version: u32) -> Result<Option<PlanRecord>> {
        self.read(|s| s.plan(name, version)).await
    }
    async fn list_plans(&self, name: Option<&PlanId>) -> Result<Vec<PlanRecord>> {
        self.read(|s| s.list_plans(name)).await
    }
    async fn meter(&self, slug: &Slug) -> Result<Option<Meter>> {
        self.read(|s| s.meter(slug)).await
    }
    async fn put_meter(&self, m: Meter) -> Result<()> {
        self.write(|s| s.put_meter(m.clone())).await
    }
    async fn list_meters(&self) -> Result<Vec<Meter>> {
        self.read(|s| s.list_meters()).await
    }
    async fn feature(&self, key: &FeatureKey) -> Result<Option<Feature>> {
        self.read(|s| s.feature(key)).await
    }
    async fn put_feature(&self, f: Feature) -> Result<()> {
        self.write(|s| s.put_feature(f.clone())).await
    }
    async fn list_features(&self) -> Result<Vec<Feature>> {
        self.read(|s| s.list_features()).await
    }
    async fn records(&self) -> Result<Records> {
        self.read(|s| s.records()).await
    }
}
