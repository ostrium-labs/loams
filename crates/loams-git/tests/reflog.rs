//! `BucketRefLog`: the sequencer and group commit (§36 §4.4–§4.6, §5.2;
//! plan GT1 Task 10, old GT1 Task 7).

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use loams_git::format::event::{TxnBody, TxnEvent, to_cloudevent};
use loams_git::format::pb;
use loams_git::limits::{MAX_FENCE_RETRIES, MAX_REFS_PER_TXN};
use loams_git::testing::{
    ManualClock, TEST_NS, ensure_repo, mem_reflog, mem_reflog_with, test_paths, test_repo,
};
use loams_git::wal::{Appended, Hint, WalError, WalStore};
use loams_git::{
    BlobId, BlobStore, BucketRefLog, Expect, GitEvent, IdemKey, ObjectId, Principal, ReadAt,
    Receipt, RefError, RefLog, RefName, RefTxn, RefUpdate, RejectReason, Seq, SequencerConfig,
    StoreBlobStore, StoreWalStore, WalBatch, WalSegment,
};
use loams_store::{Fault, Op, Store};
use sha2::{Digest, Sha256};

use support::faulty_store;

fn oid(n: u32) -> ObjectId {
    let mut bytes = [0u8; 20];
    bytes[16..].copy_from_slice(&n.to_be_bytes());
    bytes[0] = 0x11;
    ObjectId::try_from(&bytes[..]).expect("oid")
}

fn name(s: &str) -> RefName {
    RefName::parse(&format!("refs/heads/{s}")).expect("ref name")
}

fn update(branch: &str, expect: Expect, new: Option<u32>) -> RefUpdate {
    RefUpdate {
        name: name(branch),
        expect,
        new: new.map(oid),
    }
}

/// A transaction under `key`, whose digest covers the key and the updates.
fn txn(key: &str, updates: Vec<RefUpdate>) -> RefTxn {
    let mut hash = Sha256::new();
    hash.update(key.as_bytes());
    hash.update(format!("{updates:?}").as_bytes());
    RefTxn {
        key: IdemKey::parse(key).expect("key"),
        request_digest: hash.finalize().into(),
        updates,
        packs: Vec::new(),
        principal: Principal::parse("tester").expect("principal"),
        scope: None,
        trace: None,
        push_options: Vec::new(),
    }
}

fn create(key: &str, branch: &str, new: u32) -> RefTxn {
    txn(key, vec![update(branch, Expect::Absent, Some(new))])
}

async fn refs(log: &BucketRefLog) -> BTreeMap<String, ObjectId> {
    let snap = log.snapshot(ReadAt::Latest).await.expect("snapshot");
    snap.refs
        .iter()
        .map(|(name, oid)| (name.as_str().to_string(), *oid))
        .collect()
}

#[tokio::test]
async fn commit_then_snapshot_sees_it() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store.clone()).await;
    let receipt = log.commit(create("k1", "main", 1)).await.expect("commit");
    assert_eq!(
        receipt,
        Receipt {
            seq: Seq(1),
            index: 0,
            replayed: false
        }
    );
    let snap = log.snapshot(ReadAt::Latest).await.expect("snapshot");
    assert_eq!(snap.seq, Seq(1));
    assert_eq!(snap.refs.get(&name("main")), Some(&oid(1)));
    assert_eq!(
        log.snapshot(ReadAt::AtLeast(Seq(1)))
            .await
            .expect("at least")
            .refs,
        snap.refs
    );

    // Another node sees it with one more GET, and an old state exactly.
    let other = mem_reflog(store).await;
    assert_eq!(refs(&other).await, refs(&log).await);
    log.commit(create("k2", "dev", 2)).await.expect("commit");
    let latest = other.snapshot(ReadAt::Latest).await.expect("latest");
    assert_eq!(latest.seq, Seq(2), "Latest reads past the node's own state");
    for reader in [&other, &log] {
        // `other` holds seq 1; `log` holds seq 2 and replays a checkpoint.
        let at_one = reader
            .snapshot(ReadAt::Exactly(Seq(1)))
            .await
            .expect("exactly");
        assert_eq!(at_one.seq, Seq(1));
        assert!(!at_one.refs.contains_key(&name("dev")));
    }
    assert!(matches!(
        other.snapshot(ReadAt::Exactly(Seq(9))).await,
        Err(RefError::Invalid { .. })
    ));
}

