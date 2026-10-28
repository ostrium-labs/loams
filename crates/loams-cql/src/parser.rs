//! A deliberately bounded CQL subset for single-partition operations.
//!
//! This parser rejects constructs outside its grammar instead of guessing their
//! semantics. It does not check a table's schema: the executor must verify
//! that a SELECT predicate names its actual single-column primary key.

use std::collections::HashSet;
use std::fmt;

/// The maximum statement length accepted by this parser, in bytes.
pub const MAX_STATEMENT_BYTES: usize = 1024 * 1024;

/// A table name, optionally qualified by a keyspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualifiedName {
    /// Keyspace, or `None` to use the connection's current keyspace.
    pub keyspace: Option<String>,
    /// The normalized table identifier.
    pub table: String,
}

/// Supported scalar column types. Collection types require a later milestone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    /// ASCII text.
    Ascii,
    /// Signed 64-bit integer.
    BigInt,
    /// Arbitrary bytes.
    Blob,
    /// Boolean.
    Boolean,
    /// Double-precision floating point.
    Double,
    /// Single-precision floating point.
    Float,
    /// Signed 32-bit integer.
    Int,
    /// Signed 16-bit integer.
    SmallInt,
    /// Signed 8-bit integer.
    TinyInt,
    /// UTF-8 text.
    Text,
    /// Millisecond timestamp.
    Timestamp,
    /// UUID.
    Uuid,
    /// Version-1 UUID.
    TimeUuid,
    /// Variable-length UTF-8 text.
    Varchar,
}

/// A named scalar column in a CREATE TABLE statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    /// The normalized column name.
    pub name: String,
    /// The requested CQL scalar type.
    pub data_type: DataType,
}

/// A supported value expression. Bind positions start at zero per statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// CQL `null`.
    Null,
    /// Boolean literal.
    Boolean(bool),
    /// Signed integer literal.
    Integer(i64),
    /// Finite floating-point literal.
    Float(f64),
    /// Single-quoted string, after CQL quote unescaping.
    String(String),
    /// A positional `?` marker, numbered from zero in statement order.
    Bind(usize),
}

/// SELECT projection; no aliases or expressions are accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectColumns {
    /// Every column.
    All,
    /// Named columns in request order.
    Named(Vec<String>),
}

/// The supported statement forms.
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// CREATE KEYSPACE without replication options.
    CreateKeyspace { name: String, if_not_exists: bool },
    /// Change the connection's keyspace.
    Use { keyspace: String },
    /// Create a table with one primary-key column.
    CreateTable {
        name: QualifiedName,
        columns: Vec<Column>,
        primary_key: String,
        if_not_exists: bool,
    },
    /// Insert scalar values into named columns.
    Insert {
        table: QualifiedName,
        columns: Vec<String>,
        values: Vec<Value>,
    },
    /// Select by equality on one column, which the executor must verify is the key.
    Select {
        table: QualifiedName,
        columns: SelectColumns,
        key_column: String,
        key_value: Value,
    },
    /// Drop one table.
    DropTable {
        name: QualifiedName,
        if_exists: bool,
    },
}

/// A malformed or unsupported CQL statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset into the input where parsing failed.
    pub offset: usize,
    /// Description of the rejected construct.
    pub message: String,
}
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CQL parse error at byte {}: {}",
            self.offset, self.message
        )
    }
}
impl std::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Word(String),
    Ident(String),
    String(String),
    Number(String),
    Bind,
    Left,
    Right,
    Comma,
    Dot,
    Star,
    Equal,
    Semi,
}
#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    offset: usize,
}

fn error(offset: usize, message: impl Into<String>) -> ParseError {
    ParseError {
        offset,
        message: message.into(),
    }
}

