//! The RisingWave adapter: create, scale, upgrade, suspend, resume, delete
//! and observe one managed instance. Stateless towards Loams (ENG81 R1,
//! D2010): ENG-UI's `loams-engines` owns the instance record, the phase
//! machine and metering, and calls these operations with the instance; the
//! adapter provisions the stores, renders the cluster, hands it to a
//! [`Runtime`] and probes until it is ready. The `loams_engines::ManagedEngine`
//! impl over this type lands with that crate (`src/engines.rs`).
//!
//! Every operation is idempotent, so the facade may retry one that failed
//! half way.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backend::ObjectStore;
use crate::bootstrap::{ServiceUsers, ensure_service_users};
use crate::credentials::{CredentialStore, Credentials};
use crate::gateway::{GatewayNaming, RwEndpoint, endpoint};
use crate::model::{ClusterSpec, EngineVersion, InstanceRef, Resources, Role, ports};
use crate::probe::{Health, ProbeTarget, Prober};
use crate::provision::{MetaDbProvisioner, ObjectStoreProvisioner};
use crate::render::{HostStyle, ImagePolicy, RenderInput, RenderedCluster, render};
use crate::runtime::Runtime;

/// How workloads are named and found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Compose,
    Kubernetes {
        /// `eng-{key}` (ENG81 R24).
        namespace_template: String,
        cluster_domain: String,
    },
}

impl Placement {
    pub fn hosts(&self, inst: &InstanceRef) -> HostStyle {
        match self {
            Placement::Compose => HostStyle::Compose,
            Placement::Kubernetes {
                namespace_template,
                cluster_domain,
            } => HostStyle::Kubernetes {
                namespace: namespace_template.replace("{key}", inst.key.as_str()),
                cluster_domain: cluster_domain.clone(),
            },
        }
    }
}

/// The adapter's settings.
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    pub images: ImagePolicy,
    pub placement: Placement,
    /// `None`: no gateway route.
    pub gateway: Option<GatewayNaming>,
    /// How long an operation waits for the cluster to become ready.
    pub ready_timeout: Duration,
    pub poll_interval: Duration,
    /// Each probe's own timeout.
    pub probe_timeout: Duration,
    /// Set the service users' passwords once the cluster is first ready
    /// (off only in tests without a real frontend).
    pub bootstrap_users: bool,
}

impl Default for AdapterConfig {
    fn default() -> Self {
        Self {
            images: ImagePolicy::default(),
            placement: Placement::Compose,
            gateway: None,
            ready_timeout: Duration::from_secs(600),
            poll_interval: Duration::from_secs(5),
            probe_timeout: Duration::from_secs(5),
            bootstrap_users: true,
        }
    }
}

/// The seams the adapter drives.
#[derive(Clone)]
pub struct Seams {
    pub meta_db: Arc<dyn MetaDbProvisioner>,
    pub object_store: Arc<dyn ObjectStoreProvisioner>,
    pub runtime: Arc<dyn Runtime>,
    pub prober: Arc<dyn Prober>,
    pub credentials: Arc<dyn CredentialStore>,
}

impl std::fmt::Debug for Seams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seams").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// The spec or the requested change is refused (maps to `Invalid`).
    #[error("{0}")]
    Invalid(String),
    /// A dependency could not be reached; retry (maps to `Unavailable`).
    #[error("{0}")]
    Unavailable(String),
    /// The cluster did not become ready in time (maps to `Unavailable`).
    #[error("the cluster did not become ready: {0}")]
    NotReady(String),
    #[error("{0}")]
    Internal(String),
}

/// The phases the adapter reports, named as ENG-UI's `Phase` (D2012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedPhase {
    Provisioning,
    Running,
    Suspended,
    Deleted,
    Failed,
}

/// The protocol of an endpoint, as ENG-UI's `EndpointProtocol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointProtocol {
    Postgres,
    WebUi,
}

/// An address of the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterEndpoint {
    pub name: String,
    pub protocol: EndpointProtocol,
    pub url: String,
    /// Reachable only inside the instance's network (never shown to users
    /// as a way in; loams-gateway's address is the one that is not).
    pub internal: bool,
}

