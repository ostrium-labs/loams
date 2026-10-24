//! The store's contract, run against every backend: `MemStore` always, and
//! `PgStore` when `LOAMS_TEMPORAL_TEST_PG_URL` names a Postgres server whose
//! user may create databases. Each Postgres leg runs in a fresh database it
//! creates and drops.
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
    Commit, EventRow, MemStore, NamespaceRecord, OutboxInsert, PgStore, RunKey, STATUS_RUNNING,
    StartExecution, StartOutcome, Store, StoreError, VisibilityRow, shard_of,
};
use serde_json::json;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection};

const ENV: &str = "LOAMS_TEMPORAL_TEST_PG_URL";
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
        let url = std::env::var(ENV).ok().filter(|u| !u.is_empty())?;
        static N: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "lts_{test}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        );
        let admin = PgConnectOptions::from_str(&url).expect("LOAMS_TEMPORAL_TEST_PG_URL");
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
        eprintln!("{test}: {ENV} not set; Postgres leg skipped");
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
        let rows: Vec<(i32, i32)> = sqlx::query_as("SELECT version, shards FROM schema_version")
            .fetch_all(s.pool())
            .await
            .unwrap();
        assert_eq!(rows, vec![(1, 64)]);
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
        assert_eq!(n, 1);
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
