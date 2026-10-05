//! Errors: the code, the name, the HTTP status and the rendering, checked against
//! ClickHouse rather than against the House's own opinion of them (FL2 Task 3).
//!
//! # What "the reference" is here, and what it is not
//!
//! The plan asks each test to compare a Loams-raised error with the reference's own
//! answer. **There is no reference `clickhouse-server` in this sandbox** — that
//! arrives with Task 10's differential harness — so the reference these tests compare
//! against is two things that can be checked here, and the difference matters:
//!
//! * **The code, the name and the status** come from ClickHouse's own source at tag
//!   `v26.9.4.3-stable`: `src/Common/ErrorCodes.cpp` for the names,
//!   `src/Server/HTTP/exceptionCodeToHTTPStatus.cpp` for the statuses. Both files are
//!   the authority ClickHouse itself uses, transcribed in [`REFERENCE_CODES`] below.
//!   They are a *source* reading, not an observation, and `errors.rs`'s module
//!   documentation says what must be re-checked when a server exists.
//! * **The message** comes from the pinned `libchdb.so` v26.9.0 running the statement
//!   in this binary, recorded in [`ENGINE_CASES`]. That is the same ClickHouse code
//!   path a reference server runs, in the same release family, so the wording is the
//!   engine's own rather than a transcription — but it is the *library's* wording, not
//!   a server's, and the two could differ in the parts a server adds (its own system
//!   tables behind "Maybe you meant …").
//!
//! What cannot be honoured here at all is the trigger for the codes only the HTTP
//! layer raises. Those are listed in [`HOUSE_CASES`] with the component that owns the
//! trigger, and the test asserts what is Task 3's — the code, the name, the status, the
//! rendering and the mid-stream rule — rather than pretending to have run a request.
//! No test in this file passes by asserting nothing.
//!
//! # One engine per process
//!
//! The three tests share this binary's engine, as `loams-chdb`'s tests do: each asks
//! for the same [`EngineConfig`], and the storage directory is this binary's own name
//! because libchdb locks its storage directory against another process's engine.

use loams_chdb::{ChdbError, Engine, EngineConfig, Session, SessionId, Settings};
use loams_house::{CODES, ChError, HouseError, MidStreamBody};

/// The ClickHouse version the pinned `libchdb.so` v26.9.0 reports through the C ABI:
/// `SELECT version()` answers this (`loams-chdb`'s `tests/engine.rs` pins it against
/// the same library). This is the string `render` appends.
const CLICKHOUSE_VERSION: &str = "26.9.2.1";

/// The ClickHouse tag the names and statuses below were read at. It is **not** the
/// version the pinned library reports: see `errors.rs`'s module documentation, which
/// records that as the one thing a reviewer has to move to a single tag.
const REFERENCE_TAG: &str = "v26.9.4.3-stable";

/// §32 §8.8's nineteen codes, with the name `src/Common/ErrorCodes.cpp` gives each
/// (`M(60, UNKNOWN_TABLE)`) and the status `src/Server/HTTP/exceptionCodeToHTTPStatus.cpp`
/// answers it with, both read at [`REFERENCE_TAG`].
///
/// Transcribed here rather than read from [`CODES`] so that the test compares two
/// independent readings of the same two files; `CODES` is the crate's answer and this is
/// the answer it is checked against.
const REFERENCE_CODES: &[(i32, &str, u16)] = &[
    (36, "BAD_ARGUMENTS", 400),
    (48, "NOT_IMPLEMENTED", 501),
    (57, "TABLE_ALREADY_EXISTS", 500),
    (60, "UNKNOWN_TABLE", 404),
    (62, "SYNTAX_ERROR", 400),
    (73, "UNKNOWN_FORMAT", 404),
    (81, "UNKNOWN_DATABASE", 404),
    (115, "UNKNOWN_SETTING", 404),
    (159, "TIMEOUT_EXCEEDED", 408),
    (164, "READONLY", 500),
    (202, "TOO_MANY_SIMULTANEOUS_QUERIES", 500),
    (210, "NETWORK_ERROR", 500),
    (241, "MEMORY_LIMIT_EXCEEDED", 500),
    (344, "SUPPORT_IS_DISABLED", 500),
    (372, "SESSION_NOT_FOUND", 500),
    (373, "SESSION_IS_LOCKED", 500),
    (394, "QUERY_WAS_CANCELLED", 500),
    (497, "ACCESS_DENIED", 403),
    (516, "AUTHENTICATION_FAILED", 403),
];