/// What the adapter saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub phase: ObservedPhase,
    pub health: Option<Health>,
    pub version: EngineVersion,
    pub message: Option<String>,
    pub endpoints: Vec<AdapterEndpoint>,
}

/// Reserved resources over a window, for metering when no measurement is
/// available (ENG81 R17: `estimated = true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservedUsage {
    pub cpu_ms: u64,
    pub memory_mib_ms: u64,
}

/// The reserved usage of `spec` over `window` (zero when suspended).
pub fn reserved_usage(spec: &ClusterSpec, window: Duration, suspended: bool) -> ReservedUsage {
    if suspended {
        return ReservedUsage {
            cpu_ms: 0,
            memory_mib_ms: 0,
        };
    }
    let totals = spec
        .topology()
        .map(|t| t.totals())
        .unwrap_or(Resources::new(0, 0));
    let ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    ReservedUsage {
        // cpu_milli × ms / 1000 = vCPU-milliseconds.
        cpu_ms: u64::from(totals.cpu_milli).saturating_mul(ms) / 1000,
        memory_mib_ms: (totals.memory_bytes >> 20).saturating_mul(ms),
    }
}

/// The steps of a rolling upgrade (D1955): after step `i`, the first `i + 1`
/// roles of [`Role::ROLLOUT`] run `to`. Roles already at `to` (a retried
/// upgrade) are skipped.
pub fn upgrade_steps(
    done: &BTreeMap<Role, EngineVersion>,
    to: EngineVersion,
) -> Vec<BTreeMap<Role, EngineVersion>> {
    let mut versions = done.clone();
    let mut steps = Vec::new();
    for role in Role::ROLLOUT {
        if versions.get(&role) == Some(&to) {
            continue;
        }
        versions.insert(role, to);
        steps.push(versions.clone());
    }
    steps
}

/// Managed RisingWave.
#[derive(Debug, Clone)]
pub struct RisingWaveAdapter {
    config: AdapterConfig,
    seams: Seams,
}

fn unavailable(e: impl std::fmt::Display) -> AdapterError {
    AdapterError::Unavailable(e.to_string())
}

impl RisingWaveAdapter {
    pub fn new(config: AdapterConfig, seams: Seams) -> Self {
        Self { config, seams }
    }

    pub fn config(&self) -> &AdapterConfig {
        &self.config
    }

    /// Refuses a cluster before anything runs: bad layout, a version
    /// without a pinned image.
    pub fn validate(&self, spec: &ClusterSpec) -> Result<(), AdapterError> {
        spec.topology()
            .map_err(|e| AdapterError::Invalid(e.to_string()))?;
        if !self.config.images.allows(spec.version) {
            return Err(AdapterError::Invalid(format!(
                "RisingWave {} has no pinned image; supported: {}",
                spec.version,
                self.supported_versions().join(", ")
            )));
        }
        Ok(())
    }

    /// The versions with a pinned image, oldest first.
    pub fn supported_versions(&self) -> Vec<String> {
        self.config.images.digests.keys().map(|v| v.tag()).collect()
    }

    /// The instance's frontend, `host:4566`.
    pub fn frontend_address(&self, inst: &InstanceRef) -> String {
        format!(
            "{}:{}",
            self.config
                .placement
                .hosts(inst)
                .service_host(&inst.key, Role::Frontend),
            ports::PGWIRE
        )
    }

    /// The instance's loams-gateway route.
    pub fn gateway_route(&self, inst: &InstanceRef) -> Option<RwEndpoint> {
        self.config
            .gateway
            .as_ref()
            .map(|naming| endpoint(inst, naming, &self.frontend_address(inst)))
    }

    pub fn endpoints(&self, inst: &InstanceRef) -> Vec<AdapterEndpoint> {
        let hosts = self.config.placement.hosts(inst);
        let mut out = Vec::new();
        if let Some(naming) = &self.config.gateway {
            out.push(AdapterEndpoint {
                name: "sql".into(),
                protocol: EndpointProtocol::Postgres,
                url: format!(
                    "postgresql://{}:{}/dev?sslmode=require",
                    naming.hostname(inst),
                    naming.port
                ),
                internal: false,
            });
        }
        out.push(AdapterEndpoint {
            name: "frontend".into(),
            protocol: EndpointProtocol::Postgres,
            url: format!("postgresql://{}/dev", self.frontend_address(inst)),
            internal: true,
        });
        out.push(AdapterEndpoint {
            name: "dashboard".into(),
            protocol: EndpointProtocol::WebUi,
            url: format!(
                "http://{}:{}/",
                hosts.replica_host(&inst.key, Role::Meta, 0),
                ports::DASHBOARD
            ),
            internal: true,
        });
        out
    }

