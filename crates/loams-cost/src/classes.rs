//! Query-class rules in Rust (design §84 §7.4): the same language
//! `loams_meter_house::cost_sql::compile_condition` compiles for the House,
//! evaluated here over an event's data (the CLI's offline mode) and parsed
//! when `costs.yaml` is applied, so a bad rule is refused early.
//!
//! `field op value` clauses joined by `and`; `field` is `[a-z_][a-z0-9_]*`,
//! `op` one of `= != < <= > >= in`, `value` a number, a single-quoted string
//! (`''` for a quote) or a parenthesised list. Numbers compare as decimals;
//! strings compare with `=`, `!=` and `in` only.

use loams_meter_core::Amount;
use serde_json::{Map, Value as Json};

use crate::CostError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    Num(Amount),
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clause {
    Cmp {
        field: String,
        op: Op,
        value: Literal,
    },
    In {
        field: String,
        values: Vec<Literal>,
    },
}

/// A parsed condition: every clause must hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition(pub Vec<Clause>);

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Num(String),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn bad(m: impl Into<String>) -> CostError {
    CostError::invalid("classes", m)
}

fn lex(text: &str) -> Result<Vec<Tok>, CostError> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_lowercase() || ch == '_' {
            let s = i;
            while i < c.len() && (c[i].is_ascii_lowercase() || c[i].is_ascii_digit() || c[i] == '_')
            {
                i += 1;
            }
            out.push(Tok::Ident(c[s..i].iter().collect()));
        } else if ch.is_ascii_digit() || ch == '-' {
            let s = i;
            i += 1;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(c[s..i].iter().collect()));
        } else if ch == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err(bad("unterminated string")),
                    Some('\'') if c.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(x) => {
                        s.push(*x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(s));
        } else if ch == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if ch == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else if ch == ',' {
            out.push(Tok::Comma);
            i += 1;
        } else {
            let two: String = c[i..c.len().min(i + 2)].iter().collect();
            let op = match two.as_str() {
                "<=" => "<=",
                ">=" => ">=",
                "!=" => "!=",
                _ => match ch {
                    '<' => "<",
                    '>' => ">",
                    '=' => "=",
                    other => return Err(bad(format!("unexpected {other:?}"))),
                },
            };
            i += op.len();
            out.push(Tok::Op(op));
        }
    }
    Ok(out)
}

fn literal(t: Option<&Tok>) -> Result<Literal, CostError> {
    match t {
        Some(Tok::Num(n)) => Amount::parse(n)
            .map(Literal::Num)
            .map_err(|_| bad(format!("bad number {n:?}"))),
        Some(Tok::Str(s)) => Ok(Literal::Str(s.clone())),
        other => Err(bad(format!(
            "expected a number or a string, found {other:?}"
        ))),
    }
}

