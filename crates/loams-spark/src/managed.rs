//! The Spark `ManagedEngine` adapter (SP-a D1999; design 81 R1, R2, R17,
//! R19).
//!
//! ENG-UI's `crates/loams-engines` owns the instance record, ids, the phase
//! machine, the HTTP facade and metering; an adapter is stateless and only
//! talks to the runtime. That crate is not on `dev` yet, so this module
//! mirrors the shapes it posted in `engines-81.md` (with R2's `namespace`
//! for `project`) under the same names. When it lands, the mirror types are
//! deleted and replaced by `use loams_engines::{…}`; the adapter's method
//! bodies do not change.
//!
//! The runtime is reached through [`ClusterApi`] (apply, scale, delete,
//! readiness), so the adapter is tested with a fake and the kube-rs client
//! plugs in without touching it. [`InstanceRouter`] is the proxy's router
//! for Kubernetes: it resolves a namespace's instance and resumes a
//! suspended one on the first call (start-on-first-connect, D1995).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tonic::Status;

use crate::backend::{BackendKind, SparkBackend};
use crate::conf::{Catalog, ClusterConf, KubePlacement, ObjectStore, Placement, Shape, pins};
use crate::k8s::{self, KubeOptions};
use crate::route::{self, Backend, Router, Target};

// ---- Mirror of ENG-UI's loams_engines shapes (delete when it lands) ----

/// The engine kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// RisingWave.
    Risingwave,
    /// Flink.
    Flink,
    /// Spark.
    Spark,
}

/// Named size tiers (D2011).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeTier {
    /// 500m / 1 GiB per replica.
    Xs,
    /// 1 / 4 GiB.
    S,
    /// 2 / 8 GiB.
    M,
    /// 4 / 16 GiB.
    L,
    /// 8 / 32 GiB.
    Xl,
}

/// A size: tier plus per-replica shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSize {
    /// The tier.
    pub tier: SizeTier,
    /// vCPU per replica, milli.
    pub vcpu_milli: u32,
    /// Memory per replica, MiB.
    pub memory_mib: u32,
    /// Executors / Sail workers.
    pub replicas: u16,
}

impl EngineSize {
    /// The tier's fixed shape (one replica).
    pub fn tier(t: SizeTier) -> Self {
        let (vcpu_milli, memory_mib) = match t {
            SizeTier::Xs => (500, 1024),
            SizeTier::S => (1000, 4096),
            SizeTier::M => (2000, 8192),
            SizeTier::L => (4000, 16384),
            SizeTier::Xl => (8000, 32768),
        };
        Self {
            tier: t,
            vcpu_milli,
            memory_mib,
            replicas: 1,
        }
    }
}

/// What a user asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceSpec {
    /// DNS label, unique per namespace.
    pub name: String,
    /// The kind.
    pub kind: EngineKind,
    /// `None`: the adapter's default.
    pub version: Option<String>,
    /// The size.
    pub size: EngineSize,
    /// Engine knobs (Spark: `backend` = `sail` | `spark`).
    pub config: BTreeMap<String, String>,
    /// Labels.
    pub labels: BTreeMap<String, String>,
}

/// One instance, as the facade hands it to an adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    /// `eng_<26 base32>`.
    pub id: String,
    /// The Loams namespace (R2).
    pub namespace: String,
    /// The spec.
    pub spec: InstanceSpec,
    /// `engines/<ns>/<id>/` (R2).
    pub state_prefix: String,
}

/// One phase machine for all engines (D2012).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
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

/// Health.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// One health check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    /// Its name.
    pub name: String,
    /// Its state.
    pub state: HealthState,
    /// Why.
    pub message: Option<String>,
}

/// Endpoint protocols.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum EndpointProtocol {
    Postgres,
    FlinkSqlGateway,
    SparkConnect,
    Http,
    WebUi,
    Kafka,
}

