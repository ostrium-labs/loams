//! The engine's own access control (HS1 ruling R-ACL.1): with the deny list (L1)
//! off, chDB itself refuses what a House worker may not do.
//!
//! Every statement here goes to a worker through `WorkerLease::run`, which skips
//! L1 (R6.6), so what refuses it is the engine or the sandbox. Each case runs
//! twice: on a worker with the House's grants and settings profile ("sealed"),
//! where the engine's access control must refuse it, and on a worker started
//! with `--test-open-acl` (chDB's own all-powerful `default`, no profile), where
//! it must get past the engine's access control — so the refusal is shown to
//! come from the grants and the profile, not from something else. Cases whose
//! refusal is not the grants' (`INTO OUTFILE`, `executable()`, the scalar
//! `file()`) are listed with the layer that does refuse them and must be refused
//! by both workers.

#![cfg(target_os = "linux")]

mod common;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use arrow::array::UInt64Array;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use bytes::Bytes;
use common::{small, statement, tmp_root};
use loams_cache::RangeCacheConfig;
use loams_house::{HouseError, Outcome, ProcessLauncher, WorkerLease, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_ipc::policy::HOUSE_SETTINGS;
use parquet::arrow::ArrowWriter;

/// Which access control a worker starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Acl {
    /// The House's grants and settings profile.
    Sealed,
    /// `--test-open-acl`: chDB's own `default`, no profile.
    Open,
}

fn launcher(test: &str, acl: Acl) -> ProcessLauncher {
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root(&format!("{test}-{acl:?}")));
    match acl {
        Acl::Sealed => launcher,
        Acl::Open => launcher.with_open_acl(),
    }
}

async fn pool(test: &str, acl: Acl) -> WorkerPool {
    WorkerPool::start(small(1), Arc::new(launcher(test, acl)))
        .await
        .expect("the pool starts")
}

/// One statement with L1 off, its views (run on the control connection) first.
/// On an open worker, chDB's clients are told not to retry, so a statement past
/// the grants fails fast on the network; a sealed worker pins those settings.
async fn run(
    lease: &mut WorkerLease,
    acl: Acl,
    views: &[String],
    sql: &str,
) -> Result<Vec<u8>, HouseError> {
    let mut execute = statement(sql, "TSV");
    execute.views = views.to_vec();
    if acl == Acl::Open {
        execute.settings = [
            ("http_max_tries", "1"),
            ("s3_retry_attempts", "0"),
            ("http_retry_max_backoff_ms", "10"),
            ("connect_timeout_with_failover_ms", "100"),
            ("external_storage_connect_timeout_sec", "1"),
        ]
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    }
    execute.limits.max_execution_time_ms = Some(20_000);
    tokio::time::timeout(Duration::from_secs(60), lease.run(execute))
        .await
        .unwrap_or_else(|_| panic!("{sql}: bounded"))
        .map(|collected| collected.bytes)
}

/// Whether `err` is the engine's access control: a missing grant (`497`), a
/// pinned or capped setting (`452`), or a privilege `readonly` takes away
/// (`164`).
fn is_access_control(err: &HouseError) -> bool {
    match err.code() {
        497 => err.message().contains("Not enough privileges"),
        452 | 164 => true,
        _ => false,
    }
}

/// A loopback port nothing in a worker's network namespace can reach: what a
/// source names, so that a statement past the grants fails on the network, not
/// on a grant.
fn trap() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    (listener, format!("http://127.0.0.1:{port}"))
}

