//! The lexical pre-pass (plan HE1 Task 22, step 1): ClickHouse forms that
//! sqlparser 0.62 misreads or refuses (HE0 R2.3, dialect spike §2.3) are
//! taken out of the text on tokens, and what they said is recorded in
//! [`Marks`] for the planner.
//!
//! * `FROM t [AS a] FINAL` — sqlparser reads `FINAL` as an alias, and fails
//!   after an alias: the token is removed and `(t, a)` marked final.
//! * `ANY`/`ALL`/`SEMI`/`ANTI`/`ASOF` before a join — removed, the join's
//!   right-hand relation marked with its strictness.
//! * `GLOBAL IN`, `GLOBAL NOT IN`, `GLOBAL … JOIN` — `GLOBAL` removed: one
//!   bucket holds every replica, so a global subquery is a local one.
//! * `t.1`, `(…).2` — tuple access by position, which sqlparser tokenizes as a
//!   name and a number `.1`: rewritten to `tupleElement(t, 1)`.
//! * A query's trailing `SETTINGS …` and `FORMAT …` (at bracket depth 0) —
//!   taken out and recorded: they are the statement's, for the front.
//!
//! * ClickHouse's name-resolution syntax (the ternary, scalar `WITH`, aliases
//!   inside expressions, `APPLY` and other matcher transformers) is rewritten
//!   first, by [`crate::rewrite`] (plan HE1 Task 25).
//!
//! The text is edited by byte ranges from sqlparser's own token spans, so
//! everything else (literals, comments, quoting) reaches the parser exactly as
//! the client wrote it.

use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::keywords::Keyword;
use datafusion::sql::sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};
use loams_house_types::ChError;

/// How a join matches rows, beyond `INNER`/`LEFT`/…: ClickHouse's strictness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// `ANY`: the first matching row of the right side.
    Any,
    /// `ALL` (the default): every matching row.
    All,
    /// `SEMI`.
    Semi,
    /// `ANTI`.
    Anti,
    /// `ASOF`.
    Asof,
}

/// A relation as the text names it: its name (as written, dots included) and
/// its alias, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRef {
    /// `db.t`, `t`, with quotes as written.
    pub name: String,
    /// The alias, unquoted.
    pub alias: Option<String>,
}

/// What the pre-pass took out of the text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    /// Relations read with `FINAL`.
    pub finals: Vec<RelationRef>,
    /// Joins with an explicit strictness, by their right-hand relation's
    /// first token (`t`, `(` for a subquery) and position in the text.
    pub strictness: Vec<(usize, Strictness)>,
    /// Whether `GLOBAL` was removed anywhere.
    pub global: bool,
    /// The query's own trailing `SETTINGS k = v, …`, taken out of the text
    /// (sqlparser reads `SELECT 1 SETTINGS …` as an alias).
    pub settings: Vec<(String, String)>,
    /// The query's trailing `FORMAT name`, taken out of the text.
    pub format: Option<String>,
}

/// The rewritten text and its marks.
#[derive(Debug, Clone)]
pub struct PrePass {
    /// The text for sqlparser.
    pub sql: String,
    /// What was taken out.
    pub marks: Marks,
}

struct Edit {
    start: usize,
    end: usize,
    with: String,
}