/// An endpoint: the gateway's address, never a pod's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// Its name.
    pub name: String,
    /// Its protocol.
    pub protocol: EndpointProtocol,
    /// The URL clients use.
    pub url: String,
    /// Inside the cluster only.
    pub internal: bool,
}

/// What the adapter observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The phase.
    pub phase: Phase,
    /// The health.
    pub health: HealthState,
    /// The checks.
    pub checks: Vec<HealthCheck>,
    /// The running version.
    pub version: Option<String>,
    /// A message.
    pub message: Option<String>,
    /// The endpoints.
    pub endpoints: Vec<Endpoint>,
}

/// A metering sample (R17: integers, `estimated` when not measured).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageSample {
    /// CPU, milliseconds.
    pub cpu_ms: u64,
    /// Memory, MiB × milliseconds.
    pub memory_mib_ms: u64,
    /// From the reserved size, not a measurement.
    pub estimated: bool,
}

/// What a kind supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Versions.
    pub versions: Vec<String>,
    /// The default.
    pub default_version: String,
    /// Sessions (Spark Connect).
    pub sessions: bool,
    /// Batch jobs (SP-b).
    pub jobs: bool,
    /// Scale to zero keeping state.
    pub suspend: bool,
}

/// Adapter errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[allow(missing_docs)]
pub enum EngineError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("internal: {0}")]
    Internal(String),
}

/// ENG-UI's `ManagedEngine` (method set as posted; `&Instance` per R1).
#[async_trait]
pub trait ManagedEngine: Send + Sync + 'static {
    /// The kind.
    fn kind(&self) -> EngineKind;
    /// What it supports.
    fn capabilities(&self) -> Capabilities;
    /// Rejects a bad spec before anything is stored.
    ///
    /// # Errors
    ///
    /// [`EngineError::Invalid`] or [`EngineError::Unsupported`].
    fn validate(&self, spec: &InstanceSpec) -> Result<(), EngineError>;
    /// Creates the runtime objects.
    async fn create(&self, inst: &Instance) -> Result<Observed, EngineError>;
    /// Changes the size.
    async fn scale(&self, inst: &Instance, to: &EngineSize) -> Result<Observed, EngineError>;
    /// Changes the version.
    async fn upgrade(&self, inst: &Instance, to_version: &str) -> Result<Observed, EngineError>;
    /// Scales to zero, keeping state.
    async fn suspend(&self, inst: &Instance) -> Result<Observed, EngineError>;
    /// Starts again.
    async fn resume(&self, inst: &Instance) -> Result<Observed, EngineError>;
    /// Removes everything (idempotent).
    async fn delete(&self, inst: &Instance) -> Result<(), EngineError>;
    /// Status, health and endpoints.
    async fn observe(&self, inst: &Instance) -> Result<Observed, EngineError>;
    /// Usage over a window of `window` length.
    async fn usage(&self, inst: &Instance, window: Duration) -> Result<UsageSample, EngineError>;
}

// ---- The runtime seam ----

/// A Deployment's readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Readiness {
    /// Desired replicas.
    pub desired: u32,
    /// Ready replicas.
    pub ready: u32,
}

/// What the adapter needs from Kubernetes.
#[async_trait]
pub trait ClusterApi: Send + Sync + 'static {
    /// Server-side applies the objects (create or update).
    async fn apply(&self, objects: Vec<Value>) -> Result<(), String>;
    /// Sets a Deployment's replicas.
    async fn scale(&self, namespace: &str, deployment: &str, replicas: u32) -> Result<(), String>;
    /// Deletes every object (and worker pod) with the label selector.
    async fn delete_selected(&self, namespace: &str, selector: &str) -> Result<(), String>;
    /// A Deployment's readiness; `None` when it does not exist.
    async fn readiness(
        &self,
        namespace: &str,
        deployment: &str,
    ) -> Result<Option<Readiness>, String>;
}

