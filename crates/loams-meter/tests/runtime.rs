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
        events: None,
        notifier: None,
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
    let values = CounterValues::new(w.counters.clone());
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
    assert_eq!(
        b.usage,
        Amount::from_units(150),
        "the duplicate counts once"
    );
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
        policy(
            &mode,
            Some(&customer(PaymentState::Invoice, true, Some(10_000_000)))
        ),
        LeasePolicy::Overage {
            headroom: Amount::from_units(5)
        },
        "the customer's lower cap wins"
    );
}

/// Captures what the runtime appends and what it hands the notifier.
#[derive(Debug, Default)]
struct Captured {
    appended: Mutex<Vec<loams_meter::notify::MeterEvent>>,
    handled: Mutex<Vec<String>>,
}

#[derive(Debug)]
struct Sink(Arc<Captured>);

#[async_trait::async_trait]
impl loams_meter::run::MeterEventSink for Sink {
    async fn append(
        &self,
        events: Vec<loams_meter::notify::MeterEvent>,
    ) -> loams_meter::error::Result<()> {
        self.0.appended.lock().expect("lock").extend(events);
        Ok(())
    }
}

#[derive(Debug)]
struct Handler(Arc<Captured>);

#[async_trait::async_trait]
impl loams_meter::run::MeterEventHandler for Handler {
    async fn handle(&self, e: &loams_meter::notify::MeterEvent) -> loams_meter::error::Result<()> {
        self.0.handled.lock().expect("lock").push(e.id.clone());
        Ok(())
    }

    async fn retry_due(&self) -> loams_meter::error::Result<usize> {
        Ok(0)
    }
}

