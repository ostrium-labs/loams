//! SQL quoting for generated RisingWave DDL (RW-b assumption A1).
//!
//! RisingWave's frontend follows Postgres lexing: identifiers in double
//! quotes are case-sensitive and a `"` inside them is doubled; string
//! literals are in single quotes with a `'` doubled (standard-conforming
//! strings, no backslash escapes). Every identifier the builders emit goes
//! through [`quote_ident`] and every value through [`quote_literal`], so a
//! topic, table, user or password can hold any character without changing
//! the statement's shape.
//!
//! What cannot be quoted is checked instead: `WITH` option keys (bare, dotted
//! RisingWave option names such as `properties.bootstrap.server`) and column
//! data types (`INT`, `VARCHAR`, `STRUCT<a INT>`, `TIMESTAMPTZ[]`).

use super::DdlError;

/// Quotes one identifier: `orders` → `"orders"`, `we"ird` → `"we""ird"`.
///
/// Always quotes, so a name keeps its case and a keyword (`user`, `order`)
/// is never misread. An empty name or one with a NUL is refused.
pub fn quote_ident(name: &str) -> Result<String, DdlError> {
    if name.is_empty() {
        return Err(DdlError::EmptyIdent);
    }
    if name.contains('\0') {
        return Err(DdlError::Nul { what: "identifier" });
    }
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for ch in name.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
    Ok(out)
}

/// Quotes one string literal: `it's` → `'it''s'`.
///
/// Backslashes are left alone: RisingWave, like Postgres with
/// `standard_conforming_strings = on`, does not treat them as escapes in a
/// plain `'…'` literal. A NUL is refused (Postgres text cannot hold one).
pub fn quote_literal(value: &str) -> Result<String, DdlError> {
    if value.contains('\0') {
        return Err(DdlError::Nul { what: "literal" });
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push('\'');
        }
        out.push(ch);
    }
    out.push('\'');
    Ok(out)
}

/// Checks a `WITH (…)` or `FORMAT … ENCODE … (…)` option key.
///
/// Keys are emitted bare, so they are restricted to what RisingWave's option
/// names use: lower-case ASCII letters, digits, `_`, `.` and `-`, starting
/// with a letter, with no empty dotted part.
pub fn check_option_key(key: &str) -> Result<(), DdlError> {
    let bad = || DdlError::BadOptionKey(key.to_owned());
    let first = key.chars().next().ok_or_else(bad)?;
    if !first.is_ascii_lowercase() {
        return Err(bad());
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
    {
        return Err(bad());
    }
    if key.split('.').any(str::is_empty) {
        return Err(bad());
    }
    Ok(())
}

/// Checks a column data type, which is emitted as written.
///
/// Types are SQL syntax, not values, so they cannot be quoted. The check
/// admits what RisingWave's types are spelled with (`DOUBLE PRECISION`,
/// `NUMERIC(10, 2)`, `VARCHAR[]`, `STRUCT<a INT, b VARCHAR>`,
/// `MAP(VARCHAR, INT)`) and refuses anything that could end the statement or
/// open a comment or a literal: `;`, quotes, `--`, `/*`.
pub fn check_data_type(ty: &str) -> Result<(), DdlError> {
    let bad = || DdlError::BadDataType(ty.to_owned());
    let trimmed = ty.trim();
    if trimmed.is_empty() {
        return Err(bad());
    }
    if !trimmed.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | '_' | ',' | '(' | ')' | '[' | ']' | '<' | '>')
    }) {
        return Err(bad());
    }
    if trimmed.contains("--") {
        return Err(bad());
    }
    // Balanced brackets, so a type cannot close the column list early.
    let mut depth: i32 = 0;
    for c in trimmed.chars() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => {
                depth -= 1;
                if depth < 0 {
                    return Err(bad());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(bad());
    }
    Ok(())
}

/// Checks a column name that goes inside a literal list, such as a sink's
/// `primary_key = 'a,b'`: RisingWave splits that value on commas and trims
/// it, so a name there cannot hold a comma or edge whitespace.
pub fn check_list_member(name: &str) -> Result<(), DdlError> {
    if name.is_empty() || name.contains(',') || name.trim() != name || name.contains('\0') {
        return Err(DdlError::Invalid(format!(
            "{name:?} cannot be listed in a comma-separated option value"
        )));
    }
    Ok(())
}

/// Removes trailing whitespace and semicolons from a user-written query
/// (`AS <query>`), so the builder's own `;` ends the statement exactly once.
/// A semicolon left inside the query (a second statement) is refused. A query
/// whose last line ends in a `--` comment gets a newline, so the comment
/// cannot swallow the builder's `;`.
pub fn clean_query(query: &str) -> Result<String, DdlError> {
    let trimmed = query
        .trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    if trimmed.is_empty() {
        return Err(DdlError::Missing("query"));
    }
    let scan = scan(trimmed);
    if scan.top_level_semicolon {
        return Err(DdlError::Invalid(
            "the query holds more than one statement".to_owned(),
        ));
    }
    if scan.unterminated {
        return Err(DdlError::Invalid(
            "the query ends inside a quote or a block comment".to_owned(),
        ));
    }
    let mut out = trimmed.to_owned();
    if scan.ends_in_line_comment {
        out.push('\n');
    }
    Ok(out)
}

