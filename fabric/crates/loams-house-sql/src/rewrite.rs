//! Text rewrites for ClickHouse's name-resolution syntax that sqlparser 0.62
//! does not read (plan HE1 Task 25, R25.1). They run at the start of the
//! lexical pre-pass ([`crate::lex::pre_pass`]), each on sqlparser's own
//! tokens, editing the text by byte ranges so everything else reaches the
//! parser exactly as written:
//!
//! * the ternary `c ? a : b` becomes `if(c, a, b)`;
//! * a scalar `WITH expr AS name` becomes the marker CTE
//!   `name AS (SELECT expr AS __ch_with)`, which [`crate::scope`] takes out
//!   again and substitutes;
//! * an alias inside an expression, `(expr AS name)` or `f(expr AS name)`,
//!   becomes `__ch_alias(expr, name)`;
//! * a matcher (`*`, `t.*`, `COLUMNS(…)`) followed by transformers that
//!   sqlparser cannot read (`APPLY`, transformers after `COLUMNS`, `EXCEPT
//!   STRICT`, or `EXCEPT`/`REPLACE` out of sqlparser's order) becomes one
//!   `REPLACE (…)` list of markers, in order:
//!   `__ch_columns(args) AS __ch_matcher`, `__ch_except AS c`,
//!   `__ch_except_strict AS c`, `__ch_except('re') AS __ch_re`,
//!   `__ch_apply AS f`, `__ch_apply(params) AS f`,
//!   `__ch_apply(x -> …) AS __ch_lambda`, and `e AS c` for a replacement.

use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::keywords::Keyword;
use datafusion::sql::sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};
use loams_house_types::ChError;

use crate::lex::Offsets;

/// The rewrites, in order.
pub fn rewrite(sql: &str) -> Result<String, ChError> {
    let sql = ternary(sql)?;
    let sql = scalar_with(&sql)?;
    let sql = matchers(&sql)?;
    inline_aliases(&sql)
}

/// A statement's significant tokens with their byte ranges and bracket depth.
struct Toks<'s> {
    sql: &'s str,
    tokens: Vec<TokenWithSpan>,
    /// Significant token `i`'s index in `tokens`.
    sig: Vec<usize>,
    start: Vec<usize>,
    end: Vec<usize>,
    /// Bracket depth: an opening bracket has the depth outside it, its
    /// contents one more, its closing bracket the depth outside again.
    depth: Vec<i32>,
}

impl<'s> Toks<'s> {
    fn new(sql: &'s str) -> Result<Self, ChError> {
        let tokens = Tokenizer::new(&ClickHouseDialect {}, sql)
            .tokenize_with_location()
            .map_err(|e| ChError::syntax_error(e.to_string()))?;
        let offsets = Offsets::new(sql);
        let sig: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| !matches!(t.token, Token::Whitespace(_) | Token::EOF))
            .map(|(i, _)| i)
            .collect();
        let mut start = Vec::with_capacity(sig.len());
        let mut end = Vec::with_capacity(sig.len());
        let mut depth = Vec::with_capacity(sig.len());
        let mut d = 0;
        for &i in &sig {
            let t = &tokens[i];
            start.push(offsets.at(t.span.start.line, t.span.start.column));
            end.push(offsets.at(t.span.end.line, t.span.end.column));
            match t.token {
                Token::LParen | Token::LBracket | Token::LBrace => {
                    depth.push(d);
                    d += 1;
                }
                Token::RParen | Token::RBracket | Token::RBrace => {
                    d -= 1;
                    depth.push(d);
                }
                _ => depth.push(d),
            }
        }
        Ok(Self {
            sql,
            tokens,
            sig,
            start,
            end,
            depth,
        })
    }

    fn len(&self) -> usize {
        self.sig.len()
    }

    fn tok(&self, i: usize) -> &Token {
        &self.tokens[self.sig[i]].token
    }

    /// The keyword of an unquoted word.
    fn kw(&self, i: usize) -> Option<Keyword> {
        match self.tok(i) {
            Token::Word(w) if w.quote_style.is_none() => Some(w.keyword),
            _ => None,
        }
    }

    /// Whether token `i` is the unquoted word `name` (any case).
    fn is_word(&self, i: usize, name: &str) -> bool {
        i < self.len()
            && matches!(self.tok(i), Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(name))
    }

    fn is_ident(&self, i: usize) -> bool {
        i < self.len() && matches!(self.tok(i), Token::Word(_))
    }

    fn text(&self, from: usize, to: usize) -> &'s str {
        self.sql[from..to].trim()
    }

    /// The text of significant tokens `a..=b`.
    fn span_text(&self, a: usize, b: usize) -> &'s str {
        self.text(self.start[a], self.end[b])
    }

    /// The significant index of the bracket closing the one opened at `open`.
    fn closing(&self, open: usize) -> Option<usize> {
        let d = self.depth[open];
        (open + 1..self.len()).find(|&j| self.depth[j] == d)
    }

    fn is_question(&self, i: usize) -> bool {
        matches!(self.tok(i), Token::Placeholder(p) if p.starts_with('?'))
    }
}

