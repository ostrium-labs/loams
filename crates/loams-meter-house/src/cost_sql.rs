//! The cost drivers' House SQL (design §84 §7; CA1 Task 5), as text.
//!
//! **std only**, like [`crate::sql`], so fabric's real-House test can
//! compile it with `#[path]` beside `sql.rs`.
//!
//! Two queries per UTC day over `_meter.events FINAL`:
//! - [`counter_query`]: counter events (CPU, network, I/O, ops), summed per
//!   (type, subject, tenant, node, engine label, resource, query class);
//! - [`gauge_query`]: gauge events (bytes stored), as byte-seconds: each
//!   stored object's mean sampled size that day × 86,400 (a daily sample
//!   and hourly samples both work).
//!
//! Query classes are operator rules compiled by [`compile_condition`] into
//! a `multiIf` ([`class_expr`]). The rule language is tiny on purpose:
//! `field op value` clauses joined by `and`, where `field` is a JSON key of
//! the event's data, `op` one of `= != < <= > >= in`, and `value` a number,
//! a single-quoted string or a parenthesised list. Anything else is refused,
//! and every name and value is re-emitted through [`quote`], so a rule
//! cannot inject SQL.

use crate::sql::{DATABASE, at64, quote};

/// The node of an event: its `node` data field, else the last segment of
/// its `source` (`loams://<cluster>/<role>/<node>`).
pub const NODE_EXPR: &str = "if(JSON_VALUE(data, '$.node') != '', JSON_VALUE(data, '$.node'), splitByChar('/', source)[-1])";

/// The engine label the node agent sets (§84 §7.3); `''` when absent.
pub const ENGINE_EXPR: &str = "JSON_VALUE(data, '$.engine')";

/// The node agent's resource (a cgroup path); `''` for product events.
pub const RESOURCE_EXPR: &str = "JSON_VALUE(data, '$.resource')";

/// The columns both queries return first, in order.
pub const KEY_COLUMNS: [&str; 7] = [
    "type", "subject", "tenantid", "node", "engine", "resource", "class",
];

/// A query-class rule for the House: the event types it applies to (an
/// engine's types), the class, and the condition text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassRuleSql {
    pub types: Vec<String>,
    pub class: String,
    pub when: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Number(String),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_lowercase() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_lowercase() || chars[i].is_ascii_digit() || chars[i] == '_')
            {
                i += 1;
            }
            out.push(Token::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() || c == '-' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let n: String = chars[start..i].iter().collect();
            if n == "-" || n.matches('.').count() > 1 || n.ends_with('.') {
                return Err(format!("bad number {n:?}"));
            }
            out.push(Token::Number(n));
        } else if c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err("unterminated string".into()),
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            out.push(Token::Str(s));
        } else if c == '(' {
            out.push(Token::LParen);
            i += 1;
        } else if c == ')' {
            out.push(Token::RParen);
            i += 1;
        } else if c == ',' {
            out.push(Token::Comma);
            i += 1;
        } else {
            let two: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let op = match two.as_str() {
                "<=" => "<=",
                ">=" => ">=",
                "!=" => "!=",
                _ => match c {
                    '<' => "<",
                    '>' => ">",
                    '=' => "=",
                    other => return Err(format!("unexpected {other:?}")),
                },
            };
            i += op.len();
            out.push(Token::Op(op));
        }
    }
    Ok(out)
}

fn field_expr(name: &str, numeric: bool) -> String {
    let path = quote(&format!("$.{name}"));
    if numeric {
        format!("toDecimal128OrNull(JSON_VALUE(data, {path}), 9)")
    } else {
        format!("JSON_VALUE(data, {path})")
    }
}

fn literal(t: &Token) -> Result<(String, bool), String> {
    match t {
        Token::Number(n) => Ok((format!("toDecimal128({}, 9)", quote(n)), true)),
        Token::Str(s) => Ok((quote(s), false)),
        other => Err(format!("expected a number or a string, found {other:?}")),
    }
}

/// Compiles a rule's `when` into a ClickHouse boolean expression.
pub fn compile_condition(when: &str) -> Result<String, String> {
    let toks = tokens(when)?;
    let mut clauses = Vec::new();
    let mut i = 0;
    loop {
        let Some(Token::Ident(field)) = toks.get(i) else {
            return Err("a clause starts with a field name".into());
        };
        if field == "and" || field == "in" {
            return Err(format!("{field:?} is not a field name"));
        }
        i += 1;
        match toks.get(i) {
            Some(Token::Op(op)) => {
                let value = toks.get(i + 1).ok_or("a comparison needs a value")?;
                let (v, numeric) = literal(value)?;
                if !numeric && !matches!(*op, "=" | "!=") {
                    return Err(format!("{op} compares numbers only"));
                }
                clauses.push(format!("({} {op} {v})", field_expr(field, numeric)));
                i += 2;
            }
            Some(Token::Ident(kw)) if kw == "in" => {
                if toks.get(i + 1) != Some(&Token::LParen) {
                    return Err("in takes a parenthesised list".into());
                }
                i += 2;
                let mut values = Vec::new();
                let mut numeric = None;
                loop {
                    let (v, n) = literal(toks.get(i).ok_or("unterminated list")?)?;
                    if numeric.is_some_and(|x| x != n) {
                        return Err("a list mixes numbers and strings".into());
                    }
                    numeric = Some(n);
                    values.push(v);
                    i += 1;
                    match toks.get(i) {
                        Some(Token::Comma) => i += 1,
                        Some(Token::RParen) => {
                            i += 1;
                            break;
                        }
                        _ => return Err("expected , or ) in a list".into()),
                    }
                }
                clauses.push(format!(
                    "({} IN ({}))",
                    field_expr(field, numeric.unwrap_or(false)),
                    values.join(", ")
                ));
            }
            _ => return Err(format!("expected an operator after {field:?}")),
        }
        match toks.get(i) {
            None => break,
            Some(Token::Ident(kw)) if kw == "and" => i += 1,
            Some(other) => return Err(format!("expected and, found {other:?}")),
        }
    }
    if clauses.len() > 16 {
        return Err("at most 16 clauses".into());
    }
    Ok(clauses.join(" AND "))
}

