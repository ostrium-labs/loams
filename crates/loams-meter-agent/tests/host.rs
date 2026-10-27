//! The T1 source end to end: a reporter and the agent's socket server over a
//! real Unix socket in a temp directory, and the agent splitting a fake
//! wasmtime host cgroup's CPU by the reports (ME1 Task 29).

#![allow(clippy::unwrap_used)]

use std::fs;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use loams_meter_agent::host::proto::Invocation;
use loams_meter_agent::host::server::{self, HostEvent, HostServerConfig};
use loams_meter_agent::host::{Reporter, ReporterConfig, Sessions};
use loams_meter_agent::{
    Agent, CgroupReader, Meter, MeterEvent, Spool, SpoolConfig, StaticResolver,
};
use tokio::sync::mpsc;

const HOST: &str = "loams.slice/wasm-host.scope";

fn inv(org: &str, id: &str, cpu: u64) -> Invocation {
    Invocation {
        org: org.into(),
        namespace: "prod".into(),
        function: "fn".into(),
        invocation_id: id.into(),
        cpu_usec: cpu,
        fuel: cpu * 10,
        start_unix_ms: 0,
        end_unix_ms: 500,
        ..Invocation::default()
    }
}

type Recorded = Arc<Mutex<Vec<(String, u64, usize)>>>;

/// A server whose "agent loop" records what it was asked, and can be told to
/// refuse (a full spool).
struct Harness {
    _dir: tempfile::TempDir,
    socket: std::path::PathBuf,
    recorded: Recorded,
    gaps: Arc<Mutex<Vec<(u64, u64)>>>,
    refuse: Arc<Mutex<bool>>,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("meter.sock");
    let listener = server::bind(&socket).unwrap();
    let config = HostServerConfig {
        allowed_cgroups: vec![],
        ..HostServerConfig::default()
    };
    let (tx, mut rx) = mpsc::channel::<HostEvent>(64);
    tokio::spawn(server::serve(
        listener,
        Arc::new(config),
        Arc::new(Mutex::new(Sessions::new())),
        tx,
    ));
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let gaps = Arc::new(Mutex::new(Vec::new()));
    let refuse = Arc::new(Mutex::new(false));
    let (r, g, f) = (recorded.clone(), gaps.clone(), refuse.clone());
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                HostEvent::Reports {
                    host_id,
                    seq,
                    invocations,
                    done,
                } => {
                    let refuse = *f.lock().unwrap();
                    if refuse {
                        let _ = done.send(Err("spool full".into()));
                    } else {
                        r.lock().unwrap().push((host_id, seq, invocations.len()));
                        let _ = done.send(Ok(()));
                    }
                }
                HostEvent::Gap {
                    from_seq,
                    to_seq,
                    done,
                    ..
                } => {
                    g.lock().unwrap().push((from_seq, to_seq));
                    let _ = done.send(Ok(()));
                }
                HostEvent::FinalRead { done, .. } => {
                    let _ = done.send(());
                }
            }
        }
    });
    Harness {
        _dir: dir,
        socket,
        recorded,
        gaps,
        refuse,
    }
}

fn config(socket: &std::path::Path) -> ReporterConfig {
    ReporterConfig {
        socket: socket.to_path_buf(),
        flush_interval: Duration::from_millis(20),
        reconnect_min: Duration::from_millis(10),
        reconnect_max: Duration::from_millis(50),
        ..ReporterConfig::default()
    }
}

async fn wait_for(mut cond: impl FnMut() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 5 s");
}

