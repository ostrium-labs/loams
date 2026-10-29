//! `loams-wal` is version-agnostic (design §62 §9.3, NF2 Task 10): it
//! stores the compute's full `pg_version` per timeline and refuses only a
//! greeting of another major. These tests pin that down for Postgres 18
//! (beta), so a new major needs no change here.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use loams_safekeeper::Error;
use loams_safekeeper::acceptor::Acceptor;
use loams_safekeeper::proto::{AcceptorMessage, ProposerGreeting};
use loams_safekeeper::store::{MemWalStore, WalStore};
use loams_safekeeper::types::{Configuration, Id};

fn greeting(pg_version: u32, system_id: u64) -> ProposerGreeting {
    ProposerGreeting {
        tenant_id: Id([7; 16]),
        timeline_id: Id([18; 16]),
        mconf: Configuration::default(),
        pg_version,
        system_id,
        wal_seg_size: 16 << 20,
    }
}

#[tokio::test]
async fn greeting_accepts_pg18() {
    let store = Arc::new(MemWalStore::new());
    // An 18.6 compute creates its timeline, which records the full version.
    let g = greeting(180_006, 42);
    let (_, reply) = Acceptor::greet(store.clone(), 1, &g, true).await.unwrap();
    assert!(matches!(reply, AcceptorMessage::Greeting(r) if r.term == 0));
    let state = store.load(&g.timeline()).await.unwrap().unwrap();
    assert_eq!(state.server.pg_version, 180_006);
    assert_eq!(state.server.pg_major(), Some(18));

    // The next 18 minor reconnects to it (a minor update restarts the compute).
    Acceptor::greet(store.clone(), 1, &greeting(180_007, 42), false)
        .await
        .unwrap();
    // sync-safekeepers' greeting (system id 0) of the same major too.
    Acceptor::greet(store.clone(), 1, &greeting(180_007, 0), false)
        .await
        .unwrap();

    // A compute of another major never attaches to an 18 timeline: an
    // upgrade makes a new timeline (§62 §10).
    for other in [170_011, 190_000] {
        let err = Acceptor::greet(store.clone(), 1, &greeting(other, 42), false)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::Protocol(m) if m.contains("Postgres 18")),
            "{other}: {err}"
        );
    }
}
