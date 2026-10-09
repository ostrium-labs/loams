//! The seam between `pg-control` and Neon's components (PG2 Task 5).
//!
//! [`NeonApi`] is what the API service reads from the pageserver and
//! `loams-wal`; `loams-postgres` (formerly `loams-neon`: `NeonClient` and
//! `WalClient`) implements it, and tests use a fake. Task 7's reconcilers
//! extend it with the calls that change storage (attach, create and delete
//! timelines). Ids are Neon's raw 16-byte tenant and timeline ids
//! ([`crate::ids::tenant_id`], [`crate::ids::timeline_id`]).

use std::fmt;
use std::future::Future;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A Postgres LSN. Its text form is Postgres's `X/Y`: the high and low 32
/// bits in upper-case hex, without padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lsn(pub u64);

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:X}/{:X}", self.0 >> 32, self.0 & 0xffff_ffff)
    }
}

/// An LSN text that is not `X/Y`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("an LSN is \"X/Y\" in hex, at most 8 digits each: {0:?}")]
pub struct LsnError(pub String);

impl FromStr for Lsn {
    type Err = LsnError;
    fn from_str(s: &str) -> Result<Self, LsnError> {
        let bad = || LsnError(s.to_string());
        let (hi, lo) = s.split_once('/').ok_or_else(bad)?;
        let half = |part: &str| {
            if part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
            u64::from_str_radix(part, 16).map_err(|_| bad())
        };
        Ok(Lsn(half(hi)? << 32 | half(lo)?))
    }
}

/// A branch's WAL heads, from `loams-wal` (`WalClient::timeline_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalHeads {
    pub commit_lsn: Lsn,
    pub flush_lsn: Lsn,
    pub remote_consistent_lsn: Lsn,
    pub backup_lsn: Lsn,
}

/// What the pageserver says of a timeline (`TimelineInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineView {
    /// The head.
    pub last_record_lsn: Lsn,
    /// The oldest LSN a branch or a read may start at: below it, the
    /// history is gone (`lsn_out_of_retention`).
    pub min_readable_lsn: Lsn,
    pub logical_size_bytes: u64,
}

/// The LSN of a time (`get_lsn_by_timestamp`'s `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LsnAtTime {
    /// The time falls in the timeline's WAL: the LSN of the last commit at
    /// or before it.
    Present(Lsn),
    /// After the last commit: the head.
    Future(Lsn),
    /// Before the oldest WAL the pageserver keeps.
    Past,
    /// The timeline has no commit to judge the time by.
    NoData,
}

/// A failed call to a Neon component, already mapped to a registered
/// reason (`loams-postgres`'s `NeonError::reason()`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct NeonApiError {
    /// `storage_unavailable`, `not_found`, `internal`, ...
    pub reason: String,
    /// `pageserver`, `storage_controller`, `loams_wal`, `compute_ctl`.
    pub component: Option<String>,
    /// Not for the caller: it can name internal hosts.
    pub message: String,
}

/// What `pg-control`'s API service reads from Neon's components.
pub trait NeonApi: Send + Sync + 'static {
    /// The pageserver's view of a timeline.
    fn timeline(
        &self,
        tenant: [u8; 16],
        timeline: [u8; 16],
    ) -> impl Future<Output = Result<TimelineView, NeonApiError>> + Send;

    /// The LSN of the time `at_ms` (ms since the Unix epoch) on a timeline.
    fn lsn_by_timestamp(
        &self,
        tenant: [u8; 16],
        timeline: [u8; 16],
        at_ms: u64,
    ) -> impl Future<Output = Result<LsnAtTime, NeonApiError>> + Send;

    /// The timeline's WAL heads, from `loams-wal`.
    fn wal_heads(
        &self,
        tenant: [u8; 16],
        timeline: [u8; 16],
    ) -> impl Future<Output = Result<WalHeads, NeonApiError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsn_text_round_trips() {
        for (n, s) in [
            (0, "0/0"),
            (0x0169_AD58, "0/169AD58"),
            (0x1_0000_0000, "1/0"),
            (u64::MAX, "FFFFFFFF/FFFFFFFF"),
        ] {
            assert_eq!(Lsn(n).to_string(), s);
            assert_eq!(s.parse::<Lsn>(), Ok(Lsn(n)));
        }
        assert_eq!("0/169ad58".parse::<Lsn>(), Ok(Lsn(0x0169_AD58)));
        for bad in [
            "",
            "0",
            "/0",
            "0/",
            "x/0",
            "123456789/0",
            "0/-1",
            "0/1/2",
            "+1/0",
        ] {
            assert!(bad.parse::<Lsn>().is_err(), "{bad}");
        }
    }
}
