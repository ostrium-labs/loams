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

    /// One close run of `meter` at `now_ms`, driven as the `meter` role's
    /// `CloseJob` drives it.
    async fn close(&self, meter: &meter_sql::MeterDef, now_ms: i64) -> meter_sql::CloseSummary {
        let mut run =
            meter_sql::CloseRun::new(meter.clone(), now_ms, meter_sql::CloseConfig::default());
        let mut answer: Option<String> = None;
        while let Some(sql) = run.next(answer.as_deref()).expect("a readable answer") {
            answer = Some(ok(self.addr(), &sql).await);
        }
        run.summary().clone()
    }

    /// `meter`'s windows: size, subject, start, group key, value, events.
    async fn windows(&self, slug: &str) -> String {
        ok(
            self.addr(),
            &format!(
                "SELECT window_size, subject, window_start, group_key, value, events \
                 FROM _meter.windows FINAL WHERE meter = '{slug}' \
                 ORDER BY window_size, subject, window_start, group_key"
            ),
        )
        .await
    }

    /// One period run of (`subject`, `feature`) over `periods` at `now_ms`.
    async fn periods(
        &self,
        feature: &meter_sql::FeatureDef,
        periods: &[(i64, i64)],
        now_ms: i64,
    ) -> Vec<(i64, i64, meter_sql::PeriodOutcome)> {
        let mut run = meter_sql::PeriodRun::new(
            "org_42",
            feature.clone(),
            periods.to_vec(),
            now_ms,
            meter_sql::PeriodConfig::default(),
        );
        let mut answer: Option<String> = None;
        while let Some(sql) = run.next(answer.as_deref()).expect("a readable answer") {
            answer = Some(ok(self.addr(), &sql).await);
        }
        run.summary().periods.clone()
    }

    /// A meter query of `meter`, driven as `query_meter` drives it.
    async fn query(
        &self,
        meter: &meter_sql::MeterDef,
        spec: meter_sql::QuerySpec,
    ) -> meter_sql::QueryAnswer {
        let mut run = meter_sql::QueryRun::new(meter.clone(), spec);
        let mut answer: Option<String> = None;
        while let Some(sql) = run.next(answer.as_deref()).expect("a readable answer") {
            answer = Some(ok(self.addr(), &sql).await);
        }
        run.into_answer()
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

/// `sql` with Loams settings in the request, as `HouseSql::query_with`
/// sends them.
async fn ok_with(addr: SocketAddr, sql: &str, settings: &[(&str, &str)]) -> String {
    let mut params = vec![("user", "meter"), ("password", "m")];
    params.extend_from_slice(settings);
    let target = target(&params);
    let body = sql.as_bytes().to_vec();
    let response = tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client");
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.text()
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
        "close_runs\nevents\nevents_mv\nevents_q\nperiod_totals\nwindows\n"
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

/// 2026-10-19T10:00:00Z, in ms: "now" for the close runs below (minute
/// windows up to 09:58 close).
const NOW_MS: i64 = 1_792_404_000_000;

/// `house_cpu_ms`: `SUM` of `$.cpu_ms`, grouped by `$.namespace`.
fn cpu_meter() -> meter_sql::MeterDef {
    meter_sql::MeterDef {
        slug: "house_cpu_ms".to_string(),
        event_type: "io.loams.dev.usage.house.query.v1".to_string(),
        aggregation: meter_sql::Aggregation::Sum,
        value_property: Some("$.cpu_ms".to_string()),
        group_by: vec![("namespace".to_string(), "$.namespace".to_string())],
        event_from: None,
    }
}

/// A usage event with its own `data`.
fn event(id: &str, time: &str, data: serde_json::Value) -> serde_json::Value {
    let mut e = usage(id, "org_42", time, 0);
    e["data"] = data;
    e
}

/// The windows the duplicates of `duplicate_event_counted_once_in_windows`
/// close into, through the close job itself (ME1 Task 6).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicates_close_once() {
    let house = Lakehouse::start("meter-dup-close").await;
    house.schema().await;
    let a = usage("q-1/1", "org_42", "2026-10-19T09:14:03.218734Z", 1840);
    let b = usage("q-2/1", "org_42", "2026-10-19T09:14:40Z", 160);
    house.push(0, &[a.clone(), a.clone(), b.clone()]);
    house.push(1, &[a.clone(), b]);
    house.drain().await;
    let summary = house.close(&cpu_meter(), NOW_MS).await;
    assert_eq!((summary.minutes, summary.hours, summary.days), (1, 1, 1));
    assert_eq!(
        house.windows("house_cpu_ms").await,
        "minute\torg_42\t2026-10-19 09:14:00\t{\"namespace\":\"7\"}\t2000\t2\n\
         hour\torg_42\t2026-10-19 09:00:00\t{\"namespace\":\"7\"}\t2000\t2\n\
         day\torg_42\t2026-10-19 00:00:00\t{\"namespace\":\"7\"}\t2000\t2\n"
    );
}

/// `close_job_idempotent` (ME1 plan, Task 6): a run repeated, at the same
/// time or later, leaves the same windows; only the version moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_job_idempotent() {
    let house = Lakehouse::start("meter-idem").await;
    house.schema().await;
    house.push(
        0,
        &[
            usage("q-1/1", "org_42", "2026-10-19T09:14:03Z", 1840),
            usage("q-2/1", "org_42", "2026-10-19T09:59:30Z", 999),
        ],
    );
    house.push(1, &[usage("q-3/1", "org_7", "2026-10-19T09:15:01Z", 5)]);
    house.drain().await;
    let meter = cpu_meter();
    house.close(&meter, NOW_MS).await;
    let first = house.windows("house_cpu_ms").await;
    // 09:59 has not ended 2 minutes ago: it stays open.
    assert_eq!(
        first,
        "minute\torg_42\t2026-10-19 09:14:00\t{\"namespace\":\"7\"}\t1840\t1\n\
         minute\torg_7\t2026-10-19 09:15:00\t{\"namespace\":\"7\"}\t5\t1\n\
         hour\torg_42\t2026-10-19 09:00:00\t{\"namespace\":\"7\"}\t1840\t1\n\
         hour\torg_7\t2026-10-19 09:00:00\t{\"namespace\":\"7\"}\t5\t1\n\
         day\torg_42\t2026-10-19 00:00:00\t{\"namespace\":\"7\"}\t1840\t1\n\
         day\torg_7\t2026-10-19 00:00:00\t{\"namespace\":\"7\"}\t5\t1\n"
    );
    // The same run again, and a later one that closes nothing new.
    house.close(&meter, NOW_MS).await;
    assert_eq!(house.windows("house_cpu_ms").await, first);
    house.close(&meter, NOW_MS + 1).await;
    assert_eq!(house.windows("house_cpu_ms").await, first);
    assert_eq!(
        ok(house.addr(), "SELECT count() FROM _meter.close_runs FINAL").await,
        "1\n"
    );
    // A run a minute later closes 09:59 too (and recomputes its hour and day).
    house.close(&meter, NOW_MS + 120_000).await;
    let later = house.windows("house_cpu_ms").await;
    assert!(
        later.contains("minute\torg_42\t2026-10-19 09:59:00\t{\"namespace\":\"7\"}\t999\t1\n"),
        "{later}"
    );
    assert!(
        later.contains("hour\torg_42\t2026-10-19 09:00:00\t{\"namespace\":\"7\"}\t2839\t2\n"),
        "{later}"
    );
}

