//! [`LakeTable`]: a DataFusion `TableProvider` over one pinned snapshot of a
//! lake table (plan HE1 Task 26).
//!
//! * The fork's `iceberg` reads the metadata file, plans the snapshot's files
//!   (pruned by the scan's filters, [`crate::prune`]) and attaches their deletes.
//! * DataFusion's Parquet source reads the data files, with Iceberg field ids
//!   mapped to columns ([`FieldIds`]: a renamed column reads its old files),
//!   the file filter pushed into the reader, the page index and bloom filters
//!   on, deleted rows skipped by a row selection ([`crate::deletes`]), each
//!   file's sequence number as a partition value and Parquet's row number as a
//!   virtual column.
//! * A projection restores each column's ClickHouse type
//!   ([`crate::encodings::restore`]).
//!
//! The table's columns are those of the pinned snapshot's schema that the
//! catalog names, in the catalog's order (as `views.rs` reads a pin): a
//! time-travelled read sees the schema of its time, a current one the
//! table's current schema. `ALIAS` columns, `DEFAULT`s of columns added by
//! `ALTER` and the `TTL` are computed above it ([`crate::computed`]).

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::arrow::array::{Array, ArrayRef, Int64Array};
use datafusion::arrow::compute::kernels::numeric::add;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef, Schema, SchemaRef};
use datafusion::catalog::Session;
use datafusion::common::config::ConfigOptions;
use datafusion::common::tree_node::{Transformed, TransformedResult, TreeNode};
use datafusion::common::{DFSchema, DataFusionError, Result as DfResult, ScalarValue};
use datafusion::datasource::listing::PartitionedFile;
use datafusion::datasource::object_store::ObjectStoreUrl;
use datafusion::datasource::physical_plan::{FileGroup, FileScanConfigBuilder, ParquetSource};
use datafusion::datasource::source::DataSourceExec;
use datafusion::datasource::table_schema::TableSchema;
use datafusion::datasource::{TableProvider, TableType};
use datafusion::logical_expr::{
    ColumnarValue, Expr, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    TableProviderFilterPushDown, Volatility,
};
use datafusion::object_store::{ObjectStore, ObjectStoreExt};
use datafusion::parquet::arrow::PARQUET_FIELD_ID_META_KEY;
use datafusion::physical_expr::ScalarFunctionExpr;
use datafusion::physical_expr::expressions::Column;
use datafusion::physical_expr_adapter::{
    DefaultPhysicalExprAdapterFactory, PhysicalExprAdapter, PhysicalExprAdapterFactory,
};
use datafusion::physical_plan::empty::EmptyExec;
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::physical_plan::{ExecutionPlan, PhysicalExpr};
use futures::TryStreamExt;
use iceberg_engine::arrow::schema_to_arrow_schema;
use iceberg_engine::spec::{DataFileFormat, SchemaRef as IcebergSchemaRef, TableMetadata};
use iceberg_engine::table::Table;
use iceberg_engine::{Runtime, TableIdent};
use loams_house_types::arrow::{CH_TYPE, data_type};
use loams_house_types::{ChError, ChType};

use crate::deletes::Deletes;
use crate::encodings;
use crate::io::{Paths, StoreStorage};
use crate::pin::TablePin;
use crate::prune::{self, Translatable};
use crate::virtual_columns::{
    self, FIRST_ROW_ID, LAST_UPDATED, LAST_UPDATED_FIELD, ROW, ROW_ID, ROW_ID_FIELD, SEQ,
};

fn ch_error(e: impl fmt::Display) -> ChError {
    ChError::cannot_parse_text(format!("Reading a lake table: {e}"))
}

fn df(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// What one table's scans read, for tests and the worker's logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanReport {
    /// Scans planned.
    pub scans: u64,
    /// Data files opened, over all scans.
    pub files: u64,
    /// Rows the deletes removed from those files.
    pub deleted_rows: u64,
}

/// Where a table column's values come from.
#[derive(Debug, Clone)]
enum Source {
    /// A stored column (the index of its field in the file schema).
    Stored { file: usize, ty: ChType },
    /// A column of a type the engine cannot lay out (another writer's
    /// `variant`): refused when a scan names it (R26.7).
    Unsupported { ty: String },
    /// `__loams_seq`.
    Seq,
    /// `__loams_row`.
    Row,
    /// `_row_id` (the stored one's file index, if the files may hold it).
    RowId(Option<usize>),
    /// `_last_updated_sequence_number`.
    LastUpdated(Option<usize>),
}

