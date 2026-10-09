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
    // Not ClickHouse either (fix round 1, I2): refused before chDB runs it, with
    // the hint, on POST and GET alike.
    let garbage = in_session(addr, "alice", "u", "SELEC 1", &[]).await;
    assert_eq!(code(&garbage), Some("62"), "{}", garbage.text());
    assert!(
        garbage
            .text()
            .contains(loams_house::classify::OUTSIDE_SURFACE),
        "{}",
        garbage.text()
    );
    let get = as_alice(addr, "GET", "SELEC 1", b"").await;
    assert_eq!(code(&get), Some("62"), "{}", get.text());
    assert!(
        get.text().contains(loams_house::classify::OUTSIDE_SURFACE),
        "{}",
        get.text()
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

/// A directory under the worktree's `scratch/` for files a test watches; removed
/// when dropped.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../scratch")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir.canonicalize().expect("absolute"))
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).display().to_string()
    }

    fn files(&self) -> Vec<String> {
        std::fs::read_dir(&self.0)
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn as_alice(addr: SocketAddr, method: &'static str, sql: &str, body: &[u8]) -> Response {
    call(
        addr,
        method,
        vec![
            ("user", "alice".to_string()),
            ("password", "a".to_string()),
            ("query", sql.to_string()),
        ],
        body.to_vec(),
    )
    .await
}

/// Fix round 1, C1 and C2: `INTO OUTFILE` wrote a host file and `FROM INFILE` read
/// one, whatever `readonly` and the grants say (R1.10). Both are `344` on every
/// path — POST, GET, in a session, and with the INSERT's body — and nothing is
/// written or read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_files_are_never_written_or_read() {
    let (house, _pool) = house("hostio", per_namespace(1)).await;
    let addr = house.local_addr();
    let dir = Scratch::new("t4fix1-hostio");
    let writes = [
        format!("SELECT 1 INTO OUTFILE '{}' APPEND", dir.path("select.tsv")),
        format!("SHOW TABLES INTO OUTFILE '{}'", dir.path("show.tsv")),
        format!(
            "DESCRIBE TABLE system.one INTO OUTFILE '{}'",
            dir.path("describe.tsv")
        ),
        format!(
            "EXPLAIN SELECT 1 INTO OUTFILE '{}'",
            dir.path("explain.tsv")
        ),
        format!(
            "SELECT $$'$$ INTO OUTFILE '{}' --'",
            dir.path("heredoc.tsv")
        ),
    ];
    for sql in &writes {
        for method in ["POST", "GET"] {
            let response = as_alice(addr, method, sql, b"").await;
            assert_eq!(
                code(&response),
                Some("344"),
                "{method} {sql}: {}",
                response.text()
            );
        }
        let response = in_session(addr, "alice", "h", sql, &[]).await;
        assert_eq!(code(&response), Some("344"), "{sql}: {}", response.text());
    }
    assert!(dir.files().is_empty(), "written: {:?}", dir.files());

    let input = dir.path("in.csv");
    std::fs::write(&input, "7\n").expect("input file");
    let create = in_session(
        addr,
        "alice",
        "h",
        "CREATE TEMPORARY TABLE t (a UInt8) ENGINE = Memory",
        &[],
    )
    .await;
    assert_eq!(create.status, 200, "{}", create.text());
    for sql in [
        format!("INSERT INTO t FROM INFILE '{input}' FORMAT CSV"),
        format!("INSERT INTO t FROM INFILE '{input}'"),
        format!("INSERT INTO t FROM INFILE '{input}' COMPRESSION 'none' FORMAT CSV"),
        format!("INSERT INTO FUNCTION null('a UInt8') FROM INFILE '{input}' FORMAT CSV"),
        format!("INSERT INTO FUNCTION null($$'$$) FROM INFILE '{input}' --')\nFORMAT CSV"),
    ] {
        let response = in_session(addr, "alice", "h", &sql, &[]).await;
        assert_eq!(code(&response), Some("344"), "{sql}: {}", response.text());
        // The same head with its data in the body.
        let response = call(
            addr,
            "POST",
            vec![
                ("user", "alice".to_string()),
                ("password", "a".to_string()),
                ("session_id", "h".to_string()),
                ("query", sql.clone()),
            ],
            b"1\n".to_vec(),
        )
        .await;
        assert_eq!(code(&response), Some("344"), "{sql}: {}", response.text());
    }
    let count = in_session(addr, "alice", "h", "SELECT count() FROM t", &[]).await;
    assert_eq!(count.text(), "0\n", "nothing was read into t");
}

