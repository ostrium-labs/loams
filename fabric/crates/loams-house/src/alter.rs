//! `ALTER TABLE` on lake tables (HE1 Task 9, ruling R9.12).
//!
//! The front reads every `ALTER TABLE` with its own reader ([`parse_alter`]),
//! never with sqlparser 0.63, which refuses most of ClickHouse's forms (`ADD INDEX
//! IF NOT EXISTS`, `MATERIALIZE INDEX`, `MODIFY SETTING`, `MODIFY QUERY`, `DELETE
//! WHERE`, R2.3). The statement becomes an [`Alter`]: the table, its operations in
//! order ([`AlterOp`]) and the statement's trailing `SETTINGS`.
//!
//! # What each operation does
//!
//! * **Columns** (`ADD`, `DROP`, `MODIFY`, `RENAME`, `COMMENT COLUMN`) are Iceberg
//!   schema evolution. `MODIFY COLUMN` takes a change that needs no rewrite: a
//!   default, codec or comment, a place, the same stored type (`LowCardinality`
//!   comes and goes), `Nullable` added (an Iceberg field made optional), or an
//!   Iceberg promotion (`Int32` → `Int64`, `Float32` → `Float64`). Anything else
//!   would rewrite the column's data and is `48`. A column a key, the partition,
//!   `SAMPLE BY`, the `TTL` or the engine names cannot be dropped or renamed
//!   (`524`, as in ClickHouse).
//! * **Skip indexes and projections** (`ADD`, `DROP`, `MATERIALIZE`, `CLEAR`) are
//!   recorded and never built: they only let ClickHouse read less, and a lake
//!   table's answers are the same without them. `MATERIALIZE` and `CLEAR` check
//!   the name and do nothing else.
//! * **Constraints** (`ADD`, `DROP CONSTRAINT`): a `CHECK` holds on every later
//!   insert (the staging table carries it); rows already stored are not checked,
//!   as in ClickHouse.
//! * **`MODIFY SETTING`** and **`RESET SETTING`**: the table's settings, held to
//!   `CREATE TABLE`'s rules (Loams's own, or MergeTree's, which are recorded).
//! * **`MODIFY TTL`**, **`REMOVE TTL`**, **`MATERIALIZE TTL`**: the table `TTL`
//!   (delete rules only, [`ddl::parse_ttl`]); reads hide expired rows, so
//!   `MATERIALIZE TTL` has nothing left to do.
//! * **`MODIFY COMMENT`**, **`MODIFY SAMPLE BY`**, **`REMOVE SAMPLE BY`**.
//! * **`MODIFY QUERY`** on a materialized view: its `SELECT`, which must read the
//!   same source.
//! * **Mutations** (`DELETE`, `UPDATE`, `CLEAR COLUMN`, `DROP PARTITION`, `DROP
//!   PART`) are parsed here and served by the mutation path ([`mutation`]); an
//!   `ALTER` holds either mutations or metadata changes, not both.
//! * Every other operation ClickHouse has (`ATTACH`, `FREEZE`, `MOVE`, …) is
//!   [`AlterOp::Other`]: `48`, named.
//!
//! The trailing `SETTINGS` are held to the request's lists
//! ([`crate::settings::is_allowed`]); `mutations_sync` and `alter_sync` ask for
//! what the House always does (R9.11). `ON CLUSTER` is accepted and ignored (D344).
//!
//! # The AST is a contract
//!
//! [`Alter`] and [`AlterOp`] are what the mutation path builds on: a new
//! operation is a new variant, and a variant's fields do not change meaning.

use std::collections::HashMap;

use loams_house_lake::TableUpdate;
use loams_house_lake::spec::{NestedField, NestedFieldRef, PrimitiveType, Schema, Type};

use crate::catalog::{Lake, LakeTable};
use crate::ddl::{
    self, AddColumn, ChTable, Constraint, Cursor, Oracle, Position, Projection, QualifiedName,
    SkipIndex, quote_name, syntax,
};
use crate::errors::{ChError, HouseError};
use crate::types::{self, FieldIds, Needs};

/// An `ALTER TABLE` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alter {
    /// The table (or materialized view, for `MODIFY QUERY`).
    pub table: QualifiedName,
    /// The operations, in order.
    pub ops: Vec<AlterOp>,
    /// The statement's trailing `SETTINGS`, string literals unquoted.
    pub settings: Vec<(String, String)>,
}

/// `MODIFY COLUMN`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifyColumn {
    /// `IF EXISTS`.
    pub if_exists: bool,
    /// The column.
    pub name: String,
    /// What follows the name, without a trailing `FIRST`/`AFTER` (empty for a
    /// move alone): `[type] [DEFAULT|MATERIALIZED|ALIAS expr] [CODEC(…)]
    /// [COMMENT '…']`.
    pub definition: String,
    /// `REMOVE DEFAULT|MATERIALIZED|ALIAS|CODEC|COMMENT|TTL`, the property named.
    pub remove: Option<String>,
    /// `FIRST` or `AFTER c`; [`Position::Last`] when neither is written.
    pub position: Position,
}

/// One operation of an `ALTER TABLE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlterOp {
    /// `ADD COLUMN [IF NOT EXISTS] def [FIRST|AFTER c]`.
    AddColumn(AddColumn),
    /// `DROP COLUMN [IF EXISTS] c`.
    DropColumn {
        /// `IF EXISTS`.
        if_exists: bool,
        /// The column.
        name: String,
    },
    /// `MODIFY COLUMN …`.
    ModifyColumn(ModifyColumn),
    /// `RENAME COLUMN [IF EXISTS] a TO b`.
    RenameColumn {
        /// `IF EXISTS`.
        if_exists: bool,
        /// The column.
        from: String,
        /// Its new name.
        to: String,
    },
    /// `COMMENT COLUMN [IF EXISTS] c 'text'`.
    CommentColumn {
        /// `IF EXISTS`.
        if_exists: bool,
        /// The column.
        name: String,
        /// The comment.
        comment: String,
    },
    /// `ADD INDEX [IF NOT EXISTS] name expr TYPE t [GRANULARITY n]`.
    AddIndex {
        /// `IF NOT EXISTS`.
        if_not_exists: bool,
        /// The index.
        index: SkipIndex,
    },
    /// `DROP INDEX`, `MATERIALIZE INDEX` or `CLEAR INDEX [IF EXISTS] name [IN
    /// PARTITION p]`.
    Index {
        /// `DROP`, `MATERIALIZE` or `CLEAR`.
        action: String,
        /// `IF EXISTS`.
        if_exists: bool,
        /// The index.
        name: String,
    },
    /// `ADD PROJECTION [IF NOT EXISTS] name (…)`.
    AddProjection {
        /// `IF NOT EXISTS`.
        if_not_exists: bool,
        /// The projection.
        projection: Projection,
    },
    /// `DROP`, `MATERIALIZE` or `CLEAR PROJECTION [IF EXISTS] name`.
    ProjectionOp {
        /// `DROP`, `MATERIALIZE` or `CLEAR`.
        action: String,
        /// `IF EXISTS`.
        if_exists: bool,
        /// The projection.
        name: String,
    },
    /// `ADD CONSTRAINT [IF NOT EXISTS] name CHECK|ASSUME expr`.
    AddConstraint {
        /// `IF NOT EXISTS`.
        if_not_exists: bool,
        /// The constraint.
        constraint: Constraint,
    },
    /// `DROP CONSTRAINT [IF EXISTS] name`.
    DropConstraint {
        /// `IF EXISTS`.
        if_exists: bool,
        /// The constraint.
        name: String,
    },
    /// `MODIFY SETTING a = 1, …`, string literals unquoted.
    ModifySetting(Vec<(String, String)>),
    /// `RESET SETTING a, …`.
    ResetSetting(Vec<String>),
    /// `MODIFY TTL …`, as written.
    ModifyTtl(String),
    /// `REMOVE TTL`.
    RemoveTtl,
    /// `MATERIALIZE TTL [IN PARTITION p]`.
    MaterializeTtl,
    /// `MODIFY QUERY SELECT …` (a materialized view).
    ModifyQuery(String),
    /// `MODIFY COMMENT 'text'`.
    ModifyComment(String),
    /// `MODIFY SAMPLE BY expr`.
    ModifySampleBy(String),
    /// `REMOVE SAMPLE BY`.
    RemoveSampleBy,
    /// A mutation: `DELETE [IN PARTITION p] WHERE predicate`.
    Delete {
        /// `IN PARTITION p`, as written.
        partition: Option<String>,
        /// The `WHERE` predicate, as written.
        predicate: String,
    },
    /// A mutation: `UPDATE c = e, … [IN PARTITION p] WHERE predicate`.
    Update {
        /// `(column, expression)` pairs, as written.
        assignments: Vec<(String, String)>,
        /// `IN PARTITION p`, as written.
        partition: Option<String>,
        /// The `WHERE` predicate, as written.
        predicate: String,
    },
    /// A mutation: `CLEAR COLUMN [IF EXISTS] c [IN PARTITION p]`.
    ClearColumn {
        /// `IF EXISTS`.
        if_exists: bool,
        /// The column.
        name: String,
        /// `IN PARTITION p`, as written.
        partition: Option<String>,
    },
    /// A mutation: `DROP PARTITION p` (`p` as written: an expression, a literal
    /// or `ID '…'`).
    DropPartition {
        /// The partition, as written.
        partition: String,
    },
    /// A mutation: `DROP PART 'name'`.
    DropPart {
        /// The part's name.
        part: String,
    },
    /// `MATERIALIZE COLUMN [IF EXISTS] c [IN PARTITION p]` (HE1 R9.16): writes
    /// a column's default into rows stored without it, which a lake table's read
    /// views already read; it only checks the column.
    MaterializeColumn {
        /// The column.
        name: String,
    },
    /// Another operation ClickHouse has (`ATTACH PARTITION`, `FREEZE`, …): `48`.
    Other {
        /// Its leading words (`ATTACH PARTITION`).
        form: String,
    },
}