/// Statements on the user connection that only the grants or the profile
/// refuse.
fn user_cases(url: &str) -> Vec<String> {
    vec![
        // Every source the pinned chDB has a grant for (R-ACL.1).
        "SELECT * FROM file('/etc/hostname', LineAsString)".to_string(),
        format!("SELECT * FROM url('{url}/x', LineAsString)"),
        format!("SELECT * FROM s3('{url}/bkt/k', 'dummy', 'dummy', LineAsString)"),
        format!("SELECT * FROM icebergS3('{url}/bkt/t/', 'dummy', 'dummy')"),
        "SELECT * FROM s3('http://127.0.0.1:9180/bkt/ns/2/house/d/t/data/x.parquet', 'dummy', \
         'dummy', Parquet)"
            .to_string(),
        "SELECT * FROM remote('127.0.0.1:1', system.one)".to_string(),
        "SELECT * FROM mysql('127.0.0.1:1', 'd', 't', 'u', 'p')".to_string(),
        "SELECT * FROM postgresql('127.0.0.1:1', 'd', 't', 'u', 'p')".to_string(),
        "SELECT * FROM mongodb('127.0.0.1:1', 'd', 'c', 'u', 'p', 'a String')".to_string(),
        "SELECT * FROM redis('127.0.0.1:1', 'k', 'k String')".to_string(),
        "SELECT * FROM sqlite('loams-acl.db', 't')".to_string(),
        "SELECT * FROM hdfs('hdfs://127.0.0.1:1/x', CSV, 'a String')".to_string(),
        format!("SELECT * FROM azureBlobStorage('{url}/a', 'c', 'b', 'CSV', 'a String')"),
        // Writes to sources.
        "INSERT INTO FUNCTION file('loams-acl.csv', CSV, 'a String') SELECT 'x'".to_string(),
        format!("INSERT INTO FUNCTION url('{url}/x', CSV, 'a String') SELECT 'x'"),
        format!("CREATE TEMPORARY TABLE u (a String) ENGINE = URL('{url}/x', CSV)"),
        // Introspection, and settings outside the lists or over their caps.
        "SELECT addressToLine(toUInt64(0)) SETTINGS allow_introspection_functions = 1".to_string(),
        "SELECT 1 SETTINGS max_rows_to_group_by = 1".to_string(),
        "SELECT 1 SETTINGS max_bytes_before_external_sort = 1".to_string(),
        "SELECT 1 SETTINGS max_execution_time = 901".to_string(),
        "SELECT 1 SETTINGS max_memory_usage = 0".to_string(),
        "SELECT 1 SETTINGS distributed_foreground_insert = 1".to_string(),
    ]
}

