//! The agent end to end over a fake cgroupfs: scan, resolve, spool, crash
//! recovery, churn, backpressure and forwarding to a stream.

// Helpers outside `#[test]` functions unwrap too; clippy's
// `allow-unwrap-in-tests` covers only the test functions themselves.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use httpmock::MockServer;
use loams_cloudevents::partition::kafka_partition;
use loams_cloudevents::record::{RecordContext, from_record};
use loams_cloudevents::{CloudEvent, json as ce_json};
use loams_log::Record;
use loams_meter_agent::cgroup::DEFAULT_TARGETS;
use loams_meter_agent::event::PLATFORM_ORG;
use loams_meter_agent::usage::{self, NODE_CPU_TYPE, NODE_MEMORY_TYPE};
use loams_meter_agent::{
    Agent, CgroupReader, MemorySink, Meter, MeterError, MeterEvent, Sink, Spool, SpoolConfig,
    StaticResolver, StreamSink, StreamSinkConfig, forward,
};
use serde_json::Value;

const POD_A: &str = "kubepods.slice/kubepods-burstable.slice/\
                     kubepods-burstable-pod0e8f2a1c_3b4d_4e5f_8a9b_0c1d2e3f4a5b.slice";
const WORKERD_B: &str = "loams.slice/tenant-org_b.slice";
const SYSTEM: &str = "system.slice/containerd.service";

/// A fake cgroup v2 tree in a temp directory.
struct FakeCgroupFs {
    root: tempfile::TempDir,
}

impl FakeCgroupFs {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    /// Creates or updates a cgroup's files.
    fn set(&self, rel: &str, cpu_usec: u64, memory: u64, populated: bool) {
        let dir = self.path().join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("cpu.stat"),
            format!("usage_usec {cpu_usec}\nuser_usec {cpu_usec}\nsystem_usec 0\n"),
        )
        .unwrap();
        fs::write(dir.join("memory.current"), format!("{memory}\n")).unwrap();
        fs::write(dir.join("memory.peak"), format!("{memory}\n")).unwrap();
        fs::write(
            dir.join("cgroup.events"),
            format!("populated {}\nfrozen 0\n", u8::from(populated)),
        )
        .unwrap();
    }

    /// Removes a cgroup, as the runtime does after its last process exits.
    fn remove(&self, rel: &str) {
        fs::remove_dir_all(self.path().join(rel)).unwrap();
    }

    fn reader(&self) -> CgroupReader {
        CgroupReader::with_default_targets(self.path(), vec![])
    }
}

fn resolver() -> StaticResolver {
    StaticResolver::from_json(&format!(
        r#"{{"rules": [
            {{"prefix": "{POD_A}", "org": "org_a", "namespace": "prod", "tier": "t2"}},
            {{"prefix": "loams.slice/tenant-org_b.slice", "org": "org_b", "namespace": "dev",
              "function": "chat", "tier": "t0"}}
        ]}}"#
    ))
    .unwrap()
}

struct Node {
    cg: FakeCgroupFs,
    dir: tempfile::TempDir,
}

impl Node {
    fn new() -> Self {
        Self {
            cg: FakeCgroupFs::new(),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn spool_dir(&self) -> PathBuf {
        self.dir.path().join("spool")
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state.json")
    }

    fn open_with(&self, config: SpoolConfig, now_ms: i64) -> Agent<StaticResolver> {
        let spool = Spool::open(self.spool_dir(), config).unwrap();
        Agent::open(
            "node-1",
            self.state(),
            self.cg.reader(),
            resolver(),
            spool,
            now_ms,
        )
        .unwrap()
    }

    fn open(&self, now_ms: i64) -> Agent<StaticResolver> {
        self.open_with(SpoolConfig::default(), now_ms)
    }
}

fn spooled(agent: &mut Agent<StaticResolver>) -> Vec<MeterEvent> {
    agent
        .spool_mut()
        .pending(usize::MAX)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

/// Sum of `meter` for `org` over `events`.
fn total(events: &[MeterEvent], org: &str, meter: Meter) -> u64 {
    events
        .iter()
        .filter(|e| e.data.org == org && e.data.meter == meter)
        .map(|e| e.data.value)
        .sum()
}

#[test]
fn reader_samples_targets_only_and_skips_children() {
    let cg = FakeCgroupFs::new();
    cg.set(POD_A, 10, 100, true);
    // A container inside the pod: already counted in the pod's cpu.stat.
    cg.set(&format!("{POD_A}/cri-containerd-abc.scope"), 7, 50, true);
    cg.set(WORKERD_B, 20, 200, true);
    cg.set(SYSTEM, 30, 300, true);
    // A target directory with no cpu.stat (being created) is skipped.
    fs::create_dir_all(cg.path().join("loams.slice/tenant-new.slice")).unwrap();

    let samples = cg.reader().scan().unwrap();
    let paths: Vec<&str> = samples.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, vec![POD_A, WORKERD_B]);
    assert_eq!(samples[0].cpu_usec, 10);
    assert_eq!(samples[0].memory_current, Some(100));
    assert!(samples[0].populated);
    assert!(samples[0].inode > 0);

    // Scopes restrict the walk.
    let only_loam = CgroupReader::new(
        cg.path(),
        vec!["/loams.slice/".into()],
        DEFAULT_TARGETS.iter().map(|t| t.to_string()).collect(),
    );
    let samples = only_loam.scan().unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].path, WORKERD_B);
}

