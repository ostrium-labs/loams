//! Temporal's visibility query language, parsed (D2074).
//!
//! The grammar is the SQL-like subset Temporal's list filters accept:
//!
//! ```text
//! query     = [ expr ] [ "ORDER BY" order { "," order } ] [ "GROUP BY" name { "," name } ]
//! expr      = and { "OR" and }
//! and       = unary { "AND" unary }
//! unary     = "NOT" unary | "(" expr ")" | predicate
//! predicate = name op value
//!           | name [ "NOT" ] "IN" "(" value { "," value } ")"
//!           | name [ "NOT" ] "BETWEEN" value "AND" value
//!           | name [ "NOT" ] "STARTS_WITH" value
//!           | name "IS" [ "NOT" ] "NULL"
//! op        = "=" | "!=" | "<>" | "<" | "<=" | ">" | ">="
//! order     = name [ "ASC" | "DESC" ]
//! name      = identifier | `back-quoted`
//! value     = 'string' | "string" | number | TRUE | FALSE
//! ```
//!
//! Keywords are case-insensitive. Strings take `\` escapes and a doubled
//! quote. The parser only builds the syntax tree; [`crate::filter`] checks
//! names and types against the namespace's search attributes.

use std::fmt;

/// A literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lit::Str(s) => write!(f, "{s:?}"),
            Lit::Int(i) => write!(f, "{i}"),
            Lit::Float(x) => write!(f, "{x}"),
            Lit::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    /// The SQL spelling.
    pub fn sql(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "<>",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// The syntax tree of a filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp {
        name: String,
        op: CmpOp,
        value: Lit,
    },
    In {
        name: String,
        values: Vec<Lit>,
        negated: bool,
    },
    Between {
        name: String,
        low: Lit,
        high: Lit,
        negated: bool,
    },
    StartsWith {
        name: String,
        prefix: Lit,
        negated: bool,
    },
    IsNull {
        name: String,
        negated: bool,
    },
}

/// One `ORDER BY` term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBy {
    pub name: String,
    pub descending: bool,
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub filter: Option<Expr>,
    pub order_by: Vec<OrderBy>,
    pub group_by: Vec<String>,
}

/// Why a query does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid query at offset {offset}: {message}")]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    /// A back-quoted name: never a keyword.
    Quoted(String),
    Str(String),
    Int(i64),
    Float(f64),
    Op(CmpOp),
    LParen,
    RParen,
    Comma,
}

fn err<T>(offset: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        offset,
        message: message.into(),
    })
}