/// A lake table read through one pin.
pub struct LakeTable {
    pin: TablePin,
    /// The metadata read; `None` when there is nothing to read: no snapshot,
    /// or a `Null` table.
    table: Option<Arc<TableMetadata>>,
    /// The snapshot when it is not the metadata's current one.
    travel: Option<i64>,
    store: Arc<dyn ObjectStore>,
    store_url: ObjectStoreUrl,
    storage: StoreStorage,
    paths: Arc<Paths>,
    schema: SchemaRef,
    sources: Vec<Source>,
    /// Fields of the read schema that are optional in Iceberg, by name.
    optional: Vec<String>,
    file_schema: SchemaRef,
    translatable: HashMap<String, Translatable>,
    report: Mutex<ScanReport>,
}

impl fmt::Debug for LakeTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LakeTable")
            .field("db", &self.pin.db)
            .field("name", &self.pin.name)
            .field("snapshot_id", &self.pin.snapshot_id)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

static STORES: AtomicU64 = AtomicU64::new(0);

impl LakeTable {
    /// Opens `pin` over `store`: reads its metadata file and lays out its
    /// columns.
    ///
    /// # Errors
    ///
    /// An unreadable metadata file, a snapshot it does not have, or a column
    /// type that does not parse.
    pub async fn open(pin: &TablePin, store: Arc<dyn ObjectStore>) -> Result<Self, ChError> {
        let bytes = store
            .get(&crate::io::key(&pin.metadata_file))
            .await
            .map_err(|e| ch_error(format!("{}: {e}", pin.metadata_file)))?
            .bytes()
            .await
            .map_err(|e| ch_error(format!("{}: {e}", pin.metadata_file)))?;
        let metadata: TableMetadata = serde_json::from_slice(&bytes)
            .map_err(|e| ch_error(format!("{}: {e}", pin.metadata_file)))?;
        let paths = Arc::new(Paths {
            written: metadata.location().to_string(),
            read: pin.location.clone(),
        });
        let storage = StoreStorage::new(store.clone(), paths.as_ref().clone());
        let n = STORES.fetch_add(1, Ordering::Relaxed);
        let store_url = ObjectStoreUrl::parse(format!("loams-lake://t{n}/")).map_err(df_ch)?;

        let current = metadata.current_snapshot_id();
        let travel = pin.snapshot_id.filter(|id| Some(*id) != current);
        let schema_of_read: Option<IcebergSchemaRef> = match pin.snapshot_id {
            None => None,
            Some(id) if Some(id) == current => Some(metadata.current_schema().clone()),
            Some(id) => {
                let snapshot = metadata.snapshot_by_id(id).ok_or_else(|| {
                    ChError::bad_arguments(format!(
                        "Table {}.{} has no snapshot {id}",
                        pin.db, pin.name
                    ))
                })?;
                Some(snapshot.schema(&metadata).map_err(ch_error)?)
            }
        };
        let readable = schema_of_read.is_some() && !pin.null_engine();

        let mut fields: Vec<Field> = Vec::new();
        let mut sources = Vec::new();
        let mut optional = Vec::new();
        let mut translatable = HashMap::new();
        let mut file_fields: Vec<Field> = Vec::new();
        let types = pin.types()?;
        match (&schema_of_read, readable) {
            (Some(schema), true) => {
                let arrow = schema_to_arrow_schema(schema).map_err(ch_error)?;
                for column in pin.ch.columns.iter().filter(|c| c.stored()) {
                    let Some(nested) = schema.as_struct().field_by_name(&column.name) else {
                        // Not in this snapshot's schema (a time-travelled read
                        // of a column added since).
                        continue;
                    };
                    let ty = types[&column.name].clone();
                    let Ok(arrow_field) = arrow.field_with_name(&column.name) else {
                        return Err(ch_error(format!("no Arrow field for {}", column.name)));
                    };
                    if !nested.required {
                        optional.push(column.name.clone());
                    }
                    match data_type(&ty) {
                        Ok((dt, nullable)) => {
                            let file = file_fields.len();
                            file_fields.push(arrow_field.clone().with_nullable(true));
                            if let Some(iceberg) = prune::plain_cast(&ty, &nested.field_type) {
                                translatable.insert(
                                    column.name.clone(),
                                    Translatable {
                                        table_type: dt.clone(),
                                        iceberg,
                                    },
                                );
                            }
                            fields.push(ch_field(
                                &column.name,
                                dt,
                                nullable || !nested.required,
                                &ty,
                            ));
                            sources.push(Source::Stored { file, ty });
                        }
                        Err(_) => {
                            fields.push(Field::new(&column.name, DataType::Null, true));
                            sources.push(Source::Unsupported {
                                ty: column.ch_type.clone(),
                            });
                        }
                    }
                }
            }
            _ => {
                // Nothing to read: the table's stored columns, no rows.
                for column in pin.ch.columns.iter().filter(|c| c.stored()) {
                    let ty = types[&column.name].clone();
                    match data_type(&ty) {
                        Ok((dt, nullable)) => {
                            fields.push(ch_field(&column.name, dt, nullable, &ty));
                            sources.push(Source::Unsupported {
                                ty: column.ch_type.clone(),
                            });
                        }
                        Err(_) => {
                            fields.push(Field::new(&column.name, DataType::Null, true));
                            sources.push(Source::Unsupported {
                                ty: column.ch_type.clone(),
                            });
                        }
                    }
                }
            }
        }
        if pin.mode.sequenced() {
            fields.push(virtual_columns::field(SEQ));
            sources.push(Source::Seq);
            fields.push(virtual_columns::field(ROW));
            sources.push(Source::Row);
        }
        if pin.lineage {
            let mut stored = |name: &str, id: i32| {
                let file = file_fields.len();
                file_fields.push(
                    Field::new(name, DataType::Int64, true).with_metadata(
                        [(PARQUET_FIELD_ID_META_KEY.to_string(), id.to_string())]
                            .into_iter()
                            .collect(),
                    ),
                );
                file
            };
            let row_id = stored(ROW_ID, ROW_ID_FIELD);
            let last = stored(LAST_UPDATED, LAST_UPDATED_FIELD);
            fields.push(virtual_columns::field(ROW_ID));
            sources.push(Source::RowId(Some(row_id)));
            fields.push(virtual_columns::field(LAST_UPDATED));
            sources.push(Source::LastUpdated(Some(last)));
        }
        let table = readable.then(|| Arc::new(metadata));
        Ok(Self {
            pin: pin.clone(),
            table,
            travel,
            store,
            store_url,
            storage,
            paths,
            schema: Arc::new(Schema::new(fields)),
            sources,
            optional,
            file_schema: Arc::new(Schema::new(file_fields)),
            translatable,
            report: Mutex::new(ScanReport::default()),
        })
    }

