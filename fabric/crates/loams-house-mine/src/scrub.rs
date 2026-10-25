//! Replacing literals with typed placeholders (plan HE1, Review Focus 2).
//!
//! Every literal, number or string of any quoting, and every `true`/`false`,
//! becomes a ClickHouse query parameter, `{p1:String}`, `{p2:Int64}`,
//! `{p3:Float64}`, `{p4:Bool}`, numbered from 1 in each statement. A string
//! after `DATE` or `TIMESTAMP` becomes `{pN:Date}` or `{pN:DateTime}` and the
//! keyword is dropped. Comments are removed, since they can hold anything.
//! Everything else, identifiers and keywords and punctuation, is copied from
//! the input byte for byte.
//!
//! A statement the tokenizer refuses is not scrubbed: [`scrub`] returns an
//! error and the caller keeps no sample of it. Nothing unscrubbed is ever
//! returned.

use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, Whitespace};

use crate::tokens::{self, is_literal};

/// The placeholder type of a literal token.
fn literal_type(token: &Token) -> &'static str {
    match token {
        Token::Number(text, _) => number_type(text),
        _ => "String",
    }
}

fn number_type(text: &str) -> &'static str {
    let plain = text.replace('_', "");
    let lower = plain.to_ascii_lowercase();
    if lower.starts_with("0x") || lower.starts_with("0b") {
        "UInt64"
    } else if lower.contains(['.', 'e']) || lower == "inf" || lower == "nan" {
        "Float64"
    } else if plain.parse::<i64>().is_ok() {
        "Int64"
    } else if plain.parse::<u64>().is_ok() {
        "UInt64"
    } else {
        "Float64"
    }
}

/// The result of scrubbing one statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrubbed {
    /// The statement with placeholders in place of literals.
    pub text: String,
    /// How many literals were replaced.
    pub replaced: usize,
}

/// Scrubs one statement.
///
/// # Errors
///
/// The statement could not be tokenized; it must not be kept.
pub fn scrub(statement: &str) -> Result<Scrubbed, sqlparser::tokenizer::TokenizerError> {
    let tokens = tokens::tokenize(statement)?;
    let in_type = type_arguments(&tokens);
    let mut out = String::with_capacity(statement.len());
    let mut replaced = 0;
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        match &t.token {
            Token::Whitespace(Whitespace::SingleLineComment { .. }) => out.push(' '),
            Token::Whitespace(Whitespace::MultiLineComment(_)) => out.push(' '),
            Token::Word(w)
                if w.quote_style.is_none()
                    && matches!(w.keyword, Keyword::DATE | Keyword::TIMESTAMP)
                    && next_literal(&tokens, i)
                        .is_some_and(|j| !matches!(tokens[j].token, Token::Number(..))) =>
            {
                let ty = if w.keyword == Keyword::DATE {
                    "Date"
                } else {
                    "DateTime"
                };
                replaced += 1;
                push_placeholder(&mut out, replaced, ty);
                i = next_literal(&tokens, i).unwrap_or(i);
            }
            Token::Word(w)
                if w.quote_style.is_none()
                    && matches!(w.keyword, Keyword::TRUE | Keyword::FALSE) =>
            {
                replaced += 1;
                push_placeholder(&mut out, replaced, "Bool");
            }
            _ if is_tuple_index(&tokens, i, statement) || in_type[i] => {
                out.push_str(&statement[t.range.clone()]);
            }
            token if is_literal(token) => {
                replaced += 1;
                let source = &statement[t.range.clone()];
                let ty = if source.len() > 2 && source[..2].eq_ignore_ascii_case("0x") {
                    "UInt64"
                } else {
                    literal_type(token)
                };
                push_placeholder(&mut out, replaced, ty);
            }
            _ => out.push_str(&statement[t.range.clone()]),
        }
        i += 1;
    }
    Ok(Scrubbed {
        text: out.trim().to_owned(),
        replaced,
    })
}

/// The index of the literal that follows token `i` across whitespace, if the
/// next non-whitespace token is a literal.
fn next_literal(tokens: &[tokens::Spanned], i: usize) -> Option<usize> {
    let j = (i + 1..tokens.len()).find(|&j| !matches!(tokens[j].token, Token::Whitespace(_)))?;
    is_literal(&tokens[j].token).then_some(j)
}

fn push_placeholder(out: &mut String, n: usize, ty: &str) {
    use std::fmt::Write as _;
    let _ = write!(out, "{{p{n}:{ty}}}");
}

/// Type families whose arguments are part of the type, not data:
/// `DateTime64(3, 'UTC')`, `Decimal(18, 4)`, `FixedString(16)`,
/// `Enum8('a' = 1)`. Families that are also functions (`Date`) are not here.
const PARAMETERISED_TYPES: [&str; 22] = [
    "AggregateFunction",
    "Array",
    "DateTime",
    "DateTime64",
    "Decimal",
    "Decimal128",
    "Decimal256",
    "Decimal32",
    "Decimal64",
    "Enum",
    "Enum16",
    "Enum8",
    "FixedString",
    "JSON",
    "LowCardinality",
    "Map",
    "Nested",
    "Nullable",
    "SimpleAggregateFunction",
    "Time64",
    "Tuple",
    "Variant",
];