/// Runs the pre-pass over `sql`.
pub fn pre_pass(sql: &str) -> Result<PrePass, ChError> {
    let rewritten = crate::rewrite::rewrite(sql)?;
    let sql = rewritten.as_str();
    let dialect = ClickHouseDialect {};
    let tokens = Tokenizer::new(&dialect, sql)
        .tokenize_with_location()
        .map_err(|e| ChError::syntax_error(e.to_string()))?;
    let offsets = Offsets::new(sql);
    let byte = |t: &TokenWithSpan| -> (usize, usize) {
        (
            offsets.at(t.span.start.line, t.span.start.column),
            offsets.at(t.span.end.line, t.span.end.column),
        )
    };
    // Significant tokens, with their index in `tokens`.
    let sig: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| !matches!(t.token, Token::Whitespace(_) | Token::EOF))
        .map(|(i, _)| i)
        .collect();
    let kw = |i: usize| -> Option<Keyword> {
        match &tokens[sig[i]].token {
            Token::Word(w) if w.quote_style.is_none() => Some(w.keyword),
            _ => None,
        }
    };
    let mut edits: Vec<Edit> = Vec::new();
    let mut marks = Marks::default();
    for s in 0..sig.len() {
        let tok = &tokens[sig[s]];
        match kw(s) {
            Some(Keyword::FINAL) => {
                if let Some(rel) = relation_before(&tokens, &sig, s) {
                    let (start, end) = byte(tok);
                    edits.push(Edit {
                        start,
                        end,
                        with: String::new(),
                    });
                    marks.finals.push(rel);
                }
            }
            Some(Keyword::GLOBAL) => {
                let next = kw(s + 1);
                let joins = matches!(
                    next,
                    Some(
                        Keyword::IN
                            | Keyword::NOT
                            | Keyword::JOIN
                            | Keyword::LEFT
                            | Keyword::RIGHT
                            | Keyword::INNER
                            | Keyword::FULL
                            | Keyword::ANY
                            | Keyword::ALL
                            | Keyword::SEMI
                            | Keyword::ANTI
                            | Keyword::ASOF
                            | Keyword::CROSS
                    )
                );
                if joins {
                    let (start, end) = byte(tok);
                    edits.push(Edit {
                        start,
                        end,
                        with: String::new(),
                    });
                    marks.global = true;
                }
            }
            Some(
                k @ (Keyword::ANY | Keyword::ALL | Keyword::SEMI | Keyword::ANTI | Keyword::ASOF),
            ) => {
                // Strictness, then an optional kind and OUTER, then JOIN.
                let mut j = s + 1;
                while matches!(
                    kw(j),
                    Some(
                        Keyword::LEFT
                            | Keyword::RIGHT
                            | Keyword::INNER
                            | Keyword::FULL
                            | Keyword::OUTER
                    )
                ) {
                    j += 1;
                }
                if j < sig.len()
                    && kw(j) == Some(Keyword::JOIN)
                    && (j > s + 1 || !matches!(k, Keyword::SEMI | Keyword::ANTI))
                {
                    let strictness = match k {
                        Keyword::ANY => Strictness::Any,
                        Keyword::ALL => Strictness::All,
                        Keyword::SEMI => Strictness::Semi,
                        Keyword::ANTI => Strictness::Anti,
                        _ => Strictness::Asof,
                    };
                    // `LEFT SEMI JOIN` and `LEFT ANTI JOIN` parse as they are
                    // (sqlparser's semi and anti joins); others lose the word.
                    let (start, end) = byte(tok);
                    edits.push(Edit {
                        start,
                        end,
                        with: String::new(),
                    });
                    let right = sig
                        .get(j + 1)
                        .map(|&i| byte(&tokens[i]).0)
                        .unwrap_or(sql.len());
                    marks.strictness.push((right, strictness));
                }
            }
            _ => {}
        }
        // `x.1`: a name or a closing bracket, then a number token `.N`.
        if let Token::Number(n, _) = &tok.token
            && let Some(digits) = n.strip_prefix('.')
            && !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit())
            && s > 0
        {
            let (_, num_end) = byte(tok);
            let num_start = byte(tok).0;
            let prev = sig[s - 1];
            let adjacent = byte(&tokens[prev]).1 == num_start;
            if adjacent {
                let target_start = match &tokens[prev].token {
                    Token::Word(_) => Some(name_start(&tokens, &sig, s - 1, &byte)),
                    Token::RParen | Token::RBracket => {
                        matching_open(&tokens, &sig, s - 1).map(|o| byte(&tokens[sig[o]]).0)
                    }
                    _ => None,
                };
                if let Some(target_start) = target_start {
                    let target = &sql[target_start..num_start];
                    edits.retain(|e| e.end <= target_start || e.start >= num_end);
                    edits.push(Edit {
                        start: target_start,
                        end: num_end,
                        with: format!("tupleElement({target}, {digits})"),
                    });
                }
            }
        }
    }
    trailing_clauses(sql, &tokens, &sig, &byte, &mut edits, &mut marks)?;
    edits.sort_by_key(|e| e.start);
    let mut out = String::with_capacity(sql.len());
    let mut at = 0;
    for e in &edits {
        if e.start < at {
            continue;
        }
        out.push_str(&sql[at..e.start]);
        out.push_str(&e.with);
        at = e.end;
    }
    out.push_str(&sql[at..]);
    Ok(PrePass { sql: out, marks })
}

