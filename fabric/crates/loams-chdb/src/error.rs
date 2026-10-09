//! [`ChdbError`]: chDB's exception text, taken apart into the three things a
//! ClickHouse client sees.
//!
//! chDB does not return an error code through the C ABI. It returns a result
//! whose buffer is null and whose `chdb_result_error()` holds ClickHouse's own
//! rendering of the exception, which the pinned version writes as:
//!
//! ```text
//! Code: 60. DB::Exception: Unknown table expression identifier 'nope'. Maybe you meant system.one? In scope SELECT * FROM nope. (UNKNOWN_TABLE)
//! ```
//!
//! (measured against `libchdb.so` v26.9.0, whose `SELECT version()` answers
//! `26.9.2.1`; `tests/engine.rs::error_has_code_and_name` pins that string so a
//! change in the rendering fails the test rather than silently parsing wrong).
//!
//! Two things in that text are worth knowing before parsing it. The message
//! itself contains parenthesised text — `In scope SELECT nosuchfunc(1)` — so the
//! name is the **last** parenthesised group, not the first. And the message ends
//! in a period that belongs to the `Code: <n>. DB::Exception: …` sentence rather
//! than to the message, which is why [`ChdbError::message`] drops exactly one
//! trailing `.`.
//!
//! Task 3 renders these onto the HTTP interface with the version appended
//! (`ChError::render`); this crate only has to hand the parts over.

use std::fmt;

/// What chDB writes in front of the code.
const CODE_PREFIX: &str = "Code: ";
/// What ClickHouse writes between the code and the message.
const EXCEPTION_PREFIX: &str = "DB::Exception: ";

/// A ClickHouse error as chDB reports it: the code, the name and the message.
///
/// `code` is zero for a failure that did not come from the engine as a
/// ClickHouse exception: an error Loams raised itself, or an engine message with
/// no code in it — `chdb_stream_cancel_query` answers `"No active streaming
/// query"`, not a `Code: …` line. No ClickHouse error code is zero, and Task 3's
/// `CODES` table starts at 36.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChdbError {
    /// The ClickHouse error code: `60` for `UNKNOWN_TABLE`, `394` for
    /// `QUERY_WAS_CANCELLED`.
    pub code: i32,
    /// The ClickHouse error name: `UNKNOWN_TABLE`.
    pub name: String,
    /// The message without the trailing `. (NAME)`.
    pub message: String,
}

impl ChdbError {
    /// The code a Loams-side failure carries; see the type's documentation.
    pub const LOAMS_CODE: i32 = 0;

    /// ClickHouse's code for a statement the client cancelled.
    pub const CANCELLED: i32 = 394;

    /// Parses `Code: <n>. DB::Exception: <message>. (<NAME>)`.
    ///
    /// Returns `None` for text that is not that shape, which is what an engine
    /// message without a code looks like; callers that must report something use
    /// [`ChdbError::engine`] for those.
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.trim().strip_prefix(CODE_PREFIX)?;
        let (code, rest) = rest.split_once(". ")?;
        let code: i32 = code.trim().parse().ok()?;
        // `DB::Exception:` is what ClickHouse writes, but a caller of
        // chdb_query_cmdline can put anything in front of a message, so the
        // prefix is stripped when it is there and tolerated when it is not.
        let rest = rest.strip_prefix(EXCEPTION_PREFIX).unwrap_or(rest);

        // Some paths (`chdb_stream_insert`) append the server version; the House
        // renders its own, so it is not part of the message or the name.
        let rest = rest.trim_end();
        let rest = match rest.rsplit_once(" (version ") {
            Some((before, version)) if version.ends_with(')') && !version.contains('(') => before,
            _ => rest,
        };
        // The name is the last parenthesised group: the message may contain
        // others, as `In scope SELECT nosuchfunc(1)` shows.
        let (message, tail) = rest.rsplit_once(" (")?;
        let name = tail.strip_suffix(')')?;
        let message = message.strip_suffix('.').unwrap_or(message).trim_end();

