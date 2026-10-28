//! Worker leases (plan KV2 Task 10; design §77 §8.3, D1656): a row
//! `'L' ‖ name` → `holder u64 ‖ until_ms u64`, taken and renewed by
//! compare-and-put, so one expiry worker and one GC worker run per
//! keyspace on either backend, with no etcd and no topology keys.

use loams_kv::{Store, TxnError};

use crate::layout;
use crate::types::now_ms;

fn decode(v: &[u8]) -> Option<(u64, u64)> {
    let holder = u64::from_be_bytes(v.get(..8)?.try_into().ok()?);
    let until = u64::from_be_bytes(v.get(8..16)?.try_into().ok()?);
    Some((holder, until))
}

fn encode(holder: u64, until: u64) -> Vec<u8> {
    let mut v = holder.to_be_bytes().to_vec();
    v.extend_from_slice(&until.to_be_bytes());
    v
}

/// Takes or renews the lease `name` for `holder` for `ttl_ms` from now (at
/// `now`): true when `holder` holds it afterwards. A lease held by another
/// holder and not yet expired is left alone.
pub async fn hold_at(
    store: &Store,
    name: &str,
    holder: u64,
    ttl_ms: u64,
    now: u64,
) -> Result<bool, TxnError> {
    let key = layout::lease_key(name);
    let current = store.get(&key).await?;
    if let Some((h, until)) = current.as_deref().and_then(decode)
        && h != holder
        && until > now
    {
        return Ok(false);
    }
    let next = encode(holder, now.saturating_add(ttl_ms));
    Ok(store
        .compare_and_put(&key, current.as_deref(), Some(&next))
        .await?
        .is_ok())
}

/// [`hold_at`] now.
pub async fn hold(store: &Store, name: &str, holder: u64, ttl_ms: u64) -> Result<bool, TxnError> {
    hold_at(store, name, holder, ttl_ms, now_ms()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_lease_holder() {
        let store = loams_kv::testing::embedded().await;
        assert!(
            hold_at(&store, "expire", 1, 1_000, 10_000)
                .await
                .expect("a hold")
        );
        assert!(
            !hold_at(&store, "expire", 2, 1_000, 10_500)
                .await
                .expect("a hold"),
            "held by 1"
        );
        assert!(
            hold_at(&store, "expire", 1, 1_000, 10_900)
                .await
                .expect("a renewal")
        );
        assert!(
            !hold_at(&store, "expire", 2, 1_000, 11_800)
                .await
                .expect("a hold"),
            "renewed to 11,900"
        );
        assert!(
            hold_at(&store, "expire", 2, 1_000, 12_000)
                .await
                .expect("a takeover"),
            "expired"
        );
        assert!(
            !hold_at(&store, "expire", 1, 1_000, 12_100)
                .await
                .expect("a hold")
        );
        // Leases of different names are independent.
        assert!(
            hold_at(&store, "gc", 1, 1_000, 12_100)
                .await
                .expect("a hold")
        );
    }
}
