//! `loams-fabric connect` (CC1 Task 1, D1821) over a stand-in engine on a
//! Unix socket: the round trip, kill and restart, pause, resume and rate,
//! the native-only rule, the single-writer lock, the admin API and the
//! scrubbed log.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{FakeEngine, admin, catalog, eventually, factories, instance};
use loams_connector_sdk::testing::reference::ReferenceConnector;
use loams_connector_sdk::{
    ConnectorContext, ConnectorError, ConnectorFactory, PipelineState, Scrubber, SecretRef,
    SinkFactory, SourceFactory, Split, SplitEnumerator, SplitState,
};
use loams_fabric::connect::Exit;
use loams_fabric::connect::instances::{self, code};
use loams_store::Store;
use serde_json::json;
use tempfile::TempDir;

fn source_yaml(name: &str, extra_config: &str) -> String {
    format!(
        "name: {name}\nconnector: reference\ndirection: source\nnamespace: w\nstream: orders\n\
         config:\n  splits: [a, b]\n{extra_config}"
    )
}

const SINK: &str = "name: orders-out\nconnector: reference\ndirection: sink\nnamespace: w\n\
                    stream: orders\nconfig: {}\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_roundtrip_memory() {
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    let store = Store::in_memory();
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    let src = instance(
        &source_yaml(
            "orders-in",
            "  events_per_split: 50\n  pipeline: {max_batch_rows: 7}\n",
        ),
        &cat,
        &fac,
    );
    let sink = instance(SINK, &cat, &fac);
    let role = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![src, sink],
    )
    .await;
    eventually("100 events in the stream", 10, || {
        engine.ids("w", "orders").len() == 100
    })
    .await;
    eventually("100 events in the sink", 10, || rc.sink.len() == 100).await;
    // Order per key (the split is the subject): ascending in the stream.
    for split in ["a", "b"] {
        let ns: Vec<u64> = engine
            .events("w", "orders")
            .into_iter()
            .flatten()
            .filter(|e| e["subject"] == split)
            .map(|e| e["data"]["n"].as_u64().unwrap())
            .collect();
        assert_eq!(ns, (0..50).collect::<Vec<_>>(), "split {split}");
    }
    let mut ids = rc.sink.ids();
    ids.sort();
    let mut want: Vec<String> = engine.ids("w", "orders");
    want.sort();
    assert_eq!(ids, want);
    assert_eq!(rc.sink.max_applied(), 1, "no event applied twice");
    // The status as the admin API reports it.
    let (status, body) = admin(
        role.admin.local_addr(),
        "GET",
        "/v1/connect/instances",
        Some("admin-token-for-tests"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let states: Vec<&str> = body["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["running", "running"], "{body}");
    assert!(matches!(role.run(async {}).await, Exit::Stopped));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_restart_no_loss_no_dup() {
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    let store = Store::in_memory();
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    let yaml = source_yaml(
        "orders-in",
        "  events_per_split: 400\n  payload_bytes: 256\n  rate_limit: {bytes_per_sec: 40000}\n  \
         pipeline: {max_batch_rows: 5, checkpoint_interval_ms: 1}\n",
    );
    let role = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![instance(&yaml, &cat, &fac)],
    )
    .await;
    eventually("some events", 10, || engine.ids("w", "orders").len() >= 40).await;
    // A crash: abort mid-batch, no drain, no final checkpoint.
    role.supervisor.abort_all().await;
    drop(role);
    let before = engine.ids("w", "orders").len();
    assert!(before < 800, "the crash came before the end ({before})");
    // A new process: it re-reads from the last checkpoint.
    let mut yaml2 = yaml.replace("rate_limit: {bytes_per_sec: 40000}", "");
    yaml2.push('\n');
    let role = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![instance(&yaml2, &cat, &fac)],
    )
    .await;
    eventually("all 800 events", 20, || {
        engine.ids("w", "orders").len() >= 800
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let ids = engine.ids("w", "orders");
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), 800, "no duplicate in the stream");
    assert_eq!(unique.len(), 800, "no loss");
    assert!(
        engine.duplicates() > 0,
        "the re-read after the crash was deduplicated by source + id"
    );
    assert!(role.epoch() > 1);
    role.run(async {}).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_resume_and_rate() {
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    let store = Store::in_memory();
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    let yaml = "name: orders-in\nconnector: reference\ndirection: source\nnamespace: w\n\
                stream: orders\nconfig:\n  splits: [a]\n  payload_bytes: 1024\n  \
                pipeline: {max_batch_rows: 4}\n";
    let role = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![instance(yaml, &cat, &fac)],
    )
    .await;
    let addr = role.admin.local_addr();
    let token = Some("admin-token-for-tests");
    eventually("events flowing", 10, || {
        engine.ids("w", "orders").len() > 100
    })
    .await;

    // Pause: `next` stops within one poll.
    let (s, body) = admin(
        addr,
        "POST",
        "/v1/connect/instances/orders-in:pause",
        token,
        None,
    )
    .await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["state"], "paused");
    assert_eq!(body["paused_by_operator"], true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let calls = rc.source.next_calls().len();
    let events = engine.ids("w", "orders").len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        rc.source.next_calls().len() <= calls + 1,
        "next was called while paused"
    );
    assert_eq!(engine.ids("w", "orders").len(), events);
    // The pause is kept in the bucket.
    let control = store
        .get("_meta/connect/instances/orders-in/control.json")
        .await
        .unwrap()
        .0;
    assert!(String::from_utf8_lossy(&control).contains("\"paused\":true"));

    // Rate: 64 KiB/s, then resume from the checkpoint.
    let (s, body) = admin(
        addr,
        "PATCH",
        "/v1/connect/instances/orders-in",
        token,
        Some(json!({"rate_limit": {"bytes_per_sec": 65536}})),
    )
    .await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["rate_limit"]["bytes_per_sec"], 65536.0);
    let (s, body) = admin(
        addr,
        "POST",
        "/v1/connect/instances/orders-in:resume",
        token,
        None,
    )
    .await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["state"], "running");
    // The SDK's size of one event, which the limiter charges (the stand-in
    // engine keeps the JSON data re-serialised, without the padding).
    let size = {
        use loams_connector_sdk::testing::reference::CounterSource;
        use loams_connector_sdk::{EnvelopeBuilder, InstanceKey, SplitId};
        let env = EnvelopeBuilder::new("reference", &InstanceKey::new(0, "orders-in"));
        CounterSource::event(&env, &SplitId::new("a"), 1000, 1024).size_bytes() as f64
    };
    tokio::time::sleep(Duration::from_secs(2)).await;
    let n0 = engine.ids("w", "orders").len();
    tokio::time::sleep(Duration::from_secs(10)).await;
    let n1 = engine.ids("w", "orders").len();
    let rate = (n1 - n0) as f64 * size / 10.0;
    assert!(
        (rate - 65536.0).abs() <= 65536.0 * 0.10,
        "{rate:.0} B/s, not within 10% of 64 KiB/s"
    );
    let ids = engine.ids("w", "orders");
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "resume continued, no duplicate");

    // The token is required; bad bodies are refused.
    let (s, _) = admin(addr, "GET", "/v1/connect/instances", None, None).await;
    assert_eq!(s, 401);
    let (s, _) = admin(addr, "GET", "/v1/connect/instances/nope", token, None).await;
    assert_eq!(s, 404);
    let (s, _) = admin(
        addr,
        "PATCH",
        "/v1/connect/instances/orders-in",
        token,
        Some(json!({"rate_limit": {"bytes_per_sec": -1}})),
    )
    .await;
    assert_eq!(s, 400);
    let (s, _) = admin(addr, "GET", "/health", None, None).await;
    assert_eq!(s, 200);
    role.run(async {}).await;
}

