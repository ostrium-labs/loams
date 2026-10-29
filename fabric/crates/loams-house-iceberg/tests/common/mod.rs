//! Lake tables written as the chDB House writes them: rows in the engine's
//! layout are stored in the House's encodings (`encodings::store`), written
//! with `loams-house-lake`'s `DataWriter` and committed with its
//! `append_files` and `commit`, with the House's catalog properties.

#![allow(clippy::unwrap_used, dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, RecordBatch};
use datafusion::arrow::datatypes::{Field, Schema};
use datafusion::object_store::ObjectStore;
use datafusion::object_store::local::LocalFileSystem;
use datafusion::prelude::{SessionConfig, SessionContext};
use loams_house_iceberg::encodings::store;
use loams_house_iceberg::{ChColumn, TablePin};
use loams_house_lake::append::append_files;
use loams_house_lake::metadata::{self, MetadataFile};
use loams_house_lake::spec::{
    ListType, MapType, NestedField, PrimitiveType, Schema as IcebergSchema, StructType,
    TableMetadata, Transform, Type, UnboundPartitionSpec,
};
use loams_house_lake::write::DataWriter;
use loams_house_lake::{TableCreation, TableUpdate};
use loams_house_types::ChType;
use loams_house_types::storage::{Storage, storage};

/// A directory for one test's tables, under the target directory.
pub fn dir(test: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("loams-house-iceberg")
        .join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The store every fixture is read through: the local file system.
pub fn local() -> Arc<dyn ObjectStore> {
    Arc::new(LocalFileSystem::new())
}

/// A context like the engine's.
pub fn context() -> SessionContext {
    SessionContext::new_with_config(SessionConfig::new().with_target_partitions(1))
}

fn primitive(s: &Storage) -> Option<PrimitiveType> {
    Some(match s {
        Storage::Int => PrimitiveType::Int,
        Storage::Long | Storage::LongNanos => PrimitiveType::Long,
        Storage::Float => PrimitiveType::Float,
        Storage::Double => PrimitiveType::Double,
        Storage::Decimal(p, s) => PrimitiveType::Decimal {
            precision: u32::from(*p),
            scale: u32::from(*s),
        },
        Storage::Boolean => PrimitiveType::Boolean,
        Storage::String => PrimitiveType::String,
        Storage::Fixed(n) => PrimitiveType::Fixed(u64::from(*n)),
        Storage::Date => PrimitiveType::Date,
        Storage::TimestampTz => PrimitiveType::Timestamptz,
        Storage::TimestampTzNs => PrimitiveType::TimestamptzNs,
        Storage::Uuid => PrimitiveType::Uuid,
        Storage::Binary => PrimitiveType::Binary,
        _ => return None,
    })
}

/// The next nested field id (a function, not a closure: `iceberg` recurses
/// with `next` too).
fn id(next: &mut i32) -> i32 {
    *next += 1;
    *next
}

/// The Iceberg type of `t` (as `loams-house`'s `types.rs` makes it), with
/// nested ids from `next`.
fn iceberg(t: &ChType, top: bool, next: &mut i32) -> Type {
    let s = storage(t, top).unwrap();
    if let Some(p) = primitive(&s) {
        return Type::Primitive(p);
    }
    match t.base() {
        ChType::Array(inner) => {
            let eid = id(next);
            let et = iceberg(inner, false, next);
            Type::List(ListType {
                element_field: Arc::new(NestedField::list_element(eid, et, !inner.is_nullable())),
            })
        }
        ChType::Map(k, v) => {
            let kid = id(next);
            let vid = id(next);
            let kt = iceberg(k, false, next);
            let vt = iceberg(v, false, next);
            Type::Map(MapType {
                key_field: Arc::new(NestedField::map_key_element(kid, kt)),
                value_field: Arc::new(NestedField::map_value_element(vid, vt, !v.is_nullable())),
            })
        }
        ChType::Tuple(elements) => {
            let mut fields = Vec::new();
            for (i, (name, et)) in elements.iter().enumerate() {
                let fid = id(next);
                let ty = iceberg(et, false, next);
                let name = name.clone().unwrap_or_else(|| (i + 1).to_string());
                fields.push(Arc::new(if et.is_nullable() {
                    NestedField::optional(fid, name, ty)
                } else {
                    NestedField::required(fid, name, ty)
                }));
            }
            Type::Struct(StructType::new(fields))
        }
        other => panic!("no Iceberg type for {other}"),
    }
}

/// A House lake table on the local file system.
pub struct Lake {
    /// The location the metadata records (`s3://house/<db>/<name>`), as the
    /// House's bucket would hold it.
    pub location: String,
    /// Where the files are: the location's directory on the local disk.
    pub dir: PathBuf,
    pub metadata: TableMetadata,
    pub file: MetadataFile,
    pub columns: Vec<ChColumn>,
    pub db: String,
    pub name: String,
    pub properties: HashMap<String, String>,
}

/// A column `name T`.
pub fn column(name: &str, ty: &str) -> ChColumn {
    ChColumn {
        name: name.to_string(),
        ch_type: ty.to_string(),
        ..ChColumn::default()
    }
}

fn fields(columns: &[ChColumn], first_id: i32) -> (Vec<Arc<NestedField>>, i32) {
    // Top-level ids first (as Iceberg assigns them), nested ids after.
    let stored: Vec<&ChColumn> = columns.iter().filter(|c| c.stored()).collect();
    let mut next = first_id + stored.len() as i32 - 1;
    let mut out = Vec::new();
    for (i, c) in stored.iter().enumerate() {
        let t = loams_house_types::parse(&c.ch_type).unwrap();
        let ty = iceberg(&t, true, &mut next);
        let id = first_id + i as i32;
        out.push(Arc::new(if t.is_nullable() {
            NestedField::optional(id, &c.name, ty)
        } else {
            NestedField::required(id, &c.name, ty)
        }));
    }
    (out, next)
}

impl Lake {
    /// `CREATE TABLE db.name (columns) ENGINE = engine ORDER BY order_by
    /// [PARTITION BY identity(partition)] [TTL ttl]`.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        root: &Path,
        db: &str,
        name: &str,
        columns: Vec<ChColumn>,
        engine: &str,
        order_by: &str,
        partition: Option<&str>,
        ttl: &str,
    ) -> Self {
        let location = format!("s3://house/{db}/{name}");
        let dir = root.join(db).join(name);
        let (fields, _) = fields(&columns, 1);
        let schema = IcebergSchema::builder()
            .with_fields(fields.clone())
            .build()
            .unwrap();
        let mut spec = UnboundPartitionSpec::builder();
        if let Some(p) = partition {
            let id = fields.iter().find(|f| f.name == p).unwrap().id;
            spec = spec
                .add_partition_field(id, p, Transform::Identity)
                .unwrap();
        }
        let mut properties = HashMap::new();
        properties.insert("loams.ch.engine".to_string(), engine.to_string());
        properties.insert(
            "loams.ch.types".to_string(),
            serde_json::to_string(&columns).unwrap(),
        );
        properties.insert("loams.ch.order_by".to_string(), order_by.to_string());
        if let Some(p) = partition {
            properties.insert("loams.ch.partition_by".to_string(), p.to_string());
        }
        if !ttl.is_empty() {
            properties.insert("loams.ch.ttl".to_string(), ttl.to_string());
        }
        let creation = TableCreation::builder()
            .name(name.to_string())
            .location(location.clone())
            .schema(schema)
            .partition_spec(spec.build())
            .properties(properties.clone())
            .build();
        let metadata = metadata::create(creation).unwrap();
        let file = MetadataFile::first(&location);
        let lake = Self {
            location,
            dir,
            metadata,
            file,
            columns,
            db: db.to_string(),
            name: name.to_string(),
            properties,
        };
        lake.write_metadata();
        lake
    }

    fn path(&self, location: &str) -> PathBuf {
        let rest = location.strip_prefix(&self.location).unwrap();
        self.dir.join(rest.trim_start_matches('/'))
    }

    fn put(&self, location: &str, bytes: &[u8]) {
        let p = self.path(location);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn write_metadata(&self) {
        self.put(
            &self.file.to_string(),
            &metadata::to_json(&self.metadata).unwrap(),
        );
    }

    fn commit(
        &mut self,
        requirements: &[loams_house_lake::TableRequirement],
        updates: Vec<TableUpdate>,
    ) {
        self.metadata =
            metadata::commit(&self.metadata, &self.file, requirements, updates).unwrap();
        self.file = self.file.next();
        self.write_metadata();
    }

    /// `INSERT`: one batch whose columns are the current schema's, in the
    /// engine's layout. Each insert is one snapshot.
    pub async fn insert(&mut self, columns: Vec<(&str, ArrayRef)>) {
        let schema = self.metadata.current_schema().clone();
        let mut arrays = Vec::new();
        let mut fields = Vec::new();
        for f in schema.as_struct().fields() {
            let c = self.columns.iter().find(|c| c.name == f.name).unwrap();
            let t = loams_house_types::parse(&c.ch_type).unwrap();
            let (_, a) = columns.iter().find(|(n, _)| *n == f.name).unwrap();
            let s = store(&t, a, true).unwrap();
            fields.push(Field::new(&f.name, s.data_type().clone(), true));
            arrays.push(s);
        }
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap();
        let mut ipc = Vec::new();
        {
            let mut w =
                arrow_ipc::writer::StreamWriter::try_new(&mut ipc, &batch.schema()).unwrap();
            w.write(&batch).unwrap();
            w.finish().unwrap();
        }
        let prefix = uuid_like();
        let mut writer = DataWriter::new(&self.metadata, &prefix, 1 << 30, 1 << 30)
            .await
            .unwrap();
        writer.push(&ipc).await.unwrap();
        let written = writer.finish().await.unwrap();
        for (location, bytes) in &written.files {
            self.put(location, bytes);
        }
        let parent = self
            .metadata
            .current_snapshot()
            .map(|s| std::fs::read(self.path(s.manifest_list())).unwrap());
        let append = append_files(
            &self.metadata,
            parent.as_deref(),
            written.data_files,
            Vec::new(),
            now_ms(),
        )
        .await
        .unwrap();
        for (location, bytes) in &append.files {
            self.put(location, bytes);
        }
        self.commit(&append.requirements, append.updates);
    }

    /// `ALTER TABLE … ADD COLUMN`: an optional Iceberg field.
    pub fn add_column(&mut self, c: ChColumn) {
        let current = self.metadata.current_schema();
        let mut all: Vec<Arc<NestedField>> = current.as_struct().fields().to_vec();
        let t = loams_house_types::parse(&c.ch_type).unwrap();
        let id = self.metadata.last_column_id() + 1;
        let mut next = id;
        let ty = iceberg(&t, true, &mut next);
        all.push(Arc::new(NestedField::optional(id, &c.name, ty)));
        let schema = IcebergSchema::builder()
            .with_schema_id(current.schema_id() + 1)
            .with_fields(all)
            .build()
            .unwrap();
        self.columns.push(c);
        self.set_types();
        self.commit(
            &[],
            vec![
                TableUpdate::AddSchema { schema },
                TableUpdate::SetCurrentSchema { schema_id: -1 },
                TableUpdate::SetProperties {
                    updates: self.properties.clone(),
                },
            ],
        );
    }

    /// `ALTER TABLE … RENAME COLUMN from TO to`: the same field id.
    pub fn rename_column(&mut self, from: &str, to: &str) {
        let current = self.metadata.current_schema();
        let all: Vec<Arc<NestedField>> = current
            .as_struct()
            .fields()
            .iter()
            .map(|f| {
                if f.name == from {
                    let mut g = f.as_ref().clone();
                    g.name = to.to_string();
                    Arc::new(g)
                } else {
                    f.clone()
                }
            })
            .collect();
        let schema = IcebergSchema::builder()
            .with_schema_id(current.schema_id() + 1)
            .with_fields(all)
            .build()
            .unwrap();
        for c in &mut self.columns {
            if c.name == from {
                c.name = to.to_string();
            }
        }
        self.set_types();
        self.commit(
            &[],
            vec![
                TableUpdate::AddSchema { schema },
                TableUpdate::SetCurrentSchema { schema_id: -1 },
                TableUpdate::SetProperties {
                    updates: self.properties.clone(),
                },
            ],
        );
    }

    fn set_types(&mut self) {
        self.properties.insert(
            "loams.ch.types".to_string(),
            serde_json::to_string(&self.columns).unwrap(),
        );
    }

    /// The pin of the current snapshot, as the dev server makes it.
    pub fn pin(&self) -> TablePin {
        let file = format!("file://{}", self.path(&self.file.to_string()).display());
        // The metadata as the engine reads it: through the fork's
        // `TableMetadata` (`iceberg-engine`), not `loams-house-lake`'s
        // iceberg 0.10.1 that wrote it.
        let json = metadata::to_json(&self.metadata).unwrap();
        let mut pin = TablePin::from_metadata_json(&self.db, &self.name, &file, &json).unwrap();
        pin.location = format!("file://{}", self.dir.display());
        pin
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("w{}-{}", now_ms(), N.fetch_add(1, Ordering::Relaxed))
}