/// Where the adapter puts things.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterSettings {
    /// The image registry.
    pub registry: String,
    /// The Kubernetes namespace of a Loams namespace's engines.
    pub k8s_namespace_prefix: String,
    /// The region label in hostnames.
    pub region: String,
    /// The base domain (`loams.dev`).
    pub base_domain: String,
    /// Object storage.
    pub store: ObjectStore,
    /// The catalog.
    pub catalog: Catalog,
    /// The proxy's session idle timeout.
    pub proxy_idle_timeout_secs: u64,
    /// OTLP for Sail.
    pub otlp_endpoint: Option<String>,
}

/// Config keys a Spark instance accepts.
pub const CONFIG_KEYS: &[&str] = &["backend"];

/// The Spark adapter.
#[derive(Debug)]
pub struct SparkAdapter<C> {
    api: C,
    settings: AdapterSettings,
}

fn backend_of(spec: &InstanceSpec) -> Result<BackendKind, EngineError> {
    match spec.config.get("backend") {
        None => Ok(BackendKind::Sail),
        Some(b) => BackendKind::parse(b)
            .ok_or_else(|| EngineError::Invalid(format!("backend {b:?}: expected sail or spark"))),
    }
}

/// The server's memory per tier (R19's table for Spark/Sail).
pub fn driver_memory_mib(tier: SizeTier) -> u32 {
    match tier {
        SizeTier::Xs => 1024,
        SizeTier::S => 2048,
        SizeTier::M => 4096,
        SizeTier::L | SizeTier::Xl => 8192,
    }
}

impl<C: ClusterApi> SparkAdapter<C> {
    /// An adapter over a cluster.
    pub fn new(api: C, settings: AdapterSettings) -> Self {
        Self { api, settings }
    }

    /// The Kubernetes namespace of a Loams namespace.
    pub fn k8s_namespace(&self, inst: &Instance) -> String {
        format!("{}{}", self.settings.k8s_namespace_prefix, inst.namespace)
    }

    fn version_of(inst: &Instance, kind: BackendKind) -> String {
        inst.spec
            .version
            .clone()
            .unwrap_or_else(|| kind.backend().version().to_owned())
    }

    /// The cluster conf of an instance at a size and version.
    pub fn cluster_conf(
        &self,
        inst: &Instance,
        size: &EngineSize,
        version: &str,
    ) -> Result<(ClusterConf, &'static dyn SparkBackend), EngineError> {
        let kind = backend_of(&inst.spec)?;
        let backend = kind.backend();
        let name = k8s::object_name(&inst.id);
        let image = match kind {
            BackendKind::Sail => format!("{}/loams-sail:{version}", self.settings.registry),
            BackendKind::Spark => format!("{}/loams-spark:{version}", self.settings.registry),
        };
        let conf = ClusterConf {
            namespace: inst.namespace.clone(),
            instance_id: inst.id.clone(),
            state_prefix: inst.state_prefix.clone(),
            store: self.settings.store.clone(),
            catalog: self.settings.catalog.clone(),
            placement: Placement::Kubernetes(KubePlacement {
                namespace: self.k8s_namespace(inst),
                image,
                service_account: format!("{name}-spark"),
                driver_service: format!("{name}-driver"),
            }),
            shape: Shape {
                workers: size.replicas,
                worker_vcpu_milli: size.vcpu_milli,
                worker_memory_mib: size.memory_mib,
                driver_memory_mib: driver_memory_mib(size.tier),
            },
            proxy_idle_timeout_secs: self.settings.proxy_idle_timeout_secs,
            otlp_endpoint: self.settings.otlp_endpoint.clone(),
        };
        Ok((conf, backend))
    }

    async fn apply(
        &self,
        inst: &Instance,
        size: &EngineSize,
        version: &str,
        replicas: u8,
    ) -> Result<(), EngineError> {
        let (conf, backend) = self.cluster_conf(inst, size, version)?;
        let objects = k8s::render(
            backend,
            &conf,
            &KubeOptions {
                registry: self.settings.registry.clone(),
                replicas,
                server_vcpu_milli: size.vcpu_milli.min(2000),
            },
        )
        .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.api
            .apply(objects)
            .await
            .map_err(EngineError::Unavailable)
    }