#[test]
fn refuses_fallback_runtime() {
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    // A connector whose runtime is Camel.
    let camel = "name: a\nconnector: a2a\ndirection: source\nnamespace: w\nstream: s\nconfig: {}\n";
    let e = instances::parse("a.yaml", camel, &cat, &fac).err().unwrap();
    assert_eq!(e.code, code::RUNTIME_NOT_SUPPORTED_ON_VM, "{e}");
    // An instance that opts into a fallback.
    let opted = "name: k\nconnector: kafka\nruntime: fallback\ndirection: source\nnamespace: w\n\
                 stream: s\nconfig: {}\n";
    let e = instances::parse("k.yaml", opted, &cat, &fac).err().unwrap();
    assert_eq!(e.code, code::RUNTIME_NOT_SUPPORTED_ON_VM, "{e}");
    // Native, but not in this build.
    let kafka = "name: k\nconnector: kafka\ndirection: source\nnamespace: w\nstream: s\n\
                 config: {brokers: 'b:9092', topics: [t], group_id_prefix: loams-x-}\n";
    let e = instances::parse("k.yaml", kafka, &cat, &fac).err().unwrap();
    assert_eq!(e.code, code::NOT_IN_THIS_BUILD, "{e}");
    let unknown = "name: x\nconnector: nope\ndirection: source\nnamespace: w\nstream: s\n";
    let e = instances::parse("x.yaml", unknown, &cat, &fac)
        .err()
        .unwrap();
    assert_eq!(e.code, code::UNKNOWN_CONNECTOR, "{e}");
}