/// Fix round 1, C1: every query is gated by ClickHouse's own class, not only by the
/// front's keyword route. `SELECT 1 PARALLEL WITH CREATE TEMPORARY TABLE …` opens
/// with a query keyword and has no `;`, but ClickHouse classes it `Control`: it is
/// refused on GET and POST, and no table appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_query_is_gated_by_clickhouse_class() {
    let (house, _pool) = house("gate", per_namespace(1)).await;
    let addr = house.local_addr();
    let sql = "SELECT 1 PARALLEL WITH CREATE TEMPORARY TABLE x (a UInt8) ENGINE = Memory";
    let get = call(
        addr,
        "GET",
        vec![
            ("user", "alice".to_string()),
            ("password", "a".to_string()),
            ("session_id", "g".to_string()),
            ("query", sql.to_string()),
        ],
        Vec::new(),
    )
    .await;
    assert!(code(&get).is_some(), "refused on GET: {}", get.text());
    let post = in_session(addr, "alice", "g", sql, &[]).await;
    assert_eq!(code(&post), Some("62"), "{}", post.text());
    let read = in_session(addr, "alice", "g", "SELECT count() FROM x", &[]).await;
    assert_eq!(code(&read), Some("60"), "no table x: {}", read.text());
    // Reads still run, on GET too.
    assert_eq!(as_alice(addr, "GET", "SELECT 1", b"").await.text(), "1\n");
    assert_eq!(
        as_alice(addr, "GET", "SHOW DATABASES LIKE 'default'", b"")
            .await
            .text(),
        "default\n"
    );
}

/// Fix round 1, I1: one statement per request, keyword-routed ones included. A `;`
/// followed by more is `62` (a heredoc does not hide it), and so is anything
/// ClickHouse counts as more than one statement; nothing of it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_statement_per_request_on_every_route() {
    let (house, _pool) = house("multi", per_namespace(1)).await;
    let addr = house.local_addr();
    let create = in_session(
        addr,
        "alice",
        "m",
        "CREATE TEMPORARY TABLE t (a UInt8) ENGINE = Memory",
        &[],
    )
    .await;
    assert_eq!(create.status, 200, "{}", create.text());
    for sql in [
        "DROP TEMPORARY TABLE t; CREATE DATABASE evil",
        "DROP TEMPORARY TABLE t;CREATE DATABASE evil;",
        "SELECT 1; CREATE DATABASE evil",
        "SELECT $$'$$; CREATE DATABASE evil --'",
        "SHOW TABLES; CREATE DATABASE evil",
        "INSERT INTO t VALUES (1); CREATE DATABASE evil",
        "CREATE TEMPORARY TABLE u (a UInt8) ENGINE = Memory; CREATE DATABASE evil",
    ] {
        let response = in_session(addr, "alice", "m", sql, &[]).await;
        assert_eq!(code(&response), Some("62"), "{sql}: {}", response.text());
    }
    // A write that carries another statement ClickHouse's way, with no `;`.
    let parallel = in_session(
        addr,
        "alice",
        "m",
        "INSERT INTO t SELECT 1 PARALLEL WITH CREATE DATABASE evil",
        &[],
    )
    .await;
    assert_eq!(code(&parallel), Some("62"), "{}", parallel.text());
    // Nothing ran: t is still there and empty, and there is no database evil.
    let count = in_session(addr, "alice", "m", "SELECT count() FROM t", &[]).await;
    assert_eq!(count.text(), "0\n", "{}", count.text());
    let evil = in_session(addr, "alice", "m", "EXISTS DATABASE evil", &[]).await;
    assert_eq!(evil.text(), "0\n", "{}", evil.text());
    // One statement with trailing `;`s and comments still runs, and so does each
    // owned write, as ClickHouse classes it.
    let one = in_session(addr, "alice", "m", "SELECT 1; -- done\n;", &[]).await;
    assert_eq!(one.text(), "1\n", "{}", one.text());
    let values = in_session(addr, "alice", "m", "INSERT INTO t VALUES (1)", &[]).await;
    assert_eq!(values.status, 200, "{}", values.text());
    for (sql, body) in [
        ("INSERT INTO t FORMAT CSV", &b"2\n"[..]),
        ("INSERT INTO t VALUES", &b"(3)"[..]),
    ] {
        let streamed = call(
            addr,
            "POST",
            vec![
                ("user", "alice".to_string()),
                ("password", "a".to_string()),
                ("session_id", "m".to_string()),
                ("query", sql.to_string()),
            ],
            body.to_vec(),
        )
        .await;
        assert_eq!(streamed.status, 200, "{sql}: {}", streamed.text());
    }
    let sum = in_session(addr, "alice", "m", "SELECT sum(a) FROM t", &[]).await;
    assert_eq!(sum.text(), "6\n", "{}", sum.text());
    let drop = in_session(addr, "alice", "m", "DROP TEMPORARY TABLE t", &[]).await;
    assert_eq!(drop.status, 200, "{}", drop.text());
}

