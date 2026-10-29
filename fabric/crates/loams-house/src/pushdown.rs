//! Key predicates pushed into a read view (BQ1-house, 2026-10-28).
//!
//! The House stores a `UInt64` column as Iceberg `decimal(20, 0)` (lossless
//! for other readers), and the read view restores it with `CAST(c, 'UInt64')`
//! ([`crate::views`]). chDB prunes Parquet row groups with a column's min/max
//! statistics only when the predicate is on the stored column itself: behind
//! any function, `toUInt64` and the `CAST` included, every row group is read
//! (measured on chDB 26.9: `WHERE id = 424242` read 16 of 16 row groups of a
//! 1M-row file through the view, 1 of 16 on the stored column). So a point
//! lookup on a `UInt64` key read the whole table.
//!
//! [`key_filters`] finds the statement's conjuncts that compare such a
//! column with integer literals, and the view repeats each one on the stored
//! column with a decimal literal (`__loams_src.id = toDecimal128('424242',
//! 0)`), which chDB prunes with. The user's own predicate still runs above the
//! view, so the view's filter only has to be implied by it, never equal to it.
//!
//! **When it applies.** Only where adding the filter cannot change an answer:
//! the statement is one `SELECT` with no subquery, `UNION`, `JOIN`, comma
//! join, `ARRAY JOIN`, `FINAL`, `SAMPLE` or `WITH`, reading this one table once
//! (the caller checks that the statement reads one table and that the view is
//! a plain one). Its `WHERE` / `PREWHERE` are split at the top level on `AND`;
//! an `OR`, `NOT` or `XOR` at the top level, or anything unrecognised, stops
//! the whole pushdown. Each conjunct of the forms below on a `UInt64` column
//! is pushed; other conjuncts are left to the statement:
//!
//! * `c = n`, `n = c`;
//! * `c IN (n, …)` (not `NOT IN`);
//! * `c > n`, `c >= n`, `c < n`, `c <= n` and the mirrored forms;
//! * `c BETWEEN n AND m` (not `NOT BETWEEN`),
//!
//! where `c` is the column's name, optionally qualified by the table's name or
//! alias, and `n` an unsigned decimal integer literal no larger than
//! `u64::MAX`. Query parameters, other literals and expressions are not
//! pushed.

use crate::classify::{Lexeme, lex};

/// The alias the view gives its source, so that a filter names the stored
/// column and not the view's `CAST` of the same name.
pub const SOURCE_ALIAS: &str = "__loams_src";

/// Words that end a `WHERE` / `PREWHERE` at the top level.
const CLAUSE_END: &[&str] = &[
    "GROUP",
    "ORDER",
    "LIMIT",
    "SETTINGS",
    "FORMAT",
    "HAVING",
    "WINDOW",
    "QUALIFY",
    "WHERE",
    "PREWHERE",
    "OFFSET",
    "INTO",
    "UNION",
    "INTERSECT",
    "EXCEPT",
];

/// Words that make a statement too complex to push into (anywhere in it).
const REFUSE_ANYWHERE: &[&str] = &[
    "JOIN",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "FINAL",
    "SAMPLE",
    "ARRAY",
    "WITH",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// A bare word or a quoted name (unquoted).
    Name(String, bool),
    /// An unsigned integer literal's digits.
    Int(String),
    /// A symbol: `(`, `)`, `,`, `.`, `=`, `<`, `>`, `<=`, `>=`, `!=`, …
    Sym(String),
    /// Anything else (a string literal, a float, a parameter).
    Other,
}

impl Tok {
    fn word(&self, w: &str) -> bool {
        matches!(self, Tok::Name(n, false) if n.eq_ignore_ascii_case(w))
    }

    fn sym(&self, s: &str) -> bool {
        matches!(self, Tok::Sym(x) if x == s)
    }
}