#[test]
fn memory_max_and_missing_controllers() {
    let cg = FakeCgroupFs::new();
    cg.set(WORKERD_B, 1, 1, true);
    let dir = cg.path().join(WORKERD_B);
    fs::remove_file(dir.join("memory.current")).unwrap();
    fs::write(dir.join("memory.peak"), "max\n").unwrap();
    let s = &cg.reader().scan().unwrap()[0];
    assert_eq!(s.memory_current, None);
    assert_eq!(s.memory_peak, None);

    fs::write(dir.join("cpu.stat"), "user_usec 1\n").unwrap();
    assert!(matches!(cg.reader().scan(), Err(MeterError::Parse { .. })));
}

#[test]
fn ticks_emit_attributed_cpu_and_memory_events() {
    let node = Node::new();
    node.cg.set(POD_A, 1_000, 1_000_000, true);
    node.cg.set(WORKERD_B, 5_000, 2_000_000, true);
    let mut agent = node.open(1_000);
    // Cold start: baselines only.
    let report = agent.tick(1_000).unwrap();
    assert_eq!((report.resources, report.events), (2, 0));

    node.cg.set(POD_A, 1_600, 1_000_000, true);
    node.cg.set(WORKERD_B, 5_250, 2_000_000, true);
    let report = agent.tick(2_000).unwrap();
    assert_eq!(report.events, 4);

    let events = spooled(&mut agent);
    assert_eq!(total(&events, "org_a", Meter::CpuUsec), 600);
    assert_eq!(total(&events, "org_b", Meter::CpuUsec), 250);
    assert_eq!(total(&events, "org_a", Meter::MemoryByteSeconds), 1_000_000);
    assert_eq!(total(&events, "org_b", Meter::MemoryByteSeconds), 2_000_000);

    let b = events
        .iter()
        .find(|e| e.data.org == "org_b" && e.data.meter == Meter::CpuUsec)
        .unwrap();
    assert_eq!(b.subject, "org_b");
    assert_eq!(b.data.function.as_deref(), Some("chat"));
    assert_eq!(b.data.counter, Some(5_250));
    assert!(b.data.resource.starts_with(WORKERD_B));
    assert_eq!(b.id, format!("node-1/cgroup/{}", b.data.seq));

    // Sequence numbers are unique and consecutive.
    let mut seqs: Vec<u64> = events.iter().map(|e| e.data.seq).collect();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(seqs.len(), 4);
    assert_eq!(seqs[3] - seqs[0], 3);
}

#[test]
fn an_unreadable_cgroup_is_skipped_and_its_usage_carried() {
    let node = Node::new();
    node.cg.set(POD_A, 1_000, 0, true);
    node.cg.set(WORKERD_B, 5_000, 0, true);
    let mut agent = node.open(0);
    agent.tick(0).unwrap();

    // org_b's cpu.stat is malformed for one tick: org_a is still metered.
    node.cg.set(POD_A, 1_100, 0, true);
    fs::write(
        node.cg.path().join(WORKERD_B).join("cpu.stat"),
        "user_usec 1\n",
    )
    .unwrap();
    let report = agent.tick(1_000).unwrap();
    assert_eq!((report.resources, report.skipped), (1, 1));

    // Readable again: counted from the old baseline, not from zero.
    node.cg.set(POD_A, 1_100, 0, true);
    node.cg.set(WORKERD_B, 5_300, 0, true);
    let report = agent.tick(2_000).unwrap();
    assert_eq!(report.skipped, 0);

    let events = spooled(&mut agent);
    assert_eq!(total(&events, "org_a", Meter::CpuUsec), 100);
    assert_eq!(total(&events, "org_b", Meter::CpuUsec), 300);
}

