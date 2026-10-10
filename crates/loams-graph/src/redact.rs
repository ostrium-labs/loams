//! Statements without their values, for logs, spans and metric labels (GR1 Task 6; the Global
//! Constraints' "Secrets and literals").
//!
//! [`redact_literals`] replaces every string and number literal with `?` and drops comments, using
//! the same scanner as the keyword guard ([`crate::classify`]'s reading of Grafeo's lexer), so
//! what counts as a string or a comment here is what counts there. Words, `$parameters`,
//! backquoted identifiers and punctuation stay: they are the statement's shape, not its data.
//! [`fingerprint`] hashes that shape, so the same statement with other values groups together.
//!
//! Nothing here changes the statement the engine runs (D634): these are copies for telemetry.

use crate::classify::{Piece, scan};

/// The statement with each string and number literal replaced by `?` and each comment removed.
///
/// A comment is removed rather than replaced because it can hold anything (a credential pasted
/// beside a statement, say) and is not part of the statement's shape.
#[must_use]
pub fn redact_literals(statement: &str) -> String {
    let chars: Vec<char> = statement.chars().collect();
    let mut out = String::with_capacity(statement.len());
    scan(&chars, |piece, range| match piece {
        Piece::Text | Piece::Number => out.push('?'),
        Piece::Comment => {}
        Piece::Word | Piece::Identifier | Piece::Other => out.extend(&chars[range]),
    });
    out
}

/// A stable 64-bit fingerprint of a statement's shape: FNV-1a over [`redact_literals`] with runs
/// of whitespace collapsed to one space and the ends trimmed.
///
/// FNV-1a rather than the standard library's hasher, so a fingerprint means the same in every
/// process and build (a dashboard groups by it across restarts).
#[must_use]
pub fn fingerprint(statement: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let redacted = redact_literals(statement);
    let mut hash = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    let mut first = true;
    for word in redacted.split_whitespace() {
        if !first {
            feed(b" ");
        }
        first = false;
        feed(word.as_bytes());
    }
    hash
}
