//! The counter runtime (ME1 role wiring, part 2): it takes the partitions,
//! rebuilds and reads them, and then balances and leases come from its
//! counters.

mod common;

use std::sync::{Arc, Mutex};

use loams_meter::authz::Caller;
use loams_meter::entitlements::ValueSource;
use loams_meter::lease_service::LeaseServer;
use loams_meter::owner::MemLeases;
use loams_meter::run::{
    CLUSTER_SERVICE, CatalogInputs, CounterRuntime, CounterValues, Counters, RuntimeParts,
    StreamOnly, min_chunk, policy,
};
use loams_meter::store::{Customer, MemStore, MeterStore, PaymentState, SpendControls};
use loams_meter_core::{
    Aggregation, AllocatorConfig, Amount, Currency, Entitlement, EntitlementId, EntitlementKind,
    EntitlementSource, Feature, FeatureKey, Grant, GrantId, GrantKind, Interval, JsonPath,
    LeasePolicy, Meter, Mode, Money, Rollover, Slug, SubjectKey, Unit, UsagePeriod, UtcNanos,
};

use common::{Stream, TYPE, event};

const NOW: UtcNanos = UtcNanos(2 * UtcNanos::HOUR);

fn entitlement(mode: Mode) -> Entitlement {
    Entitlement {
        id: EntitlementId::new("ent1").expect("id"),
        subject: SubjectKey::new("org_a").expect("subject"),
        feature: FeatureKey::new("house_cpu").expect("feature"),
        kind: EntitlementKind::Metered {
            usage_period: UsagePeriod {
                interval: Interval::Day,
                anchor: UtcNanos(0),
            },
            mode,
            issue_after_reset: None,
            preserve_overage_at_reset: false,
        },
        created_at: UtcNanos(0),
        source: EntitlementSource::Manual { by: "test".into() },
    }
}

fn grant(units: i64) -> Grant {
    Grant {
        id: GrantId::new("g1").expect("id"),
        entitlement: EntitlementId::new("ent1").expect("id"),
        amount: Amount::from_units(units),
        priority: 10,
        effective_at: UtcNanos(0),
        expires: None,
        rollover: Rollover::default(),
        recurrence: None,
        kind: GrantKind::Included,
        voided_at: None,
        created_at: UtcNanos(0),
    }
}

async fn store() -> Arc<MemStore> {
    let store = Arc::new(MemStore::new());
    let meter = Meter {
        slug: Slug::new("house_cpu_ms").expect("slug"),
        event_type: TYPE.into(),
        aggregation: Aggregation::Sum,
        value_property: Some(JsonPath::parse("$.cpu_ms").expect("path")),
        group_by: Default::default(),
        event_from: None,
    };
    store.put_meter(meter).await.expect("meter");
    store
        .put_feature(Feature {
            key: FeatureKey::new("house_cpu").expect("feature"),
            meter: Some(Slug::new("house_cpu_ms").expect("slug")),
            filter: Default::default(),
            unit: Unit::Ms,
            archived: false,
        })
        .await
        .expect("feature");
    store
        .create_entitlement(entitlement(Mode::HardLimit), None)
        .await
        .expect("entitlement");
    store.create_grant(grant(1_000), None).await.expect("grant");
    store
}

struct World {
    runtime: CounterRuntime<MemLeases>,
    counters: Arc<Counters>,
    leases: Arc<LeaseServer>,
    stream: Arc<Stream>,
}

async fn world() -> World {
    let store = store().await;
    let stream = Arc::new(Stream(Mutex::new(vec![
        (0, event("e1", "org_a", 100)),
        (1, event("e2", "org_a", 50)),
        // Dedup layer 2: the same event again.
        (2, event("e2", "org_a", 50)),
    ])));
    // One partition, so the in-memory stream is every subject's.
    let counters = Counters::new(1);
    let clock: loams_meter::customers::Clock = Arc::new(|| NOW);
    let leases = Arc::new(LeaseServer::new(
        1,
        AllocatorConfig::default(),
        Arc::new(CatalogInputs(counters.clone())),
        vec![CLUSTER_SERVICE.to_string()],
        clock.clone(),
    ));
    let runtime = CounterRuntime::new(RuntimeParts {
        leases: MemLeases::new(),
        me: "node-1".into(),
        reader: stream.clone(),
        house: Arc::new(StreamOnly),
        store,
        counters: counters.clone(),
        lease_server: leases.clone(),
        clock,
    });
    World {
        runtime,
        counters,
        leases,
        stream,
    }
}

