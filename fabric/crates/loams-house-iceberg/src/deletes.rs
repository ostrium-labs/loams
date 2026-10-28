//! Deletes (plan HE1 Task 26): Iceberg v2 position delete files and v3
//! deletion vectors, applied as a row selection of each data file, so the
//! Parquet reader never decodes a deleted row.
//!
//! The fork's planner attaches to each data file the deletes that apply to it
//! (by sequence number and partition, R11.1). A deletion vector is one Puffin
//! blob, read by its offset and length and decoded by the fork
//! (`DeleteVector::deserialize`, which takes Spark's run containers too); a
//! position delete file is Parquet (`file_path`, `pos`), read once per scan.
//! Equality deletes, which the House never writes, are `48` until a task needs
//! them.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use datafusion::arrow::array::{Array, AsArray};
use datafusion::arrow::datatypes::Int64Type;
use datafusion::datasource::listing::PartitionedFile;
use datafusion::datasource::physical_plan::parquet::{
    DefaultParquetFileReaderFactory, ParquetFileReaderFactory,
};
use datafusion::datasource::physical_plan::parquet::{ParquetAccessPlan, RowGroupAccess};
use datafusion::object_store::ObjectStore;
use datafusion::parquet::arrow::ParquetRecordBatchStreamBuilder;
use datafusion::parquet::arrow::arrow_reader::{RowSelection, RowSelector};
use datafusion::parquet::arrow::async_reader::AsyncFileReader;
use datafusion::physical_plan::metrics::ExecutionPlanMetricsSet;
use futures::TryStreamExt;
use iceberg_engine::delete_vector::DeleteVector;
use iceberg_engine::scan::{FileScanTask, FileScanTaskDeleteFile};
use iceberg_engine::spec::{DataContentType, DataFileFormat};
use loams_house_types::ChError;
use std::sync::Mutex;

use crate::io::{Paths, StoreStorage};

fn read_error(path: &str, e: impl std::fmt::Display) -> ChError {
    ChError::cannot_parse_text(format!("Reading the delete file {path}: {e}"))
}

/// The deletes of one scan: position delete files read once, by path.
#[derive(Debug)]
pub struct Deletes {
    store: Arc<dyn ObjectStore>,
    storage: StoreStorage,
    paths: Arc<Paths>,
    #[allow(clippy::type_complexity)]
    position_files: Mutex<HashMap<String, Arc<HashMap<String, Vec<u64>>>>>,
}

impl Deletes {
    /// Deletes read from `store`.
    pub fn new(store: Arc<dyn ObjectStore>, storage: StoreStorage, paths: Arc<Paths>) -> Self {
        Self {
            store,
            storage,
            paths,
            position_files: Mutex::new(HashMap::new()),
        }
    }

    /// The deleted positions of `task`'s data file, ascending.
    ///
    /// # Errors
    ///
    /// An unreadable delete file, or an equality delete (`48`).
    pub async fn positions(&self, task: &FileScanTask) -> Result<BTreeSet<u64>, ChError> {
        let mut deleted = BTreeSet::new();
        for delete in task.deletes() {
            match (delete.file_type(), delete.file_format()) {
                (DataContentType::EqualityDeletes, _) => {
                    return Err(ChError::not_implemented(format!(
                        "Equality deletes ({}) are not supported by the Loams House engine yet",
                        delete.file_path()
                    )));
                }
                (DataContentType::PositionDeletes, DataFileFormat::Puffin) => {
                    deleted.extend(self.vector(delete).await?.iter());
                }
                (DataContentType::PositionDeletes, _) => {
                    let file = self.position_file(delete).await?;
                    if let Some(positions) = file.get(task.data_file_path()) {
                        deleted.extend(positions.iter().copied());
                    }
                }
                (other, _) => {
                    return Err(read_error(
                        delete.file_path(),
                        format!("a {other:?} file among the deletes"),
                    ));
                }
            }
        }
        Ok(deleted)
    }

    /// A Parquet reader of `path` (`size` bytes) through the store.
    fn reader(&self, path: &str, size: u64) -> Result<Box<dyn AsyncFileReader>, ChError> {
        let file = PartitionedFile::new(self.paths.key(path).to_string(), size);
        let reader = DefaultParquetFileReaderFactory::new(self.store.clone())
            .create_reader(0, file, None, &ExecutionPlanMetricsSet::new())
            .map_err(|e| read_error(path, e))?;
        Ok(reader)
    }

    async fn vector(&self, delete: &FileScanTaskDeleteFile) -> Result<DeleteVector, ChError> {
        let path = delete.file_path();
        let (Some(offset), Some(size)) = (delete.content_offset(), delete.content_size_in_bytes())
        else {
            return Err(read_error(
                path,
                "a deletion vector without its offset and size",
            ));
        };
        let blob = self
            .storage
            .read_range(path, offset..offset + size)
            .await
            .map_err(|e| read_error(path, e))?;
        DeleteVector::deserialize(&blob).map_err(|e| read_error(path, e))
    }