/// Statements on the control connection (a statement's `views`) in the shapes
/// R11.11's check lets through, which the grants must still confine to the read
/// views (`acl_engine.rs` covers the statements that check stops first). The
/// control connection takes no per-statement settings, so on an open worker the
/// statements that connect carry their own (a sealed worker refuses before
/// reading them).
fn control_cases(url: &str, acl: Acl) -> Vec<String> {
    let fast = match acl {
        Acl::Sealed => "",
        Acl::Open => " SETTINGS http_max_tries = 1, http_retry_max_backoff_ms = 10",
    };
    vec![
        // A view that does not name its columns is analysed at creation, and its
        // source needs a grant.
        format!(
            "CREATE OR REPLACE VIEW default.loams_acl_u AS\n\
             SELECT * FROM url('{url}/x', LineAsString){fast}"
        ),
        // No view or database DDL in the engine's own databases.
        "CREATE OR REPLACE VIEW system.loams_acl_v AS\nSELECT 1".to_string(),
        "CREATE OR REPLACE VIEW information_schema.loams_acl_v AS\nSELECT 1".to_string(),
        "DROP TABLE IF EXISTS system.one".to_string(),
        // The profile binds the control connection too.
        "CREATE OR REPLACE VIEW default.loams_acl_s (a UInt8) SQL SECURITY NONE AS\n\
         SELECT 1 SETTINGS max_rows_to_group_by = 1"
            .to_string(),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_refuses_without_the_deny_list() {
    let (_listener, url) = trap();
    for acl in [Acl::Sealed, Acl::Open] {
        let pool = pool("acl-refuses", acl).await;
        let mut lease = pool.acquire("ns-1").await.expect("worker");
        let mut failures = Vec::new();
        for sql in user_cases(&url) {
            let result = run(&mut lease, acl, &[], &sql).await;
            match (acl, &result) {
                (Acl::Sealed, Err(err)) if is_access_control(err) => {}
                (Acl::Open, Ok(_)) => {}
                (Acl::Open, Err(err)) if !is_access_control(err) => {}
                _ => failures.push(format!("{acl:?} user: {sql}: {result:?}")),
            }
        }
        for sql in control_cases(&url, acl) {
            let result = run(&mut lease, acl, std::slice::from_ref(&sql), "SELECT 1").await;
            match (acl, &result) {
                (Acl::Sealed, Err(err)) if is_access_control(err) => {}
                (Acl::Open, Ok(_)) => {}
                (Acl::Open, Err(err)) if !is_access_control(err) => {}
                _ => failures.push(format!("{acl:?} control: {sql}: {result:?}")),
            }
        }
        pool.release(lease, Outcome::Completed);
        pool.shutdown();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

/// What the grants do not govern, and the layer that refuses it instead: both
/// workers refuse these, with L1 off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_the_grants_do_not_govern_is_refused_elsewhere() {
    for acl in [Acl::Sealed, Acl::Open] {
        let pool = pool("acl-elsewhere", acl).await;
        let mut lease = pool.acquire("ns-1").await.expect("worker");
        for (sql, layer) in [
            // chDB's client layer writes and reads these whatever the grants say
            // (R1.10, measured again on 26.9.2.1): Landlock refuses any path
            // outside the private directory.
            (
                "SELECT 1 INTO OUTFILE '/tmp/loams-acl-out.tsv'",
                "L3 Landlock",
            ),
            ("SELECT file('/etc/hostname')", "L3 Landlock"),
            // No grant names a script: `user_scripts_path` does not exist (R5.6)
            // and seccomp refuses `execve` (R6.1).
            (
                "SELECT * FROM executable('cat', TSV, 'a String')",
                "L2 config, L3 seccomp",
            ),
            // `readonly = 2` and `allow_ddl = 0` on the user connection, sticky:
            // `readonly` also takes `ALLOW SQL SECURITY NONE` away, so the user
            // connection cannot make a view that would lend it a source.
            (
                "CREATE TEMPORARY VIEW tv (a String) SQL SECURITY NONE AS \
                 SELECT * FROM url('http://127.0.0.1:1/x', LineAsString)",
                "readonly",
            ),
            ("CREATE DATABASE loams_acl_d", "readonly"),
            ("SYSTEM DROP DNS CACHE", "readonly"),
        ] {
            let err = run(&mut lease, acl, &[], sql).await.expect_err(sql);
            assert!(err.code() != 0, "{acl:?} {sql} ({layer}): {err}");
        }
        pool.release(lease, Outcome::Completed);
        pool.shutdown();
    }
}

/// The profile leaves changeable exactly what the front allows (the shared
/// lists) and what the House's own statements set, and its list of chDB's
/// settings is the pinned chDB's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_profile_follows_the_lists_and_chdb() {
    let pool = pool("acl-profile", Acl::Sealed).await;
    let mut lease = pool.acquire("ns-1").await.expect("worker");
    let listed = run(
        &mut lease,
        Acl::Sealed,
        &[],
        "SELECT name, alias_for, readonly FROM system.settings ORDER BY name",
    )
    .await
    .expect("settings");
    let text = String::from_utf8(listed).expect("utf-8");
    let mut engine: Vec<(String, Option<String>)> = Vec::new();
    let mut changeable = std::collections::HashSet::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, alias_for, readonly] = fields[..] else {
            panic!("{line}");
        };
        engine.push((
            name.to_string(),
            (!alias_for.is_empty()).then(|| alias_for.to_string()),
        ));
        if alias_for.is_empty() && readonly == "0" {
            changeable.insert(name.to_string());
        }
    }
    // The committed list is the pinned chDB's (`the_settings_list_follows_chdb`).
    let committed: Vec<(String, Option<String>)> = loams_house_worker::settings::chdb_settings()
        .map(|(n, a)| (n.to_string(), a.map(str::to_string)))
        .collect();
    assert_eq!(
        committed, engine,
        "chdb_settings.tsv must be regenerated for this chDB"
    );

    // What the engine leaves changeable on a user connection is what the front
    // allows (by a name or an alias of it), plus the House's own settings.
    for (name, alias_for) in &engine {
        if alias_for.is_some() {
            continue;
        }
        let names: Vec<&str> = std::iter::once(name.as_str())
            .chain(
                engine
                    .iter()
                    .filter(|(_, a)| a.as_deref() == Some(name.as_str()))
                    .map(|(n, _)| n.as_str()),
            )
            .collect();
        // The front's own settings (`insert_deduplication_token`, HS1 Task 12)
        // never reach chDB, so the engine keeps them pinned.
        let allowed = names.iter().any(|n| {
            (loams_house::settings::is_allowed(n) && !loams_house::settings::is_front_only(n))
                || HOUSE_SETTINGS.contains(n)
        });
        // `readonly` and `allow_ddl` are set on the user connection itself and
        // show as unchangeable there: neither can be set back (R5.6).
        if name == "readonly" || name == "allow_ddl" {
            continue;
        }
        assert_eq!(
            changeable.contains(name),
            allowed,
            "{name}: the engine and the lists disagree"
        );
    }
    pool.release(lease, Outcome::Completed);
    pool.shutdown();
}

