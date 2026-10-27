//! Loams Meter's House tables against a real House (ME1 Tasks 5–8, design
//! §63 §7.5, §8): the `_meter` schema, the `LoamsStream` pipe from
//! `_system/_meter` and the SQL the meter runs.
//!
//! The SQL is the meter's own: `crates/loams-meter-house/src/sql.rs` is
//! std-only so that this test compiles it as it is (`#[path]`), without the
//! fabric workspace depending on a root-workspace crate. The engine's stream
//! API is stood in for by [`MemStream`], as in `pipes.rs`.

#![cfg(target_os = "linux")]

mod common;

#[path = "../../../../crates/loams-meter-house/src/sql.rs"]
#[allow(dead_code)]
mod meter_sql;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::http::{Response, request, target};
use common::{small, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, MemCatalog, PrefixGrants};
use loams_house::config::{EngineLink, HouseConfig, UserMap};
use loams_house::errors::{ChError, HouseError};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::pipes::loams_stream::cloudevent_row;
use loams_house::pipes::{Bounds, Fetched, PipeRunner, Ran, StreamSource, pipes};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_store::Store;

const NS: u64 = 2;

/// A stream in memory: records per partition, by stream name.
#[derive(Debug, Default)]
struct MemStream {
    streams: Mutex<BTreeMap<String, Vec<Vec<Vec<u8>>>>>,
}

impl MemStream {
    fn push(&self, stream: &str, partition: usize, record: &[u8]) {
        let mut streams = self.streams.lock().expect("lock");
        let parts = streams.entry(stream.to_string()).or_default();
        while parts.len() <= partition {
            parts.push(Vec::new());
        }
        parts[partition].push(record.to_vec());
    }
}

#[async_trait]
impl StreamSource for MemStream {
    async fn partitions(&self, _namespace: &str, stream: &str) -> Result<Vec<Bounds>, HouseError> {
        let streams = self.streams.lock().expect("lock");
        let Some(parts) = streams.get(stream) else {
            return Err(HouseError::from(ChError::bad_arguments(
                "the pipe's stream does not exist in its namespace",
            )));
        };
        Ok(parts
            .iter()
            .enumerate()
            .map(|(p, records)| Bounds {
                partition: p as u32,
                log_start_offset: 0,
                high_watermark: records.len() as u64,
            })
            .collect())
    }

    async fn fetch(
        &self,
        _namespace: &str,
        stream: &str,
        partition: u32,
        offset: u64,
        _max_bytes: usize,
        events: bool,
    ) -> Result<Fetched, HouseError> {
        assert!(events, "the meter's pipe reads CloudEvents");
        let streams = self.streams.lock().expect("lock");
        let records = &streams[stream][partition as usize];
        let mut rows = Vec::new();
        let mut next = offset;
        for record in records.iter().skip(offset as usize).take(5) {
            rows.push(cloudevent_row(
                &serde_json::from_slice(record).expect("an event"),
            ));
            next += 1;
        }
        Ok(Fetched {
            rows,
            next_offset: next,
            high_watermark: records.len() as u64,
        })
    }
}

struct Lakehouse {
    handle: HouseHandle,
    lake: Arc<Lake>,
    pool: WorkerPool,
    config: HouseConfig,
    stream: Arc<MemStream>,
}

