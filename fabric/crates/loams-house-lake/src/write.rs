//! Data files from rows (§49 §6.1 steps 2–3, HS1 Task 12).
//!
//! A worker formats an `INSERT`'s rows as one Arrow IPC stream (`ArrowStream`),
//! each column already cast to the ClickHouse type chDB's Iceberg reader gives
//! the column's Iceberg type, and sorted by the table's `ORDER BY`. A
//! [`DataWriter`] takes that stream's bytes as they arrive and:
//!
//! * decodes record batches as soon as they are whole (arrow-ipc's push decoder,
//!   so no more than one batch and the Parquet being written are held);
//! * conforms each column to the table's Iceberg schema as Arrow sees it —
//!   field ids in the field metadata, `Binary` → `Utf8` (validated: Iceberg
//!   strings are UTF-8), integer and decimal widths, the time zone's spelling —
//!   by position, recursively through lists, maps and structs; a value the
//!   column cannot hold (a `NULL` in a required column, bytes that are not
//!   UTF-8) fails the write;
//! * splits rows by the table's partition spec (iceberg's transforms) and writes
//!   Parquet with iceberg's writers, which roll to a new file at the target size
//!   and give each file its Iceberg statistics (value, null and NaN counts,
//!   lower and upper bounds, column sizes) from the Parquet footer.
//!
//! Files are written into iceberg's in-memory `FileIO` and come back as bytes
//! with their locations under the table's `data/` directory, for the caller to
//! upload before it commits ([`crate::append`]). Nothing here touches a store or
//! a catalog.

use std::sync::Arc;

use arrow_array_58::cast::AsArray;
use arrow_array_58::{
    Array, ArrayRef, ListArray, MapArray, RecordBatch, RecordBatchOptions, StructArray,
};
use arrow_buffer_58::Buffer;
use arrow_cast_58::{CastOptions, cast_with_options};
use arrow_ipc_58::reader::StreamDecoder;
use arrow_schema_58::{ArrowError, DataType, Schema as ArrowSchema, SchemaRef as ArrowSchemaRef};
use bytes::Bytes;
use iceberg::arrow::{RecordBatchPartitionSplitter, schema_to_arrow_schema};
use iceberg::io::FileIO;
use iceberg::spec::{DataFile, DataFileFormat, FormatVersion, SchemaRef, TableMetadata};
use iceberg::writer::base_writer::data_file_writer::{DataFileWriter, DataFileWriterBuilder};
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use iceberg::writer::partitioning::PartitioningWriter;
use iceberg::writer::partitioning::fanout_writer::FanoutWriter;
use iceberg::writer::{IcebergWriter, IcebergWriterBuilder};
use parquet_58::basic::{Compression, ZstdLevel};
use parquet_58::file::properties::WriterProperties;

use crate::metadata::LakeError;

type Builder =
    DataFileWriterBuilder<ParquetWriterBuilder, DefaultLocationGenerator, DefaultFileNameGenerator>;

/// The files one write produced.
#[derive(Debug, Clone, Default)]
pub struct Written {
    /// Each file's absolute location and bytes, to upload before the commit.
    pub files: Vec<(String, Bytes)>,
    /// The same files as Iceberg data files, with their statistics.
    pub data_files: Vec<DataFile>,
    /// Rows written.
    pub rows: u64,
    /// The Arrow bytes decoded (the rows' size as the worker sent them).
    pub input_bytes: u64,
}

impl Written {
    /// The files' total size.
    pub fn file_bytes(&self) -> u64 {
        self.files.iter().map(|(_, bytes)| bytes.len() as u64).sum()
    }
}

enum Sink {
    Unpartitioned(
        Box<
            DataFileWriter<
                ParquetWriterBuilder,
                DefaultLocationGenerator,
                DefaultFileNameGenerator,
            >,
        >,
    ),
    Partitioned {
        splitter: Box<RecordBatchPartitionSplitter>,
        writer: Box<FanoutWriter<Builder>>,
    },
}

