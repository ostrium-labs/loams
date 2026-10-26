//! Walrus ids, sha256 digests, unverified bytes and store results.

use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::error::WalrusError;

/// A Walrus blob id: 32 bytes, written as Walrus prints it (URL-safe base64
/// without padding, 43 characters).
///
/// A blob id is derived from the blob's erasure-coded content, not from a
/// plain hash of its bytes, so the fast path cannot recompute it; reads are
/// verified against a [`ContentDigest`] taken from a signed record.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(String);

/// A quilt is stored as one blob; its id is that blob's id.
pub type QuiltId = BlobId;

impl BlobId {
    /// Parses a blob id as Walrus prints it.
    pub fn parse(s: &str) -> Result<Self, WalrusError> {
        match URL_SAFE_NO_PAD.decode(s) {
            Ok(raw) if raw.len() == 32 => Ok(Self(s.to_string())),
            Ok(raw) => Err(invalid(s, format!("expected 32 bytes, got {}", raw.len()))),
            Err(e) => Err(invalid(
                s,
                format!("not URL-safe base64 without padding: {e}"),
            )),
        }
    }

    /// The blob id from its 32 raw bytes.
    pub fn from_bytes(raw: [u8; 32]) -> Self {
        Self(URL_SAFE_NO_PAD.encode(raw))
    }

    /// The id as Walrus prints it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobId({})", self.0)
    }
}

impl FromStr for BlobId {
    type Err = WalrusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for BlobId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlobId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A quilt patch id: the opaque string Walrus returns for one file of a
/// quilt (URL-safe base64, longer than a blob id).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct QuiltPatchId(String);

impl QuiltPatchId {
    /// Accepts a non-empty URL-safe base64 string, as Walrus prints it.
    pub fn parse(s: &str) -> Result<Self, WalrusError> {
        let ok = !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(s, "expected URL-safe base64".into()))
        }
    }

    /// The id as Walrus prints it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for QuiltPatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for QuiltPatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QuiltPatchId({})", self.0)
    }
}

impl<'de> Deserialize<'de> for QuiltPatchId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn invalid(value: &str, reason: String) -> WalrusError {
    WalrusError::InvalidId {
        value: value.to_string(),
        reason,
    }
}

/// The sha256 of a blob's bytes: what every verified read is checked
/// against, and the [`VerifiedDiskCache`](crate::VerifiedDiskCache) key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    /// The digest from its 32 raw bytes.
    pub fn from_bytes(raw: [u8; 32]) -> Self {
        Self(raw)
    }

    /// The sha256 of `data`.
    pub fn of(data: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(data);
        Self::from_hasher(h)
    }

    /// The digest a finished hasher holds.
    pub(crate) fn from_hasher(h: Sha256) -> Self {
        let out = h.finalize();
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&out);
        Self(raw)
    }

    /// Parses 64 hex digits (either case).
    pub fn parse_hex(s: &str) -> Result<Self, WalrusError> {
        let mut raw = [0u8; 32];
        hex::decode_to_slice(s, &mut raw)
            .map_err(|e| invalid(s, format!("expected 64 hex digits: {e}")))?;
        Ok(Self(raw))
    }

    /// The 32 raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 64 lowercase hex digits.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl Serialize for ContentDigest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentDigest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// A value that has NOT been checked against a signed record: what
/// [`Walrus::read_range`](crate::Walrus::read_range) returns. The caller
/// must check it against its own record before trusting it; getting at the
/// value takes a call whose name says so.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "unverified bytes must be checked before use"]
pub struct Unverified<T>(T);

impl<T> Unverified<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(value)
    }

    /// The value, unverified. The caller takes responsibility for checking
    /// it.
    pub fn into_unverified_inner(self) -> T {
        self.0
    }

    /// A reference to the value, unverified.
    pub fn as_unverified(&self) -> &T {
        &self.0
    }
}

impl Unverified<Bytes> {
    /// The bytes, if their sha256 is `expected` (for a caller whose record
    /// covers exactly this range).
    pub fn verify(self, expected: &ContentDigest) -> Result<Bytes, ContentDigest> {
        let actual = ContentDigest::of(&self.0);
        if actual == *expected {
            Ok(self.0)
        } else {
            Err(actual)
        }
    }
}

/// How a blob or quilt is stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreOpts {
    /// Epochs ahead to store for; `None` uses the config's `epochs`.
    pub epochs: Option<u32>,
    /// A permanent blob instead of a deletable one (the publisher's
    /// default since Walrus v1.33).
    pub permanent: bool,
}

/// What a store did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// The blob's Walrus id.
    pub blob_id: BlobId,
    /// The epoch the blob is stored until (exclusive).
    pub end_epoch: u64,
    /// `true` for `newlyCreated`, `false` for `alreadyCertified`.
    pub newly_created: bool,
    /// The unencoded size of the blob in bytes.
    pub size: u64,
    /// The Sui `Blob` object's id, for a newly created blob.
    pub sui_object_id: Option<String>,
}

/// What a quilt store did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredQuilt {
    /// The quilt's blob.
    pub blob: Stored,
    /// Each identifier and the patch id it was stored under.
    pub patches: Vec<(String, QuiltPatchId)>,
}

/// One entry of `GET /v1/quilts/{id}/patches`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PatchEntry {
    /// The identifier the file was stored under.
    pub identifier: String,
    /// The patch's id.
    pub patch_id: QuiltPatchId,
    /// Walrus-native tags, if any.
    #[serde(default)]
    pub tags: std::collections::BTreeMap<String, String>,
}