fn lex(input: &str) -> Result<Vec<(usize, Tok)>, ParseError> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push((at, Tok::LParen));
                i += 1;
            }
            ')' => {
                out.push((at, Tok::RParen));
                i += 1;
            }
            ',' => {
                out.push((at, Tok::Comma));
                i += 1;
            }
            '=' => {
                out.push((at, Tok::Op(CmpOp::Eq)));
                i += 1;
            }
            '!' => {
                if chars.get(i + 1).map(|x| x.1) == Some('=') {
                    out.push((at, Tok::Op(CmpOp::Ne)));
                    i += 2;
                } else {
                    return err(at, "expected != ");
                }
            }
            '<' => match chars.get(i + 1).map(|x| x.1) {
                Some('=') => {
                    out.push((at, Tok::Op(CmpOp::Le)));
                    i += 2;
                }
                Some('>') => {
                    out.push((at, Tok::Op(CmpOp::Ne)));
                    i += 2;
                }
                _ => {
                    out.push((at, Tok::Op(CmpOp::Lt)));
                    i += 1;
                }
            },
            '>' => {
                if chars.get(i + 1).map(|x| x.1) == Some('=') {
                    out.push((at, Tok::Op(CmpOp::Ge)));
                    i += 2;
                } else {
                    out.push((at, Tok::Op(CmpOp::Gt)));
                    i += 1;
                }
            }
            '\'' | '"' | '`' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                loop {
                    let Some(&(_, ch)) = chars.get(i) else {
                        return err(at, "unterminated quoted text");
                    };
                    if ch == '\\' && quote != '`' {
                        let Some(&(_, next)) = chars.get(i + 1) else {
                            return err(at, "unterminated quoted text");
                        };
                        s.push(match next {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                        i += 2;
                    } else if ch == quote {
                        if chars.get(i + 1).map(|x| x.1) == Some(quote) {
                            s.push(quote);
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        s.push(ch);
                        i += 1;
                    }
                }
                out.push((
                    at,
                    if quote == '`' {
                        Tok::Quoted(s)
                    } else {
                        Tok::Str(s)
                    },
                ));
            }
            c if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' => {
                let start = i;
                i += 1;
                while let Some(&(_, ch)) = chars.get(i) {
                    // A sign continues a number only after an exponent's `e`.
                    let exponent_sign = (ch == '-' || ch == '+')
                        && matches!(chars.get(i - 1).map(|x| x.1), Some('e' | 'E'));
                    if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || exponent_sign {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let text: String = chars[start..i].iter().map(|x| x.1).collect();
                if let Ok(v) = text.parse::<i64>() {
                    out.push((at, Tok::Int(v)));
                } else if let Ok(v) = text.parse::<f64>()
                    && v.is_finite()
                {
                    out.push((at, Tok::Float(v)));
                } else {
                    return err(at, format!("{text:?} is not a number"));
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while let Some(&(_, ch)) = chars.get(i) {
                    if ch.is_alphanumeric() || ch == '_' || ch == '.' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let text: String = chars[start..i].iter().map(|x| x.1).collect();
                out.push((at, Tok::Ident(text)));
            }
            other => return err(at, format!("unexpected character {other:?}")),
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(usize, Tok)>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.1)
    }

    fn offset(&self) -> usize {
        self.toks.get(self.pos).map_or(self.end, |t| t.0)
    }

    fn keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn keyword_at(&self, n: usize, kw: &str) -> bool {
        matches!(self.toks.get(self.pos + n).map(|t| &t.1), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn eat(&mut self, kw: &str) -> bool {
        if self.keyword(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kw: &str) -> Result<(), ParseError> {
        if self.eat(kw) {
            Ok(())
        } else {
            err(self.offset(), format!("expected {kw}"))
        }
    }

    fn expect_tok(&mut self, t: Tok, what: &str) -> Result<(), ParseError> {
        if self.peek() == Some(&t) {
            self.pos += 1;
            Ok(())
        } else {
            err(self.offset(), format!("expected {what}"))
        }
    }

    fn name(&mut self) -> Result<String, ParseError> {
        match self.peek().cloned() {
            Some(Tok::Quoted(s)) => {
                self.pos += 1;
                Ok(s)
            }
            Some(Tok::Ident(s)) if !is_reserved(&s) => {
                self.pos += 1;
                Ok(s)
            }
            _ => err(self.offset(), "expected a search attribute name"),
        }
    }

    fn value(&mut self) -> Result<Lit, ParseError> {
        let at = self.offset();
        let v = match self.peek().cloned() {
            Some(Tok::Str(s)) => Lit::Str(s),
            Some(Tok::Int(i)) => Lit::Int(i),
            Some(Tok::Float(x)) => Lit::Float(x),
            Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("true") => Lit::Bool(true),
            Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("false") => Lit::Bool(false),
            _ => return err(at, "expected a value"),
        };
        self.pos += 1;
        Ok(v)
    }

    fn expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.and()?;
        while self.eat("or") {
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        while self.eat("and") {
            let right = self.unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.eat("not") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.peek() == Some(&Tok::LParen) {
            self.pos += 1;
            let e = self.expr()?;
            self.expect_tok(Tok::RParen, ")")?;
            return Ok(e);
        }
        self.predicate()
    }

    fn predicate(&mut self) -> Result<Expr, ParseError> {
        let name = self.name()?;
        if let Some(Tok::Op(op)) = self.peek().cloned() {
            self.pos += 1;
            let value = self.value()?;
            return Ok(Expr::Cmp { name, op, value });
        }
        if self.eat("is") {
            let negated = self.eat("not");
            self.expect("null")?;
            return Ok(Expr::IsNull { name, negated });
        }
        let negated = self.eat("not");
        if self.eat("in") {
            self.expect_tok(Tok::LParen, "(")?;
            let mut values = vec![self.value()?];
            while self.peek() == Some(&Tok::Comma) {
                self.pos += 1;
                values.push(self.value()?);
            }
            self.expect_tok(Tok::RParen, ")")?;
            return Ok(Expr::In {
                name,
                values,
                negated,
            });
        }
        if self.eat("between") {
            let low = self.value()?;
            self.expect("and")?;
            let high = self.value()?;
            return Ok(Expr::Between {
                name,
                low,
                high,
                negated,
            });
        }
        if self.eat("starts_with") {
            let prefix = self.value()?;
            return Ok(Expr::StartsWith {
                name,
                prefix,
                negated,
            });
        }
        err(
            self.offset(),
            format!(
                "expected an operator after {name:?} (=, !=, <, <=, >, >=, IN, BETWEEN, STARTS_WITH, IS NULL)"
            ),
        )
    }
}

const RESERVED: &[&str] = &[
    "and",
    "or",
    "not",
    "in",
    "between",
    "is",
    "null",
    "order",
    "group",
    "by",
    "asc",
    "desc",
    "starts_with",
    "true",
    "false",
];

fn is_reserved(s: &str) -> bool {
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(s))
}

/// Parses a query. The empty (or blank) query has no filter.
pub fn parse(input: &str) -> Result<Query, ParseError> {
    let toks = lex(input)?;
    let mut p = Parser {
        toks,
        pos: 0,
        end: input.len(),
    };
    let mut q = Query::default();
    let at_clause =
        |p: &Parser| (p.keyword("order") || p.keyword("group")) && p.keyword_at(1, "by");
    if p.peek().is_some() && !at_clause(&p) {
        q.filter = Some(p.expr()?);
    }
    loop {
        if p.keyword("order") && p.keyword_at(1, "by") {
            if !q.order_by.is_empty() {
                return err(p.offset(), "ORDER BY appears twice");
            }
            p.pos += 2;
            loop {
                let name = p.name()?;
                let descending = if p.eat("desc") {
                    true
                } else {
                    p.eat("asc");
                    false
                };
                q.order_by.push(OrderBy { name, descending });
                if p.peek() == Some(&Tok::Comma) {
                    p.pos += 1;
                } else {
                    break;
                }
            }
        } else if p.keyword("group") && p.keyword_at(1, "by") {
            if !q.group_by.is_empty() {
                return err(p.offset(), "GROUP BY appears twice");
            }
            p.pos += 2;
            loop {
                q.group_by.push(p.name()?);
                if p.peek() == Some(&Tok::Comma) {
                    p.pos += 1;
                } else {
                    break;
                }
            }
        } else {
            break;
        }
    }
    if p.peek().is_some() {
        return err(p.offset(), "unexpected text after the query");
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(name: &str, op: CmpOp, value: Lit) -> Expr {
        Expr::Cmp {
            name: name.into(),
            op,
            value,
        }
    }

    fn s(v: &str) -> Lit {
        Lit::Str(v.into())
    }

    #[test]
    fn empty_and_blank() {
        assert_eq!(parse("").unwrap(), Query::default());
        assert_eq!(parse("   ").unwrap(), Query::default());
    }

    #[test]
    fn comparisons_and_quotes() {
        let q = parse(r#"WorkflowType = 'order' AND ExecutionStatus != "Running""#).unwrap();
        assert_eq!(
            q.filter.unwrap(),
            Expr::And(
                Box::new(cmp("WorkflowType", CmpOp::Eq, s("order"))),
                Box::new(cmp("ExecutionStatus", CmpOp::Ne, s("Running"))),
            )
        );
        let q = parse("`Custom Field` <> 'it''s' and X>=-3 and Y < 2.5e3").unwrap();
        let Some(Expr::And(left, y)) = q.filter else {
            panic!("and")
        };
        assert_eq!(*y, cmp("Y", CmpOp::Lt, Lit::Float(2500.0)));
        let Expr::And(a, x) = *left else {
            panic!("and")
        };
        assert_eq!(*a, cmp("Custom Field", CmpOp::Ne, s("it's")));
        assert_eq!(*x, cmp("X", CmpOp::Ge, Lit::Int(-3)));
        assert_eq!(
            parse(r"A = 'a\'b'").unwrap().filter.unwrap(),
            cmp("A", CmpOp::Eq, s("a'b"))
        );
    }

    #[test]
    fn precedence_and_parentheses() {
        // AND binds tighter than OR.
        let q = parse("A = 1 OR B = 2 AND C = 3").unwrap().filter.unwrap();
        assert_eq!(
            q,
            Expr::Or(
                Box::new(cmp("A", CmpOp::Eq, Lit::Int(1))),
                Box::new(Expr::And(
                    Box::new(cmp("B", CmpOp::Eq, Lit::Int(2))),
                    Box::new(cmp("C", CmpOp::Eq, Lit::Int(3))),
                )),
            )
        );
        let q = parse("(A = 1 OR B = 2) AND NOT C = 3")
            .unwrap()
            .filter
            .unwrap();
        assert!(matches!(q, Expr::And(ref l, ref r)
            if matches!(**l, Expr::Or(..)) && matches!(**r, Expr::Not(..))));
    }

    #[test]
    fn in_between_starts_with_null() {
        assert_eq!(
            parse("WorkflowId in ('a', 'b')").unwrap().filter.unwrap(),
            Expr::In {
                name: "WorkflowId".into(),
                values: vec![s("a"), s("b")],
                negated: false
            }
        );
        assert_eq!(
            parse("StartTime NOT BETWEEN '2026-01-01T00:00:00Z' AND '2026-02-01T00:00:00Z'")
                .unwrap()
                .filter
                .unwrap(),
            Expr::Between {
                name: "StartTime".into(),
                low: s("2026-01-01T00:00:00Z"),
                high: s("2026-02-01T00:00:00Z"),
                negated: true
            }
        );
        assert_eq!(
            parse("WorkflowType STARTS_WITH 'pay'")
                .unwrap()
                .filter
                .unwrap(),
            Expr::StartsWith {
                name: "WorkflowType".into(),
                prefix: s("pay"),
                negated: false
            }
        );
        assert_eq!(
            parse("CloseTime is not null").unwrap().filter.unwrap(),
            Expr::IsNull {
                name: "CloseTime".into(),
                negated: true
            }
        );
        assert_eq!(
            parse("B = true").unwrap().filter.unwrap(),
            cmp("B", CmpOp::Eq, Lit::Bool(true))
        );
    }

    #[test]
    fn order_and_group() {
        let q = parse("WorkflowType = 'x' ORDER BY StartTime DESC, WorkflowId").unwrap();
        assert!(q.filter.is_some());
        assert_eq!(
            q.order_by,
            vec![
                OrderBy {
                    name: "StartTime".into(),
                    descending: true
                },
                OrderBy {
                    name: "WorkflowId".into(),
                    descending: false
                },
            ]
        );
        let q = parse("GROUP BY ExecutionStatus").unwrap();
        assert!(q.filter.is_none());
        assert_eq!(q.group_by, vec!["ExecutionStatus".to_string()]);
        let q = parse("order by CloseTime asc").unwrap();
        assert_eq!(q.order_by.len(), 1);
    }

    #[test]
    fn errors_point_at_the_problem() {
        for bad in [
            "WorkflowId",
            "WorkflowId = ",
            "= 'x'",
            "A = 'x' AND",
            "A IN ()",
            "A IN ('x'",
            "A BETWEEN 1",
            "(A = 1",
            "A = 'unterminated",
            "A = 1 B = 2",
            "A ! 1",
            "A = 1 ORDER BY",
            "A = 1 ORDER BY X ORDER BY Y",
            "and = 1",
            "A = 1 # comment",
            "A = 1.2.3",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
        let e = parse("A = 1 B = 2").unwrap_err();
        assert_eq!(e.offset, 6);
    }
}
