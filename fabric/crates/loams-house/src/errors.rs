//! ClickHouse's errors, in ClickHouse's shape: the code, the name, the HTTP status
//! and the exception text (§32 §8.8; FL2 Task 3).
//!
//! Three things live here, and each of them is a compatibility surface rather than
//! a convenience:
//!
//! * [`CODES`] — the nineteen errors Loams raises itself, each with the name
//!   ClickHouse gives that code and the HTTP status ClickHouse answers it with.
//! * [`ChError`] — an error a driver reads: `code`, `name`, `message`, and the
//!   rendering `Code: 60. DB::Exception: … (UNKNOWN_TABLE) (version …)\n`.
//! * [`MidStreamBody`] — what happens when the failure arrives after the client has
//!   already read bytes of the result (Ruling 9).
//!
//! # Where the names and the statuses came from
//!
//! Both were **read out of ClickHouse's source at tag `v26.9.4.3-stable`**, not
//! observed on a running server, because this task has no reference
//! `clickhouse-server` to observe (Task 10 owns the differential harness that has
//! one):
//!
//! * the names from `src/Common/ErrorCodes.cpp`, the `APPLY_FOR_BUILTIN_ERROR_CODES`
//!   table — `M(60, UNKNOWN_TABLE)` and so on;
//! * the statuses from `src/Server/HTTP/exceptionCodeToHTTPStatus.cpp`, which is the
//!   function `WriteBufferFromHTTPServerResponse::setExceptionCode` calls to set the
//!   status of an error response, i.e. ClickHouse's own mapping and not this plan's
//!   summary of it.
//!
//! **Two things must be re-checked against a real server when one exists** (Task 10's
//! `codes_match_reference` compares the reference's own responses; the corpus's
//! `errors/` cases under `ErrorCode` mode are where the check lands):
//!
//! 1. **That the tag is the one this build pins.** The library here is `libchdb.so`
//!    v26.9.0, whose `SELECT version()` answers `26.9.2.1`, while the source read is
//!    `v26.9.4.3-stable` and the plan's §8.8 cites `v26.9.8.3-stable`. None of the
//!    nineteen codes or statuses moved across those three tags — that is checked by
//!    [`CODES`]'s own tests and re-checked by reading the same two files — but the
//!    pin has to move to one tag, and Global Constraints ("pinned versions move
//!    together") makes that a single PR that also regenerates `versions.rs`.
//! 2. **That the status of each code is what a driver actually receives**, which
//!    involves more than the mapping: ClickHouse can send `200 OK` with an error
//!    appended to the body once data has been sent ([`MidStreamBody`]), and a driver
//!    that only reads the status will not see the failure at all. Reading a status
//!    from a live response is the only way to catch that.
//!
//! # The statuses that surprise
//!
//! Read from the mapping rather than assumed, three of the nineteen are not what a
//! rule of thumb predicts, and they are the reason the table carries a status per
//! code at all:
//!
//! * **48 `NOT_IMPLEMENTED` is 501, not 400 or 500** — the mapping has a dedicated
//!   branch for it, and it is the only code in the nineteen with a 5xx status that is
//!   not 500.
//! * **516 `AUTHENTICATION_FAILED` is 403, not 401.** 401 belongs to code 194
//!   `REQUIRED_PASSWORD` alone; the five auth codes the mapping names (192
//!   `UNKNOWN_USER`, 193 `WRONG_PASSWORD`, 512 `SET_NON_GRANTED_ROLE`, 497
//!   `ACCESS_DENIED`, 516 `AUTHENTICATION_FAILED`) are all 403. §32 §8.8's "401/403
//!   auth" is right about the pair and imprecise about which code gets which.
//! * **115 `UNKNOWN_SETTING` is 404, not 400.** The mapping files it with the
//!   `UNKNOWN_*` names, so an unknown setting is a "not found" like an unknown table
//!   or format — which is why the whole `UNKNOWN_*` family is one branch.
//!
//! Everything the mapping does not name is 500. That is most of the nineteen: 57, 164,
//! 202, 210, 241, 344, 372, 373 and 394 are all 500, including 164 `READONLY`, which
//! is a decision rather than a failure to find something.
//!
//! # Two shapes of return value
//!
//! [`ChError::http_status`] and [`HouseError::http_status`] answer `u16` where the plan
//! wrote `StatusCode`. `StatusCode` is `http::StatusCode`, and `http` arrives with
//! Task 2's `axum`; returning the number keeps this module free of a dependency it would
//! only pass through, and Task 2's transport converts it with
//! `StatusCode::from_u16`. The numbers are the ones ClickHouse's mapping returns, so the
//! conversion cannot fail for them.
//!
//! # One thing to watch in the pass-through path
//!
//! chDB's own text is ClickHouse's text, but not always formatted **once**: a statement
//! that fails inside chDB's streaming entry point arrives wrapped in a second exception,
//! and the pinned library answers such a case with the whole rendering inside the
//! message — for `SELECT 1 SETTINGS format = 'NoSuch'` the measured `ChdbError::message`
//! begins `Code: 73. DB::Exception: Code: 73. DB::Exception: Unknown format NoSuch.` and
//! ends `(UNKNOWN_FORMAT) (version 26.9.2.1)`. That is the engine's text and
//! [`HouseError`] passes it through unchanged (§32 §8.8), so the appended version group
//! makes a second one on the wire for those cases. Whether ClickHouse's own HTTP layer
//! doubles it the same way is **not decided here**: it is one `http` request against a
//! reference server, which is Task 10's `codes_match_reference`, and until then the
//! House's text is the engine's text plus the version it promises.