    /// The pin this table reads.
    pub fn pin(&self) -> &TablePin {
        &self.pin
    }

    /// Whether `column` is optional in the Iceberg schema read (a column
    /// `ALTER … ADD COLUMN` made, when its ClickHouse type is not `Nullable`).
    pub fn optional(&self, column: &str) -> bool {
        self.optional.iter().any(|c| c == column)
    }

    /// What this table's scans have read so far.
    pub fn report(&self) -> ScanReport {
        self.report.lock().map(|r| *r).unwrap_or_default()
    }

    /// The snapshot's data files, with their deletes, pruned by `predicate`.
    /// The iceberg table is built per scan, on the scan's own runtime.
    async fn files(
        &self,
        metadata: &Arc<TableMetadata>,
        predicate: Option<iceberg_engine::expr::Predicate>,
    ) -> Result<Vec<iceberg_engine::scan::FileScanTask>, ChError> {
        let table = Table::builder()
            .metadata(metadata.clone())
            .metadata_location(self.pin.metadata_file.clone())
            .identifier(
                TableIdent::from_strs([self.pin.db.as_str(), self.pin.name.as_str()])
                    .map_err(ch_error)?,
            )
            .file_io(self.storage.file_io())
            .runtime(Runtime::try_current().map_err(ch_error)?)
            .readonly(true)
            .build()
            .map_err(ch_error)?;
        let build = |predicate: Option<iceberg_engine::expr::Predicate>| {
            let mut builder = table.scan().select_all().with_case_sensitive(true);
            if let Some(id) = self.travel {
                builder = builder.snapshot_id(id);
            }
            if let Some(p) = predicate {
                builder = builder.with_filter(p);
            }
            builder.build()
        };
        let scan = match build(predicate.clone()) {
            Ok(scan) => scan,
            // A predicate the snapshot's schema cannot bind: plan unpruned.
            Err(_) if predicate.is_some() => build(None).map_err(ch_error)?,
            Err(e) => return Err(ch_error(e)),
        };
        scan.plan_files()
            .await
            .map_err(ch_error)?
            .try_collect()
            .await
            .map_err(ch_error)
    }
}