/// Readers at one seq share one snapshot instead of copying every ref per
/// read (BQ1); a newer seq gets a new one, and an old exact read is not
/// served the newer one.
#[tokio::test]
async fn readers_at_one_seq_share_a_snapshot() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store).await;
    log.commit(create("k1", "main", 1)).await.expect("commit");
    let a = log.snapshot(ReadAt::AtLeast(Seq(1))).await.expect("a");
    let b = log.snapshot(ReadAt::Latest).await.expect("b");
    assert!(Arc::ptr_eq(&a, &b), "one seq, one snapshot");
    log.commit(create("k2", "dev", 2)).await.expect("commit");
    let c = log.snapshot(ReadAt::Latest).await.expect("c");
    assert_eq!(c.seq, Seq(2));
    assert!(c.refs.contains_key(&name("dev")));
    let old = log.snapshot(ReadAt::Exactly(Seq(1))).await.expect("old");
    assert_eq!(old.seq, Seq(1));
    assert!(!old.refs.contains_key(&name("dev")));
    let d = log.snapshot(ReadAt::AtLeast(Seq(2))).await.expect("d");
    assert!(
        Arc::ptr_eq(&c, &d),
        "the old read did not replace the cache"
    );
}

#[tokio::test(start_paused = true)]
async fn group_commit_batches_concurrent_txns() {
    let (faulty, store) = faulty_store();
    let log = mem_reflog(store).await;
    for _ in 0..200 {
        faulty.inject(Op::PutCreate, Fault::Delay(Duration::from_millis(50)));
    }
    let puts = faulty.calls(Op::PutCreate);
    let commits: Vec<_> = (0..200)
        .map(|i| {
            let log = log.clone();
            tokio::spawn(async move {
                log.commit(create(&format!("k{i}"), &format!("b{i}"), i))
                    .await
            })
        })
        .collect();
    for commit in commits {
        commit.await.expect("join").expect("commit");
    }
    let segments = log.seq().0;
    assert_eq!(
        faulty.calls(Op::PutCreate) - puts,
        segments,
        "one PUT per segment"
    );
    assert!(segments < 60, "200 commits took {segments} segments");
    assert_eq!(refs(&log).await.len(), 200);
}

/// Holds the next segment PUT for `delay`, so commits sent meanwhile form
/// one group.
async fn in_one_group(
    faulty: &loams_store::FaultyStore,
    log: &BucketRefLog,
    txns: Vec<RefTxn>,
) -> Vec<Result<Receipt, RefError>> {
    faulty.inject(Op::PutCreate, Fault::Delay(Duration::from_millis(200)));
    let holder = {
        let log = log.clone();
        tokio::spawn(async move { log.commit(create("holder", "holder", 999)).await })
    };
    tokio::time::sleep(Duration::from_millis(10)).await;
    let handles: Vec<_> = txns
        .into_iter()
        .map(|txn| {
            let log = log.clone();
            tokio::spawn(async move { log.commit(txn).await })
        })
        .collect();
    holder.await.expect("join").expect("holder commits");
    let mut out = Vec::new();
    for handle in handles {
        out.push(handle.await.expect("join"));
    }
    out
}

