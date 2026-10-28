//! The export ledger: what each exporter planned to send and whether the
//! provider accepted it, per window (platform PD7, §63 §16).
//!
//! An exporter writes the entry with `sent = false` *before* it calls the
//! provider and flips it after the provider accepts. A retry (the same run or
//! the next, after a crash) finds the unsent entry and resends its
//! identifier unchanged, so a provider that dedupes by identifier never
//! counts it twice. In the `meter` role the ledger is the `ControlStore`;
//! [`MemLedger`] is for tests and enforcement-only stacks that export nothing.

use std::collections::BTreeMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{ExportError, Quantity};

/// One window of one exporter: `(exporter, meter, subject, window_start)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LedgerKey {
    /// `stripe`, `lago`, …
    pub exporter: String,
    /// The meter slug (Stripe: the meter's `event_name`).
    pub meter: String,
    pub subject: String,
    /// The window's start, Unix seconds UTC.
    pub window_start: i64,
}

/// What was (or is being) sent for a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// The provider's idempotency key for this send.
    pub identifier: String,
    /// The value sent (Stripe: the hour's overage; Lago: the delta).
    pub value: Quantity,
    /// The sum of every value accepted for this window so far (Lago's
    /// additive events); for overwrite providers it equals `value`.
    pub total: Quantity,
    /// The input revision this send came from (the close run).
    pub revision: u64,
    /// A counter that makes `identifier` unique when the same revision is
    /// sent again with another value (D2211).
    pub seq: u32,
    /// Whether the provider accepted it.
    pub sent: bool,
}

/// Where exporters keep their ledger. Implementations must make `put`
/// durable before they return.
#[async_trait]
pub trait ExportLedger: Send + Sync + std::fmt::Debug {
    async fn get(&self, key: &LedgerKey) -> Result<Option<LedgerEntry>, ExportError>;
    async fn put(&self, key: &LedgerKey, entry: &LedgerEntry) -> Result<(), ExportError>;
}

/// An in-memory ledger.
#[derive(Debug, Default)]
pub struct MemLedger {
    entries: Mutex<BTreeMap<LedgerKey, LedgerEntry>>,
}

impl MemLedger {
    pub fn new() -> MemLedger {
        MemLedger::default()
    }

    /// Every entry, in key order.
    pub async fn entries(&self) -> Vec<(LedgerKey, LedgerEntry)> {
        self.entries
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[async_trait]
impl ExportLedger for MemLedger {
    async fn get(&self, key: &LedgerKey) -> Result<Option<LedgerEntry>, ExportError> {
        Ok(self.entries.lock().await.get(key).cloned())
    }

    async fn put(&self, key: &LedgerKey, entry: &LedgerEntry) -> Result<(), ExportError> {
        self.entries.lock().await.insert(key.clone(), entry.clone());
        Ok(())
    }
}

/// Where an exporter of raw events (OpenMeter) keeps its cursor: the next
/// stream offset to export, per `_system/_meter` partition. The provider's
/// own dedup window is configuration (OpenMeter's is off by default), so the
/// exporter does not rely on it (§63 §16).
#[async_trait]
pub trait ExportCursors: Send + Sync + std::fmt::Debug {
    async fn get(&self, exporter: &str, partition: u32) -> Result<Option<u64>, ExportError>;
    /// Durable before it returns.
    async fn put(
        &self,
        exporter: &str,
        partition: u32,
        next_offset: u64,
    ) -> Result<(), ExportError>;
}

/// In-memory cursors.
#[derive(Debug, Default)]
pub struct MemCursors {
    cursors: Mutex<BTreeMap<(String, u32), u64>>,
}

impl MemCursors {
    pub fn new() -> MemCursors {
        MemCursors::default()
    }
}

#[async_trait]
impl ExportCursors for MemCursors {
    async fn get(&self, exporter: &str, partition: u32) -> Result<Option<u64>, ExportError> {
        Ok(self
            .cursors
            .lock()
            .await
            .get(&(exporter.to_owned(), partition))
            .copied())
    }

    async fn put(
        &self,
        exporter: &str,
        partition: u32,
        next_offset: u64,
    ) -> Result<(), ExportError> {
        self.cursors
            .lock()
            .await
            .insert((exporter.to_owned(), partition), next_offset);
        Ok(())
    }
}