impl Condition {
    pub fn parse(when: &str) -> Result<Condition, CostError> {
        let t = lex(when)?;
        let mut clauses = Vec::new();
        let mut i = 0;
        loop {
            let Some(Tok::Ident(field)) = t.get(i) else {
                return Err(bad("a clause starts with a field name"));
            };
            if field == "and" || field == "in" {
                return Err(bad(format!("{field:?} is not a field name")));
            }
            i += 1;
            match t.get(i) {
                Some(Tok::Op(o)) => {
                    let value = literal(t.get(i + 1))?;
                    let op = match *o {
                        "=" => Op::Eq,
                        "!=" => Op::Ne,
                        "<" => Op::Lt,
                        "<=" => Op::Le,
                        ">" => Op::Gt,
                        _ => Op::Ge,
                    };
                    if matches!(value, Literal::Str(_)) && !matches!(op, Op::Eq | Op::Ne) {
                        return Err(bad(format!("{o} compares numbers only")));
                    }
                    clauses.push(Clause::Cmp {
                        field: field.clone(),
                        op,
                        value,
                    });
                    i += 2;
                }
                Some(Tok::Ident(kw)) if kw == "in" => {
                    if t.get(i + 1) != Some(&Tok::LParen) {
                        return Err(bad("in takes a parenthesised list"));
                    }
                    i += 2;
                    let mut values = Vec::new();
                    loop {
                        let v = literal(t.get(i))?;
                        if values.first().is_some_and(|f: &Literal| {
                            std::mem::discriminant(f) != std::mem::discriminant(&v)
                        }) {
                            return Err(bad("a list mixes numbers and strings"));
                        }
                        values.push(v);
                        i += 1;
                        match t.get(i) {
                            Some(Tok::Comma) => i += 1,
                            Some(Tok::RParen) => {
                                i += 1;
                                break;
                            }
                            _ => return Err(bad("expected , or ) in a list")),
                        }
                    }
                    clauses.push(Clause::In {
                        field: field.clone(),
                        values,
                    });
                }
                _ => return Err(bad(format!("expected an operator after {field:?}"))),
            }
            match t.get(i) {
                None => break,
                Some(Tok::Ident(kw)) if kw == "and" => i += 1,
                Some(other) => return Err(bad(format!("expected and, found {other:?}"))),
            }
        }
        if clauses.len() > 16 {
            return Err(bad("at most 16 clauses"));
        }
        Ok(Condition(clauses))
    }

    /// Whether `data` meets every clause. A missing or non-numeric field
    /// fails a numeric comparison (as `NULL` does in the House).
    pub fn eval(&self, data: &Map<String, Json>) -> bool {
        let text = |f: &str| match data.get(f) {
            Some(Json::String(s)) => Some(s.clone()),
            Some(Json::Null) | None => None,
            Some(other) => Some(other.to_string()),
        };
        let num = |f: &str| match data.get(f) {
            Some(Json::String(t)) => Amount::parse(t.trim()).ok(),
            Some(v) => Amount::from_json(v),
            None => None,
        };
        let matches = |field: &str, lit: &Literal| match lit {
            Literal::Num(n) => num(field) == Some(*n),
            Literal::Str(s) => text(field).as_deref().unwrap_or("") == s,
        };
        self.0.iter().all(|c| match c {
            Clause::In { field, values } => values.iter().any(|v| matches(field, v)),
            Clause::Cmp { field, op, value } => match (op, value) {
                (Op::Eq, v) => matches(field, v),
                (Op::Ne, v) => !matches(field, v),
                (op, Literal::Num(n)) => num(field).is_some_and(|x| match op {
                    Op::Lt => x < *n,
                    Op::Le => x <= *n,
                    Op::Gt => x > *n,
                    _ => x >= *n,
                }),
                _ => false,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(j: &str) -> Map<String, Json> {
        serde_json::from_str(j).expect("json")
    }

    #[test]
    fn evaluates_like_the_house() {
        let c = Condition::parse("bytes_scanned >= 1073741824 and code = '0'").expect("parse");
        assert!(c.eval(&data(r#"{"bytes_scanned": 2147483648, "code": 0}"#)));
        assert!(!c.eval(&data(r#"{"bytes_scanned": 10, "code": 0}"#)));
        assert!(!c.eval(&data(r#"{"code": 0}"#)));
        let c = Condition::parse("tier in ('t0', 't1')").expect("parse");
        assert!(c.eval(&data(r#"{"tier": "t1"}"#)));
        assert!(!c.eval(&data(r#"{"tier": "t2"}"#)));
        let c = Condition::parse("rows_read <= 1000").expect("parse");
        assert!(c.eval(&data(r#"{"rows_read": "1000"}"#)));
    }

    #[test]
    fn refuses_what_the_house_compiler_refuses() {
        for bad in [
            "x > 1; DROP",
            "x > 1 or y = 1",
            "X > 1",
            "x > 'a'",
            "x =",
            "x > 1 and",
            "x in (1, 'a')",
        ] {
            assert!(Condition::parse(bad).is_err(), "{bad}");
        }
    }
}
