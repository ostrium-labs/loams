//! Metadata statements (plan HE1 Task 38): `SHOW TABLES`, `SHOW DATABASES`,
//! `SHOW CREATE`, `DESCRIBE` and `EXISTS`.
//!
//! sqlparser 0.62 reads these in MySQL's and Snowflake's grammars, not
//! ClickHouse's (`SHOW TABLES FROM db NOT ILIKE 'x%' LIMIT 3`, `EXISTS
//! DICTIONARY d`, `SHOW CREATE t` without `TABLE`), so they are read here, on
//! sqlparser's tokens, after the lexical pre-pass took the trailing `SETTINGS`
//! and `FORMAT` out ([`crate::lex`]). The grammar is ClickHouse's
//! (`ParserShowTablesQuery`, `ParserTablePropertiesQuery`).
//!
//! `SHOW TABLES` and `SHOW DATABASES` become the `SELECT` ClickHouse rewrites
//! them into (`InterpreterShowTablesQuery`; [`Meta::show_select`]) and are
//! planned as queries over `system.tables` and `system.databases`. The others
//! are answered by the engine from its catalog (`loams-house-engine`'s
//! `system::meta`).

use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};
use loams_house_types::ChError;
use loams_house_types::ty::quote;

use crate::lex::Offsets;

/// A database object as a statement names it: `[db.]name`, unquoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectName {
    /// The database, when the statement names one.
    pub database: Option<String>,
    /// The object's name.
    pub name: String,
}

impl ObjectName {
    /// The database the object is in, `current` when the name has none.
    pub fn database_or<'a>(&'a self, current: &'a str) -> &'a str {
        self.database.as_deref().unwrap_or(current)
    }
}

/// What kind of object `SHOW CREATE` or `EXISTS` asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// `TABLE`, or nothing: any table, views included.
    Table,
    /// `VIEW`: only views.
    View,
    /// `DICTIONARY`: only dictionaries (the House has none).
    Dictionary,
    /// `DATABASE`.
    Database,
}

/// `[NOT] LIKE 'p'` / `[NOT] ILIKE 'p'`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Like {
    /// `NOT LIKE`.
    pub not: bool,
    /// `ILIKE`.
    pub insensitive: bool,
    /// The pattern.
    pub pattern: String,
}

/// A `SHOW` list's filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowFilter {
    /// Every row.
    None,
    /// `[NOT] [I]LIKE 'pattern'` on the name.
    Like(Like),
    /// `WHERE expr`, the expression's text as written.
    Where(String),
}

/// A metadata statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Meta {
    /// `SHOW [FULL] [TEMPORARY] TABLES [FROM|IN db] [filter] [LIMIT n]`.
    ShowTables {
        /// `FULL`: the engine too.
        full: bool,
        /// `TEMPORARY`: the session's temporary tables.
        temporary: bool,
        /// `FROM db`.
        database: Option<String>,
        /// The filter.
        filter: ShowFilter,
        /// `LIMIT n`.
        limit: Option<u64>,
    },
    /// `SHOW DATABASES [[NOT] [I]LIKE 'p'] [LIMIT n]`.
    ShowDatabases {
        /// The filter (`LIKE` only).
        filter: ShowFilter,
        /// `LIMIT n`.
        limit: Option<u64>,
    },
    /// `SHOW CREATE [TEMPORARY] [TABLE|VIEW|DICTIONARY|DATABASE] name`.
    ShowCreate {
        /// What is asked about.
        kind: ObjectKind,
        /// The object.
        object: ObjectName,
    },
    /// `DESC|DESCRIBE [TABLE] [db.]t`.
    Describe {
        /// The table.
        object: ObjectName,
    },
    /// `EXISTS [TEMPORARY] [TABLE|VIEW|DICTIONARY|DATABASE] name`.
    Exists {
        /// What is asked about.
        kind: ObjectKind,
        /// `TEMPORARY`: a temporary table (the House's sessions have none
        /// the engine sees, so the answer is `0`).
        temporary: bool,
        /// The object.
        object: ObjectName,
    },
}