/// For each token, whether it lies inside the parentheses of a
/// [`PARAMETERISED_TYPES`] family: its literals are kept (HE0 Task 2
/// ruling R2.12). Type arguments are written by the application, not by its
/// users.
fn type_arguments(tokens: &[tokens::Spanned]) -> Vec<bool> {
    let mut out = vec![false; tokens.len()];
    // For each open parenthesis: whether it opened a type's arguments.
    let mut stack: Vec<bool> = Vec::new();
    let mut previous_word: Option<&str> = None;
    for (i, t) in tokens.iter().enumerate() {
        match &t.token {
            Token::LParen => {
                let opens_type = previous_word.is_some_and(|w| PARAMETERISED_TYPES.contains(&w));
                let inside = stack.iter().any(|b| *b);
                stack.push(opens_type || inside);
            }
            Token::RParen => {
                stack.pop();
            }
            _ => {}
        }
        out[i] = stack.iter().any(|b| *b);
        match &t.token {
            Token::Whitespace(_) => {}
            Token::Word(w) if w.quote_style.is_none() => previous_word = Some(w.value.as_str()),
            _ => previous_word = None,
        }
    }
    out
}

/// `t.1`: the tokenizer reads a tuple element index as the number `.1`.
/// The index is syntax, not data (it can only be digits), so it is kept.
fn is_tuple_index(tokens: &[tokens::Spanned], i: usize, text: &str) -> bool {
    let t = &tokens[i];
    let Token::Number(n, _) = &t.token else {
        return false;
    };
    n.starts_with('.')
        && n[1..].bytes().all(|b| b.is_ascii_digit())
        && i > 0
        && tokens[i - 1].range.end == t.range.start
        && matches!(
            tokens[i - 1].token,
            Token::Word(_) | Token::RParen | Token::RBracket
        )
        && text
            .get(t.range.clone())
            .is_some_and(|s| s.starts_with('.'))
}

/// Whether `text` holds no literal: what a committed sample must satisfy.
/// Placeholders are punctuation and words to the tokenizer, so a scrubbed
/// statement passes; text the tokenizer refuses does not.
pub fn is_literal_free(text: &str) -> bool {
    tokens::tokenize(text).is_ok_and(|tokens| {
        let in_type = type_arguments(&tokens);
        tokens.iter().enumerate().all(|(i, t)| {
            (!is_literal(&t.token) || is_tuple_index(&tokens, i, text) || in_type[i])
                && !matches!(
                    &t.token,
                    Token::Whitespace(Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_))
                )
                && !matches!(&t.token, Token::Word(w)
                    if w.quote_style.is_none() && matches!(w.keyword, Keyword::TRUE | Keyword::FALSE))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_each_literal_kind_with_its_type() {
        let s = scrub(
            "SELECT 'a', 1, -2, 3.5, 1e3, 0xFF, 18446744073709551615, true, x'AB' FROM t -- c\nWHERE d = DATE '2024-01-01' /* x */ AND f(\"col\", `c2`)",
        )
        .unwrap();
        assert_eq!(
            s.text,
            "SELECT {p1:String}, {p2:Int64}, -{p3:Int64}, {p4:Float64}, {p5:Float64}, {p6:UInt64}, {p7:UInt64}, {p8:Bool}, {p9:String} FROM t  \nWHERE d = {p10:Date}   AND f(\"col\", `c2`)"
        );
        assert_eq!(s.replaced, 10);
        assert!(is_literal_free(&s.text));
    }

    #[test]
    fn keeps_existing_parameters_and_identifiers() {
        let s = scrub("SELECT {id:String}, t.`a b` FROM db.t SETTINGS max_threads = 4").unwrap();
        assert_eq!(
            s.text,
            "SELECT {id:String}, t.`a b` FROM db.t SETTINGS max_threads = {p1:Int64}"
        );
    }

    #[test]
    fn keeps_type_arguments() {
        let s = scrub(
            "CREATE TABLE t (a DateTime64(3, 'UTC'), b Decimal(18, 4), c Enum8('x' = 1), d Nullable(FixedString(16)), e AggregateFunction(quantiles(0.5), Float64)) ENGINE = MergeTree TTL a + INTERVAL 30 DAY",
        )
        .unwrap();
        assert_eq!(
            s.text,
            "CREATE TABLE t (a DateTime64(3, 'UTC'), b Decimal(18, 4), c Enum8('x' = 1), d Nullable(FixedString(16)), e AggregateFunction(quantiles(0.5), Float64)) ENGINE = MergeTree TTL a + INTERVAL {p1:Int64} DAY"
        );
        assert!(is_literal_free(&s.text));
        // A value cast to a type is still data; `DATE '...'` and `Date(x)`
        // are not type arguments.
        let s =
            scrub("SELECT CAST('v' AS DateTime64(3)), toDate('2024-01-01'), Date('2024-01-01')")
                .unwrap();
        assert_eq!(
            s.text,
            "SELECT CAST({p1:String} AS DateTime64(3)), toDate({p2:String}), Date({p3:String})"
        );
    }

    #[test]
    fn keeps_tuple_indexes() {
        let s = scrub("SELECT t.1, f(x).2, 1.5, x .5").unwrap();
        assert_eq!(s.text, "SELECT t.1, f(x).2, {p1:Float64}, x {p2:Float64}");
        assert!(is_literal_free(&s.text));
    }

    #[test]
    fn handles_backslash_escapes() {
        let s = scrub(r"SELECT 'it\'s secret\\', 'x''y'").unwrap();
        assert_eq!(s.text, "SELECT {p1:String}, {p2:String}");
    }

    #[test]
    fn refuses_what_it_cannot_tokenize() {
        assert!(scrub("SELECT 'unterminated").is_err());
        assert!(!is_literal_free("SELECT 'unterminated"));
    }
}
