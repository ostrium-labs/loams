//! Scan cursors (plan KV2 Tasks 5–9; design §77 §8.5): an opaque number
//! that maps, per connection, to the point a `SCAN`-family command resumes
//! from. Valkey's cursors encode a position in its hash table; ours name a
//! resume key the connection remembers, so the cursor stays a small
//! number, as clients expect, while the store is scanned in key order.

use std::collections::VecDeque;

/// How many open cursors a connection keeps; older ones are forgotten (a
/// forgotten cursor restarts nothing: it answers an empty, final page).
pub const KEEP: usize = 64;

/// One connection's open cursors.
#[derive(Debug, Default)]
pub struct Cursors {
    next: u64,
    open: VecDeque<(u64, Vec<u8>, Vec<u8>)>,
}

/// What a cursor argument asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// Cursor 0: from the start.
    Start,
    /// A cursor this connection handed out, for this scope: from the item.
    From(Vec<u8>),
    /// A cursor this connection does not know (or for another scope).
    Unknown,
}

impl Cursors {
    /// Remembers `item` as the resume point of `scope` (the command and its
    /// key) and returns the cursor naming it, never 0.
    pub fn save(&mut self, scope: &[u8], item: Vec<u8>) -> u64 {
        self.next = self.next.wrapping_add(1).max(1);
        if self.open.len() == KEEP {
            self.open.pop_front();
        }
        self.open.push_back((self.next, scope.to_vec(), item));
        self.next
    }

    /// Where cursor `id` resumes `scope`. A cursor is used once.
    pub fn take(&mut self, id: u64, scope: &[u8]) -> Resume {
        if id == 0 {
            return Resume::Start;
        }
        let Some(at) = self
            .open
            .iter()
            .position(|(c, s, _)| *c == id && s == scope)
        else {
            return Resume::Unknown;
        };
        match self.open.remove(at) {
            Some((_, _, item)) => Resume::From(item),
            None => Resume::Unknown,
        }
    }
}

/// A cursor argument: an unsigned decimal, or Valkey's error.
pub fn parse(arg: &[u8]) -> Result<u64, crate::Value> {
    std::str::from_utf8(arg)
        .ok()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| crate::Value::err("ERR invalid cursor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_resume_once_and_by_scope() {
        let mut c = Cursors::default();
        assert_eq!(c.take(0, b"k"), Resume::Start);
        let id = c.save(b"k", b"f7".to_vec());
        assert_ne!(id, 0);
        assert_eq!(c.take(id, b"other"), Resume::Unknown);
        assert_eq!(c.take(id, b"k"), Resume::From(b"f7".to_vec()));
        assert_eq!(c.take(id, b"k"), Resume::Unknown, "used once");
        let first = c.save(b"k", b"a".to_vec());
        for _ in 0..KEEP {
            c.save(b"k", b"b".to_vec());
        }
        assert_eq!(c.take(first, b"k"), Resume::Unknown, "forgotten");
        assert!(parse(b"12").is_ok());
        assert!(parse(b"-1").is_err());
        assert!(parse(b"x").is_err());
        assert!(parse(b"").is_err());
    }
}
