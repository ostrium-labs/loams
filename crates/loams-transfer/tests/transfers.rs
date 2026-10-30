//! Transfers against the in-memory checkpoints and bucket (DQ1 Task 1).

use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use loams_common::NamespaceId;
use loams_store::Store;
use loams_transfer::keys::new_id;
use loams_transfer::transfer::root;
use loams_transfer::{
    CreateTransfer, Error, MemCheckpoints, TransferState, Transfers, TransfersConfig,
};
use sha2::{Digest, Sha256};

const NS: NamespaceId = NamespaceId(7);
const CHUNK: u64 = 64 << 10;

struct Fx {
    ck: MemCheckpoints,
    store: Store,
    t: Transfers,
}

fn fx() -> Fx {
    let ck = MemCheckpoints::new();
    let store = Store::in_memory();
    let cfg = TransfersConfig {
        min_chunk: 4 << 10,
        part_size: 5 << 20,
        ..TransfersConfig::default()
    };
    let t = Transfers::new(store.clone(), Arc::new(ck.clone()), cfg);
    Fx { ck, store, t }
}

/// Deterministic, non-repeating content: byte `n` of the file.
fn content(size: u64) -> Vec<u8> {
    (0..size)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

fn body(b: &[u8]) -> BoxStream<'static, io::Result<Bytes>> {
    // Several frames per chunk, as a network body would arrive.
    let parts: Vec<io::Result<Bytes>> = b
        .chunks(10_000)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    stream::iter(parts).boxed()
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn req(size: u64) -> CreateTransfer {
    CreateTransfer {
        name: "big.bin".into(),
        size,
        chunk_size: CHUNK,
        client_id: "install-1".into(),
    }
}

async fn upload_all(f: &Fx, tid: &str, data: &[u8], epoch: u64) {
    for (i, c) in data.chunks(CHUNK as usize).enumerate() {
        f.t.put_chunk(NS, tid, i as u32, &sha(c), epoch, body(c))
            .await
            .expect("put");
    }
}

#[tokio::test]
async fn upload_complete_and_read_back() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 3 + 123);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    upload_all(&f, &tid, &data, g.epoch).await;
    let v =
        f.t.complete(NS, &tid, g.epoch, &sha(&data))
            .await
            .expect("complete");
    assert_eq!(v.state, TransferState::Complete);
    assert_eq!(v.committed, vec![[0, 4]]);
    assert_eq!(v.committed_bytes, data.len() as u64);
    let mut back = Vec::new();
    for i in 0..4 {
        let (h, len, mut s) = f.t.open_chunk(NS, &tid, i).await.expect("open");
        let mut c = Vec::new();
        while let Some(b) = s.next().await {
            c.extend_from_slice(&b.expect("bytes"));
        }
        assert_eq!(c.len() as u64, len);
        assert_eq!(sha(&c), h);
        back.extend(c);
    }
    assert_eq!(back, data);
    assert_eq!(f.t.verify_full(NS, &tid).await.expect("verify"), sha(&data));
}

#[tokio::test]
async fn create_is_idempotent_and_resume_with_changed_size_refused() {
    let f = fx();
    let tid = new_id();
    f.t.create(NS, &tid, req(1000)).await.expect("create");
    f.t.create(NS, &tid, req(1000))
        .await
        .expect("same spec again");
    let r = f.t.create(NS, &tid, req(1001)).await;
    assert!(
        matches!(
            r,
            Err(Error::Conflict {
                code: "transfer_exists",
                ..
            })
        ),
        "{r:?}"
    );
    let mut other = req(1000);
    other.chunk_size = CHUNK * 2;
    assert!(f.t.create(NS, &tid, other).await.is_err());
}

#[tokio::test]
async fn chunk_size_and_count_are_bounded() {
    let f = fx();
    let mut r = req(1000);
    r.chunk_size = 3000;
    assert!(matches!(
        f.t.create(NS, &new_id(), r).await,
        Err(Error::BadRequest(_))
    ));
    let r = req(CHUNK * 16_385);
    assert!(matches!(
        f.t.create(NS, &new_id(), r).await,
        Err(Error::BadRequest(_))
    ));
    assert!(matches!(
        f.t.create(NS, "nope", req(1)).await,
        Err(Error::BadRequest(_))
    ));
}