/// `late_event_recloses_window` (ME1 plan, Task 6): an event that reaches
/// the House after its minute closed re-closes the minute, its hour and its
/// day, with a newer version.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_event_recloses_window() {
    let house = Lakehouse::start("meter-late").await;
    house.schema().await;
    let mut meter = cpu_meter();
    meter.group_by.clear();
    house.push(0, &[usage("q-1/1", "org_42", "2026-10-19T09:14:03Z", 1840)]);
    house.drain().await;
    house.close(&meter, NOW_MS).await;
    assert_eq!(
        house.windows("house_cpu_ms").await,
        "minute\torg_42\t2026-10-19 09:14:00\t{}\t1840\t1\n\
         hour\torg_42\t2026-10-19 09:00:00\t{}\t1840\t1\n\
         day\torg_42\t2026-10-19 00:00:00\t{}\t1840\t1\n"
    );
    // 46 minutes late.
    house.push(0, &[usage("q-9/1", "org_42", "2026-10-19T09:14:50Z", 60)]);
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    house.drain().await;
    let later = NOW_MS + 60_000;
    let summary = house.close(&meter, later).await;
    assert_eq!(summary.minutes, 1);
    assert_eq!(
        house.windows("house_cpu_ms").await,
        "minute\torg_42\t2026-10-19 09:14:00\t{}\t1900\t2\n\
         hour\torg_42\t2026-10-19 09:00:00\t{}\t1900\t2\n\
         day\torg_42\t2026-10-19 00:00:00\t{}\t1900\t2\n"
    );
    assert_eq!(
        ok(
            house.addr(),
            "SELECT DISTINCT close_run FROM _meter.windows FINAL WHERE meter = 'house_cpu_ms'"
        )
        .await,
        format!("{later}\n")
    );
}