impl Meta {
    /// The `SELECT` ClickHouse rewrites a `SHOW TABLES` or `SHOW DATABASES`
    /// into (`InterpreterShowTablesQuery::getRewrittenQuery`), with the
    /// database it lists (whose existence ClickHouse checks first, `81`);
    /// `None` for the other statements.
    pub fn show_select(&self, current: &str) -> Option<(String, Option<String>)> {
        let like = |l: &Like, column: &str| {
            format!(
                "{column} {}{} {}",
                if l.not { "NOT " } else { "" },
                if l.insensitive { "ILIKE" } else { "LIKE" },
                quote(&l.pattern)
            )
        };
        match self {
            Meta::ShowDatabases { filter, limit } => {
                let mut q = "SELECT name FROM system.databases".to_string();
                match filter {
                    ShowFilter::None => {}
                    ShowFilter::Like(l) => q.push_str(&format!(" WHERE {}", like(l, "name"))),
                    ShowFilter::Where(e) => q.push_str(&format!(" WHERE ({e})")),
                }
                q.push_str(" ORDER BY name");
                if let Some(n) = limit {
                    q.push_str(&format!(" LIMIT {n}"));
                }
                Some((q, None))
            }
            Meta::ShowTables {
                full,
                temporary,
                database,
                filter,
                limit,
            } => {
                let columns = if *full { "name, engine" } else { "name" };
                let (mut q, listed) = if *temporary {
                    (
                        format!("SELECT {columns} FROM system.tables WHERE is_temporary"),
                        None,
                    )
                } else {
                    let db = database.clone().unwrap_or_else(|| current.to_string());
                    (
                        format!(
                            "SELECT {columns} FROM system.tables WHERE database = {}",
                            quote(&db)
                        ),
                        Some(db),
                    )
                };
                match filter {
                    ShowFilter::None => {}
                    ShowFilter::Like(l) => q.push_str(&format!(" AND {}", like(l, "name"))),
                    ShowFilter::Where(e) => q.push_str(&format!(" AND ({e})")),
                }
                q.push_str(" ORDER BY name");
                if let Some(n) = limit {
                    q.push_str(&format!(" LIMIT {n}"));
                }
                Some((q, listed))
            }
            Meta::ShowCreate { .. } | Meta::Describe { .. } | Meta::Exists { .. } => None,
        }
    }

    /// The tables the statement names, `[db, t]` or `[t]`, for the front's
    /// pins (plan HE1 Task 39).
    pub fn tables(&self) -> Vec<Vec<String>> {
        let parts = |o: &ObjectName| match &o.database {
            Some(db) => vec![db.clone(), o.name.clone()],
            None => vec![o.name.clone()],
        };
        match self {
            Meta::ShowCreate { kind, object } | Meta::Exists { kind, object, .. }
                if *kind != ObjectKind::Database =>
            {
                vec![parts(object)]
            }
            Meta::Describe { object } => vec![parts(object)],
            _ => Vec::new(),
        }
    }
}

fn syntax(message: impl Into<String>) -> ChError {
    ChError::syntax_error(message)
}

/// The statement's significant tokens and its text, for slicing.
struct Tokens<'a> {
    sql: &'a str,
    tokens: Vec<TokenWithSpan>,
    offsets: Offsets,
    at: usize,
}

