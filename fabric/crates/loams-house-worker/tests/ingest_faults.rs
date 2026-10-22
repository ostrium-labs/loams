//! Ingest fault evidence (HS1 Task 19): an acknowledged insert is never lost or
//! applied twice — whatever step a front dies at, while the catalog is down, while
//! the bucket fails in bursts, and while catalog commits time out — once the
//! client retries with its `insert_deduplication_token`.
//!
//! Faults are injected at the House's own seams (R19.1): the bucket is the
//! front's [`Store`] over a [`FaultyStore`], the catalog a [`Faults`] wrapper
//! around one [`MemCatalog`] that every front shares, as fronts share
//! Lakekeeper. A front "dies" by being stopped while one of its writes is parked
//! at the step under test, its client connection dropped, and the parked write
//! never resumed (or, for the paused-front half, resumed after the retry).

#![cfg(target_os = "linux")]

mod common;

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::http::{Response, request, target};
use common::{small, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{
    CatalogError, HouseCatalog, Lake, LakeRoot, LakeTable, MemCatalog, PrefixGrants,
};
use loams_house::config::{EngineLink, HouseConfig, UserMap};
use loams_house::errors::HouseError;
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::pipes::loams_stream::row;
use loams_house::pipes::{Bounds, Fetched, PipeRunner, Ran, StreamSource, pipes};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_lake::{NamespaceIdent, TableCreation, TableIdent, TableRequirement, TableUpdate};
use loams_store::{Fault, FaultRates, FaultyStore, Op, Store};
use tokio::sync::Notify;

const NS: u64 = 2;
const ALICE: (&str, &str) = ("alice", "a");

/// Where a commit through [`Faults`] stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Park {
    /// Before the catalog sees the commit.
    BeforeCommit,
    /// After the catalog applied it, before the front hears so.
    AfterCommit,
}

/// One parked commit: the test hears it arrive and decides whether it resumes.
#[derive(Debug, Default)]
struct Gate {
    reached: Notify,
    release: Notify,
    /// Set once the parked commit, released, has had the catalog's answer.
    done: Notify,
}

/// The catalog every front of a test shares, with its faults:
/// * `gate`: the next commit parks at a [`Park`] (once);
/// * `down`: every call is refused as unreachable (an outage);
/// * `land_then_down`: the next commit applies, and the outage begins before
///   its answer;
/// * `slow`: the next commits time out at the front after `timeout`, as the
///   REST client's timeout does, and land `late` after that unless `lost`.
#[derive(Debug)]
struct Faults {
    inner: Arc<MemCatalog>,
    gate: Mutex<Option<(Park, Arc<Gate>)>>,
    down: AtomicBool,
    land_then_down: AtomicBool,
    slow: AtomicU32,
    slow_lost: AtomicBool,
    timeout: Duration,
    late: Duration,
    commits: AtomicU32,
}

impl Faults {
    fn new(inner: Arc<MemCatalog>) -> Self {
        Self {
            inner,
            gate: Mutex::new(None),
            down: AtomicBool::new(false),
            land_then_down: AtomicBool::new(false),
            slow: AtomicU32::new(0),
            slow_lost: AtomicBool::new(false),
            timeout: Duration::from_millis(250),
            late: Duration::from_millis(150),
            commits: AtomicU32::new(0),
        }
    }

    fn arm(&self, park: Park) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        *self.gate.lock().expect("lock") = Some((park, Arc::clone(&gate)));
        gate
    }

    fn up(&self) -> Result<(), CatalogError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(CatalogError::Unavailable(
                "injected: the catalog is down".to_string(),
            ));
        }
        Ok(())
    }
}

fn timed_out() -> CatalogError {
    CatalogError::Unavailable("injected: the catalog's answer timed out".to_string())
}

