//! The answers to `DESCRIBE`, `SHOW CREATE` and `EXISTS` (plan HE1 Task 38),
//! from the catalog, as ClickHouse prints them:
//!
//! * `DESCRIBE TABLE t`: one row per column, `name`, `type`, `default_type`,
//!   `default_expression`, `comment`, `codec_expression`, `ttl_expression`,
//!   all `String` (`InterpreterDescribeQuery`), plain views included (R9.13's
//!   gap: a view is described from its columns);
//! * `SHOW CREATE TABLE t`: one row, `statement String`, the stored `CREATE`
//!   statement (the House's `loams.ch.ddl`);
//! * `EXISTS [TABLE|VIEW|DICTIONARY|DATABASE] x`: one row, `result UInt8`.

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use loams_house_sql::meta::{Meta, ObjectKind, ObjectName};
use loams_house_sql::plan::unknown_database;
use loams_house_types::ChError;
use loams_house_types::ty::quote_identifier;

use super::build::{self, Cell, Rows};
use super::fill::{columns_of, shape};
use super::shapes;
use crate::catalog::{ColumnInfo, MemCatalog};

/// `DESCRIBE`'s columns.
pub const DESCRIBE: shapes::Shape = &[
    ("name", "String"),
    ("type", "String"),
    ("default_type", "String"),
    ("default_expression", "String"),
    ("comment", "String"),
    ("codec_expression", "String"),
    ("ttl_expression", "String"),
];

/// `SHOW CREATE`'s column.
pub const SHOW_CREATE: shapes::Shape = &[("statement", "String")];

/// `EXISTS`' column.
pub const EXISTS: shapes::Shape = &[("result", "UInt8")];

fn unknown_table(db: &str, name: &str) -> ChError {
    ChError {
        code: 60,
        name: "UNKNOWN_TABLE",
        message: format!("Table {db}.{name} does not exist"),
    }
}

/// The columns of `db.name`, if it exists: a catalog table's, or a `system`
/// table's.
fn columns(catalog: &MemCatalog, db: &str, name: &str) -> Option<Vec<ColumnInfo>> {
    if db == "system" {
        let shape = shape(name)?;
        return Some(
            shape
                .iter()
                .map(|(n, t)| {
                    let alias = shapes::ALIASES
                        .iter()
                        .find(|(table, c, _)| *table == name && c == n)
                        .map(|(_, _, target)| *target);
                    ColumnInfo {
                        name: (*n).to_string(),
                        ty: (*t).to_string(),
                        default_kind: if alias.is_some() { "ALIAS" } else { "" }.to_string(),
                        default_expression: alias.unwrap_or("").to_string(),
                        ..ColumnInfo::default()
                    }
                })
                .collect(),
        );
    }
    let table = catalog.get(db, name)?;
    let info = catalog.info(db, name);
    let schema = datafusion::catalog::TableProvider::schema(table.data.as_ref());
    Some(columns_of(&schema, &info.columns))
}

/// Whether `db.name` exists and is of `kind`.
fn exists(catalog: &MemCatalog, kind: ObjectKind, object: &ObjectName, current: &str) -> bool {
    if kind == ObjectKind::Database {
        return catalog.has_database(&object.name);
    }
    let db = object.database_or(current);
    let engine = if db == "system" {
        shape(&object.name).map(|_| "System".to_string())
    } else {
        catalog.get(db, &object.name).map(|t| t.engine)
    };
    match (kind, engine) {
        (_, None) => false,
        (ObjectKind::Table, Some(_)) => true,
        (ObjectKind::View, Some(e)) => e == "View",
        (ObjectKind::Dictionary, Some(e)) => e == "Dictionary",
        (ObjectKind::Database, Some(_)) => false,
    }
}

/// A `CREATE TABLE` statement in ClickHouse's formatting, for a table with no
/// stored one.
fn create_text(
    db: &str,
    name: &str,
    engine: &str,
    sorting_key: &str,
    cols: &[ColumnInfo],
) -> String {
    let mut out = format!(
        "CREATE TABLE {}.{}\n(\n",
        quote_identifier(db),
        quote_identifier(name)
    );
    let lines: Vec<String> = cols
        .iter()
        .map(|c| {
            let mut line = format!("    `{}` {}", c.name.replace('`', "\\`"), c.ty);
            if !c.default_kind.is_empty() {
                line.push_str(&format!(" {} {}", c.default_kind, c.default_expression));
            }
            if !c.comment.is_empty() {
                line.push_str(&format!(
                    " COMMENT {}",
                    loams_house_types::ty::quote(&c.comment)
                ));
            }
            if !c.codec.is_empty() {
                line.push_str(&format!(" CODEC({})", c.codec));
            }
            if !c.ttl.is_empty() {
                line.push_str(&format!(" TTL {}", c.ttl));
            }
            line
        })
        .collect();
    out.push_str(&lines.join(",\n"));
    out.push_str(&format!("\n)\nENGINE = {engine}"));
    if !sorting_key.is_empty() {
        out.push_str(&format!("\nORDER BY {sorting_key}"));
    }
    out
}

