//! Fix round 2, N2: reading a statement out of a POST body is bounded in time and
//! memory whatever the body is. In a binary of its own, so the memory it measures
//! is this test's.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::serve;
use loams_house::{ProcessLauncher, WorkerPool};

fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn statement_reading_is_bounded() {
    let launcher = ProcessLauncher::new(common::WORKER, common::tmp_root("limits"));
    let pool = WorkerPool::start(common::small(2), Arc::new(launcher))
        .await
        .expect("pool");
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        users: vec![UserMap::dev("default", "", 1, false)],
        tmp_dir: common::tmp_root("limits-spool"),
        ..HouseConfig::default()
    };
    let max = config.max_query_size;
    let house = serve(config, pool).await.expect("serves");
    let addr = house.local_addr();

    // 256 KiB of `(` in one-byte chunks: every byte is a token and a body frame.
    let mut request =
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            .to_vec();
    for _ in 0..max {
        request.extend_from_slice(b"1\r\n(\r\n");
    }
    request.extend_from_slice(b"0\r\n\r\n");
    let before = peak_rss_kib();
    let started = Instant::now();
    let answer = tokio::task::spawn_blocking(move || {
        common::http::raw(addr, &request, Duration::from_secs(60))
    })
    .await
    .expect("client");
    let took = started.elapsed();
    let grew_mib = peak_rss_kib().saturating_sub(before) / 1024;
    assert!(
        answer.starts_with("HTTP/1.1 "),
        "{}",
        &answer[..answer.len().min(200)]
    );
    assert!(
        !answer.starts_with("HTTP/1.1 200"),
        "a statement of only `(` is an error"
    );
    assert!(took < Duration::from_secs(20), "bounded in time: {took:?}");
    assert!(
        grew_mib < 128,
        "bounded in memory: the front grew {grew_mib} MiB"
    );

    // One byte over: refused before anything runs.
    let over = format!(
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        max + 1,
        "(".repeat(max + 1)
    );
    let answer = tokio::task::spawn_blocking(move || {
        common::http::raw(addr, over.as_bytes(), Duration::from_secs(30))
    })
    .await
    .expect("client");
    assert!(
        answer
            .to_ascii_lowercase()
            .contains("x-clickhouse-exception-code: 62"),
        "{}",
        &answer[..answer.len().min(300)]
    );
    assert!(answer.contains("Max query size exceeded"));

    // An INSERT head whose first piece overshoots the limit with data: the head is
    // inside the limit, so the INSERT runs and the data streams.
    let rows: String = (0..200_000).map(|n| format!("{n}\n")).collect();
    let body = format!("INSERT INTO FUNCTION null('n UInt64') FORMAT TSV\n{rows}");
    assert!(body.len() > max);
    let insert = format!(
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let answer = tokio::task::spawn_blocking(move || {
        common::http::raw(addr, insert.as_bytes(), Duration::from_secs(30))
    })
    .await
    .expect("client");
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "{}",
        &answer[..answer.len().min(300)]
    );
    assert!(
        answer.contains("\"written_rows\":\"200000\""),
        "{}",
        &answer[..answer.len().min(600)]
    );
}
