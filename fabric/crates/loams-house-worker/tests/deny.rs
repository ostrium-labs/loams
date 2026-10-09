//! The deny list (L1) and chDB's own controls (L2), HS1 Task 5, against real
//! workers: every denied item is `344` (a denied setting `164`) wherever it stands
//! in a statement, the analysis that finds it opens nothing, the allowed table
//! functions work, the host functions answer the House's values, and with L1
//! bypassed the worker's grants and pinned settings still refuse.

mod common;

use std::collections::HashSet;
use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::Scratch;
use common::http::{Response, request, target};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::deny::{
    ALLOWED_ENGINES, ALLOWED_TABLE_FUNCTIONS, ALLOWED_TEMPORARY_ENGINES, DENIED_ENGINES,
    DENIED_FUNCTIONS, DENIED_TABLE_FUNCTIONS, HOST_FUNCTIONS, Host, INSERT_ONLY_TABLE_FUNCTIONS,
    is_denied_setting,
};
use loams_house::http::{HouseHandle, serve};
use loams_house::{Outcome, PoolConfig, ProcessLauncher, WorkerPool};

/// alice's namespace (`UserMap::dev("alice", …, 2, …)`), as the pool names it.
const NAMESPACE: &str = "2";

/// A House whose namespace has at most `workers` workers: a session's temporary
/// table pins one of them, so requests outside the session need a second.
async fn house_with(test: &str, workers: usize) -> (HouseHandle, WorkerPool) {
    let launcher = ProcessLauncher::new(common::WORKER, common::tmp_root(test));
    let config = PoolConfig {
        max_workers_per_namespace: workers,
        ..common::small(workers)
    };
    let pool = WorkerPool::start(config, Arc::new(launcher))
        .await
        .expect("pool");
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("alice", "a", 2, false)],
        tmp_dir: common::tmp_root(&format!("{test}-spool")),
        ..HouseConfig::default()
    };
    (serve(config, pool.clone()).await.expect("serves"), pool)
}

async fn house(test: &str) -> (HouseHandle, WorkerPool) {
    house_with(test, 2).await
}

async fn call(
    addr: SocketAddr,
    method: &'static str,
    params: Vec<(String, String)>,
    body: Vec<u8>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let pairs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        request(addr, method, &target(&pairs), &[], &body)
    })
    .await
    .expect("client")
}

/// One statement as alice in session `s` (so temporary tables persist), with
/// extra parameters and a body.
async fn send(
    addr: SocketAddr,
    method: &'static str,
    sql: &str,
    extra: &[(&'static str, &str)],
    body: &[u8],
) -> Response {
    let mut params = vec![
        ("user".to_string(), "alice".to_string()),
        ("password".to_string(), "a".to_string()),
        ("query".to_string(), sql.to_string()),
    ];
    if method == "POST" {
        params.push(("session_id".to_string(), "s".to_string()));
    }
    params.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    call(addr, method, params, body.to_vec()).await
}

async fn post(addr: SocketAddr, sql: &str) -> Response {
    send(addr, "POST", sql, &[], b"").await
}

fn code(response: &Response) -> Option<&str> {
    response.header("X-ClickHouse-Exception-Code")
}

/// A loopback listener that counts the connections it gets: a URL the statements
/// name, to prove the analysis itself opened nothing.
struct Trap {
    listener: TcpListener,
}

impl Trap {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener.set_nonblocking(true).expect("nonblocking");
        Self { listener }
    }

    fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/loams-trap",
            self.listener.local_addr().expect("addr").port()
        )
    }

    fn connections(&self) -> usize {
        let mut seen = 0;
        while let Ok((mut stream, _)) = self.listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let _ = stream.read(&mut [0u8; 256]);
            seen += 1;
        }
        seen
    }
}

async fn setup(addr: SocketAddr) {
    for sql in [
        "CREATE TEMPORARY TABLE IF NOT EXISTS t (a String) ENGINE = Memory",
        "CREATE TEMPORARY TABLE IF NOT EXISTS n (a UInt64) ENGINE = Memory",
    ] {
        let response = post(addr, sql).await;
        assert_eq!(response.status, 200, "{sql}: {}", response.text());
    }
}

/// §49 §13.2 L1: each denied item, in every place a statement can hold it, is
/// `344` from the deny list — not the engine's `497` — and nothing it names is
/// opened, not even by the analysis that finds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denied_everywhere_in_the_tree() {
    let (house, _pool) = house("deny-tree").await;
    let addr = house.local_addr();
    setup(addr).await;
    let trap = Trap::new();
    let url = trap.url();
    let sources = [
        format!("url('{url}', CSV, 'a String')"),
        format!("s3('{url}/k', CSV, 'a String')"),
        "file('/etc/hostname', LineAsString, 'a String')".to_string(),
        "remote('127.0.0.1:1', system.one)".to_string(),
        "executable('cat', TSV, 'a String')".to_string(),
        format!("icebergS3('{url}/')"),
        "system.disks".to_string(),
        "merge('system', '^disks$')".to_string(),
        "merge(REGEXP('^sys'), '^disks$')".to_string(),
    ];
    let places = [
        "SELECT * FROM {}",
        "SELECT * FROM (SELECT * FROM {})",
        "SELECT * FROM view(SELECT * FROM {})",
        "WITH c AS (SELECT * FROM {}) SELECT * FROM c",
        "WITH (SELECT count() FROM {}) AS k SELECT k",
        "SELECT * FROM numbers(1) AS l CROSS JOIN {} AS r",
        "SELECT 1 WHERE 1 IN (SELECT 1 FROM {})",
        "SELECT 1 UNION ALL SELECT 1 FROM {}",
        "INSERT INTO n SELECT 1 FROM {}",
        "CREATE TEMPORARY TABLE c AS SELECT * FROM {}",
        "DESCRIBE TABLE {}",
        "EXPLAIN PLAN SELECT * FROM {}",
    ];
    let mut wrong = Vec::new();
    for source in &sources {
        for place in places {
            let sql = place.replace("{}", source);
            let response = post(addr, &sql).await;
            if code(&response) != Some("344") || !response.text().contains("on the House") {
                wrong.push(format!("{sql}: {:?} {}", code(&response), response.text()));
            }
        }
    }
    // Functions in every place an expression can stand.
    let expressions = [
        "file('/etc/hostname')",
        "getMacro('replica')",
        "filesystemAvailable()",
        "`hostName`()",
    ];
    let places = [
        "SELECT {}",
        "SELECT 1 WHERE {} != ''",
        "SELECT * FROM (SELECT {} AS x)",
        "SELECT * FROM view(SELECT {} AS x)",
        "WITH {} AS x SELECT x",
        "SELECT arrayMap(y -> {}, [1])",
        "INSERT INTO t SELECT toString({})",
        "CREATE TEMPORARY TABLE c AS SELECT {} AS x",
        "CREATE TEMPORARY TABLE c (x String DEFAULT toString({})) ENGINE = Memory",
    ];
    for expression in expressions {
        for place in places {
            let sql = place.replace("{}", expression);
            let response = post(addr, &sql).await;
            if code(&response) != Some("344") {
                wrong.push(format!("{sql}: {:?} {}", code(&response), response.text()));
            }
        }
    }
    // GET (read-only) answers the deny list too.
    let get = send(addr, "GET", "SELECT * FROM system.disks", &[], b"").await;
    if code(&get) != Some("344") {
        wrong.push(format!("GET: {:?} {}", code(&get), get.text()));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(trap.connections(), 0, "nothing connected to {url}");
    let none = post(addr, "SELECT count() FROM t").await;
    assert_eq!(none.text(), "0\n", "nothing was inserted");
}