impl<'a> Tokens<'a> {
    fn new(sql: &'a str) -> Result<Self, ChError> {
        let tokens = Tokenizer::new(&ClickHouseDialect {}, sql)
            .tokenize_with_location()
            .map_err(|e| syntax(e.to_string()))?
            .into_iter()
            .filter(|t| !matches!(t.token, Token::Whitespace(_) | Token::EOF))
            .collect();
        Ok(Self {
            sql,
            tokens,
            offsets: Offsets::new(sql),
            at: 0,
        })
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|t| &t.token)
    }

    /// Whether the next token is the bare word `word` (any case).
    fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(word))
    }

    /// Takes the bare word `word`, if it is next.
    fn word(&mut self, word: &str) -> bool {
        if self.is_word(word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// The end of the statement: nothing, or one `;`.
    fn at_end(&self) -> bool {
        match self.tokens.get(self.at..) {
            None | Some([]) => true,
            Some([t]) => matches!(t.token, Token::SemiColon),
            _ => false,
        }
    }

    fn expect_end(&self) -> Result<(), ChError> {
        if self.at_end() {
            Ok(())
        } else {
            Err(syntax(format!(
                "Syntax error: unexpected {} at the end of the statement",
                self.peek().map(ToString::to_string).unwrap_or_default()
            )))
        }
    }

    /// An identifier: a bare or quoted word.
    fn ident(&mut self) -> Result<String, ChError> {
        match self.peek() {
            Some(Token::Word(w)) => {
                let v = w.value.clone();
                self.at += 1;
                Ok(v)
            }
            other => Err(syntax(format!(
                "Syntax error: expected an identifier, got {}",
                other
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "the end".into())
            ))),
        }
    }

    /// `[db.]name`.
    fn object(&mut self) -> Result<ObjectName, ChError> {
        let first = self.ident()?;
        if matches!(self.peek(), Some(Token::Period)) {
            self.at += 1;
            let name = self.ident()?;
            Ok(ObjectName {
                database: Some(first),
                name,
            })
        } else {
            Ok(ObjectName {
                database: None,
                name: first,
            })
        }
    }

    fn string(&mut self) -> Result<String, ChError> {
        match self.peek() {
            Some(Token::SingleQuotedString(s)) => {
                let s = s.clone();
                self.at += 1;
                Ok(s)
            }
            other => Err(syntax(format!(
                "Syntax error: expected a string literal, got {}",
                other
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "the end".into())
            ))),
        }
    }

    fn number(&mut self) -> Result<u64, ChError> {
        match self.peek() {
            Some(Token::Number(n, _)) => {
                let v = n
                    .parse()
                    .map_err(|_| syntax(format!("Syntax error: LIMIT {n} is not a count")))?;
                self.at += 1;
                Ok(v)
            }
            other => Err(syntax(format!(
                "Syntax error: expected a number, got {}",
                other
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "the end".into())
            ))),
        }
    }

    /// `[NOT] LIKE 'p'` / `[NOT] ILIKE 'p'`, if next.
    fn like(&mut self) -> Result<Option<Like>, ChError> {
        let start = self.at;
        let not = self.word("NOT");
        let insensitive = if self.word("LIKE") {
            false
        } else if self.word("ILIKE") {
            true
        } else {
            self.at = start;
            return Ok(None);
        };
        Ok(Some(Like {
            not,
            insensitive,
            pattern: self.string()?,
        }))
    }

    /// The text from the next token to the first bare `LIMIT` at bracket
    /// depth 0 (or the end), as written.
    fn expression_text(&mut self) -> Result<String, ChError> {
        let start = self.at;
        let mut depth = 0i32;
        while self.at < self.tokens.len() && !self.at_end() {
            match self.peek() {
                Some(Token::LParen | Token::LBracket) => depth += 1,
                Some(Token::RParen | Token::RBracket) => depth -= 1,
                _ => {}
            }
            if depth == 0 && self.is_word("LIMIT") {
                break;
            }
            self.at += 1;
        }
        if self.at == start {
            return Err(syntax("Syntax error: WHERE without an expression"));
        }
        let first = &self.tokens[start].span.start;
        let last = &self.tokens[self.at - 1].span.end;
        let from = self.offsets.at(first.line, first.column);
        let to = self.offsets.at(last.line, last.column);
        Ok(self.sql[from..to].trim().to_string())
    }
}

