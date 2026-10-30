//! Where the CA document lives (design §83 §6.3: "in the metastore").

use std::fmt;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

/// A versioned document written with compare-and-swap. The server's
/// implementation is the metastore pointer plus the bucket object that the
/// principal directory and the meter's documents use (`crates/loams`,
/// `MetaDirectoryStore`).
#[async_trait]
pub trait CaDocStore: Send + Sync + fmt::Debug {
    /// The stored version and document (`None`: never written).
    async fn load(&self) -> Result<Option<(u64, Vec<u8>)>, String>;
    /// Stores `doc` if the version is still `expected`: `Some(new
    /// version)`, or `None` when another writer moved it first.
    async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> Result<Option<u64>, String>;
}

/// A document kept in memory (tests).
#[derive(Debug, Default)]
pub struct MemoryDocStore(Mutex<Option<(u64, Vec<u8>)>>);

impl MemoryDocStore {
    /// The stored bytes, for tests that check what reaches storage.
    pub fn raw(&self) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|(_, d)| d.clone())
    }
}

#[async_trait]
impl CaDocStore for MemoryDocStore {
    async fn load(&self) -> Result<Option<(u64, Vec<u8>)>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }

    async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> Result<Option<u64>, String> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().map(|(v, _)| *v) != expected {
            return Ok(None);
        }
        let version = expected.map_or(1, |v| v + 1);
        *slot = Some((version, doc));
        Ok(Some(version))
    }
}