#[test]
fn unattributed_cgroups_are_metered_as_platform() {
    let node = Node::new();
    let orphan = "loams.slice/tenant-unknown.slice";
    node.cg.set(orphan, 0, 0, true);
    let mut agent = node.open(0);
    agent.tick(0).unwrap();
    node.cg.set(orphan, 90, 0, true);
    agent.tick(1_000).unwrap();
    let events = spooled(&mut agent);
    assert_eq!(total(&events, PLATFORM_ORG, Meter::CpuUsec), 90);
}

#[test]
fn churn_final_reads_and_recreation() {
    let node = Node::new();
    let mut agent = node.open(0);
    agent.tick(0).unwrap(); // warm, empty

    // A pod appears and runs: its whole counter is usage.
    node.cg.set(POD_A, 400, 0, true);
    agent.tick(1_000).unwrap();
    // Its last process exits: the final reading (populated 0).
    node.cg.set(POD_A, 450, 0, false);
    agent.tick(2_000).unwrap();
    // The runtime removes it, and a pod with the same path is recreated
    // (new inode): counted from zero, not as a negative delta.
    node.cg.remove(POD_A);
    agent.tick(3_000).unwrap();
    node.cg.set(POD_A, 30, 0, true);
    agent.tick(4_000).unwrap();

    let events = spooled(&mut agent);
    assert_eq!(total(&events, "org_a", Meter::CpuUsec), 400 + 50 + 30);
    let resources: std::collections::BTreeSet<&str> =
        events.iter().map(|e| e.data.resource.as_str()).collect();
    assert_eq!(resources.len(), 2);
}

#[test]
fn restart_resumes_without_double_counting() {
    let node = Node::new();
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open(1_000);
    agent.tick(1_000).unwrap();
    node.cg.set(WORKERD_B, 100, 0, true);
    agent.tick(2_000).unwrap();
    let seq_before = agent.next_seq();
    drop(agent);

    // A clean restart: resumes from the state file.
    node.cg.set(WORKERD_B, 160, 0, true);
    let mut agent = node.open(3_000);
    assert_eq!(agent.next_seq(), seq_before);
    agent.tick(3_000).unwrap();
    let events = spooled(&mut agent);
    assert_eq!(total(&events, "org_b", Meter::CpuUsec), 160);
}

#[test]
fn crash_between_spool_and_state_is_replayed() {
    let node = Node::new();
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open(1_000);
    agent.tick(1_000).unwrap();
    node.cg.set(WORKERD_B, 100, 0, true);
    agent.tick(2_000).unwrap();
    let old_state = fs::read(node.state()).unwrap();

    // This tick's events reach the spool...
    node.cg.set(WORKERD_B, 300, 0, true);
    agent.tick(3_000).unwrap();
    drop(agent);
    // ...but the crash lost its state write.
    fs::write(node.state(), old_state).unwrap();

    node.cg.set(WORKERD_B, 350, 0, true);
    let mut agent = node.open(4_000);
    agent.tick(4_000).unwrap();
    let events = spooled(&mut agent);
    // 100 + 200 + 50: the 200 is not emitted a second time.
    assert_eq!(total(&events, "org_b", Meter::CpuUsec), 350);
    let mut ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "no id reused");
}

#[test]
fn lost_state_undercounts_and_never_reuses_ids() {
    let node = Node::new();
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open(1_000);
    agent.tick(1_000).unwrap();
    node.cg.set(WORKERD_B, 100, 0, true);
    agent.tick(2_000).unwrap();
    let last_seq = agent.next_seq() - 1;
    drop(agent);
    fs::remove_file(node.state()).unwrap();

    node.cg.set(WORKERD_B, 180, 0, true);
    let mut agent = node.open(3_000);
    assert!(agent.next_seq() > last_seq);
    // Cold: the 80 since the last event is not billed.
    agent.tick(3_000).unwrap();
    node.cg.set(WORKERD_B, 200, 0, true);
    agent.tick(4_000).unwrap();
    let events = spooled(&mut agent);
    assert_eq!(total(&events, "org_b", Meter::CpuUsec), 100 + 20);
}