/// Reads `sql` (after the pre-pass) as a metadata statement: `Ok(None)` when
/// it is not one.
///
/// # Errors
///
/// `62` for a metadata statement ClickHouse would not parse; `48` for a
/// `SHOW` or `DESCRIBE` form the engine does not answer.
pub fn parse(sql: &str) -> Result<Option<Meta>, ChError> {
    let mut t = Tokens::new(sql)?;
    if t.word("SHOW") {
        return show(&mut t).map(Some);
    }
    if t.word("DESCRIBE") || t.word("DESC") {
        t.word("TABLE");
        if matches!(t.peek(), Some(Token::LParen)) {
            return Err(ChError::unsupported("DESCRIBE of a query"));
        }
        let object = t.object()?;
        if matches!(t.peek(), Some(Token::LParen)) {
            return Err(ChError::unsupported("DESCRIBE of a table function"));
        }
        t.expect_end()?;
        return Ok(Some(Meta::Describe { object }));
    }
    if t.is_word("EXISTS") {
        // `EXISTS (SELECT …)` alone is not a statement ClickHouse runs either;
        // leave it to the query parser's error.
        if matches!(t.tokens.get(1).map(|x| &x.token), Some(Token::LParen)) {
            return Ok(None);
        }
        t.word("EXISTS");
        let temporary = t.word("TEMPORARY");
        let kind = kind(&mut t);
        let object = if kind == ObjectKind::Database {
            ObjectName {
                database: None,
                name: t.ident()?,
            }
        } else {
            t.object()?
        };
        t.expect_end()?;
        return Ok(Some(Meta::Exists {
            kind,
            temporary,
            object,
        }));
    }
    Ok(None)
}

/// `TABLE`, `VIEW`, `DICTIONARY` or `DATABASE`, if next; `Table` otherwise.
fn kind(t: &mut Tokens<'_>) -> ObjectKind {
    if t.word("TABLE") {
        ObjectKind::Table
    } else if t.word("VIEW") {
        ObjectKind::View
    } else if t.word("DICTIONARY") {
        ObjectKind::Dictionary
    } else if t.word("DATABASE") {
        ObjectKind::Database
    } else {
        ObjectKind::Table
    }
}

