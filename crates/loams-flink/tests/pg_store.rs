//! The saved-job store on Postgres, when `LOAMS_FLINK_TEST_PG_URL` names a
//! server whose user may create databases (each test runs in a fresh
//! database it creates and drops). Without it the tests pass without
//! running, and say so.
//!
//! ```text
//! podman run -d --rm --name loams-flink-pg -e POSTGRES_PASSWORD=pg -p 127.0.0.1:55439:5432 \
//!     docker.io/library/postgres:17
//! LOAMS_FLINK_TEST_PG_URL=postgres://postgres:pg@127.0.0.1:55439/postgres \
//!     cargo test -p loams-flink --test pg_store
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use loams_flink::api::jobs::{JobState, JobStore, NewJob, SavedJob, StoreError};
use loams_flink::api::pg_store::{PgJobStore, SCHEMA_VERSION};
use loams_flink::catalog::{JdbcCatalog, Secret, SessionSpec};
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection};

const ENV: &str = "LOAMS_FLINK_TEST_PG_URL";

struct TestDb {
    admin: PgConnectOptions,
    name: String,
    store: PgJobStore,
}

impl TestDb {
    async fn create(test: &str) -> Option<Self> {
        let Some(url) = std::env::var(ENV).ok().filter(|u| !u.is_empty()) else {
            eprintln!("{test}: skipped, {ENV} is not set");
            return None;
        };
        static N: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "lflink_{test}_{}_{}",
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
        let store = PgJobStore::connect_with(admin.clone().database(&name))
            .await
            .expect("connect the store");
        Some(Self { admin, name, store })
    }

    async fn drop_db(self) {
        self.store.pool().close().await;
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

fn job(name: &str, instance: &str) -> SavedJob {
    let mut session = SessionSpec::default();
    session.postgres.push(JdbcCatalog {
        name: "loams_pg".into(),
        host: "compute1".into(),
        port: 5432,
        database: "shop".into(),
        username: "flink".into(),
        password: Secret::reference("pg"),
        extra: Default::default(),
    });
    SavedJob::new(
        "ns1",
        instance,
        "user:a",
        NewJob {
            name: name.into(),
            description: Some("d".into()),
            statement: "INSERT INTO loams.analytics.t SELECT 1".into(),
            setup: vec!["CREATE DATABASE IF NOT EXISTS loams.analytics".into()],
            session,
        },
    )
}

#[tokio::test]
async fn migrate_is_idempotent() {
    let Some(db) = TestDb::create("migrate").await else {
        return;
    };
    assert_eq!(db.store.schema_version().await.unwrap(), 0);
    db.store.migrate().await.unwrap();
    db.store.migrate().await.unwrap();
    assert_eq!(db.store.schema_version().await.unwrap(), SCHEMA_VERSION);
    // Concurrent migrations serialise on the advisory lock.
    let a = db.store.clone();
    let b = db.store.clone();
    let (x, y) = tokio::join!(a.migrate(), b.migrate());
    x.unwrap();
    y.unwrap();
    db.drop_db().await;
}

#[tokio::test]
async fn round_trip_and_list_order() {
    let Some(db) = TestDb::create("roundtrip").await else {
        return;
    };
    db.store.migrate().await.unwrap();
    let a = job("a", "eng_x");
    let b = job("b", "eng_x");
    let other = job("c", "eng_y");
    db.store.insert(a.clone()).await.unwrap();
    db.store.insert(b.clone()).await.unwrap();
    db.store.insert(other.clone()).await.unwrap();
    assert!(matches!(
        db.store.insert(a.clone()).await,
        Err(StoreError::Exists(_))
    ));

    let got = db.store.get("ns1", &a.id).await.unwrap().unwrap();
    assert_eq!(got, a, "the whole record round-trips");
    assert!(db.store.get("ns2", &a.id).await.unwrap().is_none());

    let listed = db.store.list("ns1", "eng_x").await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|j| j.instance == "eng_x"));
    db.drop_db().await;
}

#[tokio::test]
async fn update_is_compare_and_set() {
    let Some(db) = TestDb::create("cas").await else {
        return;
    };
    db.store.migrate().await.unwrap();
    let j = db.store.insert(job("a", "eng_x")).await.unwrap();

    let mut first = j.clone();
    first.state = JobState::Starting;
    let first = db.store.update(first).await.unwrap();
    assert_eq!(first.version, 1);

    let mut stale = j.clone();
    stale.state = JobState::Starting;
    assert!(matches!(
        db.store.update(stale).await,
        Err(StoreError::Conflict(_))
    ));

    let mut next = first.clone();
    next.state = JobState::Running;
    next.flink_job_id = Some("0".repeat(32));
    let next = db.store.update(next).await.unwrap();
    assert_eq!(next.version, 2);
    let stored = db.store.get("ns1", &j.id).await.unwrap().unwrap();
    assert_eq!(stored.state, JobState::Running);
    assert_eq!(stored.version, 2);
    assert_eq!(db.store.running("ns1", "eng_x").await.unwrap().len(), 1);

    let mut ghost = job("ghost", "eng_x");
    ghost.state = JobState::Running;
    assert!(matches!(
        db.store.update(ghost).await,
        Err(StoreError::NotFound(_))
    ));
    db.drop_db().await;
}

#[tokio::test]
async fn racing_updates_have_one_winner() {
    let Some(db) = TestDb::create("race").await else {
        return;
    };
    db.store.migrate().await.unwrap();
    let j = db.store.insert(job("a", "eng_x")).await.unwrap();
    let store = Arc::new(db.store.clone());
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let mut mine = j.clone();
        mine.state = JobState::Starting;
        tasks.push(tokio::spawn(async move { store.update(mine).await }));
    }
    let mut wins = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => wins += 1,
            Err(StoreError::Conflict(_)) => {}
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(wins, 1);
    db.drop_db().await;
}

#[tokio::test]
async fn delete() {
    let Some(db) = TestDb::create("delete").await else {
        return;
    };
    db.store.migrate().await.unwrap();
    let j = db.store.insert(job("a", "eng_x")).await.unwrap();
    assert!(db.store.delete("ns1", &j.id).await.unwrap());
    assert!(!db.store.delete("ns1", &j.id).await.unwrap());
    assert!(db.store.get("ns1", &j.id).await.unwrap().is_none());
    db.drop_db().await;
}
