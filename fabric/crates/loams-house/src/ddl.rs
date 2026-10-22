//! DDL for lake tables (HS1 Task 10, §49 §6.3; FL2 Task 5 on the lake class).
//!
//! The front owns every statement that names the namespace's databases and
//! tables: `CREATE DATABASE`, `CREATE TABLE`, `DROP`, `UNDROP`, `RENAME`, `ALTER
//! TABLE … ADD COLUMN`, `USE`, and `DESCRIBE`, `SHOW CREATE` and `EXISTS` of a
//! catalog table. None of them runs in chDB (no table is ever created inside it,
//! D762). [`execute`] answers each one from the catalog ([`Lake`]).
//!
//! # ClickHouse reads the columns
//!
//! A column list is ClickHouse's to read, not the front's: types, `DEFAULT`
//! expressions, codecs and comments go to a worker as `DESCRIBE TABLE
//! null(<columns>)`, which parses them with ClickHouse's own parser, checks every
//! default against its type and answers each column's canonical type
//! ([`Oracle::describe`]). That is also what lets column `CODEC`s, which sqlparser
//! 0.63 cannot parse (R4.1), through. Before that the statement's syntax tree goes
//! through the deny list like any other ([`Oracle::check`]). The rest of a
//! `CREATE TABLE` (engine, `ORDER BY`, `PARTITION BY`, `PRIMARY KEY`, `SETTINGS`,
//! `COMMENT`) is read here, with the House's lexer: those clauses are names and
//! column references, and each maps onto Iceberg metadata.
//!
//! # What a lake table keeps
//!
//! The Iceberg schema (the types of [`crate::types`]), a partition spec from
//! `PARTITION BY` (§32 §7.4's transforms, else `36`), a sort order from `ORDER BY`,
//! and table properties: `loams.ch.ddl` (the statement as written, which `SHOW
//! CREATE TABLE` answers), `loams.ch.engine`, `loams.ch.types` (every column's
//! ClickHouse type, default, codec and comment), `loams.ch.order_by`,
//! `loams.ch.primary_key`, `loams.ch.partition_by`, `loams.ch.settings`,
//! `loams.table_class` and `comment`. A table without `loams.ch.engine` was made
//! by another writer: it is `external`, read-only (`497`), with ClickHouse types
//! derived from its Iceberg schema.

use std::collections::HashMap;

use async_trait::async_trait;
use loams_house_ipc::EngineError;
use loams_house_lake::TableCreation;
use loams_house_lake::spec::{
    NestedField, NestedFieldRef, NullOrder, Schema, SortDirection, SortField, SortOrder,
    TableMetadata, Transform, UnboundPartitionSpec,
};
use serde::{Deserialize, Serialize};

use crate::catalog::{Lake, LakeTable, RESERVED_PREFIX};
use crate::errors::{ChError, HouseError};
use crate::request::{Token, tokens};
use crate::types::{self, ChType, FieldIds, Needs};

/// ClickHouse codes the catalog answers with that §32 §8.8's nineteen do not
/// list: they are ClickHouse's own answers to the same DDL (measured with
/// `clickhouse local` 26.7), and a Task 29 surface entry each.
pub const CATALOG_CODES: &[(i32, &str)] = &[
    (15, "DUPLICATE_COLUMN"),
    (47, "UNKNOWN_IDENTIFIER"),
    (82, "DATABASE_ALREADY_EXISTS"),
    (620, "QUERY_NOT_ALLOWED"),
];

/// An error with one of [`CATALOG_CODES`], rendered as ClickHouse renders it.
pub fn clickhouse_error(code: i32, name: &str, message: impl Into<String>) -> HouseError {
    debug_assert!(CATALOG_CODES.contains(&(code, name)), "{code} {name}");
    HouseError::Engine(EngineError {
        code,
        name: name.to_string(),
        message: message.into(),
    })
}

/// Property: the statement as written.
pub const PROP_DDL: &str = "loams.ch.ddl";
/// Property: the engine, as `SHOW CREATE` prints it.
pub const PROP_ENGINE: &str = "loams.ch.engine";
/// Property: every column, as JSON.
pub const PROP_TYPES: &str = "loams.ch.types";
/// Property: `ORDER BY`.
pub const PROP_ORDER_BY: &str = "loams.ch.order_by";
/// Property: `PRIMARY KEY`.
pub const PROP_PRIMARY_KEY: &str = "loams.ch.primary_key";
/// Property: `PARTITION BY`.
pub const PROP_PARTITION_BY: &str = "loams.ch.partition_by";
/// Property: the table's `SETTINGS`, as a JSON list of pairs.
pub const PROP_SETTINGS: &str = "loams.ch.settings";
/// Property: the table class.
pub const PROP_CLASS: &str = "loams.table_class";
/// Property: Iceberg's own table comment.
pub const PROP_COMMENT: &str = "comment";

/// A table's class (§49 §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableClass {
    /// Written by the House: Iceberg appends.
    Lake,
    /// Written by Fluss (HS1 Task 14).
    Realtime,
    /// Written by someone else; read-only here.
    External,
    /// A pipe (HS1 Task 16): a stream's records, read only by its
    /// materialized views.
    Pipe,
}

/// The class a table's metadata says it has.
pub fn class_of(metadata: &TableMetadata) -> TableClass {
    let properties = metadata.properties();
    if !properties.contains_key(PROP_ENGINE) {
        return TableClass::External;
    }
    match properties.get(PROP_CLASS).map(String::as_str) {
        Some("realtime") => TableClass::Realtime,
        Some("pipe") => TableClass::Pipe,
        _ => TableClass::Lake,
    }
}

/// One column as ClickHouse describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChColumn {
    /// Its name.
    pub name: String,
    /// Its ClickHouse type, canonical.
    #[serde(rename = "type")]
    pub ch_type: String,
    /// `DEFAULT`, `MATERIALIZED`, `ALIAS` or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_kind: String,
    /// The default's expression.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_expression: String,
    /// Its comment.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Its codec, recorded and not applied (Parquet's compression is the table's).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub codec: String,
}

impl ChColumn {
    /// Whether the column has bytes in the table (`ALIAS` columns are computed).
    pub fn stored(&self) -> bool {
        self.default_kind != "ALIAS"
    }

    /// The column as a `CREATE TABLE` line prints it.
    fn render(&self) -> String {
        let mut out = format!("{} {}", quote_name(&self.name), self.ch_type);
        if !self.default_kind.is_empty() {
            out.push_str(&format!(
                " {} {}",
                self.default_kind, self.default_expression
            ));
        }
        if !self.comment.is_empty() {
            out.push_str(&format!(" COMMENT {}", quote_string(&self.comment)));
        }
        if !self.codec.is_empty() {
            out.push_str(&format!(" CODEC({})", self.codec));
        }
        out
    }
}

/// A row of `DESCRIBE TABLE null(…)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Described {
    /// The column.
    pub column: ChColumn,
    /// Its `TTL` (refused: §49 §6.3).
    pub ttl: String,
}

/// What the front needs from a worker for DDL.
#[async_trait]
pub trait Oracle: Send {
    /// The deny list over the statement's syntax tree (HS1 Task 5): `344` for
    /// anything denied anywhere in it.
    async fn check(&mut self, sql: &str) -> Result<(), HouseError>;
    /// ClickHouse's reading of a column list: `DESCRIBE TABLE null(<columns>)`.
    async fn describe(&mut self, columns: &str) -> Result<Vec<Described>, HouseError>;
    /// The names among `names` that are MergeTree settings.
    async fn merge_tree_settings(&mut self, names: &[String]) -> Result<Vec<String>, HouseError>;
}

// ---------------------------------------------------------------------------
// Names and the lexer.

/// A name as ClickHouse prints it: bare when it is a plain identifier, else in
/// backquotes.
pub fn quote_name(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_string()
    } else {
        format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
    }
}

/// A string literal.
pub fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The value of a quoted token: a string literal or a quoted name.
pub(crate) fn unquote(raw: &str) -> String {
    let mut chars = raw.chars();
    let Some(quote) = chars.next() else {
        return String::new();
    };
    let inner: Vec<char> = chars.collect();
    let inner = if inner.last() == Some(&quote) {
        &inner[..inner.len() - 1]
    } else {
        &inner[..]
    };
    let mut out = String::new();
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if c == '\\' && i + 1 < inner.len() {
            out.push(match inner[i + 1] {
                'n' => '\n',
                't' => '\t',
                '0' => '\0',
                other => other,
            });
            i += 2;
        } else if c == quote && inner.get(i + 1) == Some(&quote) {
            out.push(quote);
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn syntax(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::syntax_error(message.into()))
}

struct Cursor<'a> {
    text: &'a str,
    toks: Vec<Token>,
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            toks: tokens(text.as_bytes()),
            at: 0,
        }
    }

    fn raw(&self, t: &Token) -> &'a str {
        &self.text[t.start..t.end]
    }

    fn peek(&self) -> Option<Token> {
        self.toks.get(self.at).copied()
    }

    fn is_word_at(&self, at: usize, word: &str) -> bool {
        self.toks
            .get(at)
            .is_some_and(|t| t.word && self.raw(t).eq_ignore_ascii_case(word))
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.is_word_at(self.at, word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// All of `words` in a row, or nothing consumed.
    fn eat_all(&mut self, words: &[&str]) -> bool {
        if words
            .iter()
            .enumerate()
            .all(|(i, w)| self.is_word_at(self.at + i, w))
        {
            self.at += words.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, word: &str) -> Result<(), HouseError> {
        if self.eat(word) {
            Ok(())
        } else {
            Err(syntax(format!(
                "Expected {word} at {:?}",
                self.rest().chars().take(40).collect::<String>()
            )))
        }
    }

    fn is_symbol(&self, t: &Token, symbol: u8) -> bool {
        !t.word && t.end == t.start + 1 && self.text.as_bytes()[t.start] == symbol
    }

    fn eat_symbol(&mut self, symbol: u8) -> bool {
        match self.peek() {
            Some(t) if self.is_symbol(&t, symbol) => {
                self.at += 1;
                true
            }
            _ => false,
        }
    }

    /// A name: a word, or a backquoted or double-quoted name.
    fn ident(&mut self) -> Result<String, HouseError> {
        let t = self
            .peek()
            .ok_or_else(|| syntax("Expected a name at the end of the statement"))?;
        let raw = self.raw(&t);
        if t.word {
            self.at += 1;
            return Ok(raw.to_string());
        }
        if raw.starts_with('`') || raw.starts_with('"') {
            self.at += 1;
            return Ok(unquote(raw));
        }
        Err(syntax(format!("Expected a name, found {raw:?}")))
    }

    /// `[db.]name`.
    fn qualified(&mut self) -> Result<(Option<String>, String), HouseError> {
        let first = self.ident()?;
        if self.eat_symbol(b'.') {
            let second = self.ident()?;
            Ok((Some(first), second))
        } else {
            Ok((None, first))
        }
    }

    /// A string literal.
    fn string(&mut self) -> Result<String, HouseError> {
        match self.peek() {
            Some(t) if self.raw(&t).starts_with('\'') => {
                self.at += 1;
                Ok(unquote(self.raw(&t)))
            }
            other => Err(syntax(format!(
                "Expected a string literal, found {:?}",
                other.map(|t| self.raw(&t))
            ))),
        }
    }

    /// `ON CLUSTER <name>`, accepted and ignored (D344).
    fn skip_on_cluster(&mut self) -> Result<(), HouseError> {
        if self.eat_all(&["ON", "CLUSTER"]) {
            match self.peek() {
                Some(t) if self.raw(&t).starts_with('\'') => {
                    self.string()?;
                }
                _ => {
                    self.ident()?;
                }
            }
        }
        Ok(())
    }

    fn done(&self) -> bool {
        self.at >= self.toks.len()
    }

    fn offset(&self) -> usize {
        self.toks.get(self.at).map_or(self.text.len(), |t| t.start)
    }

    fn rest(&self) -> &'a str {
        &self.text[self.offset()..]
    }

    fn expect_end(&self) -> Result<(), HouseError> {
        if self.done() {
            Ok(())
        } else {
            Err(syntax(format!(
                "Unexpected {:?} at the end of the statement",
                self.rest().chars().take(40).collect::<String>()
            )))
        }
    }

    /// The index of the `)` closing the `(` at `open`.
    fn closing(&self, open: usize) -> Option<usize> {
        let depth = self.toks[open].depth;
        (open + 1..self.toks.len())
            .find(|&i| self.is_symbol(&self.toks[i], b')') && self.toks[i].depth == depth)
    }

    /// The comma-separated pieces between the `(` at `open` and its `)`, as text.
    fn pieces(&self, open: usize, close: usize) -> Vec<&'a str> {
        let depth = self.toks[open].depth;
        let mut out = Vec::new();
        let mut start = self.toks[open].end;
        for i in open + 1..close {
            let t = &self.toks[i];
            if t.depth == depth && self.is_symbol(t, b',') {
                out.push(self.text[start..t.start].trim());
                start = t.end;
            }
        }
        let last = self.text[start..self.toks[close].start].trim();
        if !last.is_empty() || !out.is_empty() {
            out.push(last);
        }
        out
    }
}

