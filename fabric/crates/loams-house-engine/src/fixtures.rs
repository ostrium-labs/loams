//! Oracle snapshots: ClickHouse metadata plus Parquet data, without chDB.
use crate::catalog::MemoryCatalog;
use anyhow::{Context as _, bail};
use datafusion::{
    arrow::{array::RecordBatch, compute::cast, datatypes::Schema},
    datasource::MemTable,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path, sync::Arc};

/// The atomically published manifest of one corpus's snapshot.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// Tables, including their merge-family metadata for Task 27.
    pub tables: Vec<Table>,
}
/// One table and its relative Parquet file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    /// ClickHouse database.
    pub database: String,
    /// ClickHouse table name.
    pub name: String,
    /// Engine, without parameters.
    pub engine: String,
    /// Sorting expression.
    pub sorting_key: String,
    /// Partition expression.
    pub partition_key: String,
    /// Ordered columns.
    pub columns: Vec<Column>,
    /// Parquet path relative to the snapshot directory.
    pub parquet: String,
}
/// A column's exact ClickHouse type and default expression.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    /// Column name.
    pub name: String,
    /// Canonical ClickHouse type text.
    pub r#type: String,
    /// DEFAULT, MATERIALIZED, ALIAS, or empty.
    pub default_kind: String,
    /// Default expression, if any.
    pub default_expression: String,
}
impl Snapshot {
    /// Loads a snapshot, refusing paths outside its directory and invalid schemas.
    pub fn load(root: &Path) -> anyhow::Result<(Self, MemoryCatalog)> {
        let snapshot: Self = serde_json::from_reader(File::open(root.join("snapshot.json"))?)?;
        let mut catalog = MemoryCatalog::default();
        let root = root.canonicalize()?;
        let mut names = std::collections::BTreeSet::new();
        for table in &snapshot.tables {
            if !names.insert((&table.database, &table.name)) {
                bail!("duplicate fixture table");
            }
            let path = root.join(&table.parquet).canonicalize()?;
            if !path.starts_with(&root) {
                bail!("fixture path escapes snapshot directory");
            }
            let mut fields = Vec::new();
            for col in &table.columns {
                fields.push(loams_house_types::arrow::to_arrow(
                    &col.name,
                    &loams_house_types::parse(&col.r#type)?,
                )?);
            }
            let schema = Arc::new(Schema::new(fields));
            let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?.build()?;
            let mut batches = Vec::new();
            for batch in reader {
                let batch = batch?;
                if batch.num_columns() != schema.fields().len() {
                    bail!("fixture column count differs");
                }
                let mut arrays = Vec::new();
                for (i, field) in schema.fields().iter().enumerate() {
                    if batch.schema().field(i).name() != field.name() {
                        bail!("fixture column order differs");
                    }
                    arrays.push(restore(batch.column(i), field).with_context(|| {
                        format!(
                            "restoring {}.{}.{} ({})",
                            table.database,
                            table.name,
                            field.name(),
                            field.data_type()
                        )
                    })?);
                }
                batches.push(RecordBatch::try_new(schema.clone(), arrays)?);
            }
            catalog.insert(
                table.database.clone(),
                table.name.clone(),
                MemTable::try_new(schema, vec![batches])?,
            );
        }
        Ok((snapshot, catalog))
    }
}

/// Restore Parquet's physical encodings, including enum labels and nested fields.
fn restore(
    array: &datafusion::arrow::array::ArrayRef,
    field: &datafusion::arrow::datatypes::Field,
) -> anyhow::Result<datafusion::arrow::array::ArrayRef> {
    use datafusion::arrow::{
        array::{Array, Int8Array, Int16Array, StringArray, make_array},
        datatypes::{DataType, Field},
    };
    use loams_house_types::ChType;
    if let DataType::Dictionary(_, values) = field.data_type() {
        let plain = Field::new(field.name(), values.as_ref().clone(), field.is_nullable())
            .with_metadata(field.metadata().clone());
        return Ok(cast(&restore(array, &plain)?, field.data_type())?);
    }
    if let DataType::Dictionary(_, values) = array.data_type() {
        return restore(&cast(array, values)?, field);
    }
    let ty = loams_house_types::arrow::from_arrow(field)?;
    let mut inner = &ty;
    while let ChType::Nullable(t) | ChType::LowCardinality(t) = inner {
        inner = t;
    }
    if matches!(inner, ChType::Enum8(_) | ChType::Enum16(_))
        && matches!(
            array.data_type(),
            DataType::Binary | DataType::LargeBinary | DataType::Utf8 | DataType::LargeUtf8
        )
    {
        let strings = cast(array, &DataType::Utf8)?;
        let strings = strings
            .as_any()
            .downcast_ref::<StringArray>()
            .context("enum labels")?;
        let unknown = |name: &str| anyhow::anyhow!("unknown enum label {name:?}");
        return match inner {
            ChType::Enum8(members) => Ok(Arc::new(Int8Array::from(
                strings
                    .iter()
                    .map(|s| {
                        s.map(|s| {
                            members
                                .iter()
                                .find(|(name, _)| name == s)
                                .map(|(_, v)| *v)
                                .ok_or_else(|| unknown(s))
                        })
                        .transpose()
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ))),
            ChType::Enum16(members) => Ok(Arc::new(Int16Array::from(
                strings
                    .iter()
                    .map(|s| {
                        s.map(|s| {
                            members
                                .iter()
                                .find(|(name, _)| name == s)
                                .map(|(_, v)| *v)
                                .ok_or_else(|| unknown(s))
                        })
                        .transpose()
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ))),
            _ => unreachable!(),
        };
    }
    let children: Vec<_> = match field.data_type() {
        DataType::List(child) | DataType::Map(child, _) => vec![child.clone()],
        DataType::Struct(children) => children.iter().cloned().collect(),
        _ => return Ok(cast(array, field.data_type())?),
    };
    let data = array.to_data();
    if data.child_data().len() != children.len() {
        bail!("nested fixture layout differs");
    }
    let restored = data
        .child_data()
        .iter()
        .zip(children.iter())
        .map(|(child, field)| restore(&make_array(child.clone()), field).map(|a| a.to_data()))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(make_array(
        data.into_builder()
            .data_type(field.data_type().clone())
            .child_data(restored)
            .build()?,
    ))
}