#[tokio::test(start_paused = true)]
async fn group_rejects_only_the_stale_txn() {
    let (faulty, store) = faulty_store();
    let log = mem_reflog(store).await;
    log.commit(create("k0", "main", 1)).await.expect("commit");
    let mut events = log.events();
    let results = in_one_group(
        &faulty,
        &log,
        vec![
            create("ka", "a", 10),
            txn("kb", vec![update("main", Expect::Oid(oid(7)), Some(11))]),
            // Sees `a` created by the transaction before it in the group.
            txn("kc", vec![update("a", Expect::Oid(oid(10)), Some(12))]),
        ],
    )
    .await;
    let a = results[0].as_ref().expect("a commits");
    let c = results[2].as_ref().expect("c commits");
    assert_eq!((a.seq, a.index), (Seq(3), 0));
    assert_eq!((c.seq, c.index), (Seq(3), 1), "one segment for the group");
    match &results[1] {
        Err(RefError::Rejected(rejected)) => assert_eq!(
            rejected,
            &vec![(
                name("main"),
                RejectReason::Stale {
                    current: Some(oid(1))
                }
            )]
        ),
        other => panic!("{other:?}"),
    }
    let refs = refs(&log).await;
    assert_eq!(refs["refs/heads/a"], oid(12));
    assert_eq!(refs["refs/heads/main"], oid(1));
    let mut sizes = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let GitEvent::GroupSize(n) = event {
            sizes.push(n);
        }
    }
    assert_eq!(sizes, vec![1, 3], "the holder alone, then the three");
}

/// Two copies of one transaction in one group: the second waits for the
/// first's segment, then replays it; and when the first's outcome is not
/// learned, the second is never told it committed at that seq (PR #452
/// review).
#[tokio::test(start_paused = true)]
async fn duplicate_key_in_one_group_waits_for_its_segment() {
    let (faulty, store) = faulty_store();
    let log = mem_reflog(store).await;
    let results = in_one_group(
        &faulty,
        &log,
        vec![create("dup", "main", 1), create("dup", "main", 1)],
    )
    .await;
    let first = results[0].as_ref().expect("first");
    let second = results[1].as_ref().expect("second");
    assert!(!first.replayed);
    assert_eq!(
        *second,
        Receipt {
            replayed: true,
            ..*first
        }
    );

    // The group's append fails before anything is stored, every time.
    faulty.inject(Op::PutCreate, Fault::Delay(Duration::from_millis(200)));
    let holder = {
        let log = log.clone();
        tokio::spawn(async move { log.commit(create("holder2", "holder2", 998)).await })
    };
    tokio::time::sleep(Duration::from_millis(10)).await;
    for _ in 0..5 {
        faulty.inject(Op::PutCreate, Fault::Error);
    }
    let copies: Vec<_> = (0..2)
        .map(|_| {
            let log = log.clone();
            tokio::spawn(async move { log.commit(create("lost", "lost", 7)).await })
        })
        .collect();
    holder.await.expect("join").expect("holder");
    let mut answers = Vec::new();
    for copy in copies {
        answers.push(copy.await.expect("join"));
    }
    assert!(
        matches!(answers[0], Err(RefError::Unavailable)),
        "{:?}",
        answers[0]
    );
    // The second copy was decided after that attempt: it committed itself.
    let receipt = answers[1].as_ref().expect("the second copy commits");
    assert!(!receipt.replayed);
    let snap = log.snapshot(ReadAt::Latest).await.expect("snapshot");
    assert_eq!(snap.seq, receipt.seq);
    assert_eq!(snap.refs.get(&name("lost")), Some(&oid(7)));
}

#[tokio::test]
async fn atomic_multi_ref_all_or_none() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store).await;
    let both = txn(
        "k1",
        vec![
            update("a", Expect::Absent, Some(1)),
            update("b", Expect::Absent, Some(2)),
        ],
    );
    let receipt = log.commit(both).await.expect("commit");
    assert_eq!(receipt.seq, Seq(1));

    let half_stale = txn(
        "k2",
        vec![
            update("a", Expect::Oid(oid(1)), Some(3)),
            update("b", Expect::Oid(oid(9)), Some(4)),
            update("c", Expect::Absent, Some(5)),
        ],
    );
    match log.commit(half_stale).await {
        Err(RefError::Rejected(rejected)) => assert_eq!(
            rejected,
            vec![(
                name("b"),
                RejectReason::Stale {
                    current: Some(oid(2))
                }
            )],
            "only the refused ref is listed"
        ),
        other => panic!("{other:?}"),
    }
    let refs = refs(&log).await;
    assert_eq!(refs.len(), 2, "nothing of the refused transaction applied");
    assert_eq!(refs["refs/heads/a"], oid(1));
    assert_eq!(log.seq(), Seq(1), "a refused transaction writes no segment");
}

