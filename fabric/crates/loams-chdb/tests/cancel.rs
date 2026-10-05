//! Cancellation: a client that stops waiting does not have to wait for the engine.
//!
//! Like `tests/engine.rs`, this binary starts an engine of its own, because libchdb
//! locks its storage directory against another process's engine.
//!
//! Task 1 measured what libchdb v26.9.0 can and cannot do about a running statement,
//! and it shapes this test:
//!
//! * `chdb_stream_cancel_query` does **not** interrupt a statement. It blocks until
//!   the statement has finished on its own — `SELECT count() FROM numbers(1e12)`
//!   answered with its result after 275 s, and the cancel call returned a second
//!   earlier — and only then tears the stream down, after which a fetch says
//!   `"No active streaming query"` with no code.
//! * There is no `query_id` setting to name a statement with (`SET query_id` answers
//!   115 `UNKNOWN_SETTING`), so a cross-connection `KILL QUERY` has nothing to match
//!   on: `system.processes` lists ids the engine generated itself.
//!
//! So the cancellation a client sees is Loams': `Session::cancel` registers it, the
//! stream answers `394 QUERY_WAS_CANCELLED` at once, and the engine is asked to stop in the
//! background. That is what "cancelled within 1 s" can mean against this ABI, and
//! this test is what says so.

use std::time::{Duration, Instant};

use loams_chdb::{ChdbError, Engine, EngineConfig, SessionId, Settings};

/// The bound the plan asks for: a cancellation that has been acknowledged within a
/// second has stopped the statement as far as the client is concerned.
const WITHIN: Duration = Duration::from_secs(1);

/// This binary's engine configuration.
fn config() -> EngineConfig {
    let tmp_dir = std::env::temp_dir().join(format!(
        "loams-chdb-test-{}",
        module_path!().replace(':', "-")
    ));
    EngineConfig {
        cache_dir: tmp_dir.join("cache"),
        tmp_dir,
        ..EngineConfig::default()
    }
}

/// The process's engine, started with [`config`].
fn engine() -> &'static Engine {
    Engine::start(config()).expect("the engine starts for this binary's configuration")
}

/// A session with no settings.
fn session(engine: &Engine, id: &str) -> loams_chdb::Session {
    engine
        .session(SessionId::new(id), &Settings::new())
        .expect("a session opens")
}

#[test]
fn cancel_stops_a_long_query() {
    let engine = engine();
    let session = session(engine, "cancel");
    let query_id = "loams-cancel-test-1";

    // A statement that takes minutes if it is left alone: the engine answers
    // `chdb_stream_query` in about a millisecond and runs the work itself.
    let mut stream = session
        .execute_with_id(
            query_id,
            "SELECT count() FROM numbers(1e12)",
            "RowBinary",
            &[],
        )
        .expect("the long statement starts");
    assert_eq!(
        stream.query_id(),
        query_id,
        "the stream carries the caller's id"
    );

    // Let the engine get going before the cancellation, so this is a cancellation
    // and not a statement that was never started.
    std::thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    session
        .cancel(query_id)
        .expect("a running statement can be cancelled");

    // The stream answers at once with the code ClickHouse reports for a killed
    // statement, which is what a client that cancelled expects to see.
    let err: ChdbError = stream
        .next_chunk()
        .expect_err("a cancelled stream yields no block");
    let elapsed = started.elapsed();

    assert_eq!(
        err.code, 394,
        "ClickHouse's code for a cancelled statement, got {err}"
    );
    assert_eq!(
        err.name, "QUERY_WAS_CANCELLED",
        "and ClickHouse's name for 394"
    );
    assert!(
        err.message.contains(query_id),
        "the message names the statement that was cancelled: {}",
        err.message
    );
    assert!(
        elapsed < WITHIN,
        "the cancellation was acknowledged in {elapsed:?}, which is not within {WITHIN:?}"
    );
    assert!(stream.is_done(), "a cancelled stream is finished");
    assert!(
        stream
            .next_chunk()
            .expect("a finished stream asks no more")
            .is_none()
    );

    // A statement that has finished cannot be cancelled, and the refusal is what
    // lets `KILL QUERY` answer rather than lie.
    let mut finished = session
        .execute_with_id("loams-cancel-finished", "SELECT 1", "RowBinary", &[])
        .expect("the short statement starts");
    while finished
        .next_chunk()
        .expect("the short statement streams")
        .is_some()
    {}
    drop(finished);
    let err = session
        .cancel("loams-cancel-finished")
        .expect_err("a finished statement cannot be cancelled");
    assert_eq!(err.name, "QUERY_ID_UNKNOWN");
}

#[tokio::test]
async fn cancel_on_a_blocking_thread() {
    let engine = engine();
    let session = session(engine, "cancel-async");
    let query_id = "loams-cancel-test-3";

    let stream = session
        .execute_with_id(
            query_id,
            "SELECT count() FROM numbers(1e12)",
            "RowBinary",
            &[],
        )
        .expect("the long statement starts");
    session
        .cancel_async(query_id.to_string())
        .await
        .expect("cancel_async registers the cancellation");
    let err = stream
        .next_chunk_async()
        .await
        .expect_err("a cancelled stream yields no block");
    assert_eq!(err.code, 394, "the same answer through the async API");
}
