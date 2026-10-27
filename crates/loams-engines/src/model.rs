//! The data model shared by every managed engine (design 81, D2010–D2013, D2019).
//!
//! The wire form of each type is the schema of the same name in `api/engines/openapi.json`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::EngineError;

/// The three managed engines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Risingwave,
    Flink,
    Spark,
}

impl EngineKind {
    pub const ALL: [EngineKind; 3] = [Self::Risingwave, Self::Flink, Self::Spark];

    /// The wire and metering name: `risingwave`, `flink`, `spark`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Risingwave => "risingwave",
            Self::Flink => "flink",
            Self::Spark => "spark",
        }
    }

    /// The name people read.
    pub fn label(self) -> &'static str {
        match self {
            Self::Risingwave => "RisingWave",
            Self::Flink => "Flink SQL",
            Self::Spark => "Spark",
        }
    }
}

impl fmt::Display for EngineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EngineKind {
    type Err = EngineError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "risingwave" => Ok(Self::Risingwave),
            "flink" => Ok(Self::Flink),
            "spark" => Ok(Self::Spark),
            other => Err(EngineError::Invalid(format!(
                "unknown engine kind {other:?} (risingwave, flink, spark)"
            ))),
        }
    }
}

/// How far an engine is along (D2019). All three start as `Preview`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    #[default]
    Preview,
    Beta,
    Stable,
}

/// Named size tiers with fixed per-replica shapes (D2011), so metering and quotas have a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeTier {
    Xs,
    S,
    M,
    L,
    Xl,
}

impl SizeTier {
    pub const ALL: [SizeTier; 5] = [Self::Xs, Self::S, Self::M, Self::L, Self::Xl];

    /// `(vCPU in millicores, memory in MiB)` per replica.
    pub fn shape(self) -> (u32, u32) {
        match self {
            Self::Xs => (500, 1024),
            Self::S => (1000, 4096),
            Self::M => (2000, 8192),
            Self::L => (4000, 16384),
            Self::Xl => (8000, 32768),
        }
    }
}

/// The most replicas one instance may ask for.
pub const MAX_REPLICAS: u16 = 64;

/// An instance's size: a tier's shape times `replicas` (RisingWave compute nodes, Flink
/// TaskManagers, Spark executors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineSize {
    pub tier: SizeTier,
    pub vcpu_milli: u32,
    pub memory_mib: u32,
    pub replicas: u16,
}

impl EngineSize {
    /// The tier's shape with `replicas` replicas.
    pub fn tier(tier: SizeTier, replicas: u16) -> Self {
        let (vcpu_milli, memory_mib) = tier.shape();
        Self {
            tier,
            vcpu_milli,
            memory_mib,
            replicas,
        }
    }

    /// vCPU millicores over all replicas.
    pub fn total_vcpu_milli(&self) -> u64 {
        u64::from(self.vcpu_milli) * u64::from(self.replicas)
    }

    /// Memory in MiB over all replicas.
    pub fn total_memory_mib(&self) -> u64 {
        u64::from(self.memory_mib) * u64::from(self.replicas)
    }
}

/// What a caller asks for: a tier and, optionally, replicas (default 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SizeRequest {
    pub tier: SizeTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replicas: Option<u16>,
}

impl SizeRequest {
    pub fn resolve(&self) -> Result<EngineSize, EngineError> {
        let replicas = self.replicas.unwrap_or(1);
        if replicas == 0 || replicas > MAX_REPLICAS {
            return Err(EngineError::Invalid(format!(
                "replicas must be between 1 and {MAX_REPLICAS}, got {replicas}"
            )));
        }
        Ok(EngineSize::tier(self.tier, replicas))
    }
}

/// What a caller wants an instance to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceSpec {
    /// A DNS label, 1–40 characters, unique in the namespace.
    pub name: String,
    pub kind: EngineKind,
    /// `None` runs the adapter's default version.
    pub version: Option<String>,
    pub size: EngineSize,
    /// Engine-specific settings; the adapter validates the keys.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Checks an instance name: a lowercase DNS label of 1–40 characters.
