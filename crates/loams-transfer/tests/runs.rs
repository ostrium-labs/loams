//! Job runs against the in-memory checkpoints and bucket (DQ1 Task 1).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use loams_common::NamespaceId;
use loams_store::Store;
use loams_transfer::keys::new_id;
use loams_transfer::{Error, MemCheckpoints, RunOutcome, Runs};

const NS: NamespaceId = NamespaceId(3);

fn runs() -> (MemCheckpoints, Runs) {
    let ck = MemCheckpoints::new();
    let r = Runs::new(
        Store::in_memory(),
        Arc::new(ck.clone()),
        Duration::from_secs(30),
    );
    (ck, r)
}

fn jobs(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

async fn read_all(r: &Runs, rid: &str, from: u64) -> (Vec<u8>, bool) {
    let mut got = r.read_log(NS, rid, from).await.expect("read");
    let mut out = Vec::new();
    while let Some(b) = got.bytes.next().await {
        out.extend_from_slice(&b.expect("bytes"));
    }
    assert_eq!(out.len() as u64, got.committed - from);
    (out, got.done)
}

#[tokio::test]
async fn claim_is_exclusive_and_filters_jobs() {
    let (_, r) = runs();
    let rid = new_id();
    r.create(NS, &rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    assert!(
        r.claim(NS, "runner-x", &jobs(&["deploy"]))
            .await
            .expect("claim")
            .is_none()
    );
    let a = r
        .claim(NS, "runner-a", &jobs(&["ci"]))
        .await
        .expect("claim")
        .expect("a");
    assert_eq!(a.rid, rid);
    assert!(
        r.claim(NS, "runner-b", &jobs(&["ci"]))
            .await
            .expect("claim")
            .is_none()
    );
    let v = r.view(NS, &rid).await.expect("view");
    assert_eq!(v.state, "running");
    assert_eq!(v.runner.as_deref(), Some("runner-a"));
}

#[tokio::test]
async fn bad_job_and_params_refused() {
    let (_, r) = runs();
    assert!(matches!(
        r.create(NS, &new_id(), "../x", BTreeMap::new()).await,
        Err(Error::BadRequest(_))
    ));
    let mut p = BTreeMap::new();
    p.insert("lower".to_string(), "v".to_string());
    assert!(matches!(
        r.create(NS, &new_id(), "ci", p).await,
        Err(Error::BadRequest(_))
    ));
    let rid = new_id();
    r.create(NS, &rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    let mut p = BTreeMap::new();
    p.insert("REF".to_string(), "main".to_string());
    assert!(matches!(
        r.create(NS, &rid, "ci", p).await,
        Err(Error::Conflict { .. })
    ));
}

#[tokio::test]
async fn log_append_idempotent_by_offset() {
    let (_, r) = runs();
    let rid = new_id();
    r.create(NS, &rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    let c = r
        .claim(NS, "runner-a", &jobs(&["ci"]))
        .await
        .expect("claim")
        .expect("run");
    let a = Bytes::from_static(b"hello ");
    let b = Bytes::from_static(b"world\n");
    assert_eq!(
        r.append_log(NS, &rid, c.epoch, 0, a.clone())
            .await
            .expect("a"),
        6
    );
    // A resend of committed bytes at the same offset changes nothing.
    assert_eq!(
        r.append_log(NS, &rid, c.epoch, 0, a.clone())
            .await
            .expect("a again"),
        6
    );
    // Different bytes at a committed offset, or a gap, get the committed length.
    let r1 = r.append_log(NS, &rid, c.epoch, 0, b.clone()).await;
    assert!(matches!(r1, Err(Error::Offset { committed: 6 })), "{r1:?}");
    let r2 = r.append_log(NS, &rid, c.epoch, 9, b.clone()).await;
    assert!(matches!(r2, Err(Error::Offset { committed: 6 })), "{r2:?}");
    assert_eq!(r.append_log(NS, &rid, c.epoch, 6, b).await.expect("b"), 12);
    assert_eq!(read_all(&r, &rid, 0).await.0, b"hello world\n");
    assert_eq!(read_all(&r, &rid, 3).await.0, b"lo world\n");
    assert_eq!(read_all(&r, &rid, 12).await.0, b"");
    assert!(matches!(
        r.read_log(NS, &rid, 13).await,
        Err(Error::Offset { committed: 12 })
    ));
}

#[tokio::test]
async fn finish_records_outcome_and_artifacts() {
    let (_, r) = runs();
    let rid = new_id();
    r.create(NS, &rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    let c = r
        .claim(NS, "runner-a", &jobs(&["ci"]))
        .await
        .expect("claim")
        .expect("run");
    let tid = new_id();
    r.add_artifact(NS, &rid, c.epoch, "report.xml", &tid, 42)
        .await
        .expect("artifact");
    r.add_artifact(NS, &rid, c.epoch, "report.xml", &tid, 42)
        .await
        .expect("again");
    let v = r
        .finish(NS, &rid, c.epoch, RunOutcome::Failed, Some(3), None)
        .await
        .expect("finish");
    assert_eq!(v.state, "failed");
    assert_eq!(v.exit_code, Some(3));
    assert_eq!(v.artifacts.len(), 1);
    assert!(v.done());
    let (_, done) = read_all(&r, &rid, 0).await;
    assert!(done);
    // Nothing appends after the end.
    let late = r
        .append_log(NS, &rid, c.epoch, 0, Bytes::from_static(b"x"))
        .await;
    assert!(late.is_err());
}

#[tokio::test]
async fn cancel_queued_ends_it_and_running_sets_the_flag() {
    let (_, r) = runs();
    let q = new_id();
    r.create(NS, &q, "ci", BTreeMap::new())
        .await
        .expect("create");
    assert_eq!(r.cancel(NS, &q).await.expect("cancel").state, "cancelled");
    let rid = new_id();
    r.create(NS, &rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    let c = r
        .claim(NS, "runner-a", &jobs(&["ci"]))
        .await
        .expect("claim")
        .expect("run");
    assert!(!r.renew(NS, &rid, "runner-a", c.epoch).await.expect("renew"));
    assert!(r.cancel(NS, &rid).await.expect("cancel").cancel_requested);
    assert!(r.renew(NS, &rid, "runner-a", c.epoch).await.expect("renew"));
    // The runner's fenced appends still work after the unfenced flag write.
    r.append_log(NS, &rid, c.epoch, 0, Bytes::from_static(b"bye"))
        .await
        .expect("append");
    let v = r
        .finish(NS, &rid, c.epoch, RunOutcome::Cancelled, None, None)
        .await
        .expect("finish");
    assert_eq!(v.state, "cancelled");
}

#[tokio::test]
async fn lost_runner_is_reaped_and_fenced() {
    let (ck, r) = runs();
    let rid = new_id();
    r.create(NS, &rid, "ci", BTreeMap::new())
        .await
        .expect("create");
    let c = r
        .claim(NS, "runner-a", &jobs(&["ci"]))
        .await
        .expect("claim")
        .expect("run");
    ck.advance(Duration::from_secs(31));
    assert!(
        r.claim(NS, "runner-b", &jobs(&["ci"]))
            .await
            .expect("claim")
            .is_none()
    );
    let v = r.view(NS, &rid).await.expect("view");
    assert_eq!(v.state, "failed");
    assert!(v.message.expect("message").contains("runner lost"));
    let late = r
        .append_log(NS, &rid, c.epoch, 0, Bytes::from_static(b"x"))
        .await;
    assert!(late.is_err());
}