/// Kafka instance files checked against the real manifest and config
/// schema, with the reference connector standing in for the family.
#[test]
fn kafka_instances_are_checked_against_the_registry() {
    let rc = ReferenceConnector::new();
    let cat = catalog();
    let mut fac = loams_fabric::connect::instances::Factories::new();
    let rc2 = rc.clone();
    fac.register("kafka", Some("kafka"), move |_, _| {
        Ok(Arc::new(rc2.clone()) as Arc<dyn ConnectorFactory>)
    });
    let good = "name: orders-in\nconnector: kafka\ndirection: source\nnamespace: w\nstream: s\n\
                config:\n  brokers: 'b:9092'\n  topics: [orders]\n  group_id_prefix: loams-x-\n  \
                security_protocol: sasl_ssl\n  sasl: {mechanism: plain, username: key, password: confluent-main/password}\n  \
                rate_limit: {bytes_per_sec: 1048576}\n  pipeline: {max_batch_rows: 100}\n";
    let i = instances::parse("o.yaml", good, &cat, &fac).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(i.rate_limit.bytes_per_sec(), Some(1_048_576.0));
    assert_eq!(i.dead_letter_stream.as_deref(), Some("orders-in-dlq"));
    let bad = good.replace("group_id_prefix: loams-x-", "bogus_key: 1");
    let e = instances::parse("o.yaml", &bad, &cat, &fac).err().unwrap();
    assert_eq!(e.code, code::CONFIG, "{e}");
    let bad_rate = good.replace("bytes_per_sec: 1048576", "bytes_per_sec: 0");
    let e = instances::parse("o.yaml", &bad_rate, &cat, &fac)
        .err()
        .unwrap();
    assert_eq!(e.code, code::CONFIG, "{e}");
    let bad_profile = good.replace(
        "connector: kafka",
        "connector: kafka\nprofile: aurora-postgres",
    );
    let e = instances::parse("o.yaml", &bad_profile, &cat, &fac)
        .err()
        .unwrap();
    assert_eq!(e.code, code::PROFILE, "{e}");
    let bad_name = good.replace("name: orders-in", "name: Orders_In");
    let e = instances::parse("o.yaml", &bad_name, &cat, &fac)
        .err()
        .unwrap();
    assert_eq!(e.code, code::INVALID_INSTANCE, "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_writer_lock_fences_second_process() {
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    let store = Store::in_memory();
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    let yaml = source_yaml(
        "orders-in",
        "  pipeline: {max_batch_rows: 2, checkpoint_interval_ms: 1}\n",
    );
    let first = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![instance(&yaml, &cat, &fac)],
    )
    .await;
    let first_epoch = first.epoch();
    let first_run = tokio::spawn(first.run(std::future::pending()));
    eventually("events", 10, || engine.ids("w", "orders").len() > 10).await;
    let second = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![instance(&yaml, &cat, &fac)],
    )
    .await;
    assert_eq!(second.epoch(), first_epoch + 1);
    let exit = tokio::time::timeout(Duration::from_secs(5), first_run)
        .await
        .expect("the first process stops")
        .unwrap();
    assert!(matches!(exit, Exit::Fenced(_)), "{exit:?}");
    // The second keeps running, and its checkpoints carry its epoch.
    eventually("the second process writes", 10, || {
        engine.ids("w", "orders").len() > 40
    })
    .await;
    assert_eq!(
        second.supervisor.pipeline_state("orders-in").await,
        Some(PipelineState::Running)
    );
    second.run(async {}).await;
}