/// The corpus of named payloads (`conformance/clickhouse/corpus/deny`): each
/// file's first line is `-- expect: <code>` or `-- expect: ok`, and lines of
/// `-- param_<name>: <value>` after it are the request's query parameters (fix
/// round 1: `{t:Identifier}` naming a denied table).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deny_corpus() {
    let (house, _pool) = house("deny-corpus").await;
    let addr = house.local_addr();
    setup(addr).await;
    let corpus =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/clickhouse/corpus/deny");
    let mut seen = 0;
    let mut wrong = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(corpus)
        .expect("the corpus")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    paths.sort();
    for path in paths {
        let text = std::fs::read_to_string(&path).expect("read");
        let (head, mut sql) = text.split_once('\n').expect("an expect line");
        let expected = head.strip_prefix("-- expect: ").expect("-- expect:").trim();
        let mut params = vec![("param_u".to_string(), "http://127.0.0.1:1/x".to_string())];
        while let Some((line, rest)) = sql.split_once('\n') {
            let Some((name, value)) = line
                .strip_prefix("-- param_")
                .and_then(|p| p.split_once(": "))
            else {
                break;
            };
            params.push((format!("param_{name}"), value.trim().to_string()));
            sql = rest;
        }
        let mut request = vec![
            ("user".to_string(), "alice".to_string()),
            ("password".to_string(), "a".to_string()),
            ("query".to_string(), sql.trim().to_string()),
            ("session_id".to_string(), "s".to_string()),
        ];
        request.extend(params);
        let response = call(addr, "POST", request, Vec::new()).await;
        let got = match code(&response) {
            None if response.status == 200 => "ok".to_string(),
            Some(code) => code.to_string(),
            None => format!("status {}", response.status),
        };
        if got != expected {
            wrong.push(format!(
                "{}: expected {expected}, got {got}: {}",
                path.file_name().expect("name").to_string_lossy(),
                response.text()
            ));
        }
        seen += 1;
    }
    assert!(seen >= 60, "the corpus is there: {seen}");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every entry of the deny lists has its `344` (HS1 Global Constraints: deny by
/// default, a test per entry).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_denied_entry_is_344() {
    let (house, _pool) = house("deny-entries").await;
    let addr = house.local_addr();
    let mut wrong = Vec::new();
    let mut expect = |sql: String, response: &Response, want: &str| {
        if code(response) != Some(want) {
            wrong.push(format!("{sql}: {:?} {}", code(response), response.text()));
        }
    };
    for name in DENIED_TABLE_FUNCTIONS
        .iter()
        .chain(INSERT_ONLY_TABLE_FUNCTIONS)
    {
        // Two take a form of their own; the rest parse with any arguments.
        let sql = match *name {
            "viewIfPermitted" => {
                "SELECT * FROM viewIfPermitted(SELECT 1 ELSE null('a UInt8'))".to_string()
            }
            "viewExplain" => "SELECT * FROM viewExplain('EXPLAIN', '', (SELECT 1))".to_string(),
            _ => format!("SELECT * FROM {name}('x')"),
        };
        let response = post(addr, &sql).await;
        expect(sql, &response, "344");
    }
    for name in DENIED_FUNCTIONS {
        let sql = format!("SELECT {name}('x')");
        let response = post(addr, &sql).await;
        expect(sql, &response, "344");
    }
    for function in HOST_FUNCTIONS.iter().filter(|h| h.answer == Host::Disabled) {
        let sql = format!("SELECT {}()", function.name);
        let response = post(addr, &sql).await;
        expect(sql, &response, "344");
    }
    for engine in DENIED_ENGINES {
        let sql = format!("CREATE TEMPORARY TABLE e (a String) ENGINE = {engine}('x')");
        let response = post(addr, &sql).await;
        expect(sql, &response, "344");
        if *engine != "S3Queue" {
            // A pipe's S3Queue is the front's own (HS1 Task 18).
            let sql = format!("CREATE TABLE e (a String) ENGINE = {engine}('x')");
            let response = post(addr, &sql).await;
            expect(sql, &response, "344");
        }
    }
    for engine in ["MergeTree", "Log", "Set", "Join('ANY', 'LEFT', a)"] {
        let sql = format!("CREATE TEMPORARY TABLE e (a String) ENGINE = {engine}");
        let response = post(addr, &sql).await;
        expect(sql, &response, "344");
    }
    for sql in [
        "SYSTEM FLUSH LOGS",
        "ATTACH TABLE a (x String) ENGINE = Memory",
        "BACKUP TABLE t TO Disk('default', 'b')",
        "RESTORE TABLE t FROM Disk('default', 'b')",
        "CREATE FUNCTION f AS (x) -> x",
        "CREATE DICTIONARY d (k UInt64) PRIMARY KEY k SOURCE(NULL()) LAYOUT(FLAT()) LIFETIME(0)",
        "CREATE NAMED COLLECTION c AS a = 1",
    ] {
        let response = post(addr, sql).await;
        expect(sql.to_string(), &response, "344");
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `INTO OUTFILE` is `344` and writes nothing (R1.10: chDB writes it whatever the
/// grants say, so the refusal is L1's).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn into_outfile_is_344() {
    let (house, _pool) = house("deny-outfile").await;
    let addr = house.local_addr();
    let dir = Scratch::new("t5-outfile");
    for sql in [
        format!("SELECT 1 INTO OUTFILE '{}'", dir.path("a.tsv")),
        format!(
            "SELECT * FROM view(SELECT 1) INTO OUTFILE '{}' TRUNCATE FORMAT CSV",
            dir.path("b.csv")
        ),
        format!("SELECT 1 into\toutfile '{}'", dir.path("c.tsv")),
    ] {
        for method in ["POST", "GET"] {
            let response = send(addr, method, &sql, &[], b"").await;
            assert_eq!(
                code(&response),
                Some("344"),
                "{method} {sql}: {}",
                response.text()
            );
        }
    }
    assert!(dir.files().is_empty(), "written: {:?}", dir.files());
}

/// `format_schema*` and the other denied settings are `164` from a URL, a `SET`
/// and a query's `SETTINGS` clause; what the front cannot see in a non-query's
/// clause is pinned by the worker (`452`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn format_schema_setting_is_164() {
    let (house, _pool) = house("deny-schema").await;
    let addr = house.local_addr();
    setup(addr).await;
    for (name, value) in [
        ("format_schema", "/etc/hostname:M"),
        ("format_schema_source", "query"),
        ("user_files_path", "/"),
        ("input_format_values_interpret_expressions", "1"),
        ("default_temporary_table_engine", "Log"),
    ] {
        let url = send(addr, "POST", "SELECT 1", &[(name, value)], b"").await;
        assert_eq!(code(&url), Some("164"), "?{name}: {}", url.text());
        let set = post(addr, &format!("SET {name} = '{value}'")).await;
        assert_eq!(code(&set), Some("164"), "SET {name}: {}", set.text());
        if name != "user_files_path" {
            let clause = post(addr, &format!("SELECT 1 SETTINGS {name} = '{value}'")).await;
            assert_eq!(
                code(&clause),
                Some("164"),
                "SETTINGS {name}: {}",
                clause.text()
            );
            let insert = post(
                addr,
                &format!("INSERT INTO t SETTINGS {name} = '{value}' SELECT 'x'"),
            )
            .await;
            assert!(
                matches!(code(&insert), Some("164" | "452")),
                "INSERT … SETTINGS {name}: {:?} {}",
                code(&insert),
                insert.text()
            );
        }
    }
}

/// `CREATE FUNCTION` (a SQL or executable UDF), `executable()` and the
/// `Executable` engine are `344`, and no function is left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_function_executable_is_344() {
    let (house, _pool) = house("deny-function").await;
    let addr = house.local_addr();
    for sql in [
        "CREATE FUNCTION loams_f AS (x) -> x + 1",
        "CREATE OR REPLACE FUNCTION loams_f AS (x) -> x + 1",
        "CREATE FUNCTION IF NOT EXISTS loams_f AS (x) -> x + 1",
        "/* c */ create function loams_f as (x) -> x + 1",
        "SELECT * FROM executable('cat /etc/hostname', TSV, 'a String')",
        "SELECT * FROM executable('cat', TSV, 'a String', (SELECT 1))",
        "CREATE TEMPORARY TABLE e (a String) ENGINE = Executable('cat', TSV)",
        "CREATE TEMPORARY TABLE e (a String) ENGINE = ExecutablePool('cat', TSV)",
    ] {
        let response = post(addr, sql).await;
        assert_eq!(code(&response), Some("344"), "{sql}: {}", response.text());
    }
    let call = post(addr, "SELECT loams_f(1)").await;
    assert_eq!(
        code(&call),
        Some("46"),
        "no function was made: {}",
        call.text()
    );
}

