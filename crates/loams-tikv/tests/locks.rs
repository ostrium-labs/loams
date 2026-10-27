//! Locks a crashed writer leaves behind (the nightly TiKV crash gate's
//! "transaction conflicts outlasted the retries" and "no acknowledged op after
//! the restart"). A writer that dies mid-transaction leaves its pessimistic
//! locks, which TiKV v8.5 keeps in memory, and its prewrite locks. Once their
//! TTL (20 s for a pessimistic transaction) passes, the next writer or reader
//! must resolve them and go on; before the `tikv-client` pin move of
//! 2026-10-21 the secondary pessimistic locks stayed until garbage collection.
//! A reader does not even wait for the TTL: it pushes the crashed writer's
//! commit timestamp above its read and reads past the locks.
//!
//! A crash is a transaction whose rollback the `before-rollback` failpoint
//! suppresses: the runner drops it, its heartbeat stops, and its locks stay
//! with nobody to renew or release them. Every test needs a cluster and skips
//! without `LOAMS_TEST_PD`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use loams_tikv::testing::{self, TEST_META};
use loams_tikv::{CommitMode, Tikv, Timestamp, Txn, TxnError, TxnOptions};

/// Failpoints are process-wide: the crashes run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A pessimistic transaction's lock TTL (`tikv-client`'s `MAX_TTL`), from its
/// start timestamp.
const PESSIMISTIC_TTL: Duration = Duration::from_secs(20);

/// How long a writer gets after the crashed writer's TTL has passed.
const AFTER_TTL: Duration = Duration::from_secs(10);

/// Runs `body` in a pessimistic two-phase transaction that "crashes": the
/// body fails and the rollback is suppressed, so its locks stay. Returns its
/// start timestamp. With `after_prewrite`, the crash comes after the
/// prewrite instead (the commit fails there and its rollback is suppressed).
async fn crash<F>(tikv: &Tikv, after_prewrite: bool, body: F) -> Timestamp
where
    F: for<'t> Fn(&'t mut Txn) -> BoxFuture<'t, Result<(), TxnError>> + Send + Sync + 'static,
{
    let body = Arc::new(body);
    let start = Arc::new(Mutex::new(None));
    fail::cfg("before-rollback", "return").expect("failpoint");
    if after_prewrite {
        fail::cfg("after-prewrite", "return").expect("failpoint");
    }
    let opts = TxnOptions {
        max_attempts: 1,
        commit_mode: Some(CommitMode::TwoPc),
        ..TxnOptions::pessimistic("test.crashed_writer")
    };
    let res = tikv
        .run(opts, |txn| {
            let (body, start) = (body.clone(), start.clone());
            Box::pin(async move {
                body(txn).await?;
                *start.lock().expect("start") = Some(txn.start_ts());
                if after_prewrite {
                    Ok(())
                } else {
                    Err(TxnError::Fatal("the writer crashes".into()))
                }
            })
        })
        .await;
    fail::remove("after-prewrite");
    fail::remove("before-rollback");
    assert!(res.is_err(), "the crashed writer committed: {res:?}");
    let start = start.lock().expect("start").clone();
    start.expect("the crashed writer ran its body")
}

/// Sleeps until the crashed writer's locks have expired.
async fn until_expired(tikv: &Tikv, start: &Timestamp) {
    let expiry_ms =
        Tikv::physical_ms(start) + u64::try_from(PESSIMISTIC_TTL.as_millis()).expect("a TTL in ms");
    loop {
        let now = Tikv::physical_ms(&tikv.now().await.expect("now"));
        if now > expiry_ms + 500 {
            return;
        }
        tokio::time::sleep(Duration::from_millis((expiry_ms + 500 - now).min(1000))).await;
    }
}

