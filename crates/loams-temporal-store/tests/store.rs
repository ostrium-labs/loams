//! The store's contract (the conformance suite), run against every backend:
//! `MemStore` always, and `PgStore` when `LOAMS_TEMPORAL_TEST_PG_URL` (or its
//! alias `LOAMS_TEMPORAL_PG_URL`) names a Postgres server whose user may
//! create databases. Each Postgres leg runs in a fresh database it creates
//! and drops. A new backend joins by adding a `Backend` variant: every test
//! below then runs against it.
//!
//! ```text
//! LOAMS_TEMPORAL_TEST_PG_URL=postgres://postgres:pg@127.0.0.1:5432/postgres \
//!     cargo test -p loams-temporal-store
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use loams_temporal_store::{
    Commit, EventRow, ListRuns, MemStore, NamespaceRecord, OutboxInsert, PgStore, PgStoreOptions,
    PurgeLimits, PurgeReport, RunFilter, RunKey, STATUS_RUNNING, StartExecution, StartOutcome,
    Store, StoreError, VisibilityRow, retention, shard_of,
};
use serde_json::json;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection};

const ENV: &str = "LOAMS_TEMPORAL_TEST_PG_URL";
const ENV_ALIAS: &str = "LOAMS_TEMPORAL_PG_URL";

fn pg_url() -> Option<String> {
    [ENV, ENV_ALIAS]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|u| !u.is_empty()))
}
const COMPLETED: i32 = 2;
const NS: &str = "ns-1";

enum Backend {
    Mem(Arc<MemStore>),
    Pg(Arc<PgStore>, Box<TestDb>),
}

struct TestDb {
    admin: PgConnectOptions,
    opts: PgConnectOptions,
    name: String,
}

impl TestDb {
    async fn create(test: &str) -> Option<Self> {
        let url = pg_url()?;
        static N: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "lts_{test}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        );
        let admin = PgConnectOptions::from_str(&url).expect("the Postgres test URL");
        let mut c = PgConnection::connect_with(&admin).await.expect("connect");
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut c)
            .await
            .expect("create database");
        c.close().await.ok();
        let opts = admin.clone().database(&name);
        Some(Self { admin, opts, name })
    }

    async fn drop_db(self) {
        let mut c = PgConnection::connect_with(&self.admin)
            .await
            .expect("connect");
        sqlx::query(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut c)
        .await
        .expect("drop database");
        c.close().await.ok();
    }
}

impl Backend {
    fn store(&self) -> Arc<dyn Store> {
        match self {
            Backend::Mem(s) => s.clone(),
            Backend::Pg(s, _) => s.clone(),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Backend::Mem(_) => "mem",
            Backend::Pg(..) => "pg",
        }
    }

    fn fail_next_commit(&self) {
        match self {
            Backend::Mem(s) => s.fail_next_commit(),
            Backend::Pg(s, _) => s.fail_next_commit(),
        }
    }

    async fn done(self) {
        if let Backend::Pg(s, db) = self {
            s.close().await;
            drop(s);
            db.drop_db().await;
        }
    }
}

/// A fresh MemStore, and a PgStore on a fresh, migrated database when the
/// environment names a server.
async fn backends(test: &str) -> Vec<Backend> {
    let mut out = vec![Backend::Mem(Arc::new(MemStore::new(64).unwrap()))];
    if let Some(db) = TestDb::create(test).await {
        let s = PgStore::connect_with(db.opts.clone(), 64).await.unwrap();
        s.migrate().await.unwrap();
        out.push(Backend::Pg(Arc::new(s), Box::new(db)));
    } else {
        eprintln!("{test}: neither {ENV} nor {ENV_ALIAS} is set; Postgres leg skipped");
    }
    out
}

fn key(wf: &str, run: &str) -> RunKey {
    RunKey::new(NS, wf, run)
}

fn ev(id: i64, batch_first: i64) -> EventRow {
    EventRow {
        event_id: id,
        batch_first_event_id: batch_first,
        event_type: (id % 40) as i32,
        blob: format!("event-{id}").into_bytes(),
    }
}

/// `n` events from `first`, in one batch.
fn events(first: i64, n: i64) -> Vec<EventRow> {
    (first..first + n).map(|id| ev(id, first)).collect()
}

fn vis(status: i32, close: Option<i64>) -> VisibilityRow {
    VisibilityRow {
        workflow_type: "hello".into(),
        status,
        start_time_ms: 1_761_300_000_123,
        close_time_ms: close,
        execution_time_ms: 1_761_300_000_456,
        task_queue: "tq".into(),
        memo: Some(vec![1, 2, 0, 3]),
        search_attrs: json!({"CustomKeyword": "x"}),
    }
}

fn outbox(n: usize, tag: &str) -> Vec<OutboxInsert> {
    (0..n)
        .map(|i| OutboxInsert {
            kind: "task_create".into(),
            resonate_id: format!("wft:{tag}:{i}"),
            request: json!({"tag": tag, "i": i}),
        })
        .collect()
}

fn start(k: &RunKey, req: &str, n_events: i64, n_outbox: usize) -> StartExecution {
    StartExecution {
        key: k.clone(),
        request_id: req.into(),
        commit: Commit {
            events: events(1, n_events),
            state_blob: b"state-1".to_vec(),
            status: STATUS_RUNNING,
            next_event_id: 1 + n_events,
            outbox: outbox(n_outbox, &k.workflow_id),
            visibility: Some(vis(STATUS_RUNNING, None)),
        },
    }
}

fn step(first: i64, n: i64, status: i32, state: &str) -> Commit {
    Commit {
        events: events(first, n),
        state_blob: state.as_bytes().to_vec(),
        status,
        next_event_id: first + n,
        outbox: Vec::new(),
        visibility: None,
    }
}

async fn started(s: &dyn Store, st: StartExecution) {
    assert_eq!(
        s.start(st).await.unwrap(),
        StartOutcome::Started { version: 1 }
    );
}

