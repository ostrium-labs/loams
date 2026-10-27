//! Page tokens (D2061).
//!
//! Tokens are opaque bytes to callers (Temporal's `next_page_token` is
//! `bytes`). Each one starts with a kind byte and a format version, then 8
//! bytes that bind it to what it pages: a history token to its run, a listing
//! token to its namespace and filter. A token presented for anything else is
//! refused with [`StoreError::InvalidArgument`] instead of silently paging
//! the wrong thing. Tokens are not secret and not signed: they only carry
//! positions the caller could ask for directly.
//!
//! Listing tokens are keysets (the last row's start time, workflow ID and run
//! ID), so a run started while a client pages never shifts or repeats a row.

use sha2::{Digest, Sha256};

use crate::{FIRST_EVENT_ID, RunFilter, RunKey, StoreError};

/// The largest page [`crate::Store::history_page`] and
/// [`crate::Store::list_runs`] return.
pub const MAX_PAGE_SIZE: usize = 10_000;

const HISTORY: u8 = b'H';
const LIST: u8 = b'L';
const VERSION: u8 = 1;

pub(crate) fn check_page_size(page_size: usize) -> Result<(), StoreError> {
    if page_size == 0 || page_size > MAX_PAGE_SIZE {
        return Err(StoreError::InvalidArgument(format!(
            "page size {page_size} is outside 1..={MAX_PAGE_SIZE}"
        )));
    }
    Ok(())
}

fn bad(why: &str) -> StoreError {
    StoreError::InvalidArgument(format!("page token: {why}"))
}

/// The first 8 bytes of SHA-256 over length-prefixed fields.
fn binding(fields: &[&[u8]]) -> [u8; 8] {
    let mut h = Sha256::new();
    for f in fields {
        h.update((f.len() as u64).to_be_bytes());
        h.update(f);
    }
    let d = h.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&d[..8]);
    out
}

fn run_binding(key: &RunKey) -> [u8; 8] {
    binding(&[
        b"history",
        key.namespace_id.as_bytes(),
        key.workflow_id.as_bytes(),
        key.run_id.as_bytes(),
    ])
}

fn opt(o: &Option<String>) -> Vec<u8> {
    match o {
        None => vec![0],
        Some(s) => {
            let mut v = vec![1];
            v.extend_from_slice(s.as_bytes());
            v
        }
    }
}

fn list_binding(namespace_id: &str, f: &RunFilter) -> [u8; 8] {
    let open = match f.open {
        None => [0u8],
        Some(false) => [1],
        Some(true) => [2],
    };
    binding(&[
        b"list",
        namespace_id.as_bytes(),
        &open,
        &opt(&f.workflow_id),
        &opt(&f.workflow_type),
    ])
}

/// A reader over a token's bytes.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        if self.0.len() < n {
            return Err(bad("truncated"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }

    fn arr8(&mut self) -> Result<[u8; 8], StoreError> {
        let mut out = [0u8; 8];
        out.copy_from_slice(self.take(8)?);
        Ok(out)
    }

    fn i64(&mut self) -> Result<i64, StoreError> {
        Ok(i64::from_be_bytes(self.arr8()?))
    }

    fn string(&mut self) -> Result<String, StoreError> {
        let mut len = [0u8; 4];
        len.copy_from_slice(self.take(4)?);
        let n = u32::from_be_bytes(len) as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| bad("not UTF-8"))
    }

    fn end(&self) -> Result<(), StoreError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(bad("trailing bytes"))
        }
    }
}

fn header(r: &mut Reader<'_>, kind: u8) -> Result<(), StoreError> {
    if r.u8()? != kind {
        return Err(bad("wrong kind"));
    }
    if r.u8()? != VERSION {
        return Err(bad("unknown version"));
    }
    Ok(())
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// The token of the page that starts at event `next` of `key`.
pub(crate) fn encode_history(key: &RunKey, next: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(18);
    out.extend_from_slice(&[HISTORY, VERSION]);
    out.extend_from_slice(&run_binding(key));
    out.extend_from_slice(&next.to_be_bytes());
    out
}

/// The first event ID a token points at; [`FIRST_EVENT_ID`] for an empty one.
pub(crate) fn decode_history(key: &RunKey, token: &[u8]) -> Result<i64, StoreError> {
    if token.is_empty() {
        return Ok(FIRST_EVENT_ID);
    }
    let mut r = Reader(token);
    header(&mut r, HISTORY)?;
    if r.arr8()? != run_binding(key) {
        return Err(bad("issued for another run"));
    }
    let next = r.i64()?;
    r.end()?;
    if next < FIRST_EVENT_ID {
        return Err(bad("event ID out of range"));
    }
    Ok(next)
}

/// The last row of a listing page: the next page starts after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListCursor {
    pub start_time_ms: i64,
    pub workflow_id: String,
    pub run_id: String,
}