#[test]
fn a_full_spool_delays_usage_without_losing_it() {
    let node = Node::new();
    let config = SpoolConfig {
        max_bytes: 1_500,
        segment_bytes: 1,
    };
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open_with(config, 0);
    agent.tick(0).unwrap();
    let mut full_ticks = 0;
    for i in 1..=6u64 {
        node.cg.set(WORKERD_B, i * 100, 0, true);
        if agent.tick(i as i64 * 1_000).unwrap().spool_full {
            full_ticks += 1;
        }
    }
    assert!(full_ticks > 0, "the bound was reached");

    // Drain everything spooled so far, then tick once more.
    let mut sink = MemorySink::default();
    while forward(agent.spool_mut(), &mut sink, 100).await_blocking() > 0 {}
    node.cg.set(WORKERD_B, 700, 0, true);
    assert!(!agent.tick(7_000).unwrap().spool_full);
    while forward(agent.spool_mut(), &mut sink, 100).await_blocking() > 0 {}
    assert_eq!(total(&sink.events, "org_b", Meter::CpuUsec), 700);
}

#[tokio::test]
async fn forward_retries_after_a_failure_with_the_same_ids() {
    let node = Node::new();
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open(0);
    agent.tick(0).unwrap();
    node.cg.set(WORKERD_B, 42, 0, true);
    agent.tick(1_000).unwrap();

    let mut sink = MemorySink {
        fail_next: true,
        ..MemorySink::default()
    };
    let err = forward(agent.spool_mut(), &mut sink, 10).await.unwrap_err();
    assert!(matches!(err, MeterError::SinkUnavailable(_)));
    let before: Vec<String> = spooled(&mut agent).into_iter().map(|e| e.id).collect();
    assert_eq!(forward(agent.spool_mut(), &mut sink, 10).await.unwrap(), 1);
    assert_eq!(forward(agent.spool_mut(), &mut sink, 10).await.unwrap(), 0);
    let sent: Vec<String> = sink.events.iter().map(|e| e.id.clone()).collect();
    assert_eq!(sent, before);
}

/// An agent that has spooled one CPU and one memory event for `org_b`.
fn agent_with_events(node: &Node) -> Agent<StaticResolver> {
    node.cg.set(WORKERD_B, 0, 1_000, true);
    let mut agent = node.open(0);
    agent.tick(0).unwrap();
    node.cg.set(WORKERD_B, 42, 1_000, true);
    agent.tick(1_000).unwrap();
    agent
}