    async fn position_file(
        &self,
        delete: &FileScanTaskDeleteFile,
    ) -> Result<Arc<HashMap<String, Vec<u64>>>, ChError> {
        let path = delete.file_path().to_string();
        if let Some(found) = self
            .position_files
            .lock()
            .ok()
            .and_then(|m| m.get(&path).cloned())
        {
            return Ok(found);
        }
        let reader = self.reader(&path, delete.file_size_in_bytes())?;
        let stream = ParquetRecordBatchStreamBuilder::new(reader)
            .await
            .map_err(|e| read_error(&path, e))?
            .build()
            .map_err(|e| read_error(&path, e))?;
        let batches: Vec<_> = stream
            .try_collect()
            .await
            .map_err(|e| read_error(&path, e))?;
        let mut by_file: HashMap<String, Vec<u64>> = HashMap::new();
        for batch in batches {
            let (Some(files), Some(positions)) = (
                batch.column_by_name("file_path"),
                batch.column_by_name("pos"),
            ) else {
                return Err(read_error(&path, "no file_path and pos columns"));
            };
            let files = datafusion::arrow::compute::cast(
                files,
                &datafusion::arrow::datatypes::DataType::Utf8,
            )
            .map_err(|e| read_error(&path, e))?;
            let files = files.as_string::<i32>();
            let positions = positions
                .as_primitive_opt::<Int64Type>()
                .ok_or_else(|| read_error(&path, "pos is not a long"))?;
            for i in 0..batch.num_rows() {
                if files.is_null(i) || positions.is_null(i) {
                    continue;
                }
                let pos = u64::try_from(positions.value(i))
                    .map_err(|_| read_error(&path, "a negative position"))?;
                by_file
                    .entry(files.value(i).to_string())
                    .or_default()
                    .push(pos);
            }
        }
        let by_file = Arc::new(by_file);
        if let Ok(mut m) = self.position_files.lock() {
            m.insert(path, by_file.clone());
        }
        Ok(by_file)
    }

    /// The row groups of the data file `path` (`size` bytes), with `deleted`
    /// positions skipped: `None` when nothing is deleted.
    ///
    /// # Errors
    ///
    /// An unreadable footer.
    pub async fn access_plan(
        &self,
        path: &str,
        size: u64,
        deleted: &BTreeSet<u64>,
    ) -> Result<Option<ParquetAccessPlan>, ChError> {
        if deleted.is_empty() {
            return Ok(None);
        }
        let reader = self.reader(path, size)?;
        let builder = ParquetRecordBatchStreamBuilder::new(reader)
            .await
            .map_err(|e| read_error(path, e))?;
        let counts: Vec<u64> = builder
            .metadata()
            .row_groups()
            .iter()
            .map(|g| u64::try_from(g.num_rows()).unwrap_or(0))
            .collect();
        Ok(Some(access_plan(&counts, deleted)))
    }
}

/// Row groups of `counts` rows with the `deleted` positions skipped.
pub fn access_plan(counts: &[u64], deleted: &BTreeSet<u64>) -> ParquetAccessPlan {
    let mut groups = Vec::with_capacity(counts.len());
    let mut start = 0u64;
    for &count in counts {
        let end = start + count;
        let inside: Vec<u64> = deleted.range(start..end).map(|p| p - start).collect();
        groups.push(if inside.is_empty() {
            RowGroupAccess::Scan
        } else if inside.len() as u64 == count {
            RowGroupAccess::Skip
        } else {
            let mut selectors = Vec::new();
            let mut at = 0u64;
            for p in inside {
                if p > at {
                    selectors.push(RowSelector::select(usize::try_from(p - at).unwrap_or(0)));
                }
                selectors.push(RowSelector::skip(1));
                at = p + 1;
            }
            if at < count {
                selectors.push(RowSelector::select(
                    usize::try_from(count - at).unwrap_or(0),
                ));
            }
            RowGroupAccess::Selection(RowSelection::from(selectors))
        });
        start = end;
    }
    ParquetAccessPlan::new(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_positions_become_row_selections() {
        let deleted: BTreeSet<u64> = [0, 1, 5, 10, 11, 12].into_iter().collect();
        let plan = access_plan(&[6, 3, 4], &deleted);
        let groups = plan.inner();
        assert_eq!(groups.len(), 3);
        match &groups[0] {
            RowGroupAccess::Selection(s) => assert_eq!(s.row_count(), 3),
            other => panic!("{other:?}"),
        }
        assert!(matches!(groups[1], RowGroupAccess::Scan));
        match &groups[2] {
            RowGroupAccess::Selection(s) => assert_eq!(s.row_count(), 1),
            other => panic!("{other:?}"),
        }
        let all: BTreeSet<u64> = (0..3).collect();
        assert!(matches!(
            access_plan(&[3], &all).inner()[0],
            RowGroupAccess::Skip
        ));
    }
}
