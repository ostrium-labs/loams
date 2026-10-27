//! `JSONCompactEachRowWithNamesAndTypes`: a JSON array of the column names,
//! one of their ClickHouse types, then one JSON array per row, each value in
//! the JSON style of `loams_house_types::text`.

use arrow_array::RecordBatch;
use loams_house_types::ChError;
use loams_house_types::text::{Style, TextSettings, write_value};

use crate::Column;

fn json_string(s: &str, settings: &TextSettings, out: &mut Vec<u8>) -> Result<(), ChError> {
    let array = arrow_array::StringArray::from(vec![s]);
    write_value(
        &loams_house_types::ChType::String,
        &array,
        0,
        Style::Json,
        settings,
        out,
    )
}

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
    for header in [true, false] {
        out.push(b'[');
        for (n, c) in columns.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            // Types on one line: ClickHouse writes `Tuple(a UInt8, b String)` here
            // (measured), not `toTypeName`'s multi-line form.
            let text = if header {
                c.name.clone()
            } else {
                c.ty.to_compact_string()
            };
            json_string(&text, settings, out)?;
        }
        out.extend_from_slice(b"]\n");
    }
    for batch in batches {
        if batch.num_columns() != columns.len() {
            return Err(ChError::logical_error(format!(
                "a batch of {} columns for {} output columns",
                batch.num_columns(),
                columns.len()
            )));
        }
        for row in 0..batch.num_rows() {
            out.push(b'[');
            for (n, c) in columns.iter().enumerate() {
                if n > 0 {
                    out.push(b',');
                }
                write_value(
                    &c.ty,
                    batch.column(n).as_ref(),
                    row,
                    Style::Json,
                    settings,
                    out,
                )?;
            }
            out.extend_from_slice(b"]\n");
        }
    }
    Ok(())
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