use loams_house_ipc::EngineError;

/// The codes Loams raises itself, with the name ClickHouse gives each and the HTTP
/// status ClickHouse answers it with (§32 §8.8).
///
/// Names read from `src/Common/ErrorCodes.cpp` and statuses from
/// `src/Server/HTTP/exceptionCodeToHTTPStatus.cpp` at tag `v26.9.4.3-stable`; the
/// module's documentation says what that costs and what to re-check. This is the
/// whole set the House may raise: a statement Loams has not recognised does not get
/// a code of its own, it gets the chDB code chDB raised (`60` for an unknown table,
/// `46` for an unknown function).
pub const CODES: &[(i32, &str, u16)] = &[
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

/// The ClickHouse name of a code Loams raises, if it is one of [`CODES`].
///
/// A linear scan over nineteen rows, which is the right shape for a lookup that runs
/// once per raised error; [`CODES`] is a table a driver reads and a test checks, not
/// something worth a hash map.
pub fn name_for(code: i32) -> Option<&'static str> {
    CODES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, name, _)| *name)
}

/// The HTTP status ClickHouse answers an error code with.
///
/// This is [`CODES`]'s status column widened to **every** code the mapping names,
/// because a chDB error passes through with its own code (§32 §8.8) and the House
/// still owes it the right status: `46 UNKNOWN_FUNCTION` and `50 UNKNOWN_TYPE` are
/// not in [`CODES`] and are both 404.
///
/// The transcription of `exceptionCodeToHTTPStatus` (`v26.9.4.3-stable`), one branch
/// per `if` in that file, with the codes it names in the order the file tests them:
///
/// | status | codes |
/// |---|---|
/// | 401 | 194 `REQUIRED_PASSWORD` |
/// | 403 | 192 `UNKNOWN_USER`, 193 `WRONG_PASSWORD`, 512 `SET_NON_GRANTED_ROLE`, 497 `ACCESS_DENIED`, 516 `AUTHENTICATION_FAILED` |
/// | 400 | 36 `BAD_ARGUMENTS`, 427 `CANNOT_COMPILE_REGEXP`, 6 `CANNOT_PARSE_TEXT`, 25 `CANNOT_PARSE_ESCAPE_SEQUENCE`, 26 `CANNOT_PARSE_QUOTED_STRING`, 38 `CANNOT_PARSE_DATE`, 41 `CANNOT_PARSE_DATETIME`, 72 `CANNOT_PARSE_NUMBER`, 441 `CANNOT_PARSE_DOMAIN_VALUE_FROM_STRING`, 675 `CANNOT_PARSE_IPV4`, 676 `CANNOT_PARSE_IPV6`, 27 `CANNOT_PARSE_INPUT_ASSERTION_FAILED`, 376 `CANNOT_PARSE_UUID`, 15 `DUPLICATE_COLUMN`, 44 `ILLEGAL_COLUMN`, 37 `UNKNOWN_ELEMENT_IN_AST`, 123 `UNKNOWN_TYPE_OF_AST_NODE`, 8 `THERE_IS_NO_COLUMN`, 167 `TOO_DEEP_AST`, 168 `TOO_BIG_AST`, 223 `UNEXPECTED_AST_STRUCTURE`, 62 `SYNTAX_ERROR`, 117 `INCORRECT_DATA`, 53 `TYPE_MISMATCH`, 321 `VALUE_IS_OUT_OF_RANGE_OF_DATA_TYPE` |
/// | 404 | 60 `UNKNOWN_TABLE`, 46 `UNKNOWN_FUNCTION`, 47 `UNKNOWN_IDENTIFIER`, 50 `UNKNOWN_TYPE`, 56 `UNKNOWN_STORAGE`, 81 `UNKNOWN_DATABASE`, 115 `UNKNOWN_SETTING`, 152 `UNKNOWN_DIRECTION_OF_SORTING`, 63 `UNKNOWN_AGGREGATE_FUNCTION`, 73 `UNKNOWN_FORMAT`, 336 `UNKNOWN_DATABASE_ENGINE`, 78 `UNKNOWN_TYPE_OF_QUERY`, 511 `UNKNOWN_ROLE` |
/// | 413 | 229 `QUERY_IS_TOO_LARGE` |
/// | 501 | 48 `NOT_IMPLEMENTED` |
/// | 503 | 209 `SOCKET_TIMEOUT`, 76 `CANNOT_OPEN_FILE`, 1017 `ASYNC_INSERT_FLUSH_TIMEOUT`, 439 `CANNOT_SCHEDULE_TASK` |
/// | 411 | 381 `HTTP_LENGTH_REQUIRED` |
/// | 408 | 159 `TIMEOUT_EXCEEDED` |
/// | 415 | 779 `UNSUPPORTED_MEDIA_TYPE` |
/// | 500 | every other code |
///
/// Poco's enum is what the numbers come from (`base/poco/Net/include/Poco/Net/
/// HTTPResponse.h`: `HTTP_REQUEST_TIMEOUT = 408`, `HTTP_NOT_IMPLEMENTED = 501`, …),
/// so 408 is the one that is easy to get wrong as a timeout: it is a *request*
/// timeout, which is what ClickHouse returns for `TIMEOUT_EXCEEDED`.
pub fn status_for(code: i32) -> u16 {
    match code {
        194 => 401,
        192 | 193 | 497 | 512 | 516 => 403,
        6 | 8 | 15 | 25 | 26 | 27 | 36 | 37 | 38 | 41 | 44 | 53 | 62 | 72 | 117 | 123 | 167
        | 168 | 223 | 321 | 376 | 427 | 441 | 675 | 676 => 400,
        46 | 47 | 50 | 56 | 60 | 63 | 73 | 78 | 81 | 115 | 152 | 336 | 511 => 404,
        229 => 413,
        48 => 501,
        76 | 209 | 439 | 1017 => 503,
        381 => 411,
        159 => 408,
        779 => 415,
        _ => 500,
    }
}