/// `CREATE DICTIONARY` with an HTTP source is `344`, opens nothing, and leaves no
/// dictionary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dictionary_http_source_is_344() {
    let (house, _pool) = house("deny-dictionary").await;
    let addr = house.local_addr();
    let trap = Trap::new();
    let url = trap.url();
    for sql in [
        format!(
            "CREATE DICTIONARY d (k UInt64, v String) PRIMARY KEY k \
             SOURCE(HTTP(url '{url}' format 'TSV')) LAYOUT(FLAT()) LIFETIME(0)"
        ),
        format!(
            "CREATE OR REPLACE DICTIONARY d (k UInt64, v String) PRIMARY KEY k \
             SOURCE(HTTP(url '{url}' format 'TSV')) LAYOUT(FLAT()) LIFETIME(0)"
        ),
        format!(
            "ATTACH DICTIONARY d (k UInt64, v String) PRIMARY KEY k \
             SOURCE(HTTP(url '{url}' format 'TSV')) LAYOUT(FLAT()) LIFETIME(0)"
        ),
        "SELECT * FROM dictionary('d')".to_string(),
    ] {
        let response = post(addr, &sql).await;
        assert_eq!(code(&response), Some("344"), "{sql}: {}", response.text());
    }
    let get = post(addr, "SELECT dictGet('d', 'v', toUInt64(1))").await;
    assert!(
        matches!(code(&get), Some("36" | "60" | "86")),
        "no dictionary d: {:?} {}",
        code(&get),
        get.text()
    );
    assert_eq!(trap.connections(), 0, "nothing connected to {url}");
}

/// §32 §7.8's allowed table functions work, `merge()` over the namespace's own
/// tables (the front's views) included, and `input()` inside an `INSERT`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allowed_table_functions_work() {
    // One worker for the namespace, so the views below are where every request
    // runs; the session (and its pin) comes last.
    let (house, pool) = house_with("deny-allowed", 1).await;
    let addr = house.local_addr();
    // The namespace's own tables: views the front writes on the control
    // connection (as HS1 Task 11's lake reads will).
    let mut lease = pool
        .acquire(NAMESPACE)
        .await
        .expect("the namespace's worker");
    let mut views = common::statement("SELECT 1", "TSV");
    views.views = vec![
        "CREATE OR REPLACE VIEW default.own_1 AS SELECT 1 AS a".to_string(),
        "CREATE OR REPLACE VIEW default.own_2 AS SELECT 2 AS a".to_string(),
    ];
    lease.run(views).await.expect("views");
    pool.release(lease, Outcome::Completed);

    for (sql, expected) in [
        ("SELECT sum(number) FROM numbers(4)", "6\n"),
        ("SELECT sum(number) FROM numbers_mt(4)", "6\n"),
        ("SELECT count() FROM zeros(3)", "3\n"),
        ("SELECT sum(a) FROM values('a UInt8', 1, 2)", "3\n"),
        (
            "SELECT count() FROM (SELECT * FROM generateRandom('a UInt8', 1) LIMIT 2)",
            "2\n",
        ),
        ("SELECT a FROM format(CSV, 'a String', 'x\n')", "x\n"),
        ("SELECT count() FROM null('a UInt8')", "0\n"),
        ("SELECT * FROM view(SELECT 7)", "7\n"),
        ("SELECT sum(a) FROM merge('default', '^own_')", "3\n"),
        (
            "SELECT sum(a) FROM merge(currentDatabase(), '^own_')",
            "3\n",
        ),
        ("SELECT sum(a) FROM merge('^own_')", "3\n"),
        ("SELECT * FROM (VALUES (1, 2))", "1\t2\n"),
        (
            "SELECT sum(generate_series) FROM generate_series(1, 3)",
            "6\n",
        ),
    ] {
        let response = send(addr, "GET", sql, &[], b"").await;
        assert_eq!(response.status, 200, "{sql}: {}", response.text());
        assert_eq!(response.text(), expected, "{sql}");
    }
    setup(addr).await;
    let insert = send(
        addr,
        "POST",
        "INSERT INTO t SELECT a FROM input('a String') FORMAT CSV",
        &[],
        b"x\ny\n",
    )
    .await;
    // The deny list lets `input()` through inside an INSERT. chDB's streaming
    // insert (`chdb_stream_insert`, R1.7) then refuses `INSERT … SELECT … FROM
    // input()` itself: a surface gap for HS1 Tasks 12 and 29, not a refusal of
    // the deny list's.
    assert_ne!(code(&insert), Some("344"), "{}", insert.text());
    let null = post(addr, "INSERT INTO FUNCTION null('a String') SELECT 'x'").await;
    assert_eq!(null.status, 200, "{}", null.text());
    let literal = post(addr, "INSERT INTO t VALUES ('z')").await;
    assert_eq!(literal.status, 200, "{}", literal.text());
    let count = post(addr, "SELECT count() FROM t").await;
    assert_eq!(count.text(), "1\n");
}