/// Writes one statement's rows as data files of a table (see the module docs).
pub struct DataWriter {
    target: ArrowSchemaRef,
    decoder: StreamDecoder,
    sink: Sink,
    io: FileIO,
    rows: u64,
    input_bytes: u64,
    max_input_bytes: u64,
}

impl std::fmt::Debug for DataWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataWriter")
            .field("rows", &self.rows)
            .field("input_bytes", &self.input_bytes)
            .finish_non_exhaustive()
    }
}

fn invalid(what: &str, err: impl std::fmt::Display) -> LakeError {
    LakeError::Invalid(format!("{what}: {err}"))
}

fn data(err: impl std::fmt::Display) -> LakeError {
    LakeError::Data(err.to_string())
}

/// Rows per Parquet row group. A row group is the unit a reader skips with
/// min/max statistics, and the House's rows arrive sorted by the table's
/// sorting key, so bounded row groups let a key lookup read one group instead
/// of the whole file. parquet-rs's default (1 048 576 rows) put a whole
/// insert in one group, and a point lookup on a 1M-row table fetched and
/// decompressed every column chunk, about 19 MB, for one row (BQ1-house,
/// 2026-10-28). ClickHouse's MergeTree reads in granules of 8 192 rows; 65 536
/// keeps the footer small while a lookup reads 1/16 of a 1M-row file.
pub const ROW_GROUP_ROWS: usize = 65_536;

/// Rows per data page, so the page index (written by default) narrows a
/// lookup inside a row group to about one MergeTree granule.
pub const PAGE_ROWS: usize = 8_192;

/// How the House writes Parquet: ZSTD, bounded row groups and pages.
pub fn writer_properties() -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
        .set_data_page_row_count_limit(PAGE_ROWS)
        .build()
}

impl DataWriter {
    /// A writer of the table's current schema and default partition spec, whose
    /// files roll at `target_file_bytes` and whose input is refused past
    /// `max_input_bytes`. `prefix` names this write's files
    /// (`<prefix>-<n>.parquet`); make it unique (a UUID).
    pub async fn new(
        metadata: &TableMetadata,
        prefix: &str,
        target_file_bytes: u64,
        max_input_bytes: u64,
    ) -> Result<Self, LakeError> {
        if metadata.format_version() != FormatVersion::V2 {
            return Err(LakeError::Invalid(format!(
                "writes to a format {} table (a column with nanoseconds) are not supported \
                 yet: Iceberg v3 needs row lineage",
                metadata.format_version()
            )));
        }
        let schema: SchemaRef = metadata.current_schema().clone();
        let target = Arc::new(
            schema_to_arrow_schema(&schema).map_err(|e| invalid("the table's schema", e))?,
        );
        let io = FileIO::new_with_memory();
        let properties = writer_properties();
        let parquet = ParquetWriterBuilder::new(properties, schema.clone());
        let locations = DefaultLocationGenerator::new(metadata)
            .map_err(|e| invalid("the table's data location", e))?;
        let names =
            DefaultFileNameGenerator::new(prefix.to_string(), None, DataFileFormat::Parquet);
        let rolling = RollingFileWriterBuilder::new(
            parquet,
            usize::try_from(target_file_bytes.max(1)).unwrap_or(usize::MAX),
            io.clone(),
            locations,
            names,
        );
        let builder = DataFileWriterBuilder::new(rolling);
        let spec = metadata.default_partition_spec().clone();
        let sink = if spec.is_unpartitioned() {
            Sink::Unpartitioned(Box::new(
                builder
                    .build(None)
                    .await
                    .map_err(|e| invalid("the data file writer", e))?,
            ))
        } else {
            Sink::Partitioned {
                splitter: Box::new(
                    RecordBatchPartitionSplitter::try_new_with_computed_values(
                        schema.clone(),
                        spec,
                    )
                    .map_err(|e| invalid("the table's partition spec", e))?,
                ),
                writer: Box::new(FanoutWriter::new(builder)),
            }
        };
        Ok(Self {
            target,
            decoder: StreamDecoder::new(),
            sink,
            io,
            rows: 0,
            input_bytes: 0,
            max_input_bytes,
        })
    }

