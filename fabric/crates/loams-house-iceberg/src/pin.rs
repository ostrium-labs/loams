//! [`TablePin`]: one table of a statement, pinned to a metadata file and a
//! snapshot, with what the House's catalog knows about it as ClickHouse sees
//! it (plan HE1 Task 26). The front sends it in hsw1 v2 (Task 39), so it is
//! serialisable; its fields mirror `loams-house`'s `ReadPin` plus what
//! `views.rs` reads from the catalog.

use std::collections::HashMap;

use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::keywords::Keyword;
use datafusion::sql::sqlparser::tokenizer::{Token, Tokenizer};
use iceberg_engine::spec::TableMetadata;
use loams_house_types::{ChError, ChType};
use serde::{Deserialize, Serialize};

/// Property: the engine, as `SHOW CREATE` prints it. A table without it was
/// made by another writer (`external`).
pub const PROP_ENGINE: &str = "loams.ch.engine";
/// Property: every column, as JSON (`ChColumn`s).
pub const PROP_TYPES: &str = "loams.ch.types";
/// Property: `ORDER BY`.
pub const PROP_ORDER_BY: &str = "loams.ch.order_by";
/// Property: `PRIMARY KEY`.
pub const PROP_PRIMARY_KEY: &str = "loams.ch.primary_key";
/// Property: `PARTITION BY`.
pub const PROP_PARTITION_BY: &str = "loams.ch.partition_by";
/// Property: the table `TTL` clause, as written (HE1 R9.12).
pub const PROP_TTL: &str = "loams.ch.ttl";

/// How a statement reads a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReadMode {
    /// The rows as stored, with `ALIAS` columns computed and expired `TTL` rows
    /// hidden.
    #[default]
    Plain,
    /// `FINAL` (Task 27): the stored rows with [`crate::virtual_columns`], for
    /// the merge operator, which computes `ALIAS` and `TTL` after merging
    /// ([`crate::computed`]).
    Final,
    /// The stored rows as written, with each row's place in the table's write
    /// order (`__loams_seq`, `__loams_row`); no `ALIAS` column and no `TTL`
    /// filter, as `views.rs`'s `Mode::Sequenced`.
    Sequenced,
}

impl ReadMode {
    /// Whether the read carries `__loams_seq` and `__loams_row`.
    pub fn sequenced(self) -> bool {
        !matches!(self, Self::Plain)
    }
}

/// One column as ClickHouse describes it: the House's `loams.ch.types` entry.
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
    /// The default's expression, as the catalog keeps it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_expression: String,
}

impl ChColumn {
    /// Whether the column has bytes in the table (`ALIAS` columns are computed).
    pub fn stored(&self) -> bool {
        self.default_kind != "ALIAS"
    }
}

/// What `views.rs` reads from the catalog besides the files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChTableInfo {
    /// The columns, in order, with ClickHouse types, defaults and `ALIAS`es.
    pub columns: Vec<ChColumn>,
    /// The engine and its parameters (`ReplacingMergeTree(ver)`), `Iceberg`
    /// for an external table.
    pub engine: String,
    /// `ORDER BY`.
    #[serde(default)]
    pub order_by: String,
    /// `PRIMARY KEY`.
    #[serde(default)]
    pub primary_key: String,
    /// `PARTITION BY`.
    #[serde(default)]
    pub partition_by: String,
    /// The table `TTL` clause, as written.
    #[serde(default)]
    pub ttl: String,
}

/// One table of a statement, pinned (§49 §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TablePin {
    /// Its database.
    pub db: String,
    /// Its name.
    pub name: String,
    /// Its location (`s3://bucket/…/house/db/t-1`, `file:///…`). Every path
    /// the metadata names under the metadata's own `location` is read under
    /// this one (R26.3), so a table can be moved, as fixtures are.
    pub location: String,
    /// The metadata file read (the table's current one: it holds every
    /// snapshot).
    pub metadata_file: String,
    /// The snapshot; `None` when there is none to read (a new table, or a time
    /// before its first): no rows, with the table's columns.
    pub snapshot_id: Option<i64>,
    /// The table as ClickHouse shows it.
    pub ch: ChTableInfo,
    /// How the statement reads it.
    #[serde(default)]
    pub mode: ReadMode,
    /// Written by another engine: read as Iceberg's own types, no casts.
    #[serde(default)]
    pub external: bool,
    /// Whether Iceberg v3's row lineage columns (`_row_id`,
    /// `_last_updated_sequence_number`) are read as columns (R26.6).
    #[serde(default)]
    pub lineage: bool,
}