/// A pessimistic write of `keys`, as the metastore's WAL commit does (lock,
/// then write): it must commit within [`AFTER_TTL`].
async fn write_pessimistic(tikv: &Tikv, keys: &[&'static [u8]], value: &'static [u8]) {
    let begun = Instant::now();
    let keys: Arc<Vec<&'static [u8]>> = Arc::new(keys.to_vec());
    let opts = TxnOptions {
        max_attempts: 64,
        deadline: AFTER_TTL,
        commit_mode: Some(CommitMode::TwoPc),
        ..TxnOptions::pessimistic("test.after_the_crash")
    };
    let res = tikv
        .run(opts, |txn| {
            let keys = keys.clone();
            Box::pin(async move {
                txn.batch_get_for_update(keys.iter()).await?;
                for key in keys.iter() {
                    txn.put(key, value).await?;
                }
                Ok(())
            })
        })
        .await;
    assert!(
        res.is_ok(),
        "a write after the crashed writer's TTL failed after {:?}: {res:?}",
        begun.elapsed()
    );
}

/// An optimistic write of `key` (a lease renewal's shape): it must commit
/// within [`AFTER_TTL`].
async fn write_optimistic(tikv: &Tikv, key: &'static [u8], value: &'static [u8]) {
    let begun = Instant::now();
    let opts = TxnOptions {
        deadline: AFTER_TTL,
        max_attempts: 64,
        commit_mode: Some(CommitMode::TwoPc),
        ..TxnOptions::new("test.after_the_crash")
    };
    let res = tikv
        .run(opts, |txn| {
            Box::pin(async move {
                txn.get(key).await?;
                txn.put(key, value).await
            })
        })
        .await;
    assert!(
        res.is_ok(),
        "an optimistic write after the crashed writer's TTL failed after {:?}: {res:?}",
        begun.elapsed()
    );
}

/// The value of `key` at a fresh timestamp, read within [`AFTER_TTL`].
async fn read(tikv: &Tikv, key: &[u8]) -> Option<Vec<u8>> {
    let deadline = Instant::now() + AFTER_TTL;
    loop {
        let mut snap = tikv
            .snapshot(tikv.now().await.expect("now"))
            .await
            .expect("a snapshot");
        match snap.get(key).await {
            Ok(value) => return value,
            Err(e) if Instant::now() < deadline => {
                eprintln!("read retried: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("a read after the crashed writer's TTL failed: {e}"),
        }
    }
}

/// Locks left under the handle's root (pessimistic ones included: ScanLock
/// sees TiKV's in-memory pessimistic locks).
async fn locks_left(tikv: &Tikv) -> usize {
    let now = tikv.now().await.expect("now");
    tikv.locks_below(&now).await.expect("scan locks")
}

/// Waits until no lock is left under the handle's root: a commit returns
/// before its secondaries are committed, so the writers' own locks may
/// linger for a moment.
async fn no_locks_left(tikv: &Tikv) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = locks_left(tikv).await;
        if left == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{left} lock(s) left after the writes"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A writer dies holding pessimistic locks on a primary and on secondaries,
/// as the metastore's WAL commit does when the process is killed between
/// its locks and its commit. After the TTL, a writer that locks the primary
/// first and then the secondaries (the restarted server's next WAL commit)
/// and an optimistic writer (a lease renewal) both go through, and no lock
/// is left. Before the fix, the first resolution rolled back only the
/// primary (TiKV's `PessimisticRollBack`) and the region-wide ResolveLock
/// that followed missed the in-memory secondaries; every later encounter
/// then found no primary (`TxnNotFound`), which the client did not
/// recognise, so the secondaries stayed locked until GC.
#[tokio::test]
async fn pessimistic_locks_of_a_crashed_writer_are_resolved_after_their_ttl() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let tikv = cluster.connect(TEST_META).await;

    let start = crash(&tikv, false, |txn| {
        Box::pin(async move {
            // The primary is the first key locked.
            txn.get_for_update(b"head").await?;
            txn.batch_get_for_update([b"record".as_slice(), b"live".as_slice()])
                .await?;
            txn.put(b"entry", b"crashed").await?;
            txn.get_for_update(b"lease").await?;
            Ok(())
        })
    })
    .await;
    assert_eq!(locks_left(&tikv).await, 5, "the crashed writer's locks");

    until_expired(&tikv, &start).await;
    // Reads were never blocked by pessimistic locks.
    assert_eq!(read(&tikv, b"entry").await, None);
    write_pessimistic(&tikv, &[b"head"], b"1").await;
    write_pessimistic(&tikv, &[b"record", b"live", b"entry"], b"2").await;
    write_optimistic(&tikv, b"lease", b"3").await;
    no_locks_left(&tikv).await;
    assert_eq!(
        read(&tikv, b"entry").await.as_deref(),
        Some(b"2".as_slice())
    );
}

/// The same with the secondaries met first: the writer that resolves the
/// crashed transaction never touches its primary.
#[tokio::test]
async fn secondary_pessimistic_locks_of_a_crashed_writer_are_resolved_first() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let tikv = cluster.connect(TEST_META).await;

    let start = crash(&tikv, false, |txn| {
        Box::pin(async move {
            txn.get_for_update(b"head").await?;
            txn.put(b"entry-1", b"crashed").await?;
            txn.put(b"entry-2", b"crashed").await?;
            Ok(())
        })
    })
    .await;
    assert_eq!(locks_left(&tikv).await, 3, "the crashed writer's locks");

    until_expired(&tikv, &start).await;
    write_pessimistic(&tikv, &[b"entry-1"], b"1").await;
    write_optimistic(&tikv, b"entry-2", b"2").await;
    write_pessimistic(&tikv, &[b"head"], b"3").await;
    no_locks_left(&tikv).await;
}

/// A writer dies after its prewrite, before its commit: its locks are
/// prewrite locks now, and a reader as well as a writer resolve them after
/// the TTL (they roll the transaction back: its primary is not committed).
#[tokio::test]
async fn prewrite_locks_of_a_crashed_pessimistic_writer_are_resolved_after_their_ttl() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let tikv = cluster.connect(TEST_META).await;

    let start = crash(&tikv, true, |txn| {
        Box::pin(async move {
            txn.get_for_update(b"head").await?;
            txn.put(b"head", b"crashed").await?;
            txn.put(b"entry", b"crashed").await?;
            Ok(())
        })
    })
    .await;
    assert_eq!(locks_left(&tikv).await, 2, "the crashed writer's locks");

    until_expired(&tikv, &start).await;
    assert_eq!(read(&tikv, b"entry").await, None, "a rolled-back write");
    write_pessimistic(&tikv, &[b"head", b"entry"], b"1").await;
    no_locks_left(&tikv).await;
    assert_eq!(read(&tikv, b"head").await.as_deref(), Some(b"1".as_slice()));
}