    /// Takes the next piece of the Arrow IPC stream, writing every batch it
    /// completes.
    pub async fn push(&mut self, bytes: &[u8]) -> Result<(), LakeError> {
        self.input_bytes += bytes.len() as u64;
        if self.input_bytes > self.max_input_bytes {
            return Err(LakeError::TooLarge(self.max_input_bytes));
        }
        let mut buffer = Buffer::from(bytes.to_vec());
        while !buffer.is_empty() {
            let before = buffer.len();
            match self.decoder.decode(&mut buffer).map_err(data)? {
                Some(batch) => self.write(batch).await?,
                // Nothing taken: the decoder waits for more than this piece.
                None if buffer.len() == before => break,
                None => {}
            }
        }
        Ok(())
    }

    async fn write(&mut self, batch: RecordBatch) -> Result<(), LakeError> {
        if batch.num_rows() == 0 {
            return Ok(());
        }
        let batch = conform(&batch, &self.target).map_err(data)?;
        self.rows += batch.num_rows() as u64;
        match &mut self.sink {
            Sink::Unpartitioned(writer) => writer.write(batch).await.map_err(data),
            Sink::Partitioned { splitter, writer } => {
                for (key, part) in splitter.split(&batch).map_err(data)? {
                    writer.write(key, part).await.map_err(data)?;
                }
                Ok(())
            }
        }
    }

    /// Ends the stream (it must be whole) and answers the files.
    pub async fn finish(mut self) -> Result<Written, LakeError> {
        self.decoder.finish().map_err(data)?;
        let data_files = match self.sink {
            Sink::Unpartitioned(mut writer) => writer.close().await.map_err(data)?,
            Sink::Partitioned { writer, .. } => writer.close().await.map_err(data)?,
        };
        let mut files = Vec::with_capacity(data_files.len());
        for file in &data_files {
            let location = file.file_path().to_string();
            let bytes = self
                .io
                .new_input(&location)
                .map_err(|e| invalid("a data file", e))?
                .read()
                .await
                .map_err(|e| invalid("a data file", e))?;
            files.push((location, bytes));
        }
        Ok(Written {
            files,
            data_files,
            rows: self.rows,
            input_bytes: self.input_bytes,
        })
    }
}

/// `batch` with the table's Arrow schema: each column, by position, conformed
/// to its field's type.
pub fn conform(batch: &RecordBatch, target: &ArrowSchemaRef) -> Result<RecordBatch, ArrowError> {
    if batch.num_columns() != target.fields().len() {
        return Err(ArrowError::SchemaError(format!(
            "the rows have {} columns and the table {}",
            batch.num_columns(),
            target.fields().len()
        )));
    }
    let mut columns = Vec::with_capacity(batch.num_columns());
    for (column, field) in batch.columns().iter().zip(target.fields()) {
        let conformed = conform_array(column, field.data_type())?;
        if !field.is_nullable() && conformed.null_count() > 0 {
            return Err(ArrowError::InvalidArgumentError(format!(
                "column {} is not Nullable, and the rows hold NULL in it",
                field.name()
            )));
        }
        columns.push(conformed);
    }
    RecordBatch::try_new_with_options(
        target.clone(),
        columns,
        &RecordBatchOptions::new().with_row_count(Some(batch.num_rows())),
    )
}

