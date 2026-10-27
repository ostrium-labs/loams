//! Features from a ClickHouse server's `EXPLAIN AST`, for the statements
//! sqlparser refuses (plan HE1 Task 2).
//!
//! `EXPLAIN AST` prints the server parser's tree, one node per line, indented
//! one space per level: `Function count (children 1)`, `DataType String`,
//! `Storage definition (children 3)`. The walker reads functions, types,
//! engines, table functions and the clause nodes from it. What the tree does
//! not show (setting names, `FORMAT`, `FINAL`, the join kind) is read from
//! the statement's tokens by [`lexical_extras`], which runs only on
//! statements the server accepted.

use std::io::Read as _;

use sqlparser::tokenizer::{Token, Word};

use crate::features::Features;
use crate::tokens;

/// Something that answers `EXPLAIN AST` for a statement.
pub trait Explainer {
    /// The `EXPLAIN AST` output, or `None` if the server refused the
    /// statement.
    fn explain_ast(&mut self, statement: &str) -> Option<String>;
}

/// A ClickHouse server's HTTP interface.
#[derive(Debug)]
pub struct HttpExplainer {
    url: String,
    agent: ureq::Agent,
}

impl HttpExplainer {
    /// An explainer for the server at `url` (`http://127.0.0.1:8123`).
    pub fn new(url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .new_agent();
        Self {
            url: url.trim_end_matches('/').to_owned(),
            agent,
        }
    }
}

impl Explainer for HttpExplainer {
    fn explain_ast(&mut self, statement: &str) -> Option<String> {
        let body = format!("EXPLAIN AST {}", strip_trailing_format(statement));
        // The server refuses a statement with unbound query parameters, even
        // to explain it; bind each to a value of its type.
        let mut url = format!("{}/?default_format=TSVRaw", self.url);
        for (name, value) in parameter_values(statement) {
            url.push_str(&format!(
                "&param_{}={}",
                percent_encode(&name),
                percent_encode(&value)
            ));
        }
        let mut response = self.agent.post(&url).send(body.as_bytes()).ok()?;
        if response.status() != 200 {
            return None;
        }
        let mut text = String::new();
        response
            .body_mut()
            .as_reader()
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }
}

/// Answers recorded from a server, for tests and offline runs: a TSV of
/// `statement` and `dump` (ClickHouse TSV escaping); an empty dump records
/// that the server refused the statement.
#[derive(Debug, Default)]
pub struct Recorded {
    answers: std::collections::BTreeMap<String, String>,
    /// Statements asked for that have no recorded answer.
    pub missing: Vec<String>,
}

impl Recorded {
    /// Reads a recording.
    pub fn parse(text: &str) -> Self {
        let answers = text
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once('\t'))
            .map(|(s, d)| (crate::querylog::unescape(s), crate::querylog::unescape(d)))
            .collect();
        Self {
            answers,
            missing: Vec::new(),
        }
    }

    /// Adds an answer.
    pub fn insert(&mut self, statement: &str, dump: Option<String>) {
        self.answers
            .insert(statement.to_owned(), dump.unwrap_or_default());
    }

    /// The recording as TSV.
    pub fn to_tsv(&self) -> String {
        let esc = |s: &str| {
            s.replace('\\', "\\\\")
                .replace('\t', "\\t")
                .replace('\n', "\\n")
        };
        let mut out = String::from("statement\tdump\n");
        for (s, d) in &self.answers {
            out.push_str(&esc(s));
            out.push('\t');
            out.push_str(&esc(d));
            out.push('\n');
        }
        out
    }
}

impl Explainer for Recorded {
    fn explain_ast(&mut self, statement: &str) -> Option<String> {
        match self.answers.get(statement) {
            Some(d) if d.is_empty() => None,
            Some(d) => Some(d.clone()),
            None => {
                self.missing.push(statement.to_owned());
                None
            }
        }
    }
}