/// `unique_count_not_rolled_up` (ME1 plan, Task 6): a `UNIQUE_COUNT` hour is
/// the distinct values of the hour's events, not the sum of its minutes'.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unique_count_not_rolled_up() {
    let house = Lakehouse::start("meter-unique").await;
    house.schema().await;
    house.push(
        0,
        &[
            event(
                "u-1",
                "2026-10-19T09:14:10Z",
                serde_json::json!({"user": "u1"}),
            ),
            event(
                "u-2",
                "2026-10-19T09:14:20Z",
                serde_json::json!({"user": "u2"}),
            ),
            event(
                "u-3",
                "2026-10-19T09:15:10Z",
                serde_json::json!({"user": "u2"}),
            ),
            event(
                "u-4",
                "2026-10-19T09:15:20Z",
                serde_json::json!({"user": "u3"}),
            ),
            // No `user`: not counted.
            event("u-5", "2026-10-19T09:15:30Z", serde_json::json!({})),
        ],
    );
    house.drain().await;
    let meter = meter_sql::MeterDef {
        slug: "active_users".to_string(),
        event_type: "io.loams.dev.usage.house.query.v1".to_string(),
        aggregation: meter_sql::Aggregation::UniqueCount,
        value_property: Some("$.user".to_string()),
        group_by: Vec::new(),
        event_from: None,
    };
    house.close(&meter, NOW_MS).await;
    assert_eq!(
        house.windows("active_users").await,
        "minute\torg_42\t2026-10-19 09:14:00\t{}\t2\t2\n\
         minute\torg_42\t2026-10-19 09:15:00\t{}\t2\t3\n\
         hour\torg_42\t2026-10-19 09:00:00\t{}\t3\t5\n\
         day\torg_42\t2026-10-19 00:00:00\t{}\t3\t5\n"
    );
}

/// `period_closes_after_grace` (ME1 plan, Task 7): an hourly period of
/// `org_42`'s `house_cpu` is provisional until 2 hours after its end, then
/// closed; a late event re-closes it with the new total.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn period_closes_after_grace() {
    use meter_sql::PeriodOutcome::{AlreadyClosed, Closed, Provisional, Reclosed};
    const H09: i64 = 1_792_400_400;
    const H10: i64 = 1_792_404_000;
    const H11: i64 = 1_792_407_600;
    let house = Lakehouse::start("meter-period").await;
    house.schema().await;
    house.push(
        0,
        &[
            usage("q-1/1", "org_42", "2026-10-19T09:14:03Z", 1840),
            usage("q-2/1", "org_42", "2026-10-19T09:40:00Z", 160),
            // Another subject: not in org_42's total.
            usage("q-3/1", "org_7", "2026-10-19T09:41:00Z", 5),
        ],
    );
    house.drain().await;
    let meter = cpu_meter();
    house.close(&meter, NOW_MS).await;
    let feature = meter_sql::FeatureDef {
        key: "house_cpu".to_string(),
        meter: meter.clone(),
        filter: vec![("namespace".to_string(), "7".to_string())],
        byte_hours: false,
    };
    let periods = [(H09, H10), (H10, H11)];

    // 10:30 and 11:59:59: both periods provisional.
    for now_ms in [NOW_MS + 1_800_000, NOW_MS + 7_199_000] {
        assert_eq!(
            house.periods(&feature, &periods, now_ms).await,
            [(H09, H10, Provisional), (H10, H11, Provisional)]
        );
    }
    assert_eq!(
        period_totals(&house).await,
        "org_42\thouse_cpu\t2026-10-19 09:00:00\t2000\t2\t0\n\
         org_42\thouse_cpu\t2026-10-19 10:00:00\t0\t0\t0\n"
    );
    // 12:00, 2 hours after 09:00's period ended: it closes, once.
    assert_eq!(
        house.periods(&feature, &periods, NOW_MS + 7_200_000).await,
        [(H09, H10, Closed), (H10, H11, Provisional)]
    );
    assert_eq!(
        house.periods(&feature, &periods, NOW_MS + 7_260_000).await,
        [(H09, H10, AlreadyClosed), (H10, H11, Provisional)]
    );
    assert_eq!(
        period_totals(&house).await,
        "org_42\thouse_cpu\t2026-10-19 09:00:00\t2000\t2\t1\n\
         org_42\thouse_cpu\t2026-10-19 10:00:00\t0\t0\t0\n"
    );
    // A late event for 09:50: the close job re-closes its minute, and the
    // period closes again with the new total.
    house.push(0, &[usage("q-9/1", "org_42", "2026-10-19T09:50:00Z", 100)]);
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    house.drain().await;
    house.close(&meter, NOW_MS + 7_320_000).await;
    assert_eq!(
        house.periods(&feature, &periods, NOW_MS + 7_380_000).await,
        [(H09, H10, Reclosed), (H10, H11, Provisional)]
    );
    assert_eq!(
        period_totals(&house).await,
        "org_42\thouse_cpu\t2026-10-19 09:00:00\t2100\t3\t1\n\
         org_42\thouse_cpu\t2026-10-19 10:00:00\t0\t0\t0\n"
    );
}