/// The host functions answer the House's values under their own column names;
/// the rest of them, and any call the rewrite does not take, are `344`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_functions_return_loams_values() {
    let (house, _pool) = house("deny-host").await;
    let addr = house.local_addr();
    setup(addr).await;
    let response = post(
        addr,
        "SELECT hostName(), FQDN(), fqdn(), displayName(), serverTimezone(), currentUser(), \
         user(), hostName() = 'loams-house' AS same FORMAT TSVWithNames",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(
        response.text(),
        "hostName()\tFQDN()\tfqdn()\tdisplayName()\tserverTimezone()\tcurrentUser()\tuser()\tsame\n\
         loams-house\tloams-house\tloams-house\tloams-house\tUTC\talice\talice\t1\n"
    );
    let insert = post(addr, "INSERT INTO t SELECT hostName()").await;
    assert_eq!(insert.status, 200, "{}", insert.text());
    let read = post(addr, "SELECT a FROM t").await;
    assert_eq!(read.text(), "loams-house\n");
    let authenticated = post(addr, "SELECT authenticatedUser()").await;
    assert_eq!(authenticated.text(), "alice\n", "{}", authenticated.text());
    // The rewrite leaves strings alone.
    let string = post(addr, "SELECT 'hostName()'").await;
    assert_eq!(string.text(), "hostName()\n");
    for sql in [
        "SELECT getMacro('shard')",
        "SELECT filesystemCapacity()",
        "SELECT serverUUID()",
        "SELECT tcpPort()",
        "SELECT getServerSetting('path')",
        "SELECT `hostName`()",
        "SELECT hostName(1)",
        "SELECT uptime()",
        "SELECT logTrace('x')",
    ] {
        let response = post(addr, sql).await;
        assert_eq!(code(&response), Some("344"), "{sql}: {}", response.text());
    }
}

/// The lists follow chDB: every table function it has is allowed or denied by
/// name, and every function the lists name exists, with the case rule recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lists_follow_chdb() {
    let (house, _pool) = house("deny-lists").await;
    let addr = house.local_addr();
    let listed: HashSet<String> = ALLOWED_TABLE_FUNCTIONS
        .iter()
        .chain(INSERT_ONLY_TABLE_FUNCTIONS)
        .chain(DENIED_TABLE_FUNCTIONS)
        .map(|n| n.to_string())
        .collect();
    let functions = post(addr, "SELECT name FROM system.table_functions").await;
    assert_eq!(functions.status, 200, "{}", functions.text());
    let unlisted: Vec<_> = functions
        .text()
        .lines()
        .filter(|name| !listed.contains(*name))
        .map(str::to_string)
        .collect();
    assert!(
        unlisted.is_empty(),
        "table functions on neither list: {unlisted:?}"
    );
    let known = post(addr, "SELECT name, case_insensitive FROM system.functions").await;
    let known: Vec<(String, bool)> = known
        .text()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(n, ci)| (n.to_string(), ci == "1"))
        .collect();
    // A host function chDB has must be matched with its case rule, or the
    // rewrite would miss a spelling chDB takes (the check would still refuse
    // it). Names chDB 26.9 lacks (`catboostEvaluate` is not built in) stay
    // listed for the next version.
    for function in HOST_FUNCTIONS {
        if let Some((_, case_insensitive)) = known.iter().find(|(n, _)| n == function.name) {
            assert_eq!(
                *case_insensitive, function.case_insensitive,
                "{}",
                function.name
            );
        }
    }
    for name in [
        "hostName",
        "FQDN",
        "displayName",
        "serverTimezone",
        "currentUser",
        "file",
    ] {
        assert!(known.iter().any(|(n, _)| n == name), "{name}");
    }
}

