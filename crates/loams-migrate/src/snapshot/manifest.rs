//! `snapshot.json` (design §54 §7.3): what a snapshot holds, written last.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::files::{self, hex};
use super::{SnapshotError, io};
use crate::ledger::LedgerEntry;
use crate::stack::StackRecord;

/// The manifest's file name.
pub const MANIFEST_FILE: &str = "snapshot.json";

/// One store's copy in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    /// `bucket`, `raft`, `durable`, `durable_local`, `house`, `pg_control`, `agentd`, `tikv`,
    /// `postgres`.
    pub store: String,
    /// `copy`, `sqlite_backup`, `br`, `cold`, `branch`.
    pub method: String,
    /// Relative to the snapshot directory.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    /// The version of the component the copy was taken from (TiKV's, for
    /// `tikv`): a restore goes into that version only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `snapshot.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub id: String,
    pub created_at_ms: u64,
    /// The release the data was at (its stack record), if stamped.
    pub release: Option<String>,
    pub target_release: String,
    pub components: Vec<Component>,
    /// A copy of the ledger when the snapshot was taken.
    pub ledger: Vec<LedgerEntry>,
    pub stack_record: Option<StackRecord>,
}

/// What `snapshots list` shows: paths, sizes and digests, never a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotListing {
    pub id: String,
    pub created_at_ms: u64,
    pub release: Option<String>,
    pub target_release: String,
    pub complete: bool,
    pub bytes: u64,
    pub components: Vec<Component>,
}

impl SnapshotManifest {
    /// Writes `snapshot.json` (0600) into `dir`, through a temporary file;
    /// returns its sha256.
    pub(crate) fn write(&self, dir: &Path) -> Result<String, SnapshotError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| io("snapshot.json", e))?;
        let tmp = dir.join(format!("{MANIFEST_FILE}.tmp"));
        files::write_private(&tmp, &bytes)?;
        std::fs::rename(&tmp, dir.join(MANIFEST_FILE)).map_err(|e| io(dir.display(), e))?;
        Ok(hex(&Sha256::digest(&bytes)))
    }

    /// Reads `<dir>/snapshot.json`; [`SnapshotError::Incomplete`] without it.
    pub fn read(dir: &Path) -> Result<Self, SnapshotError> {
        if !dir.exists() {
            return Err(SnapshotError::NotFound(dir.display().to_string()));
        }
        let path = dir.join(MANIFEST_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SnapshotError::Incomplete(dir.to_path_buf()));
            }
            Err(e) => return Err(io(path.display(), e)),
        };
        serde_json::from_slice(&bytes)
            .map_err(|e| SnapshotError::Corrupt(format!("{}: {e}", path.display())))
    }

    /// Checks every file component against its digest.
    pub fn check(&self, dir: &Path) -> Result<(), SnapshotError> {
        for component in &self.components {
            if !matches!(
                component.method.as_str(),
                "copy" | "sqlite_backup" | "branch" | "br" | "cold"
            ) {
                continue;
            }
            let (_, sha256) = files::digest(&dir.join(&component.path))?;
            if sha256 != component.sha256 {
                return Err(SnapshotError::Corrupt(format!(
                    "{} does not match its digest",
                    component.path
                )));
            }
        }
        Ok(())
    }
}

/// Every snapshot directory under `dir`, by id (ULIDs sort by time).
pub(crate) fn list(dir: &Path) -> Result<Vec<SnapshotListing>, SnapshotError> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io(dir.display(), e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| io(dir.display(), e))?;
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let listing = match SnapshotManifest::read(&entry.path()) {
            Ok(m) => SnapshotListing {
                bytes: m.components.iter().map(|c| c.bytes).sum(),
                id: m.id,
                created_at_ms: m.created_at_ms,
                release: m.release,
                target_release: m.target_release,
                complete: true,
                components: m.components,
            },
            Err(_) => SnapshotListing {
                created_at_ms: id.parse::<ulid::Ulid>().map_or(0, |u| u.timestamp_ms()),
                id,
                release: None,
                target_release: String::new(),
                complete: false,
                bytes: files::size_of(&entry.path()),
                components: Vec::new(),
            },
        };
        out.push(listing);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