pub fn validate_name(name: &str) -> Result<(), EngineError> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name.as_bytes()[0].is_ascii_lowercase()
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(EngineError::Invalid(format!(
            "{name:?} is not a valid name: 1-40 characters of a-z, 0-9 and '-', starting with a letter and not ending with '-'"
        )))
    }
}

/// An instance id: `eng_` and a lowercase ULID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstanceId(pub String);

impl InstanceId {
    pub const PREFIX: &'static str = "eng_";

    pub fn generate() -> Self {
        Self(format!(
            "{}{}",
            Self::PREFIX,
            ulid::Ulid::generate().to_string().to_ascii_lowercase()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One phase machine for all three engines (D2012).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Pending,
    Provisioning,
    Running,
    Scaling,
    Upgrading,
    Suspending,
    Suspended,
    Resuming,
    Deleting,
    Deleted,
    Failed,
}

/// A lifecycle operation, for [`Phase::allows`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Scale,
    Upgrade,
    Suspend,
    Resume,
    Delete,
    /// SQL, Spark sessions and jobs: the engine must be up.
    Use,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Upgrade => "upgrade",
            Self::Suspend => "suspend",
            Self::Resume => "resume",
            Self::Delete => "delete",
            Self::Use => "use",
        }
    }
}

impl Phase {
    /// The phase an operation moves the instance into while the adapter works.
    pub fn transitional(op: Op) -> Option<Phase> {
        match op {
            Op::Scale => Some(Self::Scaling),
            Op::Upgrade => Some(Self::Upgrading),
            Op::Suspend => Some(Self::Suspending),
            Op::Resume => Some(Self::Resuming),
            Op::Delete => Some(Self::Deleting),
            Op::Use => None,
        }
    }

    /// Whether `op` may start from this phase. A transitional phase admits only delete, so
    /// two lifecycle operations never overlap; a failed instance may be deleted, scaled or
    /// upgraded (which is how it is repaired).
    pub fn allows(self, op: Op) -> bool {
        use Phase::*;
        match op {
            Op::Delete => !matches!(self, Deleting | Deleted),
            Op::Scale | Op::Upgrade => matches!(self, Running | Suspended | Failed),
            Op::Suspend => self == Running,
            Op::Resume => self == Suspended,
            Op::Use => self == Running,
        }
    }

    /// Whether compute is (or may be) running and billed for vCPU and memory.
    pub fn has_compute(self) -> bool {
        use Phase::*;
        matches!(
            self,
            Provisioning | Running | Scaling | Upgrading | Suspending | Resuming
        )
    }

    /// Whether the instance still holds state (so `state_bytes` is metered).
    pub fn has_state(self) -> bool {
        !matches!(self, Phase::Pending | Phase::Deleted)
    }