#[tokio::test]
async fn crossed_thresholds_are_appended_and_read_back_to_the_notifier() {
    let store = store().await;
    // 900 of 1,000 used: past 80%.
    let stream = Arc::new(Stream(Mutex::new(vec![(0, event("e1", "org_a", 900))])));
    let counters = Counters::new(1);
    let clock: loams_meter::customers::Clock = Arc::new(|| NOW);
    let leases = Arc::new(LeaseServer::new(
        1,
        AllocatorConfig::default(),
        Arc::new(CatalogInputs(counters.clone())),
        vec![CLUSTER_SERVICE.to_string()],
        clock.clone(),
    ));
    let captured = Arc::new(Captured::default());
    let mut runtime = CounterRuntime::new(RuntimeParts {
        leases: MemLeases::new(),
        me: "node-1".into(),
        reader: stream.clone(),
        house: Arc::new(StreamOnly),
        store,
        counters,
        lease_server: leases,
        clock,
        events: Some(Arc::new(Sink(captured.clone()))),
        notifier: Some(Arc::new(Handler(captured.clone()))),
    });
    runtime.step().await;
    let appended = captured.appended.lock().expect("lock").clone();
    assert_eq!(appended.len(), 1, "{appended:?}");
    assert_eq!(appended[0].r#type, loams_meter::notify::events::THRESHOLD);
    assert!(appended[0].id.ends_with("/80"), "{}", appended[0].id);

    // The appended event comes back on the partition: the notifier gets it.
    let back = loams_meter::run::own_event(
        &loams_meter::run::to_cloudevent(&appended[0]).expect("a CloudEvent"),
    )
    .expect("one of the meter's own events");
    stream.0.lock().expect("lock").push((1, back));
    runtime.step().await;
    assert_eq!(
        *captured.handled.lock().expect("lock"),
        vec![appended[0].id.clone()]
    );
    // Nothing new crossed: nothing more appended.
    assert_eq!(captured.appended.lock().expect("lock").len(), 1);
}

/// The owner on another node: answers a fixed count for its address.
#[derive(Debug)]
struct Peer {
    addr: String,
    counted: Amount,
}

#[async_trait::async_trait]
impl loams_meter::run::CounterPeer for Peer {
    async fn counted(
        &self,
        addr: &str,
        _key: &loams_meter::owner::CounterKey,
    ) -> loams_meter::error::Result<Option<(Amount, loams_meter_core::AsOf)>> {
        Ok((addr == self.addr).then_some((
            self.counted,
            loams_meter_core::AsOf::Counter {
                owner_epoch: 3,
                offset: 9,
            },
        )))
    }
}

/// The House: a fixed usage so far.
#[derive(Debug)]
struct HouseSoFar(Amount);

#[async_trait::async_trait]
impl loams_meter::run::UsageSoFar for HouseSoFar {
    async fn usage(
        &self,
        _e: &Entitlement,
        _start: UtcNanos,
        _at: UtcNanos,
    ) -> loams_meter::error::Result<Amount> {
        Ok(self.0)
    }
}

#[tokio::test]
async fn a_non_owner_reads_the_owner_and_then_the_house() {
    use loams_meter::owner::PartitionLeases;
    use loams_meter::run::{holder_addr, holder_name};

    // This node counts nothing; another holds partition 0 of 1.
    let counters = Counters::new(1);
    let leases = Arc::new(MemLeases::new());
    let other = holder_name(2, "01J", Some("10.0.0.2:9000"));
    assert_eq!(holder_addr(&other), Some("10.0.0.2:9000"));
    assert_eq!(holder_addr(&holder_name(1, "01J", None)), None);
    leases
        .acquire(
            &loams_meter::owner::assign::partition_key(0),
            &other,
            std::time::Duration::from_secs(15),
        )
        .await
        .expect("the other node holds it");
    let e = entitlement(Mode::HardLimit);
    let g = [grant(1_000)];

    let forwarded = CounterValues::new(counters.clone())
        .with_owners(
            leases.clone(),
            Arc::new(Peer {
                addr: "10.0.0.2:9000".into(),
                counted: Amount::from_units(300),
            }),
        )
        .with_house(Arc::new(HouseSoFar(Amount::from_units(1))));
    let b = forwarded.value(&e, &g, NOW).await.expect("from the owner");
    assert_eq!(b.usage, Amount::from_units(300));
    assert!(matches!(
        b.as_of,
        loams_meter_core::AsOf::Counter { owner_epoch: 3, .. }
    ));

    // The owner does not answer for this address: the House does.
    let fallback = CounterValues::new(counters.clone())
        .with_owners(
            leases,
            Arc::new(Peer {
                addr: "elsewhere:1".into(),
                counted: Amount::ZERO,
            }),
        )
        .with_house(Arc::new(HouseSoFar(Amount::from_units(40))));
    let b = fallback.value(&e, &g, NOW).await.expect("from the House");
    assert_eq!(b.usage, Amount::from_units(40));
    assert!(matches!(b.as_of, loams_meter_core::AsOf::House { .. }));

    // Nobody answers: unavailable, never a balance without usage.
    let none = CounterValues::new(counters).value(&e, &g, NOW).await;
    assert!(matches!(
        none,
        Err(loams_meter::error::MeterError::Unavailable(_))
    ));
}

/// A remote owner that answers one canned `AcquireLeaseResponse`.
#[derive(Debug)]
struct Remote(serde_json::Value);

#[async_trait::async_trait]
impl loams_meter::lease_client::LeaseTransport for Remote {
    async fn acquire(
        &self,
        addr: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        assert_eq!(addr, "10.0.0.2:9000");
        assert_eq!(body["subject"], "org_a");
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn the_lease_client_asks_the_local_owner_then_a_remote_one() {
    use loams_meter::lease_client::{LeaseClient, LeaseRouter};
    use loams_meter::lease_service::EpochFence;
    use loams_meter::owner::PartitionLeases;
    use loams_meter_gate::gate::{LeaseRequest as GateRequest, LeaseSource};

    let mut w = world().await;
    w.runtime.step().await;
    let fence = Arc::new(EpochFence::new());
    let request = GateRequest {
        subject: "org_a",
        feature: "house_cpu",
        node: "node-9",
        want: 10,
        held: None,
    };

    // This process owns the partition: no network.
    let local = LeaseClient::start(LeaseRouter {
        partitions: 1,
        local: Some(w.leases.clone()),
        owners: Arc::new(MemLeases::new()),
        transport: Arc::new(Remote(serde_json::Value::Null)),
        fence: fence.clone(),
        clock: Arc::new(|| NOW),
        policies: None,
    })
    .expect("client");
    let g = local
        .acquire(&request, std::time::Duration::from_secs(2))
        .expect("a lease");
    assert!(g.has_access);
    assert!(g.amount > 0 && g.amount <= 850);
    assert!(!g.counting, "a hard limit refuses locally");
    assert_eq!(
        g.fail_open, 0,
        "no customer, no payment method: fail closed"
    );

    // Another node owns it: the request goes to its address.
    let owners = Arc::new(MemLeases::new());
    owners
        .acquire(
            &loams_meter::owner::assign::partition_key(0),
            &loams_meter::run::holder_name(2, "01J", Some("10.0.0.2:9000")),
            std::time::Duration::from_secs(15),
        )
        .await
        .expect("held");
    let answer = serde_json::json!({ "lease": {
        "leaseId": "abc", "amount": "40.5", "hasAccess": true,
        "expireTime": "1970-01-01T02:00:30Z", "refreshTime": "1970-01-01T02:00:10Z",
        "ownerEpoch": "7", "partition": 0, "counting": true, "overage": false, "failOpen": "12"
    }});
    let remote = LeaseClient::start(LeaseRouter {
        partitions: 1,
        local: None,
        owners: owners.clone(),
        transport: Arc::new(Remote(answer)),
        fence: fence.clone(),
        clock: Arc::new(|| NOW),
        policies: None,
    })
    .expect("client");
    let g = remote
        .acquire(&request, std::time::Duration::from_secs(2))
        .expect("a lease");
    assert_eq!(g.amount, 40, "whole base units, floored");
    assert_eq!(g.owner_epoch, 7);
    assert!(g.counting);
    assert_eq!(g.fail_open, 12);
    assert_eq!(g.ttl, std::time::Duration::from_secs(30));
    assert_eq!(g.refresh_after, std::time::Duration::from_secs(10));

    // A grant from an older epoch than one seen is dropped.
    let stale = serde_json::json!({ "lease": {
        "leaseId": "abd", "amount": "1", "hasAccess": true, "ownerEpoch": "6"
    }});
    let fenced = LeaseClient::start(LeaseRouter {
        partitions: 1,
        local: None,
        owners,
        transport: Arc::new(Remote(stale)),
        fence,
        clock: Arc::new(|| NOW),
        policies: None,
    })
    .expect("client");
    assert!(
        fenced
            .acquire(&request, std::time::Duration::from_secs(2))
            .is_err()
    );
}

#[tokio::test]
async fn a_pair_without_a_policy_gets_an_unmetered_grant_without_asking() {
    use loams_meter::lease_client::{LeaseClient, LeaseRouter};
    use loams_meter::lease_service::EpochFence;
    use loams_meter_gate::gate::{LeaseRequest as GateRequest, LeaseSource};
    use loams_meter_gate::scope::{Policies, PolicySet};

    let policies = Policies::new();
    policies.set(PolicySet::new([(
        "org_b".to_string(),
        "house_cpu".to_string(),
    )]));
    // No owner and a transport that must not be called.
    let client = LeaseClient::start(LeaseRouter {
        partitions: 1,
        local: None,
        owners: Arc::new(MemLeases::new()),
        transport: Arc::new(Remote(serde_json::Value::Null)),
        fence: Arc::new(EpochFence::new()),
        clock: Arc::new(|| NOW),
        policies: Some(policies),
    })
    .expect("client");
    let g = client
        .acquire(
            &GateRequest {
                subject: "org_a",
                feature: "house_cpu",
                node: "n",
                want: 5,
                held: None,
            },
            std::time::Duration::from_secs(2),
        )
        .expect("an unmetered grant");
    assert!(g.has_access && g.counting);
    assert!(g.amount > 1 << 40);
    assert_eq!(g, loams_meter::lease_client::unmetered_grant());
}

#[tokio::test]
async fn usage_right_after_an_entitlement_is_made_is_counted() {
    use std::sync::atomic::{AtomicI64, Ordering};
    // The store has the meter and feature, but not the entitlement yet.
    let store = Arc::new(MemStore::new());
    let full = self::store().await;
    for m in full.list_meters().await.expect("meters") {
        store.put_meter(m).await.expect("meter");
    }
    for f in full.list_features().await.expect("features") {
        store.put_feature(f).await.expect("feature");
    }
    let stream = Arc::new(Stream(Mutex::new(Vec::new())));
    let counters = Counters::new(1);
    let now = Arc::new(AtomicI64::new(NOW.0));
    let n = now.clone();
    let clock: loams_meter::customers::Clock = Arc::new(move || UtcNanos(n.load(Ordering::SeqCst)));
    let leases = Arc::new(LeaseServer::new(
        1,
        AllocatorConfig::default(),
        Arc::new(CatalogInputs(counters.clone())),
        vec![CLUSTER_SERVICE.to_string()],
        clock.clone(),
    ));
    let mut runtime = CounterRuntime::new(RuntimeParts {
        leases: MemLeases::new(),
        me: "node-1".into(),
        reader: stream.clone(),
        house: Arc::new(StreamOnly),
        store: store.clone(),
        counters: counters.clone(),
        lease_server: leases,
        clock,
        events: None,
        notifier: None,
    });
    runtime.step().await;

    // An entitlement, and usage two seconds later: well inside the regular
    // 10 s refresh of the cached records.
    store
        .create_entitlement(entitlement(Mode::HardLimit), None)
        .await
        .expect("entitlement");
    store.create_grant(grant(1_000), None).await.expect("grant");
    now.fetch_add(2 * UtcNanos::SECOND, Ordering::SeqCst);
    stream
        .0
        .lock()
        .expect("lock")
        .push((0, event("e1", "org_a", 70)));
    runtime.step().await;
    let b = CounterValues::new(counters)
        .value(&entitlement(Mode::HardLimit), &[grant(1_000)], NOW)
        .await
        .expect("a balance");
    assert_eq!(b.usage, Amount::from_units(70));
}

/// A House with a pipe cursor and a total it reports for org_a.
#[derive(Debug)]
struct ReconcileHouse {
    cursor: &'static str,
    total: Mutex<String>,
}

impl loams_meter_house::HouseSql for ReconcileHouse {
    async fn query(&self, sql: &str) -> Result<String, loams_meter_house::HouseError> {
        if sql.contains("system.loams_pipe_cursors") {
            return Ok(self.cursor.to_string());
        }
        Ok(String::new())
    }

    async fn query_with(
        &self,
        sql: &str,
        settings: &[(&str, String)],
    ) -> Result<String, loams_meter_house::HouseError> {
        if sql.contains("_meter.events FINAL")
            && settings == [("loams_snapshot_id", "77".to_string())]
        {
            return Ok(format!("org_a\t{}\n", self.total.lock().expect("lock")));
        }
        Ok(String::new())
    }
}

#[tokio::test]
async fn reconciliation_rebases_on_the_house_at_its_cursor() {
    use std::sync::atomic::{AtomicI64, Ordering};
    let store = store().await;
    // Offsets 0 and 1 are in the House's snapshot 77 (cursor 2); offset 2
    // is the stream's tail.
    let stream = Arc::new(Stream(Mutex::new(vec![
        (0, event("e1", "org_a", 100)),
        (1, event("e2", "org_a", 50)),
        (2, event("e3", "org_a", 25)),
    ])));
    let house = Arc::new(ReconcileHouse {
        cursor: "2\t77\n",
        total: Mutex::new("150".into()),
    });
    let counters = Counters::new(1);
    let now = Arc::new(AtomicI64::new(NOW.0));
    let n = now.clone();
    let clock: loams_meter::customers::Clock = Arc::new(move || UtcNanos(n.load(Ordering::SeqCst)));
    let leases = Arc::new(LeaseServer::new(
        1,
        AllocatorConfig::default(),
        Arc::new(CatalogInputs(counters.clone())),
        vec![CLUSTER_SERVICE.to_string()],
        clock.clone(),
    ));
    let mut runtime = CounterRuntime::new(RuntimeParts {
        leases: MemLeases::new(),
        me: "node-1".into(),
        reader: stream.clone(),
        house: Arc::new(loams_meter::house::HousePipeTotals {
            house: house.clone(),
            counters: counters.clone(),
            partitions: 1,
            clock: clock.clone(),
        }),
        store,
        counters: counters.clone(),
        lease_server: leases,
        clock,
        events: None,
        notifier: None,
    });
    // The rebuild: the House's 150 at the cursor, then the tail's 25.
    runtime.step().await;
    let values = CounterValues::new(counters.clone());
    let e = entitlement(Mode::HardLimit);
    let at = UtcNanos(now.load(Ordering::SeqCst));
    let b = values
        .value(&e, &[grant(1_000)], at)
        .await
        .expect("a balance");
    assert_eq!(b.usage, Amount::from_units(175));

    // The House's deduplicated total is lower (a duplicate the counters
    // took twice): five minutes on, reconciliation rebases on it.
    *house.total.lock().expect("lock") = "140".into();
    now.fetch_add(
        i64::try_from(loams_meter::reconcile::RECONCILE_EVERY.as_nanos()).expect("nanos")
            + UtcNanos::SECOND,
        Ordering::SeqCst,
    );
    runtime.step().await;
    let at = UtcNanos(now.load(Ordering::SeqCst));
    let b = values
        .value(&e, &[grant(1_000)], at)
        .await
        .expect("a balance");
    assert_eq!(
        b.usage,
        Amount::from_units(165),
        "140 at the cursor plus the 25 after it"
    );
}