/// One array as `to`: structs by position, lists and maps through their
/// values, everything else with arrow's cast, which fails on a value it cannot
/// convert (`safe = false`) rather than making it `NULL`.
fn conform_array(array: &ArrayRef, to: &DataType) -> Result<ArrayRef, ArrowError> {
    if array.data_type() == to {
        return Ok(array.clone());
    }
    match (array.data_type(), to) {
        (DataType::Struct(_), DataType::Struct(fields)) => {
            let source = array.as_struct();
            if source.num_columns() != fields.len() {
                return Err(ArrowError::SchemaError(format!(
                    "a tuple of {} elements cannot be a struct of {}",
                    source.num_columns(),
                    fields.len()
                )));
            }
            let columns = source
                .columns()
                .iter()
                .zip(fields.iter())
                .map(|(column, field)| conform_array(column, field.data_type()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(StructArray::try_new_with_length(
                fields.clone(),
                columns,
                source.nulls().cloned(),
                source.len(),
            )?))
        }
        (DataType::LargeList(_), DataType::List(_)) => {
            let narrowed = cast_with_options(array, &narrow_list(array.data_type()), &strict())?;
            conform_array(&narrowed, to)
        }
        (DataType::List(_), DataType::List(field)) => {
            let source = array.as_list::<i32>();
            let values = conform_array(source.values(), field.data_type())?;
            Ok(Arc::new(ListArray::try_new(
                field.clone(),
                source.offsets().clone(),
                values,
                source.nulls().cloned(),
            )?))
        }
        (DataType::Map(_, _), DataType::Map(field, sorted)) => {
            let source = array.as_map();
            let entries: ArrayRef = Arc::new(source.entries().clone());
            let entries = conform_array(&entries, field.data_type())?;
            Ok(Arc::new(MapArray::try_new(
                field.clone(),
                source.offsets().clone(),
                entries.as_struct().clone(),
                source.nulls().cloned(),
                *sorted,
            )?))
        }
        _ => cast_with_options(array, to, &strict()),
    }
}

/// A `LargeList`'s type as a `List` of the same item.
fn narrow_list(ty: &DataType) -> DataType {
    match ty {
        DataType::LargeList(field) => DataType::List(field.clone()),
        other => other.clone(),
    }
}

fn strict() -> CastOptions<'static> {
    CastOptions {
        safe: false,
        ..CastOptions::default()
    }
}