fn show(t: &mut Tokens<'_>) -> Result<Meta, ChError> {
    if t.word("CREATE") {
        let _temporary = t.word("TEMPORARY");
        let kind = kind(t);
        let object = if kind == ObjectKind::Database {
            ObjectName {
                database: None,
                name: t.ident()?,
            }
        } else {
            t.object()?
        };
        t.expect_end()?;
        return Ok(Meta::ShowCreate { kind, object });
    }
    if t.word("DATABASES") {
        let filter = match t.like()? {
            Some(l) => ShowFilter::Like(l),
            None => ShowFilter::None,
        };
        let limit = if t.word("LIMIT") {
            Some(t.number()?)
        } else {
            None
        };
        t.expect_end()?;
        return Ok(Meta::ShowDatabases { filter, limit });
    }
    let full = t.word("FULL");
    let temporary = t.word("TEMPORARY");
    if t.word("TABLES") {
        let database = if t.word("FROM") || t.word("IN") {
            Some(t.ident()?)
        } else {
            None
        };
        let filter = if let Some(l) = t.like()? {
            ShowFilter::Like(l)
        } else if t.word("WHERE") {
            ShowFilter::Where(t.expression_text()?)
        } else {
            ShowFilter::None
        };
        let limit = if t.word("LIMIT") {
            Some(t.number()?)
        } else {
            None
        };
        t.expect_end()?;
        return Ok(Meta::ShowTables {
            full,
            temporary,
            database,
            filter,
            limit,
        });
    }
    let what = t.peek().map(ToString::to_string).unwrap_or_default();
    Err(ChError::unsupported(format!("SHOW {what}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(sql: &str) -> Meta {
        parse(sql).unwrap().unwrap()
    }

    fn obj(db: Option<&str>, name: &str) -> ObjectName {
        ObjectName {
            database: db.map(str::to_string),
            name: name.to_string(),
        }
    }

    #[test]
    fn queries_are_not_metadata_statements() {
        assert_eq!(parse("SELECT 1").unwrap(), None);
        assert_eq!(parse("WITH 1 AS x SELECT x").unwrap(), None);
        assert_eq!(parse("EXISTS (SELECT 1)").unwrap(), None);
    }

    #[test]
    fn show_tables_reads_as_clickhouse() {
        assert_eq!(
            meta("SHOW TABLES"),
            Meta::ShowTables {
                full: false,
                temporary: false,
                database: None,
                filter: ShowFilter::None,
                limit: None
            }
        );
        assert_eq!(
            meta("show full tables in `db` not ilike 'a%' limit 3;"),
            Meta::ShowTables {
                full: true,
                temporary: false,
                database: Some("db".into()),
                filter: ShowFilter::Like(Like {
                    not: true,
                    insensitive: true,
                    pattern: "a%".into()
                }),
                limit: Some(3)
            }
        );
        let Meta::ShowTables { filter, limit, .. } = meta(
            "SHOW TABLES FROM system WHERE engine LIKE '%System%' AND name IN ('one') LIMIT 2",
        ) else {
            unreachable!()
        };
        assert_eq!(
            filter,
            ShowFilter::Where("engine LIKE '%System%' AND name IN ('one')".into())
        );
        assert_eq!(limit, Some(2));
    }

    #[test]
    fn show_lists_become_clickhouses_selects() {
        let (q, db) = meta("SHOW TABLES LIKE 't%'").show_select("d1").unwrap();
        assert_eq!(
            q,
            "SELECT name FROM system.tables WHERE database = 'd1' AND name LIKE 't%' ORDER BY name"
        );
        assert_eq!(db.as_deref(), Some("d1"));
        let (q, db) = meta("SHOW DATABASES NOT LIKE 'sys%' LIMIT 5")
            .show_select("d1")
            .unwrap();
        assert_eq!(
            q,
            "SELECT name FROM system.databases WHERE name NOT LIKE 'sys%' ORDER BY name LIMIT 5"
        );
        assert_eq!(db, None);
        let (q, _) = meta("SHOW FULL TABLES FROM x WHERE engine = 'View'")
            .show_select("d1")
            .unwrap();
        assert_eq!(
            q,
            "SELECT name, engine FROM system.tables WHERE database = 'x' AND (engine = 'View') ORDER BY name"
        );
        assert!(meta("DESCRIBE t").show_select("d1").is_none());
    }

    #[test]
    fn describe_show_create_and_exists_name_their_object() {
        assert_eq!(
            meta("DESCRIBE TABLE db.t"),
            Meta::Describe {
                object: obj(Some("db"), "t")
            }
        );
        assert_eq!(
            meta("desc `t`"),
            Meta::Describe {
                object: obj(None, "t")
            }
        );
        assert_eq!(
            meta("SHOW CREATE t"),
            Meta::ShowCreate {
                kind: ObjectKind::Table,
                object: obj(None, "t")
            }
        );
        assert_eq!(
            meta("SHOW CREATE VIEW db.v"),
            Meta::ShowCreate {
                kind: ObjectKind::View,
                object: obj(Some("db"), "v")
            }
        );
        assert_eq!(
            meta("SHOW CREATE DATABASE db"),
            Meta::ShowCreate {
                kind: ObjectKind::Database,
                object: obj(None, "db")
            }
        );
        assert_eq!(
            meta("EXISTS t"),
            Meta::Exists {
                kind: ObjectKind::Table,
                temporary: false,
                object: obj(None, "t")
            }
        );
        assert_eq!(
            meta("EXISTS DICTIONARY db.d"),
            Meta::Exists {
                kind: ObjectKind::Dictionary,
                temporary: false,
                object: obj(Some("db"), "d")
            }
        );
        assert_eq!(
            meta("EXISTS DATABASE db").tables(),
            Vec::<Vec<String>>::new()
        );
        assert_eq!(
            meta("EXISTS db.t").tables(),
            vec![vec!["db".to_string(), "t".to_string()]]
        );
    }

    #[test]
    fn other_forms_are_refused_with_clickhouse_codes() {
        assert_eq!(parse("SHOW PROCESSLIST").unwrap_err().code, 48);
        assert_eq!(parse("DESCRIBE (SELECT 1)").unwrap_err().code, 48);
        assert_eq!(parse("DESCRIBE numbers(10)").unwrap_err().code, 48);
        assert_eq!(parse("DESCRIBE TABLE t extra").unwrap_err().code, 62);
        assert_eq!(parse("SHOW TABLES LIMIT x").unwrap_err().code, 62);
    }
}
