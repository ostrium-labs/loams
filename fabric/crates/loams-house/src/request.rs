//! What the HTTP interface reads out of a request before a worker sees it: the
//! query string, and the shape of the statement (HS1 Task 3).
//!
//! HS1 Task 4's classifier replaces the statement half with sqlparser. Until then
//! it knows just enough: the first keyword (GET and read-only users may only read),
//! a trailing `FORMAT <f>`, and where an `INSERT … FORMAT <f>` line ends, so the
//! data after it can stream. Everything here is pure and bounded, which is what
//! the `request_parse` fuzz target drives.
//!
//! The tokenizer is incremental ([`Scanner`]): bytes of a POST body are scanned
//! once as they arrive, never re-scanned per piece (Task 3 review I3).

use loams_house_ipc::InputSpec;

use crate::errors::{ChError, HouseError};

/// The parameters that are not settings (FL2 Task 2, plus Task 3 review I7):
/// handled by the front, or accepted and ignored, never passed to the engine.
/// `param_<name>` are query parameters; every other parameter is a setting.
pub const NON_SETTINGS: &[&str] = &[
    "query",
    "database",
    "default_format",
    "query_id",
    "session_id",
    "session_timeout",
    "session_check",
    "close_session",
    "user",
    "password",
    "quota_key",
    "role",
    "stacktrace",
    "client_protocol_version",
    "compress",
    "decompress",
    "enable_http_compression",
    "wait_end_of_query",
    "buffer_size",
    "send_progress_in_http_headers",
];

fn bad(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::bad_arguments(message.into()))
}

/// `a=b&c=d`: percent-decoded byte for byte (`+` is a space, as in HTML forms), and
/// refused with `36` when an escape is not two hex digits (`%+5`, `%4`) or the bytes
/// are not UTF-8 (Task 3 review M6) — never guessed at.
pub fn parse_query(query: &str) -> Result<Vec<(String, String)>, HouseError> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((percent_decode(key)?, percent_decode(value)?))
        })
        .collect()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(text: &str) -> Result<String, HouseError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let (Some(hi), Some(lo)) = (
                    bytes.get(i + 1).copied().and_then(hex),
                    bytes.get(i + 2).copied().and_then(hex),
                ) else {
                    return Err(bad(format!(
                        "Cannot parse the URL: a '%' escape must be two hex digits in {text:?}"
                    )));
                };
                out.push(hi << 4 | lo);
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| {
        bad(format!(
            "Cannot parse the URL: {text:?} does not decode to UTF-8"
        ))
    })
}

/// A word or a symbol of a statement, outside strings and comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    /// Where it starts in the text.
    pub start: usize,
    /// Where it ends.
    pub end: usize,
    /// Whether it is a word (`[A-Za-z0-9_]+`).
    pub word: bool,
    /// Parenthesis depth.
    pub depth: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lex {
    Normal,
    Word(usize),
    Quote {
        quote: u8,
        start: usize,
        escape: bool,
        closing: bool,
    },
    MaybeComment(u8),
    LineComment,
    BlockComment {
        star: bool,
    },
}

/// Where an `INSERT`'s data starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertHead {
    /// Not an `INSERT`.
    NotInsert,
    /// Not known yet: an `INSERT` whose `FORMAT <f>` line has not arrived, or no
    /// word at all yet.
    Incomplete,
    /// An `INSERT … FORMAT <f>`: the statement, the format, and where the data
    /// starts in the text.
    Insert {
        statement: String,
        format: String,
        data_start: usize,
    },
    /// An `INSERT` without `FORMAT` (`VALUES (…)` inline, or `… SELECT`).
    NoFormat,
}

/// An incremental tokenizer: [`Scanner::advance`] scans only the bytes it has not
/// seen, so feeding a body piece by piece is linear in its length.
#[derive(Debug, Clone)]
pub struct Scanner {
    pos: usize,
    depth: i32,
    lex: Lex,
    tokens: Vec<Token>,
    /// The first `FORMAT` at depth 0, by token index, once seen.
    format_at: Option<usize>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    /// A scanner at the start of a statement.
    pub fn new() -> Self {
        Self {
            pos: 0,
            depth: 0,
            lex: Lex::Normal,
            tokens: Vec::new(),
            format_at: None,
        }
    }