/// The query parameters of `statement`, `{name:Type}`, each with a value
/// its type parses.
pub fn parameter_values(statement: &str) -> Vec<(String, String)> {
    let Ok(toks) = tokens::tokenize(statement) else {
        return Vec::new();
    };
    let toks: Vec<&Token> = toks
        .iter()
        .map(|t| &t.token)
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i + 3 < toks.len() {
        if let (Token::LBrace, Token::Word(name), Token::Colon) =
            (toks[i], toks[i + 1], toks[i + 2])
        {
            let mut j = i + 3;
            let mut ty = String::new();
            while j < toks.len() && !matches!(toks[j], Token::RBrace) {
                ty.push_str(&toks[j].to_string());
                j += 1;
            }
            if j < toks.len() && !out.iter().any(|(n, _)| *n == name.value) {
                out.push((name.value.clone(), default_value(&ty)));
            }
            i = j;
        }
        i += 1;
    }
    out
}

/// A value of ClickHouse type `ty` in the text form query parameters take.
fn default_value(ty: &str) -> String {
    let ty = ty.trim();
    let (family, inner) = match ty.find('(') {
        Some(i) if ty.ends_with(')') => (&ty[..i], &ty[i + 1..ty.len() - 1]),
        _ => (ty, ""),
    };
    match family {
        "Nullable" => "\\N".to_owned(),
        "LowCardinality" => default_value(inner),
        "Array" => "[]".to_owned(),
        "Map" => "{}".to_owned(),
        "Date" | "Date32" => "1970-01-01".to_owned(),
        "DateTime" | "DateTime64" => "1970-01-01 00:00:00".to_owned(),
        "UUID" => "00000000-0000-0000-0000-000000000000".to_owned(),
        "IPv4" => "0.0.0.0".to_owned(),
        "IPv6" => "::".to_owned(),
        "String" | "FixedString" => String::new(),
        "Identifier" => "t".to_owned(),
        _ => "0".to_owned(),
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `EXPLAIN AST` refuses some statements with a trailing `FORMAT X` that
/// the plain statement accepts; the format is read lexically anyway.
fn strip_trailing_format(statement: &str) -> &str {
    let trimmed = statement.trim_end().trim_end_matches(';').trim_end();
    let Ok(toks) = tokens::tokenize(trimmed) else {
        return trimmed;
    };
    let words: Vec<&tokens::Spanned> = toks
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect();
    if words.len() >= 3
        && let [.., fmt, name] = words.as_slice()
        && word_upper(&fmt.token).as_deref() == Some("FORMAT")
        && matches!(name.token, Token::Word(_))
        && word_upper(&words[0].token).as_deref() != Some("INSERT")
    {
        return trimmed[..fmt.range.start].trim_end();
    }
    trimmed
}

/// One node of the dump.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node<'a> {
    depth: usize,
    /// `Function`, `DataType`, `Storage definition`, ...
    kind: &'a str,
    /// What follows the kind: a function or type name.
    name: &'a str,
}

/// Node kinds that take a name after them; every other line is a kind alone.
const NAMED: [&str; 13] = [
    "Function",
    "DataType",
    "EnumDataType",
    "Identifier",
    "TableIdentifier",
    "Literal",
    "QueryParameter",
    "ColumnDeclaration",
    "AlterCommand",
    "CreateQuery",
    "InsertQuery",
    "Index",
    "Projection",
];

fn parse(dump: &str) -> Vec<Node<'_>> {
    dump.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let depth = line.len() - line.trim_start_matches(' ').len();
            let mut body = line.trim();
            if let Some(i) = body.rfind(" (children ") {
                body = &body[..i];
            }
            let (kind, name) = match NAMED.iter().find(|k| {
                body.strip_prefix(**k)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
            }) {
                Some(k) => (*k, body[k.len()..].trim()),
                None => (body, ""),
            };
            Node { depth, kind, name }
        })
        .collect()
}

