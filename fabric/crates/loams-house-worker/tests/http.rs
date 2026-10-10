//! The ClickHouse HTTP interface (HS1 Task 3, FL2 Task 2's contract) on the worker
//! pool: every request here goes over a real socket to `loams_house::http::serve`,
//! and every statement runs in a real `loams-house-worker` process.

mod common;

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::http::{Response, request, target};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve};
use loams_house::{ProcessLauncher, WorkerPool};

const VERSION: &str = "26.9.2.1";

/// Basic `alice:secret` and `alice:wrong`, encoded by hand.
const BASIC_ALICE: &str = "Basic YWxpY2U6c2VjcmV0";
const BASIC_ALICE_WRONG: &str = "Basic YWxpY2U6d3Jvbmc=";

fn users() -> Vec<UserMap> {
    vec![
        UserMap::dev("default", "", 1, false),
        UserMap::dev("alice", "secret", 2, false),
        UserMap::dev("reader", "books", 2, true),
    ]
}

async fn house(test: &str, adjust: impl FnOnce(&mut HouseConfig)) -> HouseHandle {
    house_with_pool(test, common::small(3), adjust).await.0
}

/// A House and the pool behind it, for tests that watch the pool.
async fn house_with_pool(
    test: &str,
    pool_config: loams_house::PoolConfig,
    adjust: impl FnOnce(&mut HouseConfig),
) -> (HouseHandle, WorkerPool) {
    let launcher = ProcessLauncher::new(common::WORKER, common::tmp_root(test));
    let pool = WorkerPool::start(pool_config, Arc::new(launcher))
        .await
        .expect("pool");
    let mut config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: users(),
        tmp_dir: common::tmp_root(&format!("{test}-spool")),
        ..HouseConfig::default()
    };
    adjust(&mut config);
    (serve(config, pool.clone()).await.expect("serves"), pool)
}

/// Runs the blocking client off the runtime the server is on.
async fn call(
    addr: SocketAddr,
    method: &'static str,
    target: String,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        request(addr, method, &target, &headers, &body)
    })
    .await
    .expect("client")
}

async fn get(addr: SocketAddr, params: &[(&str, &str)]) -> Response {
    call(addr, "GET", target(params), Vec::new(), Vec::new()).await
}

async fn post(addr: SocketAddr, params: &[(&str, &str)], body: &[u8]) -> Response {
    call(addr, "POST", target(params), Vec::new(), body.to_vec()).await
}

