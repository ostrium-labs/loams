//! Where instance records live. Metadata belongs in Loams Postgres (design 81); this crate
//! ships the trait and an in-memory store, and the Postgres store follows.

use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use crate::{EngineError, Instance, InstanceId};

#[async_trait]
pub trait InstanceStore: Send + Sync + 'static {
    async fn get(&self, id: &InstanceId) -> Result<Option<Instance>, EngineError>;
    /// A namespace's instances, oldest first.
    async fn list(&self, namespace: &str) -> Result<Vec<Instance>, EngineError>;
    /// Every instance (the metering ticker and the reconciler walk this).
    async fn list_all(&self) -> Result<Vec<Instance>, EngineError>;
    /// Inserts or replaces by id.
    async fn put(&self, inst: &Instance) -> Result<(), EngineError>;
    async fn remove(&self, id: &InstanceId) -> Result<(), EngineError>;
}

/// An in-process store, for tests, the console mock and single-node development.
#[derive(Debug, Default)]
pub struct MemoryStore {
    rows: RwLock<BTreeMap<InstanceId, Instance>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn poisoned() -> EngineError {
    EngineError::Internal("instance store lock poisoned".into())
}

fn oldest_first(mut v: Vec<Instance>) -> Vec<Instance> {
    v.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    v
}

#[async_trait]
impl InstanceStore for MemoryStore {
    async fn get(&self, id: &InstanceId) -> Result<Option<Instance>, EngineError> {
        Ok(self.rows.read().map_err(|_| poisoned())?.get(id).cloned())
    }

    async fn list(&self, namespace: &str) -> Result<Vec<Instance>, EngineError> {
        let rows = self.rows.read().map_err(|_| poisoned())?;
        Ok(oldest_first(
            rows.values()
                .filter(|i| i.namespace == namespace)
                .cloned()
                .collect(),
        ))
    }

    async fn list_all(&self) -> Result<Vec<Instance>, EngineError> {
        let rows = self.rows.read().map_err(|_| poisoned())?;
        Ok(oldest_first(rows.values().cloned().collect()))
    }

    async fn put(&self, inst: &Instance) -> Result<(), EngineError> {
        self.rows
            .write()
            .map_err(|_| poisoned())?
            .insert(inst.id.clone(), inst.clone());
        Ok(())
    }

    async fn remove(&self, id: &InstanceId) -> Result<(), EngineError> {
        self.rows.write().map_err(|_| poisoned())?.remove(id);
        Ok(())
    }
}