/// The features an `EXPLAIN AST` dump shows.
pub fn features(dump: &str) -> Features {
    let nodes = parse(dump);
    let mut out = Features::default();
    // Each node's ancestors, nearest last.
    let mut stack: Vec<&Node<'_>> = Vec::new();
    // How many children each stack entry has had so far.
    let mut seen: Vec<usize> = Vec::new();
    // Storage definitions whose engine has been seen, by depth.
    let mut engine_seen: Vec<usize> = Vec::new();
    for node in &nodes {
        while stack.last().is_some_and(|n| n.depth >= node.depth) {
            stack.pop();
            seen.pop();
        }
        // This node's position among its parent's children.
        let index = seen.last().copied().unwrap_or(0);
        if let Some(n) = seen.last_mut() {
            *n += 1;
        }
        engine_seen.retain(|d| *d < node.depth);
        let parent = stack.last().map(|n| n.kind);
        let grandparent = stack.len().checked_sub(2).map(|i| stack[i]);
        let first = |n: &Node<'_>| n.name.split_whitespace().next().unwrap_or("").to_owned();
        match node.kind {
            "Function" => {
                let name = first(node);
                if parent == Some("Storage definition") && !engine_seen.contains(&(node.depth - 1))
                {
                    out.add("engine", &name);
                    engine_seen.push(node.depth - 1);
                } else if parent == Some("TableExpression") {
                    out.add("table_function", &name);
                } else if parent == Some("Index") && index == 1 {
                    // `INDEX i <expr> TYPE <type>`: the second child is the
                    // index type; the first is an ordinary expression.
                    out.add(
                        "syntax",
                        &format!("skip index {}", name.to_ascii_lowercase()),
                    );
                } else if name == "_CAST" {
                    // A bound query parameter: `query_log` holds the text
                    // after binding, `{p:UUID}` as `_CAST('…', 'UUID')`.
                    out.add("syntax", "query parameter");
                } else if name.eq_ignore_ascii_case("CODEC") {
                    out.add("syntax", "column CODEC");
                } else if grandparent.is_some_and(|g| first(g).eq_ignore_ascii_case("CODEC")) {
                    out.add("syntax", &format!("codec {}", name.to_ascii_uppercase()));
                } else {
                    out.function(&name);
                }
            }
            "Literal" => {
                // `CAST(x AS T)`, `x::T` and a bound parameter `_CAST(v, 'T')`
                // carry the type as their second argument, a string literal.
                // Only that one: the first is the value, which must never
                // become a feature name (Review Focus 2).
                if index == 1
                    && parent == Some("ExpressionList")
                    && grandparent.is_some_and(|g| {
                        g.kind == "Function" && matches!(first(g).as_str(), "CAST" | "_CAST")
                    })
                    && let Some(ty) = string_literal(node.name)
                {
                    out.data_type(&ty);
                }
            }
            "DataType" | "EnumDataType" => out.data_type(&first(node)),
            "WithElement" => out.add("syntax", "WITH"),
            "ArrayJoin" => out.add("syntax", "ARRAY JOIN"),
            "WindowDefinition" => out.add("syntax", "window function"),
            "Subquery" => {
                if parent == Some("TableExpression") {
                    out.add("syntax", "subquery in FROM");
                } else {
                    out.add("syntax", "scalar subquery");
                }
            }
            "Projection" => out.add("syntax", "PROJECTION"),
            _ => {}
        }
        stack.push(node);
        seen.push(0);
    }
    out
}

/// The value of a string literal as `EXPLAIN AST` prints it, `'x'` or, TSV
/// escaped, `\'x\'`.
fn string_literal(text: &str) -> Option<String> {
    let t = text.trim();
    let inner = t
        .strip_prefix("\\'")
        .and_then(|r| r.strip_suffix("\\'"))
        .or_else(|| t.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))?;
    Some(inner.replace("\\'", "'"))
}

fn word_upper(t: &Token) -> Option<String> {
    match t {
        Token::Word(Word {
            value,
            quote_style: None,
            ..
        }) => Some(value.to_ascii_uppercase()),
        _ => None,
    }
}

