//! The embedded backend: MVCC on redb (LV1 plan Task 21, Ruling 4).
//!
//! A stub in Task 20: its types are uninhabited, so no value of
//! [`Store::Embedded`](crate::Store::Embedded), `Txn::Embedded` or
//! `Snap::Embedded` exists, and [`Store::open`](crate::Store::open) refuses
//! an embedded config with [`KvError::Unsupported`](crate::KvError).

use std::convert::Infallible;

/// The text of the stub's refusal.
pub(crate) const UNSUPPORTED: &str = "embedded backend arrives in Task 21";

/// A handle on an embedded store (none exists until Task 21).
#[derive(Debug, Clone)]
pub struct Handle {
    never: Infallible,
}

/// An embedded transaction (none exists until Task 21).
#[derive(Debug)]
pub struct Txn {
    never: Infallible,
}

/// An embedded read-only view (none exists until Task 21).
#[derive(Debug)]
pub struct Snap {
    never: Infallible,
}

impl Handle {
    pub(crate) fn absurd(&self) -> ! {
        let never = self.never;
        match never {}
    }
}

impl Txn {
    pub(crate) fn absurd(&self) -> ! {
        let never = self.never;
        match never {}
    }
}

impl Snap {
    pub(crate) fn absurd(&self) -> ! {
        let never = self.never;
        match never {}
    }
}
