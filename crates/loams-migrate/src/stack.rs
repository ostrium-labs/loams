//! The stack record (design §54 §4.2, D928): the release the data was last
//! migrated to and, per format, the version enabled for writing. It lives at
//! `_system/upgrade/stack` in the metastore (UP1 Task 2).
//!
//! Encoding: a one-byte value version ([`STACK_RECORD_VERSION`], `0x01`)
//! followed by the record's postcard encoding. The byte is pinned by a golden
//! test; a change to the record's shape bumps it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::formats::FormatSpec;

/// The value version in front of an encoded [`StackRecord`].
pub const STACK_RECORD_VERSION: u8 = 0x01;

/// What release a stack's data is at, and which format versions it writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackRecord {
    /// The release the data was last migrated to, e.g. `0.1.0`.
    pub release: String,
    /// Registry id → the version enabled for writing. A format at version 1
    /// is left out ("absent means 1", D924), which keeps the record within a
    /// metastore pointer value (1024 bytes) as the registry grows.
    pub formats: BTreeMap<String, u16>,
    pub migrated_at: SystemTime,
    /// The digest of the image that wrote the record, when known.
    pub image_digest: Option<String>,
}

/// Why a stack record cannot be decoded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("stack record is empty")]
    Empty,
    #[error("stack record value version {0} is not {STACK_RECORD_VERSION}")]
    Version(u8),
    #[error("stack record does not decode: {0}")]
    Body(String),
}

impl StackRecord {
    /// A record at `release` enabling every format of `registry` at its
    /// current version (what a new stack, or a finished `migrate`, writes).
    /// Formats at version 1 are left out.
    pub fn current(
        release: impl Into<String>,
        registry: &[FormatSpec],
        migrated_at: SystemTime,
        image_digest: Option<String>,
    ) -> Self {
        StackRecord {
            release: release.into(),
            formats: registry
                .iter()
                .filter(|s| s.current != 1)
                .map(|s| (s.id.as_str().to_string(), s.current))
                .collect(),
            migrated_at,
            image_digest,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_extend(self, vec![STACK_RECORD_VERSION])
            .expect("a stack record always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        match bytes.split_first() {
            None => Err(DecodeError::Empty),
            Some((&STACK_RECORD_VERSION, body)) => {
                postcard::from_bytes(body).map_err(|e| DecodeError::Body(e.to_string()))
            }
            Some((&v, _)) => Err(DecodeError::Version(v)),
        }
    }

    /// The format versions the record names.
    pub fn stored(&self) -> StoredVersions<'_> {
        StoredVersions(&self.formats)
    }
}

/// The per-format versions a stack record names.
#[derive(Clone, Copy, Debug)]
pub struct StoredVersions<'a>(pub &'a BTreeMap<String, u16>);

/// A stored format version this binary does not read: outside the
/// registry's `min_read..=current`, or a format the registry does not know
/// (written by a newer release).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("format_unreadable: {id} version {found} is outside the readable range {}", range(*.min_read, *.current))]
pub struct Unreadable {
    pub id: String,
    pub found: u16,
    /// `None` for a format this binary does not know.
    pub min_read: Option<u16>,
    pub current: Option<u16>,
}

fn range(min_read: Option<u16>, current: Option<u16>) -> String {
    match (min_read, current) {
        (Some(lo), Some(hi)) => format!("{lo}..={hi}"),
        _ => "(unknown to this release)".to_string(),
    }
}

impl Unreadable {
    /// Newer data than this binary reads: the fix is `loams upgrade`, not a
    /// snapshot restore.
    pub fn is_newer(&self) -> bool {
        self.current.is_none_or(|current| self.found > current)
    }
}

impl StoredVersions<'_> {
    /// `Ok` when every stored version is in its format's readable range; the
    /// first one that is not, in id order, otherwise. A registry format the
    /// record does not name is at version 1 ("absent means 1").
    pub fn readable_by(&self, registry: &[FormatSpec]) -> Result<(), Unreadable> {
        for (id, &found) in self.0 {
            match registry.iter().find(|s| s.id.as_str() == id) {
                Some(spec) if spec.reads(found) => {}
                Some(spec) => {
                    return Err(Unreadable {
                        id: id.clone(),
                        found,
                        min_read: Some(spec.min_read),
                        current: Some(spec.current),
                    });
                }
                None => {
                    return Err(Unreadable {
                        id: id.clone(),
                        found,
                        min_read: None,
                        current: None,
                    });
                }
            }
        }
        for spec in registry {
            let id = spec.id.as_str();
            if !self.0.contains_key(id) && !spec.reads(1) {
                return Err(Unreadable {
                    id: id.to_string(),
                    found: 1,
                    min_read: Some(spec.min_read),
                    current: Some(spec.current),
                });
            }
        }
        Ok(())
    }
}
