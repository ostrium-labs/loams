//! The engine, its formats, its errors and its sessions.
//!
//! One engine per process (FL2 Task 1), so this binary starts one and shares it:
//! every test here asks for the same [`EngineConfig`], which is also what
//! `engine_is_process_global` checks. The configuration's storage path is this
//! binary's own, because libchdb locks its directory against another process's
//! engine — the three test binaries of this crate each start an engine of their
//! own.

use loams_chdb::{ChdbError, Engine, EngineConfig, QueryStream, SessionId, Settings};
use parquet::file::reader::FileReader as _;

/// The ClickHouse version the pinned `libchdb.so` v26.9.0 reports through the C
/// ABI: `SELECT version()` answers this. Task 0 could not read it without a
/// build, and `chdb_version()` — the other string — answers `26.9.0`.
const CLICKHOUSE_VERSION: &str = "26.9.2.1";

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

/// Every block of a streamed result, as one buffer.
fn read_all(stream: &mut QueryStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next_chunk().expect("the result streams") {
        bytes.extend_from_slice(&chunk);
    }
    bytes
}

/// Reads one ClickHouse unsigned varint, which is how the `Native` format counts.
fn varuint(bytes: &[u8], at: &mut usize) -> u64 {
    let mut value: u64 = 0;
    for shift in (0..64).step_by(7) {
        let byte = bytes[*at];
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            break;
        }
    }
    value
}

#[test]
fn select_one_in_every_format() {
    let engine = engine();
    assert_eq!(
        engine.version(),
        CLICKHOUSE_VERSION,
        "the pinned library's ClickHouse version moved"
    );
    let session = session(engine, "formats");

    for format in [
        "TSV",
        "CSV",
        "JSONEachRow",
        "RowBinary",
        "Native",
        "Parquet",
        "ArrowStream",
    ] {
        let mut stream = session
            .execute("SELECT 1 AS one", format, &[])
            .unwrap_or_else(|err| panic!("{format} should start: {err}"));
        let bytes = read_all(&mut stream);
        assert!(
            !bytes.is_empty(),
            "{format} produced no bytes, so chDB said nothing at all"
        );

        match format {
            "TSV" => assert_eq!(bytes, b"1\n", "TSV of a one-column integer"),
            "CSV" => assert_eq!(bytes, b"1\n", "CSV of a one-column integer"),
            "JSONEachRow" => assert_eq!(bytes, b"{\"one\":1}\n", "JSONEachRow"),
            // RowBinary is the value itself: a UInt8 is one byte.
            "RowBinary" => assert_eq!(bytes, vec![1u8], "RowBinary of a UInt8"),
            // Native is a block: its name, then how many columns and rows.
            // Native is a ClickHouse block, and Task 1 measured its preamble for
            // this query to be `1, 1, "one", "UInt8", 1`: the custom-serialisation
            // marker, the column count, then each column's name and type as
            // varstrings, then its data. There is no row count to read — a block's
            // length is implied by its column data.
            "Native" => {
                let mut at = 0;
                let marker = varuint(&bytes, &mut at);
                let columns = varuint(&bytes, &mut at);
                let name_len = varuint(&bytes, &mut at) as usize;
                let name = bytes[at..at + name_len].to_vec();
                at += name_len;
                let type_len = varuint(&bytes, &mut at) as usize;
                let data_type = bytes[at..at + type_len].to_vec();
                at += type_len;
                assert_eq!(marker, 1, "the Native block's serialisation marker");
                assert_eq!(columns, 1, "the Native block's column count");
                assert_eq!(name, b"one", "the Native column's name");
                assert_eq!(data_type, b"UInt8", "the Native column's type");
                assert_eq!(bytes[at..], vec![1u8], "and the one byte of data");
            }
            // Parquet is a real file, so read it with a real reader.
            "Parquet" => {
                let reader = parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(
                    bytes.clone(),
                ))
                .expect("the Parquet bytes are a Parquet file");
                let file = reader.metadata().file_metadata();
                assert_eq!(file.num_rows(), 1, "Parquet row count");
                assert_eq!(
                    file.schema_descr().columns().len(),
                    1,
                    "Parquet column count"
                );
            }
            // ArrowStream through the byte API is chDB's own framing of an Arrow
            // IPC stream; the Arrow data path is `execute_arrow`, and
            // `arrow_output_matches_input_types` in `tests/arrow.rs` reads it.
            "ArrowStream" => {
                assert!(
                    bytes.len() > 8,
                    "the ArrowStream bytes are shorter than an Arrow IPC header"
                );
            }
            other => panic!("{other} was not expected in this test"),
        }
    }
}

