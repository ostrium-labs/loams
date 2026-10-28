//! Loams Meter's `meter` role (design §63 §5.2): the part of the meter that
//! runs as a service. The sans-I/O types, the balance engine and the lease
//! allocator are in `loams-meter-core`; what products link is
//! `loams-meter-gate`.
//!
//! | Module | Task | Does |
//! |---|---|---|
//! | [`notify`] | ME1 Task 14 | Threshold and lifecycle events (deterministic ids), the notifier (signed webhooks with retries, email, console) and `NotificationService` |
//! | [`openmeter`] | ME1 Task 15 | The OpenMeter-compatible entitlement routes (value, grants), checked against OpenMeter's pinned `api/openapi.yaml` |
//! | [`owner`] | ME1 Task 12 | Counter owners: who owns which `_system/_meter` partition (fenced leases), per-partition counters with offset buckets and recent-keys dedup, the rebuild from the House and the stream tail |
//! | [`authz`], [`audit`], [`store`] | ME1 Task 10 | The OpenFGA checks (`meter_admin`, `billing_admin`, `billing_viewer`; agents never hold the admin relations), audit events, the record store |
//! | [`customers`], [`entitlements`], [`connect`] | ME1 Task 10 | `CustomerService` and `EntitlementService`, and their Connect face (`loams.meter.v1`) |
//! | [`plan_yaml`], [`plans`] | ME1 Task 11 | Plans (with their meters and features) as YAML from the GitOps repository: versions and used meters immutable; `PlanService` applies and assigns them |
//! | [`lease_service`] | ME1 Task 17 | `LeaseService` (internal Connect, service identities only) over ME-core's allocator, fenced by owner epochs; the gate's `EpochFence` |
//! | [`reconcile`] | ME1 Task 13 | Every 5 minutes and at period close: counters reset to the House's totals at the pipe's committed offset plus the tail; drift over 1% of an allowance alerts |
//!
//! The House tables and the close job (ME1 Tasks 5–8) and the exporters
//! (Tasks 24–26) live beside these in their own modules.

pub mod audit;
pub mod authz;
pub mod connect;
pub mod customers;
pub mod entitlements;
pub mod error;
pub mod lease_service;
pub mod notify;
pub mod openmeter;
pub mod owner;
pub mod plan_yaml;
pub mod plans;
pub mod reconcile;
pub mod role;
pub mod store;