impl AlterOp {
    /// Whether the operation changes rows (the mutation path's) rather than
    /// metadata.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Delete { .. }
                | Self::Update { .. }
                | Self::ClearColumn { .. }
                | Self::DropPartition { .. }
                | Self::DropPart { .. }
        )
    }
}

/// The words an operation starts with: a top-level comma followed by one of
/// these ends the operation before it (`UPDATE a = 1, b = 2` and `MODIFY SETTING
/// a = 1, b = 2` hold commas of their own).
const OP_WORDS: &[&str] = &[
    "ADD",
    "DROP",
    "MODIFY",
    "RENAME",
    "COMMENT",
    "CLEAR",
    "MATERIALIZE",
    "DELETE",
    "UPDATE",
    "RESET",
    "REMOVE",
    "DETACH",
    "ATTACH",
    "FREEZE",
    "UNFREEZE",
    "MOVE",
    "REPLACE",
    "FETCH",
    "APPLY",
    "FORGET",
];

/// Reads `ALTER TABLE [db.]t [ON CLUSTER c] op[, op …] [SETTINGS …]`.
pub fn parse_alter(text: &str) -> Result<Alter, HouseError> {
    let text = text.trim().trim_end_matches(';').trim_end();
    let mut c = Cursor::new(text);
    c.expect("ALTER")?;
    if !c.eat("TABLE") {
        return Err(HouseError::from(ChError::not_implemented(format!(
            "ALTER {} is not on the House's surface; ALTER TABLE is",
            c.rest().split_whitespace().next().unwrap_or_default()
        ))));
    }
    let table = c.qualified()?;
    c.skip_on_cluster()?;
    let first = c.at;
    // The statement's own SETTINGS: a top-level `SETTINGS` (plural; `MODIFY
    // SETTING` is singular).
    let settings_at =
        (first..c.toks.len()).find(|&i| c.toks[i].depth == 0 && c.is_word_at(i, "SETTINGS"));
    let ops_end = settings_at.unwrap_or(c.toks.len());
    let settings = match settings_at {
        Some(i) => setting_pairs(&text[c.toks[i].end..])?,
        None => Vec::new(),
    };
    if first >= ops_end {
        return Err(syntax("ALTER TABLE needs an operation"));
    }
    let mut ops = Vec::new();
    let mut start = first;
    for i in first..=ops_end {
        let cut = i == ops_end
            || (c.toks[i].depth == 0
                && c.is_symbol(&c.toks[i], b',')
                && OP_WORDS.iter().any(|w| c.is_word_at(i + 1, w)));
        if !cut {
            continue;
        }
        if i == start {
            return Err(syntax("ALTER TABLE: an empty operation"));
        }
        let op_text = &text[c.toks[start].start..c.toks[i - 1].end];
        ops.push(parse_op(op_text)?);
        start = i + 1;
    }
    Ok(Alter {
        table,
        ops,
        settings,
    })
}

/// The statement up to its trailing `SETTINGS`.
pub fn without_settings(text: &str) -> &str {
    let c = Cursor::new(text);
    match (0..c.toks.len()).find(|&i| c.toks[i].depth == 0 && c.is_word_at(i, "SETTINGS")) {
        Some(i) => text[..c.toks[i].start].trim_end(),
        None => text,
    }
}

/// `a = 1, b = 'x'`.
fn setting_pairs(text: &str) -> Result<Vec<(String, String)>, HouseError> {
    let mut c = Cursor::new(text);
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
        let raw = text[start..c.offset()].trim();
        if raw.is_empty() {
            return Err(syntax(format!("Expected a value for the setting {name}")));
        }
        let value = if raw.starts_with('\'') {
            ddl::unquote(raw)
        } else {
            raw.to_string()
        };
        out.push((name, value));
        c.eat_symbol(b',');
    }
    Ok(out)
}

/// `IN PARTITION p` from the end of `text`: the text before it and `p`.
fn split_in_partition(c: &Cursor<'_>, from: usize, to: usize) -> (usize, Option<String>) {
    match (from..to).find(|&i| {
        c.toks[i].depth == 0 && c.is_word_at(i, "IN") && c.is_word_at(i + 1, "PARTITION")
    }) {
        Some(i) => {
            let end = c.toks[to - 1].end;
            (i, Some(c.text[c.toks[i + 1].end..end].trim().to_string()))
        }
        None => (to, None),
    }
}

/// The text of tokens `from..to`.
fn span(c: &Cursor<'_>, from: usize, to: usize) -> String {
    if from >= to {
        return String::new();
    }
    c.text[c.toks[from].start..c.toks[to - 1].end]
        .trim()
        .to_string()
}

/// `WHERE predicate` at the top level of tokens `from..to`.
fn where_clause(
    c: &Cursor<'_>,
    from: usize,
    to: usize,
    what: &str,
) -> Result<(usize, String), HouseError> {
    let at = (from..to)
        .find(|&i| c.toks[i].depth == 0 && c.is_word_at(i, "WHERE"))
        .ok_or_else(|| syntax(format!("ALTER TABLE … {what} needs WHERE")))?;
    let predicate = span(c, at + 1, to);
    if predicate.is_empty() {
        return Err(syntax(format!(
            "ALTER TABLE … {what} WHERE needs a predicate"
        )));
    }
    Ok((at, predicate))
}