/// What the pinned library says about a failure: the exact words, or the prefix of
/// them when the tail is machine-dependent (a duration, an elapsed-time reading, the
/// list of tokens the parser accepts).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Message {
    Exact(&'static str),
    Prefix(&'static str),
}

impl Message {
    fn check(self, actual: &str, what: &str) {
        match self {
            Self::Exact(expected) => assert_eq!(actual, expected, "{what}: the engine's wording"),
            Self::Prefix(expected) => assert!(
                actual.starts_with(expected),
                "{what}: the engine's wording changed, got {actual:?}"
            ),
        }
    }
}

/// The codes the pinned library raises itself in this configuration, with the statement
/// that raises each and the words it uses.
///
/// The status column is the reference status from [`REFERENCE_CODES`], repeated here so
/// that the run compares **code, name and status** per case and not just the name.
struct EngineCase {
    code: i32,
    name: &'static str,
    status: u16,
    sql: &'static str,
    message: Message,
    /// `true` when the statement goes through `Session::execute_simple` rather than a
    /// streaming `execute`: ClickHouse refuses to stream `SET` and `CREATE`, and chDB
    /// reports that refusal as its own 36 rather than running the statement.
    simple: bool,
}

/// `numbers()`-backed statements only, and one statement per session: the engine is one
/// engine per process (FL2 Ruling 12), and a session is where its settings live.
const ENGINE_CASES: &[EngineCase] = &[
    EngineCase {
        code: 60,
        name: "UNKNOWN_TABLE",
        status: 404,
        sql: "SELECT * FROM nope",
        message: Message::Exact(
            "Unknown table expression identifier 'nope'. Maybe you meant system.one? In scope SELECT * FROM nope",
        ),
        simple: false,
    },
    EngineCase {
        code: 62,
        name: "SYNTAX_ERROR",
        status: 400,
        sql: "SELEC 1",
        // The full message lists every token the parser would have accepted, which is
        // the parser's business and not this test's.
        message: Message::Prefix(
            "Syntax error: failed at position 7 (1): 1. Expected one of: token sequence",
        ),
        simple: false,
    },
    EngineCase {
        code: 73,
        name: "UNKNOWN_FORMAT",
        status: 404,
        sql: "SELECT 1 AS one SETTINGS format = 'NoSuchHouseFormat'",
        // See the note on the doubled rendering in `codes_match_reference`.
        message: Message::Prefix(
            "Code: 73. DB::Exception: Code: 73. DB::Exception: Unknown format NoSuchHouseFormat.",
        ),
        simple: false,
    },
    EngineCase {
        code: 81,
        name: "UNKNOWN_DATABASE",
        status: 404,
        sql: "SELECT * FROM nope_db.t",
        message: Message::Exact("Database nope_db does not exist"),
        simple: false,
    },
    EngineCase {
        code: 115,
        name: "UNKNOWN_SETTING",
        status: 404,
        sql: "SET loams_no_such_setting = 1",
        message: Message::Exact("Unknown setting 'loams_no_such_setting'"),
        simple: true,
    },
    EngineCase {
        code: 159,
        name: "TIMEOUT_EXCEEDED",
        status: 408,
        sql: "SELECT sleep(3) SETTINGS max_execution_time = 1",
        // The elapsed and maximum figures are wall-clock measurements of this machine.
        message: Message::Prefix("Timeout exceeded: elapsed"),
        simple: false,
    },
    EngineCase {
        code: 241,
        name: "MEMORY_LIMIT_EXCEEDED",
        status: 500,
        sql: "SELECT sum(number) FROM numbers(1000000000) SETTINGS max_memory_usage = 1000",
        message: Message::Prefix("Query memory limit exceeded: would use"),
        simple: false,
    },
    EngineCase {
        code: 164,
        name: "READONLY",
        status: 500,
        // The table is not a `TEMPORARY` one: ClickHouse's `readonly` guard refuses a
        // table that creates storage and allows a session-local temporary table, which
        // is measured rather than assumed (see `run_simple`).
        sql: "CREATE TABLE loams_readonly_probe (x Int64) ENGINE = Memory",
        message: Message::Exact("default: Cannot execute query in readonly mode"),
        simple: true,
    },
];

/// The codes no statement raises in this configuration, and the component that owns the
/// trigger. The test asserts Task 3's part of each — code, name, status, rendering — and
/// says out loud which later task raises it.
///
/// Two of these are the plan's own words about this task: "`get` is read-only (an
/// `INSERT`, DDL or `SET` through GET answers 164 `READONLY`)" is Task 2's HTTP path, and
/// "`session_check=1` on an unknown session answers 372" is Task 4's session table. The
/// other two are limits the House enforces itself (Ruling 10) and a worker failure the
/// House sees (§32 §9).
const HOUSE_CASES: &[(i32, &str, u16, &str)] = &[
    (
        36,
        "BAD_ARGUMENTS",
        400,
        "§32 §7.4's `PARTITION BY` outside the supported transforms, in Task 5's DDL",
    ),
    (
        48,
        "NOT_IMPLEMENTED",
        501,
        "§32 §7.6's refusals with a name (`TRUNCATE` on a PK table, the type mapping's), in Task 5",
    ),
    (
        57,
        "TABLE_ALREADY_EXISTS",
        500,
        "`CREATE TABLE` onto an existing Fluss table, in Task 5's DDL",
    ),
    (
        164,
        "READONLY",
        500,
        "an `INSERT`, DDL or `SET` through `GET` and a setting over its cap (Ruling 10), in Task 2's HTTP path and Task 4's settings",
    ),
    (
        202,
        "TOO_MANY_SIMULTANEOUS_QUERIES",
        500,
        "the process's 64-query limit (Ruling 10), counted by the House; chDB 26.9.0 has no `max_concurrent_queries` setting to raise it",
    ),
    (
        210,
        "NETWORK_ERROR",
        500,
        "a House worker lost mid-query (§32 §9). chDB answers 198 `DNS_ERROR`, 400 `CANNOT_STAT` or 636 `CANNOT_EXTRACT_TABLE_STRUCTURE` for the network failures this binary can provoke, and buries a 210 inside the message of 519 `NO_REMOTE_SHARD_AVAILABLE` rather than raising one, so the top-level code is Loams' to raise",
    ),
    (
        344,
        "SUPPORT_IS_DISABLED",
        500,
        "§32 §7.8's deny list by name, in Task 8",
    ),
    (
        372,
        "SESSION_NOT_FOUND",
        500,
        "`session_check=1` on an unknown session, in Task 4's session table",
    ),
    (
        373,
        "SESSION_IS_LOCKED",
        500,
        "a session used by two queries at once, in Task 4's session table",
    ),
    (
        394,
        "QUERY_WAS_CANCELLED",
        500,
        "`KILL QUERY` and the House's own cancellation. FL2 Ruling 11 measured that chDB cannot produce it at all — `chdb_stream_cancel_query` does not interrupt a statement — so the House raises it",
    ),
    (
        497,
        "ACCESS_DENIED",
        403,
        "credentials that authenticate but are not allowed this, in Task 2's `auth.rs`",
    ),
    (
        516,
        "AUTHENTICATION_FAILED",
        403,
        "an unknown user or a wrong password, in Task 2's `auth.rs`. §32 §8.5/§8.8 said 401 for this; ClickHouse's mapping says 403, and 401 belongs to 194 `REQUIRED_PASSWORD` alone",
    ),
];

/// This binary's engine configuration.
fn config() -> EngineConfig {
    let tmp_dir = std::env::temp_dir().join(format!(
        "loams-house-test-{}",
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
    Engine::start(config()).unwrap_or_else(|err| panic!("the engine starts: {err}"))
}

/// A session with no settings.
fn session(id: &str) -> Session {
    engine()
        .session(SessionId::new(id), &Settings::new())
        .unwrap_or_else(|err| panic!("session {id} opens: {err}"))
}

/// Runs a statement and hands back the failure, or the number of bytes it produced.
///
/// A statement can fail before it streams anything — `SELEC 1` is refused by
/// `chdb_stream_query` with its own 62 — so the start error is the answer here too,
/// and not a panic.
fn run(id: &str, sql: &str) -> Result<usize, ChdbError> {
    let mut stream = session(id).execute(sql, "Native", &[])?;
    let mut bytes = 0;
    loop {
        match stream.next_chunk() {
            Ok(Some(chunk)) => bytes += chunk.len(),
            Ok(None) => return Ok(bytes),
            Err(err) => return Err(err),
        }
    }
}

/// The [`ChError`] constructor for a code, so that the test can check every
/// constructor's code/name pair against the reference rather than checking one of them.
fn loams_error(code: i32, message: &str) -> ChError {
    match code {
        36 => ChError::bad_arguments(message),
        48 => ChError::not_implemented(message),
        57 => ChError::table_already_exists(message),
        60 => ChError::unknown_table(message),
        62 => ChError::syntax_error(message),
        73 => ChError::unknown_format(message),
        81 => ChError::unknown_database(message),
        115 => ChError::unknown_setting(message),
        159 => ChError::timeout_exceeded(message),
        164 => ChError::readonly(message),
        202 => ChError::too_many_simultaneous_queries(message),
        210 => ChError::network_error(message),
        241 => ChError::memory_limit_exceeded(message),
        344 => ChError::support_is_disabled(message),
        372 => ChError::session_not_found(message),
        373 => ChError::session_is_locked(message),
        394 => ChError::query_was_cancelled(message),
        497 => ChError::access_denied(message),
        516 => ChError::authentication_failed(message),
        other => panic!("code {other} is not one of §32 §8.8's nineteen"),
    }
}

/// The reference row for a code.
fn reference(code: i32) -> (i32, &'static str, u16) {
    REFERENCE_CODES
        .iter()
        .copied()
        .find(|(c, _, _)| *c == code)
        .unwrap_or_else(|| panic!("code {code} is not one of §32 §8.8's nineteen"))
}

/// Removes the ` (version <v>)` group from a rendered error, which is the one part of
/// the text that is expected to differ between two builds: names and codes must match
/// byte for byte, versions carry the build's own (Global Constraints).
fn without_version(text: &str) -> &str {
    match text.rfind(" (version ") {
        Some(at) => text
            .get(..at)
            .unwrap_or_else(|| panic!("the version group starts at a byte boundary in {text:?}")),
        None => text,
    }
}

#[test]
fn codes_match_reference() {
    // 1. The table itself: every code, name and status as ClickHouse's two files say.
    assert_eq!(
        CODES, REFERENCE_CODES,
        "CODES must be §32 §8.8's nineteen codes with the names of ErrorCodes.cpp and the \
         statuses of exceptionCodeToHTTPStatus.cpp at {REFERENCE_TAG}"
    );
    assert_eq!(
        CODES.len(),
        19,
        "§32 §8.8 lists nineteen codes and the surface must not grow one silently"
    );
    assert_eq!(
        engine().version(),
        CLICKHOUSE_VERSION,
        "the pinned library's ClickHouse version moved, so the recorded engine messages and \
         the version `render` appends are no longer measured"
    );

    // 2. Every Loams-raised code, through its constructor: the code and the name the
    //    constructor pairs with it, the status it answers with, and the rendering.
    for &(code, name, status) in REFERENCE_CODES {
        let err = loams_error(code, "loams raised this");
        assert_eq!(err.code, code, "{name}: the constructor's code");
        assert_eq!(
            err.name, name,
            "{code}: the constructor's name is the one ErrorCodes.cpp gives this code"
        );
        assert_eq!(err.http_status(), status, "{code} {name}: the HTTP status");
        assert_eq!(
            err.http_status(),
            loams_house::errors::status_for(code),
            "{code} {name}: CODES' status and the status mapping must agree"
        );
        assert_eq!(
            err.render(CLICKHOUSE_VERSION),
            format!(
                "Code: {code}. DB::Exception: loams raised this. ({name}) (version {CLICKHOUSE_VERSION})\n"
            ),
            "{code} {name}: the rendered text"
        );
        assert_eq!(
            err.exception_code_header(),
            code.to_string(),
            "{code} {name}: X-ClickHouse-Exception-Code"
        );
    }

    // 3. The codes the pinned library raises, run here: the code, the name, the status
    //    and the engine's own words, with the version normalised out.
    for case in ENGINE_CASES {
        let outcome = if case.simple {
            run_simple(case)
        } else {
            run(&format!("engine-case-{}", case.code), case.sql)
        };
        let engine_err = match outcome {
            Err(err) => err,
            Ok(bytes) => panic!("{} produced {bytes} bytes instead of failing", case.sql),
        };
        let (reference_code, reference_name, reference_status) = reference(case.code);
        assert_eq!(
            engine_err.code, reference_code,
            "{}: the engine's code, which must be the reference's",
            case.sql
        );
        assert_eq!(
            engine_err.name, reference_name,
            "{}: the engine's name, which must be the reference's",
            case.sql
        );
        assert_eq!(
            case.status, reference_status,
            "{}: the status this test expects for the code",
            case.sql
        );
        case.message.check(&engine_err.message, case.sql);

        // The House's answer: the engine's code and name pass through, and the status is
        // ClickHouse's for that code — not a status from the House's own table, which
        // does not contain the engine's codes.
        let house = HouseError::from(engine_err.clone());
        assert_eq!(
            house.code(),
            case.code,
            "{}: the pass-through code",
            case.sql
        );
        assert_eq!(
            house.name(),
            case.name,
            "{}: the pass-through name",
            case.sql
        );
        assert_eq!(
            house.http_status(),
            case.status,
            "{}: the pass-through status, which `CODES` need not contain",
            case.sql
        );
        assert_eq!(
            house.render(CLICKHOUSE_VERSION),
            format!(
                "{} (version {CLICKHOUSE_VERSION})\n",
                engine_err.to_clickhouse_text()
            ),
            "{}: the pass-through rendering adds the version and changes nothing else",
            case.sql
        );
    }

    // A code the engine raises that is **not** one of CODES', which is the whole point
    // of the pass-through: the House must not rename it to something in its own table.
    let unknown_function = run("pass-through", "SELECT loams_no_such_function(1)")
        .err()
        .unwrap_or_else(|| panic!("an unknown function must fail"));
    assert_eq!(unknown_function.code, 46, "UNKNOWN_FUNCTION's code");
    assert_eq!(
        unknown_function.name, "UNKNOWN_FUNCTION",
        "the engine's own name for a code Loams never raises"
    );
    assert!(
        !CODES
            .iter()
            .any(|(code, _, _)| *code == unknown_function.code),
        "code 46 must not be in CODES: Loams raises §32 §8.8's list, and everything else is \
         the engine's"
    );
    let pass_through = HouseError::from(unknown_function);
    assert_eq!(
        pass_through.http_status(),
        loams_house::errors::status_for(46),
        "46 UNKNOWN_FUNCTION is 404 in ClickHouse's mapping even though it is not Loams'"
    );

    // 4. The codes only the House raises. Each one's compatibility surface is checked
    //    here — code, name, status, rendering — and its trigger belongs to the component
    //    named below, which does not exist in this task.
    for &(code, name, status, raised_by) in HOUSE_CASES {
        let err = loams_error(code, "loams raised this");
        assert_eq!(err.code, code, "{name}: the code");
        assert_eq!(err.name, name, "{name}: the name");
        assert_eq!(err.http_status(), status, "{name}: the status");
        assert_eq!(
            err.render(CLICKHOUSE_VERSION),
            format!(
                "Code: {code}. DB::Exception: loams raised this. ({name}) (version {CLICKHOUSE_VERSION})\n"
            ),
            "{name}: the rendering"
        );
        assert!(
            !ENGINE_CASES
                .iter()
                .any(|case| case.code == code && code != 164),
            "{name}: this code is listed as House-raised, so no engine case may raise it"
        );
        assert!(
            err.message.contains("loams raised this"),
            "{name}: the message is kept whole"
        );
        // The trigger is cited rather than assumed: a row that stops saying which
        // component raises the code (and why it cannot be raised here) is a row this
        // test cannot vouch for.
        assert!(
            raised_by.contains("Task") || raised_by.contains("§32") || raised_by.contains("Ruling"),
            "{name}: say which component raises it, citing the task, the design section or the ruling"
        );
    }

    // 164 is in both lists on purpose: chDB raises it for a `readonly` session, and the
    // House raises it for a write through `GET`. The two must agree on code, name and
    // status, which is what the two loops above have just checked.
    assert_eq!(
        ENGINE_CASES
            .iter()
            .find(|case| case.code == 164)
            .map(|case| case.name),
        Some("READONLY"),
        "the engine case for 164 and the House case for it must be the same error"
    );
}

/// Runs a case that has to go through `execute_simple`, including the `SET` that
/// prepares it. `SET` and `CREATE` cannot be streamed, and a session is where a setting
/// lives (FL2 Ruling 12), so the whole case runs in one session. `execute_simple`
/// answers no bytes, so the success value is 0 like [`run`]'s empty result.
fn run_simple(case: &EngineCase) -> Result<usize, ChdbError> {
    let outcome = match case.code {
        164 => {
            let session = session("engine-case-164");
            // `readonly` is a session setting (FL2 Ruling 12), so the session that sets
            // it is the session that must then be refused. Measured at v26.9.0: a
            // session in `readonly = 2` still runs `INSERT` into a `Memory` table, and
            // refuses `CREATE TABLE`, which is why this case creates storage.
            session
                .execute_simple("SET readonly = 2")
                .unwrap_or_else(|err| panic!("readonly is set: {err}"));
            session.execute_simple(case.sql)
        }
        _ => session("engine-case-simple").execute_simple(case.sql),
    };
    outcome.map(|()| 0)
}

#[test]
fn render_format_is_exact() {
    // Byte for byte, from a Loams-raised error.
    let err = ChError::unknown_table("Unknown table expression identifier 'nope'");
    let rendered = err.render("26.9.4.3.1");
    assert_eq!(
        rendered.as_bytes(),
        b"Code: 60. DB::Exception: Unknown table expression identifier 'nope'. (UNKNOWN_TABLE) (version 26.9.4.3.1)\n",
        "the wire form is ClickHouse's, byte for byte"
    );
    assert!(rendered.ends_with('\n'), "one newline, and only one");
    assert_eq!(
        rendered.matches("(version").count(),
        1,
        "exactly one version group, ours"
    );

    // The shape does not move with the version, the name, or the code.
    assert_eq!(
        err.render("26.9.2.1"),
        "Code: 60. DB::Exception: Unknown table expression identifier 'nope'. (UNKNOWN_TABLE) (version 26.9.2.1)\n",
        "the version is the only thing that changed"
    );

    // A message that already ends in a period gets one period, not two: ClickHouse's
    // `getExceptionMessageAndPattern` appends `.` only when the text does not end in
    // one, and `loams_chdb::ChdbError` drops the one it parsed away.
    let dotted = ChError::syntax_error("this DDL form is outside chsurface-1.");
    assert_eq!(
        dotted.text(),
        "Code: 62. DB::Exception: this DDL form is outside chsurface-1. (SYNTAX_ERROR)",
        "one period, before the name"
    );

    // An engine error renders with the engine's own text, so a message that contains
    // parentheses of its own (`In scope SELECT nosuchfunc(1)`) does not confuse the
    // rendering the way it would confuse a first-parenthesis parse.
    let engine_err = ChdbError::parse(
        "Code: 46. DB::Exception: Function with name `loams_no_such_function` does not exist. In scope SELECT loams_no_such_function(1). (UNKNOWN_FUNCTION)",
    )
    .unwrap_or_else(|| panic!("the text is a ClickHouse exception"));
    assert_eq!(
        HouseError::from(engine_err).render("26.9.4.3.1"),
        "Code: 46. DB::Exception: Function with name `loams_no_such_function` does not exist. In scope SELECT loams_no_such_function(1). (UNKNOWN_FUNCTION) (version 26.9.4.3.1)\n",
        "the pass-through rendering"
    );

    // `Display` is the same text without the version, so a log line and an HTTP body
    // differ only by the version the transport appends.
    assert_eq!(
        err.to_string(),
        err.text(),
        "Display is the text without the version"
    );
}

#[test]
fn mid_stream_error_matches_reference() {
    // A statement that writes rows and then fails: `throwIf` returns a value for every
    // row below 500000 and throws inside the block that contains 500000, so seven
    // blocks (458752 rows) reach the client before the failure does. Seven rather than
    // one, because the point is that the failure arrives *well after* the first byte —
    // a statement that fails in its first block would not exercise the path. The
    // failure is the engine's own 395 `FUNCTION_THROW_IF_VALUE_IS_NON_ZERO`, which is
    // not in `CODES`: a mid-stream failure is no more House-raised than any other.
    let mut stream = session("mid-stream")
        .execute(
            "SELECT throwIf(number >= 500000, 'boom') FROM numbers(1000000)",
            "Native",
            &[],
        )
        .unwrap_or_else(|err| panic!("the statement starts: {err}"));

    let mut body = MidStreamBody::new();
    let mut blocks = 0usize;
    let failure = loop {
        match stream.next_chunk() {
            Ok(Some(chunk)) => {
                blocks += 1;
                body.write(&chunk)
                    .unwrap_or_else(|_| panic!("block {blocks} arrives before the failure"));
            }
            Ok(None) => panic!("the statement did not fail, so there is nothing to append"),
            Err(err) => break err,
        }
    };
    assert!(
        blocks >= 2,
        "the failure has to arrive after rows were written, got {blocks} blocks"
    );
    let result_bytes = body.result_bytes();
    assert!(result_bytes > 0, "the client read rows before the failure");

    body.fail(HouseError::from(failure.clone()), CLICKHOUSE_VERSION);

    // The rows stay, and the exception text lands after them: the body is not a
    // truncated success and it is not replaced by the error either.
    let rendered = failure.to_clickhouse_text();
    let expected_tail = format!(" (version {CLICKHOUSE_VERSION})\n");
    assert_eq!(
        &body.body()[result_bytes..],
        format!("{rendered}{expected_tail}").as_bytes(),
        "the exception text is appended to the body already streamed"
    );
    assert_eq!(
        body.result_bytes(),
        result_bytes,
        "the appended text is not part of the result"
    );

    // And the text a driver reads is ClickHouse's, code and name included, with exactly
    // one version group — the one the House appended. (`ChdbError::parse` reads chDB's
    // own text, which carries no version, so the group is removed before parsing.)
    let tail = String::from_utf8(body.body()[result_bytes..].to_vec())
        .unwrap_or_else(|err| panic!("the appended text is utf-8: {err}"));
    assert_eq!(
        without_version(&tail),
        rendered,
        "the tail is ClickHouse's own text, with only the version group added"
    );
    assert_eq!(
        tail.matches("(version").count(),
        1,
        "the House appends one version group; the engine's text carries none for 395"
    );
    let reparsed = ChdbError::parse(without_version(&tail))
        .unwrap_or_else(|| panic!("the appended text is a ClickHouse exception: {tail:?}"));
    assert_eq!(reparsed.code, failure.code, "the appended text's code");
    assert_eq!(reparsed.name, failure.name, "the appended text's name");
    assert_eq!(
        reparsed.message, failure.message,
        "the appended text's message"
    );

    // What the transport must do with it: the status was sent with the first row, so it
    // is 200 and cannot become 404; the exception-code header is gone with the headers;
    // and the connection is closed without its terminating chunk.
    assert_eq!(
        body.status(),
        Some(200),
        "bytes were already sent, so the status line was already written"
    );
    assert_eq!(
        body.exception_code_header(),
        None,
        "X-ClickHouse-Exception-Code cannot be added after the headers"
    );
    assert!(body.must_close(), "the connection is closed, not finished");
    assert!(body.failed(), "the body carries the failure");
    assert_eq!(
        body.error().map(HouseError::code),
        Some(failure.code),
        "the failure is the one the engine raised"
    );
    assert!(
        body.write(b"more rows").is_err(),
        "no result byte may follow the exception text"
    );

    // The other half of Ruling 9, which is the half that carries the status: a failure
    // with nothing written yet is an ordinary HTTP error response with the code's own
    // status and the exception-code header, and no body to append to.
    let mut unsent = MidStreamBody::new();
    unsent.fail(
        HouseError::from(
            run("mid-stream-unsent", "SELECT * FROM nope_loams")
                .err()
                .unwrap_or_else(|| panic!("the statement fails")),
        ),
        CLICKHOUSE_VERSION,
    );
    assert_eq!(unsent.result_bytes(), 0, "nothing was written");
    assert_eq!(unsent.status(), Some(404), "60 UNKNOWN_TABLE is 404");
    assert_eq!(
        unsent.exception_code_header().as_deref(),
        Some("60"),
        "X-ClickHouse-Exception-Code: 60"
    );
    let unsent_body = String::from_utf8(unsent.into_body())
        .unwrap_or_else(|err| panic!("the body is utf-8: {err}"));
    assert!(
        unsent_body.starts_with("Code: 60. DB::Exception: "),
        "the whole body is the exception, because there was no result to keep: {unsent_body:?}"
    );
    // The message's own wording — including the "Maybe you meant …" suggestion, which
    // comes from the engine's system tables and so is not this test's to pin twice —
    // is the engine's; the code, the name, the version group and the newline are ours.
    let parsed = ChdbError::parse(without_version(unsent_body.trim_end()).trim_end())
        .unwrap_or_else(|| panic!("the body is a ClickHouse exception: {unsent_body:?}"));
    assert_eq!(parsed.code, 60, "the body's code");
    assert_eq!(parsed.name, "UNKNOWN_TABLE", "the body's name");
    assert!(
        unsent_body.ends_with(&format!(
            " (UNKNOWN_TABLE) (version {CLICKHOUSE_VERSION})\n"
        )),
        "the body's tail: {unsent_body:?}"
    );
}