fn summary(response: &Response) -> serde_json::Value {
    serde_json::from_str(response.header("X-ClickHouse-Summary").expect("summary"))
        .expect("the summary is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_and_root_answer_ok() {
    let house = house("ping", |_| {}).await;
    let addr = house.local_addr();
    for path in ["/ping", "/"] {
        let response = call(addr, "GET", path.to_string(), Vec::new(), Vec::new()).await;
        assert_eq!(response.status, 200, "{path}");
        assert_eq!(response.text(), "Ok.\n", "{path}");
    }
    let missing = call(addr, "GET", "/nope".to_string(), Vec::new(), Vec::new()).await;
    assert_eq!(missing.status, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_param_and_body_forms() {
    let house = house("forms", |_| {}).await;
    let addr = house.local_addr();

    let by_param = get(addr, &[("query", "SELECT 1")]).await;
    assert_eq!((by_param.status, by_param.text().as_str()), (200, "1\n"));
    assert_eq!(by_param.header("X-ClickHouse-Format"), Some("TabSeparated"));

    let by_body = post(addr, &[], b"SELECT 2").await;
    assert_eq!((by_body.status, by_body.text().as_str()), (200, "2\n"));

    let empty_body = post(addr, &[("query", "SELECT 3")], b"").await;
    assert_eq!(empty_body.text(), "3\n");

    // A trailing FORMAT clause chooses the format and names it in the header.
    let json = get(addr, &[("query", "SELECT 1 AS x FORMAT JSONEachRow")]).await;
    assert_eq!(json.text(), "{\"x\":1}\n");
    assert_eq!(json.header("X-ClickHouse-Format"), Some("JSONEachRow"));
    let csv = get(addr, &[("query", "SELECT 1"), ("default_format", "CSV")]).await;
    assert_eq!(csv.header("X-ClickHouse-Format"), Some("CSV"));
    assert_eq!(csv.text(), "1\n");

    // Query parameters.
    let param = get(
        addr,
        &[("query", "SELECT {n:UInt8} + 1"), ("param_n", "41")],
    )
    .await;
    assert_eq!(param.text(), "42\n");

    // Every response names the server, the query and the timezone.
    assert_eq!(
        by_param.header("X-ClickHouse-Server-Display-Name"),
        Some("loams-house")
    );
    assert_eq!(by_param.header("X-ClickHouse-Timezone"), Some("UTC"));
    let tz = get(addr, &[("query", "SELECT timezone()")]).await;
    assert_eq!(tz.text(), "UTC\n", "the worker's server timezone is UTC");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_with_query_param_and_data_body() {
    let house = house("insert", |_| {}).await;
    let addr = house.local_addr();

    let response = post(
        addr,
        &[("query", "INSERT INTO FUNCTION null('n UInt64') FORMAT TSV")],
        b"1\n2\n3\n",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(summary(&response)["written_rows"], "3");

    // The statement and its data in one body, as clickhouse-client sends it.
    let response = post(
        addr,
        &[],
        b"INSERT INTO FUNCTION null('n UInt64') FORMAT TSV\n4\n5\n",
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(summary(&response)["written_rows"], "2");

    // A body in a format the INSERT does not name is the engine's error.
    let bad = post(
        addr,
        &[("query", "INSERT INTO FUNCTION null('n UInt64') FORMAT TSV")],
        b"not a number\n",
    )
    .await;
    assert_ne!(bad.status, 200);
    assert!(bad.header("X-ClickHouse-Exception-Code").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_is_readonly_164() {
    let house = house("readonly", |_| {}).await;
    let addr = house.local_addr();
    for sql in [
        "INSERT INTO FUNCTION null('n UInt64') VALUES (1)",
        "CREATE TEMPORARY TABLE t (n UInt64)",
        "SET max_threads = 1",
        "DROP TABLE x",
    ] {
        let response = get(addr, &[("query", sql)]).await;
        assert_eq!(response.status, 500, "{sql}: {}", response.text());
        assert_eq!(
            response.header("X-ClickHouse-Exception-Code"),
            Some("164"),
            "{sql}"
        );
        assert!(
            response
                .text()
                .starts_with("Code: 164. DB::Exception: Cannot execute query in readonly mode"),
            "{}",
            response.text()
        );
    }
    for sql in [
        "SELECT 1",
        "  /* c */ WITH 1 AS x SELECT x",
        "SHOW DATABASES",
        "DESCRIBE TABLE system.one",
        "EXISTS TABLE system.one",
        "EXPLAIN SELECT 1",
    ] {
        let response = get(addr, &[("query", sql)]).await;
        assert_eq!(response.status, 200, "{sql}: {}", response.text());
    }

    // A read-only user is read-only over POST too.
    let response = call(
        addr,
        "POST",
        target(&[("query", "INSERT INTO FUNCTION null('n UInt64') FORMAT TSV")]),
        vec![
            ("X-ClickHouse-User", "reader".into()),
            ("X-ClickHouse-Key", "books".into()),
        ],
        b"1\n".to_vec(),
    )
    .await;
    assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("164"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_header_basic_and_params() {
    let house = house("auth", |_| {}).await;
    let addr = house.local_addr();
    let select = target(&[("query", "SELECT 1")]);

    let header = call(
        addr,
        "GET",
        select.clone(),
        vec![
            ("X-ClickHouse-User", "alice".into()),
            ("X-ClickHouse-Key", "secret".into()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(header.status, 200, "{}", header.text());

    let basic = call(
        addr,
        "GET",
        select.clone(),
        vec![("Authorization", BASIC_ALICE.into())],
        Vec::new(),
    )
    .await;
    assert_eq!(basic.status, 200, "{}", basic.text());

    let params = get(
        addr,
        &[
            ("query", "SELECT 1"),
            ("user", "alice"),
            ("password", "secret"),
        ],
    )
    .await;
    assert_eq!(params.status, 200, "{}", params.text());

    // A mix of sources is refused, as ClickHouse refuses it (Task 3 review M8;
    // FL2 Task 2 had the first source win).
    let wrong_params = target(&[
        ("query", "SELECT 1"),
        ("user", "alice"),
        ("password", "secret"),
    ]);
    for headers in [
        vec![
            ("X-ClickHouse-User", "alice".to_string()),
            ("X-ClickHouse-Key", "secret".to_string()),
        ],
        vec![("Authorization", BASIC_ALICE.to_string())],
    ] {
        let mixed = call(addr, "GET", wrong_params.clone(), headers, Vec::new()).await;
        assert_eq!(
            mixed.header("X-ClickHouse-Exception-Code"),
            Some("516"),
            "{}",
            mixed.text()
        );
        assert!(
            mixed.text().contains("Invalid authentication"),
            "{}",
            mixed.text()
        );
    }
    let headers_and_basic = call(
        addr,
        "GET",
        select,
        vec![
            ("X-ClickHouse-User", "alice".into()),
            ("X-ClickHouse-Key", "secret".into()),
            ("Authorization", BASIC_ALICE.into()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(
        headers_and_basic.header("X-ClickHouse-Exception-Code"),
        Some("516")
    );

    // No credentials at all is the `default` user.
    assert_eq!(get(addr, &[("query", "SELECT 1")]).await.status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_password_is_516() {
    let house = house("bad-password", |_| {}).await;
    let addr = house.local_addr();
    for (user, password) in [("alice", "wrong"), ("mallory", "secret"), ("alice", "")] {
        let response = get(
            addr,
            &[
                ("query", "SELECT 1"),
                ("user", user),
                ("password", password),
            ],
        )
        .await;
        assert_eq!(response.status, 403, "{user}");
        assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("516"));
        assert_eq!(
            response.text(),
            format!(
                "Code: 516. DB::Exception: {user}: Authentication failed: password is incorrect, \
                 or there is no user with such name. (AUTHENTICATION_FAILED) (version {VERSION})\n"
            )
        );
    }
    let basic = call(
        addr,
        "GET",
        target(&[("query", "SELECT 1")]),
        vec![("Authorization", BASIC_ALICE_WRONG.into())],
        Vec::new(),
    )
    .await;
    assert_eq!(basic.header("X-ClickHouse-Exception-Code"), Some("516"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settings_from_params() {
    let house = house("settings", |_| {}).await;
    let addr = house.local_addr();
    let response = get(
        addr,
        &[
            (
                "query",
                "SELECT getSetting('max_threads'), getSetting('max_block_size')",
            ),
            ("max_threads", "3"),
            ("max_block_size", "777"),
        ],
    )
    .await;
    assert_eq!(response.text(), "3\t777\n");

    // Not a setting: the parameters FL2 lists are not passed on.
    let response = get(
        addr,
        &[
            ("query", "SELECT 1"),
            ("query_id", "q-1"),
            ("buffer_size", "100"),
            ("wait_end_of_query", "0"),
        ],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());

    let unknown = get(
        addr,
        &[("query", "SELECT 1"), ("no_such_setting_loams", "1")],
    )
    .await;
    assert_eq!(
        unknown.header("X-ClickHouse-Exception-Code"),
        Some("115"),
        "{}",
        unknown.text()
    );
    assert_eq!(unknown.status, 404);

    let session_tz = get(
        addr,
        &[("query", "SELECT 1"), ("session_timezone", "Asia/Tokyo")],
    )
    .await;
    assert_eq!(
        session_tz.header("X-ClickHouse-Timezone"),
        Some("Asia/Tokyo")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_id_echoed_or_generated() {
    let house = house("query-id", |_| {}).await;
    let addr = house.local_addr();
    let echoed = get(addr, &[("query", "SELECT 1"), ("query_id", "my-query-7")]).await;
    assert_eq!(echoed.header("X-ClickHouse-Query-Id"), Some("my-query-7"));

    let generated = get(addr, &[("query", "SELECT 1")]).await;
    let id = generated.header("X-ClickHouse-Query-Id").expect("query id");
    assert_eq!(id.len(), 36, "{id}");
    assert_eq!(id.as_bytes()[14], b'4', "a UUID v4: {id}");

    let failed = get(
        addr,
        &[("query", "SELECT * FROM nope_loams"), ("query_id", "q-err")],
    )
    .await;
    assert_eq!(
        failed.header("X-ClickHouse-Query-Id"),
        Some("q-err"),
        "errors carry it too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn summary_header_is_json() {
    let house = house("summary", |_| {}).await;
    let addr = house.local_addr();
    let response = get(addr, &[("query", "SELECT count() FROM numbers(1000)")]).await;
    assert_eq!(response.text(), "1000\n");
    let summary = summary(&response);
    for key in [
        "read_rows",
        "read_bytes",
        "written_rows",
        "written_bytes",
        "total_rows_to_read",
        "result_rows",
        "result_bytes",
        "elapsed_ns",
    ] {
        assert!(
            summary[key].is_string(),
            "{key} is a string, as ClickHouse writes it: {summary}"
        );
    }
    assert_eq!(summary["result_rows"], "1");
    assert_eq!(summary["read_rows"], "1000");
    assert_eq!(summary["written_rows"], "0");
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("gzip");
    encoder.finish().expect("gzip")
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("deflate");
    encoder.finish().expect("deflate")
}

fn decode(encoding: &str, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    match encoding {
        "gzip" => flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map(|_| ()),
        "deflate" => flate2::read::ZlibDecoder::new(bytes)
            .read_to_end(&mut out)
            .map(|_| ()),
        "zstd" => zstd::stream::read::Decoder::new(bytes)
            .and_then(|mut d| d.read_to_end(&mut out))
            .map(|_| ()),
        other => panic!("{other}"),
    }
    .unwrap_or_else(|err| panic!("{encoding}: {err}"));
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gzip_and_zstd_both_ways() {
    let house = house("compression", |_| {}).await;
    let addr = house.local_addr();
    let sql = b"SELECT number FROM numbers(5000)";
    let expected: String = (0..5000).map(|n| format!("{n}\n")).collect();

    for encoding in ["gzip", "deflate", "zstd"] {
        let body = match encoding {
            "gzip" => gzip(sql),
            "deflate" => deflate(sql),
            _ => zstd::encode_all(&sql[..], 3).expect("zstd"),
        };
        let response = call(
            addr,
            "POST",
            target(&[("enable_http_compression", "1")]),
            vec![
                ("Content-Encoding", encoding.into()),
                ("Accept-Encoding", encoding.into()),
            ],
            body,
        )
        .await;
        assert_eq!(response.status, 200, "{encoding}: {}", response.text());
        assert_eq!(response.header("Content-Encoding"), Some(encoding));
        assert_eq!(
            String::from_utf8(decode(encoding, &response.body)).expect("utf-8"),
            expected,
            "{encoding}"
        );
    }

    // Without enable_http_compression the answer is plain, whatever is accepted.
    let plain = call(
        addr,
        "POST",
        "/".to_string(),
        vec![("Accept-Encoding", "gzip".into())],
        sql.to_vec(),
    )
    .await;
    assert_eq!(plain.header("Content-Encoding"), None);
    assert_eq!(plain.text(), expected);

    // An error answered to a compressing client is compressed too.
    let error = call(
        addr,
        "GET",
        target(&[
            ("query", "SELECT * FROM nope_loams"),
            ("enable_http_compression", "1"),
        ]),
        vec![("Accept-Encoding", "gzip".into())],
        Vec::new(),
    )
    .await;
    assert_eq!(error.header("X-ClickHouse-Exception-Code"), Some("60"));
    let text = String::from_utf8(decode("gzip", &error.body)).expect("utf-8");
    assert!(text.starts_with("Code: 60. DB::Exception:"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_loopback_is_refused() {
    let launcher = ProcessLauncher::new(common::WORKER, common::tmp_root("non-loopback"));
    let pool = WorkerPool::start(common::small(1), Arc::new(launcher))
        .await
        .expect("pool");
    for addr in ["0.0.0.0:0", "[::]:0", "192.0.2.1:8123"] {
        let config = HouseConfig {
            listen: addr.parse().expect("addr"),
            ..HouseConfig::default()
        };
        let err = serve(config, pool.clone()).await.expect_err(addr);
        assert!(
            err.message().contains(&format!(
                "house listen on {addr}: only loopback addresses are served until the unified \
                 auth plan (D111)"
            )),
            "{err}"
        );
    }
    let ok = serve(
        HouseConfig {
            listen: "[::1]:0".parse().expect("addr"),
            ..HouseConfig::default()
        },
        pool,
    )
    .await;
    assert!(ok.is_ok(), "IPv6 loopback is loopback");
}

/// R3.8 (Task 3 review, decision 1): hyper writes a head in one piece, so with
/// `send_progress_in_http_headers = 1` the head carries exactly one
/// `X-ClickHouse-Progress` line — the counters when it went out — instead of
/// ClickHouse's stream of lines. Deterministic: no timing is asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_headers_stream() {
    let house = house("progress", |_| {}).await;
    let addr = house.local_addr();
    // A result over `buffer_size`: the head goes out while the query runs.
    let early = get(
        addr,
        &[
            (
                "query",
                "SELECT number, sleepEachRow(0.01) FROM numbers(50) SETTINGS max_block_size = 1",
            ),
            ("send_progress_in_http_headers", "1"),
            ("buffer_size", "1"),
        ],
    )
    .await;
    // A small result: the head goes out at the end.
    let late = get(
        addr,
        &[
            ("query", "SELECT count() FROM numbers(1000)"),
            ("send_progress_in_http_headers", "1"),
        ],
    )
    .await;
    for (response, rows) in [(&early, 50), (&late, 1)] {
        assert_eq!(response.status, 200, "{}", response.text());
        assert_eq!(response.text().lines().count(), rows);
        let progress = response.all("X-ClickHouse-Progress");
        assert_eq!(progress.len(), 1, "exactly one progress line: {progress:?}");
        let json: serde_json::Value =
            serde_json::from_str(progress[0].0).expect("progress is JSON");
        for key in ["read_rows", "read_bytes", "result_rows", "elapsed_ns"] {
            assert!(json[key].is_string(), "{key}: {json}");
        }
        assert!(response.header("X-ClickHouse-Summary").is_some());
    }
    let late_progress: serde_json::Value =
        serde_json::from_str(late.all("X-ClickHouse-Progress")[0].0).expect("JSON");
    assert_eq!(
        late_progress["read_rows"], "1000",
        "the head at the end carries the final counters"
    );

    // Without the parameter, no progress header.
    let quiet = get(addr, &[("query", "SELECT 1")]).await;
    assert!(quiet.all("X-ClickHouse-Progress").is_empty());
}

/// A statement that writes rows and then fails, in small blocks.
const FAILS_LATE: &str = "SELECT number FROM numbers(200000) WHERE throwIf(number = 150000) = 0 SETTINGS max_block_size = 1000";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wait_end_of_query_buffers() {
    let house = house("wait-end", |_| {}).await;
    let addr = house.local_addr();

    // Buffered to the end, the failure is a proper error response with no rows.
    let buffered = get(
        addr,
        &[
            ("query", FAILS_LATE),
            ("buffer_size", "1000"),
            ("wait_end_of_query", "1"),
        ],
    )
    .await;
    assert_eq!(buffered.header("X-ClickHouse-Exception-Code"), Some("395"));
    assert_eq!(buffered.status, 500);
    assert!(
        buffered.text().starts_with("Code: 395. DB::Exception:"),
        "{}",
        &buffered.text()[..80.min(buffered.text().len())]
    );
    assert!(buffered.complete);

    // A success is sent whole, with the final summary.
    let whole = get(
        addr,
        &[
            ("query", "SELECT number FROM numbers(200000)"),
            ("buffer_size", "1000"),
            ("wait_end_of_query", "1"),
        ],
    )
    .await;
    assert_eq!(whole.status, 200);
    assert!(whole.complete);
    assert_eq!(whole.text().lines().count(), 200_000);
    assert_eq!(summary(&whole)["result_rows"], "200000");
    assert!(
        whole.header("Content-Length").is_some(),
        "the length is known"
    );

    // Over the spool's cap: refused, not truncated.
    let small = house_small_spool().await;
    let over = get(
        small.local_addr(),
        &[
            ("query", "SELECT number FROM numbers(200000)"),
            ("wait_end_of_query", "1"),
        ],
    )
    .await;
    assert_eq!(
        over.header("X-ClickHouse-Exception-Code"),
        Some("36"),
        "{}",
        over.text()
    );
    assert!(over.text().contains("wait_end_of_query"), "{}", over.text());
}

async fn house_small_spool() -> HouseHandle {
    house("wait-end-small", |config| {
        config.wait_end_of_query_max_bytes = 10_000
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mid_stream_error_matches_reference() {
    let house = house("mid-stream", |_| {}).await;
    let addr = house.local_addr();
    let response = get(addr, &[("query", FAILS_LATE), ("buffer_size", "1000")]).await;

    // The status went out with the first rows and cannot be taken back.
    assert_eq!(response.status, 200);
    assert_eq!(response.header("X-ClickHouse-Exception-Code"), None);
    assert!(
        !response.complete,
        "closed without the terminating chunk (Ruling 9)"
    );

    // The body is the rows written, then the exception text: exactly what
    // `MidStreamBody` (FL2 Task 3) builds from the same rows and the same error.
    let text = response.text();
    let at = text
        .find("Code: 395. DB::Exception:")
        .expect("the exception text");
    let (rows, tail) = text.split_at(at);
    assert!(rows.starts_with("0\n1\n2\n"));
    assert!(rows.ends_with('\n'));
    assert!(
        tail.ends_with(&format!(
            "(FUNCTION_THROW_IF_VALUE_IS_NON_ZERO) (version {VERSION})\n"
        )),
        "{tail}"
    );
    let message = tail
        .strip_prefix("Code: 395. DB::Exception: ")
        .and_then(|t| {
            t.strip_suffix(&format!(
                ". (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO) (version {VERSION})\n"
            ))
        })
        .expect("ClickHouse's shape");
    let mut reference = loams_house::MidStreamBody::new();
    reference.write(rows.as_bytes()).expect("rows");
    reference.fail(
        loams_house::HouseError::from(loams_house_ipc::EngineError {
            code: 395,
            name: "FUNCTION_THROW_IF_VALUE_IS_NON_ZERO".to_string(),
            message: message.to_string(),
        }),
        VERSION,
    );
    assert_eq!(reference.body(), response.body.as_slice());
    assert!(reference.must_close());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn errors_before_the_first_byte_carry_their_status() {
    let house = house("early-error", |_| {}).await;
    let addr = house.local_addr();
    let response = get(addr, &[("query", "SELECT * FROM nope_loams")]).await;
    assert_eq!(response.status, 404);
    assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("60"));
    assert!(response.complete);
    assert!(
        response
            .text()
            .ends_with(&format!("(UNKNOWN_TABLE) (version {VERSION})\n"))
    );

    // A database other than `default` does not exist yet (HS1 Task 10).
    let response = get(addr, &[("query", "SELECT 1"), ("database", "nope")]).await;
    assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("81"));

    // ClickHouse's own compressed framing is not served yet.
    let response = get(addr, &[("query", "SELECT 1"), ("compress", "1")]).await;
    assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("48"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keep_alive_serves_several_requests() {
    let house = house("keep-alive", |_| {}).await;
    let addr = house.local_addr();
    let answers = tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("timeout");
        let mut out = Vec::new();
        // One reader for the whole connection: a second one would lose what the
        // first had buffered.
        let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
        for n in 1..=3 {
            let body = format!("SELECT {n}");
            let head = format!(
                "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(head.as_bytes()).expect("write");
            out.push(read_one(&mut reader));
        }
        out
    })
    .await
    .expect("client");
    assert_eq!(answers, vec!["1\n", "2\n", "3\n"]);
}

/// Reads one keep-alive response's body as text.
fn read_one(reader: &mut impl std::io::BufRead) -> String {
    let mut chunked = false;
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("line");
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.trim().eq_ignore_ascii_case("chunked");
            }
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).expect("size");
            let size = usize::from_str_radix(size.trim(), 16).expect("hex");
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).expect("chunk");
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else if let Some(length) = length {
        body.resize(length, 0);
        reader.read_exact(&mut body).expect("body");
    }
    String::from_utf8(body).expect("utf-8")
}

async fn raw(addr: SocketAddr, bytes: Vec<u8>) -> String {
    tokio::task::spawn_blocking(move || common::http::raw(addr, &bytes, Duration::from_secs(10)))
        .await
        .expect("client")
}

/// Task 3 review I8: broken framing is hyper's `400` and a closed connection, and
/// no worker is asked; framing that could smuggle a request closes the connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_framing_is_400_and_touches_no_worker() {
    let (house, pool) = house_with_pool("framing", common::small(2), |_| {}).await;
    let addr = house.local_addr();
    // Content-Length with chunked (R3.9): hyper reads it as chunked (RFC 9112 §6.3)
    // and closes the connection after it, so a request smuggled behind the body is
    // never served.
    let smuggle = "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 13\r\nTransfer-Encoding: chunked\r\n\r\n8\r\nSELECT 1\r\n0\r\n\r\nGET /?query=SELECT%202 HTTP/1.1\r\nHost: x\r\n\r\n";
    let answer = raw(addr, smuggle.as_bytes().to_vec()).await;
    assert_eq!(
        answer.matches("HTTP/1.1 ").count(),
        1,
        "one response only: {answer:?}"
    );
    assert!(
        answer.to_ascii_lowercase().contains("connection: close"),
        "{answer:?}"
    );
    assert!(
        !answer.contains("\r\n\r\n2\n"),
        "the smuggled request did not run"
    );
    let before = pool.stats().acquired_total;
    for (what, request) in [
        (
            "two Content-Lengths",
            "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 8\r\nContent-Length: 9\r\n\r\nSELECT 1",
        ),
        (
            "a signed Content-Length",
            "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: +8\r\n\r\nSELECT 1",
        ),
    ] {
        let answer = raw(addr, request.as_bytes().to_vec()).await;
        assert!(answer.starts_with("HTTP/1.1 400"), "{what}: {answer:?}");
        assert_eq!(
            answer.matches("HTTP/1.1 ").count(),
            1,
            "{what}: closed after the 400"
        );
    }
    // A chunk that does not end where its size says: the read fails, nothing runs.
    let answer = raw(
        addr,
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n8\r\nSELECT 1XX\r\n0\r\n\r\n".to_vec(),
    )
    .await;
    assert!(!answer.starts_with("HTTP/1.1 200"), "{answer:?}");
    assert_eq!(pool.stats().acquired_total, before, "no worker was asked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn head_has_no_body() {
    let house = house("head", |_| {}).await;
    let addr = house.local_addr();
    for target in ["/ping".to_string(), target(&[("query", "SELECT 1")])] {
        let response = call(addr, "HEAD", target.clone(), Vec::new(), Vec::new()).await;
        assert_eq!(response.status, 200, "{target}");
        assert!(response.body.is_empty(), "{target}: {:?}", response.text());
    }
    // HEAD reads, like GET.
    let write = call(
        addr,
        "HEAD",
        target(&[("query", "DROP TABLE x")]),
        Vec::new(),
        Vec::new(),
    )
    .await;
    assert_eq!(write.header("X-ClickHouse-Exception-Code"), Some("164"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_1_0_is_never_chunked() {
    let house = house("http10", |_| {}).await;
    let addr = house.local_addr();
    for (query, rows) in [
        ("SELECT number FROM numbers(100000)", 100_000),
        ("SELECT 1", 1),
    ] {
        let request = format!(
            "GET {} HTTP/1.0\r\n\r\n",
            target(&[("query", query), ("buffer_size", "1000")])
        );
        let answer = raw(addr, request.into_bytes()).await;
        let (head, body) = answer.split_once("\r\n\r\n").expect("head and body");
        assert!(
            head.starts_with("HTTP/1.0 200") || head.starts_with("HTTP/1.1 200"),
            "{head}"
        );
        assert!(
            !head.to_ascii_lowercase().contains("transfer-encoding"),
            "{head}"
        );
        assert_eq!(body.lines().count(), rows, "{query}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipelined_requests_answer_in_order() {
    let house = house("pipeline", |_| {}).await;
    let addr = house.local_addr();
    let mut request = String::new();
    for n in 1..=3 {
        request.push_str(&format!(
            "GET {} HTTP/1.1\r\nHost: x\r\n\r\n",
            target(&[("query", &format!("SELECT {n}"))])
        ));
    }
    request.push_str("GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let answer = raw(addr, request.into_bytes()).await;
    assert_eq!(answer.matches("HTTP/1.1 200").count(), 4, "{answer}");
    let one = answer.find("\r\n\r\n1\n").expect("1");
    let two = answer.find("\r\n\r\n2\n").expect("2");
    let three = answer.find("\r\n\r\n3\n").expect("3");
    assert!(one < two && two < three, "in order");
}

/// Review M4: authentication answers before any `100 Continue`, so a refused client
/// never sends its body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expect_continue_comes_after_authentication() {
    let house = house("expect", |_| {}).await;
    let addr = house.local_addr();
    let result = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let mut out = Vec::new();
        for password in ["wrong", "secret"] {
            let mut stream = std::net::TcpStream::connect(addr).expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
            let body = b"SELECT 42";
            let head = format!(
                "POST {} HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                target(&[("user", "alice"), ("password", password)]),
                body.len()
            );
            stream.write_all(head.as_bytes()).expect("head");
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut first = String::new();
            reader.read_line(&mut first).expect("first line");
            if first.starts_with("HTTP/1.1 100") {
                let mut blank = String::new();
                reader.read_line(&mut blank).expect("blank");
                stream.write_all(body).expect("body");
            }
            let mut rest = String::new();
            let _ = reader.read_to_string(&mut rest);
            out.push((first, rest));
        }
        out
    })
    .await
    .expect("client");
    assert!(
        result[0].0.starts_with("HTTP/1.1 403"),
        "no 100 before a 516: {:?}",
        result[0]
    );
    assert!(
        result[0].1.contains("X-ClickHouse-Exception-Code: 516")
            || result[0]
                .1
                .to_ascii_lowercase()
                .contains("x-clickhouse-exception-code: 516")
    );
    assert!(result[1].0.starts_with("HTTP/1.1 100"), "{:?}", result[1]);
    assert!(result[1].1.starts_with("HTTP/1.1 200"), "{:?}", result[1]);
    assert!(result[1].1.ends_with("42\n"), "{:?}", result[1]);
}

/// The error path once the head is out, with a progress line in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn error_after_the_head_with_progress() {
    let house = house("error-after-head", |_| {}).await;
    let addr = house.local_addr();
    let response = get(
        addr,
        &[
            ("query", FAILS_LATE),
            ("buffer_size", "1000"),
            ("send_progress_in_http_headers", "1"),
        ],
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.all("X-ClickHouse-Progress").len(), 1);
    assert!(!response.complete, "no terminating chunk");
    assert!(response.text().ends_with(&format!(
        "(FUNCTION_THROW_IF_VALUE_IS_NON_ZERO) (version {VERSION})\n"
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_param_with_a_continuation_body() {
    let house = house("continuation", |_| {}).await;
    let addr = house.local_addr();
    let response = post(addr, &[("query", "SELECT")], b"1 + 1").await;
    assert_eq!(
        (response.status, response.text().as_str()),
        (200, "2\n"),
        "{}",
        response.text()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_failures_never_touch_the_pool() {
    let (house, pool) = house_with_pool("no-pool", common::small(2), |_| {}).await;
    let addr = house.local_addr();
    let before = pool.stats();
    for params in [
        vec![
            ("query", "SELECT 1"),
            ("user", "alice"),
            ("password", "wrong"),
        ],
        vec![("query", "SELECT 1"), ("user", "nobody")],
    ] {
        let response = get(addr, &params).await;
        assert_eq!(response.header("X-ClickHouse-Exception-Code"), Some("516"));
    }
    let mixed = call(
        addr,
        "GET",
        target(&[("query", "SELECT 1"), ("user", "alice")]),
        vec![("Authorization", BASIC_ALICE.into())],
        Vec::new(),
    )
    .await;
    assert_eq!(mixed.header("X-ClickHouse-Exception-Code"), Some("516"));
    let after = pool.stats();
    assert_eq!(after.acquired_total, before.acquired_total, "{after:?}");
    assert_eq!(after.leased, 0);
}

/// Review I1: a client that stops sending does not hold a worker. Before its data
/// starts it never gets one; once it has one, the idle timeout ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_clients_do_not_pin_a_worker() {
    let (house, pool) = house_with_pool("slow", common::small(2), |config| {
        config.receive_timeout = Duration::from_millis(500);
    })
    .await;
    let addr = house.local_addr();
    let insert = target(&[("query", "INSERT INTO FUNCTION null('n UInt64') FORMAT TSV")]);

    // No body at all.
    let before = pool.stats().acquired_total;
    let head_only = format!("POST {insert} HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n");
    let answer = raw(addr, head_only.into_bytes()).await;
    assert!(!answer.starts_with("HTTP/1.1 200"), "{answer:?}");
    assert_eq!(
        pool.stats().acquired_total,
        before,
        "no worker before the data starts"
    );

    // Some body, then nothing.
    let partial = format!("POST {insert} HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n1\n2\n");
    let cancels = pool.stats().kills_for(loams_house::ExitReason::Cancel);
    let answer = raw(addr, partial.into_bytes()).await;
    assert!(!answer.starts_with("HTTP/1.1 200"), "{answer:?}");
    assert!(
        common::eventually(Duration::from_secs(5), || {
            let stats = pool.stats();
            stats.leased == 0 && stats.kills_for(loams_house::ExitReason::Cancel) == cancels + 1
        })
        .await,
        "the worker was let go: {:?}",
        pool.stats()
    );
}

/// Review I5: a truncated compressed `INSERT` body is an error and commits nothing —
/// the worker is killed rather than told the input ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn truncated_compressed_inserts_commit_nothing() {
    let (house, pool) = house_with_pool("truncated", common::small(2), |_| {}).await;
    let addr = house.local_addr();
    let rows: String = (0..20_000).map(|n| format!("{n}\n")).collect();
    for encoding in ["gzip", "deflate", "zstd"] {
        let whole = match encoding {
            "gzip" => gzip(rows.as_bytes()),
            "deflate" => deflate(rows.as_bytes()),
            _ => zstd::encode_all(rows.as_bytes(), 3).expect("zstd"),
        };
        let cut = whole[..whole.len() - 4].to_vec();
        let cancels = pool.stats().kills_for(loams_house::ExitReason::Cancel);
        let response = call(
            addr,
            "POST",
            target(&[("query", "INSERT INTO FUNCTION null('n UInt64') FORMAT TSV")]),
            vec![("Content-Encoding", encoding.into())],
            cut,
        )
        .await;
        assert_eq!(
            response.header("X-ClickHouse-Exception-Code"),
            Some("36"),
            "{encoding}: {}",
            response.text()
        );
        assert!(
            response.text().contains("decompress"),
            "{encoding}: {}",
            response.text()
        );
        assert_eq!(
            pool.stats().kills_for(loams_house::ExitReason::Cancel),
            cancels + 1,
            "{encoding}: the INSERT was abandoned, not ended"
        );
    }
}

/// Task 3 review, decision 4, over HTTP: sessions are per user, URL settings do not
/// stay, and `close_session` ends one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_are_per_user_over_http() {
    // Several workers per namespace: the temporary table pins its session to one
    // (HS1 Task 4), so the session needs no single-worker namespace any more.
    let pool_config = loams_house::PoolConfig {
        max_workers_per_namespace: 3,
        ..common::small(3)
    };
    let (house, _pool) = house_with_pool("http-sessions", pool_config, |config| {
        config.users.push(UserMap::dev("carol", "c", 2, false));
    })
    .await;
    let addr = house.local_addr();
    let as_user =
        |user: &'static str, password: &'static str, extra: &[(&'static str, &'static str)]| {
            let mut params = vec![("user", user), ("password", password), ("session_id", "s1")];
            params.extend_from_slice(extra);
            params
        };
    let created = post(
        addr,
        &as_user(
            "alice",
            "secret",
            &[(
                "query",
                "CREATE TEMPORARY TABLE t (n UInt8) ENGINE = Memory",
            )],
        ),
        b"",
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.text());
    let inserted = post(
        addr,
        &as_user("alice", "secret", &[("query", "INSERT INTO t FORMAT TSV")]),
        b"1\n2\n",
    )
    .await;
    assert_eq!(inserted.status, 200, "{}", inserted.text());
    assert_eq!(
        get(
            addr,
            &as_user("alice", "secret", &[("query", "SELECT count() FROM t")])
        )
        .await
        .text(),
        "2\n"
    );
    let carol = get(
        addr,
        &as_user("carol", "c", &[("query", "SELECT count() FROM t")]),
    )
    .await;
    assert_eq!(
        carol.header("X-ClickHouse-Exception-Code"),
        Some("60"),
        "carol's s1 is not alice's"
    );

    let with = get(
        addr,
        &as_user(
            "alice",
            "secret",
            &[
                ("query", "SELECT getSetting('max_threads')"),
                ("max_threads", "3"),
            ],
        ),
    )
    .await;
    assert_eq!(with.text(), "3\n");
    let without = get(
        addr,
        &as_user(
            "alice",
            "secret",
            &[("query", "SELECT getSetting('max_threads')")],
        ),
    )
    .await;
    assert_ne!(without.text(), "3\n", "URL settings are per query");

    let closing = get(
        addr,
        &as_user(
            "alice",
            "secret",
            &[("query", "SELECT 1"), ("close_session", "1")],
        ),
    )
    .await;
    assert_eq!(closing.status, 200);
    let gone = get(
        addr,
        &as_user("alice", "secret", &[("query", "SELECT count() FROM t")]),
    )
    .await;
    assert_eq!(gone.header("X-ClickHouse-Exception-Code"), Some("60"));

    let too_long = get(
        addr,
        &as_user(
            "alice",
            "secret",
            &[("query", "SELECT 1"), ("session_timeout", "3601")],
        ),
    )
    .await;
    assert_eq!(too_long.header("X-ClickHouse-Exception-Code"), Some("36"));
}

/// Review M5 and R3.8: too many header fields is `431`; a URI past hyper's
/// 65 534 bytes is `414` (use POST for longer statements).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn header_and_uri_limits() {
    let house = house("limits", |config| config.max_headers = 20).await;
    let addr = house.local_addr();
    let mut many = String::from("GET /ping HTTP/1.1\r\nHost: x\r\n");
    for n in 0..30 {
        many.push_str(&format!("X-Extra-{n}: y\r\n"));
    }
    many.push_str("\r\n");
    let answer = raw(addr, many.into_bytes()).await;
    assert!(answer.starts_with("HTTP/1.1 431"), "{answer:?}");

    let long_ok = format!("SELECT length('{}')", "x".repeat(60_000));
    assert_eq!(get(addr, &[("query", &long_ok)]).await.text(), "60000\n");
    let too_long = format!(
        "GET /?query={} HTTP/1.1\r\nHost: x\r\n\r\n",
        "x".repeat(70_000)
    );
    let answer = raw(addr, too_long.into_bytes()).await;
    assert!(
        answer.starts_with("HTTP/1.1 414"),
        "{}",
        &answer[..answer.len().min(80)]
    );
}

/// Review I7: parameters ClickHouse clients send that are not settings are not
/// forwarded (each would be `115 UNKNOWN_SETTING` if it were).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserved_parameters_are_not_settings() {
    let house = house("reserved", |_| {}).await;
    let addr = house.local_addr();
    let response = get(
        addr,
        &[
            ("query", "SELECT 1"),
            ("quota_key", "q"),
            ("role", "r"),
            ("stacktrace", "1"),
            ("client_protocol_version", "54460"),
            ("close_session", "0"),
        ],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
}

/// Review M6: query parameters are percent-decoded byte for byte, and a broken
/// escape is refused rather than guessed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn percent_decoding_is_strict() {
    let house = house("percent", |_| {}).await;
    let addr = house.local_addr();
    let exact = call(
        addr,
        "GET",
        "/?query=SELECT%20%7Bp%3AString%7D&param_p=%C3%A9%2B%20x".to_string(),
        Vec::new(),
        Vec::new(),
    )
    .await;
    assert_eq!(exact.text(), "é+ x\n");
    let broken = call(
        addr,
        "GET",
        "/?query=SELECT%201&param_p=%+5".to_string(),
        Vec::new(),
        Vec::new(),
    )
    .await;
    assert_eq!(
        broken.header("X-ClickHouse-Exception-Code"),
        Some("36"),
        "{}",
        broken.text()
    );
    assert_eq!(broken.status, 400);
}

/// Review M11: `Keep-Alive` names the configured timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keep_alive_header_uses_the_config() {
    let house = house("keep-alive-header", |config| {
        config.keep_alive = Duration::from_secs(3)
    })
    .await;
    let addr = house.local_addr();
    // `request` sends `Connection: close`, so ask on a kept connection.
    let answer = raw(addr, format!("GET {} HTTP/1.1\r\nHost: x\r\n\r\nGET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", target(&[("query", "SELECT 1")])).into_bytes()).await;
    assert!(
        answer
            .to_ascii_lowercase()
            .contains("keep-alive: timeout=3"),
        "{answer}"
    );
}

/// Fix round 2, N1: `receive_timeout` bounds the *client's* silence while it sends
/// a request body — not the time a statement takes. Every way a slow statement is
/// answered (held, streamed, spooled) and a slow `INSERT` finish outlive it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_statements_outlive_the_receive_timeout() {
    let house = house("slow-statements", |config| {
        config.receive_timeout = Duration::from_millis(500);
    })
    .await;
    let addr = house.local_addr();
    let slow = "SELECT number, sleepEachRow(0.2) FROM numbers(5) SETTINGS max_block_size = 1";
    for (mode, extra) in [
        ("held", vec![]),
        ("streamed", vec![("buffer_size", "1")]),
        ("spooled", vec![("wait_end_of_query", "1")]),
    ] {
        let mut params = vec![("query", slow)];
        params.extend(extra);
        let response = get(addr, &params).await;
        assert_eq!(response.status, 200, "{mode}: {}", response.text());
        assert!(response.complete, "{mode}");
        assert_eq!(response.text().lines().count(), 5, "{mode}");
    }
}

/// Fix round 2, N1: the body arrives at once, then the `INSERT` takes about a
/// second to finish (a `DEFAULT` that sleeps per row) — twice `receive_timeout`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_insert_finish_outlives_the_receive_timeout() {
    let pool_config = loams_house::PoolConfig {
        max_workers_per_namespace: 1,
        ..common::small(2)
    };
    let (house, _pool) = house_with_pool("slow-insert", pool_config, |config| {
        config.receive_timeout = Duration::from_millis(500);
    })
    .await;
    let addr = house.local_addr();
    let session = |query: &'static str| vec![("session_id", "slow"), ("query", query)];
    let created = post(
        addr,
        &session("CREATE TEMPORARY TABLE t (n UInt64, s UInt8 DEFAULT sleepEachRow(0.2)) ENGINE = Memory"),
        b"",
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.text());
    let started = std::time::Instant::now();
    let insert = post(
        addr,
        &session("INSERT INTO t (n) FORMAT TSV"),
        b"1\n2\n3\n4\n5\n",
    )
    .await;
    assert_eq!(insert.status, 200, "{}", insert.text());
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "it was slow: {:?}",
        started.elapsed()
    );
    assert_eq!(summary(&insert)["written_rows"], "5");
}

/// Fix round 2, N4: an idle keep-alive connection is closed at the `Keep-Alive`
/// timeout the House advertises — one config value for both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_keep_alive_closes_at_the_advertised_timeout() {
    let house = house("idle-close", |config| {
        config.keep_alive = Duration::from_secs(1)
    })
    .await;
    let addr = house.local_addr();
    let (advertised, closed_after) = tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        stream
            .write_all(b"GET /ping HTTP/1.1\r\nHost: x\r\n\r\n")
            .expect("write");
        let mut answer = Vec::new();
        let mut piece = [0u8; 4096];
        while !answer.ends_with(b"Ok.\n") {
            let n = stream.read(&mut piece).expect("read");
            assert!(n > 0, "closed before the answer");
            answer.extend_from_slice(&piece[..n]);
        }
        let started = std::time::Instant::now();
        let n = stream.read(&mut piece).unwrap_or(0);
        assert_eq!(n, 0, "nothing more is sent; the server closes");
        (
            String::from_utf8_lossy(&answer).to_ascii_lowercase(),
            started.elapsed(),
        )
    })
    .await
    .expect("client");
    assert!(advertised.contains("keep-alive: timeout=1"), "{advertised}");
    assert!(
        closed_after >= Duration::from_millis(800) && closed_after < Duration::from_secs(3),
        "closed after {closed_after:?}"
    );
}
