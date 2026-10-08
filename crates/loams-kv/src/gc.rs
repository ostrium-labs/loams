//! [`GcBarrier`]: GC held at or below a timestamp (design §45 §10's backup
//! export reads below the GC window through one).

use crate::{KvError, Ts, embedded};

/// A GC barrier from [`Store::barrier`](crate::Store::barrier): while it
/// lives (until its TTL passes or [`delete`](Self::delete)), GC keeps the
/// versions a snapshot at its timestamp reads, and the store's snapshots may
/// read there. Not `Clone`: [`delete`](Self::delete) consumes it, so it runs
/// once.
#[derive(Debug)]
pub struct GcBarrier {
    pub(crate) service_id: String,
    pub(crate) at: Ts,
    pub(crate) inner: Inner,
}

#[derive(Debug)]
pub(crate) enum Inner {
    Embedded(embedded::Handle),
    #[cfg(feature = "tikv")]
    Tikv(loams_tikv::GcBarrier),
}

impl GcBarrier {
    /// `loams/<name>`.
    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    /// The timestamp GC is held at or below.
    pub fn ts(&self) -> Ts {
        self.at
    }

    /// Removes the barrier.
    pub async fn delete(self) -> Result<(), KvError> {
        match self.inner {
            Inner::Embedded(h) => {
                h.remove_barrier(&self.service_id);
                Ok(())
            }
            #[cfg(feature = "tikv")]
            Inner::Tikv(b) => Ok(b.delete(&self.service_id).await?),
        }
    }
}