/// The table's current schema as Arrow sees it (field ids in the metadata).
pub fn arrow_schema(metadata: &TableMetadata) -> Result<ArrowSchema, LakeError> {
    schema_to_arrow_schema(metadata.current_schema()).map_err(|e| invalid("the table's schema", e))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use arrow_array_58::{BinaryArray, Int32Array, Int64Array, StringArray};
    use arrow_ipc_58::writer::StreamWriter;
    use arrow_schema_58::Field;
    use iceberg::TableCreation;
    use iceberg::spec::{
        NestedField, PrimitiveType, Schema, Transform, Type, UnboundPartitionSpec,
    };

    use super::*;
    use crate::metadata;

    fn table(partitioned: bool) -> TableMetadata {
        let schema = Schema::builder()
            .with_fields(vec![
                NestedField::required(1, "k", Type::Primitive(PrimitiveType::Long)).into(),
                NestedField::optional(2, "s", Type::Primitive(PrimitiveType::String)).into(),
                NestedField::required(3, "p", Type::Primitive(PrimitiveType::Int)).into(),
            ])
            .build()
            .expect("schema");
        let mut spec = UnboundPartitionSpec::builder();
        if partitioned {
            spec = spec
                .add_partition_field(3, "p", Transform::Identity)
                .expect("field");
        }
        let creation = TableCreation::builder()
            .name("t".to_string())
            .location("memory://bucket/ns/1/house/default/t".to_string())
            .schema(schema)
            .partition_spec(spec.build())
            .properties(HashMap::new())
            .build();
        metadata::create(creation).expect("metadata")
    }

    /// What a worker sends: chDB's column types (`String` as `Binary`, `Int32`
    /// for a narrower `long`).
    fn stream(rows: &[(i32, Option<&[u8]>, i32)]) -> Vec<u8> {
        let schema = Arc::new(ArrowSchema::new(vec![
            Field::new("k", DataType::Int32, false),
            Field::new("s", DataType::Binary, true),
            Field::new("p", DataType::Int32, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.0))),
                Arc::new(BinaryArray::from_iter(rows.iter().map(|r| r.1))),
                Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.2))),
            ],
        )
        .expect("batch");
        let mut out = Vec::new();
        let mut writer = StreamWriter::try_new(&mut out, &schema).expect("writer");
        writer.write(&batch).expect("write");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        drop(writer);
        out
    }

    async fn write(
        metadata: &TableMetadata,
        bytes: &[u8],
        piece: usize,
    ) -> Result<Written, LakeError> {
        let mut writer = DataWriter::new(metadata, "test", 1 << 20, 1 << 30).await?;
        for chunk in bytes.chunks(piece) {
            writer.push(chunk).await?;
        }
        writer.finish().await
    }

    #[tokio::test]
    async fn rows_become_files_with_field_ids_and_statistics() {
        let metadata = table(false);
        let bytes = stream(&[(1, Some(b"a"), 7), (3, None, 7)]);
        // Fed in pieces that cut messages anywhere.
        let written = write(&metadata, &bytes, 7).await.expect("written");
        assert_eq!(written.rows, 4);
        assert_eq!(written.data_files.len(), 1);
        let file = &written.data_files[0];
        assert_eq!(file.record_count(), 4);
        assert_eq!(file.null_value_counts().get(&2), Some(&2));
        assert!(
            file.lower_bounds().contains_key(&1),
            "{:?}",
            file.lower_bounds()
        );
        assert!(
            file.file_path()
                .starts_with("memory://bucket/ns/1/house/default/t/data/test-"),
            "{}",
            file.file_path()
        );
        assert_eq!(written.files[0].1.len() as u64, file.file_size_in_bytes());

        // The Parquet carries the Iceberg field ids.
        let reader =
            parquet_58::file::reader::SerializedFileReader::new(written.files[0].1.clone())
                .expect("parquet");
        use parquet_58::file::reader::FileReader;
        let schema = reader.metadata().file_metadata().schema_descr();
        let ids: Vec<i32> = (0..schema.num_columns())
            .map(|i| schema.column(i).self_type().get_basic_info().id())
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn row_groups_are_bounded() {
        let p = writer_properties();
        assert_eq!(p.max_row_group_row_count(), Some(ROW_GROUP_ROWS));
        assert_eq!(p.data_page_row_count_limit(), PAGE_ROWS);
    }

    #[tokio::test]
    async fn partitions_split_files() {
        let metadata = table(true);
        let bytes = stream(&[(1, Some(b"a"), 7), (2, Some(b"b"), 8), (3, Some(b"c"), 7)]);
        let written = write(&metadata, &bytes, 1 << 16).await.expect("written");
        let mut counts: Vec<u64> = written
            .data_files
            .iter()
            .map(|f| f.record_count())
            .collect();
        counts.sort_unstable();
        assert_eq!(counts, vec![2, 4], "one file per partition value");
        assert!(written.files.iter().all(|(l, _)| l.contains("/data/p=")));
    }

    #[tokio::test]
    async fn values_the_columns_cannot_hold_fail() {
        let metadata = table(false);
        let bytes = stream(&[(1, Some(&[0xff, 0xfe]), 7)]);
        let err = write(&metadata, &bytes, 1 << 16)
            .await
            .expect_err("not UTF-8");
        assert!(matches!(err, LakeError::Data(_)), "{err}");

        // A NULL in a required column.
        let schema = Arc::new(ArrowSchema::new(vec![
            Field::new("k", DataType::Int64, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("p", DataType::Int32, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![None, Some(1)])),
                Arc::new(StringArray::from(vec![Some("a"), None])),
                Arc::new(Int32Array::from(vec![1, 2])),
            ],
        )
        .expect("batch");
        let target = Arc::new(arrow_schema(&metadata).expect("schema"));
        let err = conform(&batch, &target).expect_err("NULL in k");
        assert!(err.to_string().contains("column k"), "{err}");
    }

    #[tokio::test]
    async fn the_input_cap_holds_and_a_cut_stream_fails() {
        let metadata = table(false);
        let bytes = stream(&[(1, Some(b"a"), 7)]);
        let mut writer = DataWriter::new(&metadata, "cap", 1 << 20, 10)
            .await
            .expect("writer");
        let err = writer.push(&bytes).await.expect_err("over the cap");
        assert!(matches!(err, LakeError::TooLarge(10)), "{err}");

        let cut = &bytes[..bytes.len() - 20];
        let err = write(&metadata, cut, 1 << 16).await.expect_err("cut");
        assert!(matches!(err, LakeError::Data(_)), "{err}");
    }
}