#[async_trait]
impl HouseCatalog for Faults {
    async fn list_root_databases(&self) -> Result<Vec<NamespaceIdent>, CatalogError> {
        self.up()?;
        self.inner.list_root_databases().await
    }
    async fn list_databases(
        &self,
        parent: &NamespaceIdent,
    ) -> Result<Vec<NamespaceIdent>, CatalogError> {
        self.up()?;
        self.inner.list_databases(parent).await
    }
    async fn create_database(
        &self,
        namespace: &NamespaceIdent,
        properties: HashMap<String, String>,
    ) -> Result<(), CatalogError> {
        self.up()?;
        self.inner.create_database(namespace, properties).await
    }
    async fn database_properties(
        &self,
        namespace: &NamespaceIdent,
    ) -> Result<HashMap<String, String>, CatalogError> {
        self.up()?;
        self.inner.database_properties(namespace).await
    }
    async fn drop_database(&self, namespace: &NamespaceIdent) -> Result<(), CatalogError> {
        self.up()?;
        self.inner.drop_database(namespace).await
    }
    async fn list_tables(
        &self,
        namespace: &NamespaceIdent,
    ) -> Result<Vec<TableIdent>, CatalogError> {
        self.up()?;
        self.inner.list_tables(namespace).await
    }
    async fn load_table(&self, table: &TableIdent) -> Result<LakeTable, CatalogError> {
        self.up()?;
        self.inner.load_table(table).await
    }
    async fn create_table(
        &self,
        namespace: &NamespaceIdent,
        creation: TableCreation,
    ) -> Result<LakeTable, CatalogError> {
        self.up()?;
        self.inner.create_table(namespace, creation).await
    }
    async fn drop_table(&self, table: &TableIdent) -> Result<(), CatalogError> {
        self.up()?;
        self.inner.drop_table(table).await
    }
    async fn rename_table(&self, from: &TableIdent, to: &TableIdent) -> Result<(), CatalogError> {
        self.up()?;
        self.inner.rename_table(from, to).await
    }
    async fn commit(
        &self,
        table: &TableIdent,
        requirements: Vec<TableRequirement>,
        updates: Vec<TableUpdate>,
    ) -> Result<LakeTable, CatalogError> {
        self.up()?;
        self.commits.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().expect("lock").take();
        if let Some((park, gate)) = gate {
            if park == Park::BeforeCommit {
                gate.reached.notify_one();
                gate.release.notified().await;
            }
            let result = self.inner.commit(table, requirements, updates).await;
            if park == Park::AfterCommit {
                gate.reached.notify_one();
                gate.release.notified().await;
            }
            gate.done.notify_one();
            return result;
        }
        if self.land_then_down.swap(false, Ordering::SeqCst) {
            self.inner.commit(table, requirements, updates).await?;
            self.down.store(true, Ordering::SeqCst);
            return Err(timed_out());
        }
        let slow = self
            .slow
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if slow {
            tokio::time::sleep(self.timeout).await;
            if !self.slow_lost.load(Ordering::SeqCst) {
                // The catalog still applies it, after the front gave up (and
                // perhaps after the front's retry: then its requirement fails).
                let inner = Arc::clone(&self.inner);
                let table = table.clone();
                let late = self.late;
                tokio::spawn(async move {
                    tokio::time::sleep(late).await;
                    let _ = inner.commit(&table, requirements, updates).await;
                });
            }
            return Err(timed_out());
        }
        self.inner.commit(table, requirements, updates).await
    }
    async fn register_table(
        &self,
        table: &TableIdent,
        metadata_location: &str,
    ) -> Result<LakeTable, CatalogError> {
        self.up()?;
        self.inner.register_table(table, metadata_location).await
    }
}

/// A stream in memory, one stream name, records per partition (the engine's
/// stream API's stand-in, as in `pipes.rs`).
#[derive(Debug, Default)]
struct MemStream(Mutex<Vec<Vec<Vec<u8>>>>);

impl MemStream {
    fn push(&self, partition: usize, record: Vec<u8>) {
        let mut parts = self.0.lock().expect("lock");
        while parts.len() <= partition {
            parts.push(Vec::new());
        }
        parts[partition].push(record);
    }
}

