//! Managed stream and batch engines: RisingWave, Flink SQL and Spark (design 81).
//!
//! This crate is the shared layer the three engine crates plug into (D2010–D2019):
//!
//! - [`ManagedEngine`]: what an engine adapter implements (lifecycle, observation, usage),
//!   with the optional [`SqlEngine`] (RisingWave, Flink) and [`SparkEngine`] (Spark) surfaces.
//! - [`EngineRegistry`]: one adapter per [`EngineKind`], those whose profiles are on.
//! - [`EngineService`]: the instance records ([`InstanceStore`]), the phase machine
//!   ([`Phase`]), validation and metering ([`meter`]): one Loams Meter usage event per
//!   instance per window with vCPU-seconds, memory GB-seconds and state bytes.
//! - [`http`]: the namespace-scoped facade `/v1/namespaces/{p}/engines/…`, contract
//!   `api/engines/openapi.json`, and [`authz`]: its `can_*` declarations for IA2's enforcer.
//! - [`testing::FakeEngine`]: an adapter that runs nothing, for tests and the console.
//!
//! Every engine is preview ([`Stability::Preview`]). Engines run as Loams profile services
//! behind loams-gateway, keep state under [`Instance::state_prefix`] in Loams object storage
//! and their metadata in Loams Postgres.

pub mod authz;
mod engine;
mod error;
pub mod http;
pub mod meter;
mod model;
mod registry;
pub mod service;
mod store;
pub mod testing;

pub use engine::{
    CreateSession, DEFAULT_SQL_ROWS, DEFAULT_SQL_TIMEOUT_MS, MAX_SQL_ROWS, MAX_SQL_TIMEOUT_MS,
    ManagedEngine, SparkEngine, SparkJob, SparkJobKind, SparkJobState, SparkSession,
    SparkSessionKind, SparkSessionState, SqlColumn, SqlEngine, SqlRequest, SqlResult, SubmitJob,
};
pub use error::{EngineError, Result};
pub use model::{
    Capabilities, Endpoint, EndpointProtocol, EngineKind, EngineSize, HealthCheck, HealthState,
    Instance, InstanceId, InstanceSpec, InstanceStatus, MAX_REPLICAS, Observed, Op, Phase,
    SizeRequest, SizeTier, SqlDialect, Stability, UsageSample, validate_name,
};
pub use registry::{EngineCatalog, EngineInfo, EngineRegistry, SizeInfo};
pub use service::{CreateInstance, EngineService, UpdateInstance};
pub use store::{InstanceStore, MemoryStore};