    /// The tokens so far.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    fn push(&mut self, token: Token, text: &[u8]) {
        if token.word
            && token.depth == 0
            && self.format_at.is_none()
            && text[token.start..token.end].eq_ignore_ascii_case(b"FORMAT")
        {
            self.format_at = Some(self.tokens.len());
        }
        self.tokens.push(token);
    }

    /// Scans `text[seen..]`. `text` must be the same bytes as before, grown.
    pub fn advance(&mut self, text: &[u8]) {
        while self.pos < text.len() {
            let i = self.pos;
            let c = text[i];
            self.pos += 1;
            match self.lex {
                Lex::Normal => self.normal(c, i, text),
                Lex::Word(start) => {
                    if c.is_ascii_alphanumeric() || c == b'_' {
                        continue;
                    }
                    let depth = self.depth;
                    self.push(
                        Token {
                            start,
                            end: i,
                            word: true,
                            depth,
                        },
                        text,
                    );
                    self.lex = Lex::Normal;
                    self.normal(c, i, text);
                }
                Lex::Quote {
                    quote,
                    start,
                    escape,
                    closing,
                } => {
                    if closing {
                        if c == quote {
                            // A doubled quote: still inside.
                            self.lex = Lex::Quote {
                                quote,
                                start,
                                escape: false,
                                closing: false,
                            };
                        } else {
                            let depth = self.depth;
                            self.push(
                                Token {
                                    start,
                                    end: i,
                                    word: false,
                                    depth,
                                },
                                text,
                            );
                            self.lex = Lex::Normal;
                            self.normal(c, i, text);
                        }
                    } else if escape {
                        self.lex = Lex::Quote {
                            quote,
                            start,
                            escape: false,
                            closing: false,
                        };
                    } else if c == b'\\' {
                        self.lex = Lex::Quote {
                            quote,
                            start,
                            escape: true,
                            closing: false,
                        };
                    } else if c == quote {
                        self.lex = Lex::Quote {
                            quote,
                            start,
                            escape: false,
                            closing: true,
                        };
                    }
                }
                Lex::MaybeComment(first) => {
                    if first == b'-' && c == b'-' {
                        self.lex = Lex::LineComment;
                    } else if first == b'/' && c == b'*' {
                        self.lex = Lex::BlockComment { star: false };
                    } else {
                        let depth = self.depth;
                        self.push(
                            Token {
                                start: i - 1,
                                end: i,
                                word: false,
                                depth,
                            },
                            text,
                        );
                        self.lex = Lex::Normal;
                        self.normal(c, i, text);
                    }
                }
                Lex::LineComment => {
                    if c == b'\n' {
                        self.lex = Lex::Normal;
                    }
                }
                Lex::BlockComment { star } => {
                    self.lex = if star && c == b'/' {
                        Lex::Normal
                    } else {
                        Lex::BlockComment { star: c == b'*' }
                    };
                }
            }
        }
    }

    fn normal(&mut self, c: u8, i: usize, text: &[u8]) {
        match c {
            c if c.is_ascii_whitespace() => {}
            b'#' => self.lex = Lex::LineComment,
            b'-' | b'/' => self.lex = Lex::MaybeComment(c),
            b'\'' | b'"' | b'`' => {
                self.lex = Lex::Quote {
                    quote: c,
                    start: i,
                    escape: false,
                    closing: false,
                }
            }
            c if c.is_ascii_alphanumeric() || c == b'_' => self.lex = Lex::Word(i),
            _ => {
                if c == b'(' {
                    self.depth += 1;
                }
                let depth = self.depth;
                self.push(
                    Token {
                        start: i,
                        end: i + 1,
                        word: false,
                        depth,
                    },
                    text,
                );
                if c == b')' {
                    self.depth -= 1;
                }
            }
        }
    }

    /// Ends the text: a word, quote or symbol still open becomes a token.
    pub fn finish(&mut self, text: &[u8]) {
        self.advance(text);
        let depth = self.depth;
        match self.lex {
            Lex::Word(start) => self.push(
                Token {
                    start,
                    end: text.len(),
                    word: true,
                    depth,
                },
                text,
            ),
            Lex::Quote { start, .. } => self.push(
                Token {
                    start,
                    end: text.len(),
                    word: false,
                    depth,
                },
                text,
            ),
            Lex::MaybeComment(_) => self.push(
                Token {
                    start: text.len() - 1,
                    end: text.len(),
                    word: false,
                    depth,
                },
                text,
            ),
            _ => {}
        }
        self.lex = Lex::Normal;
    }

    /// The `INSERT` shape of `text` so far. `complete`: no more bytes will come
    /// (call [`Scanner::finish`] first). Cost: constant after the scan.
    pub fn insert_head(&self, text: &[u8], complete: bool) -> InsertHead {
        let Some(first) = self.tokens.iter().find(|t| t.word) else {
            return if complete {
                InsertHead::NotInsert
            } else {
                InsertHead::Incomplete
            };
        };
        if !text[first.start..first.end].eq_ignore_ascii_case(b"INSERT") {
            return InsertHead::NotInsert;
        }
        let Some(at) = self.format_at else {
            return if complete {
                InsertHead::NoFormat
            } else {
                InsertHead::Incomplete
            };
        };
        let format = self.tokens[at];
        let Some(name) = self.tokens.get(at + 1).filter(|n| n.word) else {
            return if complete {
                InsertHead::NoFormat
            } else {
                InsertHead::Incomplete
            };
        };
        // A name still being scanned may grow ("TS" of "TSV").
        if !complete && name.end == text.len() {
            return InsertHead::Incomplete;
        }
        let mut i = name.end;
        while i < text.len() && (text[i] == b' ' || text[i] == b'\t') {
            i += 1;
        }
        if i < text.len() && text[i] == b'\r' {
            i += 1;
        }
        if i < text.len() && text[i] == b'\n' {
            i += 1;
        } else if i >= text.len() && !complete {
            return InsertHead::Incomplete;
        }
        InsertHead::Insert {
            statement: String::from_utf8_lossy(&text[..format.start])
                .trim_end()
                .to_string(),
            format: String::from_utf8_lossy(&text[name.start..name.end]).into_owned(),
            data_start: i,
        }
    }
}

