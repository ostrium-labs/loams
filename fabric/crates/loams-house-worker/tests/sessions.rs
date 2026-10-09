//! Sessions, settings and the classifier over HTTP (HS1 Task 4; FL2 Task 4's
//! tests and the pinning of §49 §10.3), against real workers.

mod common;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::http::{Response, request, target};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve};
use loams_house::{PoolConfig, ProcessLauncher, WorkerPool};

async fn house(test: &str, pool_config: PoolConfig) -> (HouseHandle, WorkerPool) {
    let launcher = ProcessLauncher::new(common::WORKER, common::tmp_root(test));
    let pool = WorkerPool::start(pool_config, Arc::new(launcher))
        .await
        .expect("pool");
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![
            UserMap::dev("alice", "a", 2, false),
            UserMap::dev("bob", "b", 2, false),
        ],
        tmp_dir: common::tmp_root(&format!("{test}-spool")),
        ..HouseConfig::default()
    };
    (serve(config, pool.clone()).await.expect("serves"), pool)
}

fn per_namespace(workers: usize) -> PoolConfig {
    PoolConfig {
        max_workers_per_namespace: workers,
        ..common::small(workers)
    }
}

async fn call(
    addr: SocketAddr,
    method: &'static str,
    params: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        request(addr, method, &target(&pairs), &[], &body)
    })
    .await
    .expect("client")
}