struct Edit {
    start: usize,
    end: usize,
    with: String,
}

/// Applies non-overlapping edits (the first of overlapping ones wins).
fn apply(sql: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| e.start);
    let mut out = String::with_capacity(sql.len() + 32);
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
    out
}

fn syntax(what: &str) -> ChError {
    ChError::syntax_error(format!("Syntax error: {what}"))
}

/// Keywords that end a ternary's condition when read backwards from `?`.
fn ends_condition(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::SELECT
            | Keyword::DISTINCT
            | Keyword::WHERE
            | Keyword::PREWHERE
            | Keyword::HAVING
            | Keyword::ON
            | Keyword::BY
            | Keyword::WHEN
            | Keyword::THEN
            | Keyword::ELSE
            | Keyword::QUALIFY
            | Keyword::LIMIT
            | Keyword::OFFSET
            | Keyword::WITH
            | Keyword::SETTINGS
            | Keyword::RETURN
    )
}

/// Keywords that end a ternary's last operand.
fn ends_operand(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::FROM
            | Keyword::WHERE
            | Keyword::PREWHERE
            | Keyword::GROUP
            | Keyword::ORDER
            | Keyword::HAVING
            | Keyword::LIMIT
            | Keyword::OFFSET
            | Keyword::SETTINGS
            | Keyword::FORMAT
            | Keyword::UNION
            | Keyword::EXCEPT
            | Keyword::INTERSECT
            | Keyword::AS
            | Keyword::ASC
            | Keyword::DESC
            | Keyword::NULLS
            | Keyword::WITH
            | Keyword::THEN
            | Keyword::ELSE
            | Keyword::END
            | Keyword::WHEN
            | Keyword::JOIN
            | Keyword::INNER
            | Keyword::LEFT
            | Keyword::RIGHT
            | Keyword::FULL
            | Keyword::CROSS
            | Keyword::ARRAY
            | Keyword::ON
            | Keyword::USING
            | Keyword::WINDOW
            | Keyword::QUALIFY
            | Keyword::INTO
            | Keyword::COLLATE
            | Keyword::GLOBAL
            | Keyword::ANY
            | Keyword::SEMI
            | Keyword::ANTI
            | Keyword::ASOF
            | Keyword::BY
            | Keyword::SELECT
            | Keyword::FINAL
            | Keyword::SAMPLE
    )
}

