//! [`Txn`] and [`Snap`]: reads and writes under a store's root, on either
//! backend (LV1 plan Ruling 1: enums with inherent async methods).

use crate::{Ts, TxnError, embedded};

/// A key and its value, the key relative to the store's root.
pub type Pair = (Vec<u8>, Vec<u8>);

/// The largest value [`Txn::put`] and [`Txn::insert`] accept (2 MiB).
pub const MAX_VALUE_BYTES: usize = 2 * 1024 * 1024;

/// A transaction of [`Store::run`](crate::Store::run). Every key is relative
/// to the store's root.
// One per attempt or read, rarely moved.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Txn {
    Embedded(embedded::Txn),
    #[cfg(feature = "tikv")]
    Tikv(loams_tikv::Txn),
}

/// A read-only view at one timestamp, from
/// [`Store::snapshot`](crate::Store::snapshot). Every key is relative to the
/// store's root.
// One per attempt or read, rarely moved.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Snap {
    Embedded(embedded::Snap),
    #[cfg(feature = "tikv")]
    Tikv(loams_tikv::Snap),
}

/// Calls `$call` on the backend's value bound to `$t`, mapping a TiKV
/// [`TxnError`](loams_tikv::TxnError) to ours.
macro_rules! dispatch {
    ($self:expr, $t:ident => $call:expr) => {
        match $self {
            Self::Embedded($t) => $call,
            #[cfg(feature = "tikv")]
            Self::Tikv($t) => $call.map_err(TxnError::from),
        }
    };
}

impl Txn {
    /// The transaction's start timestamp: every read sees the data committed
    /// before it.
    pub fn start_ts(&self) -> Ts {
        match self {
            Txn::Embedded(e) => e.start_ts(),
            #[cfg(feature = "tikv")]
            Txn::Tikv(t) => Ts::from(t.start_ts()),
        }
    }

    /// Which attempt of [`Store::run`](crate::Store::run) this is, from 1.
    pub fn attempt(&self) -> u32 {
        match self {
            Txn::Embedded(e) => e.attempt(),
            #[cfg(feature = "tikv")]
            Txn::Tikv(t) => t.attempt(),
        }
    }

    /// Reads `key` at the start timestamp.
    pub async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        dispatch!(self, t => t.get(key).await)
    }

    /// Reads `keys` at the start timestamp; absent keys are left out. The
    /// result is sorted by key.
    pub async fn batch_get<K: AsRef<[u8]>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Vec<Pair>, TxnError> {
        dispatch!(self, t => t.batch_get(keys).await)
    }

    /// Up to `limit` pairs with keys in `start..end` in key order (`end`
    /// `None`: to the end of the root).
    pub async fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        dispatch!(self, t => t.scan(start, end, limit).await)
    }

    /// Like [`scan`](Self::scan), in descending key order.
    pub async fn scan_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        dispatch!(self, t => t.scan_reverse(start, end, limit).await)
    }

    /// Writes `value` at `key`. Refuses a value over 2 MiB
    /// ([`MAX_VALUE_BYTES`]) with `TxnError::Fatal("value over 2 MiB")`.
    pub async fn put(&mut self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<(), TxnError> {
        dispatch!(self, t => t.put(key, value).await)
    }

    /// Writes `value` at `key` if the key is absent; the transaction fails
    /// with [`TxnError::AlreadyExists`] if it is not. The 2 MiB bound applies.
    pub async fn insert(&mut self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<(), TxnError> {
        dispatch!(self, t => t.insert(key, value).await)
    }

    /// Deletes `key`.
    pub async fn delete(&mut self, key: &[u8]) -> Result<(), TxnError> {
        dispatch!(self, t => t.delete(key).await)
    }

    /// Locks `keys` without writing them: a concurrent write to one of them
    /// is a write-write conflict (design §20 §5.2's read promotion).
    pub async fn lock_keys<K: AsRef<[u8]>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<(), TxnError> {
        dispatch!(self, t => t.lock_keys(keys).await)
    }
}

impl Snap {
    /// The snapshot's timestamp.
    pub fn ts(&self) -> Ts {
        match self {
            Snap::Embedded(e) => e.ts(),
            #[cfg(feature = "tikv")]
            Snap::Tikv(s) => Ts::from(s.ts().clone()),
        }
    }

    /// Reads `key`.
    pub async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        dispatch!(self, s => s.get(key).await)
    }

    /// Reads `keys`; absent keys are left out. Sorted by key.
    pub async fn batch_get<K: AsRef<[u8]>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Vec<Pair>, TxnError> {
        dispatch!(self, s => s.batch_get(keys).await)
    }

    /// Up to `limit` pairs with keys in `start..end` in key order (`end`
    /// `None`: to the end of the root).
    pub async fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        dispatch!(self, s => s.scan(start, end, limit).await)
    }

    /// Like [`scan`](Self::scan), in descending key order.
    pub async fn scan_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        dispatch!(self, s => s.scan_reverse(start, end, limit).await)
    }
}
