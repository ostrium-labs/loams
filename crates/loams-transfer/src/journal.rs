//! The fenced journal (§85 §5.1, D2425): what a transfer or run has committed.
//!
//! - Entries live in **immutable segments** in the bucket. A segment names the
//!   one before it (`prev`), and every [`SNAPSHOT_EVERY`]th segment is a
//!   snapshot that repeats every entry so far, so a load never reads more than
//!   that many segments.
//! - A **metastore pointer** holds the [`Head`]: the newest segment's key, the
//!   entry count, the committed bytes and the state. Pointer values are at
//!   most 1 KiB, which is why the entries are not in it.
//! - **To commit**: write the new segment under a key nobody else can choose
//!   (sequence, lease epoch and a random nonce), then swap the pointer from
//!   the version the writer loaded, under the writer's lease fence. The swap
//!   is the commit. A segment whose swap lost is never referenced.
//!
//! This is the seeding script's "insert the batch and advance `next_event`
//! in one transaction" (§85 §2), on storage that has no multi-object
//! transactions.

use std::sync::Arc;

use bytes::Bytes;
use loams_common::NamespaceId;
use loams_common::meta::{Fence, MAX_KEY_LEN};
use loams_store::Store;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::checkpoint::{CheckpointError, Checkpoints};
use crate::error::{Error, Result};
use crate::keys;

/// Every this-many segments, the segment is a snapshot of all entries.
pub const SNAPSHOT_EVERY: u64 = 64;

/// The pointer's value: the journal's head and summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Head {
    /// The format version, 1.
    pub v: u8,
    /// The newest segment's sequence number (0: no segment yet).
    pub seq: u64,
    /// The newest segment's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Committed entries.
    pub n: u64,
    /// Committed bytes (chunk bytes for a transfer, log bytes for a run).
    pub bytes: u64,
    /// The state, in the owner type's vocabulary.
    pub st: String,
    /// Small fields of the owner type (a run's job name, cancel flag, exit
    /// code). The whole value must stay within the pointer limit.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub x: serde_json::Map<String, serde_json::Value>,
}

impl Head {
    /// An empty journal in state `st`.
    pub fn new(st: &str) -> Self {
        Head {
            v: 1,
            seq: 0,
            head: None,
            n: 0,
            bytes: 0,
            st: st.to_string(),
            x: serde_json::Map::new(),
        }
    }

    fn encode(&self) -> Result<String> {
        let s = serde_json::to_string(self)?;
        if s.len() > MAX_KEY_LEN {
            return Err(Error::BadRequest(format!(
                "journal head is {} bytes, over the pointer limit of {MAX_KEY_LEN}",
                s.len()
            )));
        }
        Ok(s)
    }
}

#[derive(Serialize, Deserialize)]
struct Segment<E> {
    seq: u64,
    #[serde(default)]
    prev: Option<String>,
    snapshot: bool,
    entries: Vec<E>,
}

/// A journal loaded at one pointer version.
#[derive(Clone, Debug)]
pub struct Loaded<E> {
    pub version: u64,
    pub head: Head,
    pub entries: Vec<E>,
}

/// One journal: a pointer in a namespace and its segments' key prefix.
#[derive(Clone, Debug)]
pub struct Journal {
    store: Store,
    ck: Arc<dyn Checkpoints>,
    ns: NamespaceId,
    pointer: String,
    segments: String,
}

impl Journal {
    pub(crate) fn new(
        store: Store,
        ck: Arc<dyn Checkpoints>,
        ns: NamespaceId,
        pointer: String,
        segments: String,
    ) -> Self {
        Journal {
            store,
            ck,
            ns,
            pointer,
            segments,
        }
    }