/// `_meter.period_totals`, by period.
async fn period_totals(house: &Lakehouse) -> String {
    ok(
        house.addr(),
        "SELECT subject, feature, period_start, value, events, closed \
         FROM _meter.period_totals FINAL ORDER BY period_start",
    )
    .await
}

/// `query_marks_provisional_open_window` (ME1 plan, Task 8): a query reads
/// closed minutes from `_meter.windows`, the open edge from raw events, and
/// marks the windows past `as_of` provisional.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_marks_provisional_open_window() {
    const H09: i64 = 1_792_400_400;
    const H10: i64 = 1_792_404_000;
    let house = Lakehouse::start("meter-query").await;
    house.schema().await;
    house.push(
        0,
        &[
            usage("q-1/1", "org_42", "2026-10-19T09:14:03Z", 1840),
            // After 09:58: still open when the close runs at 10:00.
            usage("q-2/1", "org_42", "2026-10-19T09:59:30Z", 999),
            usage("q-3/1", "org_7", "2026-10-19T09:15:01Z", 5),
        ],
    );
    house.drain().await;
    let meter = cpu_meter();
    let spec = |window| meter_sql::QuerySpec {
        from: H09,
        to: H10,
        window,
        subjects: vec!["org_42".to_string()],
        by_subject: true,
        group_by: vec!["namespace".to_string()],
        filters: vec![("namespace".to_string(), vec!["7".to_string()])],
    };
    // Before any close run: everything from raw events, all provisional.
    let early = house
        .query(&meter, spec(meter_sql::QueryWindow::Hour))
        .await;
    assert_eq!(early.as_of, None);
    assert_eq!(early.rows.len(), 1);
    assert_eq!(early.rows[0].value, 2_839_000_000_000);
    assert!(early.rows[0].provisional);

    house.close(&meter, NOW_MS).await;
    let as_of = H10 - 120;
    // The hour straddles as_of: closed 09:14 plus raw 09:59, provisional.
    let hour = house
        .query(&meter, spec(meter_sql::QueryWindow::Hour))
        .await;
    assert_eq!(hour.as_of, Some(as_of));
    assert_eq!(
        hour.rows,
        [meter_sql::QueryRow {
            window_start: H09,
            window_end: H10,
            subject: Some("org_42".to_string()),
            group: vec!["7".to_string()],
            value: 2_839_000_000_000,
            provisional: true,
        }]
    );
    // By minute: 09:14 is closed, 09:59 provisional.
    let minutes = house
        .query(&meter, spec(meter_sql::QueryWindow::Minute))
        .await;
    let marks: Vec<_> = minutes
        .rows
        .iter()
        .map(|r| (r.window_start - H09, r.value / 1_000_000_000, r.provisional))
        .collect();
    assert_eq!(marks, [(14 * 60, 1840, false), (59 * 60, 999, true)]);
    // Over the whole range, without subjects: both subjects in one window.
    let mut all = spec(meter_sql::QueryWindow::Whole);
    all.subjects.clear();
    all.by_subject = false;
    all.group_by.clear();
    let whole = house.query(&meter, all).await;
    assert_eq!(whole.rows.len(), 1);
    assert_eq!(whole.rows[0].value, 2_844_000_000_000);
    assert_eq!(
        (whole.rows[0].window_start, whole.rows[0].window_end),
        (H09, H10)
    );
}