/// `36` for a name the House keeps for itself or chDB's own databases.
pub fn check_new_name(what: &str, name: &str) -> Result<(), HouseError> {
    let reserved = name.is_empty()
        || name.starts_with(RESERVED_PREFIX)
        || (what == "database"
            && matches!(
                name,
                "system"
                    | "information_schema"
                    | "INFORMATION_SCHEMA"
                    | "_temporary_and_external_tables"
                    | crate::querylog::HOUSE_DATABASE
            ));
    if reserved {
        return Err(HouseError::from(ChError::bad_arguments(format!(
            "The {what} name {name:?} is reserved"
        ))));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Statement shapes.

/// `CREATE TABLE`, as written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateTable {
    /// `OR REPLACE` (refused).
    pub or_replace: bool,
    /// `IF NOT EXISTS`.
    pub if_not_exists: bool,
    /// The database, when named.
    pub database: Option<String>,
    /// The table.
    pub name: String,
    /// The column list's entries (columns only).
    pub columns: Vec<String>,
    /// A `PRIMARY KEY (…)` entry of the column list.
    pub inline_primary_key: Option<String>,
    /// An `INDEX`, `PROJECTION` or `CONSTRAINT` entry (refused), by keyword.
    pub other_entry: Option<String>,
    /// `ENGINE = name(args)`.
    pub engine: Option<(String, Vec<String>)>,
    /// `PARTITION BY`.
    pub partition_by: Option<String>,
    /// `ORDER BY`.
    pub order_by: Option<String>,
    /// `PRIMARY KEY`.
    pub primary_key: Option<String>,
    /// `SAMPLE BY` (refused).
    pub sample_by: bool,
    /// `TTL` (refused).
    pub ttl: bool,
    /// `SETTINGS`, as written (string literals unquoted).
    pub settings: Vec<(String, String)>,
    /// `COMMENT`.
    pub comment: Option<String>,
    /// `AS SELECT …` (HS1 Task 12).
    pub as_select: bool,
    /// `AS <table or function>` with no columns (refused).
    pub as_other: bool,
    /// Where the clauses after the name and column list start (HS1 Task 12).
    pub clauses_at: usize,
    /// Where `AS` (or `EMPTY AS`) starts, and where what follows it starts.
    pub as_at: Option<(usize, usize)>,
    /// `EMPTY AS`: the table only, no rows.
    pub empty_as: bool,
}

fn engine_clause(body: &str) -> Result<(String, Vec<String>), HouseError> {
    let mut c = Cursor::new(body);
    c.eat_symbol(b'=');
    let name = c.ident()?;
    let mut args = Vec::new();
    if let Some(t) = c.peek()
        && c.is_symbol(&t, b'(')
    {
        let open = c.at;
        let close = c
            .closing(open)
            .ok_or_else(|| syntax("Unbalanced parentheses in ENGINE"))?;
        args = c
            .pieces(open, close)
            .into_iter()
            .map(str::to_string)
            .collect();
        c.at = close + 1;
    }
    c.expect_end()?;
    Ok((name, args))
}

fn settings_clause(body: &str) -> Result<Vec<(String, String)>, HouseError> {
    let mut c = Cursor::new(body);
    let mut out = Vec::new();
    while !c.done() {
        let name = c.ident()?;
        if !c.eat_symbol(b'=') {
            return Err(syntax(format!("Expected = after the setting {name}")));
        }
        let start = c.offset();
        while let Some(t) = c.peek() {
            if t.depth == 0 && c.is_symbol(&t, b',') {
                break;
            }
            c.at += 1;
        }
        let raw = body[start..c.offset()].trim();
        let value = if raw.starts_with('\'') {
            unquote(raw)
        } else {
            raw.to_string()
        };
        out.push((name, value));
        c.eat_symbol(b',');
    }
    Ok(out)
}

/// Reads a `CREATE TABLE` (not `TEMPORARY`).
pub fn parse_create_table(text: &str) -> Result<CreateTable, HouseError> {
    let mut c = Cursor::new(text);
    let mut out = CreateTable::default();
    c.expect("CREATE")?;
    out.or_replace = c.eat_all(&["OR", "REPLACE"]);
    if c.is_word_at(c.at, "TEMPORARY") {
        return Err(syntax("A temporary table is not a lake table"));
    }
    c.expect("TABLE")?;
    out.if_not_exists = c.eat_all(&["IF", "NOT", "EXISTS"]);
    let (database, name) = c.qualified()?;
    out.database = database;
    out.name = name;
    if c.eat("UUID") {
        c.string()?;
    }
    c.skip_on_cluster()?;
    if let Some(t) = c.peek()
        && c.is_symbol(&t, b'(')
    {
        let open = c.at;
        let close = c
            .closing(open)
            .ok_or_else(|| syntax("Unbalanced parentheses in the column list"))?;
        for entry in c.pieces(open, close) {
            let mut e = Cursor::new(entry);
            if e.eat_all(&["PRIMARY", "KEY"]) {
                out.inline_primary_key = Some(e.rest().trim().to_string());
            } else if ["INDEX", "PROJECTION", "CONSTRAINT"]
                .iter()
                .any(|w| e.is_word_at(0, w))
            {
                out.other_entry = Some(
                    entry
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase(),
                );
            } else if !entry.is_empty() {
                out.columns.push(entry.to_string());
            }
        }
        c.at = close + 1;
    }
    // The clauses: each runs to the next clause keyword at depth 0.
    let mut clauses: Vec<(String, usize, usize)> = Vec::new();
    let mut i = c.at;
    while i < c.toks.len() {
        let t = c.toks[i];
        if t.depth != 0 || !t.word {
            i += 1;
            continue;
        }
        let word = c.raw(&t).to_ascii_uppercase();
        let two = |next: &str| c.is_word_at(i + 1, next);
        let (keyword, width) = match word.as_str() {
            "ENGINE" | "TTL" | "SETTINGS" | "COMMENT" | "AS" => (word.clone(), 1),
            "PARTITION" if two("BY") => ("PARTITION BY".to_string(), 2),
            "ORDER" if two("BY") => ("ORDER BY".to_string(), 2),
            "PRIMARY" if two("KEY") => ("PRIMARY KEY".to_string(), 2),
            "SAMPLE" if two("BY") => ("SAMPLE BY".to_string(), 2),
            "EMPTY" if two("AS") => ("AS".to_string(), 2),
            _ => {
                if clauses.is_empty() {
                    return Err(syntax(format!(
                        "Unexpected {word} in CREATE TABLE {}",
                        out.name
                    )));
                }
                i += 1;
                continue;
            }
        };
        let body_start = c.toks[i + width - 1].end;
        if clauses.is_empty() {
            out.clauses_at = t.start;
        }
        if keyword == "AS" {
            out.as_at = Some((t.start, body_start));
            out.empty_as = word == "EMPTY";
        }
        if let Some(last) = clauses.last_mut() {
            last.2 = t.start;
        }
        clauses.push((keyword.clone(), body_start, text.len()));
        if keyword == "AS" {
            break;
        }
        i += width;
    }
    if clauses.is_empty() {
        out.clauses_at = text.len();
    }
    if clauses.is_empty() && !c.done() {
        return Err(syntax(format!(
            "Unexpected {:?} in CREATE TABLE",
            c.rest().chars().take(40).collect::<String>()
        )));
    }
    for (keyword, start, end) in clauses {
        let body = text[start..end].trim();
        match keyword.as_str() {
            "ENGINE" => out.engine = Some(engine_clause(body)?),
            "PARTITION BY" => out.partition_by = Some(body.to_string()),
            "ORDER BY" => out.order_by = Some(body.to_string()),
            "PRIMARY KEY" => out.primary_key = Some(body.to_string()),
            "SAMPLE BY" => out.sample_by = true,
            "TTL" => out.ttl = true,
            "SETTINGS" => out.settings = settings_clause(body)?,
            "COMMENT" => {
                let mut b = Cursor::new(body);
                out.comment = Some(b.string()?);
                b.expect_end()?;
            }
            "AS" => {
                let first = crate::request::first_keyword(body);
                if matches!(first.as_str(), "SELECT" | "WITH") || body.starts_with('(') {
                    out.as_select = true;
                } else {
                    out.as_other = true;
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// `CREATE DATABASE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDatabase {
    /// `IF NOT EXISTS`.
    pub if_not_exists: bool,
    /// The name.
    pub name: String,
    /// `ENGINE = …`, by name.
    pub engine: Option<String>,
    /// `COMMENT`.
    pub comment: Option<String>,
}

/// Reads a `CREATE DATABASE`.
pub fn parse_create_database(text: &str) -> Result<CreateDatabase, HouseError> {
    let mut c = Cursor::new(text);
    c.expect("CREATE")?;
    c.expect("DATABASE")?;
    let if_not_exists = c.eat_all(&["IF", "NOT", "EXISTS"]);
    let name = c.ident()?;
    c.skip_on_cluster()?;
    let mut engine = None;
    let mut comment = None;
    while !c.done() {
        if c.eat("ENGINE") {
            c.eat_symbol(b'=');
            engine = Some(c.ident()?);
            if let Some(t) = c.peek()
                && c.is_symbol(&t, b'(')
            {
                let close = c
                    .closing(c.at)
                    .ok_or_else(|| syntax("Unbalanced parentheses in ENGINE"))?;
                c.at = close + 1;
            }
        } else if c.eat("COMMENT") {
            comment = Some(c.string()?);
        } else {
            return Err(syntax(format!(
                "Unexpected {:?} in CREATE DATABASE",
                c.rest().chars().take(40).collect::<String>()
            )));
        }
    }
    Ok(CreateDatabase {
        if_not_exists,
        name,
        engine,
        comment,
    })
}

/// `DROP TABLE | DATABASE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drop {
    /// `TABLE`, `DATABASE`, `VIEW`, `DICTIONARY`.
    pub kind: String,
    /// `IF EXISTS`.
    pub if_exists: bool,
    /// The names.
    pub names: Vec<(Option<String>, String)>,
}

/// Reads a `DROP` (not `TEMPORARY`).
pub fn parse_drop(text: &str) -> Result<Drop, HouseError> {
    let mut c = Cursor::new(text);
    c.expect("DROP")?;
    let kind = c.ident()?.to_ascii_uppercase();
    let if_exists = c.eat_all(&["IF", "EXISTS"]);
    let mut names = vec![c.qualified()?];
    while c.eat_symbol(b',') {
        names.push(c.qualified()?);
    }
    c.skip_on_cluster()?;
    // `SYNC`, `PERMANENTLY`, `NO DELAY`: a lake table is dropped the one way.
    let _ = c.eat("SYNC") || c.eat("PERMANENTLY") || c.eat_all(&["NO", "DELAY"]);
    c.expect_end()?;
    Ok(Drop {
        kind,
        if_exists,
        names,
    })
}

/// `CREATE MATERIALIZED VIEW`, as written (HS1 Task 16).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateMaterializedView {
    /// `OR REPLACE` (refused).
    pub or_replace: bool,
    /// `IF NOT EXISTS`.
    pub if_not_exists: bool,
    /// The view's database, when named.
    pub database: Option<String>,
    /// The view.
    pub name: String,
    /// `TO [db.]table`.
    pub to: Option<QualifiedName>,
    /// A column list or an `ENGINE`: an inner table (refused).
    pub inner_table: bool,
    /// `POPULATE` (refused).
    pub populate: bool,
    /// `REFRESH` (refused).
    pub refresh: bool,
    /// The `SELECT` after `AS`.
    pub select: String,
    /// The table the `SELECT` reads first: the view's source.
    pub source: Option<QualifiedName>,
}

/// Reads a `CREATE MATERIALIZED VIEW`.
pub fn parse_create_mv(text: &str) -> Result<CreateMaterializedView, HouseError> {
    let mut c = Cursor::new(text);
    let mut out = CreateMaterializedView::default();
    c.expect("CREATE")?;
    out.or_replace = c.eat_all(&["OR", "REPLACE"]);
    c.expect("MATERIALIZED")?;
    c.expect("VIEW")?;
    out.if_not_exists = c.eat_all(&["IF", "NOT", "EXISTS"]);
    let (database, name) = c.qualified()?;
    out.database = database;
    out.name = name;
    c.skip_on_cluster()?;
    loop {
        if c.eat("REFRESH") {
            out.refresh = true;
            // The schedule runs to `TO` or `AS`.
            while !c.done() && !c.is_word_at(c.at, "TO") && !c.is_word_at(c.at, "AS") {
                c.at += 1;
            }
        } else if c.eat("TO") {
            out.to = Some(c.qualified()?);
        } else if c.eat("POPULATE") {
            out.populate = true;
        } else if c.eat("EMPTY") {
        } else if c.eat("AS") {
            break;
        } else if let Some(t) = c.peek() {
            out.inner_table = true;
            if c.is_symbol(&t, b'(') {
                let close = c
                    .closing(c.at)
                    .ok_or_else(|| syntax("Unbalanced parentheses in the column list"))?;
                c.at = close + 1;
            } else {
                c.at += 1;
            }
        } else {
            return Err(syntax("Expected AS SELECT in CREATE MATERIALIZED VIEW"));
        }
    }
    let at = c.offset();
    out.select = text[at..].trim().trim_end_matches(';').trim().to_string();
    if out.select.is_empty() {
        return Err(syntax("Expected a SELECT after AS"));
    }
    // The source: the first table after a FROM at the SELECT's own depth.
    let depth = c.peek().map_or(0, |t| t.depth);
    while let Some(t) = c.peek() {
        if t.depth == depth && t.word && c.raw(&t).eq_ignore_ascii_case("FROM") {
            c.at += 1;
            match c.peek() {
                Some(next)
                    if next.word
                        || c.raw(&next).starts_with('`')
                        || c.raw(&next).starts_with('"') =>
                {
                    out.source = Some(c.qualified()?);
                }
                _ => {}
            }
            break;
        }
        c.at += 1;
    }
    Ok(out)
}

/// Reads `UNDROP TABLE [db.]name`.
pub fn parse_undrop(text: &str) -> Result<(Option<String>, String), HouseError> {
    let mut c = Cursor::new(text);
    c.expect("UNDROP")?;
    c.expect("TABLE")?;
    let name = c.qualified()?;
    if c.eat("UUID") {
        c.string()?;
    }
    c.skip_on_cluster()?;
    c.expect_end()?;
    Ok(name)
}

/// A table name, maybe qualified.
pub type QualifiedName = (Option<String>, String);

/// Reads `RENAME TABLE a TO b[, c TO d]`.
pub fn parse_rename(text: &str) -> Result<Vec<(QualifiedName, QualifiedName)>, HouseError> {
    let mut c = Cursor::new(text);
    c.expect("RENAME")?;
    if !c.eat("TABLE") {
        return Err(HouseError::from(ChError::not_implemented(
            "RENAME DATABASE and RENAME DICTIONARY are not on the House's surface; RENAME TABLE is",
        )));
    }
    let mut pairs = Vec::new();
    loop {
        let from = c.qualified()?;
        c.expect("TO")?;
        let to = c.qualified()?;
        pairs.push((from, to));
        if !c.eat_symbol(b',') {
            break;
        }
    }
    c.skip_on_cluster()?;
    c.expect_end()?;
    Ok(pairs)
}

/// Where an added column goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Position {
    /// At the end.
    Last,
    /// `FIRST`.
    First,
    /// `AFTER <column>`.
    After(String),
}

/// One `ADD COLUMN`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddColumn {
    /// `IF NOT EXISTS`.
    pub if_not_exists: bool,
    /// The column definition, as a column-list entry.
    pub definition: String,
    /// Its place.
    pub position: Position,
}

/// Reads `ALTER TABLE t ADD COLUMN …[, ADD COLUMN …]`; any other operation is
/// `48`.
pub fn parse_alter_add(text: &str) -> Result<(QualifiedName, Vec<AddColumn>), HouseError> {
    let mut c = Cursor::new(text);
    c.expect("ALTER")?;
    c.expect("TABLE")?;
    let table = c.qualified()?;
    c.skip_on_cluster()?;
    let mut adds = Vec::new();
    loop {
        if !c.eat_all(&["ADD", "COLUMN"]) {
            return Err(HouseError::from(ChError::not_implemented(format!(
                "ALTER TABLE … {} is not on the House's surface yet; ADD COLUMN is",
                c.rest()
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ")
            ))));
        }
        let if_not_exists = c.eat_all(&["IF", "NOT", "EXISTS"]);
        let first = c.at;
        let mut last = first;
        while last < c.toks.len() && !(c.toks[last].depth == 0 && c.is_symbol(&c.toks[last], b','))
        {
            last += 1;
        }
        // `FIRST` or `AFTER <column>` ends the operation.
        let mut position = Position::Last;
        let mut definition_end = last;
        if last > first && c.is_word_at(last - 1, "FIRST") {
            position = Position::First;
            definition_end = last - 1;
        } else if last >= first + 2 && c.is_word_at(last - 2, "AFTER") {
            let mut after = Cursor::new(c.raw(&c.toks[last - 1]));
            position = Position::After(after.ident()?);
            definition_end = last - 2;
        }
        if definition_end <= first {
            return Err(syntax("ADD COLUMN needs a column definition"));
        }
        let start = c.toks[first].start;
        let end = c.toks[definition_end - 1].end;
        c.at = last;
        adds.push(AddColumn {
            if_not_exists,
            definition: text[start..end].trim().to_string(),
            position,
        });
        if !c.eat_symbol(b',') {
            break;
        }
    }
    c.expect_end()?;
    Ok((table, adds))
}

/// What `DESCRIBE`, `SHOW CREATE` and `EXISTS` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    /// A table (or something chDB may know as one).
    Table(QualifiedName),
    /// A database.
    Database(String),
}