fn df_ch(e: impl fmt::Display) -> ChError {
    ch_error(e)
}

fn ch_field(name: &str, dt: DataType, nullable: bool, ty: &ChType) -> Field {
    Field::new(name, dt, nullable).with_metadata(
        [(CH_TYPE.to_string(), ty.to_compact_string())]
            .into_iter()
            .collect(),
    )
}

#[async_trait]
impl TableProvider for LakeTable {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> DfResult<Vec<TableProviderFilterPushDown>> {
        // Every filter prunes what it can and is applied again above.
        Ok(vec![TableProviderFilterPushDown::Inexact; filters.len()])
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        let projection: Vec<usize> = match projection {
            Some(p) => p.clone(),
            None => (0..self.schema.fields().len()).collect(),
        };
        let projected = Arc::new(self.schema.project(&projection)?);
        for &i in &projection {
            if let Source::Unsupported { ty } = &self.sources[i]
                && self.table.is_some()
            {
                return Err(df(ChError::unsupported(format!(
                    "Reading the column {} of type {ty} of {}.{}",
                    self.schema.field(i).name(),
                    self.pin.db,
                    self.pin.name
                ))));
            }
        }
        let Some(table) = &self.table else {
            return Ok(Arc::new(EmptyExec::new(projected)));
        };
        let pruning = prune::pruning(filters, &self.translatable, &self.file_schema);
        let tasks = self.files(table, pruning.predicate).await.map_err(df)?;

        // The scan's own columns: the file schema, then the partition-like
        // columns, then Parquet's row number.
        let partition_cols: Vec<FieldRef> = vec![
            Arc::new(Field::new(SEQ, DataType::Int64, true)),
            Arc::new(Field::new(FIRST_ROW_ID, DataType::Int64, true)),
        ];
        let n_file = self.file_schema.fields().len();
        let seq_index = n_file;
        let first_row_index = n_file + 1;
        let row_index = n_file + 2;

        let deletes = Deletes::new(self.store.clone(), self.storage.clone(), self.paths.clone());
        let mut files = Vec::with_capacity(tasks.len());
        let mut deleted_rows = 0u64;
        for task in &tasks {
            if task.data_file_format() != DataFileFormat::Parquet {
                return Err(df(ChError::unsupported(format!(
                    "The {:?} data file {}",
                    task.data_file_format(),
                    task.data_file_path()
                ))));
            }
            let positions = deletes.positions(task).await.map_err(df)?;
            deleted_rows += positions.len() as u64;
            let key = self.paths.key(task.data_file_path());
            let mut file = PartitionedFile::new(key.to_string(), task.file_size_in_bytes())
                .with_partition_values(vec![
                    ScalarValue::Int64(task.data_sequence_number()),
                    ScalarValue::Int64(task.first_row_id()),
                ]);
            if let Some(plan) = deletes
                .access_plan(task.data_file_path(), task.file_size_in_bytes(), &positions)
                .await
                .map_err(df)?
            {
                file = file.with_extension(plan);
            }
            files.push(file);
        }
        if let Ok(mut report) = self.report.lock() {
            report.scans += 1;
            report.files += files.len() as u64;
            report.deleted_rows += deleted_rows;
        }
        if files.is_empty() {
            return Ok(Arc::new(EmptyExec::new(projected)));
        }