/// Clause features `EXPLAIN AST` does not print, read from the tokens of a
/// statement the server accepted: `FINAL`, `PREWHERE`, `SAMPLE`,
/// `LIMIT BY`, `WITH TOTALS`/`ROLLUP`/`CUBE`, the join kinds, `UNION`,
/// `ON CLUSTER`, the table clauses (`PARTITION BY`, `ORDER BY`,
/// `PRIMARY KEY`, `TTL`), column defaults, `SETTINGS` names and `FORMAT`.
pub fn lexical_extras(text: &str) -> Features {
    let mut out = Features::default();
    let Ok(spanned) = tokens::tokenize(text) else {
        return out;
    };
    let toks: Vec<&Token> = spanned
        .iter()
        .map(|t| &t.token)
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let w: Vec<Option<String>> = toks.iter().map(|t| word_upper(t)).collect();
    let is = |i: usize, s: &str| w.get(i).and_then(|x| x.as_deref()) == Some(s);
    let head = w.iter().flatten().next().cloned().unwrap_or_default();
    let ddl = matches!(head.as_str(), "CREATE" | "ALTER" | "ATTACH");
    let mut depth = 0usize;
    for i in 0..toks.len() {
        match toks[i] {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            _ => {}
        }
        let Some(word) = w[i].as_deref() else {
            continue;
        };
        match word {
            "FINAL" => out.add(
                "syntax",
                if head == "OPTIMIZE" {
                    "OPTIMIZE FINAL"
                } else {
                    "FINAL"
                },
            ),
            "DEDUPLICATE" if head == "OPTIMIZE" => out.add("syntax", "OPTIMIZE DEDUPLICATE"),
            "PREWHERE" => out.add("syntax", "PREWHERE"),
            "SAMPLE" if !ddl || is(i + 1, "BY") => {
                out.add(
                    "syntax",
                    if is(i + 1, "BY") {
                        "SAMPLE BY"
                    } else {
                        "SAMPLE"
                    },
                );
            }
            "HAVING" => out.add("syntax", "HAVING"),
            "DISTINCT" => out.add("syntax", "DISTINCT"),
            "WITH" if is(i + 1, "TOTALS") => out.add("syntax", "WITH TOTALS"),
            "WITH" if is(i + 1, "ROLLUP") => out.add("syntax", "WITH ROLLUP"),
            "WITH" if is(i + 1, "CUBE") => out.add("syntax", "WITH CUBE"),
            "WITH" if is(i + 1, "FILL") => out.add("syntax", "WITH FILL"),
            "GROUPING" if is(i + 1, "SETS") => out.add("syntax", "GROUPING SETS"),
            "UNION" => out.add(
                "syntax",
                if is(i + 1, "ALL") {
                    "UNION ALL"
                } else if is(i + 1, "DISTINCT") {
                    "UNION DISTINCT"
                } else {
                    "UNION"
                },
            ),
            "EXCEPT" if !matches!(toks.get(i + 1), Some(Token::LParen)) || i == 0 => {
                out.add("syntax", "EXCEPT");
            }
            "INTERSECT" => out.add("syntax", "INTERSECT"),
            "ON" if is(i + 1, "CLUSTER") => out.add("syntax", "ON CLUSTER"),
            "JOIN" => {
                let mut mods = Vec::new();
                let mut j = i;
                while j > 0 {
                    j -= 1;
                    match w[j].as_deref() {
                        Some(
                            m @ ("GLOBAL" | "ANY" | "ALL" | "ASOF" | "SEMI" | "ANTI" | "LEFT"
                            | "RIGHT" | "FULL" | "INNER" | "CROSS" | "OUTER" | "ARRAY"),
                        ) => mods.push(m),
                        _ => break,
                    }
                }
                let has = |m: &str| mods.contains(&m);
                let form = if has("ARRAY") {
                    if has("LEFT") {
                        "LEFT ARRAY JOIN"
                    } else if has("INNER") {
                        "INNER ARRAY JOIN"
                    } else {
                        "ARRAY JOIN"
                    }
                } else if has("ASOF") {
                    "ASOF JOIN"
                } else if has("SEMI") {
                    "SEMI JOIN"
                } else if has("ANTI") {
                    "ANTI JOIN"
                } else if has("CROSS") {
                    "CROSS JOIN"
                } else if has("FULL") {
                    "FULL JOIN"
                } else if has("RIGHT") {
                    "RIGHT JOIN"
                } else if has("LEFT") {
                    "LEFT JOIN"
                } else {
                    "INNER JOIN"
                };
                out.add("syntax", form);
                if has("GLOBAL") {
                    out.add("syntax", "GLOBAL JOIN");
                }
                if has("ANY") {
                    out.add("syntax", "ANY JOIN");
                }
            }
            "GLOBAL" if is(i + 1, "IN") || (is(i + 1, "NOT") && is(i + 2, "IN")) => {
                out.add("syntax", "GLOBAL IN");
            }
            "LIMIT" => {
                out.add("syntax", "LIMIT");
                // LIMIT n [OFFSET m] BY ...: a BY before the next clause.
                let mut j = i + 1;
                while j < toks.len() {
                    match w[j].as_deref() {
                        Some("BY") => {
                            out.add("syntax", "LIMIT BY");
                            break;
                        }
                        Some("SETTINGS" | "FORMAT" | "UNION" | "LIMIT" | "INTO") => break,
                        _ if matches!(toks[j], Token::RParen | Token::SemiColon) => break,
                        _ => j += 1,
                    }
                }
            }
            "OFFSET" => out.add("syntax", "OFFSET"),
            "PARTITION" if ddl && is(i + 1, "BY") && depth == 0 => {
                out.add("syntax", "PARTITION BY")
            }
            "ORDER" if is(i + 1, "BY") => {
                if ddl && depth == 0 && head == "CREATE" && !after_as_select(&w, i) {
                    out.add("syntax", "table ORDER BY");
                } else if !ddl || after_as_select(&w, i) {
                    out.add("syntax", "ORDER BY");
                }
            }
            "GROUP" if is(i + 1, "BY") => out.add("syntax", "GROUP BY"),
            "PRIMARY" if ddl && is(i + 1, "KEY") && depth == 0 => {
                out.add("syntax", "table PRIMARY KEY")
            }
            "TTL" if ddl => out.add("syntax", "TTL"),
            "CODEC" if ddl => out.add("syntax", "column CODEC"),
            "DEFAULT" if ddl && depth == 1 => out.add("syntax", "column DEFAULT"),
            "MATERIALIZED" if ddl && depth == 1 => out.add("syntax", "column MATERIALIZED"),
            "ALIAS" if ddl && depth == 1 => out.add("syntax", "column ALIAS"),
            "EPHEMERAL" if ddl && depth == 1 => out.add("syntax", "column EPHEMERAL"),
            "TO" if head == "CREATE" && is(1, "MATERIALIZED") => {
                out.add("syntax", "MATERIALIZED VIEW TO")
            }
            "SETTINGS" => {
                if ddl && depth == 0 && !after_as_select(&w, i) && head == "CREATE" {
                    out.add("syntax", "table SETTINGS");
                }
                settings_after(&toks, i + 1, &mut out);
            }
            "FORMAT" if matches!(toks.get(i + 1), Some(Token::Word(_))) => {
                if let Some(Token::Word(f)) = toks.get(i + 1) {
                    out.add("format", &f.value);
                }
                if head == "INSERT" {
                    out.add("syntax", "INSERT FORMAT");
                }
            }
            "VALUES" if head == "INSERT" => out.add("syntax", "INSERT VALUES"),
            "SELECT" if head == "INSERT" => out.add("syntax", "INSERT SELECT"),
            _ => {}
        }
    }
    out
}

