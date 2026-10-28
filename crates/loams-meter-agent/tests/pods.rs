//! Kubernetes churn over a fake cgroupfs (ME1 Task 30): pods come and go
//! faster than the periodic scan, each pod's cgroup is read once more when
//! `cgroup.events` says it is empty (the final read), and the pod index
//! keeps a deleted pod resolvable until then.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

use loams_meter_agent::pods::{NAMESPACE_LABEL, ORG_LABEL, PodIndex, PodResolver, TIER_LABEL};
use loams_meter_agent::{Agent, CgroupReader, Meter, Spool, SpoolConfig, StaticResolver};

/// The periodic scan (doc 06 §3.3's floor).
const SCAN_MS: i64 = 15_000;
const STEP_MS: i64 = 100;
const PODS: u64 = 1_000;

struct Pod {
    uid: String,
    rel: String,
    org: String,
    start_ms: i64,
    end_ms: i64,
    /// CPU microseconds per millisecond of life.
    rate: u64,
    done: bool,
}

impl Pod {
    fn new(i: u64) -> Self {
        let uid = format!("{i:08x}-0000-4000-8000-{i:012x}");
        let rel = format!(
            "kubepods.slice/kubepods-besteffort.slice/kubepods-besteffort-pod{}.slice",
            uid.replace('-', "_")
        );
        let start_ms = i as i64 * 400;
        // 0.2 s to 30 s: most pods end between two scans.
        let life = 200 + (i.wrapping_mul(7_919) % 30_000) as i64;
        Self {
            uid,
            rel,
            org: format!("org_{}", i % 20),
            start_ms,
            end_ms: start_ms + life,
            rate: 100 + (i % 13) * 50,
            done: false,
        }
    }

    fn cpu_at(&self, now_ms: i64) -> u64 {
        let ms = (now_ms.min(self.end_ms) - self.start_ms).max(0) as u64;
        ms * self.rate
    }

    fn total(&self) -> u64 {
        self.cpu_at(self.end_ms)
    }
}

fn write(root: &Path, rel: &str, cpu_usec: u64, populated: bool) {
    let dir = root.join(rel);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("cpu.stat"), format!("usage_usec {cpu_usec}\n")).unwrap();
    fs::write(dir.join("memory.current"), "0\n").unwrap();
    fs::write(
        dir.join("cgroup.events"),
        format!("populated {}\nfrozen 0\n", u8::from(populated)),
    )
    .unwrap();
}

fn labels(org: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (ORG_LABEL.to_string(), org.to_string()),
        (NAMESPACE_LABEL.to_string(), "prod".to_string()),
        (TIER_LABEL.to_string(), "t2".to_string()),
    ])
}

/// Runs the churn; `final_reads` says whether a finished pod's cgroup is
/// read before it is removed. Returns (metered, true) CPU over all tenants.
// The loop indexes `pods` because it also lends the whole slice to `refresh`.
#[allow(clippy::needless_range_loop)]
fn churn(final_reads: bool) -> (u64, u64) {
    let cg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let index = Arc::new(RwLock::new(PodIndex::new()));
    let spool = Spool::open(state.path().join("spool"), SpoolConfig::default()).unwrap();
    let mut agent = Agent::open(
        "node-1",
        state.path().join("state.json"),
        CgroupReader::with_default_targets(cg.path(), vec![]),
        PodResolver::new(index.clone(), StaticResolver::default()),
        spool,
        0,
    )
    .unwrap();
    // Warm: a pod first seen after this counts from zero.
    agent.tick(0).unwrap();

    let mut pods: Vec<Pod> = (0..PODS).map(Pod::new).collect();
    let last_end = pods.iter().map(|p| p.end_ms).max().unwrap();
    // Live pods' counters only need to be current when a scan reads them.
    let refresh = |pods: &[Pod], now: i64| {
        for pod in pods
            .iter()
            .filter(|p| !p.done && p.start_ms <= now && now < p.end_ms)
        {
            write(cg.path(), &pod.rel, pod.cpu_at(now), true);
        }
    };
    let mut now = 0;
    while now <= last_end + SCAN_MS {
        now += STEP_MS;
        for i in 0..pods.len() {
            let pod = &pods[i];
            if pod.done || pod.start_ms > now {
                continue;
            }
            if index.read().unwrap().get(&pod.uid).is_none() {
                index.write().unwrap().upsert(&pod.uid, &labels(&pod.org));
            }
            if now < pod.end_ms {
                continue;
            }
            // The pod's last process exited: the kernel sets populated 0,
            // the API server deletes the pod, and the kubelet removes the
            // cgroup. With final reads, the agent scans in between.
            write(cg.path(), &pod.rel, pod.total(), false);
            index.write().unwrap().delete(&pod.uid, now);
            if final_reads {
                refresh(&pods, now);
                agent.tick(now).unwrap();
            }
            fs::remove_dir_all(cg.path().join(&pods[i].rel)).unwrap();
            pods[i].done = true;
        }
        if now % SCAN_MS == 0 {
            refresh(&pods, now);
            agent.tick(now).unwrap();
            index.write().unwrap().expire(now);
        }
    }

    let metered: u64 = agent
        .spool_mut()
        .pending(usize::MAX)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| e.data.meter == Meter::CpuUsec && e.data.org.starts_with("org_"))
        .map(|e| e.data.value)
        .sum();
    let truth: u64 = pods.iter().map(Pod::total).sum();
    (metered, truth)
}

#[test]
fn churn_1000_pods_within_1_percent() {
    let (metered, truth) = churn(true);
    assert!(truth > 0);
    let diff = metered.abs_diff(truth);
    assert!(
        diff * 100 <= truth,
        "metered {metered} vs true {truth}: off by {diff} (more than 1%)"
    );
    assert!(
        metered <= truth,
        "never more than was used: {metered} > {truth}"
    );
}

#[test]
fn without_final_reads_short_pods_are_lost() {
    // The control: the same churn with only the periodic scan loses the
    // usage of every pod between its last scan and its removal.
    let (metered, truth) = churn(false);
    assert!(
        metered.abs_diff(truth) * 100 > truth,
        "expected a loss above 1% without final reads: {metered} vs {truth}"
    );
}
