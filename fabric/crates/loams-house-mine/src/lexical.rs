//! Features read from tokens rather than from a tree, the same way whichever
//! parser read the statement: the statement kind (`syntax:SELECT`,
//! `syntax:ALTER UPDATE`, ...) and query parameters (`{id:String}`).

use sqlparser::tokenizer::{Token, Word};

use crate::features::Features;
use crate::tokens;

/// The statement-kind and query-parameter features of `text`. Text the
/// tokenizer refuses has none.
pub fn features(text: &str) -> Features {
    let mut out = Features::default();
    let Ok(spanned) = tokens::tokenize(text) else {
        return out;
    };
    let toks: Vec<&Token> = spanned
        .iter()
        .map(|t| &t.token)
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    statement_kind(&toks, &mut out);
    query_parameters(&toks, &mut out);
    if toks.iter().any(|t| matches!(t, Token::DoubleColon)) {
        out.add("syntax", "cast operator ::");
    }
    out
}

/// `text` with each migration-template placeholder `{NAME}` (a brace, one
/// identifier, a brace: not a `{name:Type}` query parameter) removed, or
/// `None` if it has none. Langfuse's canonical migrations carry
/// `{CLICKHOUSE_CLUSTER_CLAUSE}`; removing it gives the unclustered form.
pub fn strip_template_placeholders(text: &str) -> Option<String> {
    let spanned = tokens::tokenize(text).ok()?;
    let solid: Vec<&tokens::Spanned> = spanned
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect();
    let mut cuts = Vec::new();
    for w in solid.windows(3) {
        if let (Token::LBrace, Token::Word(_), Token::RBrace) =
            (&w[0].token, &w[1].token, &w[2].token)
        {
            cuts.push(w[0].range.start..w[2].range.end);
        }
    }
    if cuts.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for c in cuts {
        out.push_str(&text[at..c.start]);
        at = c.end;
    }
    out.push_str(&text[at..]);
    Some(out)
}

/// The name `CREATE [OR REPLACE] FUNCTION [IF NOT EXISTS] <name>` defines.
pub fn defined_function(text: &str) -> Option<String> {
    let spanned = tokens::tokenize(text).ok()?;
    let toks: Vec<&Token> = spanned
        .iter()
        .map(|t| &t.token)
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if !word_is(&toks, 0, "CREATE") {
        return None;
    }
    let mut i = 1;
    while word_is(&toks, i, "OR") || word_is(&toks, i, "REPLACE") {
        i += 1;
    }
    if !word_is(&toks, i, "FUNCTION") {
        return None;
    }
    i += 1;
    if word_is(&toks, i, "IF") {
        i += if word_is(&toks, i + 1, "NOT") { 3 } else { 2 };
    }
    match toks.get(i) {
        Some(Token::Word(w)) => Some(w.value.clone()),
        _ => None,
    }
}

/// `text` with each query parameter `{name:Type}` replaced by the identifier
/// `__param_name`, so sqlparser does not read the type as an expression (it
/// takes `{a:DateTime64(3)}` for a dictionary holding a call). The
/// parameters and their types are read by [`features`].
pub fn mask_parameters(text: &str) -> std::borrow::Cow<'_, str> {
    let Ok(spanned) = tokens::tokenize(text) else {
        return text.into();
    };
    let solid: Vec<&tokens::Spanned> = spanned
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect();
    let mut cuts: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut i = 0;
    while i + 3 < solid.len() {
        if let (Token::LBrace, Token::Word(name), Token::Colon) =
            (&solid[i].token, &solid[i + 1].token, &solid[i + 2].token)
            && let Some(j) = (i + 3..solid.len()).find(|&j| matches!(solid[j].token, Token::RBrace))
        {
            cuts.push((
                solid[i].range.start..solid[j].range.end,
                format!("__param_{}", name.value),
            ));
            i = j + 1;
            continue;
        }
        i += 1;
    }
    if cuts.is_empty() {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, ident) in cuts {
        out.push_str(&text[at..range.start]);
        out.push_str(&ident);
        at = range.end;
    }
    out.push_str(&text[at..]);
    out.into()
}

fn word(t: &Token) -> Option<String> {
    match t {
        Token::Word(Word {
            value,
            quote_style: None,
            ..
        }) => Some(value.to_ascii_uppercase()),
        _ => None,
    }
}

fn words_at(toks: &[&Token], at: usize, n: usize) -> Vec<String> {
    toks.iter()
        .skip(at)
        .take(n)
        .filter_map(|t| word(t))
        .collect()
}

/// Objects `CREATE`, `DROP` and friends name, longest first.
const OBJECTS: [&[&str]; 9] = [
    &["MATERIALIZED", "VIEW"],
    &["LIVE", "VIEW"],
    &["WINDOW", "VIEW"],
    &["TABLE"],
    &["VIEW"],
    &["DATABASE"],
    &["DICTIONARY"],
    &["FUNCTION"],
    &["USER"],
];

