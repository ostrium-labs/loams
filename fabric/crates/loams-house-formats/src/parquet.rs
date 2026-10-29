//! The `Parquet` format: the engine's Arrow batches through the `parquet`
//! crate's Arrow writer.
//!
//! The file carries the engine's Arrow layout, each field keeping its
//! ClickHouse type in `loams.ch.type` (ruling R36.2): reading it back gives
//! the same batches, which `parquet_reads_back_identically` checks. It is not
//! byte for byte ClickHouse's own Parquet (another writer, other encodings
//! and statistics), so the oracle compares it by value, never as bytes.
//! Parquet needs the whole file for its footer: row groups are written as
//! batches arrive, the footer at [`FormatWriter::finish`].

use std::io::Write;
use std::sync::Arc;

use ::parquet::arrow::ArrowWriter;
use arrow_array::RecordBatch;
use arrow_schema::{Schema, SchemaRef};
use loams_house_types::ChError;

use crate::Column;
use crate::writer::{FormatWriter, Stats, check_batch};

/// The Arrow schema of `columns` in the engine's layout.
pub(crate) fn schema_of(columns: &[Column]) -> Result<SchemaRef, ChError> {
    let fields = columns
        .iter()
        .map(|c| loams_house_types::arrow::to_arrow(&c.name, &c.ty))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Arc::new(Schema::new(fields)))
}

/// A batch under the writer's schema: the batch's own columns, with each
/// field's ClickHouse type in its metadata.
pub(crate) fn with_schema(schema: &SchemaRef, batch: &RecordBatch) -> Result<RecordBatch, ChError> {
    if batch.schema().fields() == schema.fields() {
        return Ok(batch.clone());
    }
    // Same columns under the declared names and metadata; Arrow checks the
    // data types match.
    // Nullability is the declared field's or the batch's, never the data's,
    // so every batch of a result has the same schema.
    let theirs = batch.schema();
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .zip(theirs.fields())
        .map(|(f, b)| {
            Arc::new(
                f.as_ref()
                    .clone()
                    .with_data_type(b.data_type().clone())
                    .with_nullable(f.is_nullable() || b.is_nullable()),
            )
        })
        .collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), batch.columns().to_vec())
        .map_err(|e| ChError::logical_error(format!("a batch for the output schema: {e}")))
}

fn parquet_error(e: ::parquet::errors::ParquetError) -> ChError {
    ChError::network_error(format!("Cannot write Parquet: {e}"))
}

/// The `Parquet` writer.
pub(crate) struct ParquetWriter<W: Write + Send> {
    columns: Vec<Column>,
    out: Option<W>,
    writer: Option<ArrowWriter<W>>,
}

impl<W: Write + Send> std::fmt::Debug for ParquetWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParquetWriter")
            .field("columns", &self.columns.len())
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> ParquetWriter<W> {
    pub(crate) fn new(columns: Vec<Column>, out: W) -> Result<Self, ChError> {
        Ok(Self {
            columns,
            out: Some(out),
            writer: None,
        })
    }

    /// The writer, opened on the first batch's schema (the engine's arrays
    /// decide the Arrow types; the columns decide names and metadata).
    fn open(&mut self, schema: SchemaRef) -> Result<&mut ArrowWriter<W>, ChError> {
        if self.writer.is_none() {
            let out = self
                .out
                .take()
                .ok_or_else(|| ChError::logical_error("the Parquet output is gone"))?;
            self.writer = Some(ArrowWriter::try_new(out, schema, None).map_err(parquet_error)?);
        }
        self.writer
            .as_mut()
            .ok_or_else(|| ChError::logical_error("the Parquet writer is gone"))
    }
}

impl<W: Write + Send> FormatWriter for ParquetWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, batch)?;
        let declared = schema_of(&self.columns)?;
        let batch = with_schema(&declared, batch)?;
        let writer = self.open(batch.schema())?;
        writer.write(&batch).map_err(parquet_error)
    }

    fn write_totals(&mut self, _totals: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn write_extremes(&mut self, _extremes: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn finish(mut self: Box<Self>, _stats: &Stats) -> Result<(), ChError> {
        if self.writer.is_none() {
            let schema = schema_of(&self.columns)?;
            self.open(schema)?;
        }
        if let Some(writer) = self.writer.take() {
            let mut out = writer.into_inner().map_err(parquet_error)?;
            out.flush().map_err(crate::writer::io_error)?;
        }
        Ok(())
    }
}
