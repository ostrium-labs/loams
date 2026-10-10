//! Pagination cursors (LV1 plan Task 4; design §45 §3).
//!
//! A cursor names where the next page starts: the last index key a page
//! read, and the index it belongs to. On the wire it is the base64url text
//! (no padding) of
//!
//! ```text
//! version:u8 (1) ‖ index_id:u32 BE ‖ last_key ‖ HMAC-SHA256(app_key, version ‖ index_id ‖ last_key)[..16]
//! ```
//!
//! An empty `last_key` is a start cursor: the start of any range, so it is
//! accepted on every index (an empty page of a table that does not exist
//! yet returns one). The tag is keyed with the
//! app's own [`CursorKey`], a catalog record written with the app
//! ([`AppKeys::cursor_key`](crate::AppKeys::cursor_key)), so a client can
//! neither forge a position nor carry a cursor to another app, whose key
//! layout is otherwise the same. Every refusal is [`LiveError::BadCursor`].

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use rand::TryRngCore;
use rand::rngs::OsRng;
use sha2::Sha256;

use crate::{IndexId, LiveError};

/// The cursor format this build writes and reads.
pub const CURSOR_VERSION: u8 = 1;

/// The bytes of a cursor's tag: HMAC-SHA256 cut to 16 bytes.
pub const TAG_BYTES: usize = 16;

/// The bytes of an app's cursor key.
pub const CURSOR_KEY_BYTES: usize = 32;

/// The longest cursor text accepted. A genuine cursor holds one index key
/// (at most `max_index_key_bytes`, 4 KiB, plus its prefix and suffix).
pub const MAX_CURSOR_CHARS: usize = 16 * 1024;

const HEADER_BYTES: usize = 1 + 4;

/// An app's cursor key: the HMAC key of its cursors. Its `Debug` hides the
/// bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct CursorKey([u8; CURSOR_KEY_BYTES]);

impl CursorKey {
    /// A key of the given bytes.
    pub fn from_bytes(bytes: [u8; CURSOR_KEY_BYTES]) -> Self {
        CursorKey(bytes)
    }

    /// A new key from the OS random source.
    pub fn random() -> Result<Self, LiveError> {
        let mut bytes = [0; CURSOR_KEY_BYTES];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|e| LiveError::Internal(format!("the OS random source failed: {e}")))?;
        Ok(CursorKey(bytes))
    }

    /// The key's bytes, as the catalog record stores them.
    pub fn as_bytes(&self) -> &[u8; CURSOR_KEY_BYTES] {
        &self.0
    }

    /// The key a catalog record holds.
    pub fn from_record(bytes: &[u8]) -> Result<Self, LiveError> {
        let bytes: [u8; CURSOR_KEY_BYTES] = bytes.try_into().map_err(|_| {
            LiveError::Corrupt(format!(
                "the app's cursor key has {} bytes, not {CURSOR_KEY_BYTES}",
                bytes.len()
            ))
        })?;
        Ok(CursorKey(bytes))
    }

    fn mac(&self) -> Hmac<Sha256> {
        // HMAC takes a key of any length; a 32-byte key never fails.
        #[allow(clippy::unwrap_used)]
        <Hmac<Sha256> as KeyInit>::new_from_slice(&self.0).unwrap()
    }
}

impl fmt::Debug for CursorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CursorKey(..)")
    }
}

/// Where a page ends, and so where the next one starts: the last key read
/// in `index` (exclusive, in the query's order), or the start of any range
/// when `key` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBound {
    pub index: IndexId,
    pub key: Vec<u8>,
}

/// The cursor text of `end`, tagged with the app's key.
pub fn encode(app_key: &CursorKey, end: &KeyBound) -> String {
    let mut raw = Vec::with_capacity(HEADER_BYTES + end.key.len() + TAG_BYTES);
    raw.push(CURSOR_VERSION);
    raw.extend_from_slice(&end.index.0.to_be_bytes());
    raw.extend_from_slice(&end.key);
    let mut mac = app_key.mac();
    mac.update(&raw);
    raw.extend_from_slice(&mac.finalize().into_bytes()[..TAG_BYTES]);
    data_encoding::BASE64URL_NOPAD.encode(&raw)
}

/// The position `cursor` names, if the app's key tagged it. A malformed,
/// truncated or altered cursor, one of another version and one tagged by
/// another app are all [`LiveError::BadCursor`], with no detail of which.
pub fn decode(app_key: &CursorKey, cursor: &str) -> Result<KeyBound, LiveError> {
    let bad = || LiveError::BadCursor("the pagination cursor is not valid for this query".into());
    if cursor.len() > MAX_CURSOR_CHARS {
        return Err(bad());
    }
    let raw = data_encoding::BASE64URL_NOPAD
        .decode(cursor.as_bytes())
        .map_err(|_| bad())?;
    if raw.len() < HEADER_BYTES + TAG_BYTES {
        return Err(bad());
    }
    let (body, tag) = raw.split_at(raw.len() - TAG_BYTES);
    let mut mac = app_key.mac();
    mac.update(body);
    // Constant time: the comparison does not tell how much of a forged tag
    // was right.
    mac.verify_truncated_left(tag).map_err(|_| bad())?;
    if body[0] != CURSOR_VERSION {
        return Err(bad());
    }
    let mut index = [0; 4];
    index.copy_from_slice(&body[1..HEADER_BYTES]);
    Ok(KeyBound {
        index: IndexId(u32::from_be_bytes(index)),
        key: body[HEADER_BYTES..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_keys_differ_and_hide_in_debug() {
        let (a, b) = (
            CursorKey::random().expect("a key"),
            CursorKey::random().expect("a key"),
        );
        assert_ne!(a, b);
        assert_eq!(format!("{a:?}"), "CursorKey(..)");
        assert_eq!(CursorKey::from_record(a.as_bytes()).expect("32 bytes"), a);
        assert!(matches!(
            CursorKey::from_record(&[1; 31]),
            Err(LiveError::Corrupt(_))
        ));
    }

    #[test]
    fn a_cursor_of_another_version_is_refused() {
        let key = CursorKey::from_bytes([1; 32]);
        let mut raw = vec![CURSOR_VERSION + 1, 0, 0, 0, 1];
        let mut mac = key.mac();
        mac.update(&raw);
        raw.extend_from_slice(&mac.finalize().into_bytes()[..TAG_BYTES]);
        let text = data_encoding::BASE64URL_NOPAD.encode(&raw);
        assert!(matches!(decode(&key, &text), Err(LiveError::BadCursor(_))));
    }
}
