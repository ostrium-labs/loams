//! Tokens with byte ranges, from sqlparser's ClickHouse tokenizer.
//!
//! The scrubber and the migration splitter both work on tokens rather than on
//! the AST, so they also handle statements the parser refuses. The tokenizer
//! reports 1-based line and character columns; [`tokenize`] turns them into
//! byte ranges of the input.

use sqlparser::dialect::ClickHouseDialect;
use sqlparser::tokenizer::{Location, Token, Tokenizer};

/// A token and the bytes of the input it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    /// The token.
    pub token: Token,
    /// Its byte range in the input.
    pub range: std::ops::Range<usize>,
    /// Its 1-based line.
    pub line: u64,
}

/// Tokenizes `text` with the ClickHouse dialect.
///
/// # Errors
///
/// The tokenizer's error, for example an unterminated string.
pub fn tokenize(text: &str) -> Result<Vec<Spanned>, sqlparser::tokenizer::TokenizerError> {
    let dialect = ClickHouseDialect {};
    let tokens = Tokenizer::new(&dialect, text).tokenize_with_location()?;
    let index = LineIndex::new(text);
    Ok(tokens
        .into_iter()
        .filter(|t| t.token != Token::EOF)
        .map(|t| {
            let start = index.offset(t.span.start);
            let end = index.offset(t.span.end);
            Spanned {
                token: t.token,
                range: start..end.max(start),
                line: t.span.start.line,
            }
        })
        .collect())
}

/// Maps the tokenizer's (line, column) locations to byte offsets.
struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    fn offset(&self, at: Location) -> usize {
        let Some(line) = usize::try_from(at.line).ok().and_then(|l| l.checked_sub(1)) else {
            return 0;
        };
        let Some(&start) = self.starts.get(line) else {
            return self.text.len();
        };
        let column = usize::try_from(at.column.saturating_sub(1)).unwrap_or(usize::MAX);
        self.text[start..]
            .char_indices()
            .nth(column)
            .map_or(self.text.len(), |(i, _)| start + i)
    }
}

/// Whether `token` is a literal value: a number or any kind of string.
pub fn is_literal(token: &Token) -> bool {
    matches!(
        token,
        Token::Number(..)
            | Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::TripleSingleQuotedString(_)
            | Token::TripleDoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::SingleQuotedByteStringLiteral(_)
            | Token::DoubleQuotedByteStringLiteral(_)
            | Token::TripleSingleQuotedByteStringLiteral(_)
            | Token::TripleDoubleQuotedByteStringLiteral(_)
            | Token::SingleQuotedRawStringLiteral(_)
            | Token::DoubleQuotedRawStringLiteral(_)
            | Token::TripleSingleQuotedRawStringLiteral(_)
            | Token::TripleDoubleQuotedRawStringLiteral(_)
            | Token::NationalStringLiteral(_)
            | Token::QuoteDelimitedStringLiteral(_)
            | Token::NationalQuoteDelimitedStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::UnicodeStringLiteral(_)
            | Token::HexStringLiteral(_)
    )
}

/// One statement of a file and the line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// The 1-based line of its first token.
    pub line: u64,
    /// Its text, without the terminating `;`.
    pub text: String,
}

/// Splits a file of statements on `;`, outside strings and comments.
///
/// A file the tokenizer refuses is returned as one statement starting at
/// line 1, so it is counted (as unparsed) rather than dropped.
pub fn split_statements(text: &str) -> Vec<Statement> {
    let Ok(tokens) = tokenize(text) else {
        return if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![Statement {
                line: 1,
                text: text.trim().to_owned(),
            }]
        };
    };
    let mut out = Vec::new();
    let mut start: Option<(usize, u64)> = None;
    let mut end = 0;
    let mut flush = |start: &mut Option<(usize, u64)>, end: usize| {
        if let Some((s, line)) = start.take() {
            out.push(Statement {
                line,
                text: text[s..end].trim().to_owned(),
            });
        }
    };
    for t in &tokens {
        match &t.token {
            Token::SemiColon => {
                flush(&mut start, end);
            }
            Token::Whitespace(_) => {}
            _ => {
                if start.is_none() {
                    start = Some((t.range.start, t.line));
                }
                end = t.range.end;
            }
        }
    }
    flush(&mut start, end);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_bytes_of_the_input() {
        let text = "SELECT 'é'\n, x";
        let tokens = tokenize(text).unwrap();
        for t in &tokens {
            assert!(text.get(t.range.clone()).is_some(), "{t:?}");
        }
        let x = tokens
            .iter()
            .find(|t| matches!(&t.token, Token::Word(w) if w.value == "x"));
        assert_eq!(&text[x.unwrap().range.clone()], "x");
        assert_eq!(x.unwrap().line, 2);
    }

    #[test]
    fn splits_outside_strings_and_comments() {
        let text =
            "-- a; comment\nCREATE TABLE a (s String DEFAULT ';');\n\n/* ; */ SELECT 1;\nSELECT 2";
        let statements = split_statements(text);
        let lines: Vec<_> = statements.iter().map(|s| s.line).collect();
        assert_eq!(lines, [2, 4, 5]);
        assert_eq!(statements[0].text, "CREATE TABLE a (s String DEFAULT ';')");
        assert_eq!(statements[1].text, "SELECT 1");
    }

    #[test]
    fn an_untokenizable_file_is_one_statement() {
        let statements = split_statements("SELECT 'unterminated");
        assert_eq!(statements.len(), 1);
    }
}