#[tokio::test]
async fn same_chunk_twice_is_noop() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 2);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let c = &data[..CHUNK as usize];
    let first =
        f.t.put_chunk(NS, &tid, 0, &sha(c), g.epoch, body(c))
            .await
            .expect("first");
    assert!(!first.already);
    // The second send is answered without reading or writing the body.
    let empty: BoxStream<'static, io::Result<Bytes>> = stream::empty().boxed();
    let again =
        f.t.put_chunk(NS, &tid, 0, &sha(c), g.epoch, empty)
            .await
            .expect("again");
    assert!(again.already);
    assert_eq!(again.committed_chunks, 1);
    assert_eq!(
        f.t.view(NS, &tid).await.expect("view").committed_bytes,
        CHUNK
    );
}

#[tokio::test]
async fn changed_chunk_conflicts() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 2);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let c = &data[..CHUNK as usize];
    f.t.put_chunk(NS, &tid, 0, &sha(c), g.epoch, body(c))
        .await
        .expect("first");
    let changed = vec![1u8; CHUNK as usize];
    let r =
        f.t.put_chunk(NS, &tid, 0, &sha(&changed), g.epoch, body(&changed))
            .await;
    assert!(
        matches!(
            r,
            Err(Error::Conflict {
                code: "chunk_conflict",
                ..
            })
        ),
        "{r:?}"
    );
}

#[tokio::test]
async fn bad_hash_or_length_refused_and_nothing_written() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 2);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let c = &data[..CHUNK as usize];
    let wrong = sha(b"something else");
    let r = f.t.put_chunk(NS, &tid, 0, &wrong, g.epoch, body(c)).await;
    assert!(matches!(r, Err(Error::Integrity(_))), "{r:?}");
    let short = &c[..100];
    let r =
        f.t.put_chunk(NS, &tid, 0, &sha(short), g.epoch, body(short))
            .await;
    assert!(matches!(r, Err(Error::Integrity(_))), "{r:?}");
    let long = content(CHUNK + 1);
    let r =
        f.t.put_chunk(NS, &tid, 0, &sha(&long), g.epoch, body(&long))
            .await;
    assert!(matches!(r, Err(Error::Integrity(_))), "{r:?}");
    let listed = f
        .store
        .list(&format!("transfers/{NS}/{tid}/chunks/"))
        .await
        .expect("list");
    assert!(listed.is_empty(), "no chunk object may exist: {listed:?}");
    assert_eq!(f.t.view(NS, &tid).await.expect("view").committed_chunks, 0);
}

#[tokio::test]
async fn second_holder_gets_lease_held() {
    let f = fx();
    let tid = new_id();
    f.t.create(NS, &tid, req(10)).await.expect("create");
    f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let r = f.t.lease(NS, &tid, "install-2").await;
    assert!(
        matches!(r, Err(Error::LeaseHeld { ref holder, .. }) if holder == "install-1"),
        "{r:?}"
    );
    let v = f.t.view(NS, &tid).await.expect("view");
    assert_eq!(v.lease.expect("lease").holder, "install-1");
}

#[tokio::test]
async fn takeover_after_expiry_fences_old_holder() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 2);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let old = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let c0 = &data[..CHUNK as usize];
    f.t.put_chunk(NS, &tid, 0, &sha(c0), old.epoch, body(c0))
        .await
        .expect("c0");
    f.ck.advance(Duration::from_secs(31));
    let new = f.t.lease(NS, &tid, "install-2").await.expect("takeover");
    assert_eq!(new.epoch, old.epoch + 1);
    let c1 = &data[CHUNK as usize..];
    let r =
        f.t.put_chunk(NS, &tid, 1, &sha(c1), old.epoch, body(c1))
            .await;
    assert!(matches!(r, Err(Error::LeaseLost)), "{r:?}");
    // The new holder resumes from the checkpoint: chunk 0 is committed.
    let v = f.t.view(NS, &tid).await.expect("view");
    assert_eq!(v.committed, vec![[0, 1]]);
    f.t.put_chunk(NS, &tid, 1, &sha(c1), new.epoch, body(c1))
        .await
        .expect("c1");
    f.t.complete(NS, &tid, new.epoch, &sha(&data))
        .await
        .expect("complete");
}

