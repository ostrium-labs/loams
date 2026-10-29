//! Transactional scalar-cell rows for the first CQL single-partition executor.
use crate::cassandra_token;
use loams_kv::{Store, TxnError, TxnOptions};
use loams_tuple::key::{put_bytes, put_u32, put_u64};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
/// A scalar value with its Cassandra client-supplied microsecond timestamp.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cell {
    /// Timestamp used for last-write-wins reconciliation.
    pub timestamp: i64,
    /// The value encoded using the column's Cassandra wire type.
    pub value: Vec<u8>,
}
/// Durable row operations on a tenant-scoped KV handle.
#[derive(Clone, Debug)]
pub struct RowStore {
    store: Store,
}
fn row_key(table: u32, partition: &[u8], clustering: &[u8]) -> Vec<u8> {
    let mut out = vec![b'C', b'c'];
    put_u32(&mut out, table);
    put_u64(
        &mut out,
        (cassandra_token(partition) as u64) ^ 0x8000_0000_0000_0000,
    );
    put_bytes(&mut out, partition);
    put_bytes(&mut out, clustering);
    out
}
fn decode(bytes: Option<Vec<u8>>) -> Result<BTreeMap<String, Cell>, TxnError> {
    bytes
        .map(|v| serde_json::from_slice(&v).map_err(|_| TxnError::Fatal("invalid CQL row".into())))
        .unwrap_or_else(|| Ok(BTreeMap::new()))
}
impl RowStore {
    /// Uses an already tenant-scoped transactional KV store.
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    /// Reconciles cells atomically. Equal timestamps choose the unsigned greater value.
    /// TTLs, deletes and non-frozen collections are not implemented by this scalar-row slice.
    pub async fn upsert(
        &self,
        table: u32,
        partition: &[u8],
        clustering: &[u8],
        cells: BTreeMap<String, Cell>,
    ) -> Result<(), TxnError> {
        if partition.is_empty() {
            return Err(TxnError::Fatal("empty partition key".into()));
        }
        let key = row_key(table, partition, clustering);
        self.store
            .run(TxnOptions::pessimistic("cql.upsert"), move |txn| {
                let (key, cells) = (key.clone(), cells.clone());
                Box::pin(async move {
                    let mut row = decode(txn.get_for_update(&key).await?)?;
                    for (name, cell) in cells {
                        let replace = row.get(&name).is_none_or(|old| {
                            cell.timestamp > old.timestamp
                                || (cell.timestamp == old.timestamp && cell.value > old.value)
                        });
                        if replace {
                            row.insert(name, cell);
                        }
                    }
                    let value = serde_json::to_vec(&row)
                        .map_err(|_| TxnError::Fatal("CQL row encoding failed".into()))?;
                    txn.put(&key, value).await
                })
            })
            .await
            .map(|_| ())
    }
    /// Reads one row, preserving per-cell timestamps for reconciliation and query encoding.
    pub async fn get(
        &self,
        table: u32,
        partition: &[u8],
        clustering: &[u8],
    ) -> Result<Option<BTreeMap<String, Cell>>, TxnError> {
        self.store
            .get(&row_key(table, partition, clustering))
            .await?
            .map(|v| decode(Some(v)))
            .transpose()
    }
}
