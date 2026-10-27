//! A small client for the Confluent Cloud APIs Loams calls, and the
//! `loams byoc confluent` commands (design 80 §7.2, D1827; plan CC1 Tasks 4
//! and 6).
//!
//! - [`Client`]: the Cloud API with a Cloud API key (basic auth), retries on
//!   429 (honouring `Retry-After`) and 5xx, and errors that never carry a
//!   credential.
//! - [`keys`]: create, list and delete API keys. A created key's secret is a
//!   [`Secret`], which zeroises on drop and never prints.
//! - [`clusters`]: read and list Kafka clusters (cloud, region, bootstrap) and
//!   set an elastic cluster's `max_ecku` (the guard's throttle).
//! - [`metrics`]: hourly Metrics API queries per cluster (and per principal
//!   for request and response bytes, CC0 R0.7).
//! - [`costs`]: the Costs API's daily lines over any UTC date range, split
//!   into windows of at most 30 days.
//! - [`store`]: write a value into the user's cloud secret store through the
//!   cloud's own CLI (`aws`, `gcloud`, `az`) with the user's credentials; the
//!   value goes on stdin, never in argv.
//! - [`cli`]: `loams byoc confluent key create|rotate` and `check`.
//!
//! Regions are read by `scripts/byoc/confluent-tables.sh` (CC0 R0.4: an
//! undocumented endpoint), not here. The budget guard (CC2) comes later.

pub mod cli;
pub mod clusters;
pub mod costs;
pub mod keys;
pub mod metrics;
pub mod store;

mod client;
mod secret;

pub use client::{Client, CloudKey, DEFAULT_BASE, Error};
pub use secret::Secret;
