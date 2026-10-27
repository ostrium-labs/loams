//! Loams Meter's `meter` role (design §63 §5.2): the part of the meter that
//! runs as a service. The sans-I/O types, the balance engine and the lease
//! allocator are in `loams-meter-core`; what products link is
//! `loams-meter-gate`.
//!
//! | Module | Task | Does |
//! |---|---|---|
//! | [`owner`] | ME1 Task 12 | Counter owners: who owns which `_system/_meter` partition (fenced leases), per-partition counters with offset buckets and recent-keys dedup, the rebuild from the House and the stream tail |
//! | [`authz`], [`audit`], [`store`] | ME1 Task 10 | The OpenFGA checks (`meter_admin`, `billing_admin`, `billing_viewer`; agents never hold the admin relations), audit events, the record store |
//! | [`customers`], [`entitlements`], [`connect`] | ME1 Task 10 | `CustomerService` and `EntitlementService`, and their Connect face (`loams.meter.v1`) |
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
pub mod owner;
pub mod reconcile;
pub mod store;
