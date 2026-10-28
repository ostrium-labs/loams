//! Loams Meter's sans-I/O core (design §63 §5.2, D1120).
//!
//! The types of §9 (events, meters, features, entitlements, grants,
//! balances, money) and the usage profile of §6 on top of
//! `loams-cloudevents`. The balance engine and the lease allocator are pure
//! state machines over these types. No async, no I/O, no Arrow: the
//! `fabric/` workspace links this crate by path.

pub mod amount;
pub mod entitlement;
pub mod event;
pub mod ids;
pub mod lease;
pub mod meter;
pub mod refusal;

pub use amount::{Amount, AmountError, Currency, Money};
pub use entitlement::{
    AsOf, Balance, Entitlement, EntitlementKind, EntitlementSource, Expiry, ExpiryUnit, Feature,
    Grant, GrantError, GrantKind, Interval, Mode, Period, Rollover, Unit, UsagePeriod,
};
pub use event::{Event, EventError, Origin, is_loams_usage_type, is_reserved_type};
pub use ids::{
    EntitlementId, FeatureKey, GrantId, IdError, LeaseId, PlanId, Slug, SubjectKey, TenantId,
    UtcNanos,
};
pub use lease::{Allocator, AllocatorConfig, LeaseGrant, LeasePolicy, LeaseReport, LeaseRequest};
pub use meter::{Aggregation, JsonPath, Meter, MeterError, WindowSize};
pub use refusal::RefusalReason;