#[tokio::test]
async fn balances_wait_for_the_counters_then_come_from_them() {
    let mut w = world().await;
    let values = CounterValues(w.counters.clone());
    let e = entitlement(Mode::HardLimit);
    let g = [grant(1_000)];

    let before = values.value(&e, &g, NOW).await;
    assert!(
        matches!(before, Err(loams_meter::error::MeterError::Unavailable(_))),
        "{before:?}"
    );

    w.runtime.step().await;
    assert_eq!(w.counters.owned().await.len(), 1);
    assert_eq!(w.leases.owned().len(), 1, "granting once rebuilt");

    let b = values.value(&e, &g, NOW).await.expect("a balance");
    assert_eq!(b.usage, Amount::from_units(150), "the duplicate counts once");
    assert_eq!(b.balance, Amount::from_units(850));
    assert!(b.has_access);

    w.stream
        .0
        .lock()
        .expect("lock")
        .push((3, event("e3", "org_a", 25)));
    w.runtime.step().await;
    let b = values.value(&e, &g, NOW).await.expect("a balance");
    assert_eq!(b.usage, Amount::from_units(175));
}

#[tokio::test]
async fn leases_follow_the_counters() {
    let mut w = world().await;
    w.runtime.step().await;
    let granted = w
        .leases
        .acquire(
            &Caller::Service(CLUSTER_SERVICE.into()),
            SubjectKey::new("org_a").expect("subject"),
            FeatureKey::new("house_cpu").expect("feature"),
            "node-9".into(),
            Amount::from_units(10),
            Vec::new(),
            0,
        )
        .await
        .expect("a lease");
    assert!(granted.grant.has_access);
    assert!(granted.grant.amount.is_positive());
    assert!(granted.grant.amount <= Amount::from_units(850));

    let other = w
        .leases
        .acquire(
            &Caller::Service(CLUSTER_SERVICE.into()),
            SubjectKey::new("org_b").expect("subject"),
            FeatureKey::new("house_cpu").expect("feature"),
            "node-9".into(),
            Amount::from_units(10),
            Vec::new(),
            0,
        )
        .await
        .expect("an answer");
    assert!(!other.grant.has_access, "no entitlement, no lease");
}

#[tokio::test]
async fn release_gives_the_partitions_back() {
    let mut w = world().await;
    w.runtime.step().await;
    w.runtime.release().await;
    assert!(w.leases.owned().is_empty());
}

#[test]
fn chunks_follow_the_unit() {
    assert_eq!(min_chunk(&Unit::Requests), Amount::from_units(1));
    assert_eq!(min_chunk(&Unit::Bytes), Amount::from_units(65_536));
    assert_eq!(min_chunk(&Unit::Tokens), Amount::from_units(1_000));
    assert_eq!(min_chunk(&Unit::CuMs), Amount::from_units(100));
}

fn customer(state: PaymentState, overage: bool, cap: Option<i64>) -> Customer {
    let eur = Currency::parse("EUR").expect("currency");
    Customer {
        id: "c1".into(),
        org: "a".into(),
        display_name: "A".into(),
        subjects: vec![SubjectKey::new("org_a").expect("subject")],
        payment_state: state,
        spend_controls: SpendControls {
            overage_enabled: overage,
            spend_cap: cap.map(|m| Money::new(m, eur)),
            ..SpendControls::default()
        },
        stripe_customer_id: None,
        currency: eur,
        plan: None,
        created_at: UtcNanos(0),
        updated_at: UtcNanos(0),
        version: 1,
    }
}

#[test]
fn overage_needs_a_payer_who_turned_it_on() {
    let eur = Currency::parse("EUR").expect("currency");
    let mode = Mode::OverageAllowed {
        overage_price: Money::new(2_000_000, eur),
        spend_cap: Money::new(100_000_000, eur),
    };
    assert_eq!(policy(&Mode::HardLimit, None), LeasePolicy::HardLimit);
    assert_eq!(policy(&Mode::SoftLimit, None), LeasePolicy::SoftLimit);
    assert_eq!(policy(&mode, None), LeasePolicy::HardLimit);
    assert_eq!(
        policy(&mode, Some(&customer(PaymentState::None, true, None))),
        LeasePolicy::HardLimit,
        "no payment method"
    );
    assert_eq!(
        policy(&mode, Some(&customer(PaymentState::Card, false, None))),
        LeasePolicy::HardLimit,
        "overage off"
    );
    assert_eq!(
        policy(&mode, Some(&customer(PaymentState::Card, true, None))),
        LeasePolicy::Overage {
            headroom: Amount::from_units(50)
        },
        "100 EUR at 2 EUR a unit"
    );
    assert_eq!(
        policy(&mode, Some(&customer(PaymentState::Invoice, true, Some(10_000_000)))),
        LeasePolicy::Overage {
            headroom: Amount::from_units(5)
        },
        "the customer's lower cap wins"
    );
}
