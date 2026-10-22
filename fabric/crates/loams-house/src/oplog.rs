//! Operator logs (HS1 Task 23, design §49 §15): one line per statement, with
//! its query id, namespace, user, kind, timings, counters, the ClickHouse code
//! it ended with and its text **with every literal redacted** ([`redact`]).
//! The full text is kept only in the tenant's own query log
//! ([`crate::querylog`]), in the tenant's bucket.
//!
//! [`StderrLines`] writes each line as one JSON object to stderr, which is
//! where a container's logs are collected; [`Memory`] keeps them for tests.

use std::io::Write as _;
use std::sync::Mutex;

use serde::Serialize;

use crate::classify::{Lexeme, lex};

/// What redaction puts in place of a literal.
pub const REDACTED: &str = "?";

/// `sql` with every string, number and heredoc literal replaced by `?` and
/// every comment removed, cut where ClickHouse's lexer cuts it
/// ([`crate::classify`]'s `lex`). Quoted names stay: they name things, and
/// redacting them would make the line useless. Text between lexemes (spaces,
/// comments, bytes outside ASCII) becomes one space.
pub fn redact(sql: &str) -> String {
    let lexemes = lex(sql);
    let mut out = String::with_capacity(sql.len());
    let mut last_end = 0;
    let mut i = 0;
    while i < lexemes.len() {
        let (lexeme, range) = &lexemes[i];
        if range.start > last_end && !out.is_empty() {
            out.push(' ');
        }
        let raw = &sql[range.clone()];
        let mut end = range.end;
        match lexeme {
            Lexeme::Other if raw.starts_with('`') || raw.starts_with('"') => out.push_str(raw),
            Lexeme::Other => out.push_str(REDACTED),
            Lexeme::Word(..) if raw.as_bytes()[0].is_ascii_digit() => {
                // A number: `1`, `0x1F`, `1e5`, and `1.5`, `1.5e-3` as the lexer
                // cuts them (words and symbols): one `?`.
                let mut j = i + 1;
                while j < lexemes.len() {
                    let (next, next_range) = &lexemes[j];
                    if next_range.start != end {
                        break;
                    }
                    let text = &sql[next_range.clone()];
                    let continues = match next {
                        Lexeme::Symbol(b'.') => true,
                        Lexeme::Symbol(b'-' | b'+') => sql[..end].ends_with(['e', 'E']),
                        Lexeme::Word(..) => {
                            text.as_bytes()[0].is_ascii_digit() || sql[..end].ends_with('.')
                        }
                        _ => false,
                    };
                    if !continues {
                        break;
                    }
                    end = next_range.end;
                    j += 1;
                }
                out.push_str(REDACTED);
                i = j - 1;
            }
            _ => out.push_str(raw),
        }
        last_end = end;
        i += 1;
    }
    out
}

/// One statement's line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct OperatorLine {
    /// When it ended, in Unix milliseconds.
    pub at_ms: u64,
    pub query_id: String,
    /// The surface: `http` or `api`.
    pub surface: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<u64>,
    pub user: String,
    /// The statement's kind (`Query`, `Insert`, …).
    pub kind: String,
    pub duration_ms: u64,
    /// The ClickHouse code it ended with; `0` for success.
    pub code: i32,
    pub read_rows: u64,
    pub read_bytes: u64,
    pub written_rows: u64,
    pub written_bytes: u64,
    pub result_rows: u64,
    /// The text, literals redacted ([`redact`]).
    pub query: String,
}

/// Where operator lines go.
pub trait OperatorLog: Send + Sync + std::fmt::Debug {
    /// Records one line. Never fails the statement.
    fn record(&self, line: &OperatorLine);
}

/// One JSON object per line on stderr.
#[derive(Debug, Default, Clone, Copy)]
pub struct StderrLines;

impl OperatorLog for StderrLines {
    fn record(&self, line: &OperatorLine) {
        let Ok(mut bytes) = serde_json::to_vec(&serde_json::json!({ "house_statement": line }))
        else {
            return;
        };
        bytes.push(b'\n');
        let _ = std::io::stderr().lock().write_all(&bytes);
    }
}

/// Lines kept in memory (tests).
#[derive(Debug, Default)]
pub struct Memory(Mutex<Vec<OperatorLine>>);

impl Memory {
    /// Every line so far.
    pub fn lines(&self) -> Vec<OperatorLine> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl OperatorLog for Memory {
    fn record(&self, line: &OperatorLine) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_logs_redact_literals() {
        for (sql, want) in [
            (
                "SELECT * FROM t WHERE password = 'hunter2' AND id = 42",
                "SELECT * FROM t WHERE password = ? AND id = ?",
            ),
            (
                "INSERT INTO `my t` VALUES (1.5, -2e-3, 0x1F, 'a''b', \"col\")",
                "INSERT INTO `my t` VALUES (?, -?, ?, ?, \"col\")",
            ),
            (
                "SELECT $tag$secret$tag$, 'x\\'y' -- a comment with 'secret'\nFROM t /* 'secret' */",
                "SELECT ?, ? FROM t",
            ),
            ("SELECT {id:UInt64}", "SELECT {id:UInt64}"),
            (
                "SELECT 1.5e10, t.1, toDate('2026-01-01')",
                "SELECT ?, t.?, toDate(?)",
            ),
            (
                "SELECT x FROM t\u{00a0}WHERE y = 'é'",
                "SELECT x FROM t WHERE y = ?",
            ),
        ] {
            assert_eq!(redact(sql), want, "{sql}");
        }
    }

    #[test]
    fn no_literal_survives() {
        let secrets = ["hunter2", "s3cr3t", "4111111111111111"];
        let sql = "SELECT 'hunter2', $$s3cr3t$$, 4111111111111111 # trailing 'hunter2'\n";
        let out = redact(sql);
        for secret in secrets {
            assert!(!out.contains(secret), "{out}");
        }
    }

    #[test]
    fn stderr_line_is_json() {
        let line = OperatorLine {
            query_id: "q".to_string(),
            surface: "http",
            query: redact("SELECT 'x'"),
            ..OperatorLine::default()
        };
        let text = serde_json::to_string(&line).expect("json");
        assert!(text.contains("\"query\":\"SELECT ?\""), "{text}");
    }
}