    /// The public Spark Connect URL (R8).
    pub fn connect_url(&self, inst: &Instance) -> String {
        format!(
            "sc://{}.{}.{}.spark.{}:443/;use_ssl=true",
            inst.spec.name, inst.namespace, self.settings.region, self.settings.base_domain
        )
    }

    fn observed(&self, inst: &Instance, r: Option<Readiness>, version: String) -> Observed {
        let (phase, health, message) = match r {
            None => (
                Phase::Pending,
                HealthState::Unknown,
                Some("no server yet".into()),
            ),
            Some(r) if r.desired == 0 => (Phase::Suspended, HealthState::Unknown, None),
            Some(r) if r.ready >= r.desired => (Phase::Running, HealthState::Healthy, None),
            Some(_) => (
                Phase::Provisioning,
                HealthState::Degraded,
                Some("waiting for the Spark Connect server".into()),
            ),
        };
        Observed {
            phase,
            health,
            checks: vec![HealthCheck {
                name: "spark-connect".into(),
                state: health,
                message: message.clone(),
            }],
            version: Some(version),
            message,
            endpoints: vec![Endpoint {
                name: "spark-connect".into(),
                protocol: EndpointProtocol::SparkConnect,
                url: self.connect_url(inst),
                internal: false,
            }],
        }
    }

    async fn read(&self, inst: &Instance) -> Result<Option<Readiness>, EngineError> {
        self.api
            .readiness(&self.k8s_namespace(inst), &k8s::server_name(&inst.id))
            .await
            .map_err(EngineError::Unavailable)
    }
}