/// The class of an event: the producer's `query_class` when set, then the
/// rules in order (each limited to its types), then `default`.
pub fn class_expr(rules: &[ClassRuleSql], default: &str) -> Result<String, String> {
    let mut branches = vec![
        "JSON_VALUE(data, '$.query_class') != ''".to_string(),
        "JSON_VALUE(data, '$.query_class')".to_string(),
    ];
    for r in rules {
        let cond = compile_condition(&r.when).map_err(|e| format!("class {}: {e}", r.class))?;
        let types: Vec<String> = r.types.iter().map(|t| quote(t)).collect();
        if types.is_empty() {
            return Err(format!("class {}: no event types", r.class));
        }
        branches.push(format!("(type IN ({}) AND {cond})", types.join(", ")));
        branches.push(quote(&r.class));
    }
    branches.push(quote(default));
    Ok(format!("multiIf({})", branches.join(", ")))
}

fn types_in(types: &[String]) -> String {
    types
        .iter()
        .map(|t| quote(t))
        .collect::<Vec<_>>()
        .join(", ")
}

fn day_filter(day_start: i64) -> String {
    format!(
        "time >= {} AND time < {}",
        at64(day_start),
        at64(day_start + 86_400)
    )
}

/// Counter events of one day: per key, the event count and the sum of each
/// field (as a nine-decimal string).
pub fn counter_query(day_start: i64, types: &[String], fields: &[String], class: &str) -> String {
    let sums: Vec<String> = fields
        .iter()
        .map(|f| {
            format!(
                "toString(sum(ifNull({}, toDecimal128(0, 9)))) AS {}",
                field_expr(f, true),
                quote_ident(f)
            )
        })
        .collect();
    format!(
        "SELECT type, subject, tenantid, {NODE_EXPR} AS node, {ENGINE_EXPR} AS engine, \
         {RESOURCE_EXPR} AS resource, {class} AS class, count() AS events{}{} \
         FROM {DATABASE}.events FINAL \
         WHERE {} AND type IN ({}) \
         GROUP BY type, subject, tenantid, node, engine, resource, class \
         ORDER BY type, subject, tenantid, node, engine, resource, class \
         FORMAT TabSeparatedWithNames",
        if sums.is_empty() { "" } else { ", " },
        sums.join(", "),
        day_filter(day_start),
        types_in(types),
    )
}

/// The identity of a stored object for gauge averaging.
const GAUGE_IDENTITY: &str = "concat(subject, '|', tenantid, '|', \
     JSON_VALUE(data, '$.database'), '|', JSON_VALUE(data, '$.table'), '|', \
     JSON_VALUE(data, '$.branch'), '|', JSON_VALUE(data, '$.endpoint'), '|', \
     JSON_VALUE(data, '$.repository'), '|', JSON_VALUE(data, '$.namespace'))";

/// Gauge events of one day as byte-seconds per key: each object's mean
/// sampled value × 86,400, summed. `gauges` pairs an event type with its
/// value field.
pub fn gauge_query(day_start: i64, gauges: &[(String, String)]) -> String {
    let value = {
        let mut arms = Vec::new();
        for (t, f) in gauges {
            arms.push(format!("type = {}", quote(t)));
            arms.push(field_expr(f, true));
        }
        arms.push("CAST(NULL, 'Nullable(Decimal(38, 9))')".to_string());
        format!("multiIf({})", arms.join(", "))
    };
    let types: Vec<String> = gauges.iter().map(|(t, _)| t.clone()).collect();
    format!(
        "SELECT type, subject, tenantid, node, engine, resource, '_' AS class, \
         toUInt64(sum(samples)) AS events, toString(sum(byte_seconds)) AS byte_seconds \
         FROM (SELECT type, subject, tenantid, {NODE_EXPR} AS node, {ENGINE_EXPR} AS engine, \
         {RESOURCE_EXPR} AS resource, {GAUGE_IDENTITY} AS object, count() AS samples, \
         sum(ifNull({value}, toDecimal128(0, 9))) * 86400 / count() AS byte_seconds \
         FROM {DATABASE}.events FINAL \
         WHERE {} AND type IN ({}) \
         GROUP BY type, subject, tenantid, node, engine, resource, object) \
         GROUP BY type, subject, tenantid, node, engine, resource \
         ORDER BY type, subject, tenantid, node, engine, resource \
         FORMAT TabSeparatedWithNames",
        day_filter(day_start),
        types_in(&types),
    )
}