#[test]
fn error_has_code_and_name() {
    let engine = engine();
    let session = session(engine, "errors");

    let mut stream = session
        .execute("SELECT * FROM nope", "TSV", &[])
        .expect("the statement starts; the engine reports the error on the first block");
    let err: ChdbError = stream
        .next_chunk()
        .expect_err("an unknown table is an error, not an empty result");

    assert_eq!(err.code, 60, "UNKNOWN_TABLE's code");
    assert_eq!(err.name, "UNKNOWN_TABLE", "the error name");
    assert!(
        err.message
            .starts_with("Unknown table expression identifier 'nope'"),
        "the engine's own wording is kept, got {:?}",
        err.message
    );
    assert_eq!(
        err.to_clickhouse_text(),
        format!("Code: 60. DB::Exception: {}. (UNKNOWN_TABLE)", err.message),
        "the rendered form is the shape ClickHouse writes"
    );
    assert!(err.is(60), "`is` answers for the code");
}

#[test]
fn sessions_are_isolated() {
    let engine = engine();
    // Different settings, which only reach chDB as a `SET` before each statement
    // (a second `chdb_connect` with different arguments is refused).
    let mut with_three = Settings::new();
    with_three.set("max_threads", "3");
    let mut with_five = Settings::new();
    with_five.set("max_threads", "5");
    let a = engine
        .session(SessionId::new("isolation-a"), &with_three)
        .expect("the first session opens");
    let b = engine
        .session(SessionId::new("isolation-b"), &with_five)
        .expect("the second session opens");

    let mut a_setting = a
        .execute("SELECT getSetting('max_threads') AS mt", "TSV", &[])
        .expect("the first session's setting is readable");
    assert_eq!(
        String::from_utf8(read_all(&mut a_setting))
            .unwrap_or_default()
            .trim(),
        "3",
        "the first session has its own setting"
    );
    let mut b_setting = b
        .execute("SELECT getSetting('max_threads') AS mt", "TSV", &[])
        .expect("the second session's setting is readable");
    assert_eq!(
        String::from_utf8(read_all(&mut b_setting))
            .unwrap_or_default()
            .trim(),
        "5",
        "the second session's setting is its own"
    );

    a.execute_simple("CREATE TEMPORARY TABLE t_isolated (x Int64) ENGINE = Memory")
        .expect("the first session creates a temporary table");
    let mut created = a
        .execute("SELECT count() FROM t_isolated", "TSV", &[])
        .expect("the first session sees its own table");
    assert_eq!(
        String::from_utf8(read_all(&mut created))
            .unwrap_or_default()
            .trim(),
        "0"
    );

    let mut seen_by_b = b
        .execute("SELECT count() FROM t_isolated", "TSV", &[])
        .expect("the second session's statement starts");
    let err = seen_by_b
        .next_chunk()
        .expect_err("a temporary table belongs to the session that made it");
    assert_eq!(err.code, 60, "the second session does not see the table");
    assert_eq!(err.name, "UNKNOWN_TABLE");
}

#[test]
fn engine_is_process_global() {
    let started = engine();

    // The same configuration answers with the same engine, which is the only
    // thing libchdb can offer: one engine per process.
    let again = Engine::start(config()).expect("the same configuration returns the same engine");
    assert!(
        std::ptr::eq(started, again),
        "a second start with the same configuration must not build a second engine"
    );

    // A different configuration is refused rather than silently ignored.
    let mut other = config();
    other.cache_bytes += 1;
    let err = Engine::start(other).expect_err("a second engine is refused");
    assert_eq!(err.name, "ALREADY_STARTED", "the refusal says why");
    assert_eq!(err.code, 0, "and it is not a ClickHouse code");
    assert!(
        err.message.contains("one engine per process"),
        "the refusal explains itself: {}",
        err.message
    );
    assert_eq!(
        engine().chdb_version(),
        "26.9.0",
        "chdb_version() is the chDB release, not the ClickHouse version"
    );
}

#[tokio::test]
async fn async_calls_go_through_a_blocking_thread() {
    let engine = engine();
    let session = session(engine, "async");
    let stream = session
        .execute_async(
            "SELECT 2 AS two".to_string(),
            "JSONEachRow".to_string(),
            Vec::new(),
        )
        .await
        .expect("execute_async starts the statement");
    let chunk = stream
        .next_chunk_async()
        .await
        .expect("next_chunk_async yields a block")
        .expect("the block is not the end of the stream");
    assert_eq!(chunk.as_ref(), b"{\"two\":2}\n");
    assert_eq!(
        engine.version_async().await.expect("the version"),
        CLICKHOUSE_VERSION
    );
}
