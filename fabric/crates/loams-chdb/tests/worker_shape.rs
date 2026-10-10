//! The connection shapes the House worker needs (HS1 Task 2): a user connection
//! with query-level arguments of its own beside the engine's control connection
//! (HS1 R1.9), and a streamed `INSERT` body (HS1 R1.7).

use loams_chdb::{Engine, EngineConfig, Session, SessionId, Settings};

fn config() -> EngineConfig {
    let tmp_dir = std::env::temp_dir().join(format!(
        "loams-chdb-test-{}",
        module_path!().replace(':', "-")
    ));
    EngineConfig {
        cache_dir: tmp_dir.join("cache"),
        tmp_dir,
        filesystem_cache: false,
        ..EngineConfig::default()
    }
}

fn engine() -> &'static Engine {
    Engine::start(config()).unwrap_or_else(|err| panic!("the engine starts: {err}"))
}

fn tsv(session: &Session, sql: &str) -> String {
    let mut stream = session
        .execute(sql, "TSV", &[])
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
    let mut out = Vec::new();
    while let Some(chunk) = stream
        .next_chunk()
        .unwrap_or_else(|err| panic!("{sql}: {err}"))
    {
        out.extend_from_slice(&chunk);
    }
    String::from_utf8(out)
        .expect("utf-8")
        .trim_end()
        .to_string()
}

#[test]
fn user_connection_has_its_own_sticky_readonly() {
    let engine = engine();
    let control = engine
        .session(SessionId::new("control"), &Settings::new())
        .expect("control connection");
    let user = engine
        .session_with_args(
            SessionId::new("user"),
            &Settings::new(),
            &["--readonly=2".to_string(), "--max_threads=2".to_string()],
        )
        .expect("a second connection with different query-level arguments (HS1 R1.9)");

    assert_eq!(tsv(&user, "SELECT getSetting('readonly')"), "2");
    assert_eq!(tsv(&user, "SELECT getSetting('max_threads')"), "2");
    assert_eq!(
        tsv(&control, "SELECT getSetting('readonly')"),
        "0",
        "the control connection keeps the engine's own settings"
    );

    let err = user
        .execute_simple("SET readonly = 0")
        .expect_err("readonly = 2 is sticky");
    assert_eq!(err.code, 164, "{err:?}");
    let err = user
        .execute_simple("CREATE DATABASE nope ENGINE = Memory")
        .expect_err("readonly = 2 refuses DDL");
    assert_eq!(err.code, 164, "{err:?}");
    user.execute_simple("SET max_threads = 3")
        .expect("other settings can still be SET");
    assert_eq!(tsv(&user, "SELECT getSetting('max_threads')"), "3");
}

#[test]
fn stream_insert_takes_native_in_unaligned_chunks() {
    let engine = engine();
    let session = engine
        .session_with_args(
            SessionId::new("insert"),
            &Settings::new(),
            &["--readonly=2".to_string()],
        )
        .expect("user connection");
    session
        .execute_simple("CREATE TEMPORARY TABLE staged (n UInt64) ENGINE = Memory")
        .expect("temporary tables are allowed under readonly = 2");

    let mut body = Vec::new();
    let mut stream = session
        .execute("SELECT number AS n FROM numbers(200000)", "Native", &[])
        .expect("Native output");
    while let Some(chunk) = stream.next_chunk().expect("chunk") {
        body.extend_from_slice(&chunk);
    }
    drop(stream);

    let mut insert = session
        .insert("INSERT INTO staged", "Native")
        .expect("insert stream starts");
    for piece in body.chunks(7_777) {
        insert.append(piece).expect("append");
    }
    let summary = insert.finish().expect("insert commits");
    assert_eq!(summary.rows, 200_000, "{summary:?}");

    assert_eq!(
        tsv(&session, "SELECT count(), sum(n) FROM staged"),
        "200000\t19999900000"
    );
}

#[test]
fn stream_insert_reports_a_bad_body() {
    let engine = engine();
    let session = engine
        .session(SessionId::new("bad-body"), &Settings::new())
        .expect("session");
    session
        .execute_simple("CREATE TEMPORARY TABLE bad (n UInt64) ENGINE = Memory")
        .expect("temporary table");
    let mut insert = session
        .insert("INSERT INTO bad", "Parquet")
        .expect("insert stream starts");
    insert
        .append(b"definitely not parquet")
        .expect("append buffers");
    let err = insert.finish().expect_err("a truncated Parquet body fails");
    assert_ne!(err.code, 0, "the engine's own code comes through: {err:?}");
}

/// HS1 Task 4 (FL2 Ruling 6): ClickHouse's own parser classifies what Loams's
/// classifier does not own. Measured classes for the pinned library.
#[test]
fn chdb_classifies_statements() {
    use loams_chdb::QueryClass;
    let engine = engine();
    let session = engine
        .session(SessionId::new("classify"), &Settings::new())
        .expect("session");
    for (sql, class, statements) in [
        ("SELECT 1", QueryClass::ReadOnly, 1),
        ("SHOW TABLES", QueryClass::ReadOnly, 1),
        ("EXISTS TABLE t", QueryClass::ReadOnly, 1),
        ("INSERT INTO t VALUES (1)", QueryClass::Mutating, 1),
        (
            "CREATE TABLE t (a Int8) ENGINE = Memory",
            QueryClass::Mutating,
            1,
        ),
        ("CREATE USER u", QueryClass::MutatingGlobal, 1),
        ("SET max_threads = 1", QueryClass::Control, 1),
        ("SYSTEM DROP DNS CACHE", QueryClass::Control, 1),
        ("SELEC 1", QueryClass::Unknown, 0),
    ] {
        let analysis = session
            .classify(sql)
            .unwrap_or_else(|err| panic!("{sql}: {err}"));
        assert_eq!(
            (analysis.class, analysis.statements),
            (class, statements),
            "{sql}"
        );
    }
}