#[async_trait]
impl StreamSource for MemStream {
    async fn partitions(&self, _namespace: &str, _stream: &str) -> Result<Vec<Bounds>, HouseError> {
        Ok(self
            .0
            .lock()
            .expect("lock")
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
        _stream: &str,
        partition: u32,
        offset: u64,
        _max_bytes: usize,
        _events: bool,
    ) -> Result<Fetched, HouseError> {
        let parts = self.0.lock().expect("lock");
        let records = &parts[partition as usize];
        let rows: Vec<_> = records
            .iter()
            .skip(offset as usize)
            .take(4)
            .map(|r| row(r.clone()))
            .collect();
        Ok(Fetched {
            next_offset: offset + rows.len() as u64,
            rows,
            high_watermark: records.len() as u64,
        })
    }
}

/// What every front of a test shares: the bucket, the catalog, the workers and
/// `house-cache`.
struct Bed {
    bucket: Arc<FaultyStore>,
    store: Store,
    faults: Arc<Faults>,
    pool: WorkerPool,
    proxy: HouseCache,
    config: HouseConfig,
}

/// One front over the [`Bed`].
struct Front {
    handle: HouseHandle,
    lake: Arc<Lake>,
}

impl Front {
    fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// The front dies: it takes no more connections, and what it has parked
    /// stays parked.
    fn kill(self) {
        self.handle.shutdown();
    }
}

impl Bed {
    async fn new(test: &str) -> Self {
        let dir = tmp_root(&format!("{test}-bucket"));
        let url = format!("file://{}", dir.display());
        let plain = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
        let bucket = Arc::new(FaultyStore::new(Arc::clone(plain.inner())));
        let store = Store::new(bucket.clone());
        let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
        let upstream = Upstream::open(&url, StoreOptions::default(), RangeCacheConfig::default())
            .await
            .expect("upstream");
        let proxy = HouseCache::new(Arc::new(OneStore(Arc::new(upstream))));
        let launcher = ProcessLauncher::new(common::WORKER, tmp_root(test))
            .with_forward_target(Arc::new(proxy.clone()));
        let pool = WorkerPool::start(small(3), Arc::new(launcher))
            .await
            .expect("the pool starts");
        let config = HouseConfig {
            listen: "127.0.0.1:0".parse().expect("addr"),
            users: vec![UserMap::dev(ALICE.0, ALICE.1, NS, false)],
            tmp_dir: tmp_root(&format!("{test}-spool")),
            compaction_interval: Duration::ZERO,
            retention_interval: Duration::ZERO,
            pipe_interval: Duration::ZERO,
            view_interval: Duration::ZERO,
            // The faults count catalog commits: the query log's own group
            // commits (HS1 Task 23) would take some of them.
            query_log_interval: Duration::ZERO,
            ..HouseConfig::default()
        };
        Self {
            bucket,
            store,
            faults: Arc::new(Faults::new(catalog)),
            pool,
            proxy,
            config,
        }
    }

    async fn front(&self) -> Front {
        let lake = Arc::new(
            Lake::new(
                self.faults.clone() as Arc<dyn HouseCatalog>,
                LakeRoot::local(),
            )
            .with_store(self.store.clone())
            .with_grants(Arc::new(self.proxy.clone()) as Arc<dyn PrefixGrants>),
        );
        let handle = serve_with_lake(self.config.clone(), self.pool.clone(), lake.clone())
            .await
            .expect("serves");
        Front { handle, lake }
    }