/// The statement's tokens, with multi-byte comparison operators joined and
/// quoted names unquoted.
fn tokens(sql: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    for (lexeme, range) in lex(sql) {
        let text = &sql[range.clone()];
        let tok = match lexeme {
            Lexeme::Word(..) => {
                if text.bytes().all(|b| b.is_ascii_digit()) {
                    Tok::Int(text.to_string())
                } else {
                    Tok::Name(text.to_string(), false)
                }
            }
            Lexeme::Semicolon => Tok::Sym(";".into()),
            Lexeme::Symbol(b) => {
                let s = (b as char).to_string();
                // `<=`, `>=`, `!=`, `<>`, `==`: the lexer gives one symbol each.
                if let Some(Tok::Sym(prev)) = out.last_mut()
                    && matches!(
                        (prev.as_str(), s.as_str()),
                        ("<", "=") | (">", "=") | ("!", "=") | ("<", ">") | ("=", "=")
                    )
                    && range.start > 0
                    && sql.as_bytes()[range.start - 1] == prev.as_bytes()[0]
                {
                    prev.push_str(&s);
                    continue;
                }
                Tok::Sym(s)
            }
            Lexeme::Other => {
                let quoted = text.len() >= 2
                    && ((text.starts_with('`') && text.ends_with('`'))
                        || (text.starts_with('"') && text.ends_with('"')));
                let inner = &text[1..text.len().saturating_sub(1).max(1)];
                if quoted && !inner.contains(['\\', '`', '"']) {
                    Tok::Name(inner.to_string(), true)
                } else {
                    Tok::Other
                }
            }
        };
        out.push(tok);
    }
    out
}

/// A pushable conjunct: the column and the condition on the stored column.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cond {
    Cmp(String, u64),
    In(Vec<u64>),
    Between(u64, u64),
}

fn int(tok: Option<&Tok>) -> Option<u64> {
    match tok {
        Some(Tok::Int(digits)) => digits.parse().ok(),
        _ => None,
    }
}

fn mirror(op: &str) -> &str {
    match op {
        ">" => "<",
        ">=" => "<=",
        "<" => ">",
        "<=" => ">=",
        other => other,
    }
}

/// A column reference at the start of `toks`: `c` or `q.c` with `q` one of
/// `qualifiers`; its name and length in tokens.
fn column(toks: &[Tok], qualifiers: &[String]) -> Option<(String, usize)> {
    match toks {
        [Tok::Name(q, _), dot, Tok::Name(c, _), ..]
            if dot.sym(".") && qualifiers.iter().any(|x| x == q) =>
        {
            Some((c.clone(), 3))
        }
        [Tok::Name(c, quoted), rest @ ..]
            if !rest.first().is_some_and(|t| t.sym(".") || t.sym("("))
                && (*quoted || !is_keyword(c)) =>
        {
            Some((c.clone(), 1))
        }
        _ => None,
    }
}

fn is_keyword(w: &str) -> bool {
    [
        "AND", "OR", "NOT", "IN", "BETWEEN", "IS", "NULL", "LIKE", "ILIKE", "GLOBAL", "XOR",
    ]
    .iter()
    .any(|k| w.eq_ignore_ascii_case(k))
}

/// One conjunct, if it is one of the pushable forms.
fn conjunct(toks: &[Tok], qualifiers: &[String]) -> Option<(String, Cond)> {
    // `c op n` / `c IN (…)` / `c BETWEEN n AND m`
    if let Some((c, used)) = column(toks, qualifiers) {
        let rest = &toks[used..];
        match rest {
            [Tok::Sym(op), Tok::Int(_)]
                if matches!(op.as_str(), "=" | "==" | "<" | "<=" | ">" | ">=") =>
            {
                let n = int(rest.get(1))?;
                let op = if op == "==" { "=" } else { op.as_str() };
                return Some((c, Cond::Cmp(op.to_string(), n)));
            }
            [inw, open, list @ .., close] if inw.word("IN") && open.sym("(") && close.sym(")") => {
                let mut values = Vec::new();
                for (i, t) in list.iter().enumerate() {
                    if i % 2 == 0 {
                        values.push(int(Some(t))?);
                    } else if !t.sym(",") {
                        return None;
                    }
                }
                if values.is_empty() || list.len() % 2 == 0 {
                    return None;
                }
                return Some((c, Cond::In(values)));
            }
            [b, Tok::Int(_), and, Tok::Int(_)] if b.word("BETWEEN") && and.word("AND") => {
                let lo = int(rest.get(1))?;
                let hi = int(rest.get(3))?;
                return Some((c, Cond::Between(lo, hi)));
            }
            _ => {}
        }
    }
    // `n op c`
    if let [Tok::Int(_), Tok::Sym(op), rest @ ..] = toks
        && matches!(op.as_str(), "=" | "==" | "<" | "<=" | ">" | ">=")
        && let Some((c, used)) = column(rest, qualifiers)
        && used == rest.len()
    {
        let n = int(toks.first())?;
        let op = if op == "==" { "=" } else { mirror(op) };
        return Some((c, Cond::Cmp(op.to_string(), n)));
    }
    None
}

