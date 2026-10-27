//! The sink's counters, per partition and in total.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// One partition's counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionMetrics {
    /// The last offset committed to Iggy.
    pub committed: Option<u64>,
    /// The next offset to read.
    pub next: u64,
    /// Messages polled from Iggy (a replay counts again).
    pub polled: u64,
    /// Events the target appended.
    pub appended: u64,
    /// Events the target already had (replays answered by the ledger).
    pub duplicates: u64,
    /// Messages the mapping could not read, kept as undecodable events.
    pub undecodable: u64,
    /// Writes retried after backpressure.
    pub backpressure: u64,
    /// Writes retried after an in-flight answer.
    pub in_flight: u64,
    /// Failed calls to the target or to Iggy (each retried).
    pub errors: u64,
    /// The last error, with its time (ms since the epoch).
    pub last_error: Option<(u64, String)>,
}

/// Every partition's counters, keyed by `(topic, partition)`.
#[derive(Debug, Default)]
pub struct SinkMetrics {
    partitions: Mutex<BTreeMap<(String, u32), PartitionMetrics>>,
    /// Topics the sink does not carry (names Loams refuses), with why.
    skipped: Mutex<BTreeMap<String, String>>,
    /// The last error of topic discovery or stream creation, with its time.
    discovery: Mutex<Option<(u64, String)>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

impl SinkMetrics {
    /// Applies `f` to one partition's counters.
    pub fn update(&self, topic: &str, partition: u32, f: impl FnOnce(&mut PartitionMetrics)) {
        let mut map = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        f(map.entry((topic.to_string(), partition)).or_default());
    }

    /// Records a failed call.
    pub fn error(&self, topic: &str, partition: u32, error: &str) {
        self.update(topic, partition, |m| {
            m.errors += 1;
            m.last_error = Some((now_ms(), error.to_string()));
        });
    }

    /// Records a failed discovery pass (listing topics, creating a stream).
    pub fn discovery_error(&self, error: &str) {
        let mut slot = self.discovery.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some((now_ms(), error.to_string()));
    }

    /// Records a topic the sink leaves alone.
    pub fn skip(&self, topic: &str, why: &str) {
        let mut map = self.skipped.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(topic.to_string(), why.to_string());
    }

    /// One partition's counters.
    pub fn partition(&self, topic: &str, partition: u32) -> Option<PartitionMetrics> {
        let map = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&(topic.to_string(), partition)).cloned()
    }

    /// The sum over every partition.
    pub fn total(&self) -> PartitionMetrics {
        let map = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        let mut total = PartitionMetrics::default();
        for m in map.values() {
            total.polled += m.polled;
            total.appended += m.appended;
            total.duplicates += m.duplicates;
            total.undecodable += m.undecodable;
            total.backpressure += m.backpressure;
            total.in_flight += m.in_flight;
            total.errors += m.errors;
            if m.last_error.as_ref().map(|e| e.0) > total.last_error.as_ref().map(|e| e.0) {
                total.last_error.clone_from(&m.last_error);
            }
        }
        total
    }

    /// The counters as JSON: `{"total", "partitions", "skipped_topics", "last_discovery_error"}`.
    pub fn to_json(&self) -> Value {
        fn one(m: &PartitionMetrics) -> Value {
            json!({
                "polled": m.polled,
                "appended": m.appended,
                "duplicates": m.duplicates,
                "undecodable": m.undecodable,
                "backpressure_retries": m.backpressure,
                "in_flight_retries": m.in_flight,
                "errors": m.errors,
                "last_error": m.last_error.as_ref().map(|(at, msg)| json!({"at_ms": at, "message": msg})),
            })
        }
        let partitions: Vec<Value> = {
            let map = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
            map.iter()
                .map(|((topic, partition), m)| {
                    let mut v = one(m);
                    v["topic"] = json!(topic);
                    v["partition"] = json!(partition);
                    v["committed_offset"] = json!(m.committed);
                    v["next_offset"] = json!(m.next);
                    v
                })
                .collect()
        };
        let skipped = self
            .skipped
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let discovery = self
            .discovery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        json!({
            "total": one(&self.total()),
            "partitions": partitions,
            "skipped_topics": skipped,
            "last_discovery_error": discovery.map(|(at, msg)| json!({"at_ms": at, "message": msg})),
        })
    }
}