/// `DESCRIBE [TABLE] [db.]t`, or `None` for a form that is chDB's (a subquery, a
/// table function, a temporary table).
pub fn parse_describe(text: &str) -> Option<QualifiedName> {
    let mut c = Cursor::new(text);
    if !(c.eat("DESCRIBE") || c.eat("DESC")) {
        return None;
    }
    c.eat("TABLE");
    let name = c.qualified().ok()?;
    c.done().then_some(name)
}

/// `SHOW CREATE [TABLE | DATABASE] name`, or `None` for chDB's forms.
pub fn parse_show_create(text: &str) -> Option<Object> {
    let mut c = Cursor::new(text);
    if !c.eat_all(&["SHOW", "CREATE"]) {
        return None;
    }
    let object = if c.eat("DATABASE") {
        Object::Database(c.ident().ok()?)
    } else {
        if c.is_word_at(c.at, "TEMPORARY")
            || c.is_word_at(c.at, "VIEW")
            || c.is_word_at(c.at, "DICTIONARY")
        {
            return None;
        }
        c.eat("TABLE");
        Object::Table(c.qualified().ok()?)
    };
    c.done().then_some(object)
}

/// A `LIKE` filter of a `SHOW` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowLike {
    /// `NOT LIKE`.
    pub not: bool,
    /// `ILIKE`.
    pub insensitive: bool,
    /// The pattern.
    pub pattern: String,
}

/// A `SHOW` statement ClickHouse answers from a `system` table the House
/// replaces (HS1 Task 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowList {
    /// `SHOW [FULL] TABLES [FROM | IN db] [[NOT] [I]LIKE 'p' | WHERE expr] [LIMIT n]`.
    Tables {
        /// `FULL`: the engine too.
        full: bool,
        /// The database (the current one when absent).
        from: Option<String>,
        /// The `LIKE` filter.
        like: Option<ShowLike>,
        /// A `WHERE` expression, as written.
        filter: Option<String>,
        /// `LIMIT`, as written.
        limit: Option<String>,
    },
    /// `SHOW DATABASES [[NOT] [I]LIKE 'p'] [LIMIT n]`.
    Databases {
        /// The `LIKE` filter.
        like: Option<ShowLike>,
        /// `LIMIT`, as written.
        limit: Option<String>,
    },
    /// `SHOW [FULL] PROCESSLIST`.
    Processlist,
}