/// `c ? a : b` → `if(c, a, b)`, the rightmost first. As in ClickHouse, the
/// ternary binds loosest (`NOT 1 ? 2 : 3` is `if(not(1), 2, 3)`), and a
/// ternary directly inside another's operand without brackets is a syntax
/// error (measured on the oracle).
pub fn ternary(sql: &str) -> Result<String, ChError> {
    let mut sql = sql.to_string();
    for _ in 0..4096 {
        let t = Toks::new(&sql)?;
        let Some(q) = (0..t.len()).rev().find(|&i| t.is_question(i)) else {
            return Ok(sql);
        };
        let d = t.depth[q];
        // The condition, read backwards.
        let mut first = q;
        while first > 0 {
            let j = first - 1;
            if t.depth[j] < d {
                break;
            }
            if t.depth[j] == d {
                match t.tok(j) {
                    Token::Comma | Token::SemiColon | Token::Arrow => break,
                    Token::Colon => {
                        return Err(syntax("a ternary inside a ternary needs brackets"));
                    }
                    _ if t.is_question(j) => {
                        return Err(syntax("a ternary inside a ternary needs brackets"));
                    }
                    _ => {}
                }
                if let Some(k) = t.kw(j) {
                    if ends_condition(k) {
                        break;
                    }
                    if k == Keyword::END {
                        // Back over `CASE … END`.
                        let mut nest = 0;
                        let mut c = j;
                        loop {
                            if t.depth[c] == d {
                                match t.kw(c) {
                                    Some(Keyword::END) => nest += 1,
                                    Some(Keyword::CASE) => {
                                        nest -= 1;
                                        if nest == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if c == 0 {
                                return Err(syntax("END without CASE"));
                            }
                            c -= 1;
                        }
                        first = c;
                        continue;
                    }
                }
            }
            first = j;
        }
        if first == q {
            return Err(syntax("a ternary without a condition"));
        }
        // The colon.
        let colon = (q + 1..t.len())
            .take_while(|&j| t.depth[j] >= d)
            .find(|&j| t.depth[j] == d && matches!(t.tok(j), Token::Colon))
            .ok_or_else(|| syntax("expected : in a ternary"))?;
        // The last operand.
        let mut last = colon;
        let mut j = colon + 1;
        while j < t.len() {
            if t.depth[j] < d {
                break;
            }
            if t.depth[j] == d {
                match t.tok(j) {
                    Token::Comma | Token::SemiColon => break,
                    Token::Colon => return Err(syntax("unexpected : after a ternary")),
                    _ if t.is_question(j) => {
                        return Err(syntax("a ternary inside a ternary needs brackets"));
                    }
                    _ => {}
                }
                match t.kw(j) {
                    Some(Keyword::CASE) => {
                        // Over `CASE … END`.
                        let mut nest = 0;
                        while j < t.len() {
                            if t.depth[j] == d {
                                match t.kw(j) {
                                    Some(Keyword::CASE) => nest += 1,
                                    Some(Keyword::END) => {
                                        nest -= 1;
                                        if nest == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            j += 1;
                        }
                        last = j.min(t.len() - 1);
                        j += 1;
                        continue;
                    }
                    Some(k) if ends_operand(k) => break,
                    _ => {}
                }
            }
            last = j;
            j += 1;
        }
        if last == colon {
            return Err(syntax("a ternary without its last operand"));
        }
        let cond = t.text(t.start[first], t.start[q]);
        // `?1` is one token: the operand starts after the `?`.
        let then = t.text(t.start[q] + 1, t.start[colon]);
        let other = t.text(t.end[colon], t.end[last]);
        if then.is_empty() {
            return Err(syntax("a ternary without its middle operand"));
        }
        let with = format!("if({cond}, {then}, {other})");
        sql = apply(
            &sql,
            vec![Edit {
                start: t.start[first],
                end: t.end[last],
                with,
            }],
        );
    }
    Err(syntax("too many ternaries"))
}

/// `WITH expr AS name` → `WITH name AS (SELECT expr AS __ch_with)`.
pub fn scalar_with(sql: &str) -> Result<String, ChError> {
    let t = Toks::new(sql)?;
    let mut edits = Vec::new();
    for s in 0..t.len() {
        if t.kw(s) != Some(Keyword::WITH) {
            continue;
        }
        if ["FILL", "TOTALS", "ROLLUP", "CUBE", "TIES", "RECURSIVE"]
            .iter()
            .any(|w| t.is_word(s + 1, w))
        {
            continue;
        }
        let starts_query = s == 0
            || matches!(t.tok(s - 1), Token::LParen)
            || matches!(
                t.kw(s - 1),
                Some(
                    Keyword::UNION
                        | Keyword::ALL
                        | Keyword::DISTINCT
                        | Keyword::EXCEPT
                        | Keyword::INTERSECT
                )
            );
        if !starts_query {
            continue;
        }
        let d = t.depth[s];
        let mut entries = Vec::new();
        let mut a = s + 1;
        let mut j = s + 1;
        while j < t.len() && t.depth[j] >= d {
            if t.depth[j] == d {
                if t.kw(j) == Some(Keyword::SELECT) {
                    break;
                }
                if matches!(t.tok(j), Token::Comma) {
                    entries.push((a, j));
                    a = j + 1;
                }
            }
            j += 1;
        }
        if j < t.len() && t.kw(j) == Some(Keyword::SELECT) {
            entries.push((a, j));
        }
        for (a, b) in entries {
            if b < a + 3 {
                continue;
            }
            // `name AS (SELECT …)`: a CTE.
            if t.is_ident(a)
                && t.kw(a + 1) == Some(Keyword::AS)
                && matches!(t.tok(a + 2), Token::LParen)
            {
                continue;
            }
            if t.kw(b - 2) == Some(Keyword::AS) && t.is_ident(b - 1) {
                let expr = t.text(t.start[a], t.start[b - 2]);
                let name = t.span_text(b - 1, b - 1);
                edits.push(Edit {
                    start: t.start[a],
                    end: t.end[b - 1],
                    with: format!("{name} AS (SELECT {expr} AS __ch_with)"),
                });
            }
        }
    }
    Ok(apply(sql, edits))
}

/// `(expr AS name)` and `f(…, expr AS name, …)` → `__ch_alias(expr, name)`,
/// the deepest first.
pub fn inline_aliases(sql: &str) -> Result<String, ChError> {
    let mut sql = sql.to_string();
    for _ in 0..1024 {
        let t = Toks::new(&sql)?;
        let mut best: Option<(i32, Edit)> = None;
        for s in 1..t.len().saturating_sub(2) {
            if t.kw(s) != Some(Keyword::AS) || t.depth[s] == 0 || !t.is_ident(s + 1) {
                continue;
            }
            let d = t.depth[s];
            let after = &t.tok(s + 2);
            if !matches!(after, Token::RParen | Token::Comma) || t.depth[s + 2] < d - 1 {
                continue;
            }
            // The segment's start and the enclosing bracket.
            let mut seg = None;
            let mut open = None;
            let mut skip = false;
            let mut j = s;
            while j > 0 {
                j -= 1;
                if t.depth[j] < d {
                    open = Some(j);
                    break;
                }
                if t.depth[j] == d {
                    if matches!(t.tok(j), Token::Comma) && seg.is_none() {
                        seg = Some(j + 1);
                    }
                    if matches!(
                        t.kw(j),
                        Some(Keyword::SELECT | Keyword::WITH | Keyword::FROM | Keyword::JOIN)
                    ) {
                        skip = true;
                    }
                }
            }
            let Some(open) = open else { continue };
            if skip || !matches!(t.tok(open), Token::LParen) {
                continue;
            }
            let seg = seg.unwrap_or(open + 1);
            if seg >= s {
                continue;
            }
            if open > 0 {
                let before = open - 1;
                let excluded = [
                    "CAST",
                    "TRY_CAST",
                    "SAFE_CAST",
                    "REPLACE",
                    "APPLY",
                    "EXCEPT",
                    "OVER",
                ]
                .iter()
                .any(|w| t.is_word(before, w))
                    || matches!(
                        t.kw(before),
                        Some(
                            Keyword::FROM
                                | Keyword::JOIN
                                | Keyword::AS
                                | Keyword::USING
                                | Keyword::VALUES
                                | Keyword::IN
                        )
                    );
                if excluded {
                    continue;
                }
            }
            let expr = t.text(t.start[seg], t.start[s]);
            let name = t.span_text(s + 1, s + 1);
            let edit = Edit {
                start: t.start[seg],
                end: t.end[s + 1],
                with: format!("__ch_alias({expr}, {name})"),
            };
            if best.as_ref().is_none_or(|(bd, _)| d > *bd) {
                best = Some((d, edit));
            }
        }
        match best {
            None => return Ok(sql),
            Some((_, edit)) => sql = apply(&sql, vec![edit]),
        }
    }
    Err(syntax("too many aliases inside expressions"))
}

/// One transformer after a matcher.
enum Transformer {
    /// `EXCEPT [STRICT] (a, 'regexp', …)`: each item's text as written.
    Except { strict: bool, items: Vec<String> },
    /// `REPLACE (e AS c, …)`: the elements' text.
    Replace(Vec<String>),
    /// `APPLY f`, `APPLY(f)`, `APPLY(f(params))`, `APPLY(x -> …)`.
    Apply(String),
}

/// Reads the transformers after significant index `from`; returns them and
/// the last index they cover.
fn transformers(t: &Toks<'_>, from: usize) -> Result<(Vec<Transformer>, usize), ChError> {
    let mut out = Vec::new();
    let mut i = from;
    let mut last = from - 1;
    loop {
        if t.is_word(i, "EXCEPT") {
            // `* EXCEPT SELECT …` would be a set operation, never a matcher's.
            let mut j = i + 1;
            let strict = t.is_word(j, "STRICT");
            if strict {
                j += 1;
            }
            if j >= t.len() || t.kw(j) == Some(Keyword::SELECT) {
                break;
            }
            let (items, end) = if matches!(t.tok(j), Token::LParen) {
                let close = t
                    .closing(j)
                    .ok_or_else(|| syntax("unclosed EXCEPT ( … )"))?;
                if j + 1 < close && t.kw(j + 1) == Some(Keyword::SELECT) {
                    break;
                }
                let items = (j + 1..close)
                    .filter(|&k| !matches!(t.tok(k), Token::Comma))
                    .map(|k| t.span_text(k, k).to_string())
                    .collect();
                (items, close)
            } else {
                (vec![t.span_text(j, j).to_string()], j)
            };
            out.push(Transformer::Except { strict, items });
            last = end;
            i = end + 1;
        } else if t.is_word(i, "REPLACE") {
            let mut j = i + 1;
            if t.is_word(j, "STRICT") {
                return Err(ChError::not_implemented(
                    "REPLACE STRICT is not supported by the Loams House engine yet",
                ));
            }
            if j >= t.len() || !matches!(t.tok(j), Token::LParen) {
                break;
            }
            let close = t
                .closing(j)
                .ok_or_else(|| syntax("unclosed REPLACE ( … )"))?;
            let d = t.depth[j] + 1;
            let mut elements = Vec::new();
            let mut a = j + 1;
            j += 1;
            while j <= close {
                if j == close || (t.depth[j] == d && matches!(t.tok(j), Token::Comma)) {
                    if a < j {
                        elements.push(t.span_text(a, j - 1).to_string());
                    }
                    a = j + 1;
                }
                j += 1;
            }
            out.push(Transformer::Replace(elements));
            last = close;
            i = close + 1;
        } else if t.is_word(i, "APPLY") {
            let j = i + 1;
            if j >= t.len() {
                return Err(syntax("expected a function after APPLY"));
            }
            if matches!(t.tok(j), Token::LParen) {
                let close = t.closing(j).ok_or_else(|| syntax("unclosed APPLY ( … )"))?;
                if close == j + 1 {
                    return Err(syntax("expected a function after APPLY"));
                }
                out.push(Transformer::Apply(
                    t.span_text(j + 1, close - 1).to_string(),
                ));
                last = close;
                i = close + 1;
            } else if t.is_ident(j) {
                out.push(Transformer::Apply(t.span_text(j, j).to_string()));
                last = j;
                i = j + 1;
            } else {
                return Err(syntax("expected a function after APPLY"));
            }
        } else {
            break;
        }
    }
    Ok((out, last))
}

/// The marker element of an `APPLY` transformer.
fn apply_element(inner: &str) -> Result<String, ChError> {
    let t = Toks::new(inner)?;
    if t.len() == 1 && t.is_ident(0) {
        return Ok(format!("__ch_apply AS {inner}"));
    }
    if (0..t.len()).any(|i| t.depth[i] == 0 && matches!(t.tok(i), Token::Arrow)) {
        return Ok(format!("__ch_apply({inner}) AS __ch_lambda"));
    }
    if t.len() >= 3
        && t.is_ident(0)
        && matches!(t.tok(1), Token::LParen)
        && t.closing(1) == Some(t.len() - 1)
    {
        let name = t.span_text(0, 0);
        let params = if t.len() > 3 {
            t.span_text(2, t.len() - 2)
        } else {
            ""
        };
        return Ok(format!("__ch_apply({params}) AS {name}"));
    }
    Err(syntax(&format!("APPLY({inner}) is not a function")))
}

/// Matchers with transformers sqlparser cannot read, as one `REPLACE` list of
/// markers.
pub fn matchers(sql: &str) -> Result<String, ChError> {
    let t = Toks::new(sql)?;
    let mut edits = Vec::new();
    for s in 0..t.len() {
        // (first index, last index, the star's text, COLUMNS' arguments)
        let matcher = match t.tok(s) {
            Token::Mul if s > 0 && matches!(t.tok(s - 1), Token::Period) => {
                let mut q = s - 1;
                while q >= 1 && t.is_ident(q - 1) {
                    if q >= 2 && matches!(t.tok(q - 2), Token::Period) {
                        q -= 2;
                    } else {
                        q -= 1;
                        break;
                    }
                }
                if q == s - 1 {
                    continue;
                }
                Some((q, s, t.span_text(q, s).to_string(), None))
            }
            Token::Mul
                if s == 0
                    || matches!(t.tok(s - 1), Token::LParen | Token::Comma)
                    || matches!(
                        t.kw(s - 1),
                        Some(Keyword::SELECT | Keyword::DISTINCT | Keyword::ALL)
                    ) =>
            {
                Some((s, s, "*".to_string(), None))
            }
            Token::Word(_)
                if t.is_word(s, "COLUMNS")
                    && s + 1 < t.len()
                    && matches!(t.tok(s + 1), Token::LParen) =>
            {
                let Some(close) = t.closing(s + 1) else {
                    continue;
                };
                let args = if close > s + 2 {
                    t.span_text(s + 2, close - 1).to_string()
                } else {
                    String::new()
                };
                Some((s, close, "*".to_string(), Some(args)))
            }
            _ => None,
        };
        let Some((first, last, star, columns)) = matcher else {
            continue;
        };
        let (chain, end) = transformers(&t, last + 1)?;
        if chain.is_empty() {
            continue;
        }
        // What sqlparser reads itself: `EXCEPT` (not STRICT) then `REPLACE`.
        let plain = columns.is_none()
            && matches!(
                chain.as_slice(),
                [Transformer::Except { strict: false, .. }]
                    | [Transformer::Replace(_)]
                    | [
                        Transformer::Except { strict: false, .. },
                        Transformer::Replace(_),
                    ]
            );
        if plain {
            continue;
        }
        let mut elements = Vec::new();
        if let Some(args) = &columns {
            elements.push(format!("__ch_columns({args}) AS __ch_matcher"));
        }
        for tr in &chain {
            match tr {
                Transformer::Except { strict, items } => {
                    for item in items {
                        if item.starts_with('\'') {
                            elements.push(format!("__ch_except({item}) AS __ch_re"));
                        } else if *strict {
                            elements.push(format!("__ch_except_strict AS {item}"));
                        } else {
                            elements.push(format!("__ch_except AS {item}"));
                        }
                    }
                }
                Transformer::Replace(items) => elements.extend(items.iter().cloned()),
                Transformer::Apply(inner) => elements.push(apply_element(inner)?),
            }
        }
        edits.push(Edit {
            start: t.start[first],
            end: t.end[end],
            with: format!("{star} REPLACE ({})", elements.join(", ")),
        });
    }
    Ok(apply(sql, edits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ternaries_become_if() {
        let t = |s: &str| ternary(s).unwrap();
        assert_eq!(
            t("SELECT number > 1 ? 1 : 0 FROM t"),
            "SELECT if(number > 1, 1, 0) FROM t"
        );
        assert_eq!(
            t("SELECT a = 1 OR a = 2 ? 1 : 0, 1 ? 2 : 3 + 4 AS x FROM t"),
            "SELECT if(a = 1 OR a = 2, 1, 0), if(1, 2, 3 + 4) AS x FROM t"
        );
        assert_eq!(t("SELECT 1?2:3"), "SELECT if(1, 2, 3)");
        assert_eq!(
            t("SELECT (a > 1 ? a : 0) + 1, f(x -> x ? 1 : 0) FROM t WHERE c ? 1 : 0 ORDER BY a"),
            "SELECT (if(a > 1, a, 0)) + 1, f(x -> if(x, 1, 0)) FROM t WHERE if(c, 1, 0) ORDER BY a"
        );
        assert_eq!(
            t("SELECT c ? 'p' : (d ? 'q' : 'r')"),
            "SELECT if(c, 'p', (if(d, 'q', 'r')))"
        );
        assert_eq!(
            t("SELECT c ? CASE WHEN x THEN 1 END : 2 FROM t"),
            "SELECT if(c, CASE WHEN x THEN 1 END, 2) FROM t"
        );
        assert_eq!(t("SELECT '?' AS q"), "SELECT '?' AS q");
        // As on the oracle: an unbracketed ternary inside another is an error.
        assert_eq!(ternary("SELECT a ? 1 : b ? 2 : 3").unwrap_err().code, 62);
        assert_eq!(ternary("SELECT a ? b ? 1 : 2 : 3").unwrap_err().code, 62);
    }

    #[test]
    fn scalar_with_becomes_a_marker_cte() {
        assert_eq!(
            scalar_with("WITH 5 AS n, c AS (SELECT 1), (SELECT max(a) FROM t) AS m SELECT n")
                .unwrap(),
            "WITH n AS (SELECT 5 AS __ch_with), c AS (SELECT 1), m AS (SELECT (SELECT max(a) FROM t) AS __ch_with) SELECT n"
        );
        assert_eq!(
            scalar_with("SELECT a FROM t GROUP BY a WITH TOTALS").unwrap(),
            "SELECT a FROM t GROUP BY a WITH TOTALS"
        );
        assert_eq!(
            scalar_with("SELECT x FROM (WITH a + 1 AS z SELECT z FROM t)").unwrap(),
            "SELECT x FROM (WITH z AS (SELECT a + 1 AS __ch_with) SELECT z FROM t)"
        );
    }

    #[test]
    fn inline_aliases_become_markers() {
        assert_eq!(
            inline_aliases("SELECT (1 AS n) + 2, n").unwrap(),
            "SELECT (__ch_alias(1, n)) + 2, n"
        );
        assert_eq!(
            inline_aliases("SELECT f(a, b + 1 AS z) FROM t WHERE ((a AS y) + 1 AS w) > 0").unwrap(),
            "SELECT f(a, __ch_alias(b + 1, z)) FROM t WHERE (__ch_alias((__ch_alias(a, y)) + 1, w)) > 0"
        );
        for kept in [
            "SELECT CAST(a AS String) FROM t",
            "SELECT x FROM (SELECT a AS x FROM t)",
            "SELECT * REPLACE (a * 2 AS a) FROM t",
            "WITH c AS (SELECT 1 AS z) SELECT z FROM c",
            "SELECT a IN (SELECT b AS c FROM u) FROM t",
        ] {
            assert_eq!(inline_aliases(kept).unwrap(), kept);
        }
    }

    #[test]
    fn matchers_with_transformers_become_one_replace_list() {
        let m = |s: &str| matchers(s).unwrap();
        assert_eq!(
            m("SELECT * APPLY(toString) APPLY length FROM t"),
            "SELECT * REPLACE (__ch_apply AS toString, __ch_apply AS length) FROM t"
        );
        assert_eq!(
            m("SELECT COLUMNS('^a') APPLY(x -> x + 1), t.* EXCEPT STRICT (b) FROM t"),
            "SELECT * REPLACE (__ch_columns('^a') AS __ch_matcher, __ch_apply(x -> x + 1) AS __ch_lambda), t.* REPLACE (__ch_except_strict AS b) FROM t"
        );
        assert_eq!(
            m("SELECT * REPLACE (a + 1 AS a) EXCEPT ('^b', c) APPLY(quantile(0.5)) FROM t"),
            "SELECT * REPLACE (a + 1 AS a, __ch_except('^b') AS __ch_re, __ch_except AS c, __ch_apply(0.5) AS quantile) FROM t"
        );
        for kept in [
            "SELECT * EXCEPT (a) REPLACE (b + 1 AS b) FROM t",
            "SELECT COLUMNS('^a'), count(*), 2 * replace(s, 'a', 'b') FROM t",
            "SELECT * FROM t EXCEPT SELECT * FROM u",
        ] {
            assert_eq!(m(kept), kept);
        }
    }
}