fn one_batch(shape: shapes::Shape, rows: &Rows) -> Result<(SchemaRef, Vec<RecordBatch>), ChError> {
    let batch = build::batch(shape, rows)?;
    Ok((batch.schema(), vec![batch]))
}

/// The answer to a metadata statement, `current` being the current database.
///
/// # Errors
///
/// `60` for a table that does not exist, `81` for a database.
pub fn answer(
    catalog: &MemCatalog,
    meta: &Meta,
    current: &str,
) -> Result<(SchemaRef, Vec<RecordBatch>), ChError> {
    match meta {
        Meta::Describe { object } => {
            let db = object.database_or(current);
            if db != "system" && !catalog.has_database(db) {
                return Err(unknown_database(db));
            }
            let cols = columns(catalog, db, &object.name)
                .ok_or_else(|| unknown_table(db, &object.name))?;
            let mut rows = Rows::new(&[
                "name",
                "type",
                "default_type",
                "default_expression",
                "comment",
                "codec_expression",
                "ttl_expression",
            ]);
            for c in cols {
                rows.push(vec![
                    c.name.into(),
                    c.ty.into(),
                    c.default_kind.into(),
                    c.default_expression.into(),
                    c.comment.into(),
                    c.codec.into(),
                    c.ttl.into(),
                ]);
            }
            one_batch(DESCRIBE, &rows)
        }
        Meta::ShowCreate { kind, object } => {
            let statement = if *kind == ObjectKind::Database {
                if !catalog.has_database(&object.name) {
                    return Err(unknown_database(&object.name));
                }
                let engine = match object.name.as_str() {
                    "information_schema" | "INFORMATION_SCHEMA" => "Memory",
                    _ => "Atomic",
                };
                format!(
                    "CREATE DATABASE {}\nENGINE = {engine}",
                    quote_identifier(&object.name)
                )
            } else {
                let db = object.database_or(current);
                if !exists(catalog, *kind, object, current) {
                    if db != "system" && !catalog.has_database(db) {
                        return Err(unknown_database(db));
                    }
                    return Err(unknown_table(db, &object.name));
                }
                if db == "system" {
                    let cols = columns(catalog, db, &object.name).unwrap_or_default();
                    let engine = super::fill::SYSTEM_TABLES
                        .iter()
                        .find(|(n, _, _)| *n == object.name)
                        .map_or("System", |(_, e, _)| *e);
                    create_text(db, &object.name, engine, "", &cols)
                } else {
                    let info = catalog.info(db, &object.name);
                    if !info.show_create.is_empty() {
                        info.show_create
                    } else if !info.create_table_query.is_empty() {
                        info.create_table_query
                    } else {
                        let table = catalog
                            .get(db, &object.name)
                            .ok_or_else(|| unknown_table(db, &object.name))?;
                        let cols = columns(catalog, db, &object.name).unwrap_or_default();
                        create_text(db, &object.name, &table.engine, &table.sorting_key, &cols)
                    }
                }
            };
            let mut rows = Rows::new(&["statement"]);
            rows.push(vec![Cell::Str(statement)]);
            one_batch(SHOW_CREATE, &rows)
        }
        Meta::Exists {
            kind,
            temporary,
            object,
        } => {
            // The engine sees no session's temporary tables (ClickHouse's own
            // `EXISTS` does not find them either, 01048).
            let found = !*temporary && exists(catalog, *kind, object, current);
            let mut rows = Rows::new(&["result"]);
            rows.push(vec![Cell::from(found)]);
            one_batch(EXISTS, &rows)
        }
        Meta::ShowTables { .. } | Meta::ShowDatabases { .. } => Err(ChError::logical_error(
            "SHOW TABLES and SHOW DATABASES are planned as queries",
        )),
    }
}
