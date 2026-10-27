//! The lifecycle service over fake adapters: phases, validation, conflicts and metering.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use loams_engines::meter::{
    GateOp, MemoryMeterSink, MeterGate, MeterOrigin, RUNTIME_TYPE, STORAGE_TYPE,
};
use loams_engines::testing::{FAKE_STATE_BYTES, FakeEngine};
use loams_engines::{
    CreateInstance, CreateSession, EngineError, EngineKind, EngineRegistry, EngineService,
    Instance, Phase, SizeRequest, SizeTier, SparkJobKind, SparkJobState, SparkSessionKind,
    SqlRequest, SubmitJob, UpdateInstance,
};
use time::{Duration, OffsetDateTime};

struct World {
    svc: EngineService,
    rw: Arc<FakeEngine>,
    meter: Arc<MemoryMeterSink>,
}

fn world() -> World {
    let rw = Arc::new(FakeEngine::new(EngineKind::Risingwave));
    let meter = Arc::new(MemoryMeterSink::new());
    let registry = EngineRegistry::new()
        .with(rw.clone())
        .with(Arc::new(
            FakeEngine::new(EngineKind::Flink).without_suspend(),
        ))
        .with(Arc::new(FakeEngine::new(EngineKind::Spark)));
    let svc = EngineService::new(registry).with_meter(
        meter.clone(),
        MeterOrigin {
            cluster: "c1".into(),
            node: "n1".into(),
        },
    );
    World { svc, rw, meter }
}

fn req(name: &str, kind: EngineKind) -> CreateInstance {
    CreateInstance {
        name: name.into(),
        kind,
        version: None,
        size: SizeRequest {
            tier: SizeTier::S,
            replicas: Some(2),
        },
        config: BTreeMap::new(),
        labels: BTreeMap::new(),
    }
}

#[tokio::test]
async fn create_runs_and_lists_by_namespace_and_kind() {
    let w = world();
    let rw = w
        .svc
        .create(
            "p1",
            Some("acme".into()),
            req("orders", EngineKind::Risingwave),
        )
        .await
        .unwrap();
    assert_eq!(rw.status.phase, Phase::Running);
    assert_eq!(rw.status.version.as_deref(), Some("2.6"));
    assert_eq!(rw.state_prefix, format!("engines/p1/{}/", rw.id));
    assert!(!rw.endpoints.is_empty());
    w.svc
        .create("p1", None, req("sessions", EngineKind::Spark))
        .await
        .unwrap();
    w.svc
        .create("p2", None, req("orders", EngineKind::Flink))
        .await
        .unwrap();

    assert_eq!(w.svc.list("p1", None).await.unwrap().len(), 2);
    let spark = w.svc.list("p1", Some(EngineKind::Spark)).await.unwrap();
    assert_eq!(spark.len(), 1);
    assert_eq!(spark[0].spec.name, "sessions");
    // Another namespace's instance is not found, not forbidden.
    assert!(matches!(
        w.svc.get("p2", rw.id.as_str()).await,
        Err(EngineError::NotFound(_))
    ));
}

#[tokio::test]
async fn create_refuses_bad_specs() {
    let w = world();
    let bad_name = w
        .svc
        .create("p1", None, req("Orders", EngineKind::Risingwave))
        .await;
    assert!(matches!(bad_name, Err(EngineError::Invalid(_))));

    let mut bad_version = req("a", EngineKind::Risingwave);
    bad_version.version = Some("0.1".into());
    assert!(matches!(
        w.svc.create("p1", None, bad_version).await,
        Err(EngineError::Invalid(_))
    ));

    let mut bad_config = req("b", EngineKind::Risingwave);
    bad_config.config.insert("nope".into(), "1".into());
    assert!(matches!(
        w.svc.create("p1", None, bad_config).await,
        Err(EngineError::Invalid(_))
    ));

    w.svc
        .create("p1", None, req("dup", EngineKind::Risingwave))
        .await
        .unwrap();
    assert!(matches!(
        w.svc
            .create("p1", None, req("dup", EngineKind::Flink))
            .await,
        Err(EngineError::Conflict(_))
    ));
    assert!(w.svc.list("p1", None).await.unwrap().len() == 1);
}

#[tokio::test]
async fn a_kind_without_an_adapter_is_not_enabled() {
    let svc = EngineService::new(
        EngineRegistry::new().with(Arc::new(FakeEngine::new(EngineKind::Spark))),
    );
    let e = svc
        .create("p1", None, req("rw", EngineKind::Risingwave))
        .await
        .unwrap_err();
    assert!(matches!(e, EngineError::NotEnabled(_)));
    assert_eq!(e.status().as_u16(), 501);
    let catalog = svc.catalog();
    assert_eq!(catalog.engines.len(), 3);
    assert!(
        catalog
            .engines
            .iter()
            .all(|e| e.enabled == (e.kind == EngineKind::Spark))
    );
    assert_eq!(catalog.sizes.len(), 5);
}