/// The `SHOW` forms of [`ShowList`]; `None` for any other (chDB's, or a form the
/// deny list refuses). `Some(Err)` for one of these forms that cannot be read.
pub fn parse_show_list(text: &str) -> Option<Result<ShowList, HouseError>> {
    let mut c = Cursor::new(text);
    if !c.eat("SHOW") {
        return None;
    }
    let full = c.eat("FULL");
    if c.eat("PROCESSLIST") {
        return Some(if c.done() {
            Ok(ShowList::Processlist)
        } else {
            Err(syntax("Unexpected text after SHOW PROCESSLIST"))
        });
    }
    let tables = c.is_word_at(c.at, "TABLES");
    let databases = !full && c.is_word_at(c.at, "DATABASES");
    if !tables && !databases {
        return None;
    }
    c.at += 1;
    Some((|| {
        let mut from = None;
        if tables && (c.eat("FROM") || c.eat("IN")) {
            from = Some(c.ident()?);
        }
        let not = c.eat("NOT");
        let like = if c.is_word_at(c.at, "LIKE") || c.is_word_at(c.at, "ILIKE") {
            let insensitive = c.is_word_at(c.at, "ILIKE");
            c.at += 1;
            Some(ShowLike {
                not,
                insensitive,
                pattern: c.string()?,
            })
        } else if not {
            return Err(syntax("Expected LIKE or ILIKE after NOT"));
        } else {
            None
        };
        let mut filter = None;
        if tables && like.is_none() && c.eat("WHERE") {
            let start = c.offset();
            let mut end = c.text.len();
            while let Some(t) = c.peek() {
                if t.depth == 0 && c.is_word_at(c.at, "LIMIT") {
                    end = t.start;
                    break;
                }
                c.at += 1;
            }
            let expr = c.text[start..end].trim();
            if expr.is_empty() {
                return Err(syntax("Expected an expression after WHERE"));
            }
            filter = Some(expr.to_string());
        }
        let mut limit = None;
        if c.eat("LIMIT") {
            let t = c
                .peek()
                .ok_or_else(|| syntax("Expected a number after LIMIT"))?;
            let raw = c.raw(&t);
            if !raw.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(syntax("Expected a number after LIMIT"));
            }
            limit = Some(raw.to_string());
            c.at += 1;
        }
        c.expect_end()?;
        Ok(if tables {
            ShowList::Tables {
                full,
                from,
                like,
                filter,
                limit,
            }
        } else {
            ShowList::Databases { like, limit }
        })
    })())
}

/// `EXISTS [TABLE | DATABASE] name`, or `None` for chDB's forms.
pub fn parse_exists(text: &str) -> Option<Object> {
    let mut c = Cursor::new(text);
    if !c.eat("EXISTS") {
        return None;
    }
    let object = if c.eat("DATABASE") {
        Object::Database(c.ident().ok()?)
    } else {
        if c.is_word_at(c.at, "TEMPORARY")
            || c.is_word_at(c.at, "VIEW")
            || c.is_word_at(c.at, "DICTIONARY")
        {
            return None;
        }
        c.eat("TABLE");
        Object::Table(c.qualified().ok()?)
    };
    c.done().then_some(object)
}

/// The table an `INSERT` writes, or `None` for a table function.
pub fn insert_target(text: &str) -> Option<QualifiedName> {
    let mut c = Cursor::new(text);
    if !c.eat("INSERT") || !c.eat("INTO") {
        return None;
    }
    if c.eat("FUNCTION") {
        return None;
    }
    c.eat("TABLE");
    c.qualified().ok()
}

/// An `INSERT` into a table, read up to its data or its query (HS1 Task 12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertHead {
    /// The table.
    pub target: QualifiedName,
    /// The column list, when the statement has one.
    pub columns: Option<Vec<String>>,
    /// The statement's own `SETTINGS` (string literals unquoted).
    pub settings: Vec<(String, String)>,
    /// What follows.
    pub rest: InsertRest,
}

/// What follows an `INSERT`'s head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertRest {
    /// Nothing: the data streams after it (`FORMAT` was taken off).
    Nothing,
    /// `VALUES`, and the tuples from this offset.
    Values(usize),
    /// A query (`SELECT`, `WITH`, a parenthesised query) from this offset.
    Select(usize),
}

/// Reads an `INSERT INTO [TABLE] [db.]t [(c, …)] [SETTINGS …]` head and what
/// follows it. A column list must be plain names (`48` otherwise: no
/// `* EXCEPT`, no `COLUMNS(…)` on lake tables), and a `FORMAT` must have been
/// taken off by the request's planner.
pub fn parse_insert(text: &str) -> Result<InsertHead, HouseError> {
    let mut c = Cursor::new(text);
    c.expect("INSERT")?;
    c.expect("INTO")?;
    if c.is_word_at(c.at, "FUNCTION") {
        return Err(syntax("INSERT INTO FUNCTION does not write a table"));
    }
    c.eat("TABLE");
    let target = c.qualified()?;
    let mut columns = None;
    if c.peek().is_some_and(|t| c.is_symbol(&t, b'(')) {
        let close = c
            .closing(c.at)
            .ok_or_else(|| syntax("The column list of the INSERT is not closed"))?;
        let mut names = Vec::new();
        for piece in c.pieces(c.at, close) {
            let mut column = Cursor::new(piece);
            let name = column.ident().ok().filter(|_| column.done());
            match name {
                Some(name) => names.push(name),
                None => {
                    return Err(HouseError::from(ChError::not_implemented(format!(
                        "INSERT into a lake table names its columns one by one; {piece:?} is \
                         not a column name"
                    ))));
                }
            }
        }
        c.at = close + 1;
        columns = Some(names);
    }
    let mut settings = Vec::new();
    if c.eat("SETTINGS") {
        loop {
            let name = c.ident()?;
            if !c.eat_symbol(b'=') {
                return Err(syntax(format!("Expected = after the setting {name}")));
            }
            let mut value = String::new();
            if c.eat_symbol(b'-') {
                value.push('-');
            }
            let t = c
                .peek()
                .ok_or_else(|| syntax(format!("Expected a value for the setting {name}")))?;
            let raw = c.raw(&t);
            value.push_str(&if raw.starts_with('\'') {
                unquote(raw)
            } else if t.word {
                raw.to_string()
            } else {
                return Err(syntax(format!("Expected a value for the setting {name}")));
            });
            c.at += 1;
            settings.push((name, value));
            if !c.eat_symbol(b',') {
                break;
            }
        }
    }
    let rest = match c.peek() {
        None => InsertRest::Nothing,
        Some(t) if t.word && c.raw(&t).eq_ignore_ascii_case("VALUES") => InsertRest::Values(t.end),
        Some(t)
            if (t.word
                && (c.raw(&t).eq_ignore_ascii_case("SELECT")
                    || c.raw(&t).eq_ignore_ascii_case("WITH")))
                || c.is_symbol(&t, b'(') =>
        {
            InsertRest::Select(t.start)
        }
        Some(t) => {
            return Err(syntax(format!(
                "Expected VALUES, FORMAT or a query after the INSERT's table, found {:?}",
                c.raw(&t)
            )));
        }
    };
    Ok(InsertHead {
        target,
        columns,
        settings,
        rest,
    })
}

/// `OPTIMIZE TABLE` (HS1 Task 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizeStmt {
    /// The table.
    pub table: QualifiedName,
    /// `PARTITION …` (the House compacts the whole table, R13.2).
    pub partition: bool,
    /// `FINAL`.
    pub final_: bool,
    /// `DEDUPLICATE`.
    pub deduplicate: bool,
}

/// Reads `OPTIMIZE TABLE [db.]t [ON CLUSTER c] [PARTITION p | PARTITION ID 'p']
/// [FINAL] [DEDUPLICATE]`. `DEDUPLICATE BY` is `48`.
pub fn parse_optimize(text: &str) -> Result<OptimizeStmt, HouseError> {
    let mut c = Cursor::new(text);
    c.expect("OPTIMIZE")?;
    c.expect("TABLE")?;
    let table = c.qualified()?;
    c.skip_on_cluster()?;
    let mut out = OptimizeStmt {
        table,
        partition: false,
        final_: false,
        deduplicate: false,
    };
    if c.eat("PARTITION") {
        out.partition = true;
        // The partition's expression runs to FINAL, DEDUPLICATE or the end.
        while let Some(t) = c.peek() {
            if t.depth == 0 && (c.is_word_at(c.at, "FINAL") || c.is_word_at(c.at, "DEDUPLICATE")) {
                break;
            }
            c.at += 1;
        }
    }
    out.final_ = c.eat("FINAL");
    if c.eat("DEDUPLICATE") {
        out.deduplicate = true;
        if c.eat("BY") {
            return Err(HouseError::from(ChError::not_implemented(
                "OPTIMIZE … DEDUPLICATE BY is not served on lake tables; DEDUPLICATE compares \
                 whole rows",
            )));
        }
    }
    c.expect_end()?;
    Ok(out)
}

/// `TRUNCATE [TABLE] [IF EXISTS] [db.]t [ON CLUSTER c]`: the table and `IF
/// EXISTS` (HS1 Task 12).
pub fn parse_truncate(text: &str) -> Option<(QualifiedName, bool)> {
    let mut c = Cursor::new(text);
    if !c.eat("TRUNCATE") {
        return None;
    }
    if c.is_word_at(c.at, "TEMPORARY")
        || c.is_word_at(c.at, "DATABASE")
        || c.is_word_at(c.at, "ALL")
    {
        return None;
    }
    c.eat("TABLE");
    let if_exists = c.eat_all(&["IF", "EXISTS"]);
    let name = c.qualified().ok()?;
    c.skip_on_cluster().ok()?;
    c.done().then_some((name, if_exists))
}

/// `USE db`.
pub fn parse_use(text: &str) -> Option<String> {
    let mut c = Cursor::new(text);
    if !c.eat("USE") {
        return None;
    }
    let name = c.ident().ok()?;
    c.done().then_some(name)
}

// ---------------------------------------------------------------------------
// Expressions the clauses hold.

/// One element of `ORDER BY`, `PRIMARY KEY` or `PARTITION BY`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Element {
    /// A column.
    Column(String),
    /// `f(column)`.
    Call(String, String),
    /// Anything else.
    Expr(String),
}

/// An expression's elements: `tuple(a, b)` and `(a, b)` are a list, `tuple()`
/// none, anything else one.
fn elements(expr: &str) -> Vec<Element> {
    let expr = expr.trim();
    let c = Cursor::new(expr);
    let list = |open: usize| -> Option<Vec<&str>> {
        let close = c.closing(open)?;
        (close + 1 == c.toks.len()).then(|| c.pieces(open, close))
    };
    let pieces: Vec<&str> =
        if c.toks.len() >= 2 && c.is_word_at(0, "tuple") && c.is_symbol(&c.toks[1], b'(') {
            list(1).unwrap_or_else(|| vec![expr])
        } else if c.toks.first().is_some_and(|t| c.is_symbol(t, b'(')) {
            list(0).unwrap_or_else(|| vec![expr])
        } else if expr.is_empty() {
            Vec::new()
        } else {
            vec![expr]
        };
    pieces.into_iter().map(element).collect()
}

/// A key's elements as written (`ORDER BY (a, toDate(b))` → `a`, `toDate(b)`),
/// each with the column it is when it is a bare column (HS1 Task 11's `FINAL`
/// views group and deduplicate by them).
pub fn key_elements(expr: &str) -> Vec<(String, Option<String>)> {
    elements(expr)
        .into_iter()
        .map(|element| match element {
            Element::Column(c) => (quote_name(&c), Some(c)),
            Element::Call(f, c) => (format!("{f}({})", quote_name(&c)), None),
            Element::Expr(text) => (text, None),
        })
        .collect()
}

fn element(text: &str) -> Element {
    let mut c = Cursor::new(text);
    if let Ok(name) = c.ident() {
        if c.done() {
            return Element::Column(name);
        }
        if c.eat_symbol(b'(')
            && let Ok(column) = c.ident()
            && c.eat_symbol(b')')
            && c.done()
        {
            return Element::Call(name, column);
        }
    }
    Element::Expr(text.to_string())
}

