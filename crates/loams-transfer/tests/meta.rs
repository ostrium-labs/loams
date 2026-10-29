//! The journal on a real single-node metastore (DQ1 Task 1): the adapter
//! maps leases, fences and pointer swaps as the in-memory checkpoints do.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{self, StreamExt};
use loams_common::meta::MetaStore;
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_store::Store;
use loams_transfer::keys::new_id;
use loams_transfer::{
    CreateTransfer, Error, MetaCheckpoints, TransferState, Transfers, TransfersConfig,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[tokio::test]
async fn transfer_on_the_metastore() {
    let meta_dir = TempDir::new().expect("temp dir");
    let clock = Arc::new(SystemClock);
    let mut config = MetaConfig::new(1, meta_dir.path(), Store::in_memory());
    config.clock = clock.clone();
    let node = MetaNode::start(config, &Router::new())
        .await
        .expect("start meta");
    node.initialize([1]).await.expect("initialize");
    node.wait_for_leader(Duration::from_secs(30))
        .await
        .expect("leader");
    let meta: Arc<dyn MetaStore> = Arc::new(MetaClient::new(
        node.clone(),
        vec![],
        clock,
        MetaClientConfig::default(),
    ));
    let ns = meta.create_namespace("acme").await.expect("namespace");

    let cfg = TransfersConfig {
        min_chunk: 4 << 10,
        ..TransfersConfig::default()
    };
    let t = Transfers::new(
        Store::in_memory(),
        Arc::new(MetaCheckpoints::new(meta)),
        cfg,
    );
    let tid = new_id();
    let chunk = 4096usize;
    let data: Vec<u8> = (0..chunk * 3).map(|n| (n % 251) as u8).collect();
    t.create(
        ns,
        &tid,
        CreateTransfer {
            name: "f".into(),
            size: data.len() as u64,
            chunk_size: chunk as u64,
            client_id: "install-1".into(),
        },
    )
    .await
    .expect("create");
    let g = t.lease(ns, &tid, "install-1").await.expect("lease");
    assert!(matches!(
        t.lease(ns, &tid, "install-2").await,
        Err(Error::LeaseHeld { .. })
    ));
    for (i, c) in data.chunks(chunk).enumerate() {
        let sha = hex::encode(Sha256::digest(c));
        let body = stream::iter([Ok(Bytes::copy_from_slice(c))]).boxed();
        t.put_chunk(ns, &tid, i as u32, &sha, g.epoch, body)
            .await
            .expect("put");
    }
    let v = t
        .complete(ns, &tid, g.epoch, &hex::encode(Sha256::digest(&data)))
        .await
        .expect("complete");
    assert_eq!(v.state, TransferState::Complete);
    assert_eq!(v.committed, vec![[0, 3]]);
    drop(node);
}
