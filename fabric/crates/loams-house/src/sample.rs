//! `SAMPLE` on lake tables (HE1 Task 9, ruling R9.15).
//!
//! A lake table is read through a view, and ClickHouse samples only a
//! MergeTree with `SAMPLE BY`: `FROM view SAMPLE n` is an error in chDB. The
//! House reads **every row** instead: [`rewrite`] takes the `SAMPLE k [OFFSET
//! m]` clause out of a statement that reads lake tables and reads the
//! `_sample_factor` virtual column as `1`. A sampled query's answer is then the
//! exact one its sample estimates (on ClickHouse too when the table holds fewer
//! rows than `SAMPLE n` asks for, Langfuse's case), at the cost of reading the
//! whole table.

use crate::request::{Token, tokens};

/// What a `_sample_factor` reads as when nothing is sampled.
pub const FACTOR: &str = "toFloat64(1)";

fn is_symbol(text: &str, t: &Token, symbol: &str) -> bool {
    !t.word && &text[t.start..t.end] == symbol
}

fn word(text: &str, t: &Token) -> String {
    text[t.start..t.end].to_ascii_uppercase()
}

/// The statement without its `SAMPLE` clauses and with `_sample_factor` read
/// as 1; `None` when it has neither.
pub fn rewrite(sql: &str) -> Option<String> {
    let toks = tokens(sql.as_bytes());
    let mut cuts: Vec<(usize, usize, &str)> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.word
            && word(sql, &t) == "SAMPLE"
            && !toks
                .get(i + 1)
                .is_some_and(|n| n.word && word(sql, n) == "BY")
        {
            // The sample: numbers, `.`, `/`, a `{name: Type}` parameter, and
            // `OFFSET` and another of them.
            let mut j = i + 1;
            let mut taken = false;
            while j < toks.len() {
                let n = toks[j];
                let raw = &sql[n.start..n.end];
                if is_symbol(sql, &n, "{") {
                    match (j..toks.len()).find(|&k| is_symbol(sql, &toks[k], "}")) {
                        Some(close) => j = close + 1,
                        None => break,
                    }
                    taken = true;
                } else if (n.word && raw.chars().next().is_some_and(|c| c.is_ascii_digit()))
                    || is_symbol(sql, &n, ".")
                    || is_symbol(sql, &n, "/")
                {
                    j += 1;
                    taken = true;
                } else if taken && n.word && word(sql, &n) == "OFFSET" {
                    j += 1;
                    taken = false;
                } else {
                    break;
                }
            }
            if taken {
                cuts.push((t.start, toks[j - 1].end, ""));
                i = j;
                continue;
            }
        }
        if t.word && &sql[t.start..t.end] == "_sample_factor" {
            // `e._sample_factor`: the qualifier goes too.
            let start = if i >= 2
                && is_symbol(sql, &toks[i - 1], ".")
                && toks[i - 1].start == toks[i - 2].end
                && toks[i].start == toks[i - 1].end
            {
                toks[i - 2].start
            } else {
                t.start
            };
            cuts.push((start, t.end, FACTOR));
        }
        i += 1;
    }
    if cuts.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(sql.len());
    let mut at = 0;
    for (start, end, with) in cuts {
        out.push_str(&sql[at..start]);
        out.push_str(with);
        at = end;
    }
    out.push_str(&sql[at..]);
    Some(out)
}

/// The clauses after which a `PREWHERE` or `WHERE` condition ends, at its
/// own depth.
const CLAUSES: &[&str] = &[
    "WHERE",
    "GROUP",
    "HAVING",
    "ORDER",
    "LIMIT",
    "SETTINGS",
    "FORMAT",
    "UNION",
    "WINDOW",
    "QUALIFY",
    "INTERSECT",
    "EXCEPT",
    "OFFSET",
    "FETCH",
];

/// Where the condition starting at token `from` (depth `depth`) ends: the
/// index of the next clause keyword at its depth, of the `)` that closes its
/// subquery, or of a `;`; `toks.len()` at the end.
fn condition_end(sql: &str, toks: &[Token], from: usize, depth: i32, stop_at_where: bool) -> usize {
    (from..toks.len())
        .find(|&k| {
            let t = toks[k];
            t.depth == depth
                && ((t.word
                    && CLAUSES
                        .iter()
                        .any(|c| (stop_at_where || *c != "WHERE") && word(sql, &t) == *c))
                    || is_symbol(sql, &t, ")")
                    || is_symbol(sql, &t, ";"))
        })
        .unwrap_or(toks.len())
}