/// The rules of the Loams usage profile (design §63 §6.1) that a node
/// agent's event must meet. They mirror `loams-meter-core`'s
/// `Event::from_cloudevent(_, Origin::Service)` (ME1 Task 1); this test moves
/// onto that function once the core crate is on `dev`.
fn check_usage_profile(ce: &CloudEvent) {
    assert_eq!(ce.attr("specversion"), Some("1.0"));
    assert!(!ce.id().is_empty());
    assert!(ce.source().starts_with("loams://"), "{}", ce.source());
    // io.loams.dev.usage.<product>.<name>.v<major>
    let rest = ce.ty().strip_prefix("io.loams.dev.usage.").unwrap();
    let parts: Vec<&str> = rest.split('.').collect();
    assert_eq!(parts.len(), 3, "{}", ce.ty());
    for part in &parts[..2] {
        assert!(!part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    }
    let major = parts[2].strip_prefix('v').unwrap();
    assert!(major.parse::<u32>().unwrap() >= 1);
    // subject: the billing subject, required (OpenMeter's Event requires it).
    let subject = ce.attr("subject").unwrap();
    assert!(!subject.is_empty() && !subject.contains('/'), "{subject}");
    // time: RFC 3339, UTC, nanoseconds.
    let time = ce.attr("time").unwrap();
    assert!(ce.time_ms().is_some(), "{time}");
    let fraction = time.rsplit_once('.').unwrap().1;
    assert_eq!(fraction.len(), 10, "{time}");
    assert!(fraction.ends_with('Z'));
    assert_eq!(ce.datacontenttype(), Some("application/json"));
    assert_eq!(
        ce.attr("dataschema").unwrap(),
        format!("urn:loams:json:{}", ce.ty())
    );
    let (org, namespace) = ce.attr("tenantid").unwrap().split_once('/').unwrap();
    assert!(!org.is_empty() && !namespace.is_empty() && !namespace.contains('/'));
    // data: a JSON object; quantities are integers, dimensions strings.
    let data: Value = serde_json::from_slice(ce.data().unwrap()).unwrap();
    let data = data.as_object().unwrap();
    for (name, value) in data {
        match name.as_str() {
            "estimated" => assert!(value.is_boolean()),
            n if n.ends_with("_usec")
                || n.ends_with("_bytes")
                || n.ends_with("_seconds")
                || n == "fuel" =>
            {
                assert!(value.is_u64(), "{name}: {value}")
            }
            _ => assert!(value.is_string(), "{name}: {value}"),
        }
    }
    // The structured JSON form (what OpenMeter's POST /api/v1/events takes)
    // reads back as the same event.
    let reparsed = ce_json::parse_event(&ce_json::write_event(ce)).unwrap();
    assert_eq!(&reparsed, ce);
}

#[test]
fn agent_event_passes_usage_profile() {
    let node = Node::new();
    let mut agent = agent_with_events(&node);
    let events = spooled(&mut agent);
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(types.contains(&NODE_CPU_TYPE), "{types:?}");
    assert!(types.contains(&NODE_MEMORY_TYPE), "{types:?}");
    for event in &events {
        let ce = usage::to_cloudevent(event, "eu1").unwrap();
        check_usage_profile(&ce);
        assert_eq!(ce.ty(), event.event_type);
        assert_eq!(ce.id(), event.id);
        assert_eq!(ce.source(), "loams://eu1/meter-agent/node-1");
        assert_eq!(ce.attr("subject"), Some("org_b"));
        assert_eq!(ce.attr("tenantid"), Some("org_b/dev"));
        let data: Value = serde_json::from_slice(ce.data().unwrap()).unwrap();
        assert_eq!(data["function"], "chat");
        assert_eq!(data["tier"], "t0");
    }
    // Unattributed usage is metered under _platform, which also passes.
    let mut platform = events[0].clone();
    platform.data.org = PLATFORM_ORG.into();
    platform.data.namespace = PLATFORM_ORG.into();
    check_usage_profile(&usage::to_cloudevent(&platform, "eu1").unwrap());
}

/// Decodes a produce-route record body back into a log record, as the
/// engine's `POST .../records` handler does.
fn decode_record(body: &Value) -> Record {
    let b64 = |v: &Value| {
        v.as_str()
            .map(|s| bytes::Bytes::from(BASE64.decode(s).unwrap()))
    };
    Record {
        key: b64(&body["key"]),
        value: b64(&body["value"]),
        headers: body["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| (h["key"].as_str().unwrap().to_string(), b64(&h["value"])))
            .collect(),
        timestamp_ms: body["timestamp_ms"].as_i64().unwrap(),
    }
}

#[test]
fn agent_records_rebuild_as_cloudevents() {
    let node = Node::new();
    let mut agent = agent_with_events(&node);
    let events = spooled(&mut agent);
    assert!(!events.is_empty());
    for event in &events {
        let ce = usage::to_cloudevent(event, "eu1").unwrap();
        let (partition, body) = loams_meter_agent::stream::record(event, "eu1", 32).unwrap();
        let record = decode_record(&body);
        // D270's binary-mode layout: ce_ headers, content-type, key = subject.
        assert_eq!(record.key.as_deref(), Some(&b"org_b"[..]));
        assert!(record.headers.iter().any(|(k, _)| k == "ce_specversion"));
        assert!(record.headers.iter().any(|(k, _)| k == "ce_tenantid"));
        assert!(record.headers.iter().any(|(k, _)| k == "content-type"));
        assert!(
            !record
                .headers
                .iter()
                .any(|(k, _)| k == "ce_datacontenttype")
        );
        assert_eq!(record.timestamp_ms, 1_000);
        let rebuilt = from_record(
            &record,
            RecordContext {
                namespace: "_system",
                stream: "_meter",
                stream_id: 1,
                partition,
                offset: 0,
            },
        );
        assert_eq!(rebuilt, ce);
        assert_eq!(rebuilt.dedup_key(), ce.dedup_key());
    }
}

#[test]
fn agent_partition_matches_counter_owner() {
    let node = Node::new();
    let mut agent = agent_with_events(&node);
    let events = spooled(&mut agent);
    for partitions in [1, 4, 32, 64] {
        let sink = StreamSink::new(StreamSinkConfig::new("http://unused", partitions)).unwrap();
        for event in &events {
            let ce = usage::to_cloudevent(event, "eu1").unwrap();
            // The counter owner of a subject holds kafka_partition(subject)
            // (§63 §11.1); the agent must produce there.
            let owner = kafka_partition(b"org_b", partitions);
            let (partition, _) =
                loams_meter_agent::stream::record(event, "eu1", partitions).unwrap();
            assert_eq!(partition, owner);
            assert_eq!(usage::partition(&ce, partitions), owner);
            assert_eq!(sink.partition_of("org_b"), owner);
            assert_eq!(
                loams_cloudevents::partition::partition_of(&ce, partitions),
                owner
            );
        }
    }
    // Different subjects spread by the same function.
    for subject in ["org_a", "org_42", PLATFORM_ORG] {
        assert_eq!(
            usage::subject_partition(subject, 32),
            kafka_partition(subject.as_bytes(), 32)
        );
    }
}

#[tokio::test]
async fn stream_sink_produces_to_the_subject_partition() {
    let server = MockServer::start_async().await;
    let node = Node::new();
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open(0);
    agent.tick(0).unwrap();
    node.cg.set(WORKERD_B, 42, 0, true);
    agent.tick(1_000).unwrap();
    let event = spooled(&mut agent).remove(0);
    let ce = usage::to_cloudevent(&event, "local").unwrap();

    let mut sink = StreamSink::new(StreamSinkConfig::new(server.base_url(), 4)).unwrap();
    let partition = kafka_partition(b"org_b", 4);
    let value = BASE64.encode(ce.data().unwrap());
    let mock = server
        .mock_async(|when, then| {
            when.method("POST")
                .path(format!("/internal/v1/meter/partitions/{partition}/records"))
                .json_body_includes(
                    serde_json::json!({
                        "records": [{
                            "key": BASE64.encode("org_b"),
                            "value": value,
                            "timestamp_ms": 1_000,
                        }]
                    })
                    .to_string(),
                );
            then.status(200)
                .json_body(serde_json::json!({"base_offset": 0, "last_offset": 0}));
        })
        .await;

    assert_eq!(forward(agent.spool_mut(), &mut sink, 10).await.unwrap(), 1);
    mock.assert_async().await;
    assert_eq!(agent.spool_mut().pending(10).unwrap().len(), 0);
}

#[tokio::test]
async fn stream_errors_keep_events_spooled() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("POST");
            then.status(503).body("busy");
        })
        .await;
    let node = Node::new();
    node.cg.set(WORKERD_B, 0, 0, true);
    let mut agent = node.open(0);
    agent.tick(0).unwrap();
    node.cg.set(WORKERD_B, 42, 0, true);
    agent.tick(1_000).unwrap();

    let mut sink = StreamSink::new(StreamSinkConfig::new(server.base_url(), 1)).unwrap();
    let err = forward(agent.spool_mut(), &mut sink, 10).await.unwrap_err();
    assert!(matches!(err, MeterError::SinkUnavailable(_)));
    assert_eq!(agent.spool_mut().pending(10).unwrap().len(), 1);

    let rejecting = MockServer::start_async().await;
    rejecting
        .mock_async(|when, then| {
            when.method("POST");
            then.status(404).body("no such stream");
        })
        .await;
    let mut sink = StreamSink::new(StreamSinkConfig::new(rejecting.base_url(), 1)).unwrap();
    let err = sink.send(&spooled(&mut agent)).await.unwrap_err();
    assert!(matches!(err, MeterError::SinkRejected(_)));
}

/// Runs a future to completion on a fresh runtime, for sync tests.
trait AwaitBlocking {
    type Output;
    fn await_blocking(self) -> Self::Output;
}

impl<F: std::future::Future<Output = Result<usize, MeterError>>> AwaitBlocking for F {
    type Output = usize;
    fn await_blocking(self) -> usize {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(self)
            .unwrap()
    }
}
