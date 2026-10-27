//! The structured form of a `system` table the front sends (plan HE1 Tasks 38
//! and 39): in Rust mode the front decides what each `system` table shows, as
//! it does for chDB today (`loams-house/src/system.rs`, R9.62), and sends the
//! rows in the statement's pins as `TablePin::Inline` (Task 39). The engine
//! serves them as in-memory tables in place of what it would fill from its own
//! catalog.
//!
//! On the wire an [`InlineTable`] is its name and an Arrow IPC stream (the
//! schema, each field carrying its ClickHouse type in `loams.ch.type`, then
//! the batches): the front writes it without depending on the engine
//! (`engine_crates_stay_apart`), and Arrow's IPC format is the same across the
//! front's and the engine's Arrow versions.

use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::ipc::reader::StreamReader;
use datafusion::arrow::ipc::writer::StreamWriter;
use datafusion::datasource::MemTable;
use loams_house_types::ChError;
use loams_house_types::arrow::from_arrow;

use super::fill;

/// A `system` table's rows, sent by the front.
#[derive(Debug, Clone)]
pub struct InlineTable {
    /// The table's name in `system` (`tables`, `parts`, …).
    pub name: String,
    /// Its columns, each with its ClickHouse type in the field metadata.
    pub schema: SchemaRef,
    /// Its rows.
    pub batches: Vec<RecordBatch>,
}

fn bad(e: impl std::fmt::Display) -> ChError {
    ChError::bad_arguments(format!("An inline system table: {e}"))
}

impl InlineTable {
    /// The table as an Arrow IPC stream.
    ///
    /// # Errors
    ///
    /// An Arrow error writing the stream.
    pub fn to_ipc(&self) -> Result<Vec<u8>, ChError> {
        let mut out = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut out, &self.schema).map_err(bad)?;
            for b in &self.batches {
                writer.write(b).map_err(bad)?;
            }
            writer.finish().map_err(bad)?;
        }
        Ok(out)
    }

    /// Reads `system.<name>` from an Arrow IPC stream, and checks it against
    /// the engine's shape of that table: the same columns, in order, of the
    /// same ClickHouse types.
    ///
    /// # Errors
    ///
    /// An unreadable stream, a table the engine does not serve, or columns
    /// that differ from its shape.
    pub fn from_ipc(name: &str, bytes: &[u8]) -> Result<Self, ChError> {
        let reader = StreamReader::try_new(bytes, None).map_err(bad)?;
        let schema = reader.schema();
        let batches = reader.collect::<Result<Vec<_>, _>>().map_err(bad)?;
        let table = Self {
            name: name.to_string(),
            schema,
            batches,
        };
        table.check()?;
        Ok(table)
    }

    /// Checks the columns against the engine's shape of `system.<name>`.
    ///
    /// # Errors
    ///
    /// A table the engine does not serve, or columns that differ.
    pub fn check(&self) -> Result<(), ChError> {
        let shape = fill::shape(&self.name).ok_or_else(|| {
            bad(format!(
                "system.{} is not a table the engine serves",
                self.name
            ))
        })?;
        let fields = self.schema.fields();
        if fields.len() != shape.len() {
            return Err(bad(format!(
                "system.{} has {} columns, not {}",
                self.name,
                fields.len(),
                shape.len()
            )));
        }
        for (f, (name, ty)) in fields.iter().zip(shape.iter()) {
            let expected = loams_house_types::parse(ty)?;
            let got = from_arrow(f)?;
            if f.name() != name || got != expected {
                return Err(bad(format!(
                    "system.{} column {} {got} where {name} {expected} was expected",
                    self.name,
                    f.name()
                )));
            }
        }
        Ok(())
    }

    /// The table as an in-memory table.
    ///
    /// # Errors
    ///
    /// Batches whose schema is not the table's.
    pub fn provider(&self) -> Result<Arc<MemTable>, ChError> {
        Ok(Arc::new(
            MemTable::try_new(self.schema.clone(), vec![self.batches.clone()]).map_err(bad)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::build::{Cell, Rows, batch};
    use crate::system::shapes;

    #[test]
    fn inline_tables_round_trip_through_ipc() {
        let mut rows = Rows::new(&["macro", "substitution"]);
        rows.push(vec![Cell::from("shard"), Cell::from("01")]);
        let b = batch(shapes::MACROS, &rows).unwrap();
        let t = InlineTable {
            name: "macros".into(),
            schema: b.schema(),
            batches: vec![b],
        };
        let back = InlineTable::from_ipc("macros", &t.to_ipc().unwrap()).unwrap();
        assert_eq!(back.batches, t.batches);
        assert!(InlineTable::from_ipc("tables", &t.to_ipc().unwrap()).is_err());
        assert!(InlineTable::from_ipc("nope", &t.to_ipc().unwrap()).is_err());
    }
}