/// The Iceberg transform of a `PARTITION BY` element's function (§32 §7.4).
fn partition_transform(function: &str) -> Option<(Transform, &'static str)> {
    match function {
        "toYYYYMM" => Some((Transform::Month, "month")),
        "toYYYYMMDD" | "toDate" => Some((Transform::Day, "day")),
        "toStartOfHour" => Some((Transform::Hour, "hour")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The table.

/// A lake table's engine (D765), with replication accepted and ignored (D344).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeEngine {
    /// Appends.
    MergeTree,
    /// `ReplacingMergeTree([ver[, is_deleted]])`.
    Replacing {
        /// The version column.
        ver: Option<String>,
        /// The deleted-flag column.
        is_deleted: Option<String>,
    },
    /// `SummingMergeTree([columns])`.
    Summing {
        /// The summed columns.
        columns: Option<Vec<String>>,
    },
}

impl LakeEngine {
    /// The engine as `SHOW CREATE` prints it.
    pub fn render(&self) -> String {
        match self {
            Self::MergeTree => "MergeTree".to_string(),
            Self::Replacing { ver, is_deleted } => {
                let args: Vec<String> = ver
                    .iter()
                    .chain(is_deleted.iter())
                    .map(|c| quote_name(c))
                    .collect();
                if args.is_empty() {
                    "ReplacingMergeTree".to_string()
                } else {
                    format!("ReplacingMergeTree({})", args.join(", "))
                }
            }
            Self::Summing { columns } => match columns {
                None => "SummingMergeTree".to_string(),
                Some(columns) => format!(
                    "SummingMergeTree(({}))",
                    columns
                        .iter()
                        .map(|c| quote_name(c))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        }
    }

    /// The engine a table's `loams.ch.engine` property names (HS1 Task 11's
    /// `FINAL` views).
    pub fn parse(text: &str) -> Result<Self, HouseError> {
        let (name, args) = engine_clause(text)?;
        Self::from_clause(&name, &args)
    }

    /// Reads an `ENGINE` clause; `48` for an engine that is not Tier 1.
    pub fn from_clause(name: &str, args: &[String]) -> Result<Self, HouseError> {
        let base = name.strip_prefix("Replicated").unwrap_or(name);
        let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
        if base != name {
            // A replicated engine's Keeper path and replica name come first.
            while args.first().is_some_and(|a| a.starts_with('\'')) {
                args.remove(0);
            }
        }
        let column = |text: &str| -> Result<String, HouseError> {
            match element(text) {
                Element::Column(c) => Ok(c),
                _ => Err(HouseError::from(ChError::bad_arguments(format!(
                    "Engine {name}'s argument {text} must be a column"
                )))),
            }
        };
        match base {
            "MergeTree" if args.is_empty() => Ok(Self::MergeTree),
            "MergeTree" => Err(HouseError::from(ChError::bad_arguments(
                "MergeTree takes no arguments: the old syntax MergeTree(date, key, granularity) \
                 is not supported; use ORDER BY and PARTITION BY",
            ))),
            "ReplacingMergeTree" if args.len() <= 2 => Ok(Self::Replacing {
                ver: args.first().map(|a| column(a)).transpose()?,
                is_deleted: args.get(1).map(|a| column(a)).transpose()?,
            }),
            "SummingMergeTree" if args.len() <= 1 => Ok(Self::Summing {
                columns: match args.first() {
                    None => None,
                    Some(arg) => Some(
                        elements(arg)
                            .into_iter()
                            .map(|e| match e {
                                Element::Column(c) => Ok(c),
                                _ => Err(HouseError::from(ChError::bad_arguments(format!(
                                    "SummingMergeTree's columns must be columns: {arg}"
                                )))),
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                },
            }),
            "ReplacingMergeTree" | "SummingMergeTree" => Err(HouseError::from(
                ChError::bad_arguments(format!("Engine {name} has too many arguments")),
            )),
            _ => Err(HouseError::from(ChError::not_implemented(format!(
                "Engine {name} is not a lake table engine: lake tables are MergeTree, \
                 ReplacingMergeTree or SummingMergeTree (§49 §6.2)"
            )))),
        }
    }
}

/// A catalog table as ClickHouse shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChTable {
    /// Its database.
    pub database: String,
    /// Its name.
    pub name: String,
    /// Its class.
    pub class: TableClass,
    /// Its engine (`Iceberg` for an external table).
    pub engine: String,
    /// Its columns, in order.
    pub columns: Vec<ChColumn>,
    /// `ORDER BY`.
    pub order_by: String,
    /// `PRIMARY KEY`.
    pub primary_key: String,
    /// `PARTITION BY`.
    pub partition_by: String,
    /// `SETTINGS`.
    pub settings: Vec<(String, String)>,
    /// Its comment.
    pub comment: String,
    /// `SHOW CREATE TABLE`'s answer.
    pub ddl: String,
}

impl ChTable {
    /// The table a catalog entry is.
    pub fn from_lake(database: &str, name: &str, table: &LakeTable) -> Self {
        let metadata = &table.metadata;
        let properties = metadata.properties();
        let get = |key: &str| properties.get(key).cloned().unwrap_or_default();
        let class = class_of(metadata);
        if class == TableClass::External {
            let columns = metadata
                .current_schema()
                .as_struct()
                .fields()
                .iter()
                .map(|f| ChColumn {
                    name: f.name.clone(),
                    ch_type: types::from_iceberg(&f.field_type, f.required),
                    comment: f.doc.clone().unwrap_or_default(),
                    ..ChColumn::default()
                })
                .collect();
            let mut out = Self {
                database: database.to_string(),
                name: name.to_string(),
                class,
                engine: "Iceberg".to_string(),
                columns,
                order_by: String::new(),
                primary_key: String::new(),
                partition_by: String::new(),
                settings: Vec::new(),
                comment: get(PROP_COMMENT),
                ddl: String::new(),
            };
            out.ddl = out.render();
            return out;
        }
        let columns: Vec<ChColumn> = serde_json::from_str(&get(PROP_TYPES)).unwrap_or_default();
        let settings: Vec<(String, String)> =
            serde_json::from_str(&get(PROP_SETTINGS)).unwrap_or_default();
        let mut out = Self {
            database: database.to_string(),
            name: name.to_string(),
            class,
            engine: get(PROP_ENGINE),
            columns,
            order_by: get(PROP_ORDER_BY),
            primary_key: get(PROP_PRIMARY_KEY),
            partition_by: get(PROP_PARTITION_BY),
            settings,
            comment: get(PROP_COMMENT),
            ddl: get(PROP_DDL),
        };
        if out.ddl.is_empty() {
            out.ddl = out.render();
        }
        out
    }

    /// The table as ClickHouse's `SHOW CREATE TABLE` formats one.
    pub fn render(&self) -> String {
        let columns: Vec<String> = self
            .columns
            .iter()
            .map(|c| format!("    {}", c.render()))
            .collect();
        let mut out = format!(
            "CREATE TABLE {}.{}\n(\n{}\n)\nENGINE = {}",
            quote_name(&self.database),
            quote_name(&self.name),
            columns.join(",\n"),
            self.engine
        );
        if !self.partition_by.is_empty() {
            out.push_str(&format!("\nPARTITION BY {}", self.partition_by));
        }
        if !self.primary_key.is_empty() {
            out.push_str(&format!("\nPRIMARY KEY {}", self.primary_key));
        }
        if !matches!(self.class, TableClass::External | TableClass::Pipe) {
            let order_by = if self.order_by.is_empty() {
                "tuple()"
            } else {
                &self.order_by
            };
            out.push_str(&format!("\nORDER BY {order_by}"));
        }
        if !self.settings.is_empty() {
            let settings: Vec<String> = self
                .settings
                .iter()
                .map(|(k, v)| format!("{k} = {}", setting_literal(v)))
                .collect();
            out.push_str(&format!("\nSETTINGS {}", settings.join(", ")));
        }
        if !self.comment.is_empty() {
            out.push_str(&format!("\nCOMMENT {}", quote_string(&self.comment)));
        }
        out
    }
}

fn setting_literal(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        value.to_string()
    } else {
        quote_string(value)
    }
}

fn field_id(schema: &Schema, column: &str) -> Option<i32> {
    schema
        .as_struct()
        .fields()
        .iter()
        .find(|f| f.name == column)
        .map(|f| f.id)
}

fn unknown_column(column: &str, what: &str) -> HouseError {
    clickhouse_error(
        47,
        "UNKNOWN_IDENTIFIER",
        format!("Missing columns: '{column}' while processing {what}"),
    )
}

/// The Iceberg table a `CREATE TABLE` makes, given ClickHouse's reading of its
/// columns and the MergeTree settings it names.
pub fn plan_table(
    stmt: &CreateTable,
    text: &str,
    location: String,
    described: &[Described],
    merge_tree_settings: &[String],
) -> Result<TableCreation, HouseError> {
    let not_on_surface = |what: &str| {
        Err(HouseError::from(ChError::not_implemented(format!(
            "{what} is not supported on lake tables (§49 §6.3)"
        ))))
    };
    if stmt.or_replace {
        return not_on_surface("CREATE OR REPLACE TABLE");
    }
    if stmt.sample_by {
        return not_on_surface("SAMPLE BY");
    }
    if stmt.ttl {
        return not_on_surface("TTL");
    }
    if let Some(entry) = &stmt.other_entry {
        return not_on_surface(&format!("{entry} in the column list"));
    }
    if stmt.as_other {
        return not_on_surface("CREATE TABLE … AS <table or table function>");
    }
    let engine = match &stmt.engine {
        Some((name, args)) => LakeEngine::from_clause(name, args)?,
        None => LakeEngine::MergeTree,
    };

    // Columns: ClickHouse's reading, then the type table.
    let mut columns = Vec::with_capacity(described.len());
    for d in described {
        if !d.ttl.is_empty() {
            return not_on_surface("A column TTL");
        }
        if !matches!(
            d.column.default_kind.as_str(),
            "" | "DEFAULT" | "MATERIALIZED" | "ALIAS"
        ) {
            return not_on_surface(&format!("A {} column", d.column.default_kind));
        }
        columns.push(d.column.clone());
    }
    if columns.is_empty() {
        return Err(HouseError::from(ChError::bad_arguments(
            "A lake table needs at least one column",
        )));
    }
    let mut ids = FieldIds::after(0);
    let mut needs = Needs::V2;
    let mut fields: Vec<NestedFieldRef> = Vec::new();
    for column in columns.iter().filter(|c| c.stored()) {
        let field =
            types::to_iceberg_field(&column.name, &column.ch_type, &mut ids, false, &mut needs)?;
        let field = if column.comment.is_empty() {
            field
        } else {
            let mut with_doc: NestedField = field.as_ref().clone();
            with_doc.doc = Some(column.comment.clone());
            with_doc.into()
        };
        fields.push(field);
    }
    let schema = Schema::builder()
        .with_schema_id(0)
        .with_fields(fields)
        .build()
        .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?;
    let ch_type = |name: &str| {
        columns
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.ch_type.clone())
    };

    // ORDER BY and PRIMARY KEY.
    let order_by = stmt
        .order_by
        .clone()
        .or_else(|| stmt.primary_key.clone())
        .or_else(|| stmt.inline_primary_key.clone())
        .unwrap_or_default();
    let primary_key = stmt
        .primary_key
        .clone()
        .or_else(|| stmt.inline_primary_key.clone())
        .unwrap_or_default();
    let order_elements = elements(&order_by);
    for element in order_elements.iter().chain(elements(&primary_key).iter()) {
        if let Element::Column(c) | Element::Call(_, c) = element
            && ch_type(c).is_none()
        {
            return Err(unknown_column(c, "the sorting key"));
        }
    }
    let primary_elements = elements(&primary_key);
    if !primary_elements.is_empty()
        && (primary_elements.len() > order_elements.len()
            || primary_elements
                .iter()
                .zip(&order_elements)
                .any(|(p, o)| p != o))
    {
        return Err(HouseError::from(ChError::bad_arguments(
            "Primary key must be a prefix of the sorting key",
        )));
    }
    let mut sort_fields = Vec::new();
    for element in &order_elements {
        let (column, transform) = match element {
            Element::Column(c) => (c, Transform::Identity),
            Element::Call(f, c) => match partition_transform(f) {
                Some((t, _)) => (c, t),
                None => break,
            },
            Element::Expr(_) => break,
        };
        let Some(source_id) = field_id(&schema, column) else {
            break;
        };
        let ch = ChType::parse(&ch_type(column).unwrap_or_default())?;
        if matches!(
            ch.base(),
            ChType::Array(_) | ChType::Map(..) | ChType::Tuple(_)
        ) {
            break;
        }
        sort_fields.push(SortField {
            source_id,
            transform,
            direction: SortDirection::Ascending,
            null_order: NullOrder::First,
        });
    }

    // PARTITION BY (§32 §7.4).
    let mut spec = UnboundPartitionSpec::builder().with_spec_id(0);
    let partition_by = stmt.partition_by.clone().unwrap_or_default();
    for element in elements(&partition_by) {
        let refused = |why: String| {
            HouseError::from(ChError::bad_arguments(format!(
                "PARTITION BY {partition_by}: {why}. A lake table's partition is a column, or \
                 toYYYYMM, toYYYYMMDD, toDate or toStartOfHour of a date or time column"
            )))
        };
        let (column, transform, suffix) = match &element {
            Element::Column(c) => (c.clone(), Transform::Identity, None),
            Element::Call(f, c) => match partition_transform(f) {
                Some((t, suffix)) => (c.clone(), t, Some(suffix)),
                None => return Err(refused(format!("{f} is not a supported transform"))),
            },
            Element::Expr(e) => return Err(refused(format!("{e} is not a supported transform"))),
        };
        let Some(ch_text) = ch_type(&column) else {
            return Err(unknown_column(&column, "the partition key"));
        };
        let source_id = field_id(&schema, &column)
            .ok_or_else(|| refused(format!("{column} is an ALIAS column")))?;
        let ch = ChType::parse(&ch_text)?;
        let base = ch.base();
        let ok = match transform {
            Transform::Identity => {
                !matches!(base, ChType::Array(_) | ChType::Map(..) | ChType::Tuple(_))
            }
            Transform::Hour => matches!(base, ChType::DateTime(_) | ChType::DateTime64(..)),
            _ => {
                matches!(base, ChType::DateTime(_) | ChType::DateTime64(..))
                    || matches!(base, ChType::Simple(s) if s == "Date" || s == "Date32")
            }
        };
        if !ok {
            return Err(refused(format!("{column} is {ch_text}")));
        }
        let name = match suffix {
            Some(suffix) => format!("{column}_{suffix}"),
            None => column.clone(),
        };
        spec = spec
            .add_partition_field(source_id, name, transform)
            .map_err(|err| refused(err.to_string()))?;
    }

    // The engine's columns.
    let check_column =
        |column: &str, allowed: &dyn Fn(&ChType) -> bool, what: &str| -> Result<(), HouseError> {
            let text =
                ch_type(column).ok_or_else(|| unknown_column(column, "the engine's arguments"))?;
            let ch = ChType::parse(&text)?;
            if allowed(ch.base()) {
                Ok(())
            } else {
                Err(HouseError::from(ChError::bad_arguments(format!(
                    "{what} {column} cannot be {text}"
                ))))
            }
        };
    let integer = |t: &ChType| {
        matches!(t, ChType::Simple(s) if s.starts_with("Int") || s.starts_with("UInt") || s == "Date" || s == "Date32")
            || matches!(t, ChType::DateTime(_) | ChType::DateTime64(..))
    };
    let numeric = |t: &ChType| {
        matches!(t, ChType::Simple(s) if s.starts_with("Int") || s.starts_with("UInt") || s.starts_with("Float"))
            || matches!(t, ChType::Decimal(..))
    };
    match &engine {
        LakeEngine::MergeTree => {}
        LakeEngine::Replacing { ver, is_deleted } => {
            if let Some(ver) = ver {
                check_column(ver, &integer, "ReplacingMergeTree's version column")?;
            }
            if let Some(deleted) = is_deleted {
                check_column(
                    deleted,
                    &|t| matches!(t, ChType::Simple(s) if s == "UInt8"),
                    "ReplacingMergeTree's is_deleted column",
                )?;
            }
        }
        LakeEngine::Summing { columns } => {
            for column in columns.iter().flatten() {
                check_column(column, &numeric, "SummingMergeTree's summed column")?;
            }
        }
    }

    // SETTINGS: Loams's own, else MergeTree's (recorded, not applied), else 115.
    for (name, value) in &stmt.settings {
        match name.as_str() {
            "loams_table_class" => match value.as_str() {
                "lake" => {}
                "realtime" => {
                    return Err(HouseError::from(ChError::bad_arguments(
                        "loams_table_class = 'realtime' needs the Fabric (FL1), which this House \
                         is not deployed with (§49 §5.2)",
                    )));
                }
                other => {
                    return Err(HouseError::from(ChError::bad_arguments(format!(
                        "loams_table_class is 'lake' or 'realtime', not {other:?}"
                    ))));
                }
            },
            "loams_tiering_freshness" | "loams_bucket_key" => {}
            _ if merge_tree_settings.iter().any(|s| s == name) => {}
            _ => {
                return Err(HouseError::from(ChError::unknown_setting(format!(
                    "Unknown setting '{name}': for storage MergeTree"
                ))));
            }
        }
    }

    let mut properties = HashMap::from([
        (PROP_DDL.to_string(), text.trim().to_string()),
        (PROP_ENGINE.to_string(), engine.render()),
        (
            PROP_TYPES.to_string(),
            serde_json::to_string(&columns)
                .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?,
        ),
        (PROP_CLASS.to_string(), "lake".to_string()),
    ]);
    for (key, value) in [
        (PROP_ORDER_BY, order_by.trim().to_string()),
        (PROP_PRIMARY_KEY, primary_key.trim().to_string()),
        (PROP_PARTITION_BY, partition_by.trim().to_string()),
        (PROP_COMMENT, stmt.comment.clone().unwrap_or_default()),
    ] {
        if !value.is_empty() {
            properties.insert(key.to_string(), value);
        }
    }
    if !stmt.settings.is_empty() {
        properties.insert(
            PROP_SETTINGS.to_string(),
            serde_json::to_string(&stmt.settings)
                .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?,
        );
    }
    let sort_order = if sort_fields.is_empty() {
        SortOrder::unsorted_order()
    } else {
        SortOrder {
            order_id: 1,
            fields: sort_fields,
        }
    };
    Ok(TableCreation::builder()
        .name(stmt.name.clone())
        .location(location)
        .schema(schema)
        .partition_spec(spec.build())
        .sort_order(sort_order)
        .properties(properties)
        .format_version(needs.format_version())
        .build())
}

// ---------------------------------------------------------------------------
// Execution.

/// What the front does with a statement after [`execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Done: an empty success.
    Done,
    /// Answer with the rows of this query (a `SELECT` over constants), which the
    /// worker formats as the request asked.
    Rows(String),
    /// Not the catalog's: chDB answers the statement as it stands.
    PassThrough,
}

/// The tables and databases a catalog statement names (HS1 Task 20's
/// grants), with an unqualified table in `current`. A statement whose names
/// cannot be read names nothing here: [`execute`] answers it.
pub fn touched(kind: Kind, text: &str, current: &str) -> Vec<Object> {
    let table = |(db, name): QualifiedName| {
        Object::Table((Some(db.unwrap_or_else(|| current.to_string())), name))
    };
    let qualify = |object: Object| match object {
        Object::Table(name) => table(name),
        database => database,
    };
    match kind {
        Kind::CreateDatabase => parse_create_database(text)
            .map(|c| vec![Object::Database(c.name)])
            .unwrap_or_default(),
        Kind::CreateTable => parse_create_table(text)
            .map(|c| vec![table((c.database, c.name))])
            .unwrap_or_default(),
        Kind::Drop => match parse_drop(text) {
            Ok(drop) if drop.kind == "DATABASE" => drop
                .names
                .into_iter()
                .map(|(_, name)| Object::Database(name))
                .collect(),
            Ok(drop) => drop.names.into_iter().map(table).collect(),
            Err(_) => Vec::new(),
        },
        Kind::Undrop => parse_undrop(text)
            .map(|n| vec![table(n)])
            .unwrap_or_default(),
        Kind::Rename => parse_rename(text)
            .map(|pairs| {
                pairs
                    .into_iter()
                    .flat_map(|(from, to)| [table(from), table(to)])
                    .collect()
            })
            .unwrap_or_default(),
        Kind::AlterAdd => parse_alter_add(text)
            .map(|(name, _)| vec![table(name)])
            .unwrap_or_default(),
        Kind::Describe => parse_describe(text).map(table).into_iter().collect(),
        Kind::Show => parse_show_create(text).map(qualify).into_iter().collect(),
        Kind::Exists => parse_exists(text).map(qualify).into_iter().collect(),
        Kind::Insert => Vec::new(),
        Kind::CreateMaterializedView => parse_create_mv(text)
            .map(|mv| {
                let mut out = vec![table((mv.database, mv.name))];
                out.extend(mv.to.map(table));
                out.extend(mv.source.map(table));
                out
            })
            .unwrap_or_default(),
    }
}

/// What a statement is, for [`execute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `CREATE DATABASE`.
    CreateDatabase,
    /// `CREATE TABLE`.
    CreateTable,
    /// `DROP` (not `TEMPORARY`).
    Drop,
    /// `UNDROP TABLE`.
    Undrop,
    /// `RENAME`.
    Rename,
    /// `ALTER TABLE … ADD COLUMN`.
    AlterAdd,
    /// `DESCRIBE`.
    Describe,
    /// `SHOW …`.
    Show,
    /// `EXISTS`.
    Exists,
    /// `INSERT`.
    Insert,
    /// `CREATE MATERIALIZED VIEW` (HS1 Task 16).
    CreateMaterializedView,
}

/// The constant query a set of rows answers as: `SELECT … FROM values(…)`.
fn rows_query(columns: &[(&str, &str)], rows: &[Vec<String>]) -> String {
    let structure = columns
        .iter()
        .map(|(name, ty)| format!("{} {ty}", quote_name(name)))
        .collect::<Vec<_>>()
        .join(", ");
    if rows.is_empty() {
        let casts = columns
            .iter()
            .map(|(name, ty)| format!("CAST(NULL, 'Nullable({ty})') AS {}", quote_name(name)))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("SELECT {casts} WHERE 0");
    }
    let rows = rows
        .iter()
        .map(|row| {
            format!(
                "({})",
                row.iter()
                    .map(|v| quote_string(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("SELECT * FROM values({}, {rows})", quote_string(&structure))
}

/// The rows of `DESCRIBE TABLE`.
pub fn describe_query(table: &ChTable) -> String {
    let rows: Vec<Vec<String>> = table
        .columns
        .iter()
        .map(|c| {
            vec![
                c.name.clone(),
                c.ch_type.clone(),
                c.default_kind.clone(),
                c.default_expression.clone(),
                c.comment.clone(),
                c.codec.clone(),
                String::new(),
            ]
        })
        .collect();
    rows_query(
        &[
            ("name", "String"),
            ("type", "String"),
            ("default_type", "String"),
            ("default_expression", "String"),
            ("comment", "String"),
            ("codec_expression", "String"),
            ("ttl_expression", "String"),
        ],
        &rows,
    )
}

fn one_value(column: &str, value: &str) -> String {
    format!("SELECT {} AS {}", value, quote_name(column))
}

/// Runs a catalog statement for namespace `namespace` whose current database is
/// `current`: [`Answer::PassThrough`] when it names nothing of the catalog's.
pub async fn execute(
    lake: &Lake,
    namespace: u64,
    current: &str,
    kind: Kind,
    text: &str,
    oracle: &mut dyn Oracle,
) -> Result<Answer, HouseError> {
    let db_of = |db: &Option<String>| db.clone().unwrap_or_else(|| current.to_string());
    match kind {
        Kind::CreateDatabase => {
            let stmt = parse_create_database(text)?;
            if let Some(engine) = &stmt.engine {
                crate::deny::check_database_engine(engine).map_err(HouseError::from)?;
            }
            lake.create_database(
                namespace,
                &stmt.name,
                stmt.comment.as_deref(),
                stmt.if_not_exists,
            )
            .await?;
            Ok(Answer::Done)
        }
        Kind::CreateTable => {
            let stmt = parse_create_table(text)?;
            if stmt.as_select {
                return Err(HouseError::from(ChError::not_implemented(
                    "CREATE TABLE … AS SELECT is served from HS1 Task 12; this House does not serve it yet",
                )));
            }
            let database = db_of(&stmt.database);
            lake.require_database(namespace, &database).await?;
            if stmt.if_not_exists && lake.load(namespace, &database, &stmt.name).await?.is_some() {
                return Ok(Answer::Done);
            }
            if let Some((engine, _)) = &stmt.engine
                && crate::pipes::is_pipe_engine(engine)
            {
                crate::pipes::create_pipe(lake, namespace, &database, &stmt, text, oracle).await?;
                return Ok(Answer::Done);
            }
            oracle.check(text).await?;
            let described = if stmt.columns.is_empty() {
                Vec::new()
            } else {
                oracle.describe(&stmt.columns.join(", ")).await?
            };
            let names: Vec<String> = stmt
                .settings
                .iter()
                .map(|(k, _)| k.clone())
                .filter(|k| !k.starts_with("loams_"))
                .collect();
            let known = if names.is_empty() {
                Vec::new()
            } else {
                oracle.merge_tree_settings(&names).await?
            };
            let location = lake.root().table_location(namespace, &database, &stmt.name);
            let creation = plan_table(&stmt, text, location, &described, &known)?;
            lake.create_table(namespace, &database, creation, stmt.if_not_exists)
                .await?;
            Ok(Answer::Done)
        }
        Kind::Drop => {
            let stmt = parse_drop(text)?;
            match stmt.kind.as_str() {
                "TABLE" => {
                    for (db, name) in &stmt.names {
                        let database = db_of(db);
                        // A materialized view is dropped as a table too, as in
                        // ClickHouse (HS1 Task 16).
                        if lake.load(namespace, &database, name).await?.is_none()
                            && crate::pipes::drop_materialized_view(
                                lake, namespace, &database, name,
                            )
                            .await?
                        {
                            continue;
                        }
                        lake.drop_table(namespace, &database, name, stmt.if_exists)
                            .await?;
                    }
                    Ok(Answer::Done)
                }
                "VIEW" => {
                    for (db, name) in &stmt.names {
                        let database = db_of(db);
                        if !crate::pipes::drop_materialized_view(lake, namespace, &database, name)
                            .await?
                            && !stmt.if_exists
                        {
                            return Err(HouseError::from(ChError::unknown_table(format!(
                                "Table {database}.{name} does not exist"
                            ))));
                        }
                    }
                    Ok(Answer::Done)
                }
                "DATABASE" => {
                    for (db, name) in &stmt.names {
                        if db.is_some() {
                            return Err(syntax("A database name is not qualified"));
                        }
                        lake.drop_database(namespace, name, stmt.if_exists).await?;
                    }
                    Ok(Answer::Done)
                }
                other => Err(HouseError::from(ChError::not_implemented(format!(
                    "DROP {other} is not on the House's surface"
                )))),
            }
        }
        Kind::CreateMaterializedView => {
            oracle.check(text).await?;
            crate::pipes::create_materialized_view(lake, namespace, current, text).await?;
            Ok(Answer::Done)
        }
        Kind::Undrop => {
            let (db, name) = parse_undrop(text)?;
            lake.undrop_table(namespace, &db_of(&db), &name).await?;
            Ok(Answer::Done)
        }
        Kind::Rename => {
            for ((from_db, from), (to_db, to)) in parse_rename(text)? {
                let (from_db, to_db) = (db_of(&from_db), db_of(&to_db));
                let renamed = lake
                    .rename_table(namespace, (&from_db, &from), (&to_db, &to))
                    .await?;
                // SHOW CREATE names the table: write it out again under the new name.
                let mut table = ChTable::from_lake(&to_db, &to, &renamed);
                table.ddl = table.render();
                lake.catalog()
                    .commit(
                        &renamed.ident,
                        Vec::new(),
                        vec![loams_house_lake::TableUpdate::SetProperties {
                            updates: HashMap::from([(PROP_DDL.to_string(), table.ddl)]),
                        }],
                    )
                    .await?;
            }
            Ok(Answer::Done)
        }
        Kind::AlterAdd => {
            let ((db, name), adds) = parse_alter_add(text)?;
            let database = db_of(&db);
            let table = lake
                .lake_table(namespace, &database, &name, "ALTER TABLE")
                .await?;
            oracle.check(text).await?;
            add_columns(lake, &database, &name, table, adds, oracle).await?;
            Ok(Answer::Done)
        }
        Kind::Describe => {
            let Some((db, name)) = parse_describe(text) else {
                return Ok(Answer::PassThrough);
            };
            let database = db_of(&db);
            match catalog_table(lake, namespace, &database, &name).await? {
                Some(table) => Ok(Answer::Rows(describe_query(&table))),
                None => Ok(Answer::PassThrough),
            }
        }
        Kind::Show => match parse_show_create(text) {
            Some(Object::Table((db, name))) => {
                let database = db_of(&db);
                match catalog_table(lake, namespace, &database, &name).await? {
                    Some(table) => Ok(Answer::Rows(rows_query(
                        &[("statement", "String")],
                        &[vec![table.ddl]],
                    ))),
                    None => Ok(Answer::PassThrough),
                }
            }
            Some(Object::Database(database)) => {
                lake.require_database(namespace, &database).await?;
                let comment = lake
                    .databases(namespace)
                    .await?
                    .into_iter()
                    .find(|d| d.name == database)
                    .map(|d| d.comment)
                    .unwrap_or_default();
                let mut ddl = format!("CREATE DATABASE {}\nENGINE = Atomic", quote_name(&database));
                if !comment.is_empty() {
                    ddl.push_str(&format!("\nCOMMENT {}", quote_string(&comment)));
                }
                Ok(Answer::Rows(rows_query(
                    &[("statement", "String")],
                    &[vec![ddl]],
                )))
            }
            None => Ok(Answer::PassThrough),
        },
        Kind::Exists => match parse_exists(text) {
            Some(Object::Database(database)) => {
                let exists = lake.database_exists(namespace, &database).await?;
                Ok(Answer::Rows(one_value(
                    "result",
                    if exists { "toUInt8(1)" } else { "toUInt8(0)" },
                )))
            }
            Some(Object::Table((db, name))) => {
                let database = db_of(&db);
                if !lake.database_exists(namespace, &database).await? {
                    return Ok(Answer::PassThrough);
                }
                match lake.load(namespace, &database, &name).await? {
                    Some(_) => Ok(Answer::Rows(one_value("result", "toUInt8(1)"))),
                    // A temporary table is chDB's.
                    None if db.is_none() => Ok(Answer::PassThrough),
                    None => Ok(Answer::Rows(one_value("result", "toUInt8(0)"))),
                }
            }
            None => Ok(Answer::PassThrough),
        },
        // `gate::catalog_answer` gives an `INSERT` naming a lake table to
        // `insert::target` (HS1 Task 12) before it gets here.
        Kind::Insert => Ok(Answer::PassThrough),
    }
}

/// The catalog's table, or `None` when the database or table is not the
/// catalog's (then chDB answers: a temporary table, `system`).
async fn catalog_table(
    lake: &Lake,
    namespace: u64,
    database: &str,
    name: &str,
) -> Result<Option<ChTable>, HouseError> {
    if !lake.database_exists(namespace, database).await? {
        return Ok(None);
    }
    Ok(lake
        .load(namespace, database, name)
        .await?
        .map(|table| ChTable::from_lake(database, name, &table)))
}

/// `ALTER TABLE … ADD COLUMN`: Iceberg schema evolution. The new fields are
/// optional (Iceberg v2 cannot add a required field to a table that may have
/// rows); the ClickHouse type stays what was written, and the read views of HS1
/// Task 11 give older rows the column's default.
async fn add_columns(
    lake: &Lake,
    database: &str,
    name: &str,
    table: LakeTable,
    adds: Vec<AddColumn>,
    oracle: &mut dyn Oracle,
) -> Result<(), HouseError> {
    let mut ch = ChTable::from_lake(database, name, &table);
    let mut fields: Vec<NestedFieldRef> = table
        .metadata
        .current_schema()
        .as_struct()
        .fields()
        .to_vec();
    let mut ids = FieldIds::after(table.metadata.last_column_id());
    let mut needs = if table.metadata.format_version() == loams_house_lake::spec::FormatVersion::V3
    {
        Needs::V3
    } else {
        Needs::V2
    };
    let mut changed = false;
    for add in adds {
        let described = oracle.describe(&add.definition).await?;
        let [column] = described.as_slice() else {
            return Err(syntax(format!(
                "ADD COLUMN {}: one column at a time",
                add.definition
            )));
        };
        if !column.ttl.is_empty() {
            return Err(HouseError::from(ChError::not_implemented(
                "A column TTL is not supported on lake tables (§49 §6.3)",
            )));
        }
        let column = column.column.clone();
        if ch.columns.iter().any(|c| c.name == column.name) {
            if add.if_not_exists {
                continue;
            }
            return Err(clickhouse_error(
                15,
                "DUPLICATE_COLUMN",
                format!(
                    "Cannot add column {}: column with this name already exists",
                    quote_name(&column.name)
                ),
            ));
        }
        let at = match &add.position {
            Position::Last => ch.columns.len(),
            Position::First => 0,
            Position::After(after) => {
                ch.columns
                    .iter()
                    .position(|c| &c.name == after)
                    .ok_or_else(|| unknown_column(after, "ADD COLUMN … AFTER"))?
                    + 1
            }
        };
        if column.stored() {
            let mut field: NestedField =
                types::to_iceberg_field(&column.name, &column.ch_type, &mut ids, true, &mut needs)?
                    .as_ref()
                    .clone();
            if !column.comment.is_empty() {
                field.doc = Some(column.comment.clone());
            }
            // Iceberg's field order follows the stored columns' order.
            let stored_before = ch.columns[..at].iter().filter(|c| c.stored()).count();
            fields.insert(stored_before.min(fields.len()), field.into());
        }
        ch.columns.insert(at, column);
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    if needs == Needs::V3
        && table.metadata.format_version() != loams_house_lake::spec::FormatVersion::V3
    {
        return Err(HouseError::from(ChError::not_implemented(
            "A DateTime64 column with more than 6 digits needs Iceberg format v3; this table is v2",
        )));
    }
    let schema = Schema::builder()
        .with_fields(fields)
        .build()
        .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?;
    ch.ddl = ch.render();
    let properties = HashMap::from([
        (
            PROP_TYPES.to_string(),
            serde_json::to_string(&ch.columns)
                .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?,
        ),
        (PROP_DDL.to_string(), ch.ddl.clone()),
    ]);
    lake.catalog()
        .update_schema(&table, schema, properties)
        .await
        .map_err(HouseError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_table_reads_every_clause() {
        let stmt = parse_create_table(
            "CREATE TABLE IF NOT EXISTS db.`my t` ON CLUSTER c (\n  a UInt64 CODEC(ZSTD(3)),\n  b String DEFAULT 'x, y' COMMENT 'hi',\n  ts DateTime64(3, 'UTC'),\n  PRIMARY KEY (a)\n) ENGINE = ReplicatedReplacingMergeTree('/p/{shard}', '{replica}', ts) PARTITION BY toYYYYMM(ts) ORDER BY (a, b) SETTINGS index_granularity = 8192, loams_table_class = 'lake' COMMENT 'tbl'",
        )
        .expect("parses");
        assert!(stmt.if_not_exists);
        assert_eq!(stmt.database.as_deref(), Some("db"));
        assert_eq!(stmt.name, "my t");
        assert_eq!(
            stmt.columns,
            vec![
                "a UInt64 CODEC(ZSTD(3))",
                "b String DEFAULT 'x, y' COMMENT 'hi'",
                "ts DateTime64(3, 'UTC')"
            ]
        );
        assert_eq!(stmt.inline_primary_key.as_deref(), Some("(a)"));
        let (engine, args) = stmt.engine.clone().expect("engine");
        assert_eq!(engine, "ReplicatedReplacingMergeTree");
        assert_eq!(
            LakeEngine::from_clause(&engine, &args).expect("engine"),
            LakeEngine::Replacing {
                ver: Some("ts".into()),
                is_deleted: None
            }
        );
        assert_eq!(stmt.partition_by.as_deref(), Some("toYYYYMM(ts)"));
        assert_eq!(stmt.order_by.as_deref(), Some("(a, b)"));
        assert_eq!(
            stmt.settings,
            vec![
                ("index_granularity".to_string(), "8192".to_string()),
                ("loams_table_class".to_string(), "lake".to_string())
            ]
        );
        assert_eq!(stmt.comment.as_deref(), Some("tbl"));
        assert!(
            parse_create_table(
                "CREATE TABLE t (a UInt8) ENGINE = MergeTree ORDER BY a AS SELECT 1"
            )
            .expect("parses")
            .as_select
        );
        assert!(
            parse_create_table("CREATE TABLE t (a UInt8) ENGINE = MergeTree ORDER BY a TTL a + 1")
                .expect("parses")
                .ttl
        );
        assert!(parse_create_table("CREATE TABLE t (a UInt8) BANANA").is_err());
    }

    #[test]
    fn optimize_is_read() {
        let o = parse_optimize(
            "OPTIMIZE TABLE d.t ON CLUSTER c PARTITION tuple(1, 'x') FINAL DEDUPLICATE",
        )
        .expect("parses");
        assert_eq!(o.table, (Some("d".to_string()), "t".to_string()));
        assert!(o.partition && o.final_ && o.deduplicate);
        let o = parse_optimize("optimize table t").expect("bare");
        assert!(!o.partition && !o.final_ && !o.deduplicate);
        assert_eq!(
            parse_optimize("OPTIMIZE TABLE t FINAL DEDUPLICATE BY a")
                .expect_err("by")
                .code(),
            48
        );
        assert!(parse_optimize("OPTIMIZE TABLE t FINAL; DROP TABLE x").is_err());
    }

    #[test]
    fn truncate_names_its_table() {
        assert_eq!(
            parse_truncate("TRUNCATE TABLE IF EXISTS d.t ON CLUSTER c"),
            Some(((Some("d".to_string()), "t".to_string()), true))
        );
        assert_eq!(
            parse_truncate("truncate t"),
            Some(((None, "t".to_string()), false))
        );
        assert_eq!(parse_truncate("TRUNCATE TEMPORARY TABLE t"), None);
        assert_eq!(parse_truncate("TRUNCATE ALL TABLES FROM d"), None);
        assert_eq!(parse_truncate("TRUNCATE t; DROP TABLE x"), None);
    }

    #[test]
    fn insert_heads_are_read() {
        let head = parse_insert(
            "INSERT INTO TABLE d.`my t` (a, `b c`) SETTINGS x = 1, y = 'z' VALUES (1)",
        )
        .expect("parses");
        assert_eq!(head.target, (Some("d".to_string()), "my t".to_string()));
        assert_eq!(head.columns, Some(vec!["a".to_string(), "b c".to_string()]));
        assert_eq!(
            head.settings,
            vec![
                ("x".to_string(), "1".to_string()),
                ("y".to_string(), "z".to_string())
            ]
        );
        let text = "INSERT INTO t VALUES (1), (2)";
        let InsertRest::Values(at) = parse_insert(text).expect("values").rest else {
            panic!("values");
        };
        assert_eq!(&text[at..], " (1), (2)");
        let text = "insert into t (k) with 1 as x select x";
        let InsertRest::Select(at) = parse_insert(text).expect("select").rest else {
            panic!("select");
        };
        assert_eq!(&text[at..], "with 1 as x select x");
        assert_eq!(
            parse_insert("INSERT INTO t").expect("bare").rest,
            InsertRest::Nothing
        );
        // Column lists of plain names only; nothing else after the head.
        for bad in [
            "INSERT INTO t (* EXCEPT (a)) VALUES (1)",
            "INSERT INTO t (COLUMNS('a')) VALUES (1)",
            "INSERT INTO t (a + 1) VALUES (1)",
            "INSERT INTO t DROP TABLE x",
            "INSERT INTO FUNCTION null('a UInt8') VALUES (1)",
        ] {
            assert!(parse_insert(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn small_statements() {
        assert_eq!(
            parse_create_database("CREATE DATABASE IF NOT EXISTS d ENGINE = Atomic COMMENT 'c'")
                .expect("parses"),
            CreateDatabase {
                if_not_exists: true,
                name: "d".into(),
                engine: Some("Atomic".into()),
                comment: Some("c".into())
            }
        );
        let drop = parse_drop("DROP TABLE IF EXISTS a, d.b SYNC").expect("parses");
        assert_eq!(drop.kind, "TABLE");
        assert!(drop.if_exists);
        assert_eq!(
            drop.names,
            vec![(None, "a".into()), (Some("d".into()), "b".into())]
        );
        assert_eq!(
            parse_undrop("UNDROP TABLE d.t").expect("parses"),
            (Some("d".into()), "t".into())
        );
        assert_eq!(
            parse_rename("RENAME TABLE a TO d.b, c TO e").expect("parses"),
            vec![
                ((None, "a".into()), (Some("d".into()), "b".into())),
                ((None, "c".into()), (None, "e".into()))
            ]
        );
        let ((_, table), adds) = parse_alter_add(
            "ALTER TABLE t ADD COLUMN IF NOT EXISTS x UInt8 DEFAULT 1 AFTER a, ADD COLUMN y String FIRST, ADD COLUMN z Array(UInt8)",
        )
        .expect("parses");
        assert_eq!(table, "t");
        assert_eq!(
            adds,
            vec![
                AddColumn {
                    if_not_exists: true,
                    definition: "x UInt8 DEFAULT 1".into(),
                    position: Position::After("a".into())
                },
                AddColumn {
                    if_not_exists: false,
                    definition: "y String".into(),
                    position: Position::First
                },
                AddColumn {
                    if_not_exists: false,
                    definition: "z Array(UInt8)".into(),
                    position: Position::Last
                },
            ]
        );
        assert_eq!(
            parse_alter_add("ALTER TABLE t DROP COLUMN x")
                .expect_err("48")
                .code(),
            48
        );
        assert_eq!(
            parse_describe("DESCRIBE TABLE d.t"),
            Some((Some("d".into()), "t".into()))
        );
        assert_eq!(parse_describe("DESC (SELECT 1)"), None);
        assert_eq!(parse_describe("DESCRIBE numbers(3)"), None);
        assert_eq!(
            parse_show_create("SHOW CREATE TABLE t"),
            Some(Object::Table((None, "t".into())))
        );
        assert_eq!(
            parse_show_create("SHOW CREATE DATABASE d"),
            Some(Object::Database("d".into()))
        );
        assert_eq!(parse_show_create("SHOW CREATE TEMPORARY TABLE t"), None);
        assert_eq!(
            parse_exists("EXISTS DATABASE d"),
            Some(Object::Database("d".into()))
        );
        assert_eq!(
            parse_exists("EXISTS t"),
            Some(Object::Table((None, "t".into())))
        );
        assert_eq!(
            insert_target("INSERT INTO TABLE d.t (a) FORMAT CSV"),
            Some((Some("d".into()), "t".into()))
        );
        assert_eq!(
            insert_target("INSERT INTO FUNCTION null('a UInt8') VALUES (1)"),
            None
        );
        assert_eq!(parse_use("USE `my db`"), Some("my db".into()));
    }

    #[test]
    fn elements_of_keys() {
        assert_eq!(elements("tuple()"), Vec::new());
        assert_eq!(
            elements("(a, toYYYYMM(ts))"),
            vec![
                Element::Column("a".into()),
                Element::Call("toYYYYMM".into(), "ts".into())
            ]
        );
        assert_eq!(elements("tuple(a)"), vec![Element::Column("a".into())]);
        assert_eq!(elements("a % 2"), vec![Element::Expr("a % 2".into())]);
        assert_eq!(elements("(a) + 1"), vec![Element::Expr("(a) + 1".into())]);
    }

    #[test]
    fn rows_queries_quote_everything() {
        assert_eq!(
            rows_query(&[("statement", "String")], &[vec!["it's".into()]]),
            "SELECT * FROM values('statement String', ('it\\'s'))"
        );
        assert_eq!(
            rows_query(&[("a", "String")], &[]),
            "SELECT CAST(NULL, 'Nullable(String)') AS a WHERE 0"
        );
        assert_eq!(quote_name("a b"), "`a b`");
        assert_eq!(quote_name("ab_1"), "ab_1");
        assert_eq!(unquote("`a``b`"), "a`b");
        assert_eq!(unquote("'a\\'b'"), "a'b");
    }
}
