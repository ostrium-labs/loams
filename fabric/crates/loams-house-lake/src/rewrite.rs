//! A `replace` snapshot: files removed and files added without changing the
//! table's data (HS1 Task 13, §49 §6.2 compaction).
//!
//! Compaction reads a pinned snapshot's rows (merged for Replacing and Summing
//! tables), writes them as new files ([`crate::write`]), and commits this:
//!
//! * every manifest of the parent that lists a removed file is written again,
//!   its entries kept as `EXISTING` (with their snapshot and sequence numbers)
//!   and the removed ones as `DELETED`;
//! * the other manifests are carried over unchanged;
//! * one new manifest lists the added files;
//! * the commit requires `main` to be the snapshot that was read
//!   (`assert-ref-snapshot-id`), so a concurrent commit makes it fail and the
//!   caller abandons the work rather than lose the other commit's rows.
//!
//! Like [`crate::append`], nothing here touches a store: the caller reads the
//! parent's manifest list and manifests and writes what comes back.

use std::collections::{HashMap, HashSet};

use bytes::Bytes;
use iceberg::io::FileIO;
use iceberg::spec::{
    DataFile, FormatVersion, Manifest, ManifestContentType, ManifestFile, ManifestList,
    ManifestListWriter, ManifestWriterBuilder, Operation, Snapshot, SnapshotReference,
    SnapshotRetention, Summary, TableMetadata,
};
use iceberg::{TableRequirement, TableUpdate};

use crate::append::{Append, MAIN};
use crate::metadata::LakeError;

fn invalid(what: &str, err: impl std::fmt::Display) -> LakeError {
    LakeError::Invalid(format!("{what}: {err}"))
}

/// The manifests of the parent's manifest list, which the caller reads.
pub fn parent_manifests(
    list: &[u8],
    version: FormatVersion,
) -> Result<Vec<ManifestFile>, LakeError> {
    Ok(ManifestList::parse_with_version(list, version)
        .map_err(|e| invalid("the parent's manifest list", e))?
        .consume_entries()
        .into_iter()
        .collect())
}

fn fresh_snapshot_id(metadata: &TableMetadata) -> i64 {
    loop {
        let bits = uuid::Uuid::new_v4().as_u128();
        let id = ((bits as u64) & (i64::MAX as u64)) as i64;
        if id != 0 && metadata.snapshot_by_id(id).is_none() {
            return id;
        }
    }
}

