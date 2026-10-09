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
    ALLOWED_TABLE_FUNCTIONS, DENIED_ENGINES, DENIED_FUNCTIONS, DENIED_TABLE_FUNCTIONS,
    HOST_FUNCTIONS, Host, INSERT_ONLY_TABLE_FUNCTIONS, is_denied_setting,
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
        ("user", "alice".to_string()),
        ("password", "a".to_string()),
        ("query", sql.to_string()),
    ];
    if method == "POST" {
        params.push(("session_id", "s".to_string()));
    }
    params.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
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
/// file's first line is `-- expect: <code>` or `-- expect: ok`.
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
        let (head, sql) = text.split_once('\n').expect("an expect line");
        let expected = head.strip_prefix("-- expect: ").expect("-- expect:").trim();
        let response = send(
            addr,
            "POST",
            sql.trim(),
            &[("param_u", "http://127.0.0.1:1/x")],
            b"",
        )
        .await;
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
        assert_eq!(err.code(), 452, "{sql}: {err}");
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
