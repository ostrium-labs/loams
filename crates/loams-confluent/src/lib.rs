//! A small client for the Confluent Cloud APIs Loams calls, and the
//! `loams byoc confluent` commands (design 80 §7.2, D1827; plan CC1 Tasks 4
//! and 6).
//!
//! - [`Client`]: the Cloud API with a Cloud API key (basic auth), retries on
//!   429 (honouring `Retry-After`) and 5xx, and errors that never carry a
//!   credential.
//! - [`keys`]: create, list and delete API keys. A created key's secret is a
//!   [`Secret`], which zeroises on drop and never prints.
//! - [`clusters`]: read a Kafka cluster (cloud, region, bootstrap).
//! - [`store`]: write a value into the user's cloud secret store through the
//!   cloud's own CLI (`aws`, `gcloud`, `az`) with the user's credentials; the
//!   value goes on stdin, never in argv.
//! - [`cli`]: `loams byoc confluent key create|rotate` and `check`.
//!
//! The metrics, costs and regions calls (Task 4's rest) and the budget guard
//! (CC2) come later.

pub mod cli;
pub mod clusters;
pub mod keys;
pub mod store;

mod client;
mod secret;

pub use client::{Client, CloudKey, DEFAULT_BASE, Error};
pub use secret::Secret;