#[async_trait]
impl<C: ClusterApi> ManagedEngine for SparkAdapter<C> {
    fn kind(&self) -> EngineKind {
        EngineKind::Spark
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            versions: vec![pins::SAIL_VERSION.into(), pins::SPARK_VERSION.into()],
            default_version: pins::SAIL_VERSION.into(),
            sessions: true,
            jobs: true,
            suspend: true,
        }
    }

    fn validate(&self, spec: &InstanceSpec) -> Result<(), EngineError> {
        if spec.kind != EngineKind::Spark {
            return Err(EngineError::Invalid("not a Spark spec".into()));
        }
        if let Some(k) = spec
            .config
            .keys()
            .find(|k| !CONFIG_KEYS.contains(&k.as_str()))
        {
            return Err(EngineError::Invalid(format!(
                "unknown config key {k:?} (known: {CONFIG_KEYS:?})"
            )));
        }
        let kind = backend_of(spec)?;
        if let Some(v) = &spec.version
            && v != kind.backend().version()
        {
            return Err(EngineError::Unsupported(format!(
                "{kind:?} version {v} (supported: {})",
                kind.backend().version()
            )));
        }
        if spec.size.replicas == 0 || spec.size.replicas > 64 {
            return Err(EngineError::Invalid("replicas must be 1..=64".into()));
        }
        if spec.name.is_empty()
            || spec.name.len() > 40
            || !spec
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(EngineError::Invalid(
                "name must be a DNS label of 1-40 chars".into(),
            ));
        }
        Ok(())
    }

    async fn create(&self, inst: &Instance) -> Result<Observed, EngineError> {
        self.validate(&inst.spec)?;
        let kind = backend_of(&inst.spec)?;
        let version = Self::version_of(inst, kind);
        self.apply(inst, &inst.spec.size, &version, 1).await?;
        let r = self.read(inst).await?;
        let mut o = self.observed(inst, r, version);
        if o.phase == Phase::Pending {
            o.phase = Phase::Provisioning;
        }
        Ok(o)
    }

    async fn scale(&self, inst: &Instance, to: &EngineSize) -> Result<Observed, EngineError> {
        let kind = backend_of(&inst.spec)?;
        let version = Self::version_of(inst, kind);
        let running = self.read(inst).await?.is_some_and(|r| r.desired > 0);
        self.apply(inst, to, &version, u8::from(running)).await?;
        let mut o = self.observed(inst, self.read(inst).await?, version);
        if running && o.phase != Phase::Running {
            o.phase = Phase::Scaling;
        }
        Ok(o)
    }

    async fn upgrade(&self, inst: &Instance, to_version: &str) -> Result<Observed, EngineError> {
        let kind = backend_of(&inst.spec)?;
        if to_version != kind.backend().version() {
            return Err(EngineError::Unsupported(format!(
                "{kind:?} version {to_version}"
            )));
        }
        let running = self.read(inst).await?.is_some_and(|r| r.desired > 0);
        // A new server image restarts the server: open sessions end, which
        // the facade announces before calling this.
        self.apply(inst, &inst.spec.size, to_version, u8::from(running))
            .await?;
        let mut o = self.observed(inst, self.read(inst).await?, to_version.to_owned());
        if running && o.phase != Phase::Running {
            o.phase = Phase::Upgrading;
        }
        Ok(o)
    }

    async fn suspend(&self, inst: &Instance) -> Result<Observed, EngineError> {
        let ns = self.k8s_namespace(inst);
        self.api
            .scale(&ns, &k8s::server_name(&inst.id), 0)
            .await
            .map_err(EngineError::Unavailable)?;
        // Workers and executors belong to the server; remove stragglers.
        let selector = format!(
            "{}={},app.kubernetes.io/component in (worker,executor)",
            k8s::INSTANCE_LABEL,
            k8s::object_name(&inst.id)
        );
        self.api
            .delete_selected(&ns, &selector)
            .await
            .map_err(EngineError::Unavailable)?;
        let kind = backend_of(&inst.spec)?;
        let mut o = self.observed(inst, self.read(inst).await?, Self::version_of(inst, kind));
        o.phase = Phase::Suspended;
        Ok(o)
    }

    async fn resume(&self, inst: &Instance) -> Result<Observed, EngineError> {
        self.api
            .scale(&self.k8s_namespace(inst), &k8s::server_name(&inst.id), 1)
            .await
            .map_err(EngineError::Unavailable)?;
        let kind = backend_of(&inst.spec)?;
        let mut o = self.observed(inst, self.read(inst).await?, Self::version_of(inst, kind));
        if o.phase != Phase::Running {
            o.phase = Phase::Resuming;
        }
        Ok(o)
    }

    async fn delete(&self, inst: &Instance) -> Result<(), EngineError> {
        let selector = format!("{}={}", k8s::INSTANCE_LABEL, k8s::object_name(&inst.id));
        self.api
            .delete_selected(&self.k8s_namespace(inst), &selector)
            .await
            .map_err(EngineError::Unavailable)
    }

    async fn observe(&self, inst: &Instance) -> Result<Observed, EngineError> {
        let kind = backend_of(&inst.spec)?;
        Ok(self.observed(inst, self.read(inst).await?, Self::version_of(inst, kind)))
    }

    async fn usage(&self, inst: &Instance, window: Duration) -> Result<UsageSample, EngineError> {
        // Reserved size of the server while it runs (workers come and go
        // with load; measured usage from k8s metrics is a follow-up).
        let running = self.read(inst).await?.is_some_and(|r| r.desired > 0);
        if !running {
            return Ok(UsageSample {
                cpu_ms: 0,
                memory_mib_ms: 0,
                estimated: true,
            });
        }
        let ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
        let vcpu = u64::from(inst.spec.size.vcpu_milli.min(2000));
        let mem = u64::from(driver_memory_mib(inst.spec.size.tier));
        Ok(UsageSample {
            cpu_ms: vcpu.saturating_mul(ms) / 1000,
            memory_mib_ms: mem.saturating_mul(ms),
            estimated: true,
        })
    }
}