fn decimal(n: u64) -> String {
    format!("toDecimal128('{n}', 0)")
}

/// The filters on the stored columns that the statement `sql`'s own `WHERE`
/// and `PREWHERE` imply, for the read view of table `database.name` (named in
/// `sql` as `name` or `database.name`, `current` being the statement's
/// database). `uint64` lists the table's `UInt64` columns stored as
/// `decimal(20, 0)`. Empty when nothing can be pushed safely.
pub fn key_filters(
    sql: &str,
    database: &str,
    name: &str,
    current: &str,
    uint64: &[&str],
) -> Vec<String> {
    if uint64.is_empty() {
        return Vec::new();
    }
    let toks = tokens(sql);
    // One statement, one SELECT, nothing that reads another relation.
    let selects = toks.iter().filter(|t| t.word("SELECT")).count();
    if selects != 1
        || !toks.first().is_some_and(|t| t.word("SELECT"))
        || toks.iter().any(|t| matches!(t, Tok::Sym(s) if s == ";"))
            && !toks.last().is_some_and(|t| t.sym(";"))
        || toks
            .iter()
            .any(|t| REFUSE_ANYWHERE.iter().any(|w| t.word(w)))
    {
        return Vec::new();
    }
    // Depth of every token.
    let mut depth = 0i32;
    let depths: Vec<i32> = toks
        .iter()
        .map(|t| {
            if t.sym("(") {
                depth += 1;
                depth - 1
            } else if t.sym(")") {
                depth -= 1;
                depth
            } else {
                depth
            }
        })
        .collect();
    let top = |i: usize| depths[i] == 0;
    // `FROM <table> [[AS] alias]`, exactly once at the top level.
    let froms: Vec<usize> = (0..toks.len())
        .filter(|&i| top(i) && toks[i].word("FROM"))
        .collect();
    let [from] = froms.as_slice() else {
        return Vec::new();
    };
    let mut i = from + 1;
    let (named_db, table) = match &toks[i..] {
        [Tok::Name(d, _), dot, Tok::Name(t, _), ..] if dot.sym(".") => {
            i += 3;
            (d.clone(), t.clone())
        }
        [Tok::Name(t, q), ..] if *q || !is_keyword(t) => {
            i += 1;
            (current.to_string(), t.clone())
        }
        _ => return Vec::new(),
    };
    if named_db != database || table != name {
        return Vec::new();
    }
    let mut qualifiers = vec![name.to_string()];
    if toks.get(i).is_some_and(|t| t.word("AS")) {
        i += 1;
    }
    if let Some(Tok::Name(alias, quoted)) = toks.get(i)
        && (*quoted
            || !(is_keyword(alias) || CLAUSE_END.iter().any(|w| alias.eq_ignore_ascii_case(w))))
    {
        qualifiers.push(alias.clone());
        i += 1;
    }
    // Nothing else may follow the table at the top level but clauses: a comma
    // or parenthesis there is a join or a table function.
    if toks
        .get(i)
        .is_some_and(|t| t.sym(",") || t.sym("(") || t.sym("."))
    {
        return Vec::new();
    }
    // The conjuncts of each top-level WHERE / PREWHERE.
    let mut conds: Vec<(String, Cond)> = Vec::new();
    let mut j = i;
    while j < toks.len() {
        if !(top(j) && (toks[j].word("WHERE") || toks[j].word("PREWHERE"))) {
            j += 1;
            continue;
        }
        let start = j + 1;
        let mut end = start;
        while end < toks.len()
            && !(top(end) && (CLAUSE_END.iter().any(|w| toks[end].word(w)) || toks[end].sym(";")))
        {
            end += 1;
        }
        // Split on top-level AND, keeping BETWEEN's own AND.
        let mut part = start;
        let mut between = false;
        for k in start..=end {
            let at_end = k == end;
            if !at_end && top(k) {
                let t = &toks[k];
                if t.word("OR") || t.word("NOT") || t.word("XOR") {
                    return Vec::new();
                }
                if t.word("BETWEEN") {
                    between = true;
                    continue;
                }
            }
            if at_end || (top(k) && toks[k].word("AND")) {
                if !at_end && between {
                    between = false;
                    continue;
                }
                let piece = &toks[part..k];
                // A parenthesised conjunct: look inside only if it is one
                // pushable form on its own.
                let piece = match piece {
                    [open, inner @ .., close]
                        if open.sym("(")
                            && close.sym(")")
                            && !inner.iter().any(|t| t.sym("(") || t.word("AND")) =>
                    {
                        inner
                    }
                    other => other,
                };
                if let Some((c, cond)) = conjunct(piece, &qualifiers)
                    && uint64.iter().any(|u| *u == c)
                {
                    conds.push((c, cond));
                }
                part = k + 1;
            }
        }
        j = end;
    }
    conds
        .into_iter()
        .map(|(c, cond)| {
            let col = format!("{SOURCE_ALIAS}.{}", crate::ddl::quote_name(&c));
            match cond {
                Cond::Cmp(op, n) => format!("{col} {op} {}", decimal(n)),
                Cond::In(values) => format!(
                    "{col} IN ({})",
                    values
                        .into_iter()
                        .map(decimal)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Cond::Between(lo, hi) => {
                    format!("{col} BETWEEN {} AND {}", decimal(lo), decimal(hi))
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(sql: &str) -> Vec<String> {
        key_filters(sql, "db", "kv", "db", &["id", "v"])
    }

    #[test]
    fn point_lookups_push_the_key() {
        assert_eq!(
            f("SELECT v, s FROM kv WHERE id = 424242"),
            vec!["__loams_src.`id` = toDecimal128('424242', 0)"]
        );
        assert_eq!(
            f("select * from db.kv as k prewhere k.id = 7 where 3 < v format JSON"),
            vec![
                "__loams_src.`id` = toDecimal128('7', 0)",
                "__loams_src.`v` > toDecimal128('3', 0)"
            ]
        );
        assert_eq!(
            f("SELECT count() FROM kv WHERE id IN (1, 2, 3) AND s = 'x' AND id BETWEEN 1 AND 9"),
            vec![
                "__loams_src.`id` IN (toDecimal128('1', 0), toDecimal128('2', 0), toDecimal128('3', 0))",
                "__loams_src.`id` BETWEEN toDecimal128('1', 0) AND toDecimal128('9', 0)"
            ]
        );
        assert_eq!(
            f("SELECT 1 FROM kv WHERE (id >= 10) AND `id` <= 20 ORDER BY id LIMIT 3"),
            vec![
                "__loams_src.`id` >= toDecimal128('10', 0)",
                "__loams_src.`id` <= toDecimal128('20', 0)"
            ]
        );
    }

    #[test]
    fn anything_else_pushes_nothing() {
        for sql in [
            "SELECT v FROM kv WHERE id = 1 OR id = 2",
            "SELECT v FROM kv WHERE NOT id = 1",
            "SELECT v FROM kv WHERE id NOT IN (1, 2)",
            "SELECT v FROM kv FINAL WHERE id = 1",
            "SELECT v FROM kv WHERE id = (SELECT 1)",
            "SELECT v FROM kv WHERE id IN (SELECT id FROM kv)",
            "SELECT v FROM kv JOIN other USING id WHERE id = 1",
            "SELECT v FROM kv, other WHERE id = 1",
            "SELECT v FROM other WHERE id = 1",
            "SELECT v FROM kv UNION ALL SELECT v FROM kv WHERE id = 1",
            "WITH 1 AS x SELECT v FROM kv WHERE id = x",
            "SELECT v FROM kv WHERE id = {p:UInt64}",
            "SELECT v FROM kv WHERE id = -1",
            "SELECT v FROM kv WHERE id = 1.5",
            "SELECT v FROM kv WHERE id = 18446744073709551616",
            "SELECT v FROM kv WHERE toUInt64(id) = 1",
            "SELECT v FROM numbers(3) WHERE id = 1",
        ] {
            assert!(f(sql).is_empty(), "{sql}: {:?}", f(sql));
        }
        // A conjunct on a column that is not UInt64 is not pushed, the
        // others are.
        assert_eq!(
            f("SELECT v FROM kv WHERE s = 'a' AND id = 5 AND t = 4"),
            vec!["__loams_src.`id` = toDecimal128('5', 0)"]
        );
        assert!(key_filters("SELECT v FROM kv WHERE id = 1", "db", "kv", "db", &[]).is_empty());
    }
}