    fn observation(
        &self,
        inst: &InstanceRef,
        phase: ObservedPhase,
        version: EngineVersion,
        health: Option<Health>,
    ) -> Observation {
        Observation {
            phase,
            health,
            version,
            message: None,
            endpoints: self.endpoints(inst),
        }
    }

    async fn credentials(&self, inst: &InstanceRef) -> Result<Credentials, AdapterError> {
        self.seams
            .credentials
            .load(&inst.key)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| {
                AdapterError::Internal(format!(
                    "no credentials for instance {}: it was never created",
                    inst.id
                ))
            })
    }

    fn render(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
        creds: &Credentials,
        role_versions: &BTreeMap<Role, EngineVersion>,
        suspended: bool,
    ) -> Result<RenderedCluster, AdapterError> {
        let hosts = self.config.placement.hosts(inst);
        render(&RenderInput {
            key: &inst.key,
            spec,
            meta_db: &creds.meta_db,
            store: &creds.store,
            images: &self.config.images,
            hosts: &hosts,
            role_versions,
            suspended,
        })
        .map_err(|e| AdapterError::Invalid(e.to_string()))
    }

    fn probe_target(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
        creds: &Credentials,
    ) -> ProbeTarget {
        let hosts = self.config.placement.hosts(inst);
        let mut t = ProbeTarget::new(
            hosts.service_host(&inst.key, Role::Frontend),
            hosts.replica_host(&inst.key, Role::Meta, 0),
        );
        if creds.users_set {
            t.password = Some(creds.users.root_password.clone());
        }
        if let Ok(topology) = spec.topology() {
            for (role, rs) in topology.iter() {
                t.expect.insert(role, rs.replicas);
            }
        }
        t.timeout = self.config.probe_timeout;
        t
    }

    async fn wait_ready(&self, target: &ProbeTarget) -> Result<Health, AdapterError> {
        let deadline = tokio::time::Instant::now() + self.config.ready_timeout;
        loop {
            let health = self.seams.prober.probe(target).await;
            if health.ready {
                return Ok(health);
            }
            if tokio::time::Instant::now() >= deadline {
                let failing: Vec<String> = health
                    .checks
                    .iter()
                    .filter(|c| !c.ok)
                    .map(|c| format!("{}: {}", c.name, c.detail))
                    .collect();
                return Err(AdapterError::NotReady(failing.join("; ")));
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    /// Applies a render and waits for readiness, then sets the service
    /// users if they are not set yet.
    async fn apply_running(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
        creds: &mut Credentials,
        role_versions: &BTreeMap<Role, EngineVersion>,
    ) -> Result<Health, AdapterError> {
        let cluster = self.render(inst, spec, creds, role_versions, false)?;
        self.seams
            .runtime
            .apply(&cluster)
            .await
            .map_err(unavailable)?;
        let health = self
            .wait_ready(&self.probe_target(inst, spec, creds))
            .await?;
        if self.config.bootstrap_users && !creds.users_set {
            let target = self.probe_target(inst, spec, creds);
            ensure_service_users(
                &target.frontend_host,
                target.frontend_port,
                &creds.users,
                self.config.probe_timeout,
            )
            .await
            .map_err(AdapterError::Unavailable)?;
            creds.users_set = true;
            self.seams
                .credentials
                .save(&inst.key, creds)
                .await
                .map_err(unavailable)?;
        }
        Ok(health)
    }

    /// Provisions the meta database and the state prefix, starts the
    /// cluster and waits until it is ready.
    pub async fn create(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
    ) -> Result<Observation, AdapterError> {
        self.validate(spec)?;
        let prefix = ObjectStore::prefix_from_state(&inst.state_prefix).ok_or_else(|| {
            AdapterError::Invalid(format!(
                "state prefix {:?} is not usable",
                inst.state_prefix
            ))
        })?;
        // A retried create keeps the users it may already have set.
        let existing = self
            .seams
            .credentials
            .load(&inst.key)
            .await
            .map_err(unavailable)?;
        let meta_db = self
            .seams
            .meta_db
            .ensure(&inst.key)
            .await
            .map_err(unavailable)?;
        let store = self
            .seams
            .object_store
            .ensure(&inst.key, &prefix)
            .await
            .map_err(unavailable)?;
        let (users, users_set) = match existing {
            Some(c) => (c.users, c.users_set),
            None => (ServiceUsers::generate(), false),
        };
        let mut creds = Credentials {
            meta_db,
            store,
            users,
            users_set,
        };
        self.seams
            .credentials
            .save(&inst.key, &creds)
            .await
            .map_err(unavailable)?;
        let health = self
            .apply_running(inst, spec, &mut creds, &BTreeMap::new())
            .await?;
        Ok(self.observation(inst, ObservedPhase::Running, spec.version, Some(health)))
    }

    /// Re-renders with `to` (new layout) and waits for readiness. A
    /// suspended instance only records the size, so nothing runs.
    pub async fn scale(
        &self,
        inst: &InstanceRef,
        to: &ClusterSpec,
        suspended: bool,
    ) -> Result<Observation, AdapterError> {
        self.validate(to)?;
        if suspended {
            return Ok(self.observation(inst, ObservedPhase::Suspended, to.version, None));
        }
        let mut creds = self.credentials(inst).await?;
        let health = self
            .apply_running(inst, to, &mut creds, &BTreeMap::new())
            .await?;
        Ok(self.observation(inst, ObservedPhase::Running, to.version, Some(health)))
    }

    /// A rolling upgrade from `spec.version` to `to`: meta, compactor,
    /// compute, frontend, each ready before the next (D1955). Forward only,
    /// at most one major. A suspended instance resumes on `to`.
    pub async fn upgrade(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
        to: EngineVersion,
        suspended: bool,
    ) -> Result<Observation, AdapterError> {
        spec.version
            .check_upgrade_to(to)
            .map_err(|e| AdapterError::Invalid(e.to_string()))?;
        let mut target = spec.clone();
        target.version = to;
        self.validate(&target)?;
        if suspended {
            return Ok(self.observation(inst, ObservedPhase::Suspended, to, None));
        }
        let mut creds = self.credentials(inst).await?;
        let mut health = None;
        for versions in upgrade_steps(&BTreeMap::new(), to) {
            health = Some(
                self.apply_running(inst, spec, &mut creds, &versions)
                    .await?,
            );
        }
        Ok(self.observation(inst, ObservedPhase::Running, to, health))
    }

    /// Every role to zero replicas; the meta database and the state prefix
    /// stay (D1954).
    pub async fn suspend(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
    ) -> Result<Observation, AdapterError> {
        let creds = self.credentials(inst).await?;
        let cluster = self.render(inst, spec, &creds, &BTreeMap::new(), true)?;
        self.seams
            .runtime
            .apply(&cluster)
            .await
            .map_err(unavailable)?;
        Ok(self.observation(inst, ObservedPhase::Suspended, spec.version, None))
    }

    /// Starts every role again and waits for readiness; RisingWave recovers
    /// from the meta store and Hummock.
    pub async fn resume(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
    ) -> Result<Observation, AdapterError> {
        self.validate(spec)?;
        let mut creds = self.credentials(inst).await?;
        let health = self
            .apply_running(inst, spec, &mut creds, &BTreeMap::new())
            .await?;
        Ok(self.observation(inst, ObservedPhase::Running, spec.version, Some(health)))
    }

    /// Removes the workloads, the state prefix, the object-store
    /// credentials, the meta database and the stored credentials.
    /// Idempotent: what is already gone is skipped.
    pub async fn delete(&self, inst: &InstanceRef) -> Result<(), AdapterError> {
        self.seams
            .runtime
            .remove(&inst.key)
            .await
            .map_err(unavailable)?;
        if let Some(prefix) = ObjectStore::prefix_from_state(&inst.state_prefix) {
            self.seams
                .object_store
                .purge(&inst.key, &prefix)
                .await
                .map_err(unavailable)?;
        }
        self.seams
            .object_store
            .revoke(&inst.key)
            .await
            .map_err(unavailable)?;
        self.seams
            .meta_db
            .drop_db(&inst.key)
            .await
            .map_err(unavailable)?;
        self.seams
            .credentials
            .delete(&inst.key)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    /// Probes the instance. `suspended` comes from the stored phase.
    pub async fn observe(
        &self,
        inst: &InstanceRef,
        spec: &ClusterSpec,
        suspended: bool,
    ) -> Result<Observation, AdapterError> {
        let Some(creds) = self
            .seams
            .credentials
            .load(&inst.key)
            .await
            .map_err(unavailable)?
        else {
            return Ok(self.observation(inst, ObservedPhase::Deleted, spec.version, None));
        };
        if suspended {
            return Ok(self.observation(inst, ObservedPhase::Suspended, spec.version, None));
        }
        let health = self
            .seams
            .prober
            .probe(&self.probe_target(inst, spec, &creds))
            .await;
        let phase = if health.ready {
            ObservedPhase::Running
        } else {
            ObservedPhase::Provisioning
        };
        let mut obs = self.observation(inst, phase, spec.version, Some(health));
        if phase != ObservedPhase::Running {
            obs.message = Some("not ready".into());
        }
        Ok(obs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryCredentialStore;
    use crate::model::{ComputeSize, Size};
    use crate::probe::FixedProber;
    use crate::provision::MemoryProvisioner;
    use crate::runtime::MemoryRuntime;

    const ID: &str = "eng_abcdefghijklmnopqrstuvwxyz";

    struct Harness {
        rw: RisingWaveAdapter,
        prov: MemoryProvisioner,
        rt: MemoryRuntime,
        creds: MemoryCredentialStore,
    }

    fn harness_with(prober: FixedProber, images: ImagePolicy) -> Harness {
        let prov = MemoryProvisioner::default();
        let rt = MemoryRuntime::default();
        let creds = MemoryCredentialStore::default();
        let config = AdapterConfig {
            images,
            gateway: Some(GatewayNaming {
                region: "eu1".into(),
                base_domain: "loams.dev".into(),
                port: 5432,
                secrets_dir: "/var/run/loams/risingwave".into(),
            }),
            ready_timeout: Duration::from_millis(30),
            poll_interval: Duration::from_millis(10),
            bootstrap_users: false,
            ..AdapterConfig::default()
        };
        let seams = Seams {
            meta_db: Arc::new(prov.clone()),
            object_store: Arc::new(prov.clone()),
            runtime: Arc::new(rt.clone()),
            prober: Arc::new(prober),
            credentials: Arc::new(creds.clone()),
        };
        Harness {
            rw: RisingWaveAdapter::new(config, seams),
            prov,
            rt,
            creds,
        }
    }

    fn harness() -> Harness {
        harness_with(FixedProber::ready(), ImagePolicy::default())
    }

    fn inst() -> InstanceRef {
        InstanceRef::new(ID, "clicks", "dev", &format!("engines/dev/{ID}/")).unwrap()
    }

    fn spec(tier: Size, replicas: u32) -> ClusterSpec {
        let (vcpu_milli, memory_mib) = match tier {
            Size::Xs => (500, 1024),
            Size::S => (1000, 4096),
            Size::M => (2000, 8192),
            Size::L => (4000, 16384),
            Size::Xl => (8000, 32768),
        };
        ClusterSpec::from_parts(
            ComputeSize {
                tier,
                replicas,
                vcpu_milli,
                memory_mib,
            },
            None,
            &BTreeMap::new(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn create_provisions_renders_and_runs() {
        let h = harness();
        let obs = h.rw.create(&inst(), &spec(Size::Xs, 1)).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Running);
        assert!(obs.health.unwrap().ready);
        assert_eq!(
            obs.endpoints[0].url,
            "postgresql://clicks.dev.eu1.rw.loams.dev:5432/dev?sslmode=require"
        );
        assert!(!obs.endpoints[0].internal);
        assert!(obs.endpoints[1..].iter().all(|e| e.internal));
        assert_eq!(
            *h.prov.log.lock().await,
            vec![
                format!("meta.ensure {}", inst().key),
                format!("store.ensure {}", inst().key)
            ]
        );
        let applied = h.rt.applied.lock().await;
        assert_eq!(applied.len(), 1);
        let meta = applied[0].role(Role::Meta);
        assert!(matches!(
            meta.env.get("RW_DATA_DIRECTORY"),
            Some(crate::render::EnvValue::Plain(d)) if *d == format!("engines/dev/{ID}/hummock")
        ));
        assert!(h.creds.load(&inst().key).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn create_is_idempotent_and_keeps_users() {
        let h = harness();
        h.rw.create(&inst(), &spec(Size::Xs, 1)).await.unwrap();
        let first = h.creds.load(&inst().key).await.unwrap().unwrap();
        h.rw.create(&inst(), &spec(Size::Xs, 1)).await.unwrap();
        let second = h.creds.load(&inst().key).await.unwrap().unwrap();
        assert_eq!(first.users, second.users);
    }

    #[tokio::test]
    async fn invalid_specs_are_refused_before_anything_runs() {
        let h = harness();
        let mut s = spec(Size::Xs, 1);
        s.version = EngineVersion::new(9, 9, 9);
        assert!(matches!(
            h.rw.create(&inst(), &s).await,
            Err(AdapterError::Invalid(_))
        ));
        let mut bad = inst();
        bad.state_prefix = "/".into();
        assert!(matches!(
            h.rw.create(&bad, &spec(Size::Xs, 1)).await,
            Err(AdapterError::Invalid(_))
        ));
        assert!(h.prov.log.lock().await.is_empty());
        assert!(h.rt.applied.lock().await.is_empty());
    }

    #[tokio::test]
    async fn scale_suspend_resume_delete() {
        let h = harness();
        let i = inst();
        h.rw.create(&i, &spec(Size::Xs, 1)).await.unwrap();

        let obs = h.rw.scale(&i, &spec(Size::M, 3), false).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Running);
        assert_eq!(
            h.rt.applied
                .lock()
                .await
                .last()
                .unwrap()
                .role(Role::Compute)
                .replicas,
            3
        );

        let obs = h.rw.suspend(&i, &spec(Size::M, 3)).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Suspended);
        assert!(
            h.rt.applied
                .lock()
                .await
                .last()
                .unwrap()
                .roles
                .iter()
                .all(|r| r.replicas == 0)
        );
        let obs = h.rw.observe(&i, &spec(Size::M, 3), true).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Suspended);

        // Scaling while suspended runs nothing.
        let applies = h.rt.applied.lock().await.len();
        let obs = h.rw.scale(&i, &spec(Size::L, 2), true).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Suspended);
        assert_eq!(h.rt.applied.lock().await.len(), applies);

        let obs = h.rw.resume(&i, &spec(Size::L, 2)).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Running);

        h.rw.delete(&i).await.unwrap();
        assert_eq!(*h.rt.removed.lock().await, vec![i.key.clone()]);
        let log = h.prov.log.lock().await.clone();
        assert!(log.ends_with(&[
            format!("store.purge {}", i.key),
            format!("store.revoke {}", i.key),
            format!("meta.drop {}", i.key),
        ]));
        assert!(h.creds.load(&i.key).await.unwrap().is_none());
        // Deleting again succeeds; observing a deleted instance says so.
        h.rw.delete(&i).await.unwrap();
        let obs = h.rw.observe(&i, &spec(Size::L, 2), false).await.unwrap();
        assert_eq!(obs.phase, ObservedPhase::Deleted);
    }

    #[test]
    fn upgrade_steps_go_role_by_role() {
        let to = EngineVersion::new(3, 1, 1);
        let steps = upgrade_steps(&BTreeMap::new(), to);
        assert_eq!(steps.len(), 4);
        for (i, step) in steps.iter().enumerate() {
            let roles: Vec<Role> = step.keys().copied().collect();
            assert_eq!(roles.len(), i + 1);
            for role in &Role::ROLLOUT[..=i] {
                assert_eq!(step[role], to);
            }
        }
        // A retry skips the roles already done.
        let mut done = BTreeMap::new();
        done.insert(Role::Meta, to);
        done.insert(Role::Compactor, to);
        assert_eq!(upgrade_steps(&done, to).len(), 2);
    }

    #[tokio::test]
    async fn rolling_upgrade_applies_each_role_in_order() {
        let mut images = ImagePolicy::default();
        images
            .digests
            .insert(EngineVersion::new(3, 1, 1), "sha256:11".into());
        let h = harness_with(FixedProber::ready(), images);
        let i = inst();
        let s = spec(Size::Xs, 1);
        h.rw.create(&i, &s).await.unwrap();
        let next = EngineVersion::new(3, 1, 1);
        let obs = h.rw.upgrade(&i, &s, next, false).await.unwrap();
        assert_eq!(obs.version, next);

        let applied = h.rt.applied.lock().await;
        assert_eq!(applied.len(), 5);
        for (step, cluster) in applied[1..].iter().enumerate() {
            for (j, role) in Role::ROLLOUT.iter().enumerate() {
                let want = if j <= step {
                    next
                } else {
                    EngineVersion::new(3, 1, 0)
                };
                assert_eq!(cluster.role(*role).version, want, "step {step}, {role}");
            }
        }
        drop(applied);

        // Downgrades and unpinned versions are refused.
        let mut upgraded = s.clone();
        upgraded.version = next;
        assert!(matches!(
            h.rw.upgrade(&i, &upgraded, EngineVersion::new(3, 1, 0), false)
                .await,
            Err(AdapterError::Invalid(_))
        ));
        assert!(matches!(
            h.rw.upgrade(&i, &upgraded, EngineVersion::new(3, 2, 0), false)
                .await,
            Err(AdapterError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn a_cluster_that_never_gets_ready_fails() {
        let h = harness_with(FixedProber::down(), ImagePolicy::default());
        let err = h.rw.create(&inst(), &spec(Size::Xs, 1)).await;
        let Err(AdapterError::NotReady(msg)) = err else {
            panic!("expected NotReady, got {err:?}");
        };
        assert!(msg.contains("pgwire"));
        let obs =
            h.rw.observe(&inst(), &spec(Size::Xs, 1), false)
                .await
                .unwrap();
        assert_eq!(obs.phase, ObservedPhase::Provisioning);
    }

    #[tokio::test]
    async fn runtime_and_provisioning_failures_are_unavailable() {
        let h = harness();
        *h.prov.fail.lock().await = Some("store.ensure".into());
        assert!(matches!(
            h.rw.create(&inst(), &spec(Size::Xs, 1)).await,
            Err(AdapterError::Unavailable(_))
        ));
        assert!(h.rt.applied.lock().await.is_empty());
        *h.prov.fail.lock().await = None;
        h.rw.create(&inst(), &spec(Size::Xs, 1)).await.unwrap();

        *h.rt.fail_apply.lock().await = true;
        assert!(matches!(
            h.rw.suspend(&inst(), &spec(Size::Xs, 1)).await,
            Err(AdapterError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn operations_on_a_never_created_instance() {
        let h = harness();
        assert!(matches!(
            h.rw.resume(&inst(), &spec(Size::Xs, 1)).await,
            Err(AdapterError::Internal(_))
        ));
    }

    #[test]
    fn gateway_route_and_usage() {
        let h = harness();
        let route = h.rw.gateway_route(&inst()).unwrap();
        assert_eq!(route.name, ID);
        assert_eq!(route.namespace, "dev");
        assert_eq!(
            route.upstreams,
            vec!["rw-abcdefghijklmnopqrstuvwxyz-frontend-0:4566".to_owned()]
        );
        let s = spec(Size::Xs, 1);
        let u = reserved_usage(&s, Duration::from_secs(60), false);
        // xs: four roles of 500m and 1 GiB.
        assert_eq!(u.cpu_ms, 2000 * 60);
        assert_eq!(u.memory_mib_ms, 4096 * 60_000);
        assert_eq!(
            reserved_usage(&s, Duration::from_secs(60), true),
            ReservedUsage {
                cpu_ms: 0,
                memory_mib_ms: 0
            }
        );
    }
}