/// An error Loams raised: one of the [`CODES`], the name that code carries, and the
/// message.
///
/// The `name` is `&'static str` because it is not the caller's to choose — §32 §8.8
/// fixes the name of each code, and a House that renders `READONLY` as
/// `UNKNOWN_SETTING` is a wrong compatibility surface. That is why every code has
/// its own constructor below and there is no public way to pair a code with a
/// hand-written name.
///
/// A code that is *not* in [`CODES`] is not this type: an error the engine raised is a
/// `loams_chdb::ChdbError` (as an `hsw1` [`EngineError`]), which carries its own name as a `String`, and reaches the transport
/// as [`HouseError::Engine`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChError {
    /// The ClickHouse error code, one of [`CODES`].
    pub code: i32,
    /// The ClickHouse name of `code`, as [`CODES`] records it.
    pub name: &'static str,
    /// The message, without the trailing `. (NAME)` that [`ChError::render`] adds.
    pub message: String,
}

impl ChError {
    /// The ClickHouse rendering of the exception, without the server version.
    ///
    /// The shape is ClickHouse's own: `getExceptionMessageAndPattern` in
    /// `src/Common/Exception.cpp` writes `"Code: " << code << ". " << displayText`
    /// and then a `.` **only if the text does not already end in one**, then
    /// `" (" << name << ")"`. `loams_chdb::ChdbError` drops the one trailing `.` when
    /// it parses the library's text, so this puts one back; a message that arrived with
    /// its own period keeps that one and does not get a second.
    pub fn text(&self) -> String {
        let period = if self.message.ends_with('.') { "" } else { "." };
        format!(
            "{}DB::Exception: {}{period} ({})",
            format_args!("Code: {}. ", self.code),
            self.message,
            self.name
        )
    }