// A connector that resolves a secret and then fails with it in the message,
// to show that nothing connect reports or logs carries it.
struct LeakyEnumerator;

#[async_trait::async_trait]
impl SplitEnumerator for LeakyEnumerator {
    async fn discover(&mut self, _known: &[SplitState]) -> Result<Vec<Split>, ConnectorError> {
        Ok(vec![Split::new("0", json!({}))])
    }
}

/// A source factory that resolves `creds/password` and fails to open with it.
struct LeakyFactory(Arc<ConnectorContext>);

#[derive(Clone)]
struct LeakyConnector;

impl ConnectorFactory for LeakyConnector {
    fn id(&self) -> &'static str {
        "leaky"
    }

    fn source(
        &self,
        ctx: &ConnectorContext,
    ) -> Result<Option<(Box<dyn SplitEnumerator>, Box<dyn SourceFactory>)>, ConnectorError> {
        Ok(Some((
            Box::new(LeakyEnumerator),
            Box::new(LeakyFactory(Arc::new(ctx.clone()))),
        )))
    }

    fn sink(
        &self,
        _ctx: &ConnectorContext,
    ) -> Result<Option<Box<dyn SinkFactory>>, ConnectorError> {
        Ok(None)
    }
}

impl SourceFactory for LeakyFactory {
    fn create(
        &self,
        _split: &Split,
    ) -> Result<Box<dyn loams_connector_sdk::Source>, ConnectorError> {
        Ok(Box::new(LeakySource(self.0.clone())))
    }
}

struct LeakySource(Arc<ConnectorContext>);

#[async_trait::async_trait]
impl loams_connector_sdk::Source for LeakySource {
    async fn open(
        &mut self,
        _split: &Split,
        _pos: Option<loams_connector_sdk::Position>,
    ) -> Result<(), ConnectorError> {
        let s = self
            .0
            .secrets
            .resolve(&SecretRef::new("creds/password"))
            .await?;
        let pw = s.expose_str().unwrap_or_default().to_owned();
        tracing::warn!("logging in with {pw}");
        Err(ConnectorError::auth(format!(
            "login refused for password {pw}"
        )))
    }

    async fn next(
        &mut self,
        _b: loams_connector_sdk::Budget,
    ) -> Result<Option<loams_connector_sdk::SourceBatch>, ConnectorError> {
        Ok(None)
    }

    async fn committed(
        &mut self,
        _upto: &loams_connector_sdk::Position,
    ) -> Result<(), ConnectorError> {
        Ok(())
    }