/// Takes a query's depth-0 `SETTINGS` and `FORMAT` clauses out of the text.
fn trailing_clauses(
    sql: &str,
    tokens: &[TokenWithSpan],
    sig: &[usize],
    byte: &impl Fn(&TokenWithSpan) -> (usize, usize),
    edits: &mut Vec<Edit>,
    marks: &mut Marks,
) -> Result<(), ChError> {
    let kw = |i: usize| match &tokens[sig[i]].token {
        Token::Word(w) if w.quote_style.is_none() => Some(w.keyword),
        _ => None,
    };
    if !matches!(
        sig.first().map(|_| kw(0)),
        Some(Some(
            Keyword::SELECT
                | Keyword::WITH
                // Metadata statements carry them too (plan HE1 Task 38).
                | Keyword::SHOW
                | Keyword::DESCRIBE
                | Keyword::DESC
                | Keyword::EXISTS
        ))
    ) && !matches!(sig.first().map(|&i| &tokens[i].token), Some(Token::LParen))
    {
        return Ok(());
    }
    let mut depth = 0i32;
    let mut clauses = Vec::new();
    for (s, &i) in sig.iter().enumerate() {
        match tokens[i].token {
            Token::LParen | Token::LBracket => depth += 1,
            Token::RParen | Token::RBracket => depth -= 1,
            _ => {}
        }
        if depth == 0 && matches!(kw(s), Some(Keyword::SETTINGS | Keyword::FORMAT)) {
            clauses.push(s);
        }
    }
    for (n, &start) in clauses.iter().enumerate() {
        let end = clauses.get(n + 1).copied().unwrap_or(sig.len());
        // A trailing `;` is not part of the clause.
        let end = if end == sig.len()
            && end > start + 1
            && matches!(tokens[sig[end - 1]].token, Token::SemiColon)
        {
            end - 1
        } else {
            end
        };
        let from = byte(&tokens[sig[start]]).0;
        let to = byte(&tokens[sig[end - 1]]).1;
        if kw(start) == Some(Keyword::FORMAT) {
            match (
                end - start,
                tokens
                    .get(sig[start + 1.min(end - start - 1)])
                    .map(|t| &t.token),
            ) {
                (2, Some(Token::Word(w))) => marks.format = Some(w.value.clone()),
                _ => return Err(ChError::syntax_error("Expected a format name after FORMAT")),
            }
        } else {
            let mut s = start + 1;
            while s < end {
                let key = match &tokens[sig[s]].token {
                    Token::Word(w) => w.value.clone(),
                    _ => return Err(ChError::syntax_error("Expected a setting name in SETTINGS")),
                };
                if !matches!(
                    tokens
                        .get(sig.get(s + 1).copied().unwrap_or(usize::MAX))
                        .map(|t| &t.token),
                    Some(Token::Eq)
                ) {
                    return Err(ChError::syntax_error(format!(
                        "Expected = after the setting {key}"
                    )));
                }
                let v0 = s + 2;
                let mut v1 = v0;
                let mut d = 0i32;
                while v1 < end {
                    match tokens[sig[v1]].token {
                        Token::LParen | Token::LBracket => d += 1,
                        Token::RParen | Token::RBracket => d -= 1,
                        Token::Comma if d == 0 => break,
                        _ => {}
                    }
                    v1 += 1;
                }
                if v1 == v0 {
                    return Err(ChError::syntax_error(format!(
                        "Expected a value for the setting {key}"
                    )));
                }
                let value = if v1 == v0 + 1
                    && let Token::SingleQuotedString(v) = &tokens[sig[v0]].token
                {
                    v.clone()
                } else {
                    sql[byte(&tokens[sig[v0]]).0..byte(&tokens[sig[v1 - 1]]).1]
                        .trim()
                        .to_string()
                };
                marks.settings.push((key, value));
                s = v1 + 1;
            }
        }
        edits.push(Edit {
            start: from,
            end: to,
            with: String::new(),
        });
    }
    Ok(())
}