/// Fix round 1, I3: the caps hold however a value is written and wherever it comes
/// from. URL parameters and `SET` (one or several) are read the ClickHouse way by
/// the front; the `SETTINGS` clause, which the front does not parse, meets the
/// worker profile's constraints after the engine's own conversion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setting_caps_hold_on_every_route() {
    let (house, _pool) = house("caps", per_namespace(1)).await;
    let addr = house.local_addr();
    let get_setting = "SELECT getSetting('max_memory_usage')";
    // The profile starts at the cap.
    assert_eq!(
        in_session(addr, "alice", "c", get_setting, &[])
            .await
            .text(),
        "4294967296\n"
    );

    // URL parameters.
    let ok = in_session(
        addr,
        "alice",
        "u",
        get_setting,
        &[("max_memory_usage", "1Gi")],
    )
    .await;
    assert_eq!(ok.text(), "1073741824\n", "{}", ok.text());
    for (name, value, expected) in [
        ("max_memory_usage", "0", "164"),
        ("max_memory_usage", "5G", "164"),
        ("max_memory_usage", "18446744073709551616", "36"),
        ("max_memory_usage", "20000000Ti", "36"),
        ("max_execution_time", "0", "164"),
        ("max_execution_time", "1e-7", "164"),
        ("max_execution_time", "-1", "164"),
        ("max_execution_time", "inf", "36"),
        ("max_threads", "100000", "164"),
    ] {
        let response = in_session(addr, "alice", "u", "SELECT 1", &[(name, value)]).await;
        assert_eq!(
            code(&response),
            Some(expected),
            "{name}={value}: {}",
            response.text()
        );
    }

    // SET, one and several: refused whole, nothing applied.
    let set = in_session(addr, "alice", "s", "SET max_memory_usage = '2Gi'", &[]).await;
    assert_eq!(set.status, 200, "{}", set.text());
    for sql in [
        "SET max_memory_usage = 0",
        "SET max_memory_usage = '18446744073709551616'",
        "SET max_execution_time = 0",
        "SET max_memory_usage = '1Gi', max_execution_time = 0",
        "SET max_threads = 1, max_memory_usage = '20000000Ti'",
    ] {
        let response = in_session(addr, "alice", "s", sql, &[]).await;
        assert!(
            matches!(code(&response), Some("164" | "36")),
            "{sql}: {}",
            response.text()
        );
    }
    assert_eq!(
        in_session(addr, "alice", "s", get_setting, &[])
            .await
            .text(),
        "2147483648\n"
    );

    // The SETTINGS clause: the engine's constraints (452).
    for clause in [
        "max_memory_usage = 0",
        "max_memory_usage = '18446744073709551616'",
        "max_memory_usage = '20000000Ti'",
        "max_memory_usage = '5G'",
        "max_execution_time = 0",
        "max_execution_time = '1e-7'",
        "max_execution_time = -1",
        "max_execution_time = 301",
        "max_threads = 100000",
        "max_memory_usage = 1, max_execution_time = 0",
        "output_format_schema = '/abs/path/x.proto'",
        "input_format_record_errors_file_path = 'errors.log'",
        "format_schema = 'x.proto:M'",
    ] {
        let sql = format!("SELECT 1 SETTINGS {clause}");
        let response = in_session(addr, "alice", "c", &sql, &[]).await;
        assert_eq!(code(&response), Some("452"), "{sql}: {}", response.text());
    }
    let within = in_session(
        addr,
        "alice",
        "c",
        "SELECT getSetting('max_memory_usage') SETTINGS max_memory_usage = '1Gi', max_execution_time = 0.5",
        &[],
    )
    .await;
    assert_eq!(within.text(), "1073741824\n", "{}", within.text());
}