    /// The full wire form, with the trailing newline the transport writes:
    /// `Code: 60. DB::Exception: … (UNKNOWN_TABLE) (version 26.9.2.1)\n`.
    ///
    /// `version` is passed in rather than read from a global because the House's
    /// version is the one the House serves (`versions.rs`, Global Constraints), and
    /// because `render` is a pure function of its two arguments — a test can render
    /// the same error at two versions and compare.
    pub fn render(&self, version: &str) -> String {
        format!("{} (version {})\n", self.text(), version)
    }

    /// The HTTP status for this error's code: see [`status_for`].
    pub fn http_status(&self) -> u16 {
        status_for(self.code)
    }

    /// The `X-ClickHouse-Exception-Code` value for this error.
    pub fn exception_code_header(&self) -> String {
        self.code.to_string()
    }

    // The nineteen constructors of §32 §8.8's list. One per code, so that the code
    // and the name cannot be paired wrongly at a call site, and so that the set of
    // errors the House can raise is readable in one screen.
    //
    // The doc comment on each names the case it exists for; the design section that
    // raises it is cited so a reader knows whether the wording is the design's or
    // this crate's.

    /// 36 `BAD_ARGUMENTS`: a statement whose arguments cannot be served. §32 §7.4:
    /// a `PARTITION BY` outside the supported transforms.
    pub fn bad_arguments(message: impl Into<String>) -> Self {
        Self {
            code: 36,
            name: "BAD_ARGUMENTS",
            message: message.into(),
        }
    }