fn object_after(toks: &[&Token], mut at: usize) -> Option<String> {
    // Skip modifiers: OR REPLACE, TEMPORARY, IF [NOT] EXISTS come before or
    // after the object.
    while let Some(w) = toks.get(at).and_then(|t| word(t)) {
        if matches!(w.as_str(), "OR" | "REPLACE" | "TEMPORARY") {
            at += 1;
        } else {
            break;
        }
    }
    let ahead = words_at(toks, at, 2);
    OBJECTS
        .iter()
        .find(|o| {
            ahead.len() >= o.len()
                && ahead[..o.len()] == o.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()[..]
        })
        .map(|o| o.join(" "))
}

fn statement_kind(toks: &[&Token], out: &mut Features) {
    // A statement may start with parentheses: `(SELECT 1) UNION ALL ...`.
    let first = toks.iter().position(|t| !matches!(t, Token::LParen));
    let Some(first) = first else { return };
    let Some(head) = word(toks[first]) else {
        return;
    };
    match head.as_str() {
        "SELECT" | "WITH" => out.add("syntax", "SELECT"),
        "CREATE" | "DROP" | "ATTACH" | "DETACH" => {
            let object = object_after(toks, first + 1).unwrap_or_else(|| "(other)".to_owned());
            out.add("syntax", &format!("{head} {object}"));
        }
        "ALTER" => {
            out.add("syntax", "ALTER TABLE");
            alter_commands(toks, first + 1, out);
        }
        "SHOW" | "SYSTEM" => {
            let rest = words_at(toks, first + 1, 1);
            out.add(
                "syntax",
                &format!("{head} {}", rest.first().map_or("(other)", String::as_str)),
            );
        }
        "DESC" => out.add("syntax", "DESCRIBE"),
        "INSERT" | "DELETE" | "UPDATE" | "OPTIMIZE" | "TRUNCATE" | "RENAME" | "EXCHANGE"
        | "SET" | "DESCRIBE" | "EXISTS" | "USE" | "EXPLAIN" | "KILL" | "GRANT" | "REVOKE"
        | "CHECK" | "WATCH" | "UNDROP" => out.add("syntax", &head),
        _ => out.add("syntax", "(other statement)"),
    }
}

/// `ALTER TABLE t [ON CLUSTER c] DELETE WHERE ..., UPDATE ...`: each command
/// at parenthesis depth 0, after the table name and the cluster.
fn alter_commands(toks: &[&Token], at: usize, out: &mut Features) {
    let mut i = at;
    if word_is(toks, i, "TABLE") {
        i += 1;
    }
    // The table name: `t` or `db.t`.
    if matches!(toks.get(i), Some(Token::Word(_))) {
        i += 1;
        while matches!(toks.get(i), Some(Token::Period))
            && matches!(toks.get(i + 1), Some(Token::Word(_)))
        {
            i += 2;
        }
    }
    if word_is(toks, i, "ON") && word_is(toks, i + 1, "CLUSTER") {
        out.add("syntax", "ON CLUSTER");
        i += 3;
    }
    let mut depth = 0usize;
    let mut command_start = true;
    while i < toks.len() {
        match toks[i] {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            // A comma starts a new command only before a command verb: in
            // `UPDATE a = 1, b = 2` and `MODIFY SETTING a = 1, b = 2` it
            // separates assignments.
            Token::Comma
                if depth == 0
                    && words_at(toks, i + 1, 1)
                        .first()
                        .is_some_and(|w| ALTER_VERBS.contains(&w.as_str())) =>
            {
                command_start = true;
                i += 1;
                continue;
            }
            _ => {}
        }
        if command_start {
            command_start = false;
            let w = words_at(toks, i, 2);
            let name = match w.first().map(String::as_str) {
                Some("DELETE") => "ALTER DELETE".to_owned(),
                Some("UPDATE") => "ALTER UPDATE".to_owned(),
                Some(
                    verb @ ("ADD" | "DROP" | "MODIFY" | "CLEAR" | "MATERIALIZE" | "RENAME"
                    | "COMMENT"),
                ) => match w.get(1).map(String::as_str) {
                    Some(
                        obj @ ("COLUMN" | "INDEX" | "PROJECTION" | "CONSTRAINT" | "TTL" | "SETTING"
                        | "ORDER" | "QUERY" | "STATISTICS" | "PARTITION"),
                    ) => {
                        format!("ALTER {verb} {obj}")
                    }
                    _ => format!("ALTER {verb}"),
                },
                Some(other) => format!("ALTER {other}"),
                None => "ALTER (other)".to_owned(),
            };
            out.add("syntax", &name);
        }
        i += 1;
    }
}