#[tokio::test]
async fn stale_epoch_commit_refused() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let r =
        f.t.put_chunk(NS, &tid, 0, &sha(&data), g.epoch + 5, body(&data))
            .await;
    assert!(matches!(r, Err(Error::LeaseLost)), "{r:?}");
    let r = f.t.complete(NS, &tid, g.epoch + 5, &sha(&data)).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn zombie_commit_is_harmless() {
    // A dead process's request lands after the same holder resumed: the same
    // bytes go to the same key and commit the same entry.
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let again =
        f.t.lease(NS, &tid, "install-1")
            .await
            .expect("same holder again");
    assert_eq!(g.epoch, again.epoch);
    let h = sha(&data);
    let (a, b) = tokio::join!(
        f.t.put_chunk(NS, &tid, 0, &h, g.epoch, body(&data)),
        f.t.put_chunk(NS, &tid, 0, &h, g.epoch, body(&data)),
    );
    a.expect("a");
    b.expect("b");
    let v = f.t.view(NS, &tid).await.expect("view");
    assert_eq!((v.committed_chunks, v.committed_bytes), (1, CHUNK));
}

#[tokio::test]
async fn concurrent_chunks_group_commit() {
    let f = Arc::new(fx());
    let tid = new_id();
    let n = 40u64;
    let data = Arc::new(content(CHUNK * n));
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let mut tasks = Vec::new();
    for i in (0..n).rev() {
        let (f, data, tid) = (Arc::clone(&f), Arc::clone(&data), tid.clone());
        tasks.push(tokio::spawn(async move {
            let c = &data[(i * CHUNK) as usize..((i + 1) * CHUNK) as usize];
            f.t.put_chunk(NS, &tid, i as u32, &sha(c), g.epoch, body(c))
                .await
        }));
    }
    for t in tasks {
        t.await.expect("join").expect("put");
    }
    let v = f.t.view(NS, &tid).await.expect("view");
    assert_eq!(v.committed, vec![[0, n as u32]]);
    assert_eq!(v.committed_bytes, CHUNK * n);
    let segments = f
        .store
        .list(&format!("transfers/{NS}/{tid}/journal/"))
        .await
        .expect("list");
    assert!(segments.len() as u64 <= n, "at most one segment per chunk");
}

#[tokio::test]
async fn complete_requires_all_chunks_and_is_idempotent() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 2);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let c0 = &data[..CHUNK as usize];
    f.t.put_chunk(NS, &tid, 0, &sha(c0), g.epoch, body(c0))
        .await
        .expect("c0");
    let r = f.t.complete(NS, &tid, g.epoch, &sha(&data)).await;
    assert!(matches!(r, Err(Error::State(_))), "{r:?}");
    let c1 = &data[CHUNK as usize..];
    f.t.put_chunk(NS, &tid, 1, &sha(c1), g.epoch, body(c1))
        .await
        .expect("c1");
    let a =
        f.t.complete(NS, &tid, g.epoch, &sha(&data))
            .await
            .expect("complete");
    let b =
        f.t.complete(NS, &tid, g.epoch, &sha(&data))
            .await
            .expect("again");
    assert_eq!(a.root, b.root);
    let r = f.t.complete(NS, &tid, g.epoch, &sha(b"other")).await;
    assert!(matches!(r, Err(Error::Conflict { .. })), "{r:?}");
    // No more chunks or leases on a complete transfer.
    assert!(f.t.lease(NS, &tid, "install-1").await.is_err());
}

#[test]
fn root_is_order_and_length_bound() {
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let base = root([(0, 10, a.as_str()), (1, 10, b.as_str())]).expect("root");
    let swapped = root([(0, 10, b.as_str()), (1, 10, a.as_str())]).expect("root");
    let longer = root([(0, 11, a.as_str()), (1, 10, b.as_str())]).expect("root");
    assert_ne!(base, swapped);
    assert_ne!(base, longer);
}

#[tokio::test]
async fn abandon_keeps_bytes_and_refuses_more() {
    let f = fx();
    let tid = new_id();
    let data = content(CHUNK * 2);
    f.t.create(NS, &tid, req(data.len() as u64))
        .await
        .expect("create");
    let g = f.t.lease(NS, &tid, "install-1").await.expect("lease");
    let c0 = &data[..CHUNK as usize];
    f.t.put_chunk(NS, &tid, 0, &sha(c0), g.epoch, body(c0))
        .await
        .expect("c0");
    let v = f.t.abandon(NS, &tid).await.expect("abandon");
    assert_eq!(v.state, TransferState::Abandoned);
    assert_eq!(v.committed_chunks, 1);
    let c1 = &data[CHUNK as usize..];
    assert!(
        f.t.put_chunk(NS, &tid, 1, &sha(c1), g.epoch, body(c1))
            .await
            .is_err()
    );
    let listed = f.t.list(NS).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, TransferState::Abandoned);
}
