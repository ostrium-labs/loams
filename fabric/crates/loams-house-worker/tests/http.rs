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

/// Basic `alice:secret`, `reader:books` and `alice:wrong`, encoded by hand.
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
    let launcher = ProcessLauncher::new(common::WORKER, common::tmp_root(test));
    let pool = WorkerPool::start(common::small(3), Arc::new(launcher))
        .await
        .expect("pool");
    let mut config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: users(),
        tmp_dir: common::tmp_root(&format!("{test}-spool")),
        ..HouseConfig::default()
    };
    adjust(&mut config);
    serve(config, pool).await.expect("serves")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_headers_stream() {
    let house = house("progress", |_| {}).await;
    let addr = house.local_addr();
    // One row a block, 15 ms each: about 1.5 s of output in small pieces.
    let response = get(
        addr,
        &[
            (
                "query",
                "SELECT number, sleepEachRow(0.015) FROM numbers(100) SETTINGS max_block_size = 1",
            ),
            ("send_progress_in_http_headers", "1"),
        ],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert_eq!(response.text().lines().count(), 100);
    let progress = response.all("X-ClickHouse-Progress");
    assert!(
        progress.len() >= 3,
        "progress while the query runs: {progress:?}"
    );
    for (value, _) in &progress {
        let json: serde_json::Value = serde_json::from_str(value).expect("progress is JSON");
        assert!(json["read_rows"].is_string(), "{json}");
        assert!(json["elapsed_ns"].is_string(), "{json}");
    }
    // They arrived as the query ran, not all at once at the end, and at most every
    // 100 ms (with scheduling slack).
    let first = progress.first().expect("first").1;
    let last = progress.last().expect("last").1;
    assert!(
        last.duration_since(first) >= Duration::from_millis(400),
        "spread over time"
    );
    for pair in progress.windows(2) {
        assert!(
            pair[1].1.duration_since(pair[0].1) >= Duration::from_millis(80),
            "throttled"
        );
    }
    let summary_at = response
        .all("X-ClickHouse-Summary")
        .first()
        .expect("summary at the end of the headers")
        .1;
    assert!(summary_at >= last);

    // Without the parameter, no progress headers.
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