fn total(summary: Option<&Summary>, key: &str) -> u64 {
    summary
        .and_then(|s| s.additional_properties.get(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Builds the `replace` of the files at `removed` by `added` on the current
/// snapshot. `manifests` are the current snapshot's manifests with their bytes
/// ([`parent_manifests`]). Every removed file must be live in them.
pub async fn replace(
    metadata: &TableMetadata,
    manifests: Vec<(ManifestFile, Bytes)>,
    removed: &HashSet<String>,
    added: Vec<DataFile>,
    summary: Vec<(String, String)>,
    timestamp_ms: i64,
) -> Result<Append, LakeError> {
    rewrite(
        metadata,
        manifests,
        removed,
        added,
        summary,
        timestamp_ms,
        Operation::Replace,
    )
    .await
}

/// Builds a `delete` snapshot that removes the whole files at `removed` and
/// adds none (HS1 Task 23: the query log's retention drops its old days'
/// files). It requires `main` to be the snapshot it was built on, like
/// [`replace`].
pub async fn delete_files(
    metadata: &TableMetadata,
    manifests: Vec<(ManifestFile, Bytes)>,
    removed: &HashSet<String>,
    summary: Vec<(String, String)>,
    timestamp_ms: i64,
) -> Result<Append, LakeError> {
    rewrite(
        metadata,
        manifests,
        removed,
        Vec::new(),
        summary,
        timestamp_ms,
        Operation::Delete,
    )
    .await
}

async fn rewrite(
    metadata: &TableMetadata,
    manifests: Vec<(ManifestFile, Bytes)>,
    removed: &HashSet<String>,
    added: Vec<DataFile>,
    summary: Vec<(String, String)>,
    timestamp_ms: i64,
    operation: Operation,
) -> Result<Append, LakeError> {
    let version = metadata.format_version();
    if version != FormatVersion::V2 {
        return Err(LakeError::Invalid(format!(
            "compaction of a format {version} table is not supported yet"
        )));
    }
    let parent = metadata
        .current_snapshot()
        .ok_or_else(|| LakeError::Invalid("the table has no snapshot to compact".to_string()))?;
    let snapshot_id = fresh_snapshot_id(metadata);
    let sequence_number = metadata.next_sequence_number();
    let schema = metadata.current_schema().clone();
    let base = format!("{}/metadata", metadata.location().trim_end_matches('/'));
    let token = uuid::Uuid::new_v4();
    let io = FileIO::new_with_memory();
    let mut files: Vec<(String, Bytes)> = Vec::new();
    let mut list_entries: Vec<ManifestFile> = Vec::new();
    let mut found: HashSet<String> = HashSet::new();
    let (mut deleted_records, mut deleted_bytes, mut deleted_files) = (0u64, 0u64, 0u64);

    for (index, (manifest_file, bytes)) in manifests.into_iter().enumerate() {
        let manifest = Manifest::parse_avro(&bytes)
            .map_err(|e| invalid(&format!("manifest {}", manifest_file.manifest_path), e))?;
        let touched = manifest_file.content == ManifestContentType::Data
            && manifest
                .entries()
                .iter()
                .any(|e| e.is_alive() && removed.contains(e.data_file().file_path()));
        if !touched {
            list_entries.push(manifest_file);
            continue;
        }
        let spec = metadata
            .partition_spec_by_id(manifest_file.partition_spec_id)
            .ok_or_else(|| {
                LakeError::Invalid(format!(
                    "manifest {} names partition spec {}, which the table does not have",
                    manifest_file.manifest_path, manifest_file.partition_spec_id
                ))
            })?
            .as_ref()
            .clone();
        let location = format!("{base}/{token}-r{index}.avro");
        let output = io
            .new_output(&location)
            .map_err(|e| invalid("a rewritten manifest", e))?;
        let mut writer =
            ManifestWriterBuilder::new(output, Some(snapshot_id), schema.clone(), spec)
                .build_v2_data();
        for entry in manifest.entries().iter().filter(|e| e.is_alive()) {
            let file = entry.data_file().clone();
            let sequence = entry
                .sequence_number()
                .unwrap_or(manifest_file.sequence_number);
            let file_sequence = entry.file_sequence_number.or(Some(sequence));
            if removed.contains(file.file_path()) {
                found.insert(file.file_path().to_string());
                deleted_files += 1;
                deleted_records += file.record_count();
                deleted_bytes += file.file_size_in_bytes();
                writer
                    .add_delete_file(file, sequence, file_sequence)
                    .map_err(|e| invalid("a rewritten manifest", e))?;
            } else {
                let added_by = entry
                    .snapshot_id()
                    .unwrap_or(manifest_file.added_snapshot_id);
                writer
                    .add_existing_file(file, added_by, sequence, file_sequence)
                    .map_err(|e| invalid("a rewritten manifest", e))?;
            }
        }
        let written = writer
            .write_manifest_file()
            .await
            .map_err(|e| invalid("a rewritten manifest", e))?;
        let bytes = io
            .new_input(&location)
            .map_err(|e| invalid("a rewritten manifest", e))?
            .read()
            .await
            .map_err(|e| invalid("a rewritten manifest", e))?;
        files.push((location, bytes));
        list_entries.push(written);
    }
    if let Some(missing) = removed.iter().find(|r| !found.contains(*r)) {
        return Err(LakeError::Conflict(format!(
            "{missing} is not a live file of the current snapshot"
        )));
    }

    let (mut added_records, mut added_bytes) = (0u64, 0u64);
    let added_files = added.len() as u64;
    if !added.is_empty() {
        let spec = metadata.default_partition_spec().as_ref().clone();
        let location = format!("{base}/{token}-a.avro");
        let output = io
            .new_output(&location)
            .map_err(|e| invalid("the new manifest", e))?;
        let mut writer =
            ManifestWriterBuilder::new(output, Some(snapshot_id), schema.clone(), spec)
                .build_v2_data();
        for file in added {
            added_records += file.record_count();
            added_bytes += file.file_size_in_bytes();
            writer
                .add_file(file, sequence_number)
                .map_err(|e| invalid("the new manifest", e))?;
        }
        let written = writer
            .write_manifest_file()
            .await
            .map_err(|e| invalid("the new manifest", e))?;
        let bytes = io
            .new_input(&location)
            .map_err(|e| invalid("the new manifest", e))?
            .read()
            .await
            .map_err(|e| invalid("the new manifest", e))?;
        files.push((location, bytes));
        list_entries.push(written);
    }

    let list_location = format!("{base}/snap-{snapshot_id}-1-{token}.avro");
    let output = io
        .new_output(&list_location)
        .map_err(|e| invalid("the manifest list", e))?;
    let mut list = ManifestListWriter::v2(
        output
            .writer()
            .await
            .map_err(|e| invalid("the manifest list", e))?,
        snapshot_id,
        Some(parent.snapshot_id()),
        sequence_number,
    );
    list.add_manifests(list_entries.into_iter())
        .map_err(|e| invalid("the manifest list", e))?;
    list.close()
        .await
        .map_err(|e| invalid("the manifest list", e))?;
    let list_bytes = io
        .new_input(&list_location)
        .map_err(|e| invalid("the manifest list", e))?
        .read()
        .await
        .map_err(|e| invalid("the manifest list", e))?;
    files.push((list_location.clone(), list_bytes));

    let previous = Some(parent.summary());
    let mut properties: HashMap<String, String> = summary.into_iter().collect();
    let mut put = |key: &str, value: u64| {
        properties.insert(key.to_string(), value.to_string());
    };
    put("added-data-files", added_files);
    put("added-records", added_records);
    put("added-files-size", added_bytes);
    put("deleted-data-files", deleted_files);
    put("deleted-records", deleted_records);
    put("removed-files-size", deleted_bytes);
    let net = |key: &str, add: u64, sub: u64| (total(previous, key) + add).saturating_sub(sub);
    put(
        "total-data-files",
        net("total-data-files", added_files, deleted_files),
    );
    put(
        "total-records",
        net("total-records", added_records, deleted_records),
    );
    put(
        "total-files-size",
        net("total-files-size", added_bytes, deleted_bytes),
    );
    for key in [
        "total-delete-files",
        "total-position-deletes",
        "total-equality-deletes",
    ] {
        put(key, total(previous, key));
    }
    let snapshot = Snapshot::builder()
        .with_snapshot_id(snapshot_id)
        .with_parent_snapshot_id(Some(parent.snapshot_id()))
        .with_sequence_number(sequence_number)
        .with_timestamp_ms(timestamp_ms.max(metadata.last_updated_ms()))
        .with_manifest_list(list_location)
        .with_summary(Summary {
            operation,
            additional_properties: properties,
        })
        .with_schema_id(schema.schema_id())
        .build();
    Ok(Append {
        snapshot_id,
        files,
        requirements: vec![TableRequirement::RefSnapshotIdMatch {
            r#ref: MAIN.to_string(),
            snapshot_id: Some(parent.snapshot_id()),
        }],
        updates: vec![
            TableUpdate::AddSnapshot { snapshot },
            TableUpdate::SetSnapshotRef {
                ref_name: MAIN.to_string(),
                reference: SnapshotReference::new(
                    snapshot_id,
                    SnapshotRetention::branch(None, None, None),
                ),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use iceberg::TableCreation;
    use iceberg::spec::{NestedField, PrimitiveType, Schema, Struct, Type, UnboundPartitionSpec};

    use super::*;
    use crate::append::{NewDataFile, fast_append};
    use crate::files;
    use crate::metadata::{self, MetadataFile};

    fn table() -> TableMetadata {
        let schema = Schema::builder()
            .with_fields(vec![
                NestedField::required(1, "k", Type::Primitive(PrimitiveType::Long)).into(),
            ])
            .build()
            .expect("schema");
        let creation = TableCreation::builder()
            .name("t".to_string())
            .location("s3://local/ns/1/house/default/t-x".to_string())
            .schema(schema)
            .partition_spec(UnboundPartitionSpec::builder().build())
            .properties(HashMap::new())
            .build();
        metadata::create(creation).expect("metadata")
    }

    fn file(name: &str, rows: u64) -> NewDataFile {
        NewDataFile {
            location: format!("s3://local/ns/1/house/default/t-x/data/{name}.parquet"),
            record_count: rows,
            size_bytes: 100 * rows,
            partition: Struct::empty(),
        }
    }

    /// A store in a map: what the commits wrote.
    async fn commit(
        metadata: TableMetadata,
        store: &mut HashMap<String, Bytes>,
        built: Append,
    ) -> TableMetadata {
        for (location, bytes) in built.files {
            store.insert(location, bytes);
        }
        let file = MetadataFile::first(metadata.location());
        metadata::commit(&metadata, &file, &built.requirements, built.updates).expect("commit")
    }

    fn manifests_of(
        metadata: &TableMetadata,
        store: &HashMap<String, Bytes>,
    ) -> Vec<(ManifestFile, Bytes)> {
        let snapshot = metadata.current_snapshot().expect("snapshot");
        parent_manifests(&store[snapshot.manifest_list()], FormatVersion::V2)
            .expect("manifests")
            .into_iter()
            .map(|m| {
                let bytes = store[&m.manifest_path].clone();
                (m, bytes)
            })
            .collect()
    }

    fn live(metadata: &TableMetadata, store: &HashMap<String, Bytes>) -> Vec<(String, i64)> {
        let snapshot = metadata.current_snapshot().expect("snapshot");
        let mut out = Vec::new();
        for m in
            files::manifests(&store[snapshot.manifest_list()], FormatVersion::V2).expect("list")
        {
            for f in files::live_files(&store[&m.location], &m).expect("files") {
                out.push((
                    f.location.rsplit('/').next().unwrap_or("").to_string(),
                    f.sequence_number,
                ));
            }
        }
        out.sort();
        out
    }

    #[tokio::test]
    async fn replace_removes_and_adds_and_keeps_the_rest() {
        let mut store: HashMap<String, Bytes> = HashMap::new();
        let mut metadata = table();
        for (round, names) in [vec!["a", "b"], vec!["c"]].into_iter().enumerate() {
            let parent = metadata
                .current_snapshot()
                .map(|s| store[s.manifest_list()].clone());
            let built = fast_append(
                &metadata,
                parent.as_deref(),
                names.into_iter().map(|n| file(n, 10)).collect(),
                1_000 + round as i64,
            )
            .await
            .expect("append");
            metadata = commit(metadata, &mut store, built).await;
        }
        assert_eq!(
            live(&metadata, &store),
            vec![
                ("a.parquet".into(), 1),
                ("b.parquet".into(), 1),
                ("c.parquet".into(), 2)
            ]
        );

        // Compact a and c into d.
        let removed: HashSet<String> = ["a", "c"]
            .iter()
            .map(|n| format!("s3://local/ns/1/house/default/t-x/data/{n}.parquet"))
            .collect();
        let mut d = DataFileBuilderFor::new("d", 20);
        let built = replace(
            &metadata,
            manifests_of(&metadata, &store),
            &removed,
            vec![d.build(&metadata)],
            vec![("loams.compaction".to_string(), "1".to_string())],
            5_000,
        )
        .await
        .expect("replace");
        let parent = metadata.current_snapshot().expect("parent").snapshot_id();
        assert_eq!(
            built.requirements,
            vec![TableRequirement::RefSnapshotIdMatch {
                r#ref: MAIN.to_string(),
                snapshot_id: Some(parent),
            }]
        );
        metadata = commit(metadata, &mut store, built).await;
        let snapshot = metadata.current_snapshot().expect("replace");
        assert_eq!(snapshot.summary().operation, Operation::Replace);
        let props = &snapshot.summary().additional_properties;
        assert_eq!(props["total-records"], "30");
        assert_eq!(props["deleted-data-files"], "2");
        assert_eq!(props["total-data-files"], "2");
        // b keeps its sequence number; d has the new one.
        assert_eq!(
            live(&metadata, &store),
            vec![("b.parquet".into(), 1), ("d.parquet".into(), 3)]
        );

        // A file that is no longer live: refused, nothing built.
        let err = replace(
            &metadata,
            manifests_of(&metadata, &store),
            &removed,
            Vec::new(),
            Vec::new(),
            6_000,
        )
        .await
        .expect_err("gone");
        assert!(matches!(err, LakeError::Conflict(_)), "{err}");
    }

    /// The query log's retention (HS1 Task 23): whole files go, nothing is
    /// added, and the snapshot says `delete`.
    #[tokio::test]
    async fn delete_files_removes_whole_files() {
        let mut store: HashMap<String, Bytes> = HashMap::new();
        let mut metadata = table();
        let built = fast_append(&metadata, None, vec![file("a", 10), file("b", 5)], 1_000)
            .await
            .expect("append");
        metadata = commit(metadata, &mut store, built).await;
        let removed: HashSet<String> =
            ["s3://local/ns/1/house/default/t-x/data/a.parquet".to_string()].into();
        let built = delete_files(
            &metadata,
            manifests_of(&metadata, &store),
            &removed,
            vec![("loams.retention".to_string(), "1".to_string())],
            2_000,
        )
        .await
        .expect("delete");
        metadata = commit(metadata, &mut store, built).await;
        let snapshot = metadata.current_snapshot().expect("delete");
        assert_eq!(snapshot.summary().operation, Operation::Delete);
        let props = &snapshot.summary().additional_properties;
        assert_eq!(props["total-records"], "5");
        assert_eq!(props["deleted-records"], "10");
        assert_eq!(props["added-data-files"], "0");
        assert_eq!(live(&metadata, &store), vec![("b.parquet".into(), 1)]);
    }

    /// A data file for a test, as the writer would make one.
    struct DataFileBuilderFor {
        name: String,
        rows: u64,
    }

    impl DataFileBuilderFor {
        fn new(name: &str, rows: u64) -> Self {
            Self {
                name: name.to_string(),
                rows,
            }
        }

        fn build(&mut self, metadata: &TableMetadata) -> DataFile {
            iceberg::spec::DataFileBuilder::default()
                .content(iceberg::spec::DataContentType::Data)
                .file_path(format!(
                    "s3://local/ns/1/house/default/t-x/data/{}.parquet",
                    self.name
                ))
                .file_format(iceberg::spec::DataFileFormat::Parquet)
                .partition(Struct::empty())
                .partition_spec_id(metadata.default_partition_spec().spec_id())
                .record_count(self.rows)
                .file_size_in_bytes(100 * self.rows)
                .build()
                .expect("a data file")
        }
    }
}