#[tokio::test]
async fn reports_flow_and_are_acked() {
    let h = harness().await;
    let reporter = Reporter::start(config(&h.socket));
    reporter.record(inv("org_a", "i1", 100));
    reporter.record(inv("org_b", "i2", 200));
    reporter.flush();
    wait_for(|| reporter.stats().acked_seq.load(Ordering::Relaxed) >= 1).await;
    wait_for(|| {
        h.recorded
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.2)
            .sum::<usize>()
            == 2
    })
    .await;
    let recorded = h.recorded.lock().unwrap().clone();
    // One batch normally; a flush tick between the two records may split
    // them, so check the host, the numbering and the total.
    assert!(recorded.iter().all(|r| r.0 == reporter.host_id()));
    assert_eq!(recorded[0].1, 1);
    reporter.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn resends_unacked_after_reconnect_and_ack_is_cumulative() {
    let h = harness().await;
    // The agent cannot spool: nothing is acked, the reports stay buffered.
    *h.refuse.lock().unwrap() = true;
    let reporter = Reporter::start(config(&h.socket));
    reporter.record(inv("org_a", "i1", 100));
    reporter.flush();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(reporter.stats().acked_seq.load(Ordering::Relaxed), 0);
    reporter.record(inv("org_a", "i2", 100));
    reporter.flush();
    // The spool has room again: the reconnect resends seq 1 and 2.
    *h.refuse.lock().unwrap() = false;
    wait_for(|| reporter.stats().acked_seq.load(Ordering::Relaxed) >= 2).await;
    let seqs: Vec<u64> = h.recorded.lock().unwrap().iter().map(|r| r.1).collect();
    assert_eq!(seqs, vec![1, 2], "each report recorded once, in order");
    reporter.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn stalled_consumer_drops_oldest_and_announces_a_gap() {
    let h = harness().await;
    *h.refuse.lock().unwrap() = true;
    let mut cfg = config(&h.socket);
    // Room for about one report.
    cfg.max_buffer_bytes = 64;
    let reporter = Reporter::start(cfg);
    for i in 0..3 {
        reporter.record(inv("org_a", &format!("i{i}"), 100));
        reporter.flush();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_for(|| reporter.stats().dropped.load(Ordering::Relaxed) >= 1).await;
    *h.refuse.lock().unwrap() = false;
    wait_for(|| reporter.stats().acked_seq.load(Ordering::Relaxed) >= 3).await;
    // The agent was told about the dropped reports before the next one and
    // never acked past them silently.
    let gaps = h.gaps.lock().unwrap().clone();
    assert!(!gaps.is_empty(), "a Gap was sent");
    let recorded: Vec<u64> = h.recorded.lock().unwrap().iter().map(|r| r.1).collect();
    assert!(recorded.contains(&3));
    for (from, to) in gaps {
        assert!(!recorded.iter().any(|s| (from..=to).contains(s)));
    }
    reporter.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn no_consumer_costs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let reporter = Reporter::start(config(&dir.path().join("absent.sock")));
    for i in 0..100 {
        reporter.record(inv("org_a", &format!("i{i}"), 1));
    }
    reporter.flush();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(reporter.stats().connects.load(Ordering::Relaxed), 0);
    reporter.shutdown(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn restart_gets_new_host_id() {
    let dir = tempfile::tempdir().unwrap();
    let a = Reporter::start(config(&dir.path().join("absent.sock")));
    let b = Reporter::start(config(&dir.path().join("absent.sock")));
    assert_ne!(a.host_id(), b.host_id());
    assert_eq!(a.host_id().len(), 32);
}

#[tokio::test]
async fn sandbox_finished_waits_for_read_ack() {
    let h = harness().await;
    let reporter = Reporter::start(config(&h.socket));
    assert!(
        reporter
            .sandbox_finished("loams.slice/wasm-host.scope/sb-1", Duration::from_secs(5))
            .await
    );
    reporter.shutdown(Duration::from_secs(1)).await;
}

// The agent's side: the shared host cgroup's CPU is split by the reports.

fn set_cgroup(root: &std::path::Path, rel: &str, cpu_usec: u64) {
    let dir = root.join(rel);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("cpu.stat"), format!("usage_usec {cpu_usec}\n")).unwrap();
    fs::write(dir.join("memory.current"), "0\n").unwrap();
    fs::write(dir.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
}

fn cpu_events(events: &[MeterEvent]) -> Vec<&MeterEvent> {
    events
        .iter()
        .filter(|e| e.data.meter == Meter::CpuUsec)
        .collect()
}

fn open_agent(cg: &std::path::Path, state: &std::path::Path, now_ms: i64) -> Agent<StaticResolver> {
    let spool = Spool::open(state.join("spool"), SpoolConfig::default()).unwrap();
    let mut agent = Agent::open(
        "node-1",
        state.join("state.json"),
        CgroupReader::with_default_targets(cg, vec![]),
        StaticResolver::default(),
        spool,
        now_ms,
    )
    .unwrap();
    agent.set_host_reports(true);
    agent
}

#[test]
fn t1_host_cpu_is_split_by_reports_within_the_cgroup_total() {
    let cg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    set_cgroup(cg.path(), HOST, 0);
    let mut agent = open_agent(cg.path(), state.path(), 0);
    agent.tick(0).unwrap();

    // Reports claim 1,500 µs, the cgroup saw 1,000: scaled down.
    agent
        .record_host_report(
            "h1",
            1,
            &[inv("org_a", "i1", 1_200), inv("org_b", "i2", 300)],
        )
        .unwrap();
    set_cgroup(cg.path(), HOST, 1_000);
    agent.tick(1_000).unwrap();

    let events: Vec<MeterEvent> = agent
        .spool_mut()
        .pending(usize::MAX)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let fuel: Vec<&MeterEvent> = events
        .iter()
        .filter(|e| e.data.meter == Meter::WasmFuel)
        .collect();
    assert_eq!(fuel.len(), 2);
    assert_eq!(fuel[0].id, "node-1/wasm/h1:1:0");
    assert_eq!(fuel[0].event_type, "io.loams.dev.usage.function.fuel.v1");
    let cpu = cpu_events(&events);
    let total: u64 = cpu.iter().map(|e| e.data.value).sum();
    assert!(total <= 1_000, "split {total} exceeds the cgroup total");
    let a = cpu.iter().find(|e| e.data.org == "org_a").unwrap();
    let b = cpu.iter().find(|e| e.data.org == "org_b").unwrap();
    assert_eq!(a.data.value, 800);
    assert_eq!(b.data.value, 200);
    assert!(a.data.estimated, "a scaled split is an estimate");
    assert_eq!(a.subject, "org_a");

    // After a restart the ledger and baselines come back from state.
    drop(agent);
    let mut agent = open_agent(cg.path(), state.path(), 2_000);
    agent
        .record_host_report("h1", 2, &[inv("org_a", "i3", 495)])
        .unwrap();
    set_cgroup(cg.path(), HOST, 1_500);
    let before = agent.spool_mut().next_offset();
    agent.tick(2_000).unwrap();
    let new: Vec<MeterEvent> = agent
        .spool_mut()
        .pending(usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|(o, _)| *o >= before)
        .map(|(_, e)| e)
        .collect();
    let cpu = cpu_events(&new);
    // 495 of 500 is within the 2% tolerance: the report bills, and the rest
    // is the host's overhead under _platform.
    let a = cpu.iter().find(|e| e.data.org == "org_a").unwrap();
    assert_eq!(a.data.value, 495);
    assert!(!a.data.estimated);
    let overhead = cpu.iter().find(|e| e.data.org == "_platform").unwrap();
    assert_eq!(overhead.data.value, 5);
}