// ---- The proxy's router on Kubernetes ----

/// What the router needs to know about a namespace's instances (the
/// facade's store answers it).
#[async_trait]
pub trait InstanceDirectory: Send + Sync + 'static {
    /// The instance named `name` in `namespace`, or the namespace's only (or
    /// oldest) Spark instance when `name` is `None`.
    async fn find(&self, namespace: &str, name: Option<&str>) -> Result<Option<Instance>, String>;
    /// The instance with this id.
    async fn get(&self, instance_id: &str) -> Result<Option<Instance>, String>;
    /// Asks the facade to resume a suspended instance (it runs the phase
    /// machine and calls the adapter's `resume`).
    async fn request_resume(&self, instance: &Instance) -> Result<(), String>;
}

/// Resolves a call to an instance's in-cluster Service, resuming it first
/// when suspended.
pub struct InstanceRouter<C> {
    dir: Arc<dyn InstanceDirectory>,
    adapter: Arc<SparkAdapter<C>>,
    wake_timeout: Duration,
    tokens: Arc<dyn Fn(&Instance) -> Option<Arc<str>> + Send + Sync>,
}

impl<C> std::fmt::Debug for InstanceRouter<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceRouter")
            .field("wake_timeout", &self.wake_timeout)
            .finish_non_exhaustive()
    }
}

impl<C: ClusterApi> InstanceRouter<C> {
    /// A router. `tokens` returns an Apache Spark instance's pre-shared
    /// token (from its `-connect` Secret).
    pub fn new(
        dir: Arc<dyn InstanceDirectory>,
        adapter: Arc<SparkAdapter<C>>,
        wake_timeout: Duration,
        tokens: Arc<dyn Fn(&Instance) -> Option<Arc<str>> + Send + Sync>,
    ) -> Self {
        Self {
            dir,
            adapter,
            wake_timeout,
            tokens,
        }
    }

    fn backend(&self, inst: &Instance) -> Result<Backend, Status> {
        let kind = backend_of(&inst.spec).map_err(|e| Status::internal(e.to_string()))?;
        let uri = k8s::backend_uri(kind.backend(), &inst.id, &self.adapter.k8s_namespace(inst));
        Ok(Backend {
            instance_id: inst.id.clone(),
            channel: route::channel(&uri, Duration::from_secs(5)).map_err(Status::internal)?,
            token: if kind.backend().has_token_auth() {
                (self.tokens)(inst)
            } else {
                None
            },
        })
    }

