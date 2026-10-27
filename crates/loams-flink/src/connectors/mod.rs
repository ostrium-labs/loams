//! Connector table templates (D1983, D1984) and the connector-jar bundle
//! (D1986).
//!
//! A template is a typed spec ([`TableSpec`]) that renders to a
//! [`TableDdl`]: `CREATE TEMPORARY TABLE IF NOT EXISTS … WITH (…)`. Tables
//! are temporary (session-scoped, in `default_catalog.default_database`), so
//! credentials in their options live only as long as the session.

pub mod jars;
pub mod kafka;
pub mod postgres_cdc;

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::catalog::quote::{RenderCtx, SqlError, WithOptions, quote_ident, quote_literal};

pub use kafka::{
    Format, KafkaAuth, KafkaGateway, KafkaSecurity, KafkaTable, StartupMode, UpsertKafkaTable,
};
pub use postgres_cdc::{CdcStartup, PostgresCdcTable, slot_name};

/// A metadata column's source (`METADATA [FROM 'key'] [VIRTUAL]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// The connector's metadata key, when it differs from the column name
    /// (`timestamp`, `partition`, `offset`, `op_ts`, …).
    #[serde(default)]
    pub from: Option<String>,
    /// Read-only: excluded when the table is a sink.
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
}

/// A column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// A Flink type: `STRING`, `BIGINT`, `TIMESTAMP_LTZ(3)`,
    /// `DECIMAL(10, 2)`, `ARRAY<STRING>`, `ROW<a INT, b STRING>`. Ignored
    /// for a processing-time column.
    #[serde(rename = "type", default)]
    pub data_type: String,
    #[serde(default)]
    pub not_null: bool,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    /// `name AS PROCTIME()`.
    #[serde(default)]
    pub proctime: bool,
}

impl Column {
    pub fn new(name: &str, data_type: &str) -> Self {
        Self {
            name: name.to_string(),
            data_type: data_type.to_string(),
            not_null: false,
            metadata: None,
            proctime: false,
        }
    }

    pub fn not_null(mut self) -> Self {
        self.not_null = true;
        self
    }

    pub fn metadata(mut self, from: Option<&str>, is_virtual: bool) -> Self {
        self.metadata = Some(Metadata {
            from: from.map(str::to_string),
            is_virtual,
        });
        self
    }

    pub fn proctime(name: &str) -> Self {
        Self {
            proctime: true,
            ..Self::new(name, "")
        }
    }

    fn render(&self) -> Result<String, SqlError> {
        let name = quote_ident(&self.name)?;
        if self.proctime {
            return Ok(format!("{name} AS PROCTIME()"));
        }
        check_type(&self.data_type)?;
        let mut out = format!("{name} {}", self.data_type.trim());
        if self.not_null {
            out.push_str(" NOT NULL");
        }
        if let Some(m) = &self.metadata {
            out.push_str(" METADATA");
            if let Some(from) = &m.from {
                let _ = write!(out, " FROM {}", quote_literal(from, "metadata key")?);
            }
            if m.is_virtual {
                out.push_str(" VIRTUAL");
            }
        }
        Ok(out)
    }
}

/// A data type is a plain Flink type expression: ASCII letters, digits,
/// `_`, spaces, `,`, `(`, `)`, `<`, `>`, balanced, starting with a letter.
/// No quotes, comments or semicolons, so it cannot end the column list.
pub fn check_type(t: &str) -> Result<(), SqlError> {
    let t = t.trim();
    let bad = || SqlError::BadType(t.to_string());
    if !t.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()) {
        return Err(bad());
    }
    let mut paren = 0i32;
    let mut angle = 0i32;
    for b in t.bytes() {
        match b {
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'<' => angle += 1,
            b'>' => angle -= 1,
            b'_' | b' ' | b',' => {}
            b if b.is_ascii_alphanumeric() => {}
            _ => return Err(bad()),
        }
        if paren < 0 || angle < 0 {
            return Err(bad());
        }
    }
    if paren != 0 || angle != 0 {
        return Err(bad());
    }
    Ok(())
}

/// `WATERMARK FOR `c` AS `c` - INTERVAL 'n' SECOND`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watermark {
    pub column: String,
    /// How late an event may be, in milliseconds.
    #[serde(default = "default_delay")]
    pub delay_ms: u64,
}

fn default_delay() -> u64 {
    5_000
}

impl Watermark {
    pub fn new(column: &str, delay_ms: u64) -> Self {
        Self {
            column: column.to_string(),
            delay_ms,
        }
    }