/// `rows` numbers in one Parquet file.
fn parquet(rows: u64) -> Bytes {
    let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::UInt64, false)]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(UInt64Array::from_iter_values(0..rows))],
    )
    .expect("batch");
    let mut out = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut out, schema, None).expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    Bytes::from(out)
}

/// The bucket is reached only through the views the control connection makes:
/// a statement cannot name it, its own namespace's objects included, and
/// another namespace's objects are refused by the engine before `house-cache`
/// is asked. With the grants open, `house-cache` is what refuses them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_views_reach_the_bucket() {
    let dir = tmp_root("acl-bucket-store");
    let upstream = Upstream::open(
        &format!("file://{}", dir.display()),
        StoreOptions(Vec::new()),
        RangeCacheConfig {
            block_size: 256 * 1024,
            memory_bytes: 64 << 20,
            disk: None,
        },
    )
    .await
    .expect("the upstream opens");
    let upstream = Arc::new(upstream);
    for (key, rows) in [
        ("ns/tenant-a/t.parquet", 1_000),
        ("ns/tenant-b/t.parquet", 10),
    ] {
        upstream.store().put(key, parquet(rows)).await.expect("put");
    }
    let prefix = format!("http://127.0.0.1:9180/{}", upstream.bucket());
    let own = format!("{prefix}/ns/tenant-a/t.parquet");
    let foreign = format!("{prefix}/ns/tenant-b/t.parquet");
    let direct = |url: &str| format!("SELECT count() FROM s3('{url}', 'dummy', 'dummy', Parquet)");

    for acl in [Acl::Sealed, Acl::Open] {
        let proxy = HouseCache::new(Arc::new(OneStore(Arc::clone(&upstream))));
        let launcher = launcher("acl-bucket", acl).with_forward_target(Arc::new(proxy.clone()));
        let pool = WorkerPool::start(small(1), Arc::new(launcher))
            .await
            .expect("the pool starts");
        let mut lease = pool.acquire("tenant-a").await.expect("worker");

        // The read path: a view the control connection makes, as `read.rs`
        // makes them, columns named and `SQL SECURITY NONE`.
        let views = vec![
            "CREATE DATABASE IF NOT EXISTS d ENGINE = Memory".to_string(),
            format!(
                "CREATE OR REPLACE VIEW d.t (n UInt64) SQL SECURITY NONE AS\n\
                 SELECT n FROM s3('{own}', 'dummy', 'dummy', Parquet)"
            ),
        ];
        let read = run(&mut lease, acl, &views, "SELECT count(), sum(n) FROM d.t")
            .await
            .expect("the view reads");
        assert_eq!(String::from_utf8_lossy(&read).trim(), "1000\t499500");

        let refused = proxy.metrics().namespaces["tenant-a"].refused;
        for url in [&own, &foreign] {
            let result = run(&mut lease, acl, &[], &direct(url)).await;
            match acl {
                Acl::Sealed => {
                    let err = result.expect_err(url);
                    assert!(is_access_control(&err), "{url}: {err}");
                }
                Acl::Open if url == &own => {
                    assert_eq!(result.expect(url), b"1000\n");
                }
                Acl::Open => assert!(result.is_err(), "house-cache refuses {url}"),
            }
        }
        let refused_now = proxy.metrics().namespaces["tenant-a"].refused;
        match acl {
            Acl::Sealed => assert_eq!(refused_now, refused, "the engine refused first"),
            Acl::Open => assert!(refused_now > refused, "house-cache refused"),
        }
        pool.release(lease, Outcome::Completed);
        pool.shutdown();
    }
}
