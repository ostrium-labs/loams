//! `loams_engines::ManagedEngine` for RisingWave (design 81, D2010; ENG81
//! R1). The facade in `loams-engines` owns the instance record, the phase
//! machine and metering; this maps an `Instance` to the adapter's
//! [`InstanceRef`] and [`ClusterSpec`] and the adapter's [`Observation`]
//! back to `Observed`.

use async_trait::async_trait;
use loams_engines::{
    Capabilities, Endpoint, EndpointProtocol, EngineError, EngineKind, EngineSize, HealthCheck,
    HealthState, Instance, InstanceSpec, ManagedEngine, Observed, Phase, SizeTier, SqlDialect,
    Stability, UsageSample,
};
use time::OffsetDateTime;

use crate::adapter::{
    AdapterError, EndpointProtocol as RwProtocol, Observation, ObservedPhase, RisingWaveAdapter,
    reserved_usage,
};
use crate::model::{ClusterSpec, ComputeSize, DEFAULT_VERSION, EngineVersion, InstanceRef, Size};
use crate::probe::Health;

/// Managed RisingWave as a `loams-engines` adapter.
#[derive(Debug, Clone)]
pub struct RisingWaveEngine {
    adapter: RisingWaveAdapter,
}

impl RisingWaveEngine {
    pub fn new(adapter: RisingWaveAdapter) -> Self {
        Self { adapter }
    }

    pub fn adapter(&self) -> &RisingWaveAdapter {
        &self.adapter
    }
}

/// The adapter's tier for an `EngineSize` tier (R19: one table).
pub fn tier(t: SizeTier) -> Size {
    match t {
        SizeTier::Xs => Size::Xs,
        SizeTier::S => Size::S,
        SizeTier::M => Size::M,
        SizeTier::L => Size::L,
        SizeTier::Xl => Size::Xl,
    }
}

/// The cluster an instance spec runs at `size`.
pub fn cluster_spec(
    spec: &InstanceSpec,
    size: &EngineSize,
    version: Option<&str>,
) -> Result<ClusterSpec, EngineError> {
    ClusterSpec::from_parts(
        ComputeSize {
            tier: tier(size.tier),
            replicas: u32::from(size.replicas),
            vcpu_milli: size.vcpu_milli,
            memory_mib: size.memory_mib,
        },
        version,
        &spec.config,
    )
    .map_err(|e| EngineError::Invalid(e.to_string()))
}

fn instance_ref(inst: &Instance) -> Result<InstanceRef, EngineError> {
    InstanceRef::new(
        inst.id.as_str(),
        &inst.spec.name,
        &inst.namespace,
        &inst.state_prefix,
    )
    .map_err(|e| EngineError::Invalid(e.to_string()))
}

fn current_spec(inst: &Instance) -> Result<ClusterSpec, EngineError> {
    cluster_spec(&inst.spec, &inst.spec.size, inst.spec.version.as_deref())
}

fn suspended(inst: &Instance) -> bool {
    inst.status.phase == Phase::Suspended
}

/// The facade's error for an adapter error.
pub fn engine_error(e: AdapterError) -> EngineError {
    match e {
        AdapterError::Invalid(m) => EngineError::Invalid(m),
        AdapterError::Unavailable(m) => EngineError::Unavailable(m),
        e @ AdapterError::NotReady(_) => EngineError::Unavailable(e.to_string()),
        AdapterError::Internal(m) => EngineError::Internal(m),
    }
}

fn health_state(h: Option<&Health>) -> HealthState {
    match h {
        None => HealthState::Unknown,
        Some(h) if h.ready => HealthState::Healthy,
        Some(h) if h.live => HealthState::Degraded,
        Some(_) => HealthState::Unhealthy,
    }
}

/// The facade's `Observed` for an adapter observation.
pub fn observed(obs: Observation) -> Observed {
    let phase = match obs.phase {
        ObservedPhase::Provisioning => Phase::Provisioning,
        ObservedPhase::Running => Phase::Running,
        ObservedPhase::Suspended => Phase::Suspended,
        ObservedPhase::Deleted => Phase::Deleted,
        ObservedPhase::Failed => Phase::Failed,
    };
    let checks = obs
        .health
        .as_ref()
        .map(|h| {
            h.checks
                .iter()
                .map(|c| HealthCheck {
                    name: c.name.clone(),
                    state: if c.ok {
                        HealthState::Healthy
                    } else {
                        HealthState::Unhealthy
                    },
                    message: (!c.detail.is_empty()).then(|| c.detail.clone()),
                })
                .collect()
        })
        .unwrap_or_default();
    Observed {
        phase,
        health: health_state(obs.health.as_ref()),
        checks,
        version: Some(obs.version.tag()),
        message: obs.message,
        endpoints: obs
            .endpoints
            .into_iter()
            .map(|e| Endpoint {
                name: e.name,
                protocol: match e.protocol {
                    RwProtocol::Postgres => EndpointProtocol::Postgres,
                    RwProtocol::WebUi => EndpointProtocol::WebUi,
                },
                url: e.url,
                internal: e.internal,
            })
            .collect(),
    }
}