/// A column alias from a field name (letters, digits and `_` only, as the
/// catalogue's fields are).
fn quote_ident(name: &str) -> String {
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    format!("`f_{safe}`")
}

/// The column a field's sum comes back as.
pub fn field_column(name: &str) -> String {
    quote_ident(name).trim_matches('`').to_string()
}

/// `TabSeparatedWithNames` rows as (column, value) maps, unescaped.
pub fn parse_rows(text: &str) -> Result<Vec<std::collections::BTreeMap<String, String>>, String> {
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    let names: Vec<String> = header.split('\t').map(crate::sql::unescape_tsv).collect();
    let mut out = Vec::new();
    for (n, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let values: Vec<&str> = line.split('\t').collect();
        if values.len() != names.len() {
            return Err(format!(
                "row {}: {} columns, header has {}",
                n + 1,
                values.len(),
                names.len()
            ));
        }
        out.push(
            names
                .iter()
                .cloned()
                .zip(values.into_iter().map(crate::sql::unescape_tsv))
                .collect(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_rules_compile_to_multiif() {
        let rules = vec![ClassRuleSql {
            types: vec!["io.loams.dev.usage.house.query.v1".into()],
            class: "heavy_scan".into(),
            when: "bytes_scanned >= 10737418240 and code = '0'".into(),
        }];
        let e = class_expr(&rules, "default").expect("expr");
        assert!(
            e.starts_with("multiIf(JSON_VALUE(data, '$.query_class') != ''"),
            "{e}"
        );
        assert!(
            e.contains("(toDecimal128OrNull(JSON_VALUE(data, '$.bytes_scanned'), 9) >= toDecimal128('10737418240', 9))"),
            "{e}"
        );
        assert!(e.contains("(JSON_VALUE(data, '$.code') = '0')"), "{e}");
        assert!(
            e.contains("type IN ('io.loams.dev.usage.house.query.v1')"),
            "{e}"
        );
        assert!(e.ends_with("'heavy_scan', 'default')"), "{e}");
    }

    #[test]
    fn in_lists() {
        let c = compile_condition("tier in ('t0', 't1')").expect("in");
        assert_eq!(c, "(JSON_VALUE(data, '$.tier') IN ('t0', 't1'))");
        let c = compile_condition("rows_read in (1, 2.5)").expect("in");
        assert!(
            c.contains("IN (toDecimal128('1', 9), toDecimal128('2.5', 9))"),
            "{c}"
        );
        assert!(compile_condition("tier in ('t0', 1)").is_err());
    }

    #[test]
    fn class_rule_rejects_injection() {
        for bad in [
            "bytes_scanned > 1; DROP TABLE x",
            "bytes_scanned > 1 or 1 = 1",
            "Bytes > 1",
            "x > 'a'",
            "x = ",
            "x > 1 and",
            "and > 1",
            "x = 'unterminated",
            "x = `y`",
            "x = 1)",
        ] {
            assert!(compile_condition(bad).is_err(), "{bad}");
        }
        // Quotes inside strings are escaped on the way out.
        let c = compile_condition("x = 'a''b\\\\'").expect("ok");
        assert_eq!(c, "(JSON_VALUE(data, '$.x') = 'a\\'b\\\\\\\\')");
    }

    #[test]
    fn driver_sql_groups_by_day_engine_tenant_class() {
        let q = counter_query(
            1_793_232_000,
            &["io.loams.dev.usage.house.query.v1".into()],
            &["cpu_ms".into(), "statements".into()],
            "'_'",
        );
        assert!(q.contains("FROM _meter.events FINAL"), "{q}");
        assert!(
            q.contains("GROUP BY type, subject, tenantid, node, engine, resource, class"),
            "{q}"
        );
        assert!(q.contains("AS `f_cpu_ms`"), "{q}");
        assert!(q.contains("time >= toDateTime64(1793232000"), "{q}");
        assert!(q.ends_with("FORMAT TabSeparatedWithNames"));
        let g = gauge_query(
            1_793_232_000,
            &[("io.loams.dev.usage.house.storage.v1".into(), "bytes".into())],
        );
        assert!(g.contains("* 86400 / count()"), "{g}");
        assert!(g.contains("'_' AS class"), "{g}");
    }

    #[test]
    fn rows_parse() {
        let rows = parse_rows("type\tsubject\tf_cpu_ms\na\\tb\torg_1\t12.5\n").expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["type"], "a\tb");
        assert_eq!(rows[0]["f_cpu_ms"], "12.5");
        assert!(parse_rows("a\tb\n1\n").is_err());
        assert_eq!(field_column("cpu_ms"), "f_cpu_ms");
    }
}