fn parse_op(text: &str) -> Result<AlterOp, HouseError> {
    let mut c = Cursor::new(text);
    let end = c.toks.len();
    let lead = |c: &Cursor<'_>, n: usize| -> String {
        (0..n)
            .filter_map(|i| {
                c.toks
                    .get(i)
                    .filter(|t| t.word)
                    .map(|t| c.raw(t).to_ascii_uppercase())
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let if_exists = |c: &mut Cursor<'_>| c.eat_all(&["IF", "EXISTS"]);
    let if_not_exists = |c: &mut Cursor<'_>| c.eat_all(&["IF", "NOT", "EXISTS"]);
    let other = |c: &Cursor<'_>| AlterOp::Other { form: lead(c, 2) };

    if c.eat("DELETE") {
        let (where_at, predicate) = where_clause(&c, c.at, end, "DELETE")?;
        let (_, partition) = split_in_partition(&c, c.at, where_at);
        return Ok(AlterOp::Delete {
            partition,
            predicate,
        });
    }
    if c.eat("UPDATE") {
        let (where_at, predicate) = where_clause(&c, c.at, end, "UPDATE")?;
        let (assign_end, partition) = split_in_partition(&c, c.at, where_at);
        let mut assignments = Vec::new();
        let mut start = c.at;
        for i in c.at..=assign_end {
            if i < assign_end && !(c.toks[i].depth == 0 && c.is_symbol(&c.toks[i], b',')) {
                continue;
            }
            let mut a = Cursor::new(&text[c.toks[start].start..c.toks[i - 1].end]);
            let column = a.ident()?;
            if !a.eat_symbol(b'=') {
                return Err(syntax(format!("UPDATE {column}: expected =")));
            }
            let value = a.rest().trim().to_string();
            if value.is_empty() {
                return Err(syntax(format!("UPDATE {column} = needs an expression")));
            }
            assignments.push((column, value));
            start = i + 1;
        }
        return Ok(AlterOp::Update {
            assignments,
            partition,
            predicate,
        });
    }
    if c.eat_all(&["ADD", "COLUMN"]) {
        let if_not_exists = if_not_exists(&mut c);
        let (definition, position) = definition_and_position(&c, c.at, end)?;
        if definition.is_empty() {
            return Err(syntax("ADD COLUMN needs a column definition"));
        }
        return Ok(AlterOp::AddColumn(AddColumn {
            if_not_exists,
            definition,
            position,
        }));
    }
    if c.eat_all(&["DROP", "COLUMN"]) {
        let if_exists = if_exists(&mut c);
        let name = c.ident()?;
        c.expect_end()?;
        return Ok(AlterOp::DropColumn { if_exists, name });
    }
    if c.eat_all(&["CLEAR", "COLUMN"]) {
        let if_exists = if_exists(&mut c);
        let name = c.ident()?;
        let (_, partition) = split_in_partition(&c, c.at, end);
        return Ok(AlterOp::ClearColumn {
            if_exists,
            name,
            partition,
        });
    }
    if c.eat_all(&["MATERIALIZE", "COLUMN"]) {
        c.eat_all(&["IF", "EXISTS"]);
        let name = c.ident()?;
        return Ok(AlterOp::MaterializeColumn { name });
    }
    if c.eat_all(&["MODIFY", "COLUMN"]) {
        let if_exists = if_exists(&mut c);
        let name = c.ident()?;
        if c.eat("REMOVE") {
            let what = c.ident()?.to_ascii_uppercase();
            c.expect_end()?;
            return Ok(AlterOp::ModifyColumn(ModifyColumn {
                if_exists,
                name,
                definition: String::new(),
                remove: Some(what),
                position: Position::Last,
            }));
        }
        let (definition, position) = definition_and_position(&c, c.at, end)?;
        return Ok(AlterOp::ModifyColumn(ModifyColumn {
            if_exists,
            name,
            definition,
            remove: None,
            position,
        }));
    }
    if c.eat_all(&["RENAME", "COLUMN"]) {
        let if_exists = if_exists(&mut c);
        let from = c.ident()?;
        c.expect("TO")?;
        let to = c.ident()?;
        c.expect_end()?;
        return Ok(AlterOp::RenameColumn {
            if_exists,
            from,
            to,
        });
    }
    if c.eat_all(&["COMMENT", "COLUMN"]) {
        let if_exists = if_exists(&mut c);
        let name = c.ident()?;
        let comment = c.string()?;
        c.expect_end()?;
        return Ok(AlterOp::CommentColumn {
            if_exists,
            name,
            comment,
        });
    }
    if c.eat_all(&["ADD", "INDEX"]) {
        let if_not_exists = if_not_exists(&mut c);
        // A trailing `FIRST` or `AFTER idx` only orders the indexes.
        let (body, _) = definition_and_position(&c, c.at, end)?;
        return Ok(AlterOp::AddIndex {
            if_not_exists,
            index: SkipIndex::parse(&body)?,
        });
    }
    for action in ["DROP", "MATERIALIZE", "CLEAR"] {
        if c.eat_all(&[action, "INDEX"]) {
            let if_exists = if_exists(&mut c);
            let name = c.ident()?;
            let (rest, _) = split_in_partition(&c, c.at, end);
            if rest != c.at {
                return Err(syntax(format!("{action} INDEX {name}: unexpected text")));
            }
            return Ok(AlterOp::Index {
                action: action.to_string(),
                if_exists,
                name,
            });
        }
        if c.eat_all(&[action, "PROJECTION"]) {
            let if_exists = if_exists(&mut c);
            let name = c.ident()?;
            let (rest, _) = split_in_partition(&c, c.at, end);
            if rest != c.at {
                return Err(syntax(format!(
                    "{action} PROJECTION {name}: unexpected text"
                )));
            }
            return Ok(AlterOp::ProjectionOp {
                action: action.to_string(),
                if_exists,
                name,
            });
        }
    }
    if c.eat_all(&["ADD", "PROJECTION"]) {
        let if_not_exists = if_not_exists(&mut c);
        let (body, _) = definition_and_position(&c, c.at, end)?;
        return Ok(AlterOp::AddProjection {
            if_not_exists,
            projection: Projection::parse(&body)?,
        });
    }
    if c.eat_all(&["ADD", "CONSTRAINT"]) {
        let if_not_exists = if_not_exists(&mut c);
        return Ok(AlterOp::AddConstraint {
            if_not_exists,
            constraint: Constraint::parse(c.rest())?,
        });
    }
    if c.eat_all(&["DROP", "CONSTRAINT"]) {
        let if_exists = if_exists(&mut c);
        let name = c.ident()?;
        c.expect_end()?;
        return Ok(AlterOp::DropConstraint { if_exists, name });
    }
    if c.eat_all(&["MODIFY", "SETTING"]) {
        return Ok(AlterOp::ModifySetting(setting_pairs(c.rest())?));
    }
    if c.eat_all(&["RESET", "SETTING"]) {
        let mut names = vec![c.ident()?];
        while c.eat_symbol(b',') {
            names.push(c.ident()?);
        }
        c.expect_end()?;
        return Ok(AlterOp::ResetSetting(names));
    }
    if c.eat_all(&["MODIFY", "TTL"]) {
        let ttl = c.rest().trim().to_string();
        ddl::parse_ttl(&ttl)?;
        return Ok(AlterOp::ModifyTtl(ttl));
    }
    if c.eat_all(&["REMOVE", "TTL"]) {
        c.expect_end()?;
        return Ok(AlterOp::RemoveTtl);
    }
    if c.eat_all(&["MATERIALIZE", "TTL"]) {
        return Ok(AlterOp::MaterializeTtl);
    }
    if c.eat_all(&["MODIFY", "QUERY"]) {
        let select = c.rest().trim().to_string();
        if select.is_empty() {
            return Err(syntax("MODIFY QUERY needs a SELECT"));
        }
        return Ok(AlterOp::ModifyQuery(select));
    }
    if c.eat_all(&["MODIFY", "COMMENT"]) {
        let comment = c.string()?;
        c.expect_end()?;
        return Ok(AlterOp::ModifyComment(comment));
    }
    if c.eat_all(&["MODIFY", "SAMPLE", "BY"]) {
        return Ok(AlterOp::ModifySampleBy(c.rest().trim().to_string()));
    }
    if c.eat_all(&["REMOVE", "SAMPLE", "BY"]) {
        c.expect_end()?;
        return Ok(AlterOp::RemoveSampleBy);
    }
    if c.eat_all(&["DROP", "PARTITION"]) {
        let partition = c.rest().trim().to_string();
        if partition.is_empty() {
            return Err(syntax("DROP PARTITION needs a partition"));
        }
        return Ok(AlterOp::DropPartition { partition });
    }
    if c.eat_all(&["DROP", "PART"]) {
        let part = c.string()?;
        c.expect_end()?;
        return Ok(AlterOp::DropPart { part });
    }
    Ok(other(&c))
}

/// A definition up to a trailing `FIRST` or `AFTER <name>`.
fn definition_and_position(
    c: &Cursor<'_>,
    from: usize,
    to: usize,
) -> Result<(String, Position), HouseError> {
    if to > from && c.is_word_at(to - 1, "FIRST") && c.toks[to - 1].depth == 0 {
        return Ok((span(c, from, to - 1), Position::First));
    }
    if to >= from + 2 && c.is_word_at(to - 2, "AFTER") && c.toks[to - 2].depth == 0 {
        let mut after = Cursor::new(c.raw(&c.toks[to - 1]));
        return Ok((span(c, from, to - 2), Position::After(after.ident()?)));
    }
    Ok((span(c, from, to), Position::Last))
}

// ---------------------------------------------------------------------------
// Execution.

fn not_implemented(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::not_implemented(message.into()))
}

fn bad(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::bad_arguments(message.into()))
}