/// Whether word `i` comes after an `AS SELECT` (the query of a view or a
/// `CREATE TABLE ... AS SELECT`), where clauses are the query's.
fn after_as_select(w: &[Option<String>], i: usize) -> bool {
    (1..i).any(|j| w[j - 1].as_deref() == Some("AS") && w[j].as_deref() == Some("SELECT"))
}

/// `SETTINGS a = 1, b = 'x'`: the names.
fn settings_after(toks: &[&Token], mut i: usize, out: &mut Features) {
    loop {
        match (toks.get(i), toks.get(i + 1)) {
            (Some(Token::Word(name)), Some(Token::Eq)) => {
                out.add("setting", &name.value);
                // Skip the value up to a comma at this level.
                i += 2;
                let mut depth = 0usize;
                while let Some(t) = toks.get(i) {
                    match t {
                        Token::LParen => depth += 1,
                        Token::RParen if depth == 0 => return,
                        Token::RParen => depth -= 1,
                        Token::Comma if depth == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                if toks.get(i).is_none() {
                    return;
                }
                i += 1;
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_a_trailing_select_format() {
        assert_eq!(strip_trailing_format("SELECT 1 FORMAT JSON;"), "SELECT 1");
        assert_eq!(
            strip_trailing_format("INSERT INTO t FORMAT JSONEachRow"),
            "INSERT INTO t FORMAT JSONEachRow"
        );
    }

    #[test]
    fn settings_names_are_read_up_to_the_next_clause() {
        let f = lexical_extras("SELECT 1 SETTINGS a = 1, b = (1, 2), c = 'x' FORMAT JSON");
        for want in ["setting:a", "setting:b", "setting:c", "format:JSON"] {
            assert!(f.contains(want), "{want} in {f:?}");
        }
    }
}