    /// 48 `NOT_IMPLEMENTED`: a form outside `chsurface-1.0` with a name for it.
    /// §32 §7.6: `TRUNCATE` on a PK table, and the type mapping's refusals.
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            code: 48,
            name: "NOT_IMPLEMENTED",
            message: message.into(),
        }
    }

    /// 57 `TABLE_ALREADY_EXISTS`: `CREATE TABLE` without `IF NOT EXISTS` onto a
    /// table that exists (§32 §7.6).
    pub fn table_already_exists(message: impl Into<String>) -> Self {
        Self {
            code: 57,
            name: "TABLE_ALREADY_EXISTS",
            message: message.into(),
        }
    }

    /// 60 `UNKNOWN_TABLE`: a table this session cannot see. §32 §7.5 raises it
    /// **before** execution, from chDB's own query analysis, so a query naming a
    /// table outside the namespace never runs.
    pub fn unknown_table(message: impl Into<String>) -> Self {
        Self {
            code: 60,
            name: "UNKNOWN_TABLE",
            message: message.into(),
        }
    }

    /// 62 `SYNTAX_ERROR`: a DDL form sqlparser cannot parse (Ruling 4), with the
    /// hint "this DDL form is outside chsurface-1".
    pub fn syntax_error(message: impl Into<String>) -> Self {
        Self {
            code: 62,
            name: "SYNTAX_ERROR",
            message: message.into(),
        }
    }

    /// 73 `UNKNOWN_FORMAT`: a format outside §32 §8.4.
    pub fn unknown_format(message: impl Into<String>) -> Self {
        Self {
            code: 73,
            name: "UNKNOWN_FORMAT",
            message: message.into(),
        }
    }

    /// 81 `UNKNOWN_DATABASE`: a database outside the session's namespace (§32 §7.2).
    pub fn unknown_database(message: impl Into<String>) -> Self {
        Self {
            code: 81,
            name: "UNKNOWN_DATABASE",
            message: message.into(),
        }
    }

    /// 115 `UNKNOWN_SETTING`: a setting that is not a ClickHouse setting and not
    /// one of the `loams_*` settings (§32 §8.5).
    pub fn unknown_setting(message: impl Into<String>) -> Self {
        Self {
            code: 115,
            name: "UNKNOWN_SETTING",
            message: message.into(),
        }
    }

    /// 159 `TIMEOUT_EXCEEDED`: a statement past `max_execution_time`, or a
    /// consistency wait that ran out (§32 §7.5).
    pub fn timeout_exceeded(message: impl Into<String>) -> Self {
        Self {
            code: 159,
            name: "TIMEOUT_EXCEEDED",
            message: message.into(),
        }
    }

    /// 164 `READONLY`: a write through a read-only path — a `GET` with an `INSERT`
    /// or DDL, a setting the namespace disallows, or a value over its cap, which is
    /// **never clamped silently** (Ruling 10).
    pub fn readonly(message: impl Into<String>) -> Self {
        Self {
            code: 164,
            name: "READONLY",
            message: message.into(),
        }
    }

    /// 202 `TOO_MANY_SIMULTANEOUS_QUERIES`: past the process's concurrency limit
    /// (Ruling 10: 64 concurrent queries).
    pub fn too_many_simultaneous_queries(message: impl Into<String>) -> Self {
        Self {
            code: 202,
            name: "TOO_MANY_SIMULTANEOUS_QUERIES",
            message: message.into(),
        }
    }

    /// 210 `NETWORK_ERROR`: a House worker lost mid-query (§32 §9; the client
    /// retries).
    pub fn network_error(message: impl Into<String>) -> Self {
        Self {
            code: 210,
            name: "NETWORK_ERROR",
            message: message.into(),
        }
    }

    /// 241 `MEMORY_LIMIT_EXCEEDED`: past `max_memory_usage` for the query
    /// (Ruling 10: 4 GiB).
    pub fn memory_limit_exceeded(message: impl Into<String>) -> Self {
        Self {
            code: 241,
            name: "MEMORY_LIMIT_EXCEEDED",
            message: message.into(),
        }
    }

    /// 344 `SUPPORT_IS_DISABLED`: something §32 §7.8 denies — a denied function,
    /// engine or table function, by name.
    pub fn support_is_disabled(message: impl Into<String>) -> Self {
        Self {
            code: 344,
            name: "SUPPORT_IS_DISABLED",
            message: message.into(),
        }
    }

    /// 372 `SESSION_NOT_FOUND`: `session_check=1` on a session id that has expired
    /// or was never opened (§32 §7.3).
    pub fn session_not_found(message: impl Into<String>) -> Self {
        Self {
            code: 372,
            name: "SESSION_NOT_FOUND",
            message: message.into(),
        }
    }

    /// 373 `SESSION_IS_LOCKED`: a session used by two queries at once (§32 §7.3).
    pub fn session_is_locked(message: impl Into<String>) -> Self {
        Self {
            code: 373,
            name: "SESSION_IS_LOCKED",
            message: message.into(),
        }
    }

    /// 394 `QUERY_WAS_CANCELLED`: `KILL QUERY`, or the House's own cancellation.
    ///
    /// The name is ClickHouse's `QUERY_WAS_CANCELLED`, read from `ErrorCodes.cpp`;
    /// there is no code called `CANCELLED`. FL2 Ruling 11 measured that chDB cannot
    /// produce 394 at all — `chdb_stream_cancel_query` does not interrupt a statement —
    /// so the House raises it, and `loams_chdb::ChdbError::cancelled` (Task 1) builds
    /// the name `CANCELLED` for the same code. **The two crates disagree today**, and
    /// this one renders the name ClickHouse uses, because that is the name a driver
    /// compares against; a reviewer should rule on whether
    /// `ChdbError::cancelled` changes to match (it is one string in Task 1's
    /// `error.rs`).
    pub fn query_was_cancelled(message: impl Into<String>) -> Self {
        Self {
            code: 394,
            name: "QUERY_WAS_CANCELLED",
            message: message.into(),
        }
    }

    /// 497 `ACCESS_DENIED`: credentials that authenticate but are not allowed this
    /// (§32 §7.2).
    pub fn access_denied(message: impl Into<String>) -> Self {
        Self {
            code: 497,
            name: "ACCESS_DENIED",
            message: message.into(),
        }
    }

    /// 516 `AUTHENTICATION_FAILED`: an unknown user or a wrong password (§32 §7.2).
    pub fn authentication_failed(message: impl Into<String>) -> Self {
        Self {
            code: 516,
            name: "AUTHENTICATION_FAILED",
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ChError {
    /// The ClickHouse text without the version, so that a `tracing` line and an HTTP
    /// body differ only by the version the transport appends.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for ChError {}

/// An error the House answers with, whichever side raised it (§32 §8.8: "errors from
/// chDB pass through unchanged").
///
/// The two variants are the two origins, and they are not interchangeable:
/// [`HouseError::Raised`] is Loams's own, so its code is in [`CODES`] and its name is
/// the one ClickHouse gives that code; [`HouseError::Engine`] is chDB's, so it
/// carries the code **and name** the pinned library wrote — `46 UNKNOWN_FUNCTION` or
/// `241 MEMORY_LIMIT_EXCEEDED` raised by a query the House never classified. Both
/// render identically and both get their status from [`status_for`], which is
/// ClickHouse's mapping for either.
#[derive(Clone, Debug, thiserror::Error)]
pub enum HouseError {
    /// Raised by Loams: a [`ChError`], whose code is in [`CODES`].
    Raised(#[from] ChError),
    /// Passed through from chDB with its own code and name, as the worker's
    /// `loams_chdb::ChdbError` parsed them out of the library's exception text and
    /// an `hsw1` `Error` frame carried them to the front (HS1 Task 2: the front
    /// links no libchdb, so it holds the parts, not the parser).
    Engine(EngineError),
}

impl HouseError {
    /// The ClickHouse error code.
    pub fn code(&self) -> i32 {
        match self {
            Self::Raised(err) => err.code,
            Self::Engine(err) => err.code,
        }
    }

    /// The ClickHouse name of the code, from [`CODES`] or from the engine's text.
    pub fn name(&self) -> &str {
        match self {
            Self::Raised(err) => err.name,
            Self::Engine(err) => &err.name,
        }
    }

    /// The message, without the trailing `. (NAME) (version …)`.
    pub fn message(&self) -> &str {
        match self {
            Self::Raised(err) => &err.message,
            Self::Engine(err) => &err.message,
        }
    }

    /// The HTTP status for this error's code: see [`status_for`].
    pub fn http_status(&self) -> u16 {
        status_for(self.code())
    }

    /// The wire form, `Code: 60. DB::Exception: … (UNKNOWN_TABLE) (version …)\n`.
    pub fn render(&self, version: &str) -> String {
        format!("{} (version {})\n", self.text(), version)
    }

    /// The ClickHouse rendering without the version, for a `tracing` line or a log.
    pub fn text(&self) -> String {
        match self {
            Self::Raised(err) => err.text(),
            Self::Engine(err) => err.to_clickhouse_text(),
        }
    }

    /// The `X-ClickHouse-Exception-Code` value.
    pub fn exception_code_header(&self) -> String {
        self.code().to_string()
    }
}

impl std::fmt::Display for HouseError {
    /// The ClickHouse text without the version, for a `tracing` line or a log: the
    /// same thing [`ChError`]'s `Display` writes, so a House that logs an error and
    /// a House that sends one produce the same words.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl From<EngineError> for HouseError {
    /// A chDB failure becomes [`HouseError::Engine`], keeping its code and name.
    ///
    /// An engine message with no `Code:` line in it — `chdb_stream_cancel_query`'s
    /// "No active streaming query", which is what Ruling 11 measured — has code 0
    /// (`loams_chdb::ChdbError::LOAMS_CODE`) and a name of Loams' own making.
    /// Code 0 is not a ClickHouse code, so [`status_for`] answers 500 for it, which
    /// is the same answer ClickHouse gives code 0 `OK`'s absence: an error it cannot
    /// classify.
    fn from(err: EngineError) -> Self {
        Self::Engine(err)
    }
}

/// A response body that a failing statement leaves half-written (Ruling 9).
///
/// Ruling 9: "errors after the first byte follow ClickHouse's behaviour (the exception
/// text appended to the body and the connection closed)". ClickHouse's own code for
/// that at `v26.9.4.3-stable` is `HTTPHandler::trySendExceptionToClient` in its last
/// branch, whose comment is the whole rule in three lines:
///
/// ```text
/// // Send the error message into already used (and possibly compressed) stream.
/// // Note that the error message will possibly be sent after some data.
/// // Also HTTP code 200 could have already been sent.
/// ```
///
/// so a body that already carries rows keeps them, gains the exception text after
/// them, and the connection is closed without the terminating chunk —
/// `WriteBufferFromHTTPServerResponse::cancelWithException` breaks the protocol on
/// purpose so that a client cannot mistake a partial result for a whole one. The
/// alternative — replacing the body with the error, or truncating at the failure — is
/// what a client reads as success, which is the one thing a streaming surface must
/// not do.
///
/// Two other branches of `trySendExceptionToClient` are worth knowing, because they
/// are *not* this type's job: when the output format can write an exception into
/// itself (a framing format, or `http_write_exception_in_output_format=1`) ClickHouse
/// writes the exception **in that format** instead of as text; and a framed response
/// that has already produced bytes fails closed with an aborted connection and no
/// appended text at all. Ruling 9 asks for the plain-text behaviour, which is the
/// branch the House's declared formats mostly take: `TabSeparated`, `CSV`, `RowBinary`
/// and `Native` do not support writing an exception into themselves.
#[derive(Clone, Debug, Default)]
pub struct MidStreamBody {
    body: Vec<u8>,
    /// How much of `body` is the result. Kept current by [`MidStreamBody::write`] and
    /// left alone by [`MidStreamBody::fail`], so `result_bytes` is the number the
    /// transport needs at the moment it asks: how many bytes went out before the
    /// exception text was appended.
    result_len: usize,
    failure: Option<HouseError>,
}

impl MidStreamBody {
    /// An empty body whose first byte has not been written.
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes a block of the result.
    ///
    /// Bytes written after a failure are refused rather than appended after the
    /// exception text: the transport stops writing when [`MidStreamBody::failed`], and
    /// a caller that ignores that would produce a body whose tail is not an exception
    /// any more.
    ///
    /// # Errors
    ///
    /// [`FailedAlready`] when the statement has already failed, which the transport
    /// reports by closing the connection as [`MidStreamBody::must_close`] says.
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), FailedAlready> {
        if self.failure.is_some() {
            return Err(FailedAlready);
        }
        self.body.extend_from_slice(chunk);
        self.result_len = self.body.len();
        Ok(())
    }

    /// Appends the exception text to the body and latches the failure (Ruling 9).
    ///
    /// Called once, with the error the transport is answering; a second call is a
    /// programming error in the caller and keeps the first failure, so the text a
    /// client sees is the first thing that went wrong.
    pub fn fail(&mut self, error: HouseError, version: &str) {
        if self.failure.is_none() {
            let rendered = error.render(version);
            self.body.extend_from_slice(rendered.as_bytes());
            self.failure = Some(error);
        }
    }

    /// Whether the statement failed.
    pub fn failed(&self) -> bool {
        self.failure.is_some()
    }

    /// The error the statement failed with, if it failed.
    pub fn error(&self) -> Option<&HouseError> {
        self.failure.as_ref()
    }

    /// How many bytes of the result were written before any failure.
    ///
    /// This is what decides the status: a body with no bytes in it can still carry
    /// the code's own status, and a body with bytes in it cannot.
    pub fn result_bytes(&self) -> usize {
        self.result_len
    }

    /// The HTTP status this response carries.
    ///
    /// `None` while the statement is still running. Once it fails: the code's own
    /// status ([`status_for`]) if no byte of the body was written, and `200` if some
    /// were — because the status line was written with the first row and cannot be
    /// taken back (§32 §8.8's status is then not a failure signal at all, which is
    /// exactly why the exception text has to be in the body).
    pub fn status(&self) -> Option<u16> {
        self.failure.as_ref().map(|err| {
            if self.result_bytes() == 0 {
                err.http_status()
            } else {
                200
            }
        })
    }

    /// Whether the transport must close the connection without its terminating chunk.
    ///
    /// True once the failure has been appended: ClickHouse breaks the protocol
    /// deliberately at that point, so the client cannot read a closed body as a
    /// complete result.
    pub fn must_close(&self) -> bool {
        self.failed()
    }

    /// The `X-ClickHouse-Exception-Code` value, or `None` when the headers were
    /// already sent with the first row.
    pub fn exception_code_header(&self) -> Option<String> {
        self.failure
            .as_ref()
            .filter(|_| self.result_bytes() == 0)
            .map(|err| err.exception_code_header())
    }

    /// The whole body: the result bytes that were written, then the exception text.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Consumes the body, handing the bytes over to the transport.
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}

/// The [`MidStreamBody::write`] refusal: the statement has already failed, so its
/// exception text is the last thing in the body and no result bytes follow it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FailedAlready;