impl TablePin {
    /// The pin of `metadata`'s current snapshot, read from `metadata_file`,
    /// with the House's catalog properties (or, for another writer's table,
    /// ClickHouse types from its Iceberg schema).
    ///
    /// # Errors
    ///
    /// `loams.ch.types` that is not JSON.
    pub fn from_metadata(
        db: &str,
        name: &str,
        metadata_file: &str,
        metadata: &TableMetadata,
    ) -> Result<Self, ChError> {
        let properties = metadata.properties();
        let get = |key: &str| properties.get(key).cloned().unwrap_or_default();
        let external = !properties.contains_key(PROP_ENGINE);
        let ch = if external {
            ChTableInfo {
                columns: metadata
                    .current_schema()
                    .as_struct()
                    .fields()
                    .iter()
                    .map(|f| ChColumn {
                        name: f.name.clone(),
                        ch_type: crate::encodings::from_iceberg(&f.field_type, f.required)
                            .to_string(),
                        ..ChColumn::default()
                    })
                    .collect(),
                engine: "Iceberg".to_string(),
                ..ChTableInfo::default()
            }
        } else {
            let columns: Vec<ChColumn> = serde_json::from_str(&get(PROP_TYPES)).map_err(|e| {
                ChError::bad_arguments(format!(
                    "Table {db}.{name}: its {PROP_TYPES} property is not readable: {e}"
                ))
            })?;
            ChTableInfo {
                columns,
                engine: get(PROP_ENGINE),
                order_by: get(PROP_ORDER_BY),
                primary_key: get(PROP_PRIMARY_KEY),
                partition_by: get(PROP_PARTITION_BY),
                ttl: get(PROP_TTL),
            }
        };
        Ok(Self {
            db: db.to_string(),
            name: name.to_string(),
            location: metadata.location().to_string(),
            metadata_file: metadata_file.to_string(),
            snapshot_id: metadata.current_snapshot_id(),
            ch,
            mode: ReadMode::Plain,
            external,
            lineage: false,
        })
    }

    /// [`Self::from_metadata`] of a metadata file's bytes.
    ///
    /// # Errors
    ///
    /// Bytes that are not Iceberg table metadata.
    pub fn from_metadata_json(
        db: &str,
        name: &str,
        metadata_file: &str,
        bytes: &[u8],
    ) -> Result<Self, ChError> {
        let metadata: TableMetadata = serde_json::from_slice(bytes).map_err(|e| {
            ChError::cannot_parse_text(format!("{metadata_file}: not Iceberg table metadata: {e}"))
        })?;
        Self::from_metadata(db, name, metadata_file, &metadata)
    }

    /// Whether the table's engine reads no rows ever (`Null`, HE1 R9.14).
    pub fn null_engine(&self) -> bool {
        !self.external && self.ch.engine.trim() == "Null"
    }

    /// The parsed ClickHouse type of every column, by name.
    ///
    /// # Errors
    ///
    /// A type that does not parse.
    pub fn types(&self) -> Result<HashMap<String, ChType>, ChError> {
        self.ch
            .columns
            .iter()
            .map(|c| Ok((c.name.clone(), loams_house_types::parse(&c.ch_type)?)))
            .collect()
    }
}

/// One delete rule of a table `TTL` (HE1 R9.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtlRule {
    /// When a row expires.
    pub expression: String,
    /// `WHERE …`.
    pub condition: Option<String>,
}

