//! ClickHouse output formats for the Loams House Rust engine (plan HE1 Task
//! 36), and input formats from HE2 (Task 61).
//!
//! Task 23 brings the first one, [`json_compact`]'s
//! `JSONCompactEachRowWithNamesAndTypes`, which the oracle reads every answer
//! in (HE0 R6.2). Task 36 adds the rest behind one writer interface.

pub mod json_compact;

use loams_house_types::{ChError, ChType};

/// An output column: its name and ClickHouse type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The column's name, as ClickHouse names it.
    pub name: String,
    /// Its ClickHouse type.
    pub ty: ChType,
}

/// The output columns of an Arrow schema: each field's `loams.ch.type`, or a
/// type inferred from its Arrow type.
///
/// # Errors
///
/// A field whose type the engine cannot name.
pub fn columns(schema: &arrow_schema::Schema) -> Result<Vec<Column>, ChError> {
    schema
        .fields()
        .iter()
        .map(|f| {
            Ok(Column {
                name: f.name().clone(),
                ty: loams_house_types::arrow::from_arrow(f)?,
            })
        })
        .collect()
}
