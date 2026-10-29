//! Object keys, lease keys and pointer keys (§85 §5), and the checks on ids
//! and hashes that go into them.

use loams_common::NamespaceId;

use crate::error::{Error, Result};

/// A transfer or run id: a ULID in its canonical 26-character form.
pub fn check_id(kind: &str, id: &str) -> Result<()> {
    match ulid::Ulid::from_string(id) {
        Ok(u) if u.to_string() == id => Ok(()),
        _ => Err(Error::BadRequest(format!(
            "{kind} id must be a canonical ULID"
        ))),
    }
}

/// A lowercase hex SHA-256.
pub fn check_sha256(what: &str, hex: &str) -> Result<()> {
    if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "{what} must be a lowercase hex SHA-256"
        )))
    }
}

/// A new random id for a transfer, run or segment nonce.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

pub(crate) fn transfer_prefix(ns: NamespaceId, tid: &str) -> String {
    format!("transfers/{ns}/{tid}/")
}

pub(crate) fn transfer_spec(ns: NamespaceId, tid: &str) -> String {
    format!("{}spec.json", transfer_prefix(ns, tid))
}

/// The chunk key carries its index and hash (D2424): the same bytes always
/// land on the same key, and different bytes never overwrite committed ones.
pub(crate) fn chunk(ns: NamespaceId, tid: &str, index: u32, sha256: &str) -> String {
    format!("{}chunks/{index:08}-{sha256}", transfer_prefix(ns, tid))
}

pub(crate) fn transfer_journal(ns: NamespaceId, tid: &str) -> String {
    format!("{}journal/", transfer_prefix(ns, tid))
}

pub(crate) fn transfer_pointer(tid: &str) -> String {
    format!("{TRANSFER_POINTER_PREFIX}{tid}")
}

pub(crate) const TRANSFER_POINTER_PREFIX: &str = "transfer/";

/// Lease keys are global in the metastore, so they carry the namespace id.
pub(crate) fn transfer_lease(ns: NamespaceId, tid: &str) -> String {
    format!("transfer/{ns}/{tid}")
}

pub(crate) fn run_prefix(ns: NamespaceId, rid: &str) -> String {
    format!("runs/{ns}/{rid}/")
}

pub(crate) fn run_spec(ns: NamespaceId, rid: &str) -> String {
    format!("{}spec.json", run_prefix(ns, rid))
}

pub(crate) fn run_journal(ns: NamespaceId, rid: &str) -> String {
    format!("{}journal/", run_prefix(ns, rid))
}

/// A log segment's key carries its start offset and hash, so a resend of the
/// same bytes at the same offset is idempotent (D2441).
pub(crate) fn run_log(ns: NamespaceId, rid: &str, start: u64, sha256: &str) -> String {
    format!("{}log/{start:016}-{sha256}", run_prefix(ns, rid))
}

pub(crate) const RUN_POINTER_PREFIX: &str = "run/";

pub(crate) fn run_pointer(rid: &str) -> String {
    format!("{RUN_POINTER_PREFIX}{rid}")
}

pub(crate) fn run_lease(ns: NamespaceId, rid: &str) -> String {
    format!("run/{ns}/{rid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_must_be_canonical_ulids() {
        let id = new_id();
        assert!(check_id("transfer", &id).is_ok());
        assert!(check_id("transfer", &id.to_lowercase()).is_err());
        assert!(check_id("transfer", "../../etc").is_err());
        assert!(check_id("transfer", "").is_err());
    }

    #[test]
    fn sha_must_be_lowercase_hex() {
        assert!(check_sha256("h", &"a".repeat(64)).is_ok());
        assert!(check_sha256("h", &"A".repeat(64)).is_err());
        assert!(check_sha256("h", &"a".repeat(63)).is_err());
        assert!(check_sha256("h", &format!("{}/", "a".repeat(63))).is_err());
    }

    #[test]
    fn chunk_keys_are_sortable_and_carry_the_hash() {
        let k = chunk(NamespaceId(7), "T", 3, &"b".repeat(64));
        assert_eq!(
            k,
            format!("transfers/7/T/chunks/00000003-{}", "b".repeat(64))
        );
    }
}