    fn runner(&self, front: &Front, stream: &Arc<MemStream>) -> PipeRunner {
        PipeRunner::new(
            Arc::clone(&front.lake),
            self.pool.clone(),
            self.config.clone(),
            EngineLink {
                url: "http://engine.invalid".to_string(),
                key: None,
                namespaces: BTreeMap::from([(NS, "engine-ns".to_string())]),
            },
            stream.clone() as Arc<dyn StreamSource>,
        )
    }
}

fn query_target(query: Option<&str>, params: &[(&str, &str)]) -> String {
    let mut all = vec![("user", ALICE.0), ("password", ALICE.1)];
    if let Some(query) = query {
        all.push(("query", query));
    }
    all.extend_from_slice(params);
    target(&all)
}

async fn post(
    addr: SocketAddr,
    query: Option<&str>,
    body: &[u8],
    params: &[(&str, &str)],
) -> Response {
    let target = query_target(query, params);
    let body = body.to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &[], &body))
        .await
        .expect("client")
}

async fn ok(addr: SocketAddr, sql: &str) -> String {
    let response = post(addr, None, sql.as_bytes(), &[]).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response.text()
}

/// Sends an `INSERT` and leaves its answer unread: the connection is the
/// client's side of a request whose front is about to die.
fn send_unanswered(
    addr: SocketAddr,
    query: &str,
    data: &[u8],
    params: &[(&str, &str)],
) -> TcpStream {
    let target = query_target(Some(query), params);
    let mut stream = TcpStream::connect(addr).expect("connect");
    let head = format!(
        "POST {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        data.len()
    );
    stream.write_all(head.as_bytes()).expect("head");
    stream.write_all(data).expect("body");
    stream
}

