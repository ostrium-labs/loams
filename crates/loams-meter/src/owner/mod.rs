//! Counter owners (design §63 §11, ME1 Task 12).
//!
//! - [`assign`]: which instance owns which `_system/_meter` partition, by
//!   fenced leases in the metastore; the lease epoch is the owner epoch.
//! - [`counters`]: one partition's counters, with dedup layer 2
//!   ([`recent`]) and offset buckets ([`buckets`]) for reconciliation.
//! - [`CounterOwners`]: what one instance runs: on gaining a partition,
//!   rebuild its counters from the House and the stream tail; then keep
//!   reading the partition.
//!
//! The stream and the House are reached through [`PartitionReader`] and
//! [`HouseTotals`], so this module has no dependency on either; the role
//! wires the stream's named consumer `meter-counters` and ME-house's period
//! query to them.

pub mod assign;
pub mod buckets;
pub mod counters;
pub mod recent;

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use loams_meter_core::{Amount, Event};

pub use assign::{
    Assigner, LeaseError, METER_PARTITIONS, MemLeases, MetaLeases, OwnershipChange,
    PartitionLeases, partition_of,
};
pub use buckets::{BUCKET_SPAN, OffsetBuckets, Tail};
pub use counters::{Applied, CounterKey, FeatureIndex, PartitionCounters, Periods};
pub use recent::RecentKeys;

/// The named consumer the counter owners read `_system/_meter` as.
pub const CONSUMER: &str = "meter-counters";

/// Errors from the stream or the House while owning a partition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OwnerError {
    #[error("stream: {0}")]
    Stream(String),
    #[error("house: {0}")]
    House(String),
}

/// Reads one partition of `_system/_meter` from an offset. Events that fail
/// the usage profile are the reader's to skip (and count); what it answers
/// is already validated.
#[async_trait]
pub trait PartitionReader: Send + Sync + fmt::Debug {
    /// Up to `max` events at offsets `>= from`, in offset order; empty at
    /// the end of the partition.
    async fn read(
        &self,
        partition: u32,
        from: u64,
        max: usize,
    ) -> Result<Vec<(u64, Event)>, OwnerError>;
}

/// The House side of a rebuild and of reconciliation (§12 steps 1–2).
#[async_trait]
pub trait HouseTotals: Send + Sync + fmt::Debug {
    /// The House pipe's committed offset for `partition` in the latest
    /// `_meter.events` snapshot, and that snapshot's id; `None` if the pipe
    /// has committed nothing for it.
    async fn committed(&self, partition: u32) -> Result<Option<(u64, i64)>, OwnerError>;

    /// Period-to-date usage per counter from `_meter.events FINAL`,
    /// restricted to `partition` and offsets `<= committed`, pinned to
    /// `snapshot`.
    async fn totals(
        &self,
        partition: u32,
        committed: u64,
        snapshot: i64,
    ) -> Result<Vec<(CounterKey, Amount)>, OwnerError>;
}

/// How many events one read asks for.
pub const READ_BATCH: usize = 1024;

/// The counter owners of one `meter` instance.
#[derive(Debug, Default)]
pub struct CounterOwners {
    partitions: BTreeMap<u32, PartitionCounters>,
}

impl CounterOwners {
    pub fn new() -> Self {
        Self::default()
    }

    /// The counters of `partition`, if owned.
    pub fn get(&self, partition: u32) -> Option<&PartitionCounters> {
        self.partitions.get(&partition)
    }

    pub fn get_mut(&mut self, partition: u32) -> Option<&mut PartitionCounters> {
        self.partitions.get_mut(&partition)
    }

    /// The partitions owned, with their epochs.
    pub fn owned(&self) -> impl Iterator<Item = (u32, u64)> + '_ {
        self.partitions.iter().map(|(p, c)| (*p, c.epoch()))
    }

    /// Acts on what the assigner reported: drop lost partitions; rebuild
    /// gained ones from the House (§11.5 step 2). The stream tail is read by
    /// [`CounterOwners::catch_up`].
    pub async fn on_change(
        &mut self,
        change: OwnershipChange,
        house: &dyn HouseTotals,
    ) -> Result<(), OwnerError> {
        match change {
            OwnershipChange::Lost { partition, epoch } => {
                if self
                    .partitions
                    .get(&partition)
                    .is_some_and(|c| c.epoch() == epoch)
                {
                    self.partitions.remove(&partition);
                }
                Ok(())
            }
            OwnershipChange::Gained { partition, epoch } => {
                let counters = rebuild(partition, epoch, house).await?;
                self.partitions.insert(partition, counters);
                Ok(())
            }
        }
    }

    /// Reads `partition` to its end, counting every event. Answers how many
    /// events were read.
    pub async fn catch_up(
        &mut self,
        partition: u32,
        reader: &dyn PartitionReader,
        index: &FeatureIndex,
        periods: &dyn Periods,
    ) -> Result<usize, OwnerError> {
        let Some(counters) = self.partitions.get_mut(&partition) else {
            return Ok(0);
        };
        let mut read = 0;
        loop {
            let batch = reader
                .read(partition, counters.next_offset(), READ_BATCH)
                .await?;
            if batch.is_empty() {
                return Ok(read);
            }
            read += batch.len();
            for (offset, event) in &batch {
                counters.apply(*offset, event, index, periods);
            }
        }
    }
}

/// Rebuilds `partition`'s counters from the House at its committed offset.
pub async fn rebuild(
    partition: u32,
    epoch: u64,
    house: &dyn HouseTotals,
) -> Result<PartitionCounters, OwnerError> {
    let committed = house.committed(partition).await?;
    let totals = match committed {
        Some((offset, snapshot)) => house.totals(partition, offset, snapshot).await?,
        None => Vec::new(),
    };
    Ok(PartitionCounters::rebuild(
        partition,
        epoch,
        totals,
        committed.map(|(o, _)| o),
    ))
}