/// `system.loams_pipe_cursors`' answer for `_meter.events` and `partition`,
/// as the meter reads it (D2401): `(next_offset, snapshot_id)`, or `None`.
async fn cursor(house: &Lakehouse, partition: u32) -> Option<(u64, i64)> {
    let answer = ok(house.addr(), &meter_sql::pipe_cursor_sql(partition)).await;
    let line = answer.lines().find(|l| !l.trim().is_empty())?;
    let (next, snapshot) = line.split_once('\t').expect("two columns");
    Some((
        next.parse().expect("next_offset"),
        snapshot.parse().expect("snapshot_id"),
    ))
}

/// `pipe_cursor_and_pinned_totals_split_exactly` (ME1, D2401; §63 §12): the
/// two statements the meter's reconciliation runs, against a real House.
/// `system.loams_pipe_cursors` answers each partition's next offset with
/// the snapshot that holds exactly the records below it, and
/// `_meter.events FINAL` pinned to that snapshot sums those records only,
/// with a duplicate counted once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_cursor_and_pinned_totals_split_exactly() {
    const H09: i64 = 1_792_400_400;
    const H10: i64 = 1_792_404_000;
    let house = Lakehouse::start("meter-cursor").await;
    house.schema().await;
    // Nothing piped yet: no cursor.
    assert_eq!(cursor(&house, 0).await, None);

    house.push(
        0,
        &[
            usage("q-1/1", "org_42", "2026-10-19T09:14:03Z", 1840),
            usage("q-2/1", "org_42", "2026-10-19T09:40:00Z", 160),
            usage("q-3/1", "org_7", "2026-10-19T09:41:00Z", 5),
        ],
    );
    house.push(1, &[usage("q-4/1", "org_9", "2026-10-19T09:42:00Z", 7)]);
    house.drain().await;
    let (next0, first) = cursor(&house, 0).await.expect("partition 0's cursor");
    assert_eq!(next0, 3);
    let (next1, snapshot1) = cursor(&house, 1).await.expect("partition 1's cursor");
    assert_eq!(next1, 1);
    // One table, one current snapshot for all of its cursors.
    assert_eq!(snapshot1, first);
    // A partition the pipe never read has no row.
    assert_eq!(cursor(&house, 5).await, None);

    let sum = meter_sql::FeatureDef {
        key: "house_cpu".to_string(),
        meter: cpu_meter(),
        filter: vec![("namespace".to_string(), "7".to_string())],
        byte_hours: false,
    };
    let mut count = sum.clone();
    count.key = "house_queries".to_string();
    count.meter.slug = "house_queries".to_string();
    count.meter.aggregation = meter_sql::Aggregation::Count;
    count.meter.value_property = None;
    let addr = house.addr();
    let totals = |feature: meter_sql::FeatureDef, snapshot: i64| async move {
        let sql = meter_sql::pinned_totals_sql(&["org_42", "org_7"], &feature, H09, H10);
        // The snapshot goes with the request (`HouseSql::query_with`); a
        // SETTINGS clause would be chDB's, which refuses it (115).
        sorted(
            &ok_with(
                addr,
                &sql,
                &[(meter_sql::SNAPSHOT_SETTING, &snapshot.to_string())],
            )
            .await,
        )
    };
    assert_eq!(
        totals(sum.clone(), first).await,
        "org_42\t2000\norg_7\t5\n"
    );

    // A second batch: a new event and a re-emission of q-1/1.
    house.push(
        0,
        &[
            usage("q-5/1", "org_42", "2026-10-19T09:50:00Z", 100),
            usage("q-1/1", "org_42", "2026-10-19T09:14:03Z", 1840),
        ],
    );
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    house.drain().await;
    let (next0, second) = cursor(&house, 0).await.expect("partition 0's cursor");
    assert_eq!(next0, 5);
    assert_ne!(second, first, "the second batch is a new snapshot");

    // Pinned to the first snapshot: the first batch only.
    assert_eq!(
        totals(sum.clone(), first).await,
        "org_42\t2000\norg_7\t5\n"
    );
    assert_eq!(
        totals(count.clone(), first).await,
        "org_42\t2\norg_7\t1\n"
    );
    // Pinned to the second: both batches, the duplicate once.
    assert_eq!(
        totals(sum.clone(), second).await,
        "org_42\t2100\norg_7\t5\n"
    );
    assert_eq!(
        totals(count.clone(), second).await,
        "org_42\t3\norg_7\t1\n"
    );
}

/// The lines of a TSV answer, sorted (`GROUP BY` has no order).
fn sorted(answer: &str) -> String {
    let mut lines: Vec<&str> = answer.lines().collect();
    lines.sort_unstable();
    lines.iter().map(|l| format!("{l}\n")).collect()
}