fn written_rows(response: &Response) -> u64 {
    let summary: serde_json::Value =
        serde_json::from_str(response.header("X-ClickHouse-Summary").expect("summary"))
            .expect("json");
    summary["written_rows"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .expect("written_rows")
}

/// Retries an `INSERT` with its token until it is answered `200`, as a client
/// does; the codes of the failed tries.
async fn insert_until_ok(
    addr: SocketAddr,
    data: &[u8],
    token: &str,
    tries: usize,
) -> (Response, Vec<String>) {
    let params = [("insert_deduplication_token", token)];
    let mut codes = Vec::new();
    for _ in 0..tries {
        let response = post(addr, Some("INSERT INTO t FORMAT CSV"), data, &params).await;
        if response.status == 200 {
            return (response, codes);
        }
        codes.push(
            response
                .header("X-ClickHouse-Exception-Code")
                .unwrap_or("none")
                .to_string(),
        );
    }
    panic!("{token} never landed in {tries} tries: {codes:?}");
}

/// Three rows `base`, `base + 1`, `base + 2` as CSV.
fn rows(base: u64) -> Vec<u8> {
    (base..base + 3)
        .map(|k| format!("{k},r{k}\n"))
        .collect::<String>()
        .into_bytes()
}

async fn wait(what: &str, notify: &Notify) {
    tokio::time::timeout(Duration::from_secs(60), notify.notified())
        .await
        .unwrap_or_else(|_| panic!("{what} never happened"));
}

const TABLE: &str = "CREATE TABLE t (k UInt64, s String) ENGINE = MergeTree ORDER BY k";

/// Where `kill_front_during_commit_matrix` stops a front.
#[derive(Clone, Copy, Debug)]
enum Death {
    /// While its data file is being uploaded.
    Upload,
    /// With the files uploaded, before the catalog sees the commit.
    BeforeCommit,
    /// After the catalog applied the commit, before the client was answered.
    AfterCommit,
    /// After the client was answered (and lost the answer, so retries).
    AfterAck,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_front_during_commit_matrix() {
    let bed = Bed::new("ingest-kill").await;
    let first = bed.front().await;
    ok(first.addr(), TABLE).await;
    first.kill();
    for (i, death) in [
        Death::Upload,
        Death::BeforeCommit,
        Death::AfterCommit,
        Death::AfterAck,
    ]
    .into_iter()
    .enumerate()
    {
        let base = 100 * i as u64;
        let data = rows(base);
        let token = format!("kill-{i}");
        let params = [("insert_deduplication_token", token.as_str())];
        let dying = bed.front().await;
        let mut gate = None;
        match death {
            Death::Upload => {
                // The next PUT, the insert's data file, hangs as a front that
                // died mid-upload does.
                let puts = bed.bucket.calls(Op::Put);
                bed.bucket
                    .inject(Op::Put, Fault::Delay(Duration::from_secs(3600)));
                let client =
                    send_unanswered(dying.addr(), "INSERT INTO t FORMAT CSV", &data, &params);
                let start = Instant::now();
                while bed.bucket.calls(Op::Put) == puts {
                    assert!(start.elapsed() < Duration::from_secs(60), "no upload");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                assert_eq!(bed.bucket.pending(Op::Put), 0, "the upload took the fault");
                drop(client);
            }
            Death::BeforeCommit | Death::AfterCommit => {
                let park = if matches!(death, Death::BeforeCommit) {
                    Park::BeforeCommit
                } else {
                    Park::AfterCommit
                };
                let parked = bed.faults.arm(park);
                let client =
                    send_unanswered(dying.addr(), "INSERT INTO t FORMAT CSV", &data, &params);
                wait(&format!("{death:?}"), &parked.reached).await;
                drop(client);
                gate = Some(parked);
            }
            Death::AfterAck => {
                let response = post(
                    dying.addr(),
                    Some("INSERT INTO t FORMAT CSV"),
                    &data,
                    &params,
                )
                .await;
                assert_eq!(response.status, 200, "{}", response.text());
                assert_eq!(written_rows(&response), 3);
            }
        }
        dying.kill();

        // The client retries on another front with its token.
        let next = bed.front().await;
        let (retry, failed) = insert_until_ok(next.addr(), &data, &token, 3).await;
        assert!(failed.is_empty(), "{death:?}: {failed:?}");
        let landed_before = matches!(death, Death::AfterCommit | Death::AfterAck);
        assert_eq!(
            written_rows(&retry),
            if landed_before { 0 } else { 3 },
            "{death:?}"
        );
        let check = format!(
            "SELECT count(), uniqExact(k) FROM t WHERE k >= {base} AND k < {}",
            base + 100
        );
        assert_eq!(ok(next.addr(), &check).await, "3\t3\n", "after {death:?}");

        // A front that was only paused, not killed, resumes after the retry:
        // its commit's requirement fails (or it had landed), and it reloads to
        // find its token held. Still once.
        if let Some(gate) = gate {
            gate.release.notify_one();
            wait("the paused commit's answer", &gate.done).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(ok(next.addr(), &check).await, "3\t3\n", "{death:?} resumed");
        }
        next.kill();
    }
    let last = bed.front().await;
    assert_eq!(
        ok(last.addr(), "SELECT count(), uniqExact(k) FROM t").await,
        "12\t12\n"
    );
}

/// `k` of every record pushed, minus the ones the view filters out.
fn expected(records: u64) -> String {
    let kept = (0..records).filter(|k| k % 10 != 9).count();
    format!("{kept}\t{kept}\n")
}

fn order(k: u64) -> Vec<u8> {
    format!("{{\"k\": {k}, \"v\": \"item{k}\"}}").into_bytes()
}

/// Rounds of `runner` until one commits nothing.
async fn drain(front: &Front, runner: &PipeRunner) {
    for _ in 0..50 {
        let mut busy = false;
        for pipe in pipes(&front.lake, NS).await.expect("pipes") {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_outage_during_pipe() {
    let bed = Bed::new("ingest-outage").await;
    let front = bed.front().await;
    let addr = front.addr();
    ok(
        addr,
        "CREATE TABLE q (k UInt64, v String) ENGINE = LoamsStream('orders', 'JSONEachRow') \
         SETTINGS loams_pipe_max_rows = 1000, loams_pipe_max_delay_ms = 1",
    )
    .await;
    ok(
        addr,
        "CREATE TABLE t (k UInt64, v String) ENGINE = MergeTree ORDER BY k",
    )
    .await;
    ok(
        addr,
        "CREATE MATERIALIZED VIEW mv TO t AS SELECT k, upper(v) AS v FROM q WHERE k % 10 != 9",
    )
    .await;
    let stream = Arc::new(MemStream::default());
    let runner = bed.runner(&front, &stream);
    let pipe = pipes(&front.lake, NS).await.expect("pipes").remove(0);
    let view = pipe.views[0].clone();
    for k in 0..20 {
        stream.push((k % 2) as usize, order(k));
    }

    // The catalog goes down: the pipe stalls, visibly, and writes nothing.
    bed.faults.down.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        assert!(runner.run(NS, &pipe, &view).await.is_err());
    }
    let (_, state) = front.lake.pipe_states().all().remove(0);
    assert!(state.last_error.contains("catalog"), "{}", state.last_error);
    assert_eq!(state.committed_records, 0);
    // It comes back: the pipe resumes from its cursor, nothing lost.
    bed.faults.down.store(false, Ordering::SeqCst);
    drain(&front, &runner).await;
    assert_eq!(
        ok(addr, "SELECT count(), uniqExact(k) FROM t").await,
        expected(20)
    );
    let (_, state) = front.lake.pipe_states().all().remove(0);
    assert!(state.last_error.is_empty(), "{}", state.last_error);

    // The outage begins mid-commit: the rows and the cursor land, the runner
    // never hears so, and neither it nor another front's runner can look.
    for k in 20..40 {
        stream.push((k % 2) as usize, order(k));
    }
    bed.faults.land_then_down.store(true, Ordering::SeqCst);
    assert!(runner.run(NS, &pipe, &view).await.is_err());
    assert!(bed.faults.down.load(Ordering::SeqCst), "the commit landed");
    let other = bed.front().await;
    let other_runner = bed.runner(&other, &stream);
    assert!(other_runner.run(NS, &pipe, &view).await.is_err());
    // Back up: both runners find the cursor moved; no record twice.
    bed.faults.down.store(false, Ordering::SeqCst);
    drain(&other, &other_runner).await;
    drain(&front, &runner).await;
    assert_eq!(
        ok(addr, "SELECT count(), uniqExact(k) FROM t").await,
        expected(40)
    );

    // A longer stream through an outage that comes and goes between rounds.
    for k in 40..80 {
        stream.push((k % 2) as usize, order(k));
    }
    for round in 0..12 {
        bed.faults.down.store(round % 3 == 0, Ordering::SeqCst);
        let runner = if round % 2 == 0 {
            &runner
        } else {
            &other_runner
        };
        let _ = runner.run(NS, &pipe, &view).await;
    }
    bed.faults.down.store(false, Ordering::SeqCst);
    drain(&front, &runner).await;
    assert_eq!(
        ok(addr, "SELECT count(), uniqExact(k) FROM t").await,
        expected(80)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bucket_503_bursts_during_insert() {
    let bed = Bed::new("ingest-bursts").await;
    let front = bed.front().await;
    let addr = front.addr();
    ok(addr, TABLE).await;

    // Bursts: for 150 ms of every 300 ms, a third of the bucket's calls fail
    // before they apply and a sixth after (the object was written, its answer
    // lost), as when a store sheds load with 503s past the client's own retries.
    let stop = Arc::new(AtomicBool::new(false));
    let bursts = {
        let bucket = Arc::clone(&bed.bucket);
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let burst = FaultRates {
                error: 0.33,
                error_after_apply: 0.17,
                ..FaultRates::none()
            };
            while !stop.load(Ordering::SeqCst) {
                bucket.set_rates(burst);
                tokio::time::sleep(Duration::from_millis(150)).await;
                bucket.set_rates(FaultRates::none());
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
    };
    let writers: Vec<_> = (0..4u64)
        .map(|w| {
            tokio::spawn(async move {
                let mut failures = Vec::new();
                for i in 0..8u64 {
                    let base = 1000 * w + 10 * i;
                    let token = format!("burst-{w}-{i}");
                    let (_, failed) = insert_until_ok(addr, &rows(base), &token, 60).await;
                    failures.extend(failed);
                }
                failures
            })
        })
        .collect();
    let mut failures = Vec::new();
    for writer in writers {
        failures.extend(writer.await.expect("writer"));
    }
    stop.store(true, Ordering::SeqCst);
    bursts.await.expect("bursts");
    bed.bucket.set_rates(FaultRates::none());

    assert!(!failures.is_empty(), "the bursts reached the inserts");
    // A failed insert says the store failed (`210`), never anything else.
    assert!(failures.iter().all(|code| code == "210"), "{failures:?}");
    // Every insert once, whatever its tries saw.
    assert_eq!(
        ok(addr, "SELECT count(), uniqExact(k) FROM t").await,
        "96\t96\n"
    );
    let reader = bed.front().await;
    assert_eq!(
        ok(reader.addr(), "SELECT count(), uniqExact(k) FROM t").await,
        "96\t96\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_catalog_commit_timeouts_are_retried() {
    let bed = Bed::new("ingest-slow").await;
    let front = bed.front().await;
    let addr = front.addr();
    ok(addr, TABLE).await;

    // One commit times out and lands late: the front's retry finds it (or its
    // own retry loses to it and then finds it), and answers once.
    let commits = bed.faults.commits.load(Ordering::SeqCst);
    bed.faults.slow.store(1, Ordering::SeqCst);
    let (response, failed) = insert_until_ok(addr, &rows(0), "slow-1", 1).await;
    assert!(failed.is_empty());
    assert_eq!(written_rows(&response), 3);
    assert!(
        bed.faults.commits.load(Ordering::SeqCst) >= commits + 2,
        "retried"
    );

    // Several time out in a row, each landing late: one lands, the others'
    // requirements fail.
    bed.faults.slow.store(3, Ordering::SeqCst);
    let (response, _) = insert_until_ok(addr, &rows(10), "slow-2", 1).await;
    assert_eq!(written_rows(&response), 3);

    // Timeouts that never land: the front retries until one does.
    bed.faults.slow_lost.store(true, Ordering::SeqCst);
    bed.faults.slow.store(3, Ordering::SeqCst);
    let (response, _) = insert_until_ok(addr, &rows(20), "slow-3", 1).await;
    assert_eq!(written_rows(&response), 3);
    bed.faults.slow_lost.store(false, Ordering::SeqCst);

    // Concurrent writers through a slow catalog that lands late.
    bed.faults.slow.store(8, Ordering::SeqCst);
    let writers: Vec<_> = (0..4u64)
        .map(|w| {
            tokio::spawn(async move {
                let token = format!("slow-w{w}");
                insert_until_ok(addr, &rows(100 + 10 * w), &token, 1)
                    .await
                    .0
            })
        })
        .collect();
    for writer in writers {
        assert_eq!(written_rows(&writer.await.expect("writer")), 3);
    }
    // Late commits still in flight settle.
    tokio::time::sleep(bed.faults.late * 2).await;

    // A catalog slower than the front's patience: the client hears 210, and its
    // retry with the token lands the rows once.
    bed.faults.slow_lost.store(true, Ordering::SeqCst);
    bed.faults
        .slow
        .store(loams_house::commit::MAX_ATTEMPTS, Ordering::SeqCst);
    let params = [("insert_deduplication_token", "slow-4")];
    let gave_up = post(addr, Some("INSERT INTO t FORMAT CSV"), &rows(200), &params).await;
    assert_eq!(
        gave_up.header("X-ClickHouse-Exception-Code"),
        Some("210"),
        "{}",
        gave_up.text()
    );
    bed.faults.slow_lost.store(false, Ordering::SeqCst);
    let (response, _) = insert_until_ok(addr, &rows(200), "slow-4", 1).await;
    assert_eq!(written_rows(&response), 3);

    assert_eq!(
        ok(addr, "SELECT count(), uniqExact(k) FROM t").await,
        "24\t24\n"
    );
}
