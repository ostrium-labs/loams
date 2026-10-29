//! Loams upgrades, the server side (design §54, UP1).
//!
//! - [`formats`]: the format registry, re-exported from
//!   [`loams_common::formats`], its only definition (D924).
//! - [`stack`]: the stack record, `_system/upgrade/stack` (§54 §4.2), and the
//!   check that this binary reads every format version it names.
//! - [`ledger`]: the migration ledger, the stack record's home and the
//!   migration lease in the `_system` namespace (§54 §6.2).
//!
//! - [`migration`] and [`runner`]: the migration contract and the runner
//!   (plan, run with resume, abandon), checked against [`path`], the release
//!   train of `release/upgrade-path.toml`.
//!
//! - [`startup`]: the startup gate every Loams process runs before it
//!   serves (§54 §4.3).
//!
//! - [`adopt`] and [`migrations`]: `0000-adopt-preview-2`, which recognises
//!   an unstamped preview.2 stack by fingerprint (§54 §5.3).
//!
//! - [`cx`]: the store contexts a migration works through, each with an
//!   expand-then-switch helper (§54 §6.3).
//!
//! - [`verify`]: the store probes of `loams migrate verify` (§54 §10.4).
//!
//! - [`snapshot`]: snapshots and restores of the Loams-owned stores, and
//!   their GC roots (§54 §7, §8).
//!
//! - [`system`]: the system objects every stack has (Loams Meter's
//!   `_system/_meter` stream, §63 §7.1), made by every `migrate run`.

/// Evaluates a named failpoint of the crash matrix. With the `failpoints`
/// feature a configured failpoint stops the caller as a crash would (it
/// returns a [`runner::CRASH`] error and writes nothing more); without the
/// feature it expands to nothing.
macro_rules! failpoint {
    ($name:literal) => {
        #[cfg(feature = "failpoints")]
        fail::fail_point!($name, |_| Err($crate::migration::MigrationError::new(
            $crate::runner::CRASH,
            $name
        )
        .into()));
    };
}
pub(crate) use failpoint;

pub mod adopt;
pub mod cx;
pub mod formats;
pub mod ledger;
pub mod migration;
pub mod migrations;
pub mod path;
pub mod runner;
pub mod snapshot;
pub mod stack;
pub mod startup;
pub mod system;
pub mod verify;

pub use ledger::{Class, Ledger, LedgerEntry, LedgerError, LedgerState, MigrationLease};
pub use migration::{Cursor, Cx, Estimate, Migration, MigrationError, Step};
pub use path::UpgradePath;
pub use runner::{Plan, Report, RunError, RunOptions, Runner};
pub use stack::{StackRecord, StoredVersions, Unreadable};
pub use startup::{StartupVerdict, check_startup};