/// Reads do not wait for a crashed writer's prewrite locks to expire: the
/// reader pushes the writer's commit timestamp above its read
/// (`CheckTxnStatus`) and reads past its locks, as client-go does. Before
/// the fork's `fix/read-past-pushed-locks`, every read of these keys waited
/// for the 20 s TTL, so the metastore's reads gave up ("transaction
/// conflicts outlasted the retries") right after a crash.
#[tokio::test]
async fn reads_go_past_a_crashed_writers_prewrite_locks_at_once() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let tikv = cluster.connect(TEST_META).await;
    write_pessimistic(&tikv, &[b"head", b"entry"], b"before").await;

    let start = crash(&tikv, true, |txn| {
        Box::pin(async move {
            txn.get_for_update(b"head").await?;
            txn.put(b"head", b"crashed").await?;
            txn.put(b"entry", b"crashed").await?;
            Ok(())
        })
    })
    .await;
    assert_eq!(locks_left(&tikv).await, 2, "the crashed writer's locks");

    // Well inside the TTL: one snapshot read per key, no retry.
    let begun = Instant::now();
    for key in [b"entry".as_slice(), b"head".as_slice()] {
        let mut snap = tikv
            .snapshot(tikv.now().await.expect("now"))
            .await
            .expect("a snapshot");
        let value = snap.get(key).await.expect("a read past the locks");
        assert_eq!(value.as_deref(), Some(b"before".as_slice()));
    }
    let mut snap = tikv
        .snapshot(tikv.now().await.expect("now"))
        .await
        .expect("a snapshot");
    let pairs = snap
        .scan(b"", None, 10)
        .await
        .expect("a scan past the locks");
    assert_eq!(pairs.len(), 2, "{pairs:?}");
    let read_in = begun.elapsed();
    let since_crash = Tikv::physical_ms(&tikv.now().await.expect("now"))
        .saturating_sub(Tikv::physical_ms(&start));
    assert!(
        since_crash < 10_000,
        "the reads took {read_in:?}, {since_crash} ms after the crash: they waited for the TTL"
    );

    // The writer that comes after the TTL still rolls the crashed one back.
    until_expired(&tikv, &start).await;
    write_pessimistic(&tikv, &[b"head", b"entry"], b"after").await;
    no_locks_left(&tikv).await;
}