#[async_trait]
impl ManagedEngine for RisingWaveEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Risingwave
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            versions: self.adapter.supported_versions(),
            default_version: DEFAULT_VERSION.tag(),
            sql_dialect: Some(SqlDialect::Postgres),
            sessions: false,
            jobs: false,
            suspend: true,
            stability: Stability::Preview,
        }
    }

    fn validate(&self, spec: &InstanceSpec) -> Result<(), EngineError> {
        let cluster = cluster_spec(spec, &spec.size, spec.version.as_deref())?;
        self.adapter.validate(&cluster).map_err(engine_error)
    }

    async fn create(&self, inst: &Instance) -> Result<Observed, EngineError> {
        let r = instance_ref(inst)?;
        let spec = current_spec(inst)?;
        self.adapter
            .create(&r, &spec)
            .await
            .map(observed)
            .map_err(engine_error)
    }

    async fn scale(&self, inst: &Instance, to: &EngineSize) -> Result<Observed, EngineError> {
        let r = instance_ref(inst)?;
        let spec = cluster_spec(&inst.spec, to, inst.spec.version.as_deref())?;
        self.adapter
            .scale(&r, &spec, suspended(inst))
            .await
            .map(observed)
            .map_err(engine_error)
    }

    async fn upgrade(&self, inst: &Instance, to_version: &str) -> Result<Observed, EngineError> {
        let r = instance_ref(inst)?;
        let spec = current_spec(inst)?;
        let to: EngineVersion = to_version
            .parse()
            .map_err(|e: crate::model::ModelError| EngineError::Invalid(e.to_string()))?;
        self.adapter
            .upgrade(&r, &spec, to, suspended(inst))
            .await
            .map(observed)
            .map_err(engine_error)
    }

    async fn suspend(&self, inst: &Instance) -> Result<Observed, EngineError> {
        let r = instance_ref(inst)?;
        let spec = current_spec(inst)?;
        self.adapter
            .suspend(&r, &spec)
            .await
            .map(observed)
            .map_err(engine_error)
    }

    async fn resume(&self, inst: &Instance) -> Result<Observed, EngineError> {
        let r = instance_ref(inst)?;
        let spec = current_spec(inst)?;
        self.adapter
            .resume(&r, &spec)
            .await
            .map(observed)
            .map_err(engine_error)
    }

    async fn delete(&self, inst: &Instance) -> Result<(), EngineError> {
        let r = instance_ref(inst)?;
        self.adapter.delete(&r).await.map_err(engine_error)
    }

    async fn observe(&self, inst: &Instance) -> Result<Observed, EngineError> {
        let r = instance_ref(inst)?;
        let spec = current_spec(inst)?;
        self.adapter
            .observe(&r, &spec, suspended(inst))
            .await
            .map(observed)
            .map_err(engine_error)
    }

    /// Reserved resources of every role (estimated, R17); suspended
    /// instances use none. State bytes are not measured yet (0).
    async fn usage(
        &self,
        inst: &Instance,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<UsageSample, EngineError> {
        let spec = current_spec(inst)?;
        let ms = u64::try_from((end - start).whole_milliseconds()).unwrap_or(0);
        let off = !inst.status.phase.has_compute();
        let u = reserved_usage(&spec, std::time::Duration::from_millis(ms), off);
        Ok(UsageSample {
            window_start: start,
            window_end: end,
            cpu_ms: u.cpu_ms,
            memory_mib_ms: u.memory_mib_ms,
            state_bytes: 0,
            estimated: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    use loams_engines::{InstanceId, InstanceStatus};

    use super::*;
    use crate::adapter::{AdapterConfig, Seams};
    use crate::credentials::MemoryCredentialStore;
    use crate::probe::FixedProber;
    use crate::provision::MemoryProvisioner;
    use crate::render::ImagePolicy;
    use crate::runtime::MemoryRuntime;

    const ID: &str = "eng_abcdefghijklmnopqrstuvwxyz";

    fn engine() -> RisingWaveEngine {
        let prov = MemoryProvisioner::default();
        let seams = Seams {
            meta_db: Arc::new(prov.clone()),
            object_store: Arc::new(prov),
            runtime: Arc::new(MemoryRuntime::default()),
            prober: Arc::new(FixedProber::ready()),
            credentials: Arc::new(MemoryCredentialStore::default()),
        };
        RisingWaveEngine::new(RisingWaveAdapter::new(
            AdapterConfig {
                images: ImagePolicy::default(),
                ready_timeout: Duration::from_millis(30),
                poll_interval: Duration::from_millis(10),
                bootstrap_users: false,
                ..AdapterConfig::default()
            },
            seams,
        ))
    }

    fn instance(phase: Phase) -> Instance {
        let now = OffsetDateTime::now_utc();
        let id = InstanceId(ID.into());
        Instance {
            state_prefix: Instance::state_prefix_for("dev", &id),
            id,
            namespace: "dev".into(),
            org: None,
            spec: InstanceSpec {
                name: "clicks".into(),
                kind: EngineKind::Risingwave,
                version: None,
                size: EngineSize::tier(SizeTier::Xs, 1),
                config: BTreeMap::new(),
                labels: BTreeMap::new(),
            },
            status: InstanceStatus::new(phase, now),
            endpoints: Vec::new(),
            stability: Stability::Preview,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn lifecycle_through_the_trait() {
        let e = engine();
        let inst = instance(Phase::Provisioning);
        e.validate(&inst.spec).unwrap();
        let o = e.create(&inst).await.unwrap();
        assert_eq!(o.phase, Phase::Running);
        assert_eq!(o.health, HealthState::Healthy);
        assert_eq!(o.version.as_deref(), Some("v3.1.0"));

        let running = instance(Phase::Running);
        let o = e
            .scale(&running, &EngineSize::tier(SizeTier::S, 2))
            .await
            .unwrap();
        assert_eq!(o.phase, Phase::Running);
        let o = e.suspend(&running).await.unwrap();
        assert_eq!(o.phase, Phase::Suspended);
        let suspended = instance(Phase::Suspended);
        let o = e
            .scale(&suspended, &EngineSize::tier(SizeTier::M, 1))
            .await
            .unwrap();
        assert_eq!(o.phase, Phase::Suspended);
        assert_eq!(e.observe(&suspended).await.unwrap().phase, Phase::Suspended);
        assert_eq!(e.resume(&suspended).await.unwrap().phase, Phase::Running);
        assert!(matches!(
            e.upgrade(&running, "v2.0.0").await,
            Err(EngineError::Invalid(_))
        ));
        assert!(matches!(
            e.upgrade(&running, "latest").await,
            Err(EngineError::Invalid(_))
        ));
        e.delete(&running).await.unwrap();
        e.delete(&running).await.unwrap();
        assert_eq!(e.observe(&running).await.unwrap().phase, Phase::Deleted);
    }

    #[test]
    fn validate_refuses_bad_config_and_versions() {
        let e = engine();
        let mut spec = instance(Phase::Pending).spec;
        spec.config.insert("compute.replicas".into(), "3".into());
        assert!(matches!(e.validate(&spec), Err(EngineError::Invalid(_))));
        let mut spec = instance(Phase::Pending).spec;
        spec.version = Some("v9.9.9".into());
        assert!(matches!(e.validate(&spec), Err(EngineError::Invalid(_))));
        let caps = e.capabilities();
        assert!(caps.suspend);
        assert_eq!(caps.versions, vec!["v3.1.0".to_owned()]);
        assert_eq!(caps.sql_dialect, Some(SqlDialect::Postgres));
    }

    #[tokio::test]
    async fn usage_is_reserved_and_zero_when_suspended() {
        let e = engine();
        let start = OffsetDateTime::now_utc();
        let end = start + time::Duration::seconds(60);
        let u = e
            .usage(&instance(Phase::Running), start, end)
            .await
            .unwrap();
        assert!(u.estimated);
        assert_eq!(u.cpu_ms, 2000 * 60);
        let u = e
            .usage(&instance(Phase::Suspended), start, end)
            .await
            .unwrap();
        assert_eq!((u.cpu_ms, u.memory_mib_ms), (0, 0));
    }

    #[test]
    fn errors_map() {
        assert!(matches!(
            engine_error(AdapterError::NotReady("x".into())),
            EngineError::Unavailable(_)
        ));
        assert!(matches!(
            engine_error(AdapterError::Internal("x".into())),
            EngineError::Internal(_)
        ));
    }
}