    pub fn as_str(self) -> &'static str {
        use Phase::*;
        match self {
            Pending => "pending",
            Provisioning => "provisioning",
            Running => "running",
            Scaling => "scaling",
            Upgrading => "upgrading",
            Suspending => "suspending",
            Suspended => "suspended",
            Resuming => "resuming",
            Deleting => "deleting",
            Deleted => "deleted",
            Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub state: HealthState,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub phase: Phase,
    pub health: HealthState,
    #[serde(default)]
    pub checks: Vec<HealthCheck>,
    /// The version that is running, which may lag the requested one during an upgrade.
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
}

impl InstanceStatus {
    pub fn new(phase: Phase, at: OffsetDateTime) -> Self {
        Self {
            phase,
            health: HealthState::Unknown,
            checks: Vec::new(),
            version: None,
            message: None,
            observed_at: at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointProtocol {
    /// RisingWave's Postgres wire.
    Postgres,
    /// Flink's SQL Gateway REST API.
    FlinkSqlGateway,
    /// Spark Connect (gRPC).
    SparkConnect,
    Http,
    /// The engine's own dashboard.
    WebUi,
    Kafka,
}

/// How to reach an instance. `url` is always the loams-gateway address: engines are never
/// exposed on their own (design 81).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub name: String,
    pub protocol: EndpointProtocol,
    pub url: String,
    pub internal: bool,
}

/// One managed engine instance, as stored and as served.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: InstanceId,
    pub namespace: String,
    /// The org that owns the namespace; the billing subject is `org_<org>` (design 63 §6.1).
    #[serde(default)]
    pub org: Option<String>,
    #[serde(flatten)]
    pub spec: InstanceSpec,
    pub status: InstanceStatus,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// Where state and checkpoints live in Loams object storage: `engines/<namespace>/<id>/`.
    pub state_prefix: String,
    pub stability: Stability,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl Instance {
    /// The object-storage prefix for an instance's state.
    pub fn state_prefix_for(namespace: &str, id: &InstanceId) -> String {
        format!("engines/{namespace}/{id}/")
    }

    /// Folds what the adapter observed into the record.
    pub fn apply(&mut self, observed: Observed, at: OffsetDateTime) {
        self.status = InstanceStatus {
            phase: observed.phase,
            health: observed.health,
            checks: observed.checks,
            version: observed.version.or_else(|| self.status.version.take()),
            message: observed.message,
            observed_at: at,
        };
        self.endpoints = observed.endpoints;
        self.updated_at = at;
    }
}

/// What an adapter saw of an instance; the service folds it into [`InstanceStatus`].
#[derive(Clone, Debug, PartialEq)]
pub struct Observed {
    pub phase: Phase,
    pub health: HealthState,
    pub checks: Vec<HealthCheck>,
    pub version: Option<String>,
    pub message: Option<String>,
    pub endpoints: Vec<Endpoint>,
}

impl Observed {
    /// A bare observation in `phase`, health unknown, no endpoints.
    pub fn phase(phase: Phase) -> Self {
        Self {
            phase,
            health: HealthState::Unknown,
            checks: Vec::new(),
            version: None,
            message: None,
            endpoints: Vec::new(),
        }
    }
}

/// One metering window of one instance (D2013, units per ENG81 R17/D1920: integers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSample {
    #[serde(with = "time::serde::rfc3339")]
    pub window_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub window_end: OffsetDateTime,
    /// CPU time in milliseconds (one vCPU busy for one second is 1000).
    pub cpu_ms: u64,
    /// Memory held, in MiB × milliseconds.
    pub memory_mib_ms: u64,
    /// State held in object storage at the end of the window (a gauge).
    pub state_bytes: u64,
    /// The reserved size, not a measurement (the runtime gave none).
    pub estimated: bool,
}

impl UsageSample {
    /// The usage of `size` running for the whole window, plus `state_bytes`, marked
    /// `estimated`. Adapters that cannot measure use this; adapters that can (cgroups, k8s
    /// metrics) report what the runtime saw with `estimated: false`.
    pub fn provisioned(
        size: &EngineSize,
        window_start: OffsetDateTime,
        window_end: OffsetDateTime,
        state_bytes: u64,
    ) -> Self {
        let ms = u128::try_from((window_end - window_start).whole_milliseconds()).unwrap_or(0);
        let scale = |per: u64| u64::try_from(u128::from(per) * ms).unwrap_or(u64::MAX);
        Self {
            window_start,
            window_end,
            // millicores × ms / 1000 = CPU ms.
            cpu_ms: scale(size.total_vcpu_milli()) / 1000,
            memory_mib_ms: scale(size.total_memory_mib()),
            state_bytes,
            estimated: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqlDialect {
    /// RisingWave speaks Postgres.
    Postgres,
    FlinkSql,
    SparkSql,
}

/// What an adapter supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub versions: Vec<String>,
    pub default_version: String,
    pub sql_dialect: Option<SqlDialect>,
    pub sessions: bool,
    pub jobs: bool,
    /// The engine can drop its compute and keep its state.
    pub suspend: bool,
    pub stability: Stability,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let long = "a".repeat(40);
        let too_long = "a".repeat(41);
        for ok in ["a", "rw-1", "orders-stream", long.as_str()] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "1a", "A", "a-", "a_b", "a.b", too_long.as_str()] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ids_are_prefixed_lowercase_ulids() {
        let id = InstanceId::generate();
        assert!(id.as_str().starts_with("eng_"));
        assert_eq!(id.as_str().len(), 4 + 26);
        assert_eq!(id.as_str(), id.as_str().to_ascii_lowercase());
        assert_ne!(id, InstanceId::generate());
    }

    #[test]
    fn sizes() {
        let s = SizeRequest {
            tier: SizeTier::M,
            replicas: Some(3),
        }
        .resolve()
        .unwrap();
        assert_eq!((s.vcpu_milli, s.memory_mib, s.replicas), (2000, 8192, 3));
        assert_eq!(s.total_vcpu_milli(), 6000);
        assert_eq!(s.total_memory_mib(), 24576);
        for r in [0, MAX_REPLICAS + 1] {
            assert!(
                SizeRequest {
                    tier: SizeTier::Xs,
                    replicas: Some(r)
                }
                .resolve()
                .is_err()
            );
        }
        assert_eq!(
            SizeRequest {
                tier: SizeTier::Xs,
                replicas: None
            }
            .resolve()
            .unwrap()
            .replicas,
            1
        );
    }

    #[test]
    fn phase_machine() {
        assert!(Phase::Running.allows(Op::Suspend));
        assert!(!Phase::Suspended.allows(Op::Suspend));
        assert!(Phase::Suspended.allows(Op::Resume));
        assert!(!Phase::Running.allows(Op::Resume));
        assert!(Phase::Failed.allows(Op::Scale));
        assert!(!Phase::Scaling.allows(Op::Upgrade));
        assert!(Phase::Scaling.allows(Op::Delete));
        assert!(!Phase::Deleted.allows(Op::Delete));
        assert!(!Phase::Suspended.allows(Op::Use));
        assert!(Phase::Running.allows(Op::Use));
        assert!(!Phase::Suspended.has_compute());
        assert!(Phase::Suspended.has_state());
        assert!(!Phase::Deleted.has_state());
    }

    #[test]
    fn provisioned_usage() {
        let t0 = OffsetDateTime::UNIX_EPOCH;
        let size = EngineSize::tier(SizeTier::S, 2);
        let u = UsageSample::provisioned(&size, t0, t0 + time::Duration::seconds(60), 7);
        // s × 2: 2 vCPU and 8 GiB for 60 s.
        assert_eq!(u.cpu_ms, 120_000);
        assert_eq!(u.memory_mib_ms, 8192 * 60_000);
        assert_eq!(u.state_bytes, 7);
        assert!(u.estimated);
        let backwards = UsageSample::provisioned(&size, t0 + time::Duration::seconds(1), t0, 0);
        assert_eq!((backwards.cpu_ms, backwards.memory_mib_ms), (0, 0));
    }

    #[test]
    fn instance_wire_form_is_flat() {
        let t0 = OffsetDateTime::UNIX_EPOCH;
        let id = InstanceId("eng_x".into());
        let inst = Instance {
            state_prefix: Instance::state_prefix_for("p1", &id),
            id,
            namespace: "p1".into(),
            org: Some("acme".into()),
            spec: InstanceSpec {
                name: "rw".into(),
                kind: EngineKind::Risingwave,
                version: None,
                size: EngineSize::tier(SizeTier::Xs, 1),
                config: BTreeMap::new(),
                labels: BTreeMap::new(),
            },
            status: InstanceStatus::new(Phase::Running, t0),
            endpoints: Vec::new(),
            stability: Stability::Preview,
            created_at: t0,
            updated_at: t0,
        };
        let v = serde_json::to_value(&inst).unwrap();
        assert_eq!(v["kind"], "risingwave");
        assert_eq!(v["name"], "rw");
        assert_eq!(v["size"]["tier"], "xs");
        assert_eq!(v["status"]["phase"], "running");
        assert_eq!(v["stability"], "preview");
        assert_eq!(v["state_prefix"], "engines/p1/eng_x/");
        assert_eq!(v["created_at"], "1970-01-01T00:00:00Z");
        let back: Instance = serde_json::from_value(v).unwrap();
        assert_eq!(back, inst);
    }
}