/// Claims until nothing is left.
async fn drain(s: &dyn Store, owner: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    loop {
        let all: Vec<u32> = (0..64).collect();
        let rows = s
            .claim_outbox(&all, 4, Duration::from_secs(60), owner)
            .await
            .unwrap();
        if rows.is_empty() {
            return ids;
        }
        ids.extend(rows.iter().map(|r| r.id));
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn cas_rejects_stale_version() {
    for b in backends("cas").await {
        let s = b.store();
        let k = key("wf-cas", "run-1");
        started(&*s, start(&k, "req-1", 2, 0)).await;
        assert_eq!(
            s.commit(&k, 1, step(3, 2, STATUS_RUNNING, "s2"))
                .await
                .unwrap(),
            2
        );
        let err = s.commit(&k, 1, step(5, 1, STATUS_RUNNING, "stale")).await;
        assert_eq!(
            err,
            Err(StoreError::Conflict {
                expected: 1,
                actual: 2
            }),
            "{}",
            b.name()
        );
        let l = s.load(&k).await.unwrap().unwrap();
        assert_eq!((l.version, l.next_event_id), (2, 5));
        assert_eq!(l.state_blob, b"s2");
        assert_eq!(s.history(&k, 1, 100).await.unwrap().len(), 4);
        // A state-only commit (no events) moves the version too.
        assert_eq!(
            s.commit(&k, 2, step(5, 0, STATUS_RUNNING, "s3"))
                .await
                .unwrap(),
            3
        );
        // An unknown run.
        assert_eq!(
            s.commit(&key("wf-cas", "nope"), 1, step(1, 1, 1, "x"))
                .await,
            Err(StoreError::NotFound)
        );
        b.done().await;
    }
}

#[tokio::test]
async fn commit_rejects_non_contiguous_events() {
    for b in backends("noncontig").await {
        let s = b.store();
        let k = key("wf-nc", "run-1");
        started(&*s, start(&k, "req-1", 1, 0)).await;
        // A gap before the batch.
        let gap = step(3, 2, STATUS_RUNNING, "x");
        assert_eq!(
            s.commit(&k, 1, gap).await,
            Err(StoreError::NonContiguous {
                expected: 2,
                got: 3
            })
        );
        // A gap inside the batch.
        let mut inner = step(2, 1, STATUS_RUNNING, "x");
        inner.events.push(ev(4, 2));
        inner.next_event_id = 5;
        assert_eq!(
            s.commit(&k, 1, inner).await,
            Err(StoreError::NonContiguous {
                expected: 3,
                got: 4
            })
        );
        // A repeated event.
        let mut dup = step(2, 1, STATUS_RUNNING, "x");
        dup.events.push(ev(2, 2));
        assert!(matches!(
            s.commit(&k, 1, dup).await,
            Err(StoreError::NonContiguous { .. })
        ));
        // next_event_id that does not follow the events.
        let mut next = step(2, 2, STATUS_RUNNING, "x");
        next.next_event_id = 7;
        assert!(matches!(
            s.commit(&k, 1, next).await,
            Err(StoreError::InvalidArgument(_))
        ));
        // A batch start outside the commit.
        let mut batch = step(2, 1, STATUS_RUNNING, "x");
        batch.events[0].batch_first_event_id = 1;
        assert!(matches!(
            s.commit(&k, 1, batch).await,
            Err(StoreError::InvalidArgument(_))
        ));
        // A start whose events do not begin at 1.
        let mut bad = start(&key("wf-nc2", "run-1"), "r", 0, 0);
        bad.commit.events = events(2, 1);
        bad.commit.next_event_id = 3;
        assert!(matches!(
            s.start(bad).await,
            Err(StoreError::NonContiguous {
                expected: 1,
                got: 2
            })
        ));
        assert_eq!(s.current_run(NS, "wf-nc2").await.unwrap(), None);
        // Nothing moved.
        let l = s.load(&k).await.unwrap().unwrap();
        assert_eq!((l.version, l.next_event_id), (1, 2), "{}", b.name());
        assert_eq!(s.history(&k, 1, 100).await.unwrap(), events(1, 1));
        b.done().await;
    }
}

#[tokio::test]
async fn commit_is_atomic() {
    for b in backends("atomic").await {
        let s = b.store();
        let k = key("wf-atomic", "run-1");
        started(&*s, start(&k, "req-1", 2, 0)).await;
        let before_vis = s.visibility(&k).await.unwrap();

        // A commit that closes the run, writes events, outbox rows and a new
        // visibility row.
        let full = || {
            let mut c = step(3, 3, COMPLETED, "closed");
            c.outbox = outbox(2, "atomic");
            c.visibility = Some(vis(COMPLETED, Some(1_761_300_009_000)));
            c
        };
        let check_untouched = |s: Arc<dyn Store>, name: &'static str| {
            let k = k.clone();
            let before_vis = before_vis.clone();
            async move {
                let l = s.load(&k).await.unwrap().unwrap();
                assert_eq!(
                    (l.version, l.next_event_id, l.status),
                    (1, 3, STATUS_RUNNING),
                    "{name}"
                );
                assert_eq!(l.state_blob, b"state-1");
                assert_eq!(s.history(&k, 1, 100).await.unwrap(), events(1, 2), "{name}");
                assert_eq!(
                    s.current_run(NS, "wf-atomic")
                        .await
                        .unwrap()
                        .unwrap()
                        .status,
                    STATUS_RUNNING
                );
                assert_eq!(s.visibility(&k).await.unwrap(), before_vis, "{name}");
                let all: Vec<u32> = (0..64).collect();
                let claimed = s
                    .claim_outbox(&all, 100, Duration::from_secs(60), "t")
                    .await
                    .unwrap();
                assert!(
                    claimed.is_empty(),
                    "{name}: outbox rows leaked: {claimed:?}"
                );
            }
        };

        // 1. A fault injected between the statements (on Postgres: a server
        //    error after the events, state, current run and outbox rows).
        b.fail_next_commit();
        let err = s.commit(&k, 1, full()).await.unwrap_err();
        assert!(
            matches!(err, StoreError::Unavailable(_) | StoreError::Database(_)),
            "{}: {err:?}",
            b.name()
        );
        check_untouched(s.clone(), b.name()).await;

        // 2. A natural failure in the last statement: NUL in a text column.
        let mut nul = full();
        nul.visibility.as_mut().unwrap().task_queue = "t\0q".into();
        assert!(
            matches!(
                s.commit(&k, 1, nul).await,
                Err(StoreError::InvalidArgument(_))
            ),
            "{}",
            b.name()
        );
        check_untouched(s.clone(), b.name()).await;

        // 3. NUL inside an outbox request's JSON.
        let mut nul_json = full();
        nul_json.outbox[1].request = json!({"x": "a\u{0}b"});
        assert!(
            matches!(
                s.commit(&k, 1, nul_json).await,
                Err(StoreError::InvalidArgument(_))
            ),
            "{}",
            b.name()
        );
        check_untouched(s.clone(), b.name()).await;

        // The same commit then goes through whole.
        assert_eq!(s.commit(&k, 1, full()).await.unwrap(), 2);
        let l = s.load(&k).await.unwrap().unwrap();
        assert_eq!((l.version, l.next_event_id, l.status), (2, 6, COMPLETED));
        assert_eq!(s.history(&k, 1, 100).await.unwrap().len(), 5);
        assert_eq!(
            s.current_run(NS, "wf-atomic")
                .await
                .unwrap()
                .unwrap()
                .status,
            COMPLETED
        );
        assert_eq!(
            s.visibility(&k).await.unwrap(),
            Some(vis(COMPLETED, Some(1_761_300_009_000)))
        );
        assert_eq!(drain(&*s, "t").await.len(), 2);
        b.done().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_two_relays_no_double_claim() {
    for b in backends("tworelays").await {
        let s = b.store();
        for i in 0..40 {
            let k = key(&format!("wf-{i}"), "run-1");
            started(&*s, start(&k, "req", 1, 3)).await;
        }
        let (a, c) = (s.clone(), s.clone());
        let ra = tokio::spawn(async move { drain(&*a, "relay-a").await });
        let rc = tokio::spawn(async move { drain(&*c, "relay-c").await });
        let (ra, rc) = (ra.await.unwrap(), rc.await.unwrap());
        let mut seen = BTreeSet::new();
        for id in ra.iter().chain(rc.iter()) {
            assert!(seen.insert(*id), "{}: row {id} claimed twice", b.name());
        }
        assert_eq!(seen.len(), 120, "{}", b.name());
        // While the leases are live nobody claims anything.
        assert!(drain(&*s, "relay-late").await.is_empty());
        // Dispatched rows never come back.
        let ids: Vec<i64> = seen.iter().copied().collect();
        s.mark_dispatched(&ids).await.unwrap();
        s.mark_dispatched(&ids).await.unwrap();
        assert!(drain(&*s, "relay-late").await.is_empty());
        b.done().await;
    }
}

#[tokio::test]
async fn outbox_expired_claim_reclaimed() {
    for b in backends("expired").await {
        let s = b.store();
        let k = key("wf-exp", "run-1");
        started(&*s, start(&k, "req", 1, 2)).await;
        let shard = shard_of(NS, "wf-exp", 64);
        let lease = Duration::from_millis(300);
        let first = s
            .claim_outbox(&[shard], 10, lease, "relay-a")
            .await
            .unwrap();
        assert_eq!(first.len(), 2, "{}", b.name());
        assert!(first.iter().all(|r| r.attempts == 1));
        assert!(
            s.claim_outbox(&[shard], 10, lease, "relay-b")
                .await
                .unwrap()
                .is_empty()
        );
        // Another shard's claim never sees them.
        let other = (shard + 1) % 64;
        assert!(
            s.claim_outbox(&[other], 10, lease, "relay-b")
                .await
                .unwrap()
                .is_empty()
        );
        // relay-a "crashed": after the lease, relay-b gets both.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let second = s
            .claim_outbox(&[shard], 10, lease, "relay-b")
            .await
            .unwrap();
        assert_eq!(
            second.iter().map(|r| r.id).collect::<Vec<_>>(),
            first.iter().map(|r| r.id).collect::<Vec<_>>()
        );
        assert!(second.iter().all(|r| r.attempts == 2));
        // One dispatched; only the other comes back after the next expiry.
        s.mark_dispatched(&[second[0].id]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let third = s
            .claim_outbox(&[shard], 10, lease, "relay-c")
            .await
            .unwrap();
        assert_eq!(third.len(), 1);
        assert_eq!((third[0].id, third[0].attempts), (second[1].id, 3));
        b.done().await;
    }
}

#[tokio::test]
async fn history_pages_in_order() {
    for b in backends("history").await {
        let s = b.store();
        let k = key("wf-hist", "run-1");
        started(&*s, start(&k, "req", 3, 0)).await;
        s.commit(&k, 1, step(4, 4, STATUS_RUNNING, "s"))
            .await
            .unwrap();
        s.commit(&k, 2, step(8, 3, STATUS_RUNNING, "s"))
            .await
            .unwrap();
        let mut all = Vec::new();
        let mut from = 1;
        loop {
            let page = s.history(&k, from, 3).await.unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= 3);
            from = page.last().unwrap().event_id + 1;
            all.extend(page);
        }
        let ids: Vec<i64> = all.iter().map(|e| e.event_id).collect();
        assert_eq!(ids, (1..=10).collect::<Vec<_>>(), "{}", b.name());
        // Batches and blobs come back as written.
        assert_eq!(all[4], ev(5, 4));
        assert_eq!(all[9], ev(10, 8));
        // A page from the middle.
        let mid = s.history(&k, 5, 2).await.unwrap();
        assert_eq!(mid, vec![ev(5, 4), ev(6, 4)]);
        assert!(s.history(&k, 11, 10).await.unwrap().is_empty());
        assert_eq!(
            s.history(&key("wf-hist", "nope"), 1, 10).await,
            Err(StoreError::NotFound)
        );
        b.done().await;
    }
}

#[tokio::test]
async fn start_rejects_running_duplicate() {
    for b in backends("dupstart").await {
        let s = b.store();
        let a = key("wf-dup", "run-a");
        let bk = key("wf-dup", "run-b");
        started(&*s, start(&a, "req-a", 1, 1)).await;
        assert_eq!(
            s.start(start(&bk, "req-b", 1, 1)).await.unwrap(),
            StartOutcome::AlreadyStarted {
                run_id: "run-a".into(),
                request_id: "req-a".into(),
                running: true
            },
            "{}",
            b.name()
        );
        assert_eq!(s.load(&bk).await.unwrap(), None);
        assert_eq!(
            drain(&*s, "r").await.len(),
            1,
            "the refused start wrote outbox rows"
        );
        // Close run-a; run-b may start (allow duplicate).
        s.commit(&a, 1, step(2, 1, COMPLETED, "done"))
            .await
            .unwrap();
        started(&*s, start(&bk, "req-b", 1, 0)).await;
        let cur = s.current_run(NS, "wf-dup").await.unwrap().unwrap();
        assert_eq!(
            (cur.run_id.as_str(), cur.status, cur.request_id.as_str()),
            ("run-b", STATUS_RUNNING, "req-b")
        );
        // A reused run ID is refused, and the current run stays.
        s.commit(&bk, 1, step(2, 1, COMPLETED, "done"))
            .await
            .unwrap();
        assert_eq!(
            s.start(start(&a, "req-c", 1, 0)).await,
            Err(StoreError::RunExists)
        );
        assert_eq!(
            s.current_run(NS, "wf-dup").await.unwrap().unwrap().run_id,
            "run-b"
        );
        // Concurrent starts of one workflow ID: exactly one wins.
        let mut tasks = Vec::new();
        for i in 0..8 {
            let s = s.clone();
            tasks.push(tokio::spawn(async move {
                s.start(start(
                    &key("wf-race", &format!("run-{i}")),
                    &format!("req-{i}"),
                    1,
                    0,
                ))
                .await
                .unwrap()
            }));
        }
        let mut wins = 0;
        for t in tasks {
            if matches!(t.await.unwrap(), StartOutcome::Started { .. }) {
                wins += 1;
            }
        }
        assert_eq!(wins, 1, "{}", b.name());
        b.done().await;
    }
}

#[tokio::test]
async fn start_same_request_id_is_idempotent() {
    for b in backends("samereq").await {
        let s = b.store();
        let a = key("wf-idem", "run-a");
        started(&*s, start(&a, "req-1", 1, 0)).await;
        // The retry carries a new run ID (the frontend makes one per call).
        let retry = start(&key("wf-idem", "run-retry"), "req-1", 1, 0);
        let want = StartOutcome::AlreadyStarted {
            run_id: "run-a".into(),
            request_id: "req-1".into(),
            running: true,
        };
        assert_eq!(s.start(retry.clone()).await.unwrap(), want, "{}", b.name());
        assert_eq!(s.start(retry.clone()).await.unwrap(), want);
        assert_eq!(s.load(&a).await.unwrap().unwrap().version, 1);
        // After the run closes, the same request still answers that run.
        s.commit(&a, 1, step(2, 1, COMPLETED, "done"))
            .await
            .unwrap();
        assert_eq!(
            s.start(retry).await.unwrap(),
            StartOutcome::AlreadyStarted {
                run_id: "run-a".into(),
                request_id: "req-1".into(),
                running: false
            }
        );
        assert_eq!(s.load(&key("wf-idem", "run-retry")).await.unwrap(), None);
        b.done().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrations_idempotent() {
    for b in backends("migrate").await {
        let Backend::Pg(s, db) = &b else {
            // MemStore has no schema.
            b.done().await;
            continue;
        };
        // Again, and from four stores at once (the advisory lock).
        s.migrate().await.unwrap();
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let opts = db.opts.clone();
            tasks.push(tokio::spawn(async move {
                let s = PgStore::connect_with(opts, 64).await.unwrap();
                s.migrate().await.unwrap();
                s.close().await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let rows: Vec<(i32, i32)> =
            sqlx::query_as("SELECT version, shards FROM schema_version ORDER BY version")
                .fetch_all(s.pool())
                .await
                .unwrap();
        assert_eq!(rows, vec![(1, 64), (2, 64)]);
        // A fresh database migrated by four stores at once ends the same.
        let fresh = TestDb::create("migrate_race").await.unwrap();
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let opts = fresh.opts.clone();
            tasks.push(tokio::spawn(async move {
                let s = PgStore::connect_with(opts, 64).await.unwrap();
                s.migrate().await.unwrap();
                s.close().await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let s2 = PgStore::connect_with(fresh.opts.clone(), 64).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM schema_version")
            .fetch_one(s2.pool())
            .await
            .unwrap();
        assert_eq!(n, i64::from(loams_temporal_store::SCHEMA_VERSION));
        s2.close().await;
        drop(s2);
        fresh.drop_db().await;
        // The store still works.
        started(&**s, start(&key("wf-m", "run-1"), "r", 1, 0)).await;
        b.done().await;
    }
}

#[tokio::test]
async fn shard_vector() {
    // X1.R6's rule, checked against Python's hashlib:
    // int.from_bytes(sha256(ns + b"\0" + wf).digest()[:8], "big") % shards
    assert_eq!(shard_of("default", "hello-workflow", 64), 63);
    assert_eq!(shard_of("ns-1", "wf-1", 64), 28);
    assert_eq!(
        shard_of("550e8400-e29b-41d4-a716-446655440000", "order-42", 64),
        58
    );
    assert_eq!(shard_of("a", "b", 1000), 387);
    // Every store files outbox rows under that shard.
    for b in backends("shardvec").await {
        let s = b.store();
        for wf in ["wf-1", "hello-workflow", "order-42"] {
            started(&*s, start(&key(wf, "run-1"), "r", 1, 1)).await;
            let shard = shard_of(NS, wf, 64);
            let rows = s
                .claim_outbox(&[shard], 10, Duration::from_secs(60), "r")
                .await
                .unwrap();
            assert_eq!(rows.len(), 1, "{} {wf}", b.name());
            assert_eq!(rows[0].shard_id, shard);
            assert_eq!(rows[0].key, key(wf, "run-1"));
            assert_eq!(rows[0].kind, "task_create");
            assert_eq!(rows[0].resonate_id, format!("wft:{wf}:0"));
            assert_eq!(rows[0].request, json!({"tag": wf, "i": 0}));
        }
        b.done().await;
    }
}

#[tokio::test]
async fn shard_count_mismatch_refused() {
    assert!(matches!(
        MemStore::new(0),
        Err(StoreError::InvalidArgument(_))
    ));
    for b in backends("shardcount").await {
        let Backend::Pg(_, db) = &b else {
            b.done().await;
            continue;
        };
        // The database was created with 64 shards.
        assert_eq!(
            PgStore::connect_with(db.opts.clone(), 32).await.err(),
            Some(StoreError::ShardCountMismatch {
                stored: 64,
                configured: 32
            })
        );
        // A store connected to an empty database with 32 shards, migrated
        // after another process created it with 64, is refused at migrate.
        let fresh = TestDb::create("shardcount2").await.unwrap();
        let s32 = PgStore::connect_with(fresh.opts.clone(), 32).await.unwrap();
        let s64 = PgStore::connect_with(fresh.opts.clone(), 64).await.unwrap();
        s64.migrate().await.unwrap();
        assert_eq!(
            s32.migrate().await,
            Err(StoreError::ShardCountMismatch {
                stored: 64,
                configured: 32
            })
        );
        s32.close().await;
        s64.close().await;
        drop((s32, s64));
        fresh.drop_db().await;
        b.done().await;
    }
}

#[tokio::test]
async fn namespaces_roundtrip() {
    for b in backends("namespaces").await {
        let s = b.store();
        let ns = |id: &str, loams: &str, name: &str| NamespaceRecord {
            namespace_id: id.into(),
            loams_namespace_id: loams.into(),
            name: name.into(),
            retention_secs: 72 * 3600,
            state: 1,
            config: json!({"limits": {"blob": 2097152}}),
        };
        s.create_namespace(ns("id-b", "L1", "zeta")).await.unwrap();
        s.create_namespace(ns("id-a", "L1", "default"))
            .await
            .unwrap();
        s.create_namespace(ns("id-c", "L2", "default"))
            .await
            .unwrap();
        assert_eq!(
            s.create_namespace(ns("id-a", "L3", "other")).await,
            Err(StoreError::NamespaceExists)
        );
        assert_eq!(
            s.create_namespace(ns("id-z", "L1", "default")).await,
            Err(StoreError::NamespaceExists)
        );
        assert_eq!(
            s.namespace_by_name("L1", "default").await.unwrap(),
            Some(ns("id-a", "L1", "default")),
            "{}",
            b.name()
        );
        assert_eq!(s.namespace_by_name("L3", "default").await.unwrap(), None);
        assert_eq!(
            s.namespace_by_id("id-c").await.unwrap(),
            Some(ns("id-c", "L2", "default"))
        );
        assert_eq!(s.namespace_by_id("nope").await.unwrap(), None);
        let names: Vec<String> = s
            .list_namespaces("L1")
            .await
            .unwrap()
            .into_iter()
            .map(|n| n.name)
            .collect();
        assert_eq!(names, vec!["default", "zeta"]);
        assert!(s.list_namespaces("L9").await.unwrap().is_empty());
        b.done().await;
    }
}

#[tokio::test]
async fn visibility_roundtrip() {
    for b in backends("visibility").await {
        let s = b.store();
        let k = key("wf-vis", "run-1");
        started(&*s, start(&k, "req", 1, 0)).await;
        assert_eq!(
            s.visibility(&k).await.unwrap(),
            Some(vis(STATUS_RUNNING, None)),
            "{}",
            b.name()
        );
        let mut c = step(2, 1, COMPLETED, "done");
        let mut v = vis(COMPLETED, Some(1_761_300_099_999));
        v.memo = None;
        v.search_attrs = json!({});
        c.visibility = Some(v.clone());
        s.commit(&k, 1, c).await.unwrap();
        assert_eq!(s.visibility(&k).await.unwrap(), Some(v));
        assert_eq!(s.visibility(&key("wf-vis", "nope")).await.unwrap(), None);
        b.done().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outbox_wakeup_on_commit() {
    for b in backends("wakeup").await {
        let s = b.store();
        let k = key("wf-wake", "run-1");
        started(&*s, start(&k, "req", 1, 0)).await;
        // On Postgres, a second store stands for another process: it hears
        // this one's commits only through LISTEN.
        let other = match &b {
            Backend::Pg(s, db) => {
                let o = PgStore::connect_with(db.opts.clone(), 64).await.unwrap();
                // Wait until both listeners are listening, then let their
                // first (connected) wake-ups land.
                let mut listening = 0;
                for _ in 0..100 {
                    listening = sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM pg_stat_activity \
                         WHERE datname = current_database() AND query LIKE 'LISTEN%'",
                    )
                    .fetch_one(s.pool())
                    .await
                    .unwrap();
                    if listening >= 2 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                assert!(listening >= 2, "listeners did not start");
                tokio::time::sleep(Duration::from_millis(300)).await;
                Some(o)
            }
            Backend::Mem(_) => None,
        };
        let mut rx = s.outbox_wakeups();
        let mut other_rx = other.as_ref().map(|o| o.outbox_wakeups());
        // A commit without outbox rows wakes nobody.
        s.commit(&k, 1, step(2, 1, STATUS_RUNNING, "s"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!rx.has_changed().unwrap(), "{}", b.name());
        if let Some(rx) = other_rx.as_ref() {
            assert!(!rx.has_changed().unwrap());
        }
        let mut c = step(3, 1, STATUS_RUNNING, "s");
        c.outbox = outbox(1, "wake");
        s.commit(&k, 2, c).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .expect("local wake-up")
            .unwrap();
        if let Some(rx) = other_rx.as_mut() {
            tokio::time::timeout(Duration::from_secs(10), rx.changed())
                .await
                .expect("LISTEN wake-up from another store")
                .unwrap();
        }
        if let Some(o) = other {
            o.close().await;
        }
        b.done().await;
    }
}

// ---------------------------------------------------------------------------
// Page tokens, the run listing, retention and the pool (D2060–D2065).
// ---------------------------------------------------------------------------

fn namespace(id: &str, retention_secs: i64) -> NamespaceRecord {
    NamespaceRecord {
        namespace_id: id.into(),
        loams_namespace_id: "L1".into(),
        name: format!("name-{id}"),
        retention_secs,
        state: 1,
        config: json!({}),
    }
}

/// A run started at `start_ms` with a visibility row of `workflow_type`.
fn start_at(k: &RunKey, start_ms: i64, workflow_type: &str) -> StartExecution {
    let mut st = start(k, &format!("req-{}-{}", k.workflow_id, k.run_id), 1, 0);
    let v = st.commit.visibility.as_mut().unwrap();
    v.start_time_ms = start_ms;
    v.workflow_type = workflow_type.into();
    st
}

/// Closes a run that has `next` as its next event ID and `version`.
async fn close(s: &dyn Store, k: &RunKey, version: u64, next: i64) {
    let mut c = step(next, 1, COMPLETED, "closed");
    let mut v = s.visibility(k).await.unwrap().unwrap();
    v.status = COMPLETED;
    v.close_time_ms = Some(v.start_time_ms + 1000);
    c.visibility = Some(v);
    s.commit(k, version, c).await.unwrap();
}

#[tokio::test]
async fn history_page_tokens() {
    for b in backends("histpage").await {
        let s = b.store();
        let k = key("wf-hp", "run-1");
        started(&*s, start(&k, "req", 4, 0)).await;
        s.commit(&k, 1, step(5, 6, STATUS_RUNNING, "s"))
            .await
            .unwrap();
        // 10 events in pages of 4: 4, 4, 2, and the last page has no token.
        let mut token = Vec::new();
        let mut sizes = Vec::new();
        let mut ids = Vec::new();
        loop {
            let p = s.history_page(&k, &token, 4).await.unwrap();
            sizes.push(p.events.len());
            ids.extend(p.events.iter().map(|e| e.event_id));
            if p.next_page_token.is_empty() {
                break;
            }
            token = p.next_page_token;
        }
        assert_eq!(sizes, vec![4, 4, 2], "{}", b.name());
        assert_eq!(ids, (1..=10).collect::<Vec<_>>());
        // Exactly one page's worth: no token (no empty trailing page).
        let p = s.history_page(&k, &[], 10).await.unwrap();
        assert_eq!(p.events.len(), 10);
        assert!(p.next_page_token.is_empty());
        // A token is bound to its run.
        let first = s.history_page(&k, &[], 3).await.unwrap();
        // Another run (of another workflow ID: wf-hp's run-1 is running).
        let other = key("wf-hp-2", "run-1");
        started(&*s, start(&other, "req-2", 5, 0)).await;
        assert!(matches!(
            s.history_page(&other, &first.next_page_token, 3).await,
            Err(StoreError::InvalidArgument(_))
        ));
        // Garbage, and page sizes out of range.
        assert!(matches!(
            s.history_page(&k, b"garbage", 3).await,
            Err(StoreError::InvalidArgument(_))
        ));
        assert!(matches!(
            s.history_page(&k, &[], 0).await,
            Err(StoreError::InvalidArgument(_))
        ));
        assert_eq!(
            s.history_page(&key("wf-hp", "nope"), &[], 3).await,
            Err(StoreError::NotFound)
        );
        // Events written after a page was served show up on its token.
        let tail = s.history_page(&k, &[], 9).await.unwrap();
        assert!(!tail.next_page_token.is_empty());
        s.commit(&k, 2, step(11, 2, STATUS_RUNNING, "s"))
            .await
            .unwrap();
        let rest = s
            .history_page(&k, &tail.next_page_token, 100)
            .await
            .unwrap();
        let rest_ids: Vec<i64> = rest.events.iter().map(|e| e.event_id).collect();
        assert_eq!(rest_ids, vec![10, 11, 12]);
        b.done().await;
    }
}

#[tokio::test]
async fn list_runs_pages_and_filters() {
    for b in backends("listruns").await {
        let s = b.store();
        // Seven runs; two share a start time to exercise the tie-break.
        let base = 1_761_300_000_000;
        let runs = [
            ("wf-a", "r1", base + 5, "hello"),
            ("wf-b", "r1", base + 4, "hello"),
            ("wf-c", "r1", base + 4, "other"),
            ("wf-d", "r1", base + 3, "hello"),
            ("wf-e", "r1", base + 2, "other"),
            ("wf-f", "r1", base + 1, "hello"),
            ("wf-g", "r1", base, "hello"),
        ];
        for (wf, run, at, ty) in runs {
            started(&*s, start_at(&key(wf, run), at, ty)).await;
        }
        close(&*s, &key("wf-b", "r1"), 1, 2).await;
        close(&*s, &key("wf-f", "r1"), 1, 2).await;
        // Another namespace never shows up.
        started(
            &*s,
            start_at(&RunKey::new("ns-2", "wf-x", "r1"), base + 9, "hello"),
        )
        .await;

        let list = |filter: RunFilter, size: usize| {
            let s = s.clone();
            async move {
                let mut token = Vec::new();
                let mut out = Vec::new();
                let mut pages = 0;
                loop {
                    let p = s
                        .list_runs(&ListRuns {
                            namespace_id: NS.into(),
                            filter: filter.clone(),
                            page_size: size,
                            page_token: token,
                        })
                        .await
                        .unwrap();
                    pages += 1;
                    assert!(p.runs.len() <= size);
                    out.extend(p.runs.into_iter().map(|r| r.key.workflow_id));
                    if p.next_page_token.is_empty() {
                        return (out, pages);
                    }
                    token = p.next_page_token;
                }
            }
        };
        let (all, pages) = list(RunFilter::default(), 3).await;
        assert_eq!(
            all,
            vec!["wf-a", "wf-b", "wf-c", "wf-d", "wf-e", "wf-f", "wf-g"],
            "{}",
            b.name()
        );
        assert_eq!(pages, 3);
        let (open, _) = list(
            RunFilter {
                open: Some(true),
                ..RunFilter::default()
            },
            2,
        )
        .await;
        assert_eq!(open, vec!["wf-a", "wf-c", "wf-d", "wf-e", "wf-g"]);
        let (closed, _) = list(
            RunFilter {
                open: Some(false),
                ..RunFilter::default()
            },
            10,
        )
        .await;
        assert_eq!(closed, vec!["wf-b", "wf-f"]);
        let (typed, _) = list(
            RunFilter {
                workflow_type: Some("other".into()),
                ..RunFilter::default()
            },
            1,
        )
        .await;
        assert_eq!(typed, vec!["wf-c", "wf-e"]);
        let (one, _) = list(
            RunFilter {
                workflow_id: Some("wf-d".into()),
                ..RunFilter::default()
            },
            5,
        )
        .await;
        assert_eq!(one, vec!["wf-d"]);

        // A run started while paging lands before the cursor: no row is
        // repeated or skipped.
        let first = s
            .list_runs(&ListRuns {
                namespace_id: NS.into(),
                filter: RunFilter::default(),
                page_size: 2,
                page_token: Vec::new(),
            })
            .await
            .unwrap();
        started(&*s, start_at(&key("wf-new", "r1"), base + 100, "hello")).await;
        let second = s
            .list_runs(&ListRuns {
                namespace_id: NS.into(),
                filter: RunFilter::default(),
                page_size: 2,
                page_token: first.next_page_token.clone(),
            })
            .await
            .unwrap();
        let ids: Vec<&str> = second
            .runs
            .iter()
            .map(|r| r.key.workflow_id.as_str())
            .collect();
        assert_eq!(ids, vec!["wf-c", "wf-d"]);
        // The listed row is the visibility row.
        assert_eq!(second.runs[0].visibility.start_time_ms, base + 4);
        assert_eq!(second.runs[0].key, key("wf-c", "r1"));

        // A token is bound to its namespace and filter.
        assert!(matches!(
            s.list_runs(&ListRuns {
                namespace_id: NS.into(),
                filter: RunFilter {
                    open: Some(true),
                    ..RunFilter::default()
                },
                page_size: 2,
                page_token: first.next_page_token.clone(),
            })
            .await,
            Err(StoreError::InvalidArgument(_))
        ));
        assert!(matches!(
            s.list_runs(&ListRuns {
                namespace_id: "ns-2".into(),
                filter: RunFilter::default(),
                page_size: 2,
                page_token: first.next_page_token,
            })
            .await,
            Err(StoreError::InvalidArgument(_))
        ));
        assert!(matches!(
            s.list_runs(&ListRuns {
                namespace_id: NS.into(),
                filter: RunFilter::default(),
                page_size: 0,
                page_token: Vec::new(),
            })
            .await,
            Err(StoreError::InvalidArgument(_))
        ));
        b.done().await;
    }
}

#[tokio::test]
async fn retention_purges_closed_runs() {
    for b in backends("retention").await {
        let s = b.store();
        s.create_namespace(namespace(NS, 0)).await.unwrap();
        s.create_namespace(namespace("ns-keep", 3600))
            .await
            .unwrap();
        // Closed, past retention: purged with everything it owns. Its one
        // outbox row is dispatched first.
        let gone = key("wf-gone", "run-1");
        started(&*s, start(&gone, "req-g", 3, 1)).await;
        let dispatched = drain(&*s, "relay").await;
        assert_eq!(dispatched.len(), 1);
        s.mark_dispatched(&dispatched).await.unwrap();
        close(&*s, &gone, 1, 4).await;
        // Still running: kept.
        let running = key("wf-running", "run-1");
        started(&*s, start(&running, "req-r", 2, 0)).await;
        // Closed in a namespace with an hour of retention: kept.
        let young = RunKey::new("ns-keep", "wf-young", "run-1");
        started(&*s, start(&young, "req-y", 1, 0)).await;
        close(&*s, &young, 1, 2).await;
        // Closed, past retention, but with an undispatched outbox row: kept
        // until the relay hands it to Resonate.
        let pending = key("wf-pending", "run-1");
        started(&*s, start(&pending, "req-p", 1, 0)).await;
        let mut c = step(2, 1, COMPLETED, "closed");
        c.outbox = outbox(1, "pending");
        s.commit(&pending, 1, c).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let limits = PurgeLimits {
            max_runs: 100,
            max_outbox_rows: 0,
            dispatched_outbox_ttl: Duration::from_secs(3600),
        };
        let r = s.purge_expired(&limits).await.unwrap();
        assert_eq!(r.runs, 1, "{}", b.name());
        assert_eq!(r.events, 4);
        assert_eq!(s.load(&gone).await.unwrap(), None);
        assert_eq!(s.visibility(&gone).await.unwrap(), None);
        assert_eq!(s.current_run(NS, "wf-gone").await.unwrap(), None);
        assert_eq!(s.history(&gone, 1, 10).await, Err(StoreError::NotFound));
        // The others are untouched.
        assert!(s.load(&running).await.unwrap().is_some());
        assert!(s.load(&young).await.unwrap().is_some());
        assert!(s.load(&pending).await.unwrap().is_some());
        // Nothing more to do.
        assert_eq!(s.purge_expired(&limits).await.unwrap().runs, 0);

        // Once the pending row is dispatched, the run goes too.
        let all: Vec<u32> = (0..64).collect();
        let rows = s
            .claim_outbox(&all, 100, Duration::from_secs(60), "relay4")
            .await
            .unwrap();
        let pend: Vec<i64> = rows
            .iter()
            .filter(|r| r.key == pending)
            .map(|r| r.id)
            .collect();
        assert_eq!(pend.len(), 1);
        s.mark_dispatched(&pend).await.unwrap();
        let r = s.purge_expired(&limits).await.unwrap();
        assert_eq!(r.runs, 1);
        assert_eq!(r.run_outbox_rows, 1);
        assert_eq!(s.load(&pending).await.unwrap(), None);
        // A purged workflow ID can start again.
        started(&*s, start(&key("wf-gone", "run-2"), "req-g2", 1, 0)).await;
        b.done().await;
    }
}

#[tokio::test]
async fn retention_keeps_newer_current_run() {
    for b in backends("retcurrent").await {
        let s = b.store();
        s.create_namespace(namespace(NS, 0)).await.unwrap();
        let old = key("wf-cur", "run-1");
        started(&*s, start(&old, "req-1", 1, 0)).await;
        close(&*s, &old, 1, 2).await;
        let new = key("wf-cur", "run-2");
        started(&*s, start(&new, "req-2", 1, 0)).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let r = s.purge_expired(&PurgeLimits::default()).await.unwrap();
        assert_eq!(r.runs, 1, "{}", b.name());
        let cur = s.current_run(NS, "wf-cur").await.unwrap().unwrap();
        assert_eq!(cur.run_id, "run-2");
        assert!(s.load(&new).await.unwrap().is_some());
        b.done().await;
    }
}

#[tokio::test]
async fn retention_limits_and_outbox_ttl() {
    for b in backends("retlimits").await {
        let s = b.store();
        s.create_namespace(namespace(NS, 0)).await.unwrap();
        for i in 0..5 {
            let k = key(&format!("wf-l{i}"), "run-1");
            started(&*s, start(&k, &format!("req-{i}"), 1, 0)).await;
            close(&*s, &k, 1, 2).await;
        }
        // Three dispatched outbox rows on a running run.
        let live = key("wf-live", "run-1");
        started(&*s, start(&live, "req-live", 1, 3)).await;
        let ids = drain(&*s, "relay").await;
        assert_eq!(ids.len(), 3);
        s.mark_dispatched(&ids).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let limits = PurgeLimits {
            max_runs: 2,
            max_outbox_rows: 2,
            dispatched_outbox_ttl: Duration::ZERO,
        };
        let r = s.purge_expired(&limits).await.unwrap();
        assert_eq!((r.runs, r.dispatched_outbox_rows), (2, 2), "{}", b.name());
        assert!(r.saturated(&limits));
        let r = s.purge_expired(&limits).await.unwrap();
        assert_eq!((r.runs, r.dispatched_outbox_rows), (2, 1));
        let r = s.purge_expired(&limits).await.unwrap();
        assert_eq!((r.runs, r.dispatched_outbox_rows), (1, 0));
        assert!(!r.saturated(&limits));
        // A long TTL keeps dispatched rows; the running run is untouched.
        let mut c = step(2, 1, STATUS_RUNNING, "s");
        c.outbox = outbox(1, "late");
        s.commit(&live, 1, c).await.unwrap();
        let ids = drain(&*s, "relay").await;
        s.mark_dispatched(&ids).await.unwrap();
        let r = s
            .purge_expired(&PurgeLimits {
                dispatched_outbox_ttl: Duration::from_secs(3600),
                ..limits
            })
            .await
            .unwrap();
        assert_eq!(r, PurgeReport::default());
        assert!(s.load(&live).await.unwrap().is_some());
        // The sweeper's round drains everything a round may.
        let report = retention::sweep_once(&*s, &retention::RetentionPolicy::default()).await;
        assert_eq!(report.runs, 0);
        b.done().await;
    }
}

#[tokio::test]
async fn pg_pool_options_applied() {
    for b in backends("pool").await {
        let Backend::Pg(_, db) = &b else {
            b.done().await;
            continue;
        };
        let mut o = PgStoreOptions::new(64);
        o.max_connections = 1;
        assert!(matches!(
            PgStore::connect_with_options(db.opts.clone(), &o).await,
            Err(StoreError::InvalidArgument(_))
        ));
        let mut o = PgStoreOptions::new(64);
        o.max_connections = 4;
        o.min_connections = 5;
        assert!(matches!(
            PgStore::connect_with_options(db.opts.clone(), &o).await,
            Err(StoreError::InvalidArgument(_))
        ));
        let mut o = PgStoreOptions::new(64);
        o.application_name = "lts-pool-test".into();
        o.statement_timeout = Some(Duration::from_millis(1500));
        let s = PgStore::connect_with_options(db.opts.clone(), &o)
            .await
            .unwrap();
        let (name, timeout): (String, String) = sqlx::query_as(
            "SELECT current_setting('application_name'), current_setting('statement_timeout')",
        )
        .fetch_one(s.pool())
        .await
        .unwrap();
        assert_eq!(name, "lts-pool-test");
        assert_eq!(timeout, "1500ms");
        // The statement timeout is enforced.
        let slow = sqlx::query("SELECT pg_sleep(3)").execute(s.pool()).await;
        assert!(slow.is_err());
        s.close().await;
        b.done().await;
    }
}

#[tokio::test]
async fn pg_upgrade_from_v1_keeps_history() {
    for b in backends("upgrade").await {
        let Backend::Pg(..) = &b else {
            b.done().await;
            continue;
        };
        // A database at schema version 1, as 0.1.2 left it.
        let db = TestDb::create("upgrade_v1").await.unwrap();
        let mut c = PgConnection::connect_with(&db.opts).await.unwrap();
        sqlx::raw_sql(AssertSqlSafe(include_str!("../migrations/0001_init.sql")))
            .execute(&mut c)
            .await
            .unwrap();
        sqlx::query("INSERT INTO schema_version (version, shards) VALUES (1, 64)")
            .execute(&mut c)
            .await
            .unwrap();
        let k = key("wf-up", "run-1");
        let shard = shard_of(NS, "wf-up", 64) as i32;
        sqlx::query(
            "INSERT INTO executions (shard_id, namespace_id, workflow_id, run_id, status, \
             next_event_id, db_record_version, state_blob) VALUES ($1, $2, 'wf-up', 'run-1', \
             $3, 3, 1, 'x')",
        )
        .bind(shard)
        .bind(NS)
        .bind(COMPLETED)
        .execute(&mut c)
        .await
        .unwrap();
        for id in 1..=2i64 {
            sqlx::query(
                "INSERT INTO history_events (shard_id, namespace_id, workflow_id, run_id, \
                 event_id, batch_first_event_id, event_type, event_blob) \
                 VALUES ($1, $2, 'wf-up', 'run-1', $3, 1, 7, $4)",
            )
            .bind(shard)
            .bind(NS)
            .bind(id)
            .bind(format!("event-{id}").into_bytes())
            .execute(&mut c)
            .await
            .unwrap();
        }
        c.close().await.unwrap();

        let s = PgStore::connect_with(db.opts.clone(), 64).await.unwrap();
        s.migrate().await.unwrap();
        let events = s.history(&k, 1, 10).await.unwrap();
        let ids: Vec<i64> = events.iter().map(|e| e.event_id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(events[1].blob, b"event-2".to_vec());
        // The closed run got a close time, and history is partitioned.
        let closed: bool = sqlx::query_scalar(
            "SELECT closed_at IS NOT NULL FROM executions WHERE workflow_id = 'wf-up'",
        )
        .fetch_one(s.pool())
        .await
        .unwrap();
        assert!(closed);
        let parts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_inherits WHERE inhparent = 'history_events'::regclass",
        )
        .fetch_one(s.pool())
        .await
        .unwrap();
        assert_eq!(parts, 16);
        s.close().await;
        drop(s);
        db.drop_db().await;
        b.done().await;
    }
}

#[tokio::test]
async fn update_namespace_replaces_mutable_fields() {
    for b in backends("nsupdate").await {
        let s = b.store();
        let orig = namespace("ns-up", 72 * 3600);
        s.create_namespace(orig.clone()).await.unwrap();
        // The name and Loams namespace in the request are ignored.
        let mut changed = orig.clone();
        changed.retention_secs = 3600;
        changed.state = 2;
        changed.config = json!({"description": "updated", "limits": {"blob": 1024}});
        changed.name = "renamed".into();
        changed.loams_namespace_id = "L-other".into();
        s.update_namespace(changed.clone()).await.unwrap();
        let got = s.namespace_by_id("ns-up").await.unwrap().unwrap();
        assert_eq!(got.retention_secs, 3600, "{}", b.name());
        assert_eq!(got.state, 2);
        assert_eq!(got.config, changed.config);
        assert_eq!(got.name, orig.name);
        assert_eq!(got.loams_namespace_id, orig.loams_namespace_id);
        assert_eq!(
            s.namespace_by_name(&orig.loams_namespace_id, &orig.name)
                .await
                .unwrap(),
            Some(got)
        );
        assert_eq!(
            s.update_namespace(namespace("nope", 1)).await,
            Err(StoreError::NotFound)
        );
        // A shorter retention takes effect for runs already closed.
        let k = RunKey::new("ns-up", "wf-r", "run-1");
        started(&*s, start(&k, "req", 1, 0)).await;
        close(&*s, &k, 1, 2).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            s.purge_expired(&PurgeLimits::default()).await.unwrap().runs,
            0
        );
        let mut zero = orig.clone();
        zero.retention_secs = 0;
        s.update_namespace(zero).await.unwrap();
        assert_eq!(
            s.purge_expired(&PurgeLimits::default()).await.unwrap().runs,
            1
        );
        b.done().await;
    }
}