        // What the scan must read, and where each output column finds it.
        let mut read: Vec<usize> = Vec::new();
        let at = |i: usize, read: &mut Vec<usize>| match read.iter().position(|&r| r == i) {
            Some(p) => p,
            None => {
                read.push(i);
                read.len() - 1
            }
        };
        enum Out {
            Restore(usize, ChType),
            Pass(usize),
            RowId(usize, usize, usize),
            LastUpdated(usize, usize),
        }
        let mut outs = Vec::with_capacity(projection.len());
        for &i in &projection {
            outs.push(match &self.sources[i] {
                Source::Stored { file, ty } => Out::Restore(at(*file, &mut read), ty.clone()),
                Source::Seq => Out::Pass(at(seq_index, &mut read)),
                Source::Row => Out::Pass(at(row_index, &mut read)),
                Source::RowId(stored) => Out::RowId(
                    at(stored.unwrap_or(row_index), &mut read),
                    at(first_row_index, &mut read),
                    at(row_index, &mut read),
                ),
                Source::LastUpdated(stored) => Out::LastUpdated(
                    at(stored.unwrap_or(seq_index), &mut read),
                    at(seq_index, &mut read),
                ),
                Source::Unsupported { .. } => {
                    return Err(df(ch_error("an unreadable column in a readable table")));
                }
            });
        }
        if read.is_empty() {
            // `count()`: one cheap column still gives the row count.
            read.push(seq_index);
        }

        let table_schema = TableSchema::builder(self.file_schema.clone())
            .with_table_partition_cols(partition_cols)
            .with_virtual_columns(vec![virtual_columns::row_number()])
            .build();
        let mut source = ParquetSource::new(table_schema)
            .with_pushdown_filters(true)
            .with_reorder_filters(true)
            .with_enable_page_index(true)
            .with_bloom_filter_on_read(true);
        if let Some(stored) = pruning.stored {
            let dfschema = DFSchema::try_from(self.file_schema.as_ref().clone())?;
            if let Ok(predicate) = state.create_physical_expr(stored, &dfschema) {
                source = source.with_predicate(predicate);
            }
        }
        let partitions = state.config().target_partitions().max(1);
        let mut groups: Vec<Vec<PartitionedFile>> = vec![Vec::new(); partitions.min(files.len())];
        let n_groups = groups.len();
        for (i, f) in files.into_iter().enumerate() {
            groups[i % n_groups].push(f);
        }
        let config = FileScanConfigBuilder::new(self.store_url.clone(), Arc::new(source))
            .with_file_groups(groups.into_iter().map(FileGroup::new).collect())
            .with_projection_indices(Some(read.clone()))?
            .with_limit(limit)
            .with_expr_adapter(Some(Arc::new(FieldIds)))
            .build();
        state
            .runtime_env()
            .register_object_store(self.store_url.as_ref(), self.store.clone());
        let scan: Arc<dyn ExecutionPlan> = DataSourceExec::from_data_source(config);
        let scan_schema = scan.schema();

        let options = Arc::new(ConfigOptions::default());
        let mut exprs: Vec<(Arc<dyn PhysicalExpr>, String)> = Vec::with_capacity(outs.len());
        for (out, field) in outs.into_iter().zip(projected.fields()) {
            let column = |p: usize| -> Arc<dyn PhysicalExpr> {
                Arc::new(Column::new(scan_schema.field(p).name(), p))
            };
            let (kind, args) = match out {
                Out::Pass(p) => {
                    exprs.push((column(p), field.name().clone()));
                    continue;
                }
                Out::Restore(p, ty) => (Kind::Restore(ty), vec![column(p)]),
                Out::RowId(s, f, r) => (Kind::RowId, vec![column(s), column(f), column(r)]),
                Out::LastUpdated(s, q) => (Kind::LastUpdated, vec![column(s), column(q)]),
            };
            let udf = Arc::new(ScalarUDF::new_from_impl(Restore::new(kind, field.clone())));
            exprs.push((
                Arc::new(ScalarFunctionExpr::new(
                    "__loams_restore",
                    udf,
                    args,
                    field.clone(),
                    options.clone(),
                )),
                field.name().clone(),
            ));
        }
        Ok(Arc::new(ProjectionExec::try_new(exprs, scan)?))
    }
}

/// How a restore projection computes a column.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Kind {
    /// The ClickHouse type from its stored values.
    Restore(ChType),
    /// `_row_id`: the stored id, else the file's first row id plus the row's
    /// position.
    RowId,
    /// `_last_updated_sequence_number`: the stored one, else the file's data
    /// sequence number.
    LastUpdated,
}

/// The restore projection's function (one per output column).
#[derive(Debug, PartialEq, Eq, Hash)]
struct Restore {
    kind: Kind,
    output: FieldRef,
    signature: Signature,
}