/// `44 ILLEGAL_COLUMN`, ClickHouse's answer to `DROP`/`MODIFY`/`RENAME` of a
/// column that is not there.
fn no_column(name: &str, what: &str) -> HouseError {
    HouseError::Engine(loams_house_ipc::EngineError {
        code: 10,
        name: "NOT_FOUND_COLUMN_IN_BLOCK".to_string(),
        message: format!(
            "Wrong column name. Cannot find column {} to {what}",
            quote_name(name)
        ),
    })
}

/// `524 ALTER_OF_COLUMN_IS_FORBIDDEN`, as ClickHouse answers for a key column.
fn forbidden(message: String) -> HouseError {
    HouseError::Engine(loams_house_ipc::EngineError {
        code: 524,
        name: "ALTER_OF_COLUMN_IS_FORBIDDEN".to_string(),
        message,
    })
}

/// The ClickHouse codes this module answers with beyond [`ddl::CATALOG_CODES`]
/// (each is ClickHouse's own answer to the same `ALTER`).
pub const ALTER_CODES: &[(i32, &str)] = &[
    (10, "NOT_FOUND_COLUMN_IN_BLOCK"),
    (524, "ALTER_OF_COLUMN_IS_FORBIDDEN"),
];

/// Runs an `ALTER TABLE` for namespace `namespace` whose current database is
/// `current`.
pub async fn execute(
    lake: &Lake,
    namespace: u64,
    current: &str,
    text: &str,
    oracle: &mut dyn Oracle,
) -> Result<(), HouseError> {
    let alter = parse_alter(text)?;
    for (name, _) in &alter.settings {
        if !crate::settings::is_allowed(name) {
            return Err(HouseError::from(ChError::unknown_setting(format!(
                "Setting {name} is not one an ALTER may set on the House"
            ))));
        }
    }
    let (db, name) = alter.table.clone();
    let database = db.unwrap_or_else(|| current.to_string());

    // A materialized view's query.
    if let [AlterOp::ModifyQuery(select)] = alter.ops.as_slice() {
        return crate::pipes::modify_materialized_view_query(
            lake, namespace, &database, &name, select,
        )
        .await;
    }
    if alter
        .ops
        .iter()
        .any(|op| matches!(op, AlterOp::ModifyQuery(_)))
    {
        return Err(not_implemented(
            "MODIFY QUERY changes a materialized view, alone in its ALTER",
        ));
    }
    if let Some(AlterOp::Other { form }) = alter
        .ops
        .iter()
        .find(|op| matches!(op, AlterOp::Other { .. }))
    {
        return Err(not_implemented(format!(
            "ALTER TABLE … {form} is not on the House's surface (HE1 R9.12)"
        )));
    }
    // A Distributed table reads its local table's columns (R9.13): a column or
    // metadata ALTER of it, which ClickHouse makes on every node beside the
    // local table's, changes no rows (SigNoz; HE1 R9.16). Its column list (what
    // `DESCRIBE`, `system.columns` and a native `INSERT`'s header show) becomes
    // the local table's as it now is (HE1 R9.71): SigNoz alters the local
    // table, then the Distributed one, and its collector inserts through the
    // Distributed table's columns.
    if let Some(loaded) = lake.load(namespace, &database, &name).await?
        && ddl::class_of(&loaded.metadata) == ddl::TableClass::View
        && let Some((local_db, local)) = ddl::distributed_target(&loaded)
        && !alter.ops.iter().any(AlterOp::is_mutation)
    {
        if let Some(local_table) = lake.load(namespace, &local_db, &local).await? {
            let shown = ddl::ChTable::from_lake(&local_db, &local, &local_table);
            let types = ddl::json(&shown.columns)?;
            if loaded.metadata.properties().get(ddl::PROP_TYPES) != Some(&types) {
                lake.catalog()
                    .commit(
                        &loaded.ident,
                        Vec::new(),
                        vec![TableUpdate::SetProperties {
                            updates: HashMap::from([(ddl::PROP_TYPES.to_string(), types)]),
                        }],
                    )
                    .await?;
            }
        }
        return Ok(());
    }
    let table = lake
        .lake_table(namespace, &database, &name, "ALTER TABLE")
        .await?;
    let mutations = alter.ops.iter().filter(|op| op.is_mutation()).count();
    if mutations > 0 {
        if mutations != alter.ops.len() {
            return Err(not_implemented(
                "An ALTER on the House either changes rows (DELETE, UPDATE, CLEAR COLUMN, DROP \
                 PARTITION) or changes the table, not both",
            ));
        }
        return mutation(lake, namespace, &database, &name, table, &alter).await;
    }
    // The deny list reads the operations; the trailing SETTINGS are the
    // front's (R9.11), which chDB's profile would refuse.
    oracle.check(without_settings(text)).await?;
    let mut edit = Edit::new(&database, &name, &table);
    for op in &alter.ops {
        edit.apply(op, oracle).await?;
    }
    edit.commit(lake, &table).await
}

/// Mutations (`ALTER TABLE … DELETE`, `UPDATE`, `CLEAR COLUMN`, `DROP
/// PARTITION`, `DROP PART`) are classified as `Stmt::Mutation` and served by
/// [`crate::mutate`] with the worker pool (HE1 R9.5); one that reaches the DDL
/// path anyway (a caller that did not classify it) is `48`, named.
async fn mutation(
    _lake: &Lake,
    _namespace: u64,
    database: &str,
    name: &str,
    _table: LakeTable,
    alter: &Alter,
) -> Result<(), HouseError> {
    let form = match &alter.ops[0] {
        AlterOp::Delete { .. } => "DELETE",
        AlterOp::Update { .. } => "UPDATE",
        AlterOp::ClearColumn { .. } => "CLEAR COLUMN",
        AlterOp::DropPartition { .. } => "DROP PARTITION",
        _ => "DROP PART",
    };
    Err(not_implemented(format!(
        "ALTER TABLE {database}.{name} {form}: a mutation is served on the ClickHouse HTTP \
         interface and loams.house.v1's statements (HE1 R9.5), not here"
    )))
}

/// The names an expression's text mentions (its words and quoted names).
fn mentions(text: &str, column: &str) -> bool {
    let c = Cursor::new(text);
    c.toks.iter().any(|t| {
        let raw = c.raw(t);
        (t.word && raw == column)
            || ((raw.starts_with('`') || raw.starts_with('"')) && ddl::unquote(raw) == column)
    })
}

/// An `Enum`'s `'name' = value` pairs, if `text` is one.
fn enum_pairs(text: &str) -> Option<Vec<(String, i64)>> {
    let inner = text
        .trim()
        .strip_prefix("Enum8(")
        .or_else(|| text.trim().strip_prefix("Enum16("))?
        .strip_suffix(')')?;
    let mut out = Vec::new();
    let mut rest = inner.trim();
    while !rest.is_empty() {
        let body = rest.strip_prefix('\'')?;
        let end = body.find('\'')?;
        let name = body[..end].to_string();
        let after = body[end + 1..].trim_start().strip_prefix('=')?.trim_start();
        let number_end = after.find(',').unwrap_or(after.len());
        let value = after[..number_end].trim().parse().ok()?;
        out.push((name, value));
        rest = after[number_end..].trim_start_matches(',').trim_start();
    }
    Some(out)
}

