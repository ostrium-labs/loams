//! Columns a read adds to a table's own (plan HE1 Task 26).
//!
//! * `__loams_seq` and `__loams_row`: a row's place in the table's write order
//!   — its data file's sequence number and its position in that file — exactly
//!   what `views.rs` gives chDB (`transform(_file, …)` and `_row_number`). They
//!   back [`crate::ReadMode::Sequenced`] and `FINAL`'s tie-breaking (Task 27).
//! * `_row_id` and `_last_updated_sequence_number`: Iceberg v3's row lineage,
//!   read when the pin asks (R26.6). A row whose file stores no value inherits
//!   it: the file's first row id plus the row's position, and the file's data
//!   sequence number. They are plain `Int64`s: iceberg-rust's reader gives the
//!   sequence number run-end encoded (R11.5), this one never does.

use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::parquet::arrow::RowNumber;
use loams_house_types::arrow::CH_TYPE;

/// A row's data file's sequence number.
pub const SEQ: &str = "__loams_seq";
/// A row's position in its data file, from 0.
pub const ROW: &str = "__loams_row";
/// Iceberg v3's row id.
pub const ROW_ID: &str = "_row_id";
/// Iceberg v3's last updated sequence number.
pub const LAST_UPDATED: &str = "_last_updated_sequence_number";
/// The reserved field id of `_row_id` (the Iceberg spec).
pub const ROW_ID_FIELD: i32 = 2_147_483_540;
/// The reserved field id of `_last_updated_sequence_number`.
pub const LAST_UPDATED_FIELD: i32 = 2_147_483_539;

/// The partition-like column that carries a file's first row id into a scan.
pub(crate) const FIRST_ROW_ID: &str = "__loams_first_row_id";

/// An `Int64` column the engine adds, typed `Int64` for ClickHouse too.
pub fn field(name: &str) -> Field {
    Field::new(name, DataType::Int64, false).with_metadata(
        [(CH_TYPE.to_string(), "Int64".to_string())]
            .into_iter()
            .collect(),
    )
}

/// Parquet's row number, as DataFusion's Parquet source adds it.
pub(crate) fn row_number() -> FieldRef {
    Arc::new(Field::new(ROW, DataType::Int64, false).with_extension_type(RowNumber))
}

/// Whether `name` is a column a read adds.
pub fn is_virtual(name: &str) -> bool {
    [SEQ, ROW, ROW_ID, LAST_UPDATED].contains(&name)
}