/// Fix round 1, I4: no setting a client may set names a file. Every setting of
/// chDB whose name says path, file or schema is either refused by the front or of
/// a type that cannot hold a path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_allowed_setting_names_a_file() {
    let (house, _pool) = house("paths", per_namespace(1)).await;
    let addr = house.local_addr();
    let listed = as_alice(
        addr,
        "GET",
        "SELECT name, type FROM system.settings WHERE name LIKE '%path%' OR name LIKE '%file%' \
         OR name LIKE '%schema%' ORDER BY name",
        b"",
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.text());
    let text = listed.text();
    let rows: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    assert!(rows.len() > 20, "the scan sees chDB's settings: {text}");
    let scalar = [
        "Bool",
        "UInt64",
        "Int64",
        "UInt32",
        "Float",
        "Double",
        "Seconds",
        "Milliseconds",
        "NonZeroUInt64",
    ];
    let named: Vec<_> = rows
        .iter()
        .filter(|(name, kind)| loams_house::settings::is_allowed(name) && !scalar.contains(kind))
        .collect();
    assert!(
        named.is_empty(),
        "allowed settings that can hold a path: {named:?}"
    );
    for name in [
        "input_format_record_errors_file_path",
        "output_format_schema",
    ] {
        assert!(
            rows.iter().any(|(n, _)| *n == name),
            "{name} is still chDB's"
        );
        assert!(!loams_house::settings::is_allowed(name), "{name}");
    }
}

/// Fix round 1, M1: a `CREATE TEMPORARY TABLE` that fails leaves a session without
/// temporary tables unpinned; one that already had them keeps its pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_first_temporary_table_unpins() {
    let (house, pool) = house("unpin", per_namespace(2)).await;
    let addr = house.local_addr();
    let bad = "CREATE TEMPORARY TABLE t (a UInt8) ENGINE = NoSuchEngine";
    let failed = in_session(addr, "alice", "f", bad, &[]).await;
    assert_ne!(failed.status, 200, "{}", failed.text());
    assert_eq!(
        pool.stats().pinned,
        0,
        "the failed statement's pin is released"
    );
    // The session is still usable, and pins on its first real temporary table.
    let good = "CREATE TEMPORARY TABLE t (a UInt8) ENGINE = Memory";
    let created = in_session(addr, "alice", "f", good, &[]).await;
    assert_eq!(created.status, 200, "{}", created.text());
    assert_eq!(pool.stats().pinned, 1);
    let again = in_session(addr, "alice", "f", bad.replace(" t ", " u ").as_str(), &[]).await;
    assert_ne!(again.status, 200, "{}", again.text());
    assert_eq!(
        pool.stats().pinned,
        1,
        "a session with temporary tables keeps its pin"
    );
    let count = in_session(addr, "alice", "f", "SELECT count() FROM t", &[]).await;
    assert_eq!(count.text(), "0\n", "{}", count.text());
}