#[tokio::test(start_paused = true)]
async fn lost_ack_resolves_to_committed() {
    let (faulty, store) = faulty_store();
    let log = mem_reflog(store.clone()).await;

    // The PUT applied but its answer was lost: the WAL reads its own batch
    // back, and the commit is acknowledged once.
    faulty.inject(Op::PutCreate, Fault::ErrorAfterApply);
    let receipt = log.commit(create("k1", "main", 1)).await.expect("commit");
    assert_eq!((receipt.seq, receipt.replayed), (Seq(1), false));
    assert_eq!(log.seq(), Seq(1));

    // The PUT applied and no read-back succeeds: the commit is answered
    // `Unavailable`, and its retry with the same key replays the receipt.
    faulty.inject(Op::PutCreate, Fault::ErrorAfterApply);
    for _ in 0..64 {
        faulty.inject(Op::Get, Fault::Error);
    }
    let lost = log.commit(create("k2", "dev", 2)).await;
    assert!(matches!(lost, Err(RefError::Unavailable)), "{lost:?}");
    faulty.clear();
    assert_eq!(log.seq(), Seq(1), "the sequencer does not know yet");
    let retried = log.commit(create("k2", "dev", 2)).await.expect("retry");
    assert_eq!(
        retried,
        Receipt {
            seq: Seq(2),
            index: 0,
            replayed: true
        }
    );
    assert_eq!(log.seq(), Seq(2));
    assert_eq!(refs(&log).await.len(), 2, "applied once");
}

#[tokio::test]
async fn fenced_group_revalidates() {
    let (_, store) = faulty_store();
    let a = mem_reflog(store.clone()).await;
    let b = mem_reflog(store.clone()).await;
    a.commit(create("ka", "main", 1)).await.expect("a commits");

    // b still holds seq 0: its append at seq 1 is fenced, it applies a's
    // segment, and the create is refused against the new state.
    let mut events = b.events();
    match b.commit(create("kb", "main", 2)).await {
        Err(RefError::Rejected(rejected)) => {
            assert_eq!(rejected, vec![(name("main"), RejectReason::Exists)]);
        }
        other => panic!("{other:?}"),
    }
    let mut fenced = false;
    while let Ok(event) = events.try_recv() {
        fenced |= matches!(event, GitEvent::Fenced { at } if at == Seq(1));
    }
    assert!(fenced, "the fence is reported");
    assert_eq!(b.seq(), Seq(1));

    // A transaction that still applies after the fence is committed at the
    // new tail.
    let c = mem_reflog(store).await;
    a.commit(create("ka2", "x", 3)).await.expect("a commits");
    let receipt = c.commit(create("kc", "y", 4)).await.expect("c commits");
    assert_eq!(receipt.seq, Seq(3));
    assert_eq!(refs(&a).await.len(), 3);
}