/// The verbs an `ALTER TABLE` command starts with.
const ALTER_VERBS: [&str; 20] = [
    "ADD",
    "APPLY",
    "ATTACH",
    "CLEAR",
    "COMMENT",
    "DELETE",
    "DETACH",
    "DROP",
    "FETCH",
    "FREEZE",
    "MATERIALIZE",
    "MODIFY",
    "MOVE",
    "REMOVE",
    "RENAME",
    "REPLACE",
    "RESET",
    "UNFREEZE",
    "UPDATE",
    "UNLOCK",
];

fn word_is(toks: &[&Token], i: usize, w: &str) -> bool {
    toks.get(i).and_then(|t| word(t)).is_some_and(|x| x == w)
}

/// `{name:Type}`: ClickHouse query parameters, and the types they declare.
fn query_parameters(toks: &[&Token], out: &mut Features) {
    let mut i = 0;
    while i + 3 < toks.len() {
        if matches!(toks[i], Token::LBrace)
            && matches!(toks[i + 1], Token::Word(_))
            && matches!(toks[i + 2], Token::Colon)
        {
            // The type runs to the matching `}`.
            let mut j = i + 3;
            let mut ty = String::new();
            while j < toks.len() && !matches!(toks[j], Token::RBrace) {
                ty.push_str(&toks[j].to_string());
                ty.push(' ');
                j += 1;
            }
            if j < toks.len() {
                out.add("syntax", "query parameter");
                out.data_type(&ty);
                i = j;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syntax(text: &str) -> Vec<String> {
        features(text)
            .0
            .into_iter()
            .filter(|f| f.starts_with("syntax:"))
            .collect()
    }

    #[test]
    fn statement_kinds() {
        assert_eq!(
            syntax("WITH a AS (SELECT 1) SELECT * FROM a"),
            ["syntax:SELECT"]
        );
        assert_eq!(
            syntax("CREATE MATERIALIZED VIEW IF NOT EXISTS v TO t AS SELECT 1"),
            ["syntax:CREATE MATERIALIZED VIEW"]
        );
        assert_eq!(
            syntax("CREATE OR REPLACE VIEW v AS SELECT 1"),
            ["syntax:CREATE VIEW"]
        );
        assert_eq!(syntax("drop table if exists t"), ["syntax:DROP TABLE"]);
        assert_eq!(syntax("SHOW TABLES"), ["syntax:SHOW TABLES"]);
    }

    #[test]
    fn alter_commands_each_count() {
        assert_eq!(
            syntax(
                "ALTER TABLE db.t ON CLUSTER default DELETE WHERE id IN (1, 2), UPDATE a = 1 WHERE b"
            ),
            [
                "syntax:ALTER DELETE",
                "syntax:ALTER TABLE",
                "syntax:ALTER UPDATE",
                "syntax:ON CLUSTER"
            ]
        );
        assert_eq!(
            syntax(
                "ALTER TABLE t ADD COLUMN IF NOT EXISTS c String, MODIFY TTL ts + INTERVAL 1 DAY"
            ),
            [
                "syntax:ALTER ADD COLUMN",
                "syntax:ALTER MODIFY TTL",
                "syntax:ALTER TABLE"
            ]
        );
    }

    #[test]
    fn defined_functions() {
        assert_eq!(
            defined_function("CREATE OR REPLACE FUNCTION f_x AS (a) -> a + 1").as_deref(),
            Some("f_x")
        );
        assert_eq!(
            defined_function("CREATE FUNCTION IF NOT EXISTS g AS (a) -> a").as_deref(),
            Some("g")
        );
        assert_eq!(
            defined_function("CREATE TABLE t (a UInt8) ENGINE = Memory"),
            None
        );
    }

    #[test]
    fn commas_inside_a_command_do_not_start_one() {
        assert_eq!(
            syntax("ALTER TABLE t UPDATE a = 1, b_visits = 2 WHERE c > 0"),
            ["syntax:ALTER TABLE", "syntax:ALTER UPDATE"]
        );
        assert_eq!(
            syntax("ALTER TABLE t MODIFY SETTING a = 1, b = 2, DROP COLUMN c"),
            [
                "syntax:ALTER DROP COLUMN",
                "syntax:ALTER MODIFY SETTING",
                "syntax:ALTER TABLE"
            ]
        );
    }

    #[test]
    fn query_parameters_and_their_types() {
        let f = features(
            "SELECT * FROM t WHERE a = {id:String} AND b IN {ids:Array(Nullable(UInt64))}",
        );
        for want in [
            "syntax:query parameter",
            "type:String",
            "type:Array",
            "type:Nullable",
            "type:UInt64",
        ] {
            assert!(f.contains(want), "{want} in {f:?}");
        }
    }
}