    async fn wake(&self, inst: &Instance) -> Result<(), Status> {
        let ready = |r: Option<Readiness>| r.is_some_and(|r| r.desired > 0 && r.ready >= r.desired);
        let read = || async move {
            self.adapter
                .read(inst)
                .await
                .map_err(|e| Status::unavailable(e.to_string()))
        };
        if ready(read().await?) {
            return Ok(());
        }
        self.dir
            .request_resume(inst)
            .await
            .map_err(Status::unavailable)?;
        let deadline = tokio::time::Instant::now() + self.wake_timeout;
        loop {
            if ready(read().await?) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Status::unavailable(
                    "the Spark instance is starting; retry in a minute",
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

#[async_trait]
impl<C: ClusterApi> Router for InstanceRouter<C> {
    async fn resolve(&self, target: &Target) -> Result<Backend, Status> {
        let inst = self
            .dir
            .find(&target.namespace, target.instance.as_deref())
            .await
            .map_err(Status::unavailable)?
            .ok_or_else(|| Status::not_found("no such Spark instance in this namespace"))?;
        if inst.namespace != target.namespace {
            return Err(Status::permission_denied("instance of another namespace"));
        }
        self.wake(&inst).await?;
        self.backend(&inst)
    }

    async fn by_instance(&self, instance_id: &str) -> Option<Backend> {
        let inst = self.dir.get(instance_id).await.ok()??;
        self.backend(&inst).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeCluster {
        applied: Mutex<Vec<Value>>,
        replicas: Mutex<BTreeMap<String, u32>>,
        deleted: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ClusterApi for Arc<FakeCluster> {
        async fn apply(&self, objects: Vec<Value>) -> Result<(), String> {
            for o in &objects {
                if o["kind"] == "Deployment" {
                    let name = o["metadata"]["name"].as_str().unwrap().to_owned();
                    let n = u32::try_from(o["spec"]["replicas"].as_u64().unwrap()).unwrap();
                    self.replicas.lock().unwrap().insert(name, n);
                }
            }
            self.applied.lock().unwrap().extend(objects);
            Ok(())
        }
        async fn scale(&self, _: &str, d: &str, n: u32) -> Result<(), String> {
            self.replicas.lock().unwrap().insert(d.to_owned(), n);
            Ok(())
        }
        async fn delete_selected(&self, _: &str, sel: &str) -> Result<(), String> {
            self.deleted.lock().unwrap().push(sel.to_owned());
            Ok(())
        }
        async fn readiness(&self, _: &str, d: &str) -> Result<Option<Readiness>, String> {
            Ok(self.replicas.lock().unwrap().get(d).map(|&n| Readiness {
                desired: n,
                ready: n,
            }))
        }
    }

    fn settings() -> AdapterSettings {
        AdapterSettings {
            registry: "ghcr.io/ostrium-labs".into(),
            k8s_namespace_prefix: "loams-".into(),
            region: "eu1".into(),
            base_domain: "loams.dev".into(),
            store: ObjectStore::default(),
            catalog: Catalog::default(),
            proxy_idle_timeout_secs: 1800,
            otlp_endpoint: None,
        }
    }

    fn inst(backend: Option<&str>) -> Instance {
        let mut config = BTreeMap::new();
        if let Some(b) = backend {
            config.insert("backend".to_owned(), b.to_owned());
        }
        Instance {
            id: "eng_01jabc".into(),
            namespace: "ns-a".into(),
            spec: InstanceSpec {
                name: "etl".into(),
                kind: EngineKind::Spark,
                version: None,
                size: EngineSize::tier(SizeTier::M),
                config,
                labels: BTreeMap::new(),
            },
            state_prefix: "engines/ns-a/eng_01jabc/".into(),
        }
    }

    #[test]
    fn validate_rejects_bad_specs() {
        let a = SparkAdapter::new(Arc::new(FakeCluster::default()), settings());
        a.validate(&inst(None).spec).unwrap();
        a.validate(&inst(Some("spark")).spec).unwrap();
        assert!(matches!(
            a.validate(&inst(Some("hive")).spec),
            Err(EngineError::Invalid(_))
        ));
        let mut s = inst(None).spec;
        s.config.insert("spark.master".into(), "yarn".into());
        assert!(a.validate(&s).is_err());
        let mut s = inst(None).spec;
        s.version = Some("0.1.0".into());
        assert!(matches!(a.validate(&s), Err(EngineError::Unsupported(_))));
        let mut s = inst(None).spec;
        s.name = "Bad_Name".into();
        assert!(a.validate(&s).is_err());
    }

    #[tokio::test]
    async fn lifecycle_create_suspend_resume_delete() {
        let fake = Arc::new(FakeCluster::default());
        let a = SparkAdapter::new(Arc::clone(&fake), settings());
        let i = inst(None);
        let o = a.create(&i).await.unwrap();
        assert_eq!(o.phase, Phase::Running);
        assert_eq!(o.version.as_deref(), Some("0.7.2"));
        assert_eq!(
            o.endpoints[0].url,
            "sc://etl.ns-a.eu1.spark.loams.dev:443/;use_ssl=true"
        );
        assert!(
            fake.applied
                .lock()
                .unwrap()
                .iter()
                .any(|o| o["kind"] == "NetworkPolicy")
        );

        assert_eq!(a.suspend(&i).await.unwrap().phase, Phase::Suspended);
        assert_eq!(a.observe(&i).await.unwrap().phase, Phase::Suspended);
        assert_eq!(
            a.usage(&i, Duration::from_secs(60)).await.unwrap().cpu_ms,
            0
        );
        assert_eq!(a.resume(&i).await.unwrap().phase, Phase::Running);
        let u = a.usage(&i, Duration::from_secs(60)).await.unwrap();
        assert_eq!(u.cpu_ms, 120_000);
        assert_eq!(u.memory_mib_ms, 4096 * 60_000);
        assert!(u.estimated);

        // Scaling a suspended instance keeps it suspended.
        a.suspend(&i).await.unwrap();
        let o = a.scale(&i, &EngineSize::tier(SizeTier::L)).await.unwrap();
        assert_eq!(o.phase, Phase::Suspended);

        a.delete(&i).await.unwrap();
        assert!(
            fake.deleted
                .lock()
                .unwrap()
                .contains(&"loams.dev/instance=eng-01jabc".to_owned())
        );
    }

    #[tokio::test]
    async fn upgrade_only_to_the_pinned_version() {
        let a = SparkAdapter::new(Arc::new(FakeCluster::default()), settings());
        let i = inst(Some("spark"));
        a.create(&i).await.unwrap();
        assert!(a.upgrade(&i, "3.5.0").await.is_err());
        assert_eq!(
            a.upgrade(&i, "4.1.3").await.unwrap().version.as_deref(),
            Some("4.1.3")
        );
    }

    struct Dir {
        inst: Instance,
        cluster: Arc<FakeCluster>,
        resumes: Mutex<u32>,
    }

    #[async_trait]
    impl InstanceDirectory for Dir {
        async fn find(&self, ns: &str, name: Option<&str>) -> Result<Option<Instance>, String> {
            Ok(
                (ns == self.inst.namespace && name.is_none_or(|n| n == self.inst.spec.name))
                    .then(|| self.inst.clone()),
            )
        }
        async fn get(&self, id: &str) -> Result<Option<Instance>, String> {
            Ok((id == self.inst.id).then(|| self.inst.clone()))
        }
        async fn request_resume(&self, inst: &Instance) -> Result<(), String> {
            *self.resumes.lock().unwrap() += 1;
            self.cluster.scale("", &k8s::server_name(&inst.id), 1).await
        }
    }

    #[tokio::test]
    async fn router_resumes_a_suspended_instance_on_first_call() {
        let fake = Arc::new(FakeCluster::default());
        let adapter = Arc::new(SparkAdapter::new(Arc::clone(&fake), settings()));
        let i = inst(None);
        adapter.create(&i).await.unwrap();
        adapter.suspend(&i).await.unwrap();
        let dir = Arc::new(Dir {
            inst: i.clone(),
            cluster: Arc::clone(&fake),
            resumes: Mutex::new(0),
        });
        let router = InstanceRouter::new(
            Arc::clone(&dir) as Arc<dyn InstanceDirectory>,
            adapter,
            Duration::from_secs(5),
            Arc::new(|_: &Instance| None),
        );
        let t = |ns: &str, name: Option<&str>| Target {
            namespace: ns.into(),
            instance: name.map(Into::into),
        };
        let b = router.resolve(&t("ns-a", None)).await.unwrap();
        assert_eq!(b.instance_id, "eng_01jabc");
        assert_eq!(*dir.resumes.lock().unwrap(), 1);
        // Running now: no second resume.
        router.resolve(&t("ns-a", Some("etl"))).await.unwrap();
        assert_eq!(*dir.resumes.lock().unwrap(), 1);
        assert_eq!(
            router.resolve(&t("ns-b", None)).await.unwrap_err().code(),
            tonic::Code::NotFound
        );
        assert!(router.by_instance("eng_01jabc").await.is_some());
    }
}