fn lex(sql: &str) -> Result<Vec<Token>, ParseError> {
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err(error(MAX_STATEMENT_BYTES, "statement exceeds size limit"));
    }
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let kind = match bytes[i] {
            b'(' => {
                i += 1;
                Kind::Left
            }
            b')' => {
                i += 1;
                Kind::Right
            }
            b',' => {
                i += 1;
                Kind::Comma
            }
            b'.' => {
                i += 1;
                Kind::Dot
            }
            b'*' => {
                i += 1;
                Kind::Star
            }
            b'=' => {
                i += 1;
                Kind::Equal
            }
            b';' => {
                i += 1;
                Kind::Semi
            }
            b'?' => {
                i += 1;
                Kind::Bind
            }
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                let mut value = Vec::new();
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        i += 1;
                        if i < bytes.len() && bytes[i] == quote {
                            value.push(quote);
                            i += 1;
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        value.push(bytes[i]);
                        i += 1;
                    }
                }
                if !closed {
                    return Err(error(start, "unterminated quoted value"));
                }
                let value = String::from_utf8(value).map_err(|_| error(start, "invalid UTF-8"))?;
                if quote == b'"' {
                    if value.is_empty() {
                        return Err(error(start, "empty quoted identifier"));
                    }
                    Kind::Ident(value)
                } else {
                    Kind::String(value)
                }
            }
            b'-' | b'0'..=b'9' => {
                if bytes[i] == b'-' {
                    i += 1;
                }
                let digits = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == digits {
                    return Err(error(start, "expected digits after minus"));
                }
                if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                Kind::Number(sql[start..i].to_owned())
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                Kind::Word(sql[start..i].to_ascii_lowercase())
            }
            _ => return Err(error(start, "unsupported character or syntax")),
        };
        tokens.push(Token {
            kind,
            offset: start,
        });
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    end: usize,
    next_bind: usize,
}
impl Parser {
    fn at(&self) -> usize {
        self.tokens.get(self.index).map_or(self.end, |t| t.offset)
    }
    fn fail<T>(&self, message: &str) -> Result<T, ParseError> {
        Err(error(self.at(), message))
    }
    fn take(&mut self, kind: &Kind) -> bool {
        if self.tokens.get(self.index).is_some_and(|t| &t.kind == kind) {
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, kind: Kind) -> Result<(), ParseError> {
        if self.take(&kind) {
            Ok(())
        } else {
            self.fail(&format!("expected {kind:?}"))
        }
    }
    fn keyword(&mut self, keyword: &str) -> bool {
        self.take(&Kind::Word(keyword.to_owned()))
    }
    fn require_keyword(&mut self, keyword: &str) -> Result<(), ParseError> {
        if self.keyword(keyword) {
            Ok(())
        } else {
            self.fail(&format!("expected {keyword}"))
        }
    }
    fn ident(&mut self) -> Result<String, ParseError> {
        match self.tokens.get(self.index).map(|t| &t.kind) {
            Some(Kind::Word(s) | Kind::Ident(s)) => {
                let s = s.clone();
                self.index += 1;
                Ok(s)
            }
            _ => self.fail("expected identifier"),
        }
    }
    fn name(&mut self) -> Result<QualifiedName, ParseError> {
        let first = self.ident()?;
        if self.take(&Kind::Dot) {
            Ok(QualifiedName {
                keyspace: Some(first),
                table: self.ident()?,
            })
        } else {
            Ok(QualifiedName {
                keyspace: None,
                table: first,
            })
        }
    }
    fn optional_if_not_exists(&mut self) -> Result<bool, ParseError> {
        if self.keyword("if") {
            self.require_keyword("not")?;
            self.require_keyword("exists")?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn optional_if_exists(&mut self) -> Result<bool, ParseError> {
        if self.keyword("if") {
            self.require_keyword("exists")?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn value(&mut self) -> Result<Value, ParseError> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or_else(|| error(self.at(), "expected value"))?
            .clone();
        let result = match token.kind {
            Kind::Bind => {
                let n = self.next_bind;
                self.next_bind += 1;
                Value::Bind(n)
            }
            Kind::String(s) => Value::String(s),
            Kind::Number(n) if n.contains('.') => {
                let n = n
                    .parse::<f64>()
                    .map_err(|_| error(token.offset, "invalid floating-point literal"))?;
                if !n.is_finite() {
                    return Err(error(token.offset, "non-finite floating-point literal"));
                }
                Value::Float(n)
            }
            Kind::Number(n) => Value::Integer(
                n.parse()
                    .map_err(|_| error(token.offset, "integer out of range"))?,
            ),
            Kind::Word(s) if s == "true" => Value::Boolean(true),
            Kind::Word(s) if s == "false" => Value::Boolean(false),
            Kind::Word(s) if s == "null" => Value::Null,
            _ => return Err(error(token.offset, "unsupported value expression")),
        };
        self.index += 1;
        Ok(result)
    }
    fn data_type(&mut self) -> Result<DataType, ParseError> {
        let offset = self.at();
        let name = match self.tokens.get(self.index).map(|token| &token.kind) {
            Some(Kind::Word(name)) => name.clone(),
            _ => return self.fail("expected scalar column type"),
        };
        self.index += 1;
        match name.as_str() {
            "ascii" => Ok(DataType::Ascii),
            "bigint" => Ok(DataType::BigInt),
            "blob" => Ok(DataType::Blob),
            "boolean" => Ok(DataType::Boolean),
            "double" => Ok(DataType::Double),
            "float" => Ok(DataType::Float),
            "int" => Ok(DataType::Int),
            "smallint" => Ok(DataType::SmallInt),
            "tinyint" => Ok(DataType::TinyInt),
            "text" => Ok(DataType::Text),
            "timestamp" => Ok(DataType::Timestamp),
            "uuid" => Ok(DataType::Uuid),
            "timeuuid" => Ok(DataType::TimeUuid),
            "varchar" => Ok(DataType::Varchar),
            _ => Err(error(offset, "unsupported column type")),
        }
    }
    fn ident_list(&mut self) -> Result<Vec<String>, ParseError> {
        let mut values = vec![self.ident()?];
        while self.take(&Kind::Comma) {
            values.push(self.ident()?);
        }
        Ok(values)
    }
    fn create(&mut self) -> Result<Statement, ParseError> {
        if self.keyword("keyspace") {
            let if_not_exists = self.optional_if_not_exists()?;
            return Ok(Statement::CreateKeyspace {
                name: self.ident()?,
                if_not_exists,
            });
        }
        self.require_keyword("table")?;
        let if_not_exists = self.optional_if_not_exists()?;
        let name = self.name()?;
        self.expect(Kind::Left)?;
        let mut columns = Vec::new();
        let mut primary_key = None;
        loop {
            if self.keyword("primary") {
                self.require_keyword("key")?;
                self.expect(Kind::Left)?;
                let key = self.ident()?;
                self.expect(Kind::Right)?;
                if primary_key.replace(key).is_some() {
                    return self.fail("multiple primary keys are unsupported");
                }
            } else {
                let column = Column {
                    name: self.ident()?,
                    data_type: self.data_type()?,
                };
                if self.keyword("primary") {
                    self.require_keyword("key")?;
                    if primary_key.replace(column.name.clone()).is_some() {
                        return self.fail("multiple primary keys are unsupported");
                    }
                }
                columns.push(column);
            }
            if !self.take(&Kind::Comma) {
                break;
            }
        }
        self.expect(Kind::Right)?;
        let key =
            primary_key.ok_or_else(|| error(self.at(), "exactly one primary key is required"))?;
        let mut seen = HashSet::new();
        for column in &columns {
            if !seen.insert(&column.name) {
                return self.fail("duplicate column name");
            }
        }
        if !seen.contains(&key) {
            return self.fail("primary key does not name a column");
        }
        Ok(Statement::CreateTable {
            name,
            columns,
            primary_key: key,
            if_not_exists,
        })
    }
    fn insert(&mut self) -> Result<Statement, ParseError> {
        self.require_keyword("into")?;
        let table = self.name()?;
        self.expect(Kind::Left)?;
        let columns = self.ident_list()?;
        self.expect(Kind::Right)?;
        let mut seen = HashSet::new();
        for c in &columns {
            if !seen.insert(c) {
                return self.fail("duplicate insert column");
            }
        }
        self.require_keyword("values")?;
        self.expect(Kind::Left)?;
        let mut values = vec![self.value()?];
        while self.take(&Kind::Comma) {
            values.push(self.value()?);
        }
        self.expect(Kind::Right)?;
        if columns.len() != values.len() {
            return self.fail("column and value counts differ");
        }
        Ok(Statement::Insert {
            table,
            columns,
            values,
        })
    }
    fn select(&mut self) -> Result<Statement, ParseError> {
        let columns = if self.take(&Kind::Star) {
            SelectColumns::All
        } else {
            SelectColumns::Named(self.ident_list()?)
        };
        self.require_keyword("from")?;
        let table = self.name()?;
        self.require_keyword("where")?;
        let key_column = self.ident()?;
        self.expect(Kind::Equal)?;
        let key_value = self.value()?;
        Ok(Statement::Select {
            table,
            columns,
            key_column,
            key_value,
        })
    }
}

/// Parse exactly one statement from the supported subset, rejecting trailing syntax.
pub fn parse(sql: &str) -> Result<Statement, ParseError> {
    let mut p = Parser {
        tokens: lex(sql)?,
        index: 0,
        end: sql.len(),
        next_bind: 0,
    };
    let statement = if p.keyword("create") {
        p.create()?
    } else if p.keyword("use") {
        Statement::Use {
            keyspace: p.ident()?,
        }
    } else if p.keyword("insert") {
        p.insert()?
    } else if p.keyword("select") {
        p.select()?
    } else if p.keyword("drop") {
        p.require_keyword("table")?;
        let if_exists = p.optional_if_exists()?;
        Statement::DropTable {
            name: p.name()?,
            if_exists,
        }
    } else {
        return p.fail("unsupported statement");
    };
    p.take(&Kind::Semi);
    if p.index != p.tokens.len() {
        return p.fail("unsupported trailing clause or multiple statements");
    }
    Ok(statement)
}