/// Whether `new` is an `Enum` holding every `name = value` of the `Enum` `old`:
/// a key column may take it (ClickHouse allows it, and the stored strings are
/// unchanged; SigNoz's `tagType`, HE1 R9.16).
fn enum_extends(old: &str, new: &str) -> bool {
    match (enum_pairs(old), enum_pairs(new)) {
        (Some(old), Some(new)) => old.iter().all(|pair| new.contains(pair)),
        _ => false,
    }
}

/// An `ALTER`'s changes to one table, applied in memory, then committed once.
struct Edit {
    ch: ChTable,
    fields: Vec<NestedFieldRef>,
    ids: FieldIds,
    needs: Needs,
    schema_changed: bool,
    changed: bool,
    v3: bool,
}

impl Edit {
    fn new(database: &str, name: &str, table: &LakeTable) -> Self {
        let v3 = table.metadata.format_version() == loams_house_lake::spec::FormatVersion::V3;
        Self {
            ch: ChTable::from_lake(database, name, table),
            fields: table
                .metadata
                .current_schema()
                .as_struct()
                .fields()
                .to_vec(),
            ids: FieldIds::after(table.metadata.last_column_id()),
            needs: if v3 { Needs::V3 } else { Needs::V2 },
            schema_changed: false,
            changed: false,
            v3,
        }
    }

    fn column_index(&self, name: &str) -> Option<usize> {
        self.ch.columns.iter().position(|c| c.name == name)
    }