impl Restore {
    fn new(kind: Kind, output: FieldRef) -> Self {
        Self {
            kind,
            output,
            signature: Signature::variadic_any(Volatility::Immutable),
        }
    }
}

impl ScalarUDFImpl for Restore {
    fn name(&self) -> &str {
        "__loams_restore"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _: &[DataType]) -> DfResult<DataType> {
        Ok(self.output.data_type().clone())
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DfResult<ColumnarValue> {
        let rows = args.number_rows;
        let arrays: Vec<ArrayRef> = args
            .args
            .iter()
            .map(|a| a.to_array(rows))
            .collect::<DfResult<_>>()?;
        let out = match &self.kind {
            Kind::Restore(ty) => encodings::restore(ty, &arrays[0], true).map_err(df)?,
            Kind::RowId => {
                let derived = add(&arrays[1], &arrays[2])?;
                fill(&arrays[0], &derived)?
            }
            Kind::LastUpdated => fill(&arrays[0], &arrays[1])?,
        };
        Ok(ColumnarValue::Array(out))
    }
}

/// `stored` where it is not `NULL`, else `inherited` (both `Int64`).
fn fill(stored: &ArrayRef, inherited: &ArrayRef) -> DfResult<ArrayRef> {
    let stored = datafusion::arrow::compute::cast(stored, &DataType::Int64)?;
    let inherited = datafusion::arrow::compute::cast(inherited, &DataType::Int64)?;
    let stored = stored
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| DataFusionError::Internal("not Int64".into()))?;
    let inherited = inherited
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| DataFusionError::Internal("not Int64".into()))?;
    let out: Int64Array = stored
        .iter()
        .zip(inherited.iter())
        .map(|(s, i)| s.or(i))
        .collect();
    Ok(Arc::new(out))
}

/// Maps a table's columns to a data file's by Iceberg field id, so a renamed
/// column reads its older files and a column dropped and added again under
/// its old name does not (R26.5). A file without field ids is read by name.
#[derive(Debug)]
pub struct FieldIds;

fn field_id(f: &Field) -> Option<i32> {
    f.metadata()
        .get(PARQUET_FIELD_ID_META_KEY)
        .and_then(|v| v.parse().ok())
}

impl PhysicalExprAdapterFactory for FieldIds {
    fn create(
        &self,
        logical_file_schema: SchemaRef,
        physical_file_schema: SchemaRef,
    ) -> DfResult<Arc<dyn PhysicalExprAdapter>> {
        let physical: HashMap<i32, String> = physical_file_schema
            .fields()
            .iter()
            .filter_map(|f| Some((field_id(f)?, f.name().clone())))
            .collect();
        let mut renames = HashMap::new();
        let fields: Vec<Field> = logical_file_schema
            .fields()
            .iter()
            .map(|f| {
                let Some(id) = field_id(f).filter(|_| !physical.is_empty()) else {
                    return f.as_ref().clone();
                };
                let name = physical
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("__loams_absent_{id}"));
                if &name != f.name() {
                    renames.insert(f.name().clone(), name.clone());
                }
                f.as_ref().clone().with_name(name)
            })
            .collect();
        let renamed = Arc::new(Schema::new_with_metadata(
            fields,
            logical_file_schema.metadata().clone(),
        ));
        Ok(Arc::new(ById {
            renames,
            inner: DefaultPhysicalExprAdapterFactory.create(renamed, physical_file_schema)?,
        }))
    }
}

#[derive(Debug)]
struct ById {
    renames: HashMap<String, String>,
    inner: Arc<dyn PhysicalExprAdapter>,
}

impl PhysicalExprAdapter for ById {
    fn rewrite(&self, expr: Arc<dyn PhysicalExpr>) -> DfResult<Arc<dyn PhysicalExpr>> {
        let renamed = if self.renames.is_empty() {
            expr
        } else {
            expr.transform(|e| {
                if let Some(c) = (e.as_ref() as &dyn Any).downcast_ref::<Column>()
                    && let Some(name) = self.renames.get(c.name())
                {
                    return Ok(Transformed::yes(
                        Arc::new(Column::new(name, c.index())) as Arc<dyn PhysicalExpr>
                    ));
                }
                Ok(Transformed::no(e))
            })
            .data()?
        };
        self.inner.rewrite(renamed)
    }
}