/// The start of a dotted name ending at significant token `s` (`db.t`).
fn name_start(
    tokens: &[TokenWithSpan],
    sig: &[usize],
    mut s: usize,
    byte: &impl Fn(&TokenWithSpan) -> (usize, usize),
) -> usize {
    while s >= 2
        && matches!(tokens[sig[s - 1]].token, Token::Period)
        && matches!(tokens[sig[s - 2]].token, Token::Word(_))
    {
        s -= 2;
    }
    byte(&tokens[sig[s]]).0
}

/// The significant index of the bracket that `close` closes.
fn matching_open(tokens: &[TokenWithSpan], sig: &[usize], close: usize) -> Option<usize> {
    let mut depth = 0i32;
    for s in (0..=close).rev() {
        match tokens[sig[s]].token {
            Token::RParen | Token::RBracket => depth += 1,
            Token::LParen | Token::LBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// The relation before a `FINAL` at significant index `s`:
/// `<from|join|,> name [[AS] alias] FINAL`.
fn relation_before(tokens: &[TokenWithSpan], sig: &[usize], s: usize) -> Option<RelationRef> {
    let word = |i: usize| match &tokens[sig[i]].token {
        Token::Word(w) => Some(w),
        _ => None,
    };
    let is_kw =
        |i: usize, k: Keyword| word(i).is_some_and(|w| w.quote_style.is_none() && w.keyword == k);
    if s == 0 {
        return None;
    }
    // Parse backwards: alias?, AS?, name (Word ( . Word )*).
    let mut i = s - 1;
    word(i)?;
    let mut alias = None;
    let name_end;
    // Is token i the end of a dotted name preceded by FROM/JOIN/','?
    let name_from = |end: usize| -> Option<usize> {
        let mut st = end;
        while st >= 2
            && matches!(tokens[sig[st - 1]].token, Token::Period)
            && word(st - 2).is_some()
        {
            st -= 2;
        }
        if st == 0 {
            return None;
        }
        let before = st - 1;
        let ok = is_kw(before, Keyword::FROM)
            || is_kw(before, Keyword::JOIN)
            || matches!(tokens[sig[before]].token, Token::Comma);
        ok.then_some(st)
    };
    if let Some(start) = name_from(i) {
        name_end = i;
        return Some(RelationRef {
            name: join_name(tokens, sig, start, name_end),
            alias,
        });
    }
    // `name alias FINAL` or `name AS alias FINAL`.
    alias = word(i).map(|w| w.value.clone());
    if i == 0 {
        return None;
    }
    i -= 1;
    if is_kw(i, Keyword::AS) {
        if i == 0 {
            return None;
        }
        i -= 1;
    }
    word(i)?;
    let start = name_from(i)?;
    Some(RelationRef {
        name: join_name(tokens, sig, start, i),
        alias,
    })
}

fn join_name(tokens: &[TokenWithSpan], sig: &[usize], start: usize, end: usize) -> String {
    (start..=end)
        .map(|i| tokens[sig[i]].token.to_string())
        .collect()
}

/// Byte offsets of sqlparser's (line, column) locations: columns count
/// characters from 1.
pub(crate) struct Offsets {
    /// For each line, the byte offset of each character, plus the end.
    lines: Vec<Vec<usize>>,
}

impl Offsets {
    pub(crate) fn new(sql: &str) -> Self {
        let mut lines = vec![Vec::new()];
        for (i, c) in sql.char_indices() {
            lines.last_mut().expect("a line").push(i);
            if c == '\n' {
                lines.push(Vec::new());
            }
        }
        lines.last_mut().expect("a line").push(sql.len());
        // Each line's end offset is the next line's start.
        let n = lines.len();
        for l in 0..n - 1 {
            let next = lines[l + 1].first().copied().unwrap_or(sql.len());
            lines[l].push(next);
        }
        Self { lines }
    }

    pub(crate) fn at(&self, line: u64, column: u64) -> usize {
        let line = &self.lines[(line.max(1) - 1) as usize];
        line[((column.max(1) - 1) as usize).min(line.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sql: &str) -> PrePass {
        pre_pass(sql).unwrap()
    }

    #[test]
    fn final_is_taken_out_and_marked() {
        let p = run("SELECT * FROM db.t FINAL WHERE x = 1");
        assert_eq!(p.sql, "SELECT * FROM db.t  WHERE x = 1");
        assert_eq!(
            p.marks.finals,
            vec![RelationRef {
                name: "db.t".into(),
                alias: None
            }]
        );
        let p = run("SELECT * FROM t AS a FINAL JOIN u b FINAL ON a.k = b.k");
        assert_eq!(p.marks.finals.len(), 2);
        assert_eq!(
            p.marks.finals[0],
            RelationRef {
                name: "t".into(),
                alias: Some("a".into())
            }
        );
        assert_eq!(
            p.marks.finals[1],
            RelationRef {
                name: "u".into(),
                alias: Some("b".into())
            }
        );
        assert!(!p.sql.to_uppercase().contains("FINAL"));
    }

    #[test]
    fn final_as_a_quoted_alias_stays() {
        let p = run("SELECT * FROM t AS `FINAL`");
        assert!(p.marks.finals.is_empty());
        assert_eq!(p.sql, "SELECT * FROM t AS `FINAL`");
    }

    #[test]
    fn strictness_and_global_are_taken_out() {
        let p =
            run("SELECT * FROM a GLOBAL ANY LEFT JOIN b USING (k) WHERE x GLOBAL IN (SELECT 1)");
        assert_eq!(
            p.sql,
            "SELECT * FROM a   LEFT JOIN b USING (k) WHERE x  IN (SELECT 1)"
        );
        assert_eq!(p.marks.strictness.len(), 1);
        assert_eq!(p.marks.strictness[0].1, Strictness::Any);
        assert!(p.marks.global);
    }

    #[test]
    fn tuple_access_becomes_tuple_element() {
        assert_eq!(
            run("SELECT t.1, (1, 2).2 FROM x").sql,
            "SELECT tupleElement(t, 1), tupleElement((1, 2), 2) FROM x"
        );
        assert_eq!(run("SELECT 1.5, t.a FROM x").sql, "SELECT 1.5, t.a FROM x");
    }

    #[test]
    fn literals_and_comments_are_untouched() {
        let sql = "SELECT 'a FINAL', 'x\\\\y' -- FINAL\nFROM t";
        assert_eq!(run(sql).sql, sql);
    }

    #[test]
    fn trailing_settings_and_format_are_the_statements() {
        let p = run("SELECT 1 SETTINGS max_threads = 2, s = 'a,b' FORMAT JSONEachRow");
        assert_eq!(p.sql.trim(), "SELECT 1");
        assert_eq!(
            p.marks.settings,
            vec![
                ("max_threads".into(), "2".into()),
                ("s".into(), "a,b".into())
            ]
        );
        assert_eq!(p.marks.format.as_deref(), Some("JSONEachRow"));
        let p = run("SELECT x FROM (SELECT 1 AS x SETTINGS a = 1) FORMAT TSV;");
        assert_eq!(p.marks.settings, vec![]);
        assert_eq!(p.marks.format.as_deref(), Some("TSV"));
    }

    #[test]
    fn non_ascii_text_keeps_its_offsets() {
        let p = run("SELECT 'é' FROM t FINAL");
        assert_eq!(p.sql, "SELECT 'é' FROM t ");
    }
}