/// HE1 R9.70: a lake table is read through a view, and a view takes no
/// `PREWHERE` ("Storage View does not support PREWHERE"; SigNoz's
/// metric-reduction views read `samples_v4_buffer PREWHERE …`). `PREWHERE p
/// WHERE w` becomes `WHERE (p) AND (w)`, and `PREWHERE p` alone `WHERE p`:
/// the same rows, as `PREWHERE` only orders the reading. `None` when there is
/// no `PREWHERE`.
pub fn prewhere_as_where(sql: &str) -> Option<String> {
    let toks = tokens(sql.as_bytes());
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if !(t.word && word(sql, t) == "PREWHERE") {
            continue;
        }
        let end = condition_end(sql, &toks, i + 1, t.depth, true);
        let is_where = toks
            .get(end)
            .is_some_and(|w| w.word && word(sql, w) == "WHERE");
        if !is_where {
            edits.push((t.start, t.end, "WHERE".to_string()));
            continue;
        }
        let where_end = condition_end(sql, &toks, end + 1, t.depth, false);
        let close_at = toks.get(where_end).map_or(sql.len(), |c| c.start);
        let last = toks[where_end - 1].end.min(close_at);
        edits.push((t.start, t.end, "WHERE (".to_string()));
        edits.push((toks[end].start, toks[end].end, ") AND (".to_string()));
        edits.push((last, last, ")".to_string()));
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(sql.len() + 16);
    let mut at = 0;
    for (start, end, with) in edits {
        out.push_str(&sql[at..start]);
        out.push_str(&with);
        at = end;
    }
    out.push_str(&sql[at..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prewhere_reads_as_where() {
        assert_eq!(
            prewhere_as_where("SELECT a FROM t PREWHERE x != 0 WHERE (y = 1) AND z GROUP BY a")
                .as_deref(),
            Some("SELECT a FROM t WHERE ( x != 0 ) AND ( (y = 1) AND z) GROUP BY a")
        );
        assert_eq!(
            prewhere_as_where("SELECT a FROM t PREWHERE x != 0 ORDER BY a").as_deref(),
            Some("SELECT a FROM t WHERE x != 0 ORDER BY a")
        );
        // SigNoz's metric reduction: inside a subquery, with `now()` in the
        // condition and the subquery's own GROUP BY after it.
        assert_eq!(
            prewhere_as_where(
                "SELECT k FROM (SELECT k FROM b PREWHERE r != 0 WHERE (f(x) = 0) AND (u < \
                 now()) GROUP BY k) WHERE k > 1"
            )
            .as_deref(),
            Some(
                "SELECT k FROM (SELECT k FROM b WHERE ( r != 0 ) AND ( (f(x) = 0) AND (u < \
                 now())) GROUP BY k) WHERE k > 1"
            )
        );
        assert_eq!(
            prewhere_as_where("SELECT 1 FROM (SELECT * FROM b PREWHERE r WHERE s)").as_deref(),
            Some("SELECT 1 FROM (SELECT * FROM b WHERE ( r ) AND ( s))")
        );
        assert_eq!(
            prewhere_as_where("SELECT 'PREWHERE', prewhere_x FROM t"),
            None
        );
    }

    #[test]
    fn langfuse_samples_read_every_row() {
        assert_eq!(
            rewrite("SELECT toString(e.user_id) AS value, toUInt64(round(count() * any(e._sample_factor))) AS count FROM events_core e SAMPLE 1000000 WHERE e.project_id = 'p' GROUP BY value").as_deref(),
            Some("SELECT toString(e.user_id) AS value, toUInt64(round(count() * any(toFloat64(1)))) AS count FROM events_core e  WHERE e.project_id = 'p' GROUP BY value")
        );
        assert_eq!(
            rewrite("SELECT 1 FROM t SAMPLE 1/10 OFFSET 1/2 WHERE x").as_deref(),
            Some("SELECT 1 FROM t  WHERE x")
        );
        assert_eq!(
            rewrite("SELECT 1 FROM t AS a SAMPLE 0.1").as_deref(),
            Some("SELECT 1 FROM t AS a ")
        );
        assert_eq!(
            rewrite("SELECT 1 FROM t SAMPLE {n: UInt64} LIMIT 1").as_deref(),
            Some("SELECT 1 FROM t  LIMIT 1")
        );
        assert_eq!(
            rewrite("SELECT `_sample_factor`, 'SAMPLE 10' FROM t"),
            None,
            "a quoted name and a string are text"
        );
        assert_eq!(rewrite("SELECT sample FROM t"), None);
        assert_eq!(rewrite("SELECT 1 FROM t"), None);
    }
}
