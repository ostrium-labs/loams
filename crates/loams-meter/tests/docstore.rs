//! `DocStore`: the meter's records in one versioned document survive a
//! restart (a new store over the same document) and a racing writer.

use std::sync::Arc;

use async_trait::async_trait;
use loams_meter::error::Result;
use loams_meter::store::{
    Customer, DocStore, MemDoc, MeterStore, PaymentState, PlanRecord, SpendControls, VersionedDoc,
};
use loams_meter_core::{Currency, PlanId, SubjectKey, UtcNanos};

fn customer(id: &str, subject: &str) -> Customer {
    Customer {
        id: id.into(),
        org: "acme".into(),
        display_name: "Acme".into(),
        subjects: vec![SubjectKey::new(subject).expect("subject")],
        payment_state: PaymentState::None,
        spend_controls: SpendControls::default(),
        stripe_customer_id: Some(format!("cus_{id}")),
        currency: Currency::parse("EUR").expect("eur"),
        plan: None,
        created_at: UtcNanos(1),
        updated_at: UtcNanos(1),
        version: 1,
    }
}

#[tokio::test]
async fn records_survive_a_new_store_over_the_same_document() {
    let doc = Arc::new(MemDoc::default());
    let a = DocStore::new(doc.clone());
    a.create_customer(customer("c1", "acme"), Some("k1"))
        .await
        .expect("create");
    a.create_plan(PlanRecord {
        name: PlanId::new("free").expect("plan"),
        version: 3,
        yaml: "plan: free\nversion: 3\nentitlements: []\n".into(),
        digest: "d".into(),
        created_at: UtcNanos(2),
    })
    .await
    .expect("plan");

    // Another instance (or this one after a restart) reads the same records.
    let b = DocStore::new(doc);
    let c = b.customer("c1").await.expect("read").expect("there");
    assert_eq!(c.subjects[0].as_str(), "acme");
    assert!(
        b.customer_by_stripe("cus_c1")
            .await
            .expect("read")
            .is_some()
    );
    let plan = b
        .plan(&PlanId::new("free").expect("plan"), 3)
        .await
        .expect("read")
        .expect("plan");
    assert_eq!(plan.digest, "d");
    // The idempotency key was kept: a retry answers the first customer.
    let again = b
        .create_customer(customer("c2", "other"), Some("k1"))
        .await
        .expect("retry");
    assert_eq!(again.id, "c1");
    assert_eq!(
        b.list_customers(None, None, 10).await.expect("list").len(),
        1
    );
}

/// A document whose first save always loses, as if another instance wrote
/// in between: the write is redone on the newer document.
#[derive(Debug, Default)]
struct Racy {
    inner: MemDoc,
    raced: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl VersionedDoc for Racy {
    async fn load(&self) -> Result<Option<(u64, Vec<u8>)>> {
        self.inner.load().await
    }
    async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> Result<Option<u64>> {
        if !self.raced.swap(true, std::sync::atomic::Ordering::SeqCst) {
            // The other writer's record lands first.
            let theirs = serde_json::json!({ "customers": [customer("x", "x")] });
            self.inner
                .save(expected, serde_json::to_vec(&theirs).expect("json"))
                .await
                .expect("their save");
            return Ok(None);
        }
        self.inner.save(expected, doc).await
    }
}

#[tokio::test]
async fn a_lost_race_is_redone_on_the_newer_document() {
    let doc = Arc::new(Racy::default());
    let store = DocStore::new(doc);
    store
        .create_customer(customer("c1", "acme"), None)
        .await
        .expect("create after the race");
    let all = store.list_customers(None, None, 10).await.expect("list");
    let ids: Vec<&str> = all.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["c1", "x"], "both writers' records are kept");
}