/// One statement as `user` in session `id`, with extra parameters.
async fn in_session(
    addr: SocketAddr,
    user: &'static str,
    id: &str,
    sql: &str,
    extra: &[(&'static str, &str)],
) -> Response {
    let password = if user == "alice" { "a" } else { "b" };
    let mut params = vec![
        ("user", user.to_string()),
        ("password", password.to_string()),
        ("session_id", id.to_string()),
        ("query", sql.to_string()),
    ];
    params.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    call(addr, "POST", params, Vec::new()).await
}

fn code(response: &Response) -> Option<&str> {
    response.header("X-ClickHouse-Exception-Code")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_persists_settings_and_database() {
    let (house, _pool) = house("persist", per_namespace(2)).await;
    let addr = house.local_addr();
    let set = in_session(
        addr,
        "alice",
        "s",
        "SET max_threads = 3, max_block_size = 777",
        &[],
    )
    .await;
    assert_eq!(set.status, 200, "{}", set.text());
    assert!(set.body.is_empty());
    assert_eq!(
        in_session(addr, "alice", "s", "USE default", &[])
            .await
            .status,
        200
    );
    let read = in_session(
        addr,
        "alice",
        "s",
        "SELECT getSetting('max_threads'), getSetting('max_block_size'), currentDatabase()",
        &[],
    )
    .await;
    assert_eq!(read.text(), "3\t777\tdefault\n");
    // A URL setting wins for its statement only.
    let url = in_session(
        addr,
        "alice",
        "s",
        "SELECT getSetting('max_threads')",
        &[("max_threads", "2")],
    )
    .await;
    assert_eq!(url.text(), "2\n");
    assert_eq!(
        in_session(addr, "alice", "s", "SELECT getSetting('max_threads')", &[])
            .await
            .text(),
        "3\n"
    );
    // Another user's session of the same id, and no session at all, have neither.
    assert_ne!(
        in_session(addr, "bob", "s", "SELECT getSetting('max_threads')", &[])
            .await
            .text(),
        "3\n"
    );
    // Only `default` exists until the catalog (HS1 Task 10).
    assert_eq!(
        code(&in_session(addr, "alice", "s", "USE elsewhere", &[]).await),
        Some("81")
    );
    // SET without a session is accepted and keeps nothing, as in ClickHouse.
    let bare = call(
        addr,
        "POST",
        vec![
            ("user", "alice".into()),
            ("password", "a".into()),
            ("query", "SET max_threads = 3".into()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(bare.status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_check_unknown() {
    let (house, _pool) = house("check", per_namespace(1)).await;
    let addr = house.local_addr();
    let unknown = in_session(addr, "alice", "nope", "SELECT 1", &[("session_check", "1")]).await;
    assert_eq!(code(&unknown), Some("372"), "{}", unknown.text());
    assert!(unknown.text().contains("Session nope not found"));
    assert_eq!(
        in_session(addr, "alice", "yes", "SELECT 1", &[])
            .await
            .status,
        200
    );
    assert_eq!(
        in_session(addr, "alice", "yes", "SELECT 1", &[("session_check", "1")])
            .await
            .status,
        200
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_locked() {
    let (house, _pool) = house("locked", per_namespace(2)).await;
    let addr = house.local_addr();
    let slow = tokio::spawn(async move {
        in_session(
            addr,
            "alice",
            "busy",
            "SELECT sleepEachRow(0.3) FROM numbers(3) SETTINGS max_block_size = 1",
            &[],
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = in_session(addr, "alice", "busy", "SELECT 1", &[]).await;
    assert_eq!(code(&second), Some("373"), "{}", second.text());
    assert_eq!(slow.await.expect("slow").status, 200);
    assert_eq!(
        in_session(addr, "alice", "busy", "SELECT 1", &[])
            .await
            .status,
        200,
        "free again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_setting_is_115() {
    let (house, _pool) = house("unknown-setting", per_namespace(1)).await;
    let addr = house.local_addr();
    let url = in_session(
        addr,
        "alice",
        "s",
        "SELECT 1",
        &[("no_such_setting_x", "1")],
    )
    .await;
    assert_eq!(code(&url), Some("115"), "{}", url.text());
    assert_eq!(url.status, 404);
    let set = in_session(addr, "alice", "s", "SET no_such_setting_x = 1", &[]).await;
    assert_eq!(code(&set), Some("115"), "{}", set.text());
    // A real setting that is not allowed is 164, not 115.
    let disallowed = in_session(addr, "alice", "s", "SET allow_ddl = 1", &[]).await;
    assert_eq!(code(&disallowed), Some("164"), "{}", disallowed.text());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setting_over_cap_is_164() {
    let (house, _pool) = house("over-cap", per_namespace(1)).await;
    let addr = house.local_addr();
    let memory = in_session(
        addr,
        "alice",
        "s",
        "SELECT 1",
        &[("max_memory_usage", "5368709120")],
    )
    .await;
    assert_eq!(code(&memory), Some("164"), "{}", memory.text());
    assert!(
        memory
            .text()
            .contains("max_memory_usage shouldn't be greater than 4294967296"),
        "{}",
        memory.text()
    );
    let time = in_session(addr, "alice", "s", "SET max_execution_time = 301", &[]).await;
    assert_eq!(code(&time), Some("164"), "{}", time.text());
    let unlimited = in_session(addr, "alice", "s", "SET max_memory_usage = 0", &[]).await;
    assert_eq!(
        code(&unlimited),
        Some("164"),
        "never clamped, never unlimited"
    );
    assert_eq!(
        in_session(addr, "alice", "s", "SET max_execution_time = 300", &[])
            .await
            .status,
        200
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn temporary_table_pins_a_worker() {
    let (house, pool) = house("pins", per_namespace(3)).await;
    let addr = house.local_addr();
    let create = in_session(
        addr,
        "alice",
        "t",
        "CREATE TEMPORARY TABLE tt (n UInt8) ENGINE = Memory",
        &[],
    )
    .await;
    assert_eq!(create.status, 200, "{}", create.text());
    let worker = create
        .header("X-Loams-Worker")
        .expect("the worker")
        .to_string();
    assert_eq!(pool.stats().pinned, 1);
    let affinity = create
        .header("X-Loams-Session-Affinity")
        .expect("affinity")
        .to_string();

    // Other traffic in the namespace never lands on the pinned worker.
    let others = (0..4).map(|_| {
        tokio::spawn(async move {
            call(
                addr,
                "GET",
                vec![
                    ("user", "bob".into()),
                    ("password", "b".into()),
                    (
                        "query",
                        "SELECT sleepEachRow(0.05) FROM numbers(2) SETTINGS max_block_size = 1"
                            .into(),
                    ),
                ],
                Vec::new(),
            )
            .await
        })
    });
    for _ in 0..5 {
        let read = in_session(addr, "alice", "t", "SELECT count() FROM tt", &[]).await;
        assert_eq!(read.text(), "0\n", "the temporary table is there");
        assert_eq!(
            read.header("X-Loams-Worker"),
            Some(worker.as_str()),
            "on the pinned worker"
        );
        assert_eq!(
            read.header("X-Loams-Session-Affinity"),
            Some(affinity.as_str())
        );
    }
    for other in others {
        let other = other.await.expect("other");
        assert_eq!(other.status, 200, "{}", other.text());
        assert_ne!(
            other.header("X-Loams-Worker"),
            Some(worker.as_str()),
            "a pinned worker is reserved"
        );
        assert_eq!(
            other.header("X-Loams-Session-Affinity"),
            None,
            "no session, no affinity"
        );
    }
    // Ending the session releases the pin.
    in_session(addr, "alice", "t", "SELECT 1", &[("close_session", "1")]).await;
    assert_eq!(pool.stats().pinned, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn third_pinned_worker_is_202() {
    let (house, pool) = house("third-pin", per_namespace(3)).await;
    let addr = house.local_addr();
    for id in ["p1", "p2"] {
        let create = in_session(
            addr,
            "alice",
            id,
            "CREATE TEMPORARY TABLE tt (n UInt8) ENGINE = Memory",
            &[],
        )
        .await;
        assert_eq!(create.status, 200, "{id}: {}", create.text());
    }
    let third = in_session(
        addr,
        "alice",
        "p3",
        "CREATE TEMPORARY TABLE tt (n UInt8) ENGINE = Memory",
        &[],
    )
    .await;
    assert_eq!(code(&third), Some("202"), "{}", third.text());
    assert_eq!(pool.stats().pinned, 2);
    // Statements without temporary tables are not limited by it.
    assert_eq!(
        in_session(addr, "alice", "p3", "SELECT 1", &[])
            .await
            .status,
        200
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_survives_worker_recycle_without_temp_tables() {
    let pool_config = PoolConfig {
        max_queries_per_worker: 2,
        ..per_namespace(2)
    };
    let (house, pool) = house("recycle", pool_config).await;
    let addr = house.local_addr();
    assert_eq!(
        in_session(addr, "alice", "r", "SET max_threads = 3", &[])
            .await
            .status,
        200
    );
    let mut workers = HashSet::new();
    for _ in 0..6 {
        let read = in_session(addr, "alice", "r", "SELECT getSetting('max_threads')", &[]).await;
        assert_eq!(
            read.text(),
            "3\n",
            "the session's settings live in the front"
        );
        workers.insert(read.header("X-Loams-Worker").expect("worker").to_string());
    }
    assert!(
        workers.len() >= 2,
        "workers were recycled under it: {workers:?}"
    );
    assert!(pool.stats().kills_for(loams_house::ExitReason::Budget) >= 2);
}

/// FL2 Ruling 6 end to end: what sqlparser cannot parse is decided by ClickHouse's
/// class from the worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unparsed_statements_are_decided_by_clickhouse() {
    let (house, _pool) = house("unparsed", per_namespace(1)).await;
    let addr = house.local_addr();
    let ddl = "CREATE TABLE t (a UInt64 CODEC(ZSTD(3))) ENGINE = MergeTree ORDER BY a";
    let refused = in_session(addr, "alice", "u", ddl, &[]).await;
    assert_eq!(code(&refused), Some("62"), "{}", refused.text());
    assert!(
        refused
            .text()
            .contains(loams_house::classify::OUTSIDE_SURFACE),
        "{}",
        refused.text()
    );
    // Not ClickHouse either: chDB answers its own 62.
    let garbage = in_session(addr, "alice", "u", "SELEC 1", &[]).await;
    assert_eq!(code(&garbage), Some("62"), "{}", garbage.text());
    assert!(
        !garbage
            .text()
            .contains(loams_house::classify::OUTSIDE_SURFACE)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owned_statements_of_later_tasks_are_48() {
    let (house, _pool) = house("later", per_namespace(1)).await;
    let addr = house.local_addr();
    for (sql, task) in [
        (
            "CREATE TABLE t (a UInt64) ENGINE = MergeTree ORDER BY a",
            "Task 10",
        ),
        (
            "CREATE TABLE q (a String) ENGINE = LoamsStream('s', 'JSONEachRow')",
            "Tasks 16 and 18",
        ),
        ("OPTIMIZE TABLE t FINAL", "Task 13"),
        ("KILL QUERY WHERE query_id = 'x'", "Task 21"),
        ("UNDROP TABLE t", "Task 15"),
    ] {
        let response = in_session(addr, "alice", "l", sql, &[]).await;
        assert_eq!(code(&response), Some("48"), "{sql}: {}", response.text());
        assert!(response.text().contains(task), "{sql}: {}", response.text());
    }
    let access = in_session(addr, "alice", "l", "GRANT SELECT ON *.* TO u", &[]).await;
    assert_eq!(code(&access), Some("48"), "{}", access.text());
}