    /// Where a column is named by something that would break without it.
    fn key_use(&self, column: &str) -> Option<&'static str> {
        let engine_args = self.ch.engine.split_once('(').map(|(_, a)| a).unwrap_or("");
        let ttl_uses = self.ch.ttl.as_str();
        [
            (self.ch.order_by.as_str(), "the sorting key"),
            (self.ch.primary_key.as_str(), "the primary key"),
            (self.ch.partition_by.as_str(), "the partition key"),
            (self.ch.sample_by.as_str(), "SAMPLE BY"),
            (ttl_uses, "the TTL"),
            (engine_args, "the engine's arguments"),
        ]
        .into_iter()
        .find(|(text, _)| mentions(text, column))
        .map(|(_, what)| what)
    }

    fn position_at(&self, position: &Position) -> Result<usize, HouseError> {
        Ok(match position {
            Position::Last => self.ch.columns.len(),
            Position::First => 0,
            Position::After(after) => {
                self.column_index(after)
                    .ok_or_else(|| ddl::unknown_column(after, "ADD COLUMN … AFTER"))?
                    + 1
            }
        })
    }

    /// Iceberg's field order follows the stored columns' order.
    fn reorder_fields(&mut self) {
        let order: Vec<&str> = self
            .ch
            .columns
            .iter()
            .filter(|c| c.stored())
            .map(|c| c.name.as_str())
            .collect();
        self.fields.sort_by_key(|f| {
            order
                .iter()
                .position(|n| *n == f.name)
                .unwrap_or(usize::MAX)
        });
    }

    async fn apply(&mut self, op: &AlterOp, oracle: &mut dyn Oracle) -> Result<(), HouseError> {
        match op {
            AlterOp::AddColumn(add) => self.add_column(add, oracle).await,
            AlterOp::DropColumn { if_exists, name } => {
                let Some(at) = self.column_index(name) else {
                    return if *if_exists {
                        Ok(())
                    } else {
                        Err(no_column(name, "drop"))
                    };
                };
                if let Some(what) = self.key_use(name) {
                    return Err(forbidden(format!(
                        "Trying to ALTER DROP key {} column which is a part of {what}",
                        quote_name(name)
                    )));
                }
                let column = self.ch.columns.remove(at);
                if column.stored() {
                    self.fields.retain(|f| f.name != *name);
                    self.schema_changed = true;
                }
                self.changed = true;
                Ok(())
            }
            AlterOp::ModifyColumn(modify) => self.modify_column(modify, oracle).await,
            AlterOp::RenameColumn {
                if_exists,
                from,
                to,
            } => {
                let Some(at) = self.column_index(from) else {
                    return if *if_exists {
                        Ok(())
                    } else {
                        Err(no_column(from, "rename"))
                    };
                };
                if self.column_index(to).is_some() {
                    return Err(ddl::clickhouse_error(
                        15,
                        "DUPLICATE_COLUMN",
                        format!(
                            "Cannot rename column to {}: column with this name already exists",
                            quote_name(to)
                        ),
                    ));
                }
                if let Some(what) = self.key_use(from) {
                    return Err(forbidden(format!(
                        "Trying to ALTER RENAME key {} column which is a part of {what}",
                        quote_name(from)
                    )));
                }
                if self.ch.columns[at].stored() {
                    for field in &mut self.fields {
                        if field.name == *from {
                            let mut renamed: NestedField = field.as_ref().clone();
                            renamed.name = to.clone();
                            *field = renamed.into();
                        }
                    }
                    self.schema_changed = true;
                }
                self.ch.columns[at].name = to.clone();
                self.changed = true;
                Ok(())
            }
            AlterOp::CommentColumn {
                if_exists,
                name,
                comment,
            } => {
                let Some(at) = self.column_index(name) else {
                    return if *if_exists {
                        Ok(())
                    } else {
                        Err(no_column(name, "comment"))
                    };
                };
                self.ch.columns[at].comment = comment.clone();
                self.set_doc(name, comment);
                self.changed = true;
                Ok(())
            }
            AlterOp::AddIndex {
                if_not_exists,
                index,
            } => {
                if self.ch.indexes.iter().any(|i| i.name == index.name) {
                    return if *if_not_exists {
                        Ok(())
                    } else {
                        Err(bad(format!(
                            "Cannot add index {}: index with this name already exists",
                            index.name
                        )))
                    };
                }
                self.ch.indexes.push(index.clone());
                self.changed = true;
                Ok(())
            }
            AlterOp::Index {
                action,
                if_exists,
                name,
            } => {
                let Some(at) = self.ch.indexes.iter().position(|i| i.name == *name) else {
                    return if *if_exists {
                        Ok(())
                    } else {
                        Err(bad(format!(
                            "Wrong index name. Cannot find index {} to {}",
                            quote_name(name),
                            action.to_ascii_lowercase()
                        )))
                    };
                };
                if action == "DROP" {
                    self.ch.indexes.remove(at);
                    self.changed = true;
                }
                Ok(())
            }
            AlterOp::AddProjection {
                if_not_exists,
                projection,
            } => {
                if self
                    .ch
                    .projections
                    .iter()
                    .any(|p| p.name == projection.name)
                {
                    return if *if_not_exists {
                        Ok(())
                    } else {
                        Err(bad(format!(
                            "Cannot add projection {}: projection with this name already exists",
                            projection.name
                        )))
                    };
                }
                self.ch.projections.push(projection.clone());
                self.changed = true;
                Ok(())
            }
            AlterOp::ProjectionOp {
                action,
                if_exists,
                name,
            } => {
                let Some(at) = self.ch.projections.iter().position(|p| p.name == *name) else {
                    return if *if_exists {
                        Ok(())
                    } else {
                        Err(bad(format!("There is no projection {name} in table")))
                    };
                };
                if action == "DROP" {
                    self.ch.projections.remove(at);
                    self.changed = true;
                }
                Ok(())
            }
            AlterOp::AddConstraint {
                if_not_exists,
                constraint,
            } => {
                if self
                    .ch
                    .constraints
                    .iter()
                    .any(|k| k.name == constraint.name)
                {
                    return if *if_not_exists {
                        Ok(())
                    } else {
                        Err(bad(format!(
                            "Constraint {} already exists",
                            constraint.name
                        )))
                    };
                }
                crate::expr::reprint(&constraint.expression)
                    .map_err(|why| bad(format!("CONSTRAINT {}: {why}", constraint.name)))?;
                self.ch.constraints.push(constraint.clone());
                self.changed = true;
                Ok(())
            }
            AlterOp::DropConstraint { if_exists, name } => {
                let before = self.ch.constraints.len();
                self.ch.constraints.retain(|k| k.name != *name);
                if self.ch.constraints.len() == before && !*if_exists {
                    return Err(bad(format!(
                        "Wrong constraint name. Cannot find constraint `{name}` to drop"
                    )));
                }
                self.changed |= self.ch.constraints.len() != before;
                Ok(())
            }
            AlterOp::ModifySetting(pairs) => {
                let names: Vec<String> = pairs
                    .iter()
                    .map(|(k, _)| k.clone())
                    .filter(|k| !k.starts_with("loams_"))
                    .collect();
                let known = if names.is_empty() {
                    Vec::new()
                } else {
                    oracle.merge_tree_settings(&names).await?
                };
                for (key, value) in pairs {
                    if key.starts_with("loams_") {
                        if !matches!(key.as_str(), "loams_tiering_freshness" | "loams_bucket_key") {
                            return Err(bad(format!("{key} cannot be changed by ALTER TABLE")));
                        }
                    } else if !known.contains(key) {
                        return Err(HouseError::from(ChError::unknown_setting(format!(
                            "Unknown setting '{key}': for storage MergeTree"
                        ))));
                    }
                    match self.ch.settings.iter_mut().find(|(k, _)| k == key) {
                        Some(pair) => pair.1 = value.clone(),
                        None => self.ch.settings.push((key.clone(), value.clone())),
                    }
                }
                self.changed = true;
                Ok(())
            }
            AlterOp::ResetSetting(names) => {
                self.ch.settings.retain(|(k, _)| !names.contains(k));
                self.changed = true;
                Ok(())
            }
            AlterOp::ModifyTtl(ttl) => {
                for rule in ddl::parse_ttl(ttl)? {
                    for text in std::iter::once(&rule.expression).chain(rule.condition.iter()) {
                        crate::expr::reprint(text).map_err(|why| bad(format!("TTL: {why}")))?;
                    }
                }
                self.ch.ttl = ttl.clone();
                self.changed = true;
                Ok(())
            }
            AlterOp::RemoveTtl => {
                if self.ch.ttl.is_empty() {
                    return Err(bad(
                        "Table doesn't have any table TTL expression, cannot remove",
                    ));
                }
                self.ch.ttl.clear();
                self.changed = true;
                Ok(())
            }
            AlterOp::MaterializeTtl => Ok(()),
            AlterOp::MaterializeColumn { name } => match self.column_index(name) {
                Some(_) => Ok(()),
                None => Err(no_column(name, "materialize")),
            },
            AlterOp::ModifyComment(comment) => {
                self.ch.comment = comment.clone();
                self.changed = true;
                Ok(())
            }
            AlterOp::ModifySampleBy(expr) => {
                self.ch.sample_by = expr.clone();
                self.changed = true;
                Ok(())
            }
            AlterOp::RemoveSampleBy => {
                self.ch.sample_by.clear();
                self.changed = true;
                Ok(())
            }
            AlterOp::ModifyQuery(_)
            | AlterOp::Other { .. }
            | AlterOp::Delete { .. }
            | AlterOp::Update { .. }
            | AlterOp::ClearColumn { .. }
            | AlterOp::DropPartition { .. }
            | AlterOp::DropPart { .. } => {
                Err(not_implemented("not a change to a table's metadata"))
            }
        }
    }

    fn set_doc(&mut self, name: &str, comment: &str) {
        for field in &mut self.fields {
            if field.name == name {
                let mut with_doc: NestedField = field.as_ref().clone();
                with_doc.doc = (!comment.is_empty()).then(|| comment.to_string());
                *field = with_doc.into();
                self.schema_changed = true;
            }
        }
    }

    /// ClickHouse's reading of one column definition, next to the table's other
    /// columns (names and types only), which its expressions may name.
    async fn describe_one(
        &self,
        definition: &str,
        except: Option<&str>,
        oracle: &mut dyn Oracle,
    ) -> Result<ddl::Described, HouseError> {
        let mut list: Vec<String> = Vec::new();
        for column in &self.ch.columns {
            if Some(column.name.as_str()) == except {
                continue;
            }
            let Some(ty) = crate::views::printable_type(&column.ch_type) else {
                return Err(bad(format!(
                    "Column {} has a type the House cannot name",
                    column.name
                )));
            };
            list.push(format!("{} {ty}", quote_name(&column.name)));
        }
        let context = list.len();
        list.push(definition.to_string());
        let described = oracle.describe(&list.join(", ")).await?;
        if described.len() != context + 1 {
            return Err(syntax(format!("{definition}: one column at a time")));
        }
        Ok(described
            .into_iter()
            .next_back()
            .expect("one more than the context"))
    }

    async fn add_column(
        &mut self,
        add: &AddColumn,
        oracle: &mut dyn Oracle,
    ) -> Result<(), HouseError> {
        let named = Cursor::new(&add.definition).ident()?;
        if self.column_index(&named).is_some() {
            if add.if_not_exists {
                return Ok(());
            }
            return Err(ddl::clickhouse_error(
                15,
                "DUPLICATE_COLUMN",
                format!(
                    "Cannot add column {}: column with this name already exists",
                    quote_name(&named)
                ),
            ));
        }
        let described = self.describe_one(&add.definition, None, oracle).await?;
        let column = &described;
        if !column.ttl.is_empty() {
            return Err(not_implemented(
                "A column TTL is not supported on lake tables (§49 §6.3)",
            ));
        }
        let column = column.column.clone();
        if self.column_index(&column.name).is_some() {
            if add.if_not_exists {
                return Ok(());
            }
            return Err(ddl::clickhouse_error(
                15,
                "DUPLICATE_COLUMN",
                format!(
                    "Cannot add column {}: column with this name already exists",
                    quote_name(&column.name)
                ),
            ));
        }
        let at = self.position_at(&add.position)?;
        if column.stored() {
            let mut field: NestedField = types::to_iceberg_field(
                &column.name,
                &column.ch_type,
                &mut self.ids,
                true,
                &mut self.needs,
            )?
            .as_ref()
            .clone();
            if !column.comment.is_empty() {
                field.doc = Some(column.comment.clone());
            }
            self.fields.push(field.into());
            self.schema_changed = true;
        }
        self.ch.columns.insert(at, column);
        self.reorder_fields();
        self.changed = true;
        Ok(())
    }

    async fn modify_column(
        &mut self,
        modify: &ModifyColumn,
        oracle: &mut dyn Oracle,
    ) -> Result<(), HouseError> {
        let Some(at) = self.column_index(&modify.name) else {
            return if modify.if_exists {
                Ok(())
            } else {
                Err(no_column(&modify.name, "modify"))
            };
        };
        let old = self.ch.columns[at].clone();
        if let Some(what) = &modify.remove {
            let column = &mut self.ch.columns[at];
            match what.as_str() {
                "DEFAULT" | "MATERIALIZED" | "ALIAS" if column.default_kind == *what => {
                    if !column.stored() {
                        return Err(not_implemented(
                            "REMOVE ALIAS would make a column with no rows stored",
                        ));
                    }
                    column.default_kind.clear();
                    column.default_expression.clear();
                }
                "DEFAULT" | "MATERIALIZED" | "ALIAS" => {
                    return Err(bad(format!(
                        "Column {} doesn't have {what}, cannot remove it",
                        quote_name(&modify.name)
                    )));
                }
                "CODEC" => column.codec.clear(),
                "COMMENT" => {
                    column.comment.clear();
                    self.set_doc(&modify.name, "");
                }
                "TTL" => {}
                other => return Err(syntax(format!("MODIFY COLUMN … REMOVE {other}"))),
            }
            self.changed = true;
            return Ok(());
        }
        if !modify.definition.is_empty() {
            // Without a type, ClickHouse keeps the column's own.
            let first = Cursor::new(&modify.definition)
                .toks
                .first()
                .map(|t| modify.definition[t.start..t.end].to_ascii_uppercase())
                .unwrap_or_default();
            let typed = !matches!(
                first.as_str(),
                "DEFAULT"
                    | "MATERIALIZED"
                    | "ALIAS"
                    | "EPHEMERAL"
                    | "CODEC"
                    | "COMMENT"
                    | "TTL"
                    | "SETTINGS"
            );
            let definition = if typed {
                format!("{} {}", quote_name(&modify.name), modify.definition)
            } else {
                format!(
                    "{} {} {}",
                    quote_name(&modify.name),
                    old.ch_type,
                    modify.definition
                )
            };
            let new = &self
                .describe_one(&definition, Some(&modify.name), oracle)
                .await?;
            if !new.ttl.is_empty() {
                return Err(not_implemented(
                    "A column TTL is not supported on lake tables (§49 §6.3)",
                ));
            }
            let mut column = new.column.clone();
            if !typed {
                // `MODIFY COLUMN c COMMENT 'x'` keeps the default; `… DEFAULT e`
                // keeps the comment and codec.
                if column.default_kind.is_empty() {
                    column.default_kind = old.default_kind.clone();
                    column.default_expression = old.default_expression.clone();
                }
                if column.comment.is_empty() {
                    column.comment = old.comment.clone();
                }
                if column.codec.is_empty() {
                    column.codec = old.codec.clone();
                }
            }
            if column.stored() != old.stored() {
                return Err(not_implemented(format!(
                    "MODIFY COLUMN {}: an ALIAS column and a stored one are not changed into each other \
                     on lake tables",
                    modify.name
                )));
            }
            if column.ch_type != old.ch_type && old.stored() {
                if self.key_use(&modify.name).is_some()
                    && !enum_extends(&old.ch_type, &column.ch_type)
                {
                    return Err(forbidden(format!(
                        "ALTER of key column {} is forbidden",
                        quote_name(&modify.name)
                    )));
                }
                self.retype(&modify.name, &old.ch_type, &column.ch_type)?;
            }
            if column.comment != old.comment {
                self.set_doc(&modify.name, &column.comment);
            }
            self.ch.columns[at] = column;
        }
        if modify.position != Position::Last {
            let column = self.ch.columns.remove(at);
            let to = self.position_at(&modify.position)?;
            self.ch.columns.insert(to, column);
            self.reorder_fields();
            self.schema_changed = true;
        }
        self.changed = true;
        Ok(())
    }

    /// A stored column's new type, when it needs no rewrite: the same Iceberg
    /// type (optional when the new type is `Nullable`), or an Iceberg promotion.
    fn retype(&mut self, name: &str, old: &str, new: &str) -> Result<(), HouseError> {
        let Some(index) = self.fields.iter().position(|f| f.name == name) else {
            return Err(no_column(name, "modify"));
        };
        let current = self.fields[index].clone();
        let mut scratch = FieldIds::after(0);
        let mut needs = self.needs;
        let wanted = types::to_iceberg_field(name, new, &mut scratch, false, &mut needs)?;
        if needs == Needs::V3 && !self.v3 {
            return Err(not_implemented(
                "A DateTime64 column with more than 6 digits needs Iceberg format v3; this table is v2",
            ));
        }
        let promoted = |from: &Type, to: &Type| match (from, to) {
            (Type::Primitive(PrimitiveType::Int), Type::Primitive(PrimitiveType::Long))
            | (Type::Primitive(PrimitiveType::Float), Type::Primitive(PrimitiveType::Double)) => {
                true
            }
            (
                Type::Primitive(PrimitiveType::Decimal {
                    precision: p1,
                    scale: s1,
                }),
                Type::Primitive(PrimitiveType::Decimal {
                    precision: p2,
                    scale: s2,
                }),
            ) => s1 == s2 && p2 >= p1,
            (a, b) => a == b,
        };
        let same_type = promoted(&current.field_type, &wanted.field_type);
        // Nullable may be added (the field becomes optional), never removed.
        let tightened = !current.required && wanted.required;
        if !same_type || tightened {
            return Err(not_implemented(format!(
                "MODIFY COLUMN {name} {new}: changing {old} to {new} would rewrite the column's data, \
                 which lake tables do not do yet (HE2); a change that keeps the stored type, adds \
                 Nullable or widens Int32, Float32 or a Decimal is served"
            )));
        }
        let mut field: NestedField = current.as_ref().clone();
        field.field_type = wanted.field_type.clone();
        field.required = current.required && wanted.required;
        self.fields[index] = field.into();
        self.schema_changed = true;
        Ok(())
    }

    async fn commit(mut self, lake: &Lake, table: &LakeTable) -> Result<(), HouseError> {
        if !self.changed {
            return Ok(());
        }
        self.ch.ddl = self.ch.render();
        let ch = &self.ch;
        let mut set = HashMap::from([
            (ddl::PROP_TYPES.to_string(), ddl::json(&ch.columns)?),
            (ddl::PROP_DDL.to_string(), ch.ddl.clone()),
        ]);
        let mut remove = Vec::new();
        let mut optional = |key: &str, value: Option<String>| match value {
            Some(v) if !v.is_empty() => {
                set.insert(key.to_string(), v);
            }
            _ => remove.push(key.to_string()),
        };
        optional(
            ddl::PROP_INDEXES,
            (!ch.indexes.is_empty())
                .then(|| ddl::json(&ch.indexes))
                .transpose()?,
        );
        optional(
            ddl::PROP_PROJECTIONS,
            (!ch.projections.is_empty())
                .then(|| ddl::json(&ch.projections))
                .transpose()?,
        );
        optional(
            ddl::PROP_CONSTRAINTS,
            (!ch.constraints.is_empty())
                .then(|| ddl::json(&ch.constraints))
                .transpose()?,
        );
        optional(
            ddl::PROP_SETTINGS,
            (!ch.settings.is_empty())
                .then(|| ddl::json(&ch.settings))
                .transpose()?,
        );
        optional(ddl::PROP_SAMPLE_BY, Some(ch.sample_by.clone()));
        optional(ddl::PROP_TTL, Some(ch.ttl.clone()));
        optional(ddl::PROP_COMMENT, Some(ch.comment.clone()));
        let present = table.metadata.properties();
        remove.retain(|k| present.contains_key(k));
        let committed = if self.schema_changed {
            let schema = Schema::builder()
                .with_fields(self.fields.clone())
                .build()
                .map_err(|err| bad(err.to_string()))?;
            lake.catalog()
                .update_schema(table, schema, set)
                .await
                .map_err(HouseError::from)?
        } else {
            lake.catalog()
                .commit(
                    &table.ident,
                    Vec::new(),
                    vec![TableUpdate::SetProperties { updates: set }],
                )
                .await
                .map_err(HouseError::from)?
        };
        if !remove.is_empty() {
            lake.catalog()
                .commit(
                    &committed.ident,
                    Vec::new(),
                    vec![TableUpdate::RemoveProperties { removals: remove }],
                )
                .await
                .map_err(HouseError::from)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(text: &str) -> Vec<AlterOp> {
        parse_alter(text)
            .unwrap_or_else(|e| panic!("{text}: {e}"))
            .ops
    }

    /// Every `ALTER` Langfuse v3's and v4's migrations send (R9.12) reads.
    #[test]
    fn langfuse_alters_read() {
        let a = parse_alter(
            "ALTER TABLE scores ON CLUSTER default ADD COLUMN session_id Nullable(String) AFTER trace_id \
             SETTINGS mutations_sync = 2",
        )
        .expect("parses");
        assert_eq!(a.table, (None, "scores".to_string()));
        assert_eq!(
            a.settings,
            vec![("mutations_sync".to_string(), "2".to_string())]
        );
        assert_eq!(
            a.ops,
            vec![AlterOp::AddColumn(AddColumn {
                if_not_exists: false,
                definition: "session_id Nullable(String)".into(),
                position: Position::After("trace_id".into()),
            })]
        );
        assert_eq!(
            ops(
                "ALTER TABLE scores ADD INDEX IF NOT EXISTS idx (project_id, trace_id) TYPE bloom_filter(0.001) GRANULARITY 1 SETTINGS mutations_sync = 2"
            ),
            vec![AlterOp::AddIndex {
                if_not_exists: true,
                index: SkipIndex {
                    name: "idx".into(),
                    expression: "(project_id, trace_id)".into(),
                    index_type: "bloom_filter(0.001)".into(),
                    granularity: Some("1".into()),
                },
            }]
        );
        assert_eq!(
            ops(
                "ALTER TABLE observations ADD INDEX IF NOT EXISTS t type TYPE bloom_filter GRANULARITY 1"
            ),
            vec![AlterOp::AddIndex {
                if_not_exists: true,
                index: SkipIndex {
                    name: "t".into(),
                    expression: "type".into(),
                    index_type: "bloom_filter".into(),
                    granularity: Some("1".into()),
                },
            }]
        );
        assert_eq!(
            ops("ALTER TABLE traces MATERIALIZE INDEX IF EXISTS idx_session_id"),
            vec![AlterOp::Index {
                action: "MATERIALIZE".into(),
                if_exists: true,
                name: "idx_session_id".into()
            }]
        );
        assert_eq!(
            ops("ALTER TABLE observations DROP INDEX IF EXISTS idx_project_id"),
            vec![AlterOp::Index {
                action: "DROP".into(),
                if_exists: true,
                name: "idx_project_id".into()
            }]
        );
        assert_eq!(
            ops(
                "ALTER TABLE scores MODIFY COLUMN trace_id Nullable(String) SETTINGS mutations_sync = 2"
            ),
            vec![AlterOp::ModifyColumn(ModifyColumn {
                if_exists: false,
                name: "trace_id".into(),
                definition: "Nullable(String)".into(),
                remove: None,
                position: Position::Last,
            })]
        );
        assert_eq!(
            ops(
                "ALTER TABLE scores MODIFY SETTING enable_block_number_column = 1, enable_block_offset_column = 1"
            ),
            vec![AlterOp::ModifySetting(vec![
                ("enable_block_number_column".into(), "1".into()),
                ("enable_block_offset_column".into(), "1".into()),
            ])]
        );
        let modify = parse_alter(
            "ALTER TABLE events_core_mv MODIFY QUERY SELECT a, leftUTF8(b, 200) AS b FROM events_full \
             SETTINGS enable_full_text_index = 1",
        )
        .expect("parses");
        assert_eq!(
            modify.ops,
            vec![AlterOp::ModifyQuery(
                "SELECT a, leftUTF8(b, 200) AS b FROM events_full".into()
            )]
        );
        assert_eq!(modify.settings[0].0, "enable_full_text_index");
        assert_eq!(
            without_settings("ALTER TABLE t ADD COLUMN a UInt8 SETTINGS mutations_sync = 2"),
            "ALTER TABLE t ADD COLUMN a UInt8"
        );
    }

    #[test]
    fn operations_split_at_their_own_words() {
        assert_eq!(
            ops(
                "ALTER TABLE t ADD COLUMN a UInt8 DEFAULT 1, ADD COLUMN b String FIRST, DROP COLUMN IF EXISTS c"
            ),
            vec![
                AlterOp::AddColumn(AddColumn {
                    if_not_exists: false,
                    definition: "a UInt8 DEFAULT 1".into(),
                    position: Position::Last
                }),
                AlterOp::AddColumn(AddColumn {
                    if_not_exists: false,
                    definition: "b String".into(),
                    position: Position::First
                }),
                AlterOp::DropColumn {
                    if_exists: true,
                    name: "c".into()
                },
            ]
        );
        // Commas of their own: UPDATE's assignments, a tuple, a function call.
        assert_eq!(
            ops(
                "ALTER TABLE t UPDATE a = 1, b = concat(c, 'x, y') IN PARTITION 202610 WHERE (k, j) IN ((1, 2))"
            ),
            vec![AlterOp::Update {
                assignments: vec![
                    ("a".into(), "1".into()),
                    ("b".into(), "concat(c, 'x, y')".into())
                ],
                partition: Some("202610".into()),
                predicate: "(k, j) IN ((1, 2))".into(),
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t DELETE WHERE project_id = 'p' AND trace_id IN ('a', 'b')"),
            vec![AlterOp::Delete {
                partition: None,
                predicate: "project_id = 'p' AND trace_id IN ('a', 'b')".into()
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t DELETE IN PARTITION ID '202610' WHERE x = 1"),
            vec![AlterOp::Delete {
                partition: Some("ID '202610'".into()),
                predicate: "x = 1".into()
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t DROP PARTITION 202610, DROP PART 'all_1_1_0'"),
            vec![
                AlterOp::DropPartition {
                    partition: "202610".into()
                },
                AlterOp::DropPart {
                    part: "all_1_1_0".into()
                },
            ]
        );
        assert!(ops("ALTER TABLE t DELETE WHERE x = 1")[0].is_mutation());
        assert!(!ops("ALTER TABLE t DROP COLUMN x")[0].is_mutation());
    }

    #[test]
    fn every_other_form_reads() {
        assert_eq!(
            ops("ALTER TABLE t MODIFY TTL d + INTERVAL 1 DAY"),
            vec![AlterOp::ModifyTtl("d + INTERVAL 1 DAY".into())]
        );
        assert_eq!(ops("ALTER TABLE t REMOVE TTL"), vec![AlterOp::RemoveTtl]);
        assert_eq!(
            ops("ALTER TABLE t MATERIALIZE TTL IN PARTITION 1"),
            vec![AlterOp::MaterializeTtl]
        );
        assert_eq!(
            ops("ALTER TABLE t MODIFY COMMENT 'c'"),
            vec![AlterOp::ModifyComment("c".into())]
        );
        assert_eq!(
            ops("ALTER TABLE t RESET SETTING a, b"),
            vec![AlterOp::ResetSetting(vec!["a".into(), "b".into()])]
        );
        assert_eq!(
            ops("ALTER TABLE t RENAME COLUMN IF EXISTS a TO b"),
            vec![AlterOp::RenameColumn {
                if_exists: true,
                from: "a".into(),
                to: "b".into()
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t COMMENT COLUMN a 'x'"),
            vec![AlterOp::CommentColumn {
                if_exists: false,
                name: "a".into(),
                comment: "x".into()
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t MODIFY COLUMN a REMOVE DEFAULT"),
            vec![AlterOp::ModifyColumn(ModifyColumn {
                if_exists: false,
                name: "a".into(),
                definition: String::new(),
                remove: Some("DEFAULT".into()),
                position: Position::Last,
            })]
        );
        assert_eq!(
            ops("ALTER TABLE t ADD CONSTRAINT c CHECK a > 0"),
            vec![AlterOp::AddConstraint {
                if_not_exists: false,
                constraint: Constraint {
                    name: "c".into(),
                    kind: "CHECK".into(),
                    expression: "a > 0".into()
                },
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t ADD PROJECTION p (SELECT a ORDER BY b)"),
            vec![AlterOp::AddProjection {
                if_not_exists: false,
                projection: Projection {
                    name: "p".into(),
                    query: "SELECT a ORDER BY b".into()
                },
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t FREEZE"),
            vec![AlterOp::Other {
                form: "FREEZE".into()
            }]
        );
        assert_eq!(
            ops("ALTER TABLE t ATTACH PARTITION 1 FROM u"),
            vec![AlterOp::Other {
                form: "ATTACH PARTITION".into()
            }]
        );
        assert_eq!(
            parse_alter("ALTER TABLE t")
                .expect_err("no operation")
                .code(),
            62
        );
        assert_eq!(
            parse_alter("ALTER TABLE t DELETE x = 1")
                .expect_err("no WHERE")
                .code(),
            62
        );
        assert_eq!(
            parse_alter("ALTER USER u").expect_err("not a table").code(),
            48
        );
        assert_eq!(
            parse_alter("ALTER TABLE t MODIFY TTL d TO DISK 'x'")
                .expect_err("a move")
                .code(),
            48
        );
    }
}
