//! Leader failover on a real cluster. Needs kind and a kubeconfig, so it is
//! ignored locally and runs in CI.

use std::time::Duration;

use loams_kube::{KubeLeaseStore, LeaseConfig, LeaseStore, TokioClock, run_leader};

/// Two candidates on one Lease; delete the holder's Lease record by releasing
/// nothing (as a crash does); the other leads within 15 s + 2 x 2 s.
#[tokio::test]
#[ignore = "needs a kind cluster"]
async fn it_kind_leader_failover() {
    let client = kube::Client::try_default().await.expect("kubeconfig");
    let ns = "default";
    let name = format!("it-leader-{}", std::process::id());
    let mk = |id: &str| {
        let mut c = LeaseConfig::new(ns, id);
        c.name = name.clone();
        c
    };
    let store = KubeLeaseStore::new(client.clone(), ns);

    let (tx_a, rx_a) = tokio::sync::oneshot::channel();
    let a = tokio::spawn(run_leader(
        mk("a"),
        store.clone(),
        TokioClock::new(),
        |_| async move {
            let _ = tx_a.send(());
            std::future::pending::<()>().await;
        },
    ));
    rx_a.await.expect("a leads");

    // The holder dies without releasing.
    a.abort();
    let died = tokio::time::Instant::now();

    let (tx_b, rx_b) = tokio::sync::oneshot::channel();
    let b = tokio::spawn(run_leader(
        mk("b"),
        store.clone(),
        TokioClock::new(),
        |_| async move {
            let _ = tx_b.send(());
        },
    ));
    tokio::time::timeout(Duration::from_secs(15 + 2 * 2 + 2), rx_b)
        .await
        .expect("b leads within 15 s + 2 x 2 s")
        .expect("b ran");
    assert!(died.elapsed() >= Duration::from_secs(10));
    let _ = b.await;

    let api: kube::Api<k8s_openapi::api::coordination::v1::Lease> =
        kube::Api::namespaced(client, ns);
    let _ = api.delete(&name, &Default::default()).await;
    let _ = store.get(&name).await;
}
