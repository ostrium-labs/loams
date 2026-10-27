//! Shard assignment (X1.R6).
//!
//! A run's rows live in shard `shard_of(namespace_id, workflow_id, S)`, where
//! `S` is the shard count fixed when the database is created (default 64,
//! [`DEFAULT_SHARDS`]). Every run of one workflow ID lands in the same shard,
//! so the current-run check and the runs it guards share a shard.

use sha2::{Digest, Sha256};

/// The shard count of a new `loams_temporal` database (X1.R6, Q1152).
pub const DEFAULT_SHARDS: u32 = 64;

/// The largest shard count a store accepts. `shard_id` is a Postgres
/// `integer`, and a count this large is already far past any planned use.
pub const MAX_SHARDS: u32 = 1 << 20;

/// The shard of a workflow: the first 8 bytes of
/// SHA-256(`namespace_id` ‖ 0x00 ‖ `workflow_id`), read big-endian, mod `shards`.
///
/// # Panics
///
/// If `shards` is 0. The stores refuse a zero shard count when they are
/// created, so a store never calls this with 0.
pub fn shard_of(namespace_id: &str, workflow_id: &str, shards: u32) -> u32 {
    assert!(shards > 0, "the shard count must be positive");
    let mut h = Sha256::new();
    h.update(namespace_id.as_bytes());
    h.update([0u8]);
    h.update(workflow_id.as_bytes());
    let digest = h.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    // The remainder is below `shards`, a u32.
    (u64::from_be_bytes(first) % u64::from(shards)) as u32
}

/// The shards relay `replica` of `replicas` claims outbox rows from (D2064):
/// every shard `s` with `s % replicas == replica`. Together the replicas
/// cover every shard exactly once; a single replica owns them all. Claims
/// are leased and `SKIP LOCKED`, so a brief overlap while replicas change
/// count is safe, only wasteful.
///
/// Empty if `replicas` is 0 or `replica >= replicas`.
pub fn owned_shards(replica: u32, replicas: u32, shards: u32) -> Vec<u32> {
    if replicas == 0 || replica >= replicas {
        return Vec::new();
    }
    (0..shards).filter(|s| s % replicas == replica).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_shards_partition() {
        assert_eq!(owned_shards(0, 1, 4), vec![0, 1, 2, 3]);
        assert_eq!(owned_shards(1, 3, 8), vec![1, 4, 7]);
        assert!(owned_shards(3, 3, 8).is_empty());
        assert!(owned_shards(0, 0, 8).is_empty());
        let mut all: Vec<u32> = (0..5).flat_map(|r| owned_shards(r, 5, 64)).collect();
        all.sort_unstable();
        assert_eq!(all, (0..64).collect::<Vec<_>>());
    }

    // Computed independently with Python's hashlib:
    // int.from_bytes(sha256(ns + b"\0" + wf).digest()[:8], "big") % shards
    #[test]
    fn vectors() {
        assert_eq!(shard_of("default", "hello-workflow", 64), 63);
        assert_eq!(shard_of("ns-1", "wf-1", 64), 28);
        assert_eq!(shard_of("", "", 64), 24);
        assert_eq!(
            shard_of("550e8400-e29b-41d4-a716-446655440000", "order-42", 64),
            58
        );
        assert_eq!(shard_of("a", "b", 1000), 387);
        assert_eq!(shard_of("a", "b", 1), 0);
    }
}