/// A refusal is decided against the tail, not against a stale state
/// (ruling T10.2): found while running `two_sequencers_one_store_linearizable`.
#[tokio::test]
async fn rejection_is_decided_at_the_tail() {
    let (_, store) = faulty_store();
    let a = mem_reflog(store.clone()).await;
    let b = mem_reflog(store).await;
    a.commit(create("ka", "main", 1)).await.expect("a commits");
    // b still holds seq 0, where `main` does not exist; at the tail it
    // points at 1, so the update applies.
    let receipt = b
        .commit(txn(
            "kb",
            vec![update("main", Expect::Oid(oid(1)), Some(2))],
        ))
        .await
        .expect("decided at the tail");
    assert_eq!(receipt.seq, Seq(2));
    // And a refusal names the tail's value.
    match a
        .commit(txn(
            "ka2",
            vec![update("main", Expect::Oid(oid(1)), Some(3))],
        ))
        .await
    {
        Err(RefError::Rejected(rejected)) => assert_eq!(
            rejected,
            vec![(
                name("main"),
                RejectReason::Stale {
                    current: Some(oid(2))
                }
            )]
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn replay_returns_original_receipt() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store.clone()).await;
    let first = log.commit(create("k1", "main", 1)).await.expect("commit");
    log.commit(create("k2", "dev", 2)).await.expect("commit");
    let again = log.commit(create("k1", "main", 1)).await.expect("replay");
    assert_eq!(
        again,
        Receipt {
            replayed: true,
            ..first
        }
    );
    assert_eq!(log.seq(), Seq(2), "a replay writes nothing");
    assert!(matches!(
        log.commit(create("k1", "main", 5)).await,
        Err(RefError::IdempotencyMismatch)
    ));

    // cas_ref binds the key to its update the same way.
    let key = IdemKey::parse("cas-1").expect("key");
    let cas = log
        .cas_ref(name("tag"), Expect::Absent, Some(oid(3)), key.clone())
        .await
        .expect("cas");
    let replay = log
        .cas_ref(name("tag"), Expect::Absent, Some(oid(3)), key.clone())
        .await
        .expect("cas replay");
    assert_eq!(
        replay,
        Receipt {
            replayed: true,
            ..cas
        }
    );
    assert!(matches!(
        log.cas_ref(name("tag"), Expect::Absent, Some(oid(4)), key)
            .await,
        Err(RefError::IdempotencyMismatch)
    ));

    // A restarted sequencer rebuilds the window from the segments.
    log.shutdown().await;
    let reopened = mem_reflog(store).await;
    assert_eq!(
        reopened
            .commit(create("k1", "main", 1))
            .await
            .expect("replay"),
        Receipt {
            replayed: true,
            ..first
        }
    );
}

#[tokio::test(start_paused = true)]
async fn snapshot_is_a_prefix_of_commits() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store).await;
    let clock = Arc::new(AtomicU64::new(0));
    let writers: Vec<_> = (0..4u32)
        .map(|w| {
            let (log, clock) = (log.clone(), clock.clone());
            tokio::spawn(async move {
                let mut done = Vec::new();
                for j in 0..25u32 {
                    let branch = format!("w{w}-{j}");
                    let receipt = log
                        .commit(create(&format!("k{w}-{j}"), &branch, w * 100 + j))
                        .await
                        .expect("commit");
                    done.push((branch, receipt.seq, clock.fetch_add(1, Ordering::SeqCst)));
                }
                done
            })
        })
        .collect();
    let reader = {
        let (log, clock) = (log.clone(), clock.clone());
        tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..60 {
                let invoked = clock.fetch_add(1, Ordering::SeqCst);
                let snap = log.snapshot(ReadAt::Latest).await.expect("snapshot");
                seen.push((invoked, snap));
                tokio::task::yield_now().await;
            }
            seen
        })
    };
    let mut commits = Vec::new();
    for writer in writers {
        commits.extend(writer.await.expect("join"));
    }
    for (invoked, snap) in reader.await.expect("join") {
        let expected: Vec<String> = {
            let mut names: Vec<String> = commits
                .iter()
                .filter(|(_, seq, _)| *seq <= snap.seq)
                .map(|(branch, _, _)| format!("refs/heads/{branch}"))
                .collect();
            names.sort();
            names
        };
        let got: Vec<String> = snap.refs.keys().map(|n| n.as_str().to_string()).collect();
        assert_eq!(got, expected, "the snapshot at {} is the prefix", snap.seq);
        for (_, seq, completed) in &commits {
            if *completed < invoked {
                assert!(*seq <= snap.seq, "an acknowledged commit is visible");
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn watch_has_no_gaps_across_fences() {
    let (_, store) = faulty_store();
    let a = mem_reflog(store.clone()).await;
    let b = mem_reflog(store).await;
    let mut stream = a.watch(Seq(1));
    let mut keys = Vec::new();
    for i in 0..10u32 {
        let writer = if i % 3 == 0 { &b } else { &a };
        let key = format!("k{i}");
        writer
            .commit(create(&key, &format!("b{i}"), i))
            .await
            .expect("commit");
        keys.push(key);
    }
    let mut seen = Vec::new();
    while seen.len() < 10 {
        let txn = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("the stream keeps up")
            .expect("open")
            .expect("item");
        seen.push((txn.seq, txn.index, txn.event.key.as_str().to_string()));
    }
    let expected: Vec<(Seq, u32, String)> = keys
        .iter()
        .enumerate()
        .map(|(i, key)| (Seq(i as u64 + 1), 0, key.clone()))
        .collect();
    assert_eq!(seen, expected, "every seq, in order, once");

    // A watch resumes from any seq.
    let resumed: Vec<Seq> = a
        .watch(Seq(8))
        .take(3)
        .map(|txn| txn.expect("item").seq)
        .collect()
        .await;
    assert_eq!(resumed, vec![Seq(8), Seq(9), Seq(10)]);
}

#[tokio::test]
async fn missing_pack_is_refused_before_queueing() {
    let (faulty, store) = faulty_store();
    let log = mem_reflog(store.clone()).await;
    let checksum = "ab".repeat(20);
    let pack = pb::PackRef {
        checksum: checksum.clone(),
        object_count: 1,
        pack_len: 3,
        idx_offset: 3,
        idx_len: 3,
        has_bitmap: false,
    };
    let mut with_pack = create("k1", "main", 1);
    with_pack.packs = vec![pack.clone()];
    let puts = faulty.calls(Op::PutCreate);
    match log.commit(with_pack.clone()).await {
        Err(RefError::Wal(WalError::Store(loams_store::StoreError::NotFound { .. }))) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(faulty.calls(Op::PutCreate), puts, "nothing was written");
    assert_eq!(log.seq(), Seq(0));

    let blobs = StoreBlobStore::new(store, test_paths());
    let id = BlobId::parse(&checksum).expect("id");
    blobs
        .put(&id, Bytes::from_static(b"pack bytes"))
        .await
        .expect("put");
    log.commit(with_pack).await.expect("commit");
    let snap = log.snapshot(ReadAt::Latest).await.expect("snapshot");
    assert_eq!(snap.packs, vec![pack]);
}

#[tokio::test(start_paused = true)]
async fn shutdown_answers_unavailable() {
    let (faulty, store) = faulty_store();
    let log = mem_reflog(store.clone()).await;
    faulty.inject(Op::PutCreate, Fault::Delay(Duration::from_millis(100)));
    let first = {
        let log = log.clone();
        tokio::spawn(async move { log.commit(create("k0", "main", 1)).await })
    };
    tokio::time::sleep(Duration::from_millis(10)).await;
    let queued: Vec<_> = (1..4u32)
        .map(|i| {
            let log = log.clone();
            tokio::spawn(async move {
                log.commit(create(&format!("k{i}"), &format!("b{i}"), i))
                    .await
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(10)).await;
    log.clone().shutdown().await;

    assert_eq!(first.await.expect("join").expect("in flight").seq, Seq(1));
    for handle in queued {
        assert!(matches!(
            handle.await.expect("join"),
            Err(RefError::Unavailable)
        ));
    }
    assert!(matches!(
        log.commit(create("k9", "late", 9)).await,
        Err(RefError::Unavailable)
    ));
    let wal = StoreWalStore::new(store, test_paths());
    let hint = wal.hint().await.expect("hint");
    assert_eq!(
        (hint.seq, hint.checkpoint),
        (Seq(1), Seq(0)),
        "the last hint is written"
    );
}

/// A WAL where another writer takes the next `remaining` seqs just before
/// each of this sequencer's appends.
#[derive(Debug)]
struct Rival {
    inner: StoreWalStore,
    remaining: AtomicUsize,
}

#[async_trait]
impl WalStore for Rival {
    async fn append(&self, expect_seq: Seq, batch: &WalBatch) -> Result<Appended, WalError> {
        if self
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            let key = format!("rival-{expect_seq}");
            let body = pb::RefTransaction {
                updates: vec![pb::RefUpdate {
                    name: format!("refs/heads/{key}"),
                    expect: Some(pb::ref_update::Expect::MustNotExist(true)),
                    new_oid: oid(1).as_bytes().to_vec(),
                }],
                packs: Vec::new(),
                principal: "rival".into(),
                push_options: Vec::new(),
                object_format: pb::ObjectFormat::Sha1 as i32,
                request_digest: vec![7; 32],
            };
            let event = TxnEvent {
                ns: TEST_NS,
                repo: test_repo(),
                tenant: "org/1".into(),
                key: IdemKey::parse(&key).expect("key"),
                trace: None,
                time_unix_ms: 0,
                body: TxnBody::RefTxn(body),
            };
            let rival = WalBatch {
                batch_id: [9; 16],
                events: vec![to_cloudevent(&event, expect_seq)],
            };
            self.inner.append(expect_seq, &rival).await?;
        }
        self.inner.append(expect_seq, batch).await
    }

    async fn read_from(&self, from: Seq, max: usize) -> Result<Vec<WalSegment>, WalError> {
        self.inner.read_from(from, max).await
    }

    async fn hint(&self) -> Result<Hint, WalError> {
        self.inner.hint().await
    }

    async fn publish_hint(&self, hint: &Hint) -> Result<(), WalError> {
        self.inner.publish_hint(hint).await
    }

    async fn put_checkpoint(&self, checkpoint: &pb::Checkpoint) -> Result<(), WalError> {
        self.inner.put_checkpoint(checkpoint).await
    }

    async fn latest_checkpoint(
        &self,
        at_or_below: Seq,
    ) -> Result<Option<pb::Checkpoint>, WalError> {
        self.inner.latest_checkpoint(at_or_below).await
    }
}

async fn rival_log(fences: usize) -> BucketRefLog {
    let (_, store): (_, Store) = faulty_store();
    let wal = Arc::new(Rival {
        inner: StoreWalStore::new(store.clone(), test_paths()),
        remaining: AtomicUsize::new(fences),
    });
    ensure_repo(wal.as_ref()).await.expect("repo");
    let blobs = Arc::new(StoreBlobStore::new(store, test_paths()));
    let config = SequencerConfig::new("org/1", TEST_NS, test_repo());
    BucketRefLog::open(wal, blobs, config, Arc::new(ManualClock::new(1_000)))
        .await
        .expect("open")
}

#[tokio::test]
async fn fence_retry_limit_is_unavailable() {
    // At the limit: fenced 16 times, then committed.
    let log = rival_log(MAX_FENCE_RETRIES).await;
    let receipt = log.commit(create("k", "main", 1)).await.expect("commit");
    assert_eq!(receipt.seq, Seq(MAX_FENCE_RETRIES as u64 + 1));

    // One past it: answered `Unavailable`, and nothing of it applied.
    let log = rival_log(MAX_FENCE_RETRIES + 1).await;
    assert!(matches!(
        log.commit(create("k", "main", 1)).await,
        Err(RefError::Unavailable)
    ));
    assert_eq!(log.seq(), Seq(MAX_FENCE_RETRIES as u64 + 1));
    assert!(!refs(&log).await.contains_key("refs/heads/main"));
}

#[tokio::test]
async fn transaction_size_is_bounded() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store).await;
    let updates = |n: usize| -> Vec<RefUpdate> {
        (0..n)
            .map(|i| update(&format!("b{i:05}"), Expect::Absent, Some(i as u32)))
            .collect()
    };
    assert!(matches!(
        log.commit(txn("empty", Vec::new())).await,
        Err(RefError::Invalid { .. })
    ));
    assert!(matches!(
        log.commit(txn("over", updates(MAX_REFS_PER_TXN + 1))).await,
        Err(RefError::Invalid { .. })
    ));
    let receipt = log
        .commit(txn("at", updates(MAX_REFS_PER_TXN)))
        .await
        .expect("4096 updates commit");
    assert_eq!(receipt.seq, Seq(1));
    assert_eq!(refs(&log).await.len(), MAX_REFS_PER_TXN);

    let mut huge = create("huge", "huge", 1);
    huge.push_options = vec!["x".repeat(1024 * 1024)];
    assert!(matches!(log.commit(huge).await, Err(RefError::TooLarge)));
}

#[tokio::test(start_paused = true)]
async fn events_carry_the_apply_time() {
    // Every clock read differs, so a sequencer that stamped events with
    // another read than it applied with would restore other expiries.
    let (_, store) = faulty_store();
    let clock = Arc::new(ManualClock::stepping(1_000_000, 7));
    let log = mem_reflog_with(store.clone(), clock.clone(), |_| {}).await;
    let commits: Vec<_> = (0..20u32)
        .map(|i| {
            let log = log.clone();
            tokio::spawn(async move {
                log.commit(create(&format!("k{i}"), &format!("b{i}"), i))
                    .await
            })
        })
        .collect();
    for commit in commits {
        commit.await.expect("join").expect("commit");
    }
    let replayed = mem_reflog_with(store, clock, |_| {}).await;
    assert_eq!(replayed.seq(), log.seq());
    assert_eq!(log.checkpoint(1_000_000), replayed.checkpoint(1_000_000));
    assert_eq!(log.checkpoint(1_000_000).idempotency.len(), 20);
}

fn head_to(branch: &str, protections: Vec<pb::Protection>) -> pb::ConfigChange {
    pb::ConfigChange {
        symrefs: vec![pb::SymRef {
            name: "HEAD".into(),
            target: format!("refs/heads/{branch}"),
        }],
        protections,
        ..Default::default()
    }
}

/// A configuration change commits through the sequencer under an
/// idempotency key (GT1 Task 13, ruling T13.2): a retry replays its receipt,
/// also after a restart rebuilds the state from the segment, and the key
/// with another change is a mismatch.
#[tokio::test]
async fn commit_config_sets_head_and_replays() {
    let (_, store) = faulty_store();
    let log = mem_reflog(store.clone()).await;
    log.commit(create("k1", "main", 1)).await.expect("commit");
    let rule = pb::Protection {
        pattern: "refs/heads/main".into(),
        deny_delete: true,
        deny_force: true,
    };
    let key = IdemKey::parse("cfg-1").expect("key");
    let receipt = log
        .commit_config(key.clone(), head_to("trunk", vec![rule.clone()]))
        .await
        .expect("config");
    assert_eq!(
        receipt,
        Receipt {
            seq: Seq(2),
            index: 0,
            replayed: false
        }
    );
    let snap = log.snapshot(ReadAt::Latest).await.expect("snapshot");
    assert_eq!(snap.symrefs["HEAD"], name("trunk"));
    assert_eq!(snap.protections, vec![rule.clone()]);
    // The rule now refuses deleting main.
    let err = log
        .commit(txn("k2", vec![update("main", Expect::Oid(oid(1)), None)]))
        .await
        .expect_err("protected");
    assert!(matches!(err, RefError::Rejected(_)), "{err:?}");

    let again = log
        .commit_config(key.clone(), head_to("trunk", vec![rule.clone()]))
        .await
        .expect("replay");
    assert_eq!(
        again,
        Receipt {
            replayed: true,
            ..receipt
        }
    );
    let other = log
        .commit_config(key.clone(), head_to("dev", Vec::new()))
        .await
        .expect_err("another change under the key");
    assert!(matches!(err_kind(&other), "mismatch"), "{other:?}");
    log.shutdown().await;

    // A restart rebuilds the window from the segment.
    let reopened = mem_reflog(store).await;
    let replayed = reopened
        .commit_config(key, head_to("trunk", vec![rule]))
        .await
        .expect("replay after restart");
    assert_eq!(
        replayed,
        Receipt {
            replayed: true,
            ..receipt
        }
    );
    // A target that is not a ref name is refused before queueing.
    let mut bad = head_to("trunk", Vec::new());
    bad.symrefs[0].target = "not a ref".into();
    let err = reopened
        .commit_config(IdemKey::parse("cfg-2").expect("key"), bad)
        .await
        .expect_err("invalid");
    assert!(matches!(err, RefError::Invalid { .. }), "{err:?}");
    assert_eq!(reopened.seq(), Seq(2));
}

fn err_kind(err: &RefError) -> &'static str {
    match err {
        RefError::IdempotencyMismatch => "mismatch",
        _ => "other",
    }
}