impl ListCursor {
    /// Whether a row sorts after the cursor (start time descending, then
    /// workflow ID and run ID ascending).
    pub(crate) fn before(&self, start_time_ms: i64, workflow_id: &str, run_id: &str) -> bool {
        start_time_ms < self.start_time_ms
            || (start_time_ms == self.start_time_ms
                && (workflow_id, run_id) > (self.workflow_id.as_str(), self.run_id.as_str()))
    }
}

pub(crate) fn encode_list(namespace_id: &str, f: &RunFilter, c: &ListCursor) -> Vec<u8> {
    let mut out = Vec::with_capacity(26 + c.workflow_id.len() + c.run_id.len());
    out.extend_from_slice(&[LIST, VERSION]);
    out.extend_from_slice(&list_binding(namespace_id, f));
    out.extend_from_slice(&c.start_time_ms.to_be_bytes());
    put_string(&mut out, &c.workflow_id);
    put_string(&mut out, &c.run_id);
    out
}

/// The cursor a token carries; `None` for an empty token (the first page).
pub(crate) fn decode_list(
    namespace_id: &str,
    f: &RunFilter,
    token: &[u8],
) -> Result<Option<ListCursor>, StoreError> {
    if token.is_empty() {
        return Ok(None);
    }
    let mut r = Reader(token);
    header(&mut r, LIST)?;
    if r.arr8()? != list_binding(namespace_id, f) {
        return Err(bad("issued for another namespace or filter"));
    }
    let start_time_ms = r.i64()?;
    let workflow_id = r.string()?;
    let run_id = r.string()?;
    r.end()?;
    Ok(Some(ListCursor {
        start_time_ms,
        workflow_id,
        run_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(run: &str) -> RunKey {
        RunKey::new("ns", "wf", run)
    }

    #[test]
    fn history_roundtrip_and_binding() {
        let t = encode_history(&k("r1"), 42);
        assert_eq!(t.len(), 18);
        assert_eq!(decode_history(&k("r1"), &t), Ok(42));
        assert_eq!(decode_history(&k("r1"), &[]), Ok(FIRST_EVENT_ID));
        assert!(matches!(
            decode_history(&k("r2"), &t),
            Err(StoreError::InvalidArgument(_))
        ));
        // The old frontend's 8-byte big-endian event ID is not a store token.
        assert!(decode_history(&k("r1"), &42i64.to_be_bytes()).is_err());
        let mut long = t.clone();
        long.push(0);
        assert!(decode_history(&k("r1"), &long).is_err());
        assert!(decode_history(&k("r1"), &encode_history(&k("r1"), 0)).is_err());
    }

    #[test]
    fn list_roundtrip_and_binding() {
        let f = RunFilter {
            open: Some(true),
            workflow_id: None,
            workflow_type: Some("hello".into()),
        };
        let c = ListCursor {
            start_time_ms: 1_761_300_000_123,
            workflow_id: "wf-é".into(),
            run_id: "run-1".into(),
        };
        let t = encode_list("ns", &f, &c);
        assert_eq!(decode_list("ns", &f, &t), Ok(Some(c.clone())));
        assert_eq!(decode_list("ns", &f, &[]), Ok(None));
        assert!(decode_list("ns2", &f, &t).is_err());
        let mut other = f.clone();
        other.open = Some(false);
        assert!(decode_list("ns", &other, &t).is_err());
        // Filters that only differ in which field is set bind differently.
        let a = RunFilter {
            workflow_id: Some("x".into()),
            ..RunFilter::default()
        };
        let b = RunFilter {
            workflow_type: Some("x".into()),
            ..RunFilter::default()
        };
        assert_ne!(list_binding("ns", &a), list_binding("ns", &b));
        assert!(decode_list("ns", &f, &t[..t.len() - 1]).is_err());
        assert!(decode_list("ns", &f, &encode_history(&k("r"), 1)).is_err());
    }

    #[test]
    fn cursor_order() {
        let c = ListCursor {
            start_time_ms: 100,
            workflow_id: "b".into(),
            run_id: "2".into(),
        };
        assert!(c.before(99, "a", "1"));
        assert!(!c.before(101, "z", "9"));
        assert!(c.before(100, "b", "3"));
        assert!(c.before(100, "c", "0"));
        assert!(!c.before(100, "b", "2"));
        assert!(!c.before(100, "a", "9"));
    }

    #[test]
    fn page_sizes() {
        assert!(check_page_size(0).is_err());
        assert!(check_page_size(1).is_ok());
        assert!(check_page_size(MAX_PAGE_SIZE).is_ok());
        assert!(check_page_size(MAX_PAGE_SIZE + 1).is_err());
    }
}