/// The delete rules of a `TTL` clause: `expr [DELETE] [WHERE cond]`,
/// comma-separated. A rule that moves (`TO DISK`, `TO VOLUME`), recompresses
/// or rolls up rows is `48`, as the House's DDL refuses it.
///
/// # Errors
///
/// `62` for text that is not a `TTL`; `48` for a rule that is not a delete.
pub fn ttl_rules(text: &str) -> Result<Vec<TtlRule>, ChError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let tokens = Tokenizer::new(&ClickHouseDialect {}, text)
        .tokenize()
        .map_err(|e| ChError::syntax_error(format!("TTL: {e}")))?;
    let mut rules = Vec::new();
    let mut depth = 0i32;
    let mut current: Vec<Token> = Vec::new();
    let mut parts: Vec<Vec<Token>> = Vec::new();
    for token in tokens {
        match &token {
            Token::LParen | Token::LBracket => depth += 1,
            Token::RParen | Token::RBracket => depth -= 1,
            Token::Comma if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(token);
    }
    parts.push(current);
    for part in parts {
        let mut depth = 0i32;
        let mut expression = String::new();
        let mut condition: Option<String> = None;
        let mut deleted = false;
        for token in part {
            match &token {
                Token::LParen | Token::LBracket => depth += 1,
                Token::RParen | Token::RBracket => depth -= 1,
                _ => {}
            }
            if depth == 0
                && let Token::Word(w) = &token
                && w.quote_style.is_none()
            {
                let upper = w.value.to_ascii_uppercase();
                match w.keyword {
                    _ if matches!(upper.as_str(), "TO" | "RECOMPRESS" | "GROUP") => {
                        return Err(ChError::not_implemented(format!(
                            "TTL … {} is not supported on lake tables: a TTL there deletes rows \
                             (HE1 R9.12)",
                            w.value.to_uppercase()
                        )));
                    }
                    Keyword::DELETE if condition.is_none() => {
                        deleted = true;
                        continue;
                    }
                    Keyword::WHERE if condition.is_none() => {
                        condition = Some(String::new());
                        continue;
                    }
                    _ => {}
                }
            }
            let text = token.to_string();
            match &mut condition {
                Some(c) => c.push_str(&text),
                None if !deleted => expression.push_str(&text),
                None => {
                    if !matches!(token, Token::Whitespace(_)) {
                        return Err(ChError::syntax_error(format!(
                            "TTL: unexpected {text} after DELETE"
                        )));
                    }
                }
            }
        }
        let expression = expression.trim().to_string();
        let condition = condition.map(|c| c.trim().to_string());
        if expression.is_empty() || condition.as_deref() == Some("") {
            return Err(ChError::syntax_error(format!("TTL: cannot read {text:?}")));
        }
        rules.push(TtlRule {
            expression,
            condition,
        });
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_rules_split_at_top_level_commas() {
        let rules = ttl_rules("ts + toIntervalDay(1), toDate(d, 'UTC') DELETE WHERE x IN (1, 2)")
            .expect("rules");
        assert_eq!(
            rules,
            vec![
                TtlRule {
                    expression: "ts + toIntervalDay(1)".to_string(),
                    condition: None
                },
                TtlRule {
                    expression: "toDate(d, 'UTC')".to_string(),
                    condition: Some("x IN (1, 2)".to_string())
                },
            ]
        );
        assert!(ttl_rules("").expect("none").is_empty());
        assert_eq!(
            ttl_rules("d TO DISK 'x'").expect_err("move").code,
            48,
            "a move is not a delete"
        );
        assert_eq!(ttl_rules("d GROUP BY k").expect_err("rollup").code, 48);
    }

    #[test]
    fn a_pin_round_trips_through_json() {
        let pin = TablePin {
            db: "d".into(),
            name: "t".into(),
            location: "s3://b/t".into(),
            metadata_file: "s3://b/t/metadata/1.metadata.json".into(),
            snapshot_id: Some(7),
            ch: ChTableInfo {
                columns: vec![ChColumn {
                    name: "k".into(),
                    ch_type: "UInt64".into(),
                    ..ChColumn::default()
                }],
                engine: "MergeTree".into(),
                order_by: "k".into(),
                ..ChTableInfo::default()
            },
            mode: ReadMode::Sequenced,
            external: false,
            lineage: false,
        };
        let text = serde_json::to_string(&pin).expect("json");
        assert_eq!(serde_json::from_str::<TablePin>(&text).expect("back"), pin);
        // The House's `loams.ch.types` entries read as columns.
        let columns: Vec<ChColumn> = serde_json::from_str(
            r#"[{"name":"a","type":"UInt8","default_kind":"ALIAS","default_expression":"k + 1","comment":"c"}]"#,
        )
        .expect("columns");
        assert!(!columns[0].stored());
    }
}