/// The tokens of a complete text.
pub fn tokens(text: &[u8]) -> Vec<Token> {
    let mut scanner = Scanner::new();
    scanner.finish(text);
    scanner.tokens
}

fn word_is(text: &[u8], t: &Token, word: &str) -> bool {
    t.word && text[t.start..t.end].eq_ignore_ascii_case(word.as_bytes())
}

/// The first keyword, upper-cased, past comments and opening parentheses.
pub fn first_keyword(sql: &str) -> String {
    let bytes = sql.as_bytes();
    tokens(bytes)
        .into_iter()
        .find(|t| t.word)
        .map(|t| String::from_utf8_lossy(&bytes[t.start..t.end]).to_ascii_uppercase())
        .unwrap_or_default()
}

/// Whether a statement only reads (what GET and read-only users may run).
pub fn is_read(sql: &str) -> bool {
    matches!(
        first_keyword(sql).as_str(),
        "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXISTS" | "EXPLAIN" | "CHECK"
    )
}

/// The statement as the worker gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The statement to run after any input (empty for a bare `INSERT`).
    pub sql: String,
    /// The output format a trailing `FORMAT` chose.
    pub format: Option<String>,
    /// Where the `INSERT` data goes.
    pub input: Option<InputSpec>,
    /// Data that came with the statement text, before the rest of the body.
    pub first_data: Vec<u8>,
}

