//! The `Arrow` (IPC file) and `ArrowStream` (IPC stream) formats: the
//! engine's Arrow batches through `arrow-ipc`.
//!
//! As with `Parquet` (ruling R36.2), the schema is the engine's Arrow layout
//! with each field's ClickHouse type in `loams.ch.type`; ClickHouse's own
//! Arrow output maps some types differently (`Date` as `UInt16`, strings as
//! `Binary`), so the oracle compares these by value. `ArrowStream` writes
//! each batch as it arrives; `Arrow` adds its footer at the end.

use std::io::Write;

use arrow_array::RecordBatch;
use arrow_ipc::writer::{FileWriter, StreamWriter};
use arrow_schema::SchemaRef;
use loams_house_types::ChError;

use crate::Column;
use crate::parquet::{schema_of, with_schema};
use crate::writer::{FormatWriter, Stats, check_batch};

enum Ipc<W: Write> {
    File(FileWriter<W>),
    Stream(StreamWriter<W>),
}

fn ipc_error(e: arrow_schema::ArrowError) -> ChError {
    ChError::network_error(format!("Cannot write Arrow: {e}"))
}

/// The `Arrow`/`ArrowStream` writer.
pub(crate) struct ArrowWriter<W: Write> {
    columns: Vec<Column>,
    file: bool,
    out: Option<W>,
    writer: Option<Ipc<W>>,
}

impl<W: Write> std::fmt::Debug for ArrowWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrowWriter")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl<W: Write> ArrowWriter<W> {
    pub(crate) fn new(columns: Vec<Column>, out: W, file: bool) -> Result<Self, ChError> {
        Ok(Self {
            columns,
            file,
            out: Some(out),
            writer: None,
        })
    }

    fn open(&mut self, schema: &SchemaRef) -> Result<&mut Ipc<W>, ChError> {
        if self.writer.is_none() {
            let out = self
                .out
                .take()
                .ok_or_else(|| ChError::logical_error("the Arrow output is gone"))?;
            self.writer = Some(if self.file {
                Ipc::File(FileWriter::try_new(out, schema).map_err(ipc_error)?)
            } else {
                Ipc::Stream(StreamWriter::try_new(out, schema).map_err(ipc_error)?)
            });
        }
        self.writer
            .as_mut()
            .ok_or_else(|| ChError::logical_error("the Arrow writer is gone"))
    }
}

impl<W: Write + Send> FormatWriter for ArrowWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, batch)?;
        let declared = schema_of(&self.columns)?;
        let batch = with_schema(&declared, batch)?;
        match self.open(&batch.schema())? {
            Ipc::File(w) => w.write(&batch).map_err(ipc_error),
            Ipc::Stream(w) => w.write(&batch).map_err(ipc_error),
        }
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
            self.open(&schema)?;
        }
        match self.writer.take() {
            Some(Ipc::File(mut w)) => w.finish().map_err(ipc_error),
            Some(Ipc::Stream(mut w)) => w.finish().map_err(ipc_error),
            None => Ok(()),
        }
    }
}