    fn render(&self) -> Result<String, SqlError> {
        let c = quote_ident(&self.column)?;
        let secs = self.delay_ms / 1000;
        let millis = self.delay_ms % 1000;
        let interval = if millis == 0 {
            format!("'{secs}'")
        } else {
            format!("'{secs}.{millis:03}'")
        };
        Ok(format!(
            "WATERMARK FOR {c} AS {c} - INTERVAL {interval} SECOND"
        ))
    }
}

/// `CREATE [TEMPORARY] TABLE IF NOT EXISTS <name> (…) WITH (…)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDdl {
    /// One part (`clicks`) or a qualified name (`cat`, `db`, `t`).
    pub name: Vec<String>,
    pub temporary: bool,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
    pub watermark: Option<Watermark>,
    pub options: WithOptions,
}

impl TableDdl {
    pub fn render(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        if self.columns.is_empty() {
            return Err(SqlError::Invalid(format!(
                "table {:?} has no columns",
                self.name.join(".")
            )));
        }
        let parts: Vec<&str> = self.name.iter().map(String::as_str).collect();
        let name = crate::catalog::quote::quote_name(&parts)?;
        let mut lines = Vec::new();
        for c in &self.columns {
            lines.push(c.render()?);
        }
        if let Some(w) = &self.watermark {
            if !self.columns.iter().any(|c| c.name == w.column) {
                return Err(SqlError::Invalid(format!(
                    "watermark column {:?} is not a column",
                    w.column
                )));
            }
            lines.push(w.render()?);
        }
        if !self.primary_key.is_empty() {
            for k in &self.primary_key {
                if !self.columns.iter().any(|c| &c.name == k) {
                    return Err(SqlError::Invalid(format!(
                        "primary key column {k:?} is not a column"
                    )));
                }
            }
            let keys = self
                .primary_key
                .iter()
                .map(|k| quote_ident(k))
                .collect::<Result<Vec<_>, _>>()?;
            lines.push(format!("PRIMARY KEY ({}) NOT ENFORCED", keys.join(", ")));
        }
        let mut out = String::from("CREATE ");
        if self.temporary {
            out.push_str("TEMPORARY ");
        }
        let _ = writeln!(out, "TABLE IF NOT EXISTS {name} (");
        for (i, line) in lines.iter().enumerate() {
            out.push_str("  ");
            out.push_str(line);
            out.push_str(if i + 1 == lines.len() { "\n" } else { ",\n" });
        }
        out.push_str(") ");
        out.push_str(&self.options.render(ctx)?);
        Ok(out)
    }
}

/// A connector table, as a saved job or a session spec stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "connector", rename_all = "snake_case")]
pub enum TableSpec {
    Kafka(KafkaTable),
    UpsertKafka(UpsertKafkaTable),
    PostgresCdc(PostgresCdcTable),
}

impl TableSpec {
    pub fn ddl(&self) -> Result<TableDdl, SqlError> {
        match self {
            Self::Kafka(t) => t.ddl(),
            Self::UpsertKafka(t) => t.ddl(),
            Self::PostgresCdc(t) => t.ddl(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Kafka(t) => &t.name,
            Self::UpsertKafka(t) => &t.name,
            Self::PostgresCdc(t) => &t.name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types() {
        for ok in [
            "STRING",
            "TIMESTAMP_LTZ(3)",
            "DECIMAL(10, 2)",
            "ARRAY<STRING>",
            "MAP<STRING, ARRAY<INT>>",
            "ROW<a INT, b STRING>",
            "BIGINT",
        ] {
            assert!(check_type(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "STRING) WITH ('connector'='filesystem'",
            "INT;",
            "INT -- x",
            "ARRAY<STRING",
            "ROW<`a` INT>",
            "1INT",
            "INT)(",
        ] {
            assert!(check_type(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn watermark_intervals() {
        assert_eq!(
            Watermark::new("ts", 5_000).render().as_deref(),
            Ok("WATERMARK FOR `ts` AS `ts` - INTERVAL '5' SECOND")
        );
        assert_eq!(
            Watermark::new("ts", 250).render().as_deref(),
            Ok("WATERMARK FOR `ts` AS `ts` - INTERVAL '0.250' SECOND")
        );
    }

    #[test]
    fn table_refuses_unknown_key_and_watermark_columns() {
        let mut t = TableDdl {
            name: vec!["t".into()],
            temporary: true,
            columns: vec![Column::new("a", "INT")],
            primary_key: vec!["b".into()],
            watermark: None,
            options: WithOptions::new(),
        };
        assert!(t.render(&RenderCtx::redacted()).is_err());
        t.primary_key.clear();
        t.watermark = Some(Watermark::new("ts", 1));
        assert!(t.render(&RenderCtx::redacted()).is_err());
    }
}