        Some(Self {
            code,
            name: name.to_string(),
            message: message.to_string(),
        })
    }

    /// An error Loams raised, outside any ClickHouse code.
    pub fn loams(name: &str, message: impl Into<String>) -> Self {
        Self {
            code: Self::LOAMS_CODE,
            name: name.to_string(),
            message: message.into(),
        }
    }

    /// An engine message that carries no ClickHouse code, kept whole.
    pub fn engine(message: impl Into<String>) -> Self {
        Self::loams("LOAMS_ENGINE_ERROR", message)
    }

    /// The 394 a cancelled statement reports.
    ///
    /// chDB itself does not answer 394 after `chdb_stream_cancel_query`: the
    /// handle is gone and the next fetch says `"No active streaming query"`,
    /// with no code and no name (measured, v26.9.0). A client that killed a
    /// statement expects `Code: 394. DB::Exception: … (QUERY_WAS_CANCELLED)`, and the House's own
    /// `KILL QUERY` path (Task 4) needs a code to answer with, so a stream that
    /// Loams itself cancelled reports 394 and keeps the engine's words as the
    /// message.
    ///
    /// The name is ClickHouse's, read from `APPLY_FOR_BUILTIN_ERROR_CODES` in
    /// `src/Common/ErrorCodes.cpp` at tag `v26.9.4.3-stable`: 394 is
    /// `QUERY_WAS_CANCELLED`. An earlier draft here said `CANCELLED`, which is
    /// not a ClickHouse error name at all, so a client matching on the name would
    /// have failed on a cancellation it caused itself.
    pub fn cancelled(query_id: &str, detail: impl Into<String>) -> Self {
        Self {
            code: Self::CANCELLED,
            name: "QUERY_WAS_CANCELLED".to_string(),
            message: format!("query {query_id} was cancelled: {}", detail.into()),
        }
    }

    /// Whether this is the given ClickHouse code.
    pub fn is(&self, code: i32) -> bool {
        self.code == code
    }

    /// Renders ClickHouse's own shape, `Code: 60. DB::Exception: … (UNKNOWN_TABLE)`.
    ///
    /// Task 3's `ChError::render` is the wire form of this — the same text with
    /// the server version appended — and is what the House sends.
    pub fn to_clickhouse_text(&self) -> String {
        format!(
            "{}DB::Exception: {}. ({})",
            format_args!("{CODE_PREFIX}{}. ", self.code),
            self.message,
            self.name
        )
    }
}

impl fmt::Display for ChdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "code {} ({}) {}", self.code, self.name, self.message)
    }
}

impl std::error::Error for ChdbError {}

#[cfg(test)]
mod tests {
    use super::ChdbError;

    /// The exact text the pinned library produced for `SELECT * FROM nope`, so a
    /// change in ClickHouse's rendering is a failing test rather than a silently
    /// mis-parsed error.
    const UNKNOWN_TABLE: &str = "Code: 60. DB::Exception: Unknown table expression identifier 'nope'. Maybe you meant system.one? In scope SELECT * FROM nope. (UNKNOWN_TABLE)";

    #[test]
    fn parses_a_clickhouse_exception() {
        let err = ChdbError::parse(UNKNOWN_TABLE).expect("the text is a ClickHouse exception");
        assert_eq!(err.code, 60);
        assert_eq!(err.name, "UNKNOWN_TABLE");
        assert_eq!(
            err.message,
            "Unknown table expression identifier 'nope'. Maybe you meant system.one? In scope SELECT * FROM nope"
        );
    }

    #[test]
    fn a_trailing_version_group_is_not_the_name() {
        // Measured (HS1 Task 3 fix round 2): errors from `chdb_stream_insert` end in
        // the server version, which the House renders itself.
        let text = "Code: 44. DB::Exception: The argument of function sleep must be constant: \
                    While executing ValuesBlockInputFormat. (ILLEGAL_COLUMN) (version 26.9.2.1)";
        let err = ChdbError::parse(text).expect("parses");
        assert_eq!(err.code, 44);
        assert_eq!(err.name, "ILLEGAL_COLUMN");
        assert_eq!(
            err.message,
            "The argument of function sleep must be constant: While executing ValuesBlockInputFormat"
        );
        assert!(!err.to_clickhouse_text().contains("version"));
    }

    #[test]
    fn takes_the_last_parenthesised_group_as_the_name() {
        // Measured for `SELECT nosuchfunc(1)`: the message contains its own
        // parentheses before the name.
        let err = ChdbError::parse(
            "Code: 46. DB::Exception: Function with name `nosuchfunc` does not exist. In scope SELECT nosuchfunc(1). (UNKNOWN_FUNCTION)",
        )
        .expect("the text is a ClickHouse exception");
        assert_eq!(err.code, 46);
        assert_eq!(err.name, "UNKNOWN_FUNCTION");
        assert!(err.message.contains("nosuchfunc(1)"));
    }

    #[test]
    fn engine_text_without_a_code_is_not_parsed() {
        assert!(ChdbError::parse("No active streaming query").is_none());
        assert!(ChdbError::parse("").is_none());
    }

    #[test]
    fn round_trips_through_clickhouse_text() {
        let err = ChdbError::parse(UNKNOWN_TABLE).expect("the text is a ClickHouse exception");
        assert_eq!(err.to_clickhouse_text(), UNKNOWN_TABLE);
    }
}
