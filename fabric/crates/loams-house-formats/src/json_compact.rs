//! `JSONCompactEachRowWithNamesAndTypes`, the format the oracle reads every
//! answer in (HE0 R6.2): a JSON array of the column names, one of their
//! ClickHouse types, then one JSON array per row. Since Task 36 it is one of
//! [`crate::json`]'s writers; this is the shortcut Task 23's callers use.

use arrow_array::RecordBatch;
use loams_house_types::ChError;
use loams_house_types::text::TextSettings;

use crate::Column;
use crate::settings::FormatSettings;
use crate::writer::{Format, Stats, writer_for};

/// Writes `batches` (whose columns are `columns`) in
/// `JSONCompactEachRowWithNamesAndTypes`.
///
/// # Errors
///
/// A column whose Arrow layout does not match its ClickHouse type.
pub fn write(
    columns: &[Column],
    batches: &[RecordBatch],
    settings: &TextSettings,
    out: &mut Vec<u8>,
) -> Result<(), ChError> {
    let settings = FormatSettings {
        text: settings.clone(),
        ..FormatSettings::default()
    };
    let format = Format::JsonCompactEachRow {
        strings: false,
        names: true,
        types: true,
    };
    let mut w = writer_for(format, columns, &settings, out)?;
    for batch in batches {
        w.write_batch(batch)?;
    }
    w.finish(&Stats::default())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Int64Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};

    use super::*;

    #[test]
    fn names_types_then_rows() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("plus(a, 1)", DataType::Int64, false),
            Field::new("s", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec![Some("a/b"), None])),
            ],
        )
        .expect("batch");
        let cols = crate::columns(&schema).expect("columns");
        let mut out = Vec::new();
        write(&cols, &[batch], &TextSettings::default(), &mut out).expect("write");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "[\"plus(a, 1)\",\"s\"]\n[\"Int64\",\"Nullable(String)\"]\n[1,\"a\\/b\"]\n[2,null]\n"
        );
    }
}
