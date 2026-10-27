//! Every byte of a lake table comes through the caller's `object_store`
//! (R26.2): the metadata file, manifests and deletes through an iceberg
//! `FileIO` whose storage is [`StoreStorage`], the data files through
//! DataFusion's Parquet source over the same store.
//!
//! [`Paths`] maps the paths the metadata names to the store's keys: a path
//! under the metadata's own `location` is read under the pin's (R26.3), and a
//! `scheme://bucket/key` URL reads `key` (the store is the location's bucket;
//! `file:///a/b` reads `a/b` from a store rooted at `/`).

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use datafusion::object_store::path::Path;
use datafusion::object_store::{ObjectStore, ObjectStoreExt};
use futures::stream::BoxStream;
use iceberg_engine::io::{
    FileIO, FileIOBuilder, FileMetadata, FileRead, FileWrite, InputFile, OutputFile, Storage,
    StorageConfig, StorageFactory,
};
use iceberg_engine::{Error, ErrorKind, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// How the paths a table's metadata names map to the store's keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The `location` the metadata file records.
    pub written: String,
    /// The pin's location, where the table is read.
    pub read: String,
}

impl Paths {
    /// `path` (as the metadata names it) under the pin's location.
    pub fn rebase(&self, path: &str) -> String {
        let written = self.written.trim_end_matches('/');
        match path.strip_prefix(written) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", self.read.trim_end_matches('/'))
            }
            _ => path.to_string(),
        }
    }

    /// The store key of `path` (as the metadata names it).
    pub fn key(&self, path: &str) -> Path {
        key(&self.rebase(path))
    }
}

/// The store key of a location: what follows `scheme://authority/`, or the
/// path itself without its leading `/`.
pub fn key(location: &str) -> Path {
    let rest = match location.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, key)| key),
        None => location.strip_prefix("file:").unwrap_or(location),
    };
    Path::from(rest.trim_start_matches('/'))
}

fn io_error(path: &str, e: impl fmt::Display) -> Error {
    Error::new(ErrorKind::Unexpected, format!("reading {path}: {e}"))
}

fn read_only(path: &str) -> Error {
    Error::new(
        ErrorKind::FeatureUnsupported,
        format!("the Loams House engine reads lake tables only ({path})"),
    )
}

/// An iceberg `Storage` over an `object_store`, read-only. It lives in one
/// process: it is never serialised (a worker builds its own, Task 39).
#[derive(Clone)]
pub struct StoreStorage {
    store: Arc<dyn ObjectStore>,
    paths: Arc<Paths>,
}

impl fmt::Debug for StoreStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreStorage")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl StoreStorage {
    /// Storage reading `paths` from `store`.
    pub fn new(store: Arc<dyn ObjectStore>, paths: Paths) -> Self {
        Self {
            store,
            paths: Arc::new(paths),
        }
    }

    /// A `FileIO` over this storage.
    pub fn file_io(&self) -> FileIO {
        FileIOBuilder::new(Arc::new(StoreFactory(self.clone()))).build()
    }

    /// The bytes of `path` in `range`.
    pub async fn read_range(&self, path: &str, range: Range<u64>) -> Result<Bytes> {
        self.store
            .get_range(&self.paths.key(path), range)
            .await
            .map_err(|e| io_error(path, e))
    }
}

impl Serialize for StoreStorage {
    fn serialize<S: Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "the engine's lake storage is not serialisable",
        ))
    }
}

impl<'de> Deserialize<'de> for StoreStorage {
    fn deserialize<D: Deserializer<'de>>(_: D) -> std::result::Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "the engine's lake storage is not serialisable",
        ))
    }
}

#[async_trait]
#[typetag::serde]
impl Storage for StoreStorage {
    async fn exists(&self, path: &str) -> Result<bool> {
        match self.store.head(&self.paths.key(path)).await {
            Ok(_) => Ok(true),
            Err(datafusion::object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(io_error(path, e)),
        }
    }

    async fn metadata(&self, path: &str) -> Result<FileMetadata> {
        let meta = self
            .store
            .head(&self.paths.key(path))
            .await
            .map_err(|e| io_error(path, e))?;
        Ok(FileMetadata { size: meta.size })
    }

    async fn read(&self, path: &str) -> Result<Bytes> {
        self.store
            .get(&self.paths.key(path))
            .await
            .map_err(|e| io_error(path, e))?
            .bytes()
            .await
            .map_err(|e| io_error(path, e))
    }

    async fn reader(&self, path: &str) -> Result<Box<dyn FileRead>> {
        Ok(Box::new(Reader {
            storage: self.clone(),
            path: path.to_string(),
        }))
    }

    async fn write(&self, path: &str, _: Bytes) -> Result<()> {
        Err(read_only(path))
    }

    async fn writer(&self, path: &str) -> Result<Box<dyn FileWrite>> {
        Err(read_only(path))
    }

    async fn delete(&self, path: &str) -> Result<()> {
        Err(read_only(path))
    }

    async fn delete_prefix(&self, path: &str) -> Result<()> {
        Err(read_only(path))
    }

    async fn delete_stream(&self, _: BoxStream<'static, String>) -> Result<()> {
        Err(read_only("a stream of paths"))
    }

    fn new_input(&self, path: &str) -> Result<InputFile> {
        Ok(InputFile::new(Arc::new(self.clone()), path.to_string()))
    }

    fn new_output(&self, path: &str) -> Result<OutputFile> {
        Err(read_only(path))
    }
}

struct Reader {
    storage: StoreStorage,
    path: String,
}

#[async_trait]
impl FileRead for Reader {
    async fn read(&self, range: Range<u64>) -> Result<Bytes> {
        self.storage.read_range(&self.path, range).await
    }
}

/// The factory a `FileIO` builds [`StoreStorage`] with.
#[derive(Debug, Clone)]
struct StoreFactory(StoreStorage);

impl Serialize for StoreFactory {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for StoreFactory {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        StoreStorage::deserialize(d).map(Self)
    }
}

#[typetag::serde]
impl StorageFactory for StoreFactory {
    fn build(&self, _: &StorageConfig) -> Result<Arc<dyn Storage>> {
        Ok(Arc::new(self.0.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_rebase_under_the_pin() {
        let paths = Paths {
            written: "s3://warehouse/db/t".into(),
            read: "file:///fixtures/t/".into(),
        };
        assert_eq!(
            paths.rebase("s3://warehouse/db/t/data/a.parquet"),
            "file:///fixtures/t/data/a.parquet"
        );
        assert_eq!(
            paths.key("s3://warehouse/db/t/data/a.parquet").as_ref(),
            "fixtures/t/data/a.parquet"
        );
        // Not under the location: as written.
        assert_eq!(
            paths.rebase("s3://warehouse/db/tt/x"),
            "s3://warehouse/db/tt/x"
        );
        assert_eq!(key("s3://bucket/a/b").as_ref(), "a/b");
        assert_eq!(key("/a/b").as_ref(), "a/b");
        assert_eq!(key("file:/a/b").as_ref(), "a/b");
    }
}