/// L2 with L1 bypassed: statements sent straight to a worker (no deny list) are
/// still refused by its grants, its pinned settings and `allow_ddl = 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_controls_hold_without_the_deny_list() {
    let pool = common::pool("deny-l2", common::small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let trap = Trap::new();
    let url = trap.url();
    let session = common::session("l2");
    let mut create =
        common::statement("CREATE TEMPORARY TABLE t (a String) ENGINE = Memory", "TSV");
    create.session = session.clone();
    lease.run(create).await.expect("temporary table");

    // The grants (HS1 R1.8): file(), url(), s3() and remote reads and writes.
    // Measured: the *scalar* `file('/etc/hostname')` reads the file with no FILE
    // grant, as `INTO OUTFILE` writes one (R1.10), so it rests on L1 (`344`,
    // `deny_corpus`) and L3 alone.
    for sql in [
        "SELECT * FROM file('/etc/hostname', LineAsString)".to_string(),
        "SELECT * FROM filesystem('/')".to_string(),
        format!("SELECT * FROM url('{url}', LineAsString)"),
        format!("SELECT * FROM s3('{url}/k', LineAsString)"),
        "SELECT * FROM remote('127.0.0.1:1', system.one)".to_string(),
        "INSERT INTO FUNCTION file('loams-l2.csv', CSV, 'a String') SELECT 'x'".to_string(),
        format!("INSERT INTO FUNCTION url('{url}', CSV, 'a String') SELECT 'x'"),
        format!("CREATE TEMPORARY TABLE u (a String) ENGINE = URL('{url}', CSV)"),
    ] {
        let mut statement = common::statement(&sql, "TSV");
        statement.session = session.clone();
        let err = lease.run(statement).await.expect_err(&sql);
        assert!(
            matches!(err.code(), 497 | 344),
            "{sql}: {} {err}",
            err.code()
        );
    }
    // `executable()` is not a grant: it runs a script from `user_scripts_path`,
    // which the worker points at a directory that never exists (and
    // `into_outfile_create_parent_directories` is pinned off, so nothing makes
    // it). L1 refuses it and L3's seccomp denies `execve`.
    let err = lease
        .run(common::statement(
            "SELECT * FROM executable('cat', TSV, 'a String')",
            "TSV",
        ))
        .await
        .expect_err("executable");
    assert!(err.message().contains("no-user-scripts"), "{err}");

    // `allow_ddl = 0`, sticky; temporary tables still work (above).
    for (sql, codes) in [
        ("CREATE VIEW v AS SELECT 1", &[164, 392][..]),
        ("CREATE DATABASE d2", &[164, 392][..]),
        ("CREATE FUNCTION f AS (x) -> x", &[164, 392, 497][..]),
    ] {
        let err = lease
            .run(common::statement(sql, "TSV"))
            .await
            .expect_err(sql);
        assert!(codes.contains(&err.code()), "{sql}: {err}");
    }
    let ddl = lease
        .run(common::statement("SELECT getSetting('allow_ddl')", "TSV"))
        .await
        .expect("runs");
    assert_eq!(ddl.bytes, b"false\n");
    let mut back = common::statement("SELECT 1", "TSV");
    back.settings = vec![("allow_ddl".to_string(), "1".to_string())];
    assert!(lease.run(back).await.is_err(), "allow_ddl is sticky");

    // Expressions in `INSERT … VALUES` data are not evaluated (no EXPLAIN sees
    // them); literals still insert.
    let mut values = common::statement("INSERT INTO t VALUES (file('/etc/hostname'))", "TSV");
    values.session = session.clone();
    let err = lease
        .run(values)
        .await
        .expect_err("an expression in VALUES");
    assert!(matches!(err.code(), 344 | 497 | 27 | 62), "{err}");
    let mut values = common::statement("INSERT INTO t VALUES (hostName())", "TSV");
    values.session = session.clone();
    assert!(lease.run(values).await.is_err(), "hostName() in VALUES");
    let mut literal = common::statement("INSERT INTO t VALUES ('x')", "TSV");
    literal.session = session.clone();
    lease.run(literal).await.expect("a literal");
    let mut count = common::statement("SELECT count() FROM t", "TSV");
    count.session = session.clone();
    assert_eq!(lease.run(count).await.expect("count").bytes, b"1\n");

    // Every denied setting chDB has is pinned by the profile: a SETTINGS clause
    // that changes it is `452`.
    let listed = lease
        .run(common::statement(
            "SELECT name, value, type FROM system.settings",
            "TSVRaw",
        ))
        .await
        .expect("settings");
    let text = String::from_utf8(listed.bytes).expect("utf-8");
    let mut denied = 0;
    for line in text.lines() {
        let mut fields = line.split('\t');
        let (Some(name), Some(value), Some(kind)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !is_denied_setting(name) {
            continue;
        }
        denied += 1;
        assert!(
            loams_house_worker::settings::pinned().any(|p| p == name),
            "{name} is denied by the front but not pinned by the worker"
        );
        let other = match kind {
            "Bool" => if value == "0" { "1" } else { "0" }.to_string(),
            "DefaultTableEngine" => if value == "Log" { "Memory" } else { "Log" }.to_string(),
            _ => format!("{value}x"),
        };
        let sql = format!(
            "SELECT 1 SETTINGS {name} = '{}'",
            other.replace('\\', "\\\\").replace('\'', "\\'")
        );
        let err = lease
            .run(common::statement(&sql, "TSV"))
            .await
            .expect_err(&sql);
        // chDB refuses `allow_python_table_function` itself first (`392`).
        let expected: &[i32] = if name == "allow_python_table_function" {
            &[452, 392]
        } else {
            &[452]
        };
        assert!(expected.contains(&err.code()), "{sql}: {err}");
    }
    assert!(denied >= 12, "the denied settings are chDB's: {denied}");

    // `displayName()` is the House's even without the rewrite.
    let display = lease
        .run(common::statement("SELECT displayName()", "TSV"))
        .await
        .expect("runs");
    assert_eq!(display.bytes, b"loams-house\n");
    assert_eq!(
        loams_house_worker::settings::DISPLAY_NAME,
        loams_house::config::DISPLAY_NAME
    );
    pool.release(lease, Outcome::Completed);
    assert_eq!(trap.connections(), 0, "nothing connected to {url}");
}

/// The worker's explains are TSV whatever the statement says (fix round 1): a
/// top-level `FORMAT` the front did not strip (one before `SETTINGS`) would make
/// them JSON, CSV or one raw line, which the deny list cannot read. The worker
/// refuses the statement (`62`), and the check refuses any explain that does not
/// open with a statement root (`344`, the unit tests); nothing is opened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explain_output_shape_is_enforced() {
    let (house, _pool) = house("deny-shape").await;
    let addr = house.local_addr();
    setup(addr).await;
    let trap = Trap::new();
    let url = trap.url();
    let mut wrong = Vec::new();
    for format in [
        "JSON",
        "JSONEachRow",
        "CSV",
        "TSVRaw",
        "RawBLOB",
        "Vertical",
        "TSVWithNames",
    ] {
        for sql in [
            format!(
                "SELECT * FROM url('{url}', CSV, 'a String') FORMAT {format} SETTINGS max_threads = 1"
            ),
            format!("SELECT * FROM system.disks FORMAT {format} SETTINGS max_threads = 1"),
            format!("SELECT file('/etc/hostname') FORMAT {format} SETTINGS max_threads = 1"),
            format!("EXPLAIN SELECT * FROM system.disks FORMAT {format} SETTINGS max_threads = 1"),
            format!("SELECT 1 FORMAT {format} SETTINGS max_threads = 1"),
        ] {
            let response = post(addr, &sql).await;
            if code(&response) != Some("62") {
                wrong.push(format!("{sql}: {:?} {}", code(&response), response.text()));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(trap.connections(), 0, "nothing connected to {url}");
    // FORMAT last, a trailing comment and an INSERT's own FORMAT still work.
    for (sql, expected) in [
        ("SELECT 1 SETTINGS max_threads = 1 FORMAT CSV", "1\n"),
        ("SELECT number FROM numbers(2) -- c", "0\n1\n"),
        ("SELECT 2 # c", "2\n"),
    ] {
        let response = post(addr, sql).await;
        assert_eq!(response.status, 200, "{sql}: {}", response.text());
        assert_eq!(response.text(), expected, "{sql}");
    }
    let insert = send(addr, "POST", "INSERT INTO t FORMAT CSV", &[], b"x\n").await;
    assert_eq!(insert.status, 200, "{}", insert.text());
    let values = post(addr, "INSERT INTO t VALUES ('y')").await;
    assert_eq!(values.status, 200, "{}", values.text());
    assert_eq!(post(addr, "SELECT count() FROM t").await.text(), "2\n");
}

/// `SHOW` forms are reads of `system` tables no explain shows (fix round 1):
/// the forms the system allow-list allows work, the rest are `344`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn show_forms_follow_the_system_allow_list() {
    let (house, _pool) = house("deny-show").await;
    let addr = house.local_addr();
    setup(addr).await;
    let mut wrong = Vec::new();
    for sql in [
        "SHOW TABLES",
        "SHOW FULL TABLES",
        "SHOW TEMPORARY TABLES",
        "SHOW DATABASES",
        "SHOW SETTINGS LIKE 'max_threads'",
        "SHOW CHANGED SETTINGS LIKE 'max_threads'",
        "SHOW SETTING max_threads",
        "SHOW ENGINES",
        "SHOW FUNCTIONS LIKE 'plus'",
        "SHOW PROCESSLIST",
        "SHOW COLUMNS FROM system.one",
        "SHOW INDEXES FROM system.one",
        "SHOW CREATE TEMPORARY TABLE t",
        "SHOW CREATE DATABASE default",
    ] {
        let response = post(addr, sql).await;
        if response.status != 200 {
            wrong.push(format!("{sql}: {:?} {}", code(&response), response.text()));
        }
    }
    // Allowed, and answered by the engine: chDB keeps no `CREATE` of a system
    // table (`60`).
    let one = post(addr, "SHOW CREATE TABLE system.one").await;
    if code(&one) == Some("344") {
        wrong.push(format!("SHOW CREATE TABLE system.one: {}", one.text()));
    }
    for sql in [
        "SHOW CLUSTERS",
        "SHOW CLUSTER 'default'",
        "SHOW ACCESS",
        "SHOW GRANTS",
        "SHOW USERS",
        "SHOW ROLES",
        "SHOW PROFILES",
        "SHOW SETTINGS PROFILES",
        "SHOW QUOTAS",
        "SHOW QUOTA",
        "SHOW POLICIES",
        "SHOW ROW POLICIES",
        "SHOW PRIVILEGES",
        "SHOW CURRENT ROLES",
        "SHOW ENABLED ROLES",
        "SHOW CREATE USER default",
        "SHOW CREATE ROLE r",
        "SHOW CREATE QUOTA",
        "SHOW FILESYSTEM CACHES",
        "SHOW MERGES",
        "SHOW DICTIONARIES",
        "SHOW CREATE DICTIONARY d",
        "SHOW CREATE TABLE system.disks",
        "show create table system.server_settings",
    ] {
        let response = post(addr, sql).await;
        if code(&response) != Some("344") {
            wrong.push(format!("{sql}: {:?} {}", code(&response), response.text()));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every engine chDB has is denied or reviewed as allowed (fix round 1), and
/// every allowed one exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_engine_lists_follow_chdb() {
    let (house, _pool) = house("deny-engines").await;
    let addr = house.local_addr();
    let engines = post(addr, "SELECT name FROM system.table_engines").await;
    assert_eq!(engines.status, 200, "{}", engines.text());
    let engines: HashSet<String> = engines.text().lines().map(str::to_string).collect();
    let unlisted: Vec<_> = engines
        .iter()
        .filter(|e| !DENIED_ENGINES.contains(&e.as_str()) && !ALLOWED_ENGINES.contains(&e.as_str()))
        .collect();
    assert!(unlisted.is_empty(), "engines on neither list: {unlisted:?}");
    for engine in ALLOWED_ENGINES {
        assert!(engines.contains(*engine), "{engine} is not chDB's");
    }
    for engine in ALLOWED_TEMPORARY_ENGINES {
        assert!(ALLOWED_ENGINES.contains(engine), "{engine}");
    }
}

/// A denied table function called as a function, in a scalar or in an `IN`, is
/// `344` too (fix round 1): `file('/etc/hostname')` reads the host as a scalar.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denied_table_functions_as_scalars_and_in_in() {
    let (house, _pool) = house("deny-scalars").await;
    let addr = house.local_addr();
    let mut wrong = Vec::new();
    for name in DENIED_TABLE_FUNCTIONS {
        let mut forms = vec![
            format!("SELECT {name}('x')"),
            format!("SELECT 1 IN {name}('x')"),
        ];
        if !matches!(*name, "viewIfPermitted" | "viewExplain") {
            // Those two parse in a FROM only with their own forms
            // (`every_denied_entry_is_344`).
            forms.push(format!("SELECT 1 WHERE 1 IN (SELECT * FROM {name}('x'))"));
        }
        for sql in forms {
            let response = post(addr, &sql).await;
            if code(&response) != Some("344") {
                wrong.push(format!("{sql}: {:?} {}", code(&response), response.text()));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `format(Values, …)` parses its inline data as `INSERT … VALUES` does, which no
/// explain sees: an expression there is refused by L2 (R5.6), and a literal
/// still reads (fix round 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn format_values_expressions_are_refused() {
    let (house, _pool) = house("deny-format-values").await;
    let addr = house.local_addr();
    for data in [
        "(file('/etc/hostname'))",
        "(hostName())",
        "((SELECT 'x'))",
        "(concat('a', 'b'))",
    ] {
        let sql = format!("SELECT * FROM format(Values, 'a String', $${data}$$)");
        let response = post(addr, &sql).await;
        assert_ne!(response.status, 200, "{sql}: {}", response.text());
    }
    let literal = post(addr, "SELECT * FROM format(Values, 'a String', $$('x')$$)").await;
    assert_eq!(literal.status, 200, "{}", literal.text());
    assert_eq!(literal.text(), "x\n");
}

/// Every setting that names a path, a file, a URL, a directory, a schema, an
/// endpoint or a host, and every `allow_*` switch, is denied by the front and
/// pinned by the worker, or reviewed (fix round 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_like_settings_are_denied() {
    let pool = common::pool("deny-path-settings", common::small(1)).await;
    let mut lease = pool.acquire("ns").await.expect("worker");
    let listed = lease
        .run(common::statement(
            "SELECT name FROM system.settings \
             WHERE match(name, 'path|file|url|dir|schema|endpoint|host') \
             OR startsWith(name, 'allow_')",
            "TSVRaw",
        ))
        .await
        .expect("settings");
    pool.release(lease, Outcome::Completed);
    let names: Vec<String> = String::from_utf8(listed.bytes)
        .expect("utf-8")
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        names.len() > 100,
        "the scan found chDB's settings: {}",
        names.len()
    );
    let pinned: HashSet<&str> = loams_house_worker::settings::pinned().collect();
    let mut wrong = Vec::new();
    for name in &names {
        let denied = is_denied_setting(name);
        if denied && !pinned.contains(name.as_str()) {
            wrong.push(format!(
                "{name}: denied by the front, not pinned by the worker"
            ));
        }
        if !denied && !REVIEWED_SETTINGS.contains(&name.as_str()) {
            wrong.push(format!("{name}: neither denied and pinned, nor reviewed"));
        }
    }
    for name in REVIEWED_SETTINGS {
        if !names.iter().any(|n| n == name) {
            wrong.push(format!("{name}: reviewed, but chDB has no such setting"));
        }
    }
    for name in [
        "allow_get_client_http_header",
        "format_display_secrets_in_show_and_select",
    ] {
        if !(is_denied_setting(name) && pinned.contains(name)) {
            wrong.push(format!("{name}: must be denied and pinned"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The settings `path_like_settings_are_denied` scans that are neither denied
/// nor pinned, reviewed (HS1 Task 5 fix round 1). A setting chDB adds that
/// matches the scan fails the test until it is denied and pinned or reviewed
/// here.
const REVIEWED_SETTINGS: &[&str] = &[
    // What these switch on or tune is refused already: an engine, table
    // function or dictionary source on the deny lists (object stores, lakes,
    // Kafka, Keeper, `url()`, `file()`), a `system` table's data, or DDL, which
    // user connections do not have (`allow_ddl = 0`, which is sticky, R5.6).
    "allow_archive_path_syntax",
    "allow_asynchronous_read_from_io_pool_for_merge_tree",
    "allow_calculating_subcolumns_sizes_for_merge_tree_reading",
    "allow_changing_replica_until_first_data_packet",
    "allow_create_index_without_type",
    "allow_database_glue_catalog",
    "allow_database_iceberg",
    "allow_database_unity_catalog",
    "allow_ddl",
    "allow_delta_kernel_rs",
    "allow_delta_lake_create_table",
    "allow_delta_lake_writes",
    "allow_deprecated_database_ordinary",
    "allow_deprecated_syntax_for_merge_tree",
    "allow_drop_detached",
    "allow_experimental_alias_table_engine",
    "allow_experimental_alter_materialized_view_structure",
    "allow_experimental_annoy_index",
    "allow_experimental_codecs",
    "allow_experimental_database_atomic",
    "allow_experimental_database_glue_catalog",
    "allow_experimental_database_hms_catalog",
    "allow_experimental_database_iceberg",
    "allow_experimental_database_materialized_mysql",
    "allow_experimental_database_materialized_postgresql",
    "allow_experimental_database_paimon_rest_catalog",
    "allow_experimental_database_replicated",
    "allow_experimental_database_unity_catalog",
    "allow_experimental_delta_kernel_rs",
    "allow_experimental_delta_lake_writes",
    "allow_experimental_full_text_index",
    "allow_experimental_geo_types_in_iceberg",
    "allow_experimental_inverted_index",
    "allow_experimental_kafka_offsets_storage_in_keeper",
    "allow_experimental_lightweight_delete",
    "allow_experimental_lightweight_update",
    "allow_experimental_live_view",
    "allow_experimental_materialized_postgresql_table",
    "allow_experimental_object_storage_queue_hive_partitioning",
    "allow_experimental_paimon_storage_engine",
    "allow_experimental_parallel_reading_from_replicas",
    "allow_experimental_refreshable_materialized_view",
    "allow_experimental_s3queue",
    "allow_experimental_shared_merge_tree",
    "allow_experimental_statistic",
    "allow_experimental_statistics",
    "allow_experimental_text_index_lazy_apply",
    "allow_experimental_time_series_table",
    "allow_experimental_undrop_table_query",
    "allow_experimental_unique_key",
    "allow_experimental_url_wildcard_from_index_pages",
    "allow_experimental_usearch_index",
    "allow_experimental_vector_similarity_index",
    "allow_experimental_window_view",
    "allow_experimental_ytsaurus_dictionary_source",
    "allow_experimental_ytsaurus_table_engine",
    "allow_experimental_ytsaurus_table_function",
    "allow_geo_types_in_iceberg",
    "allow_kafka_offsets_storage_in_keeper",
    "allow_materialized_view_with_bad_select",
    "allow_metadata_only_named_tuple_alter",
    "allow_minmax_index_for_json",
    "allow_non_metadata_alters",
    "allow_nondeterministic_mutations",
    "allow_push_predicate_ast_for_distributed_subqueries",
    "allow_replace_partition_from_empty_source",
    "allow_statistic_optimize",
    "allow_statistics",
    "allow_statistics_optimize",
    "allow_suspicious_codecs",
    "allow_url_wildcard_from_index_pages",
    "azure_create_new_file_on_insert",
    "azure_ignore_file_doesnt_exist",
    "azure_max_inflight_parts_for_one_file",
    "azure_max_redirects",
    "azure_skip_empty_files",
    "azure_throw_on_zero_files_match",
    "backup_restore_failure_after_host_disconnected_for_seconds",
    "compatibility_s3_presigned_url_query_in_path",
    "delta_lake_insert_max_bytes_in_data_file",
    "delta_lake_insert_max_rows_in_data_file",
    "delta_lake_reload_schema_for_consistency",
    "distributed_cache_file_cache_name",
    "distributed_directory_monitor_batch_inserts",
    "distributed_directory_monitor_max_sleep_time_ms",
    "distributed_directory_monitor_sleep_time_ms",
    "distributed_directory_monitor_split_batch_on_failure",
    "enable_url_encoding",
    "engine_file_allow_create_multiple_files",
    "engine_file_empty_if_not_exists",
    "engine_file_skip_empty_files",
    "engine_file_truncate_on_insert",
    "engine_url_skip_empty_files",
    "file_like_engine_default_partition_strategy",
    "hdfs_create_new_file_on_insert",
    "hdfs_ignore_file_doesnt_exist",
    "hdfs_skip_empty_files",
    "hdfs_throw_on_zero_files_match",
    "http_allow_database_as_path",
    "http_allow_filters_as_path",
    "http_allow_filters_as_unrecognized_url_parameters",
    "http_allow_table_as_file",
    "http_skip_not_found_url_for_globs",
    "iceberg_compaction_max_bytes_in_data_file",
    "iceberg_compaction_max_rows_in_data_file",
    "iceberg_data_file_size_lower_threshold_compaction",
    "iceberg_data_file_size_upper_threshold_compaction",
    "iceberg_engine_ignore_schema_evolution",
    "iceberg_file_entries_queue_size",
    "iceberg_insert_max_bytes_in_data_file",
    "iceberg_insert_max_rows_in_data_file",
    "iceberg_max_number_datafiles_to_compact",
    "iceberg_orphan_files_older_than_seconds",
    "max_http_get_redirects",
    "max_streams_for_files_processing_in_cluster_functions",
    "merge_tree_clear_old_temporary_directories_interval_seconds",
    "merge_tree_min_bytes_for_concurrent_read_for_remote_filesystem",
    "merge_tree_min_rows_for_concurrent_read_for_remote_filesystem",
    "prefer_localhost_replica",
    "query_plan_direct_read_from_text_index",
    "s3_create_new_file_on_insert",
    "s3_ignore_file_doesnt_exist",
    "s3_max_inflight_parts_for_one_file",
    "s3_max_redirects",
    "s3_path_filter_limit",
    "s3_skip_empty_files",
    "s3_throw_on_zero_files_match",
    "schema_inference_use_cache_for_azure",
    "schema_inference_use_cache_for_file",
    "schema_inference_use_cache_for_hdfs",
    "schema_inference_use_cache_for_s3",
    "schema_inference_use_cache_for_url",
    "stream_like_engine_allow_direct_select",
    "traverse_shadow_remote_data_paths",
    "url_wildcard_max_directories_to_read",
    "use_iceberg_metadata_files_cache",
    "use_paimon_metadata_files_cache",
    "write_full_path_in_iceberg_metadata",
    // Read, join, type and format behaviour, caches and profilers that keep
    // their data in the process: none names a path, a host or a credential.
    "allow_aggregate_partitions_independently",
    "allow_correlated_subqueries",
    "allow_creating_set_partitions_independently",
    "allow_deprecated_error_prone_window_functions",
    "allow_deprecated_snowflake_conversion_functions",
    "allow_distinct_partitions_independently",
    "allow_dynamic_type_in_join_keys",
    "allow_execute_multiif_columnar",
    "allow_experimental_analyzer",
    "allow_experimental_bfloat16_type",
    "allow_experimental_bigint_types",
    "allow_experimental_correlated_subqueries",
    "allow_experimental_dynamic_type",
    "allow_experimental_funnel_functions",
    "allow_experimental_geo_types",
    "allow_experimental_hash_functions",
    "allow_experimental_join_condition",
    "allow_experimental_join_right_table_sorting",
    "allow_experimental_json_lazy_type_hints",
    "allow_experimental_json_type",
    "allow_experimental_map_type",
    "allow_experimental_nlp_functions",
    "allow_experimental_nullable_tuple_type",
    "allow_experimental_object_type",
    "allow_experimental_projection_optimization",
    "allow_experimental_qbit_type",
    "allow_experimental_query_cache",
    "allow_experimental_query_deduplication",
    "allow_experimental_shared_set_join",
    "allow_experimental_time_series_aggregate_functions",
    "allow_experimental_time_time64_type",
    "allow_experimental_ts_to_grid_aggregate_function",
    "allow_experimental_variant_type",
    "allow_experimental_window_functions",
    "allow_general_join_planning",
    "allow_hyperscan",
    "allow_join_right_table_sorting",
    "allow_key_condition_coalesce_rewrite",
    "allow_limit_by_partitions_independently",
    "allow_lossy_numeric_supertype",
    "allow_nonconst_timezone_arguments",
    "allow_nondeterministic_optimize_skip_unused_shards",
    "allow_not_comparable_types_in_comparison_functions",
    "allow_not_comparable_types_in_order_by",
    "allow_nullable_tuple_in_extracted_subcolumns",
    "allow_prefetched_read_pool_for_local_filesystem",
    "allow_prefetched_read_pool_for_remote_filesystem",
    "allow_preliminary_distinct_abandoning",
    "allow_push_predicate_when_subquery_contains_with",
    "allow_rank_dense_rank_arguments",
    "allow_reorder_prewhere_conditions",
    "allow_settings_after_format_in_insert",
    "allow_simdjson",
    "allow_special_bool_values_inside_variant",
    "allow_special_serialization_kinds_in_output_formats",
    "allow_suspicious_fixed_string_types",
    "allow_suspicious_indices",
    "allow_suspicious_low_cardinality_types",
    "allow_suspicious_primary_key",
    "allow_suspicious_ttl_expressions",
    "allow_suspicious_types_in_group_by",
    "allow_suspicious_types_in_order_by",
    "allow_suspicious_variant_types",
    "allow_window_partitions_independently",
    "column_names_for_schema_inference",
    "enable_filesystem_cache",
    "enable_filesystem_cache_log",
    "enable_filesystem_cache_on_write_operations",
    "enable_filesystem_read_prefetches_log",
    "filesystem_cache_allow_background_download",
    "filesystem_cache_boundary_alignment",
    "filesystem_cache_enable_background_download_during_fetch",
    "filesystem_cache_enable_background_download_for_metadata_files_in_packed_storage",
    "filesystem_cache_max_download_size",
    "filesystem_cache_name",
    "filesystem_cache_prefer_bigger_buffer_size",
    "filesystem_cache_reserve_space_wait_lock_timeout_milliseconds",
    "filesystem_cache_segments_batch_size",
    "filesystem_cache_skip_download_if_exceeds_per_query_cache_write_limit",
    "filesystem_cache_verbose_logging",
    "filesystem_cache_wait_for_concurrent_download_timeout_milliseconds",
    "filesystem_prefetch_max_memory_usage",
    "filesystem_prefetch_min_bytes_for_single_read_task",
    "filesystem_prefetch_step_bytes",
    "filesystem_prefetch_step_marks",
    "filesystem_prefetches_limit",
    "format_avro_schema_registry_connection_timeout",
    "format_avro_schema_registry_max_retries",
    "format_avro_schema_registry_receive_timeout",
    "format_avro_schema_registry_retry_initial_backoff_ms",
    "format_avro_schema_registry_send_timeout",
    "format_capn_proto_use_autogenerated_schema",
    "format_protobuf_use_autogenerated_schema",
    "input_format_arrow_skip_columns_with_unsupported_types_in_schema_inference",
    "input_format_bson_skip_fields_with_unsupported_types_in_schema_inference",
    "input_format_capn_proto_skip_fields_with_unsupported_types_in_schema_inference",
    "input_format_csv_use_best_effort_in_schema_inference",
    "input_format_json_use_string_type_for_ambiguous_paths_in_named_tuples_inference_from_objects",
    "input_format_max_bytes_to_read_for_schema_inference",
    "input_format_max_rows_to_read_for_schema_inference",
    "input_format_orc_skip_columns_with_unsupported_types_in_schema_inference",
    "input_format_parquet_local_file_min_bytes_for_seek",
    "input_format_parquet_skip_columns_with_unsupported_types_in_schema_inference",
    "input_format_protobuf_skip_fields_with_unsupported_types_in_schema_inference",
    "input_format_tsv_use_best_effort_in_schema_inference",
    "jemalloc_profile_text_collapsed_use_count",
    "jemalloc_profile_text_output_format",
    "jemalloc_profile_text_symbolize_with_inline",
    "join_on_disk_max_files_to_merge",
    "local_filesystem_read_method",
    "local_filesystem_read_prefetch",
    "log_processors_profiles",
    "log_profile_events",
    "memory_profiler_sample_max_allocation_size",
    "memory_profiler_sample_min_allocation_size",
    "memory_profiler_sample_probability",
    "memory_profiler_step",
    "merge_table_max_tables_to_look_for_schema_inference",
    "min_bytes_to_use_direct_io",
    "optimize_count_from_files",
    "output_format_avro_rows_in_file",
    "query_plan_optimize_lazy_materialization_for_file",
    "query_profiler_cpu_time_period_ns",
    "query_profiler_real_time_period_ns",
    "read_from_filesystem_cache_if_exists_otherwise_bypass_cache",
    "remote_filesystem_read_method",
    "remote_filesystem_read_prefetch",
    "schema_inference_cache_require_modification_time_for_url",
    "schema_inference_hints",
    "schema_inference_make_columns_nullable",
    "schema_inference_make_json_columns_nullable",
    "schema_inference_mode",
    "send_profile_events",
    "storage_file_read_method",
    "temporary_files_buffer_size",
    "temporary_files_codec",
    "trace_profile_events",
    "trace_profile_events_list",
    "type_json_skip_duplicated_paths",
    "type_json_skip_invalid_typed_paths",
    "type_json_skip_null_typed_paths",
    "type_json_use_partial_match_to_skip_paths_by_regexp",
    "use_cache_for_count_from_files",
    "use_page_cache_for_disks_without_file_cache",
];