#[derive(Debug, Default)]
struct Scan {
    top_level_semicolon: bool,
    ends_in_line_comment: bool,
    unterminated: bool,
}

/// Walks `sql` past quotes, quoted identifiers, dollar quotes and comments.
fn scan(sql: &str) -> Scan {
    let bytes = sql.as_bytes();
    let mut out = Scan::default();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    out.unterminated = true;
                    return out;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                if i >= bytes.len() {
                    out.ends_in_line_comment = true;
                    return out;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 >= bytes.len() {
                    out.unterminated = true;
                    return out;
                }
                i += 1;
            }
            b'$' => {
                // $tag$ … $tag$ (the tag may be empty). A `$` that opens no
                // dollar quote (`$1`) is an ordinary character.
                let start = i;
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let is_quote = j < bytes.len()
                    && bytes[j] == b'$'
                    && !bytes[i + 1..j].first().is_some_and(u8::is_ascii_digit);
                if is_quote {
                    let tag = &sql[start..=j];
                    match sql[j + 1..].find(tag) {
                        Some(end) => i = j + 1 + end + tag.len() - 1,
                        None => {
                            out.unterminated = true;
                            return out;
                        }
                    }
                }
            }
            b';' => {
                out.top_level_semicolon = true;
                return out;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idents_are_always_quoted_and_quotes_doubled() {
        assert_eq!(quote_ident("orders").unwrap(), "\"orders\"");
        assert_eq!(quote_ident("Orders").unwrap(), "\"Orders\"");
        assert_eq!(quote_ident("user").unwrap(), "\"user\"");
        assert_eq!(quote_ident("we\"ird").unwrap(), "\"we\"\"ird\"");
        assert_eq!(quote_ident("a b.c").unwrap(), "\"a b.c\"");
        assert!(matches!(quote_ident(""), Err(DdlError::EmptyIdent)));
        assert!(matches!(quote_ident("a\0b"), Err(DdlError::Nul { .. })));
    }

    #[test]
    fn literals_double_single_quotes_and_keep_backslashes() {
        assert_eq!(quote_literal("x").unwrap(), "'x'");
        assert_eq!(quote_literal("").unwrap(), "''");
        assert_eq!(quote_literal("it's").unwrap(), "'it''s'");
        assert_eq!(quote_literal("a\\'b").unwrap(), "'a\\''b'");
        assert_eq!(
            quote_literal("'; DROP TABLE t; --").unwrap(),
            "'''; DROP TABLE t; --'"
        );
        assert!(matches!(quote_literal("\0"), Err(DdlError::Nul { .. })));
    }

    #[test]
    fn option_keys_are_restricted() {
        for ok in [
            "connector",
            "properties.bootstrap.server",
            "scan.startup.mode",
            "s3.path.style.access",
            "catalog.oauth2_server_uri",
            "jdbc.url",
            "schema.registry",
        ] {
            check_option_key(ok).unwrap();
        }
        for bad in [
            "",
            "Connector",
            "a..b",
            ".a",
            "a.",
            "a b",
            "a='x'",
            "1a",
            "a;b",
        ] {
            assert!(check_option_key(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn data_types_admit_risingwave_spellings_and_refuse_injection() {
        for ok in [
            "INT",
            "BIGINT",
            "DOUBLE PRECISION",
            "NUMERIC(10, 2)",
            "VARCHAR[]",
            "STRUCT<a INT, b VARCHAR>",
            "MAP(VARCHAR, INT)",
            "TIMESTAMP WITH TIME ZONE",
            "jsonb",
        ] {
            check_data_type(ok).unwrap();
        }
        for bad in [
            "",
            "INT); DROP TABLE t; --",
            "INT -- x",
            "VARCHAR DEFAULT 'x'",
            "INT)",
            "STRUCT<a INT",
            "INT /* */",
        ] {
            assert!(check_data_type(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn list_members_cannot_hold_commas() {
        check_list_member("id").unwrap();
        check_list_member("Order Id").unwrap();
        assert!(check_list_member("a,b").is_err());
        assert!(check_list_member(" a").is_err());
        assert!(check_list_member("").is_err());
    }

    #[test]
    fn queries_lose_trailing_semicolons_and_refuse_a_second_statement() {
        assert_eq!(clean_query("SELECT 1;  \n").unwrap(), "SELECT 1");
        assert_eq!(clean_query("SELECT ';' AS x;;").unwrap(), "SELECT ';' AS x");
        assert_eq!(
            clean_query("SELECT 1 -- note\n;").unwrap(),
            "SELECT 1 -- note\n"
        );
        assert_eq!(clean_query("SELECT 1 -- ;\n").unwrap(), "SELECT 1 --\n");
        assert_eq!(
            clean_query("SELECT \"a;b\" FROM t").unwrap(),
            "SELECT \"a;b\" FROM t"
        );
        assert_eq!(clean_query("SELECT $$;$$").unwrap(), "SELECT $$;$$");
        assert_eq!(clean_query("SELECT /* ; */ 1").unwrap(), "SELECT /* ; */ 1");
        assert!(clean_query("SELECT 1; DROP TABLE t").is_err());
        assert!(clean_query("  ;  ").is_err());
        assert!(clean_query("SELECT 'open").is_err());
        assert!(clean_query("SELECT 1 /* open").is_err());
        assert_eq!(clean_query("SELECT $1, $2").unwrap(), "SELECT $1, $2");
    }
}
