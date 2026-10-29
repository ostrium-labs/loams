//! The `meter` role put together (design §63 §5.2): what `loams` builds
//! when its feature `meter` is on.
//!
//! Part 1 serves the public services (`CustomerService`,
//! `EntitlementService`, `PlanService`, `NotificationService`) on the main
//! port, over records kept in a [`VersionedDoc`]. Part 2 adds the counter
//! owners ([`crate::run`]): balances come from their counters
//! ([`crate::run::CounterValues`]), and the internal `LeaseService` grants
//! from them. The House schedule and House-backed meter usage come next;
//! until then every meter that exists counts as used ([`NoHouseYet`]), so a
//! plan apply may never change a meter that might have events.
//! [`CountersNotRunning`] is for hosts (and tests) that run no counters.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use loams_authz::Authorizer;
use loams_meter_core::{Balance, Entitlement, Grant, Slug, UtcNanos};

use crate::audit::AuditSink;
use crate::authz::MeterAuthz;
use crate::customers::{Clock, Customers};
use crate::entitlements::{Entitlements, ValueSource};
use crate::error::{MeterError, Result};
use crate::house::{HouseSchedule, RoleHouse};
use crate::queries::MeterQueries;
use crate::notify::Notifier;
use crate::notify::notifier::{Console, Mailer, NotificationStore};
use crate::notify::webhook::Transport;
use crate::plans::{MeterUsage, Plans};
use crate::store::{DocStore, MeterStore, VersionedDoc};

/// The billing exporter's service identity: the only caller of
/// `SetPaymentState` (§63 §19 point 5).
pub const BILLING_SERVICE: &str = "meter-export";

/// Balances before the counter owners run: refused, never guessed.
#[derive(Debug, Clone, Copy, Default)]
pub struct CountersNotRunning;

#[async_trait]
impl ValueSource for CountersNotRunning {
    async fn value(&self, _: &Entitlement, _: &[Grant], _: UtcNanos) -> Result<Balance> {
        Err(MeterError::Unavailable(
            "the meter's counters are not running on this instance yet".into(),
        ))
    }
}

/// Without a House to ask: every meter that exists counts as having events
/// (the safe answer for "may its semantics change").
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHouseYet;

#[async_trait]
impl MeterUsage for NoHouseYet {
    async fn has_events(&self, _: &Slug) -> Result<bool> {
        Ok(true)
    }
}

/// What the role needs from its host.
pub struct RoleParts {
    /// Where customers, entitlements, grants and plans are kept.
    pub records: Arc<dyn VersionedDoc>,
    /// Where webhook endpoints, rules and deliveries are kept.
    pub notifications: Arc<dyn NotificationStore>,
    pub authorizer: Arc<dyn Authorizer>,
    pub audit: Arc<dyn AuditSink>,
    pub transport: Arc<dyn Transport>,
    pub mailer: Option<Arc<dyn Mailer>>,
    pub console: Option<Arc<dyn Console>>,
    /// 32 random bytes per call (webhook secrets).
    pub random: Arc<dyn Fn() -> [u8; 32] + Send + Sync>,
    pub clock: Clock,
    pub values: Arc<dyn ValueSource>,
    pub usage: Arc<dyn MeterUsage>,
    /// The House meter queries and the close schedule run against
    /// ([`RoleHouse::Absent`]: none configured).
    pub house: Arc<RoleHouse>,
}

impl fmt::Debug for RoleParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoleParts").finish_non_exhaustive()
    }
}

/// The role's services.
#[derive(Clone)]
pub struct MeterRole {
    pub store: Arc<dyn MeterStore>,
    pub house: Arc<RoleHouse>,
    pub queries: MeterQueries<RoleHouse>,
    pub customers: Customers,
    pub entitlements: Entitlements,
    pub plans: Plans,
    pub notifier: Notifier,
}

impl fmt::Debug for MeterRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MeterRole").finish_non_exhaustive()
    }
}

impl MeterRole {
    pub fn new(p: RoleParts) -> Self {
        let store: Arc<dyn MeterStore> = Arc::new(DocStore::new(p.records));
        let authz = MeterAuthz::new(p.authorizer, BILLING_SERVICE);
        MeterRole {
            queries: MeterQueries::new(store.clone(), authz.clone(), p.house.clone()),
            house: p.house,
            customers: Customers::new(
                store.clone(),
                authz.clone(),
                p.audit.clone(),
                p.clock.clone(),
            ),
            entitlements: Entitlements::new(
                store.clone(),
                authz.clone(),
                p.audit.clone(),
                p.values,
                p.clock.clone(),
            ),
            plans: Plans::new(
                store.clone(),
                authz.clone(),
                p.audit.clone(),
                p.usage,
                p.clock.clone(),
            ),
            notifier: Notifier::new(
                store.clone(),
                p.notifications,
                authz,
                p.audit,
                p.transport,
                p.mailer,
                p.console,
                p.random,
                p.clock,
            ),
            store,
        }
    }

    /// The public `loams.meter.v1` services, on the main port's Connect
    /// router.
    pub fn register(&self, router: connectrpc::Router) -> connectrpc::Router {
        let router = crate::connect::register(
            router,
            self.customers.clone(),
            self.entitlements.clone(),
            self.plans.clone(),
        );
        let router = crate::connect::meters::register(router, self.queries.clone());
        crate::connect::notifications::register(router, self.notifier.clone())
    }

    /// The OpenMeter-compatible HTTP routes (entitlement values and grants,
    /// meter queries), for the role's host to mount behind its
    /// authentication, which puts the [`crate::authz::Caller`] in each
    /// request.
    pub fn http_routes(&self) -> axum::Router {
        crate::openmeter::router(self.entitlements.clone())
            .merge(crate::house::query_router(self.queries.clone()))
    }

    /// The House schedule (schema, close job, period close), when a House
    /// is configured.
    pub fn house_schedule(&self, clock: Clock) -> Option<HouseSchedule<RoleHouse>> {
        self.house
            .is_configured()
            .then(|| HouseSchedule::new(self.house.clone(), self.store.clone(), clock))
    }
}

/// The public services of `loams.meter.v1` (the internal `LeaseService` is
/// not among them).
pub const PUBLIC_SERVICES: &[&str] = &[
    "loams.meter.v1.CustomerService",
    "loams.meter.v1.EntitlementService",
    "loams.meter.v1.PlanService",
    "loams.meter.v1.NotificationService",
    "loams.meter.v1.MeterService",
];
