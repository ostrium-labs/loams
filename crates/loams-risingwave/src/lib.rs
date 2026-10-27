//! Managed RisingWave on Loams (design 81, plan RW1; RisingWave is Loams'
//! companion stream processor, D22, D341). Status: preview, not yet run end
//! to end.
//!
//! One RisingWave cluster per managed instance (ENG81 R3: no shared
//! clusters), in the instance's Loams namespace:
//!
//! - [`model`]: the instance key and reference, the four node roles (meta,
//!   compactor, compute, frontend), size tiers and `config` overrides,
//!   engine versions (D1950).
//! - [`backend`]: the meta store, a per-instance database on Loams Postgres
//!   (D1951), and the Hummock state store under the instance's state prefix
//!   in Loams object storage (D1952).
//! - [`render`]: a cluster as runtime-neutral workloads, its credentials
//!   Secret and `risingwave.toml`; [`runtime`] runs them with compose or
//!   Helm.
//! - [`adapter`]: create, scale, upgrade (rolling, D1955), suspend and
//!   resume (D1954), delete and observe, stateless towards Loams: ENG-UI's
//!   `loams-engines` owns the record, the phase machine and metering
//!   (ENG81 R1).
//! - [`provision`]: where the stores come from; [`credentials`]: where the
//!   instance's passwords are kept; [`bootstrap`]: the service users set on
//!   a new cluster.
//! - [`probe`]: liveness and readiness over PG wire (:4566) and the meta
//!   dashboard (:5691) (D1956).
//! - [`gateway`]: the loams-gateway route of an instance (D1957, R8).
//! - [`engines`]: `loams_engines::ManagedEngine` over the adapter
//!   ([`engines::RisingWaveEngine`]), registered by the facade's host.

pub mod adapter;
pub mod backend;
pub mod bootstrap;
pub mod credentials;
pub mod engines;
pub mod gateway;
pub mod model;
pub mod probe;
pub mod provision;
pub mod render;
pub mod runtime;

// Lane RW-b's modules (connectors, the SQL API, the CLI) are declared below.
pub mod cli;
pub mod connectors;
pub mod sql_api;

pub use adapter::{AdapterConfig, AdapterError, Observation, ObservedPhase, RisingWaveAdapter};
pub use engines::RisingWaveEngine;
pub use model::{ClusterSpec, EngineVersion, InstanceKey, InstanceRef, Role, Size};