impl Lakehouse {
    async fn start(test: &str) -> Lakehouse {
        let dir = tmp_root(&format!("{test}-bucket"));
        let url = format!("file://{}", dir.display());
        let store = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
        let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
        let upstream = Upstream::open(&url, StoreOptions::default(), RangeCacheConfig::default())
            .await
            .expect("upstream");
        let proxy = HouseCache::new(Arc::new(OneStore(Arc::new(upstream))));
        let launcher = ProcessLauncher::new(common::WORKER, tmp_root(test))
            .with_forward_target(Arc::new(proxy.clone()));
        let pool = WorkerPool::start(small(2), Arc::new(launcher))
            .await
            .expect("the pool starts");
        let lake = Arc::new(
            Lake::new(catalog as Arc<dyn HouseCatalog>, LakeRoot::local())
                .with_store(store)
                .with_grants(Arc::new(proxy) as Arc<dyn PrefixGrants>),
        );
        let config = HouseConfig {
            listen: "127.0.0.1:0".parse().expect("addr"),
            users: vec![UserMap::dev("meter", "m", NS, false)],
            tmp_dir: tmp_root(&format!("{test}-spool")),
            compaction_interval: Duration::ZERO,
            retention_interval: Duration::ZERO,
            pipe_interval: Duration::ZERO,
            ..HouseConfig::default()
        };
        let handle = serve_with_lake(config.clone(), pool.clone(), lake.clone())
            .await
            .expect("serves");
        Lakehouse {
            handle,
            lake,
            pool,
            config,
            stream: Arc::new(MemStream::default()),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    fn runner(&self) -> PipeRunner {
        PipeRunner::new(
            Arc::clone(&self.lake),
            self.pool.clone(),
            self.config.clone(),
            EngineLink {
                url: "http://engine.invalid".to_string(),
                key: None,
                namespaces: BTreeMap::from([(NS, "_system".to_string())]),
            },
            self.stream.clone() as Arc<dyn StreamSource>,
        )
    }

    /// Pipe rounds until one commits nothing (`loams_pipe_max_delay_ms` is
    /// waited out between rounds, so short batches commit too).
    async fn drain(&self) {
        let runner = self.runner();
        for _ in 0..50 {
            let mut busy = false;
            for pipe in pipes(&self.lake, NS).await.expect("pipes") {
                for view in &pipe.views {
                    if let Ran::Committed { .. } = runner.run(NS, &pipe, view).await.expect("run") {
                        busy = true;
                    }
                }
            }
            if !busy {
                return;
            }
        }
        panic!("the pipe never caught up");
    }

    /// Sets the schema up as the `meter` role does (`ensure_schema`).
    async fn schema(&self) {
        for statement in meter_sql::schema() {
            ok(self.addr(), &statement).await;
        }
    }

    /// Appends usage events to `_system/_meter`'s `partition`.
    fn push(&self, partition: usize, events: &[serde_json::Value]) {
        for event in events {
            self.stream.push(
                meter_sql::STREAM,
                partition,
                &serde_json::to_vec(event).expect("json"),
            );
        }
    }
}

async fn post(addr: SocketAddr, sql: &str) -> Response {
    let target = target(&[("user", "meter"), ("password", "m")]);
    let body = sql.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client")
}

async fn ok(addr: SocketAddr, sql: &str) -> String {
    let response = post(addr, sql).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.text()
}

/// A `house.query` usage event of §63 §6.2.
fn usage(id: &str, subject: &str, time: &str, cpu_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "specversion": "1.0",
        "id": id,
        "source": "loams://test/house/node-1",
        "type": "io.loams.dev.usage.house.query.v1",
        "subject": subject,
        "time": time,
        "datacontenttype": "application/json",
        "dataschema": "urn:loams:json:io.loams.dev.usage.house.query.v1",
        "tenantid": "42/7",
        "loamslease": "l9k2m4q8x1",
        "data": {"namespace": "7", "cpu_ms": cpu_ms, "bytes_scanned": 100, "statements": 1, "code": 0},
    })
}

/// The schema applies, and applies again unchanged (the `meter` role runs
/// it at every start).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meter_schema_applies_twice() {
    let house = Lakehouse::start("meter-schema").await;
    house.schema().await;
    house.schema().await;
    assert_eq!(
        ok(
            house.addr(),
            "SELECT name FROM system.tables WHERE database = '_meter' ORDER BY name"
        )
        .await,
        "events\nevents_mv\nevents_q\nwindows\n"
    );
}

/// `duplicate_event_counted_once_in_windows` (ME1 plan, Task 5; review
/// focus 1): the same event (`source` + `id`) appended twice in one batch,
/// again in a later batch, and on another partition, is one row of
/// `_meter.events FINAL` and counts once in its minute window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_event_counted_once_in_windows() {
    let house = Lakehouse::start("meter-dup").await;
    house.schema().await;
    let a = usage("q-1/1", "org_42", "2026-10-19T09:14:03.218734Z", 1840);
    let b = usage("q-2/1", "org_42", "2026-10-19T09:14:40Z", 160);
    let c = usage("q-3/1", "org_7", "2026-10-19T09:15:01Z", 5);
    // An emitter retry inside one batch, and a different subject.
    house.push(0, &[a.clone(), a.clone(), b.clone()]);
    house.push(1, &[c.clone()]);
    house.drain().await;
    // A re-emission after a crash, in a later batch, and a D270
    // `CommitUnknown` re-append landing on another partition.
    house.push(0, &[a.clone(), b.clone()]);
    house.push(1, &[a.clone()]);
    // A short batch waits for `loams_pipe_max_delay_ms` (2 s by default)
    // after the last commit.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    house.drain().await;

    let addr = house.addr();
    assert_eq!(
        ok(addr, "SELECT count() FROM _meter.events FINAL").await,
        "3\n"
    );
    // The CloudEvent's attributes and extensions land in their columns.
    assert_eq!(
        ok(
            addr,
            "SELECT subject, type, source, id, time, tenantid, lease, \
             JSONExtractUInt(data, 'cpu_ms') FROM _meter.events FINAL WHERE id = 'q-1/1'"
        )
        .await,
        "org_42\tio.loams.dev.usage.house.query.v1\tloams://test/house/node-1\tq-1/1\t\
         2026-10-19 09:14:03.218734\t42/7\tl9k2m4q8x1\t1840\n"
    );
    // The minute windows, as the close job aggregates them: every event once.
    assert_eq!(
        ok(
            addr,
            "SELECT subject, toDateTime(toStartOfMinute(time), 'UTC') AS w, count(), \
             sum(toDecimal128OrNull(JSON_VALUE(data, '$.cpu_ms'), 9)) \
             FROM _meter.events FINAL GROUP BY subject, w ORDER BY subject, w"
        )
        .await,
        "org_42\t2026-10-19 09:14:00\t2\t2000\n\
         org_7\t2026-10-19 09:15:00\t1\t5\n"
    );
}
