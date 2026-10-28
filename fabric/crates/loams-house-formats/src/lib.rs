//! ClickHouse output formats for the Loams House Rust engine (plan HE1 Task
//! 36), and input formats from HE2 (Task 61).
//!
//! [`writer`] picks a [`FormatWriter`] for a format name; each writer
//! streams a result's batches as they come and ends it with
//! [`FormatWriter::finish`]. The formats are the `JSON*` family ([`json`]),
//! TSV, CSV and `Values` ([`text`]), `RowBinary*` ([`binary`]), `Native`
//! ([`native`], through `loams-house-native`'s block framing), `Parquet`
//! ([`parquet`]), `Arrow`/`ArrowStream` ([`arrow`]) and `Pretty*`
//! ([`pretty`]). [`FormatSettings`] holds the settings that change them.
//!
//! Value text is `loams_house_types::text`, already checked against the
//! oracle; the layouts follow ClickHouse's output formats
//! (`src/Processors/Formats/Impl/`, Apache-2.0) and are checked byte for byte
//! by the oracle corpus `he1/36` (`-- oracle: format = <F>`).

pub mod arrow;
pub mod binary;
pub mod json;
pub mod json_compact;
pub mod native;
pub mod parquet;
pub mod pretty;
pub mod settings;
pub mod text;
pub mod writer;

pub use settings::FormatSettings;
pub use writer::{Format, FormatWriter, Stats, write_all, writer, writer_for};

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