    /// Creates the journal with `head` if it does not exist. Returns `false`
    /// when it already existed.
    pub async fn create(&self, head: &Head) -> Result<bool> {
        match self
            .ck
            .cas_pointer(self.ns, &self.pointer, None, head.encode()?, None)
            .await
        {
            Ok(_) => Ok(true),
            Err(CheckpointError::VersionMismatch { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// The pointer's version and head, without reading segments.
    pub async fn head(&self) -> Result<Option<(u64, Head)>> {
        let Some(p) = self.ck.pointer(self.ns, &self.pointer).await? else {
            return Ok(None);
        };
        let head: Head = serde_json::from_str(&p.value)?;
        Ok(Some((p.version, head)))
    }

    /// The journal with every committed entry, oldest first.
    pub async fn load<E: DeserializeOwned>(&self) -> Result<Option<Loaded<E>>> {
        let Some((version, head)) = self.head().await? else {
            return Ok(None);
        };
        let entries = self.entries(&head).await?;
        Ok(Some(Loaded {
            version,
            head,
            entries,
        }))
    }

    async fn entries<E: DeserializeOwned>(&self, head: &Head) -> Result<Vec<E>> {
        let mut chain: Vec<Vec<E>> = Vec::new();
        let mut next = head.head.clone();
        let mut walked = 0u64;
        while let Some(key) = next {
            let (data, _) = self.store.get(&key).await?;
            let seg: Segment<E> = serde_json::from_slice(&data)?;
            walked += 1;
            if walked > SNAPSHOT_EVERY + 1 {
                return Err(Error::Corrupt(format!(
                    "journal {} has no snapshot within {SNAPSHOT_EVERY} segments",
                    self.pointer
                )));
            }
            next = if seg.snapshot { None } else { seg.prev };
            chain.push(seg.entries);
        }
        let mut out = Vec::new();
        for part in chain.into_iter().rev() {
            out.extend(part);
        }
        if out.len() as u64 != head.n {
            return Err(Error::Corrupt(format!(
                "journal {} lists {} entries, its head says {}",
                self.pointer,
                out.len(),
                head.n
            )));
        }
        Ok(out)
    }

    /// Commits `new` entries with the head `next` (its `seq`, `head` and `n`
    /// are set here), on top of `cur`, under `fence`.
    ///
    /// Returns `Ok(None)` when the pointer moved since `cur` was loaded: the
    /// caller reloads, re-checks and tries again. A broken fence is
    /// [`Error::LeaseLost`].
    pub async fn append<E: Serialize + Clone>(
        &self,
        cur: &Loaded<E>,
        new: Vec<E>,
        mut next: Head,
        fence: Option<Fence>,
    ) -> Result<Option<Loaded<E>>> {
        let seq = cur.head.seq + 1;
        let snapshot = seq.is_multiple_of(SNAPSHOT_EVERY);
        let mut all = cur.entries.clone();
        all.extend(new.iter().cloned());
        let seg = Segment {
            seq,
            prev: cur.head.head.clone(),
            snapshot,
            entries: if snapshot { all.clone() } else { new },
        };
        let epoch = fence.as_ref().map_or(0, |f| f.epoch);
        let key = format!("{}{seq:08}-{epoch}-{}.json", self.segments, keys::new_id());
        next.seq = seq;
        next.head = Some(key.clone());
        next.n = all.len() as u64;
        let value = next.encode()?;
        self.store
            .put_if_absent(&key, Bytes::from(serde_json::to_vec(&seg)?))
            .await?;
        match self
            .ck
            .cas_pointer(self.ns, &self.pointer, Some(cur.version), value, fence)
            .await
        {
            Ok(version) => Ok(Some(Loaded {
                version,
                head: next,
                entries: all,
            })),
            Err(CheckpointError::VersionMismatch { .. }) => {
                // Our segment is unreferenced; remove it on a best-effort basis.
                let _ = self.store.delete(&key).await;
                Ok(None)
            }
            Err(e) => {
                let _ = self.store.delete(&key).await;
                Err(e.into())
            }
        }
    }

    /// Swaps the head only (no new entries), as `append` does.
    pub async fn set_head(
        &self,
        version: u64,
        head: &Head,
        fence: Option<Fence>,
    ) -> Result<Option<u64>> {
        match self
            .ck
            .cas_pointer(self.ns, &self.pointer, Some(version), head.encode()?, fence)
            .await
        {
            Ok(v) => Ok(Some(v)),
            Err(CheckpointError::VersionMismatch { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::MemCheckpoints;
    use std::time::Duration;

    fn journal(ck: &MemCheckpoints, store: &Store) -> Journal {
        Journal::new(
            store.clone(),
            Arc::new(ck.clone()),
            NamespaceId(1),
            "j/1".into(),
            "j/1/journal/".into(),
        )
    }

    #[tokio::test]
    async fn journal_roundtrip_with_snapshots() {
        let ck = MemCheckpoints::new();
        let store = Store::in_memory();
        let j = journal(&ck, &store);
        assert!(j.create(&Head::new("open")).await.expect("create"));
        assert!(!j.create(&Head::new("open")).await.expect("create again"));
        let mut cur: Loaded<u32> = j.load().await.expect("load").expect("exists");
        for i in 0..(SNAPSHOT_EVERY as u32 * 2 + 5) {
            let mut head = cur.head.clone();
            head.bytes += 10;
            cur = j
                .append(&cur, vec![i], head, None)
                .await
                .expect("append")
                .expect("not stale");
        }
        let back: Loaded<u32> = j.load().await.expect("load").expect("exists");
        let want: Vec<u32> = (0..(SNAPSHOT_EVERY as u32 * 2 + 5)).collect();
        assert_eq!(back.entries, want);
        assert_eq!(back.head.bytes, 10 * want.len() as u64);
        assert_eq!(back.version, cur.version);
    }

    #[tokio::test]
    async fn stale_append_is_reported_and_leaves_no_reference() {
        let ck = MemCheckpoints::new();
        let store = Store::in_memory();
        let j = journal(&ck, &store);
        j.create(&Head::new("open")).await.expect("create");
        let a: Loaded<u32> = j.load().await.expect("load").expect("exists");
        let b = a.clone();
        let a2 = j
            .append(&a, vec![1], a.head.clone(), None)
            .await
            .expect("a");
        assert!(a2.is_some());
        let b2 = j
            .append(&b, vec![2], b.head.clone(), None)
            .await
            .expect("b");
        assert!(b2.is_none(), "b loaded before a committed, so it is stale");
        let back: Loaded<u32> = j.load().await.expect("load").expect("exists");
        assert_eq!(back.entries, vec![1]);
    }

    #[tokio::test]
    async fn fenced_append_after_takeover_is_lease_lost() {
        let ck = MemCheckpoints::new();
        let store = Store::in_memory();
        let j = journal(&ck, &store);
        j.create(&Head::new("open")).await.expect("create");
        let ttl = Duration::from_secs(30);
        let a = ck.acquire_lease("L", "a", ttl).await.expect("a");
        ck.advance(Duration::from_secs(31));
        ck.acquire_lease("L", "b", ttl).await.expect("b takes over");
        let cur: Loaded<u32> = j.load().await.expect("load").expect("exists");
        let fence = Fence {
            lease: "L".into(),
            epoch: a.epoch,
        };
        let r = j.append(&cur, vec![1], cur.head.clone(), Some(fence)).await;
        assert!(matches!(r, Err(Error::LeaseLost)), "{r:?}");
    }
}