#[tokio::test]
async fn runtime_failure_on_create_keeps_a_failed_record() {
    let w = world();
    w.rw.fail_next(EngineError::Unavailable("runtime down".into()));
    let inst = w
        .svc
        .create("p1", None, req("rw", EngineKind::Risingwave))
        .await
        .unwrap();
    assert_eq!(inst.status.phase, Phase::Failed);
    assert_eq!(inst.status.message.as_deref(), Some("runtime down"));
    // A failed instance can be repaired by a scale, or deleted.
    let fixed = w
        .svc
        .scale(
            "p1",
            inst.id.as_str(),
            SizeRequest {
                tier: SizeTier::M,
                replicas: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(fixed.status.phase, Phase::Running);
    assert_eq!(fixed.spec.size.tier, SizeTier::M);
}

#[tokio::test]
async fn invalid_on_create_leaves_nothing() {
    let w = world();
    w.rw.fail_next(EngineError::Invalid("no".into()));
    assert!(
        w.svc
            .create("p1", None, req("rw", EngineKind::Risingwave))
            .await
            .is_err()
    );
    assert!(w.svc.list("p1", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn lifecycle_follows_the_phase_machine() {
    let w = world();
    let inst = w
        .svc
        .create("p1", None, req("rw", EngineKind::Risingwave))
        .await
        .unwrap();
    let id = inst.id.as_str();

    let s = w.svc.suspend("p1", id).await.unwrap();
    assert_eq!(s.status.phase, Phase::Suspended);
    assert!(s.endpoints.is_empty());
    assert!(matches!(
        w.svc.suspend("p1", id).await,
        Err(EngineError::FailedPrecondition(_))
    ));
    assert!(matches!(
        w.svc.sql("p1", id, SqlRequest::new("select 1")).await,
        Err(EngineError::FailedPrecondition(_))
    ));
    let r = w.svc.resume("p1", id).await.unwrap();
    assert_eq!(r.status.phase, Phase::Running);

    let up = w
        .svc
        .update(
            "p1",
            id,
            UpdateInstance {
                size: None,
                version: Some("2.5".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(up.spec.version.as_deref(), Some("2.5"));
    assert_eq!(up.status.version.as_deref(), Some("2.5"));
    assert!(matches!(
        w.svc
            .update(
                "p1",
                id,
                UpdateInstance {
                    size: None,
                    version: Some("9.9".into())
                }
            )
            .await,
        Err(EngineError::Invalid(_))
    ));
    // The refused upgrade put the phase back.
    assert_eq!(
        w.svc.get("p1", id).await.unwrap().status.phase,
        Phase::Running
    );
    assert!(matches!(
        w.svc.update("p1", id, UpdateInstance::default()).await,
        Err(EngineError::Invalid(_))
    ));

    let gone = w.svc.delete("p1", id).await.unwrap();
    assert_eq!(gone.status.phase, Phase::Deleted);
    assert!(matches!(
        w.svc.get("p1", id).await,
        Err(EngineError::NotFound(_))
    ));
    assert!(w.rw.calls().iter().any(|c| c.starts_with("delete ")));
}

#[tokio::test]
async fn a_failed_operation_restores_the_phase_with_the_error() {
    let w = world();
    let inst = w
        .svc
        .create("p1", None, req("rw", EngineKind::Risingwave))
        .await
        .unwrap();
    let id = inst.id.as_str();
    w.rw.fail_next(EngineError::Unavailable("busy cluster".into()));
    assert!(matches!(
        w.svc.suspend("p1", id).await,
        Err(EngineError::Unavailable(_))
    ));
    let now = w.svc.get("p1", id).await.unwrap();
    assert_eq!(now.status.phase, Phase::Running);
    assert_eq!(now.status.message.as_deref(), Some("busy cluster"));
}

#[tokio::test]
async fn suspend_needs_the_capability() {
    let w = world();
    let fl = w
        .svc
        .create("p1", None, req("fl", EngineKind::Flink))
        .await
        .unwrap();
    assert!(matches!(
        w.svc.suspend("p1", fl.id.as_str()).await,
        Err(EngineError::Unsupported(_))
    ));
}

#[tokio::test]
async fn sql_and_spark_surfaces_by_kind() {
    let w = world();
    let fl = w
        .svc
        .create("p1", None, req("fl", EngineKind::Flink))
        .await
        .unwrap();
    let sp = w
        .svc
        .create("p1", None, req("sp", EngineKind::Spark))
        .await
        .unwrap();

    let out = w
        .svc
        .sql(
            "p1",
            fl.id.as_str(),
            SqlRequest::new("INSERT INTO sink SELECT * FROM src"),
        )
        .await
        .unwrap();
    assert_eq!(out.row_count, 1);
    assert!(out.notice.unwrap().starts_with("job submitted"));
    assert!(matches!(
        w.svc
            .sql("p1", fl.id.as_str(), SqlRequest::new("select fail"))
            .await,
        Err(EngineError::Invalid(_))
    ));
    assert!(matches!(
        w.svc
            .sql("p1", fl.id.as_str(), SqlRequest::new("   "))
            .await,
        Err(EngineError::Invalid(_))
    ));
    assert!(matches!(
        w.svc.list_jobs("p1", fl.id.as_str()).await,
        Err(EngineError::Unsupported(_))
    ));

    let sid = sp.id.as_str();
    let session = w
        .svc
        .create_session(
            "p1",
            sid,
            CreateSession {
                kind: SparkSessionKind::Python,
                conf: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(w.svc.list_sessions("p1", sid).await.unwrap().len(), 1);
    w.svc.close_session("p1", sid, &session.id).await.unwrap();
    assert!(matches!(
        w.svc.close_session("p1", sid, &session.id).await,
        Err(EngineError::NotFound(_))
    ));

    let job = w
        .svc
        .submit_job(
            "p1",
            sid,
            SubmitJob {
                name: "nightly".into(),
                kind: SparkJobKind::Python,
                entrypoint: "s3://jobs/etl.py".into(),
                args: vec!["--day".into(), "2026-10-27".into()],
                conf: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(job.state, SparkJobState::Queued);
    let cancelled = w.svc.cancel_job("p1", sid, &job.id).await.unwrap();
    assert_eq!(cancelled.state, SparkJobState::Cancelled);
    assert!(matches!(
        w.svc.cancel_job("p1", sid, &job.id).await,
        Err(EngineError::FailedPrecondition(_))
    ));
    assert!(matches!(
        w.svc.sql("p1", sid, SqlRequest::new("select 1")).await,
        Err(EngineError::Unsupported(_))
    ));
}

#[tokio::test]
async fn metering_emits_runtime_per_window_and_storage_hourly() {
    let w = world();
    let inst = w
        .svc
        .create("p1", Some("acme".into()), req("rw", EngineKind::Risingwave))
        .await
        .unwrap();
    let t1 = inst.created_at + Duration::seconds(60);
    // The first round sends the runtime window and the first storage gauge.
    assert_eq!(w.svc.meter_tick(t1).await.unwrap(), 2);
    // A second round at the same instant has an empty window.
    assert_eq!(w.svc.meter_tick(t1).await.unwrap(), 0);
    let t2 = t1 + Duration::seconds(60);
    // Runtime only: the storage gauge is hourly.
    assert_eq!(w.svc.meter_tick(t2).await.unwrap(), 1);

    let events = w.meter.events();
    assert_eq!(events.len(), 3);
    let storage = &events[1];
    assert_eq!(storage.ty, STORAGE_TYPE);
    assert_eq!(storage.data["state_bytes"], FAKE_STATE_BYTES);
    assert_eq!(storage.data["namespace"], "p1");
    assert_eq!(storage.data["engine"], "risingwave");

    let e = &events[2];
    assert_eq!(e.ty, RUNTIME_TYPE);
    assert_eq!(e.ty, "io.loams.dev.usage.engine.runtime.v1");
    assert_eq!(e.subject, "org_acme");
    assert_eq!(e.tenantid, "acme/p1");
    assert_eq!(e.source, "loams://c1/engines/n1");
    assert_eq!(e.id, format!("{}/{}", inst.id, t2.unix_timestamp()));
    assert_eq!(e.dataschema, format!("urn:loams:json:{RUNTIME_TYPE}"));
    // s × 2 replicas: 2 vCPU and 8 GiB for 60 s, as integers.
    assert_eq!(e.data["cpu_ms"], 120_000);
    assert_eq!(e.data["memory_mib_ms"], 8192_u64 * 60_000);
    assert_eq!(e.data["size"], "s");
    assert_eq!(e.data["estimated"], true);
    assert_eq!(e.data["instance"], inst.id.as_str());

    // Two hours on, the gauge goes out again.
    let t3 = t2 + Duration::hours(2);
    assert_eq!(w.svc.meter_tick(t3).await.unwrap(), 2);

    // Windows are contiguous.
    let usage = w
        .svc
        .usage("p1", inst.id.as_str(), None, None)
        .await
        .unwrap();
    assert_eq!(usage.samples.len(), 3);
    assert_eq!(usage.samples[0].window_end, usage.samples[1].window_start);
    assert_eq!(usage.samples[1].window_end, usage.samples[2].window_start);
    assert_eq!(usage.totals.state_bytes, FAKE_STATE_BYTES);
    let later = w
        .svc
        .usage("p1", inst.id.as_str(), Some(t2), None)
        .await
        .unwrap();
    assert_eq!(later.samples.len(), 1);
}

#[tokio::test]
async fn a_suspended_instance_meters_storage_only() {
    let w = world();
    let inst = w
        .svc
        .create("p1", None, req("rw", EngineKind::Risingwave))
        .await
        .unwrap();
    let id = inst.id.as_str();
    w.svc.suspend("p1", id).await.unwrap();
    let before = w.meter.events().len();
    let later = OffsetDateTime::now_utc() + Duration::hours(2);
    assert_eq!(w.svc.meter_tick(later).await.unwrap(), 1);
    let events = w.meter.events();
    assert_eq!(events.len(), before + 1);
    let e = events.last().unwrap();
    assert_eq!(e.ty, STORAGE_TYPE);
    assert_eq!(e.subject, "org_default");
    assert_eq!(e.data["state_bytes"], FAKE_STATE_BYTES);
    let usage = w.svc.usage("p1", id, None, None).await.unwrap();
    let last = usage.samples.last().unwrap();
    assert_eq!((last.cpu_ms, last.memory_mib_ms), (0, 0));
}

#[tokio::test]
async fn delete_meters_the_last_window() {
    let w = world();
    let inst = w
        .svc
        .create("p1", None, req("rw", EngineKind::Risingwave))
        .await
        .unwrap();
    // Let the clock move past created_at so the final window is not empty.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    w.svc.delete("p1", inst.id.as_str()).await.unwrap();
    let events = w.meter.events();
    assert!(
        events
            .iter()
            .any(|e| e.ty == RUNTIME_TYPE && e.id.starts_with(inst.id.as_str()))
    );
}

/// Refuses every resume and scale up; admits creates of xs only.
struct Gate;

#[async_trait::async_trait]
impl MeterGate for Gate {
    async fn admit(&self, inst: &Instance, op: GateOp) -> Result<(), EngineError> {
        match op {
            GateOp::Create if inst.spec.size.tier == SizeTier::Xs => Ok(()),
            _ => Err(EngineError::QuotaExceeded(format!(
                "engine compute is used up ({op:?})"
            ))),
        }
    }
}

#[tokio::test]
async fn the_meter_gate_guards_create_resume_and_scale_up() {
    let registry = EngineRegistry::new().with(Arc::new(FakeEngine::new(EngineKind::Risingwave)));
    let svc = EngineService::new(registry).with_gate(Arc::new(Gate));
    let refused = svc
        .create("p1", None, req("big", EngineKind::Risingwave))
        .await
        .unwrap_err();
    assert!(matches!(refused, EngineError::QuotaExceeded(_)));
    assert_eq!(refused.status().as_u16(), 402);
    assert!(svc.list("p1", None).await.unwrap().is_empty());

    let mut small = req("small", EngineKind::Risingwave);
    small.size = SizeRequest {
        tier: SizeTier::Xs,
        replicas: None,
    };
    let inst = svc.create("p1", None, small).await.unwrap();
    let id = inst.id.as_str();
    let up = SizeRequest {
        tier: SizeTier::M,
        replicas: None,
    };
    assert!(matches!(
        svc.scale("p1", id, up).await,
        Err(EngineError::QuotaExceeded(_))
    ));
    assert_eq!(
        svc.get("p1", id).await.unwrap().status.phase,
        Phase::Running
    );

    // At a hard limit the Meter suspends, never deletes; resume is then refused.
    let held = svc
        .suspend_for_limit("p1", id, "monthly allowance")
        .await
        .unwrap();
    assert_eq!(held.status.phase, Phase::Suspended);
    assert!(held.status.message.unwrap().contains("monthly allowance"));
    assert!(matches!(
        svc.resume("p1", id).await,
        Err(EngineError::QuotaExceeded(_))
    ));
    assert_eq!(
        svc.get("p1", id).await.unwrap().status.phase,
        Phase::Suspended
    );
}