    async fn close(&mut self) -> Result<(), ConnectorError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn connect_logs_scrub_resolved_secrets() {
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    // FileSecretStore: <root>/<namespace_id>/<secret>/<key>, owner-only.
    let secret_dir = dir.path().join("secrets/0/creds");
    std::fs::create_dir_all(&secret_dir).unwrap();
    let file = secret_dir.join("password");
    std::fs::write(&file, "hunter2-very-secret\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let scrubber = Scrubber::new();
    let out = Captured::default();
    let subscriber = loams_fabric::connect::log::ScrubbingSubscriber::new(
        scrubber.clone(),
        Box::new(out.clone()),
        tracing::Level::DEBUG,
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    let cat = catalog().allow_unregistered("leaky");
    let mut fac = loams_fabric::connect::instances::Factories::new();
    fac.register("leaky", None, |_, _| {
        Ok(Arc::new(LeakyConnector) as Arc<dyn ConnectorFactory>)
    });
    let yaml = "name: leaky\nconnector: leaky\ndirection: source\nnamespace: w\nstream: s\n\
                dead_letter_stream: ''\nconfig: {}\n";
    let role = common::start(
        &engine,
        &Store::in_memory(),
        &dir.path().join("secrets"),
        scrubber.clone(),
        vec![instance(yaml, &cat, &fac)],
    )
    .await;
    let addr = role.admin.local_addr();
    let mut body = json!(null);
    for _ in 0..200 {
        let (_, b) = admin(
            addr,
            "GET",
            "/v1/connect/instances/leaky",
            Some("admin-token-for-tests"),
            None,
        )
        .await;
        body = b;
        if body["state"] == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(body["state"], "failed", "{body}");
    let status = body.to_string();
    assert!(status.contains("<redacted>"), "{status}");
    assert!(!status.contains("hunter2-very-secret"), "{status}");
    role.supervisor.write_statuses().await;
    let log = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("logging in with <redacted>"), "{log}");
    assert!(!log.contains("hunter2-very-secret"), "{log}");
    role.run(async {}).await;
}

/// A pipeline that failed with a transient error (its retries ran out, as
/// when the brokers stay down past the retry policy) is restarted with backoff
/// from its checkpoint and finishes, instead of staying `failed` until an
/// operator restarts it; one that failed for good (`Fatal`) stays failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_failure_is_restarted_with_backoff() {
    use loams_connector_sdk::ErrorClass;
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    let store = Store::in_memory();
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    let retry =
        "  pipeline: {max_batch_rows: 5, retry: {max_attempts: 2, initial_ms: 10, max_ms: 20}}\n";
    // Two failures exhaust the two attempts; the third is retried after the
    // restart and the split then reads to its end.
    let down = || {
        (
            ErrorClass::Retryable { after: None },
            "AllBrokersDown (Local: All broker connections are down)".to_owned(),
        )
    };
    rc.source.fail_next("a", [down(), down(), down()]);
    let role = common::start(
        &engine,
        &store,
        dir.path(),
        Scrubber::new(),
        vec![instance(
            &source_yaml("orders-in", &format!("  events_per_split: 20\n{retry}")),
            &cat,
            &fac,
        )],
    )
    .await;
    role.supervisor
        .set_restart_backoff(Duration::from_millis(50), Duration::from_millis(200));
    let sup = role.supervisor.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while sup.pipeline_state("orders-in").await != Some(PipelineState::Failed) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the instance never failed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The role's tick, by hand: the first call schedules, a later one restarts.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while engine.ids("w", "orders").len() < 40 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "not restarted: {:?}",
            sup.status("orders-in").await
        );
        sup.retry_failed_starts().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let status = sup.status("orders-in").await.unwrap();
    assert_eq!(status["state"], "running", "{status}");
    assert_eq!(status["auto_restarts"], 1, "{status}");
    let ids = engine.ids("w", "orders");
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 40, "no loss");
    role.run(async {}).await;

    // A Fatal failure is not restarted.
    let dir = TempDir::new().unwrap();
    let engine = FakeEngine::start(dir.path()).await;
    let rc = ReferenceConnector::new();
    let (cat, fac) = (catalog(), factories(&rc));
    rc.source
        .fail_next("a", [(ErrorClass::Fatal, "the table is gone".to_owned())]);
    let role = common::start(
        &engine,
        &Store::in_memory(),
        dir.path(),
        Scrubber::new(),
        vec![instance(
            &source_yaml("orders-in", &format!("  events_per_split: 20\n{retry}")),
            &cat,
            &fac,
        )],
    )
    .await;
    role.supervisor
        .set_restart_backoff(Duration::from_millis(10), Duration::from_millis(20));
    let sup = role.supervisor.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while sup.pipeline_state("orders-in").await != Some(PipelineState::Failed) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the instance never failed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for _ in 0..10 {
        sup.retry_failed_starts().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let status = sup.status("orders-in").await.unwrap();
    assert_eq!(status["state"], "failed", "{status}");
    assert_eq!(status["auto_restarts"], 0, "{status}");
    role.run(async {}).await;
}