/// What to run for a complete statement `text`. `body_follows`: the request body is
/// (also) the `INSERT`'s data.
pub fn plan(text: &[u8], body_follows: bool) -> Plan {
    let mut scanner = Scanner::new();
    scanner.finish(text);
    match scanner.insert_head(text, true) {
        InsertHead::Insert {
            statement,
            format,
            data_start,
        } => Plan {
            sql: String::new(),
            format: None,
            input: Some(InputSpec {
                insert: statement,
                format,
            }),
            first_data: text[data_start.min(text.len())..].to_vec(),
        },
        InsertHead::NoFormat
            if body_follows && ends_with_word(text, scanner.tokens(), "VALUES") =>
        {
            // `INSERT INTO t VALUES` with the tuples in the body.
            let values = scanner.tokens().last().map_or(text.len(), |t| t.start);
            Plan {
                sql: String::new(),
                format: None,
                input: Some(InputSpec {
                    insert: String::from_utf8_lossy(&text[..values])
                        .trim_end()
                        .to_string(),
                    format: "Values".to_string(),
                }),
                first_data: Vec::new(),
            }
        }
        _ => {
            let sql = String::from_utf8_lossy(text).into_owned();
            let (sql, format) = split_format(&sql);
            Plan {
                sql,
                format,
                input: None,
                first_data: Vec::new(),
            }
        }
    }
}

fn ends_with_word(text: &[u8], tokens: &[Token], word: &str) -> bool {
    tokens.last().is_some_and(|t| word_is(text, t, word))
}

/// Strips a trailing top-level `FORMAT <f>` (and `;`), returning the format.
pub fn split_format(sql: &str) -> (String, Option<String>) {
    let bytes = sql.as_bytes();
    let toks: Vec<Token> = tokens(bytes)
        .into_iter()
        .filter(|t| !(bytes[t.start] == b';' && t.end == t.start + 1))
        .collect();
    if toks.len() >= 2 {
        let name = toks[toks.len() - 1];
        let keyword = toks[toks.len() - 2];
        if name.word && keyword.depth == 0 && word_is(bytes, &keyword, "FORMAT") {
            return (
                sql[..keyword.start].trim_end().to_string(),
                Some(sql[name.start..name.end].to_string()),
            );
        }
    }
    (
        sql.trim_end().trim_end_matches(';').trim_end().to_string(),
        None,
    )
}

/// The `request_parse` fuzz target's body (Task 3 review I8), shared with a
/// stable property test: every parser here on arbitrary bytes, never panicking,
/// and the incremental scan equal to the whole one at an arbitrary split.
pub fn fuzz_request(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let _ = parse_query(&text);
    let _ = is_read(&text);
    let _ = split_format(&text);
    let _ = plan(data, true);
    let _ = plan(data, false);
    let cut = data.first().map_or(0, |b| *b as usize % (data.len() + 1));
    let mut whole = Scanner::new();
    whole.finish(data);
    let mut pieces = Scanner::new();
    pieces.advance(&data[..cut]);
    pieces.advance(data);
    pieces.finish(data);
    assert_eq!(
        whole.tokens(),
        pieces.tokens(),
        "scanning in pieces changed the tokens"
    );
    assert_eq!(
        whole.insert_head(data, true),
        pieces.insert_head(data, true)
    );
    let mut partial = Scanner::new();
    partial.advance(&data[..cut]);
    let _ = partial.insert_head(&data[..cut], false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_decode_byte_for_byte() {
        assert_eq!(
            parse_query("query=SELECT%201+%2B%201&x=&y&p=%C3%A9").expect("decodes"),
            vec![
                ("query".to_string(), "SELECT 1 + 1".to_string()),
                ("x".to_string(), String::new()),
                ("y".to_string(), String::new()),
                ("p".to_string(), "é".to_string()),
            ]
        );
        for bad in ["a=%+5", "a=%4", "a=%zz", "a=%", "a=%FF", "%G0=1"] {
            assert_eq!(parse_query(bad).expect_err(bad).code(), 36, "{bad}");
        }
    }

    #[test]
    fn read_only_keywords() {
        for sql in [
            "SELECT 1",
            " -- c\n select 1",
            "/* x */ WITH 1 AS a SELECT a",
            "(SELECT 1)",
            "SHOW TABLES",
            "EXPLAIN SELECT 1",
        ] {
            assert!(is_read(sql), "{sql}");
        }
        for sql in [
            "INSERT INTO t VALUES (1)",
            "SET a = 1",
            "CREATE TABLE t (a Int8)",
            "DROP TABLE t",
            "",
            "KILL QUERY WHERE 1",
            "# x\nDROP TABLE t",
        ] {
            assert!(!is_read(sql), "{sql}");
        }
    }

    fn head(text: &[u8], complete: bool) -> InsertHead {
        let mut scanner = Scanner::new();
        if complete {
            scanner.finish(text);
        } else {
            scanner.advance(text);
        }
        scanner.insert_head(text, complete)
    }

    #[test]
    fn insert_heads() {
        let text = b"INSERT INTO FUNCTION null('n UInt64') FORMAT TSV\n1\n2\n";
        match head(text, true) {
            InsertHead::Insert {
                statement,
                format,
                data_start,
            } => {
                assert_eq!(statement, "INSERT INTO FUNCTION null('n UInt64')");
                assert_eq!(format, "TSV");
                assert_eq!(&text[data_start..], b"1\n2\n");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            head(b"INSERT INTO t FORMAT TS", false),
            InsertHead::Incomplete
        );
        assert_eq!(
            head(b"INSERT INTO t FORMAT TSV", false),
            InsertHead::Incomplete
        );
        assert!(matches!(
            head(b"INSERT INTO t FORMAT TSV", true),
            InsertHead::Insert { .. }
        ));
        assert_eq!(
            head(b"INSERT INTO t VALUES (1)", true),
            InsertHead::NoFormat
        );
        assert_eq!(head(b"SELECT 1 FORMAT TSV", true), InsertHead::NotInsert);
        assert_eq!(
            head(b"INSERT INTO t SELECT 'FORMAT TSV', format('x')", true),
            InsertHead::NoFormat
        );
    }

    /// The scan in pieces finds exactly what the scan in one go finds, for every
    /// split of a statement (review I3: incremental, not re-scanned).
    #[test]
    fn scanning_in_pieces_equals_scanning_whole() {
        let text: &[u8] =
            b"INSERT /* c */ INTO t (a, `b c`) SETTINGS x = 'it''s -- not' FORMAT CSV\r\n1,2\n";
        let whole = {
            let mut s = Scanner::new();
            s.finish(text);
            (s.tokens().to_vec(), s.insert_head(text, true))
        };
        for cut in 0..=text.len() {
            let mut s = Scanner::new();
            s.advance(&text[..cut]);
            s.finish(text);
            assert_eq!(
                (s.tokens().to_vec(), s.insert_head(text, true)),
                whole,
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn trailing_format() {
        assert_eq!(
            split_format("SELECT 1 FORMAT JSON"),
            ("SELECT 1".to_string(), Some("JSON".to_string()))
        );
        assert_eq!(
            split_format("SELECT 1 FORMAT JSON;"),
            ("SELECT 1".to_string(), Some("JSON".to_string()))
        );
        assert_eq!(split_format("SELECT 1;"), ("SELECT 1".to_string(), None));
        assert_eq!(
            split_format("SELECT 'FORMAT JSON'"),
            ("SELECT 'FORMAT JSON'".to_string(), None)
        );
        assert_eq!(
            split_format("SELECT format('x', 1)"),
            ("SELECT format('x', 1)".to_string(), None)
        );
    }

    #[test]
    fn plans() {
        let p = plan(b"INSERT INTO t VALUES", true);
        assert_eq!(
            p.input,
            Some(InputSpec {
                insert: "INSERT INTO t".to_string(),
                format: "Values".to_string()
            })
        );
        let p = plan(b"INSERT INTO t VALUES (1)", true);
        assert_eq!(p.input, None);
        assert_eq!(p.sql, "INSERT INTO t VALUES (1)");
    }
}
