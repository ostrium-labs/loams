//! `CustomerService` and `EntitlementService` (ME1 Task 10) over the
//! in-memory store, the built-in authorizer (model v1 in process) and an
//! in-memory audit log.

use std::sync::Arc;

use async_trait::async_trait;
use loams_authz::{BuiltinAuthorizer, Subject, Tuple};
use loams_meter::audit::MemAudit;
use loams_meter::authz::{Caller, MeterAuthz};
use loams_meter::customers::{Customers, NewCustomer};
use loams_meter::entitlements::{Entitlements, NewGrant, ValueSource};
use loams_meter::error::{MeterError, Result};
use loams_meter::store::{MemStore, PaymentState, SpendControls};
use loams_meter_core::{
    Amount, AsOf, Balance, Currency, Entitlement, EntitlementKind, Grant, GrantKind, Interval,
    Mode, Money, Period, RefusalReason, Rollover, UsagePeriod, UtcNanos,
};

const NOW: UtcNanos = UtcNanos(1_700_000_000_000_000_000);

/// Balance = live grants minus a fixed usage of 40 units.
#[derive(Debug)]
struct FixedUsage;

#[async_trait]
impl ValueSource for FixedUsage {
    async fn value(&self, e: &Entitlement, grants: &[Grant], at: UtcNanos) -> Result<Balance> {
        let total: Amount = grants
            .iter()
            .filter(|g| g.is_live(at))
            .map(|g| g.amount)
            .sum();
        let usage = Amount::from_units(40);
        let balance = total.saturating_sub(usage).clamp_non_negative();
        Ok(Balance {
            entitlement: e.id.clone(),
            period: Period {
                start: UtcNanos(0),
                end: UtcNanos(i64::MAX),
            },
            has_access: balance.is_positive(),
            balance,
            usage,
            overage: usage.saturating_sub(total).clamp_non_negative(),
            total_available: total,
            grants: Vec::new(),
            prepaid_only: false,
            as_of: AsOf::Replay,
        })
    }
}

struct World {
    customers: Customers,
    entitlements: Entitlements,
    audit: Arc<MemAudit>,
}

fn world() -> World {
    let fga = BuiltinAuthorizer::new();
    fga.write(&[
        Tuple::new("platform:default", "platform", "org:acme"),
        Tuple::new("user:mia", "meter_admin", "platform:default"),
        Tuple::new("user:bea", "billing_admin", "org:acme"),
        Tuple::new("user:bill", "billing_viewer", "org:acme"),
    ])
    .expect("tuples");
    let authz = MeterAuthz::new(Arc::new(fga), "meter-export");
    let store = Arc::new(MemStore::new());
    let audit = Arc::new(MemAudit::new());
    let clock: loams_meter::customers::Clock = Arc::new(|| NOW);
    World {
        customers: Customers::new(store.clone(), authz.clone(), audit.clone(), clock.clone()),
        entitlements: Entitlements::new(store, authz, audit.clone(), Arc::new(FixedUsage), clock),
        audit,
    }
}

fn person(id: &str) -> Caller {
    Caller::Principal(Subject::user(id))
}

fn agent() -> Caller {
    Caller::Principal(Subject::agent("agent-1"))
}

fn metered() -> EntitlementKind {
    EntitlementKind::Metered {
        usage_period: UsagePeriod {
            interval: Interval::Month,
            anchor: UtcNanos(0),
        },
        mode: Mode::HardLimit,
        issue_after_reset: None,
        preserve_overage_at_reset: false,
    }
}

fn grant_of(e: &Entitlement, units: i64) -> NewGrant {
    NewGrant {
        entitlement: e.id.clone(),
        amount: Amount::from_units(units),
        priority: 10,
        effective_at: Some(UtcNanos(0)),
        expires: None,
        rollover: Rollover::default(),
        recurrence: None,
        kind: GrantKind::Promotional,
        reason: "launch credit".into(),
        idempotency_key: None,
    }
}

async fn setup(w: &World) -> Entitlement {
    w.customers
        .create(
            &person("mia"),
            NewCustomer {
                org: "acme".into(),
                display_name: "Acme".into(),
                subjects: vec!["acme".into()],
                currency: "EUR".into(),
                stripe_customer_id: Some("cus_1".into()),
                idempotency_key: Some("k1".into()),
            },
        )
        .await
        .expect("customer");
    w.entitlements
        .create(&person("mia"), "acme", "house_cpu", metered(), "plan", None)
        .await
        .expect("entitlement")
}

#[tokio::test]
async fn agent_cannot_write_grants() {
    let w = world();
    let e = setup(&w).await;
    let err = w
        .entitlements
        .create_grant(&agent(), grant_of(&e, 100))
        .await
        .expect_err("an agent may not grant");
    assert_eq!(err.reason(), "agent_not_allowed");
    // Nor through a person: mia acting via an agent.
    let mut via = Subject::user("mia");
    via.act.push(Subject::agent("agent-1"));
    let err = w
        .entitlements
        .create_grant(&Caller::Principal(via), grant_of(&e, 100))
        .await
        .expect_err("not through an agent either");
    assert_eq!(err.reason(), "agent_not_allowed");
    // A billing admin is not a meter admin.
    let err = w
        .entitlements
        .create_grant(&person("bea"), grant_of(&e, 100))
        .await
        .expect_err("billing admin");
    assert_eq!(err.reason(), "meter_admin_required");
    // Nothing was written or audited.
    let (grants, _) = w
        .entitlements
        .list_grants(&person("mia"), e.id.as_str(), true, None, 10)
        .await
        .expect("list");
    assert!(grants.is_empty());
    assert!(
        w.audit
            .events()
            .iter()
            .all(|a| a.action != "meter.grant.create")
    );
    // The meter admin can, and it is audited with the reason.
    let g = w
        .entitlements
        .create_grant(&person("mia"), grant_of(&e, 100))
        .await
        .expect("meter admin grants");
    let audited = w
        .audit
        .events()
        .into_iter()
        .find(|a| a.action == "meter.grant.create")
        .expect("audited");
    assert_eq!(audited.actor, "user:mia");
    assert_eq!(audited.object, format!("grant:{}", g.id));
    assert_eq!(audited.reason, "launch credit");
    assert_eq!(audited.org.as_deref(), Some("acme"));
}

#[tokio::test]
async fn balances_and_access_follow_grants() {
    let w = world();
    let e = setup(&w).await;
    let v = w
        .entitlements
        .value(&person("bill"), "acme", "house_cpu")
        .await
        .expect("viewer reads");
    assert!(!v.has_access, "no grants: 0 available, 40 used");
    w.entitlements
        .create_grant(&person("mia"), grant_of(&e, 100))
        .await
        .expect("grant");
    let v = w
        .entitlements
        .value(&person("bill"), "acme", "house_cpu")
        .await
        .expect("value");
    assert!(v.has_access);
    assert_eq!(v.balance, Amount::from_units(60));
    let access = w
        .entitlements
        .check_access(
            &person("bill"),
            &[
                ("acme".into(), "house_cpu".into()),
                ("acme".into(), "llm_tokens".into()),
            ],
        )
        .await
        .expect("check");
    assert!(access[0].has_access);
    assert_eq!(access[1].reason, Some(RefusalReason::NotEntitled));
    // Someone outside the org reads nothing.
    let err = w
        .entitlements
        .value(&person("mona"), "acme", "house_cpu")
        .await
        .expect_err("outsider");
    assert_eq!(err.reason(), "billing_viewer_required");
}

#[tokio::test]
async fn a_voided_grant_stops_counting() {
    let w = world();
    let e = setup(&w).await;
    let g = w
        .entitlements
        .create_grant(&person("mia"), grant_of(&e, 100))
        .await
        .expect("grant");
    w.entitlements
        .void_grant(&person("mia"), g.id.as_str(), "mistake")
        .await
        .expect("void");
    let (live, _) = w
        .entitlements
        .list_grants(&person("bill"), e.id.as_str(), false, None, 10)
        .await
        .expect("list");
    assert!(live.is_empty());
}

#[tokio::test]
async fn overage_needs_a_card_and_a_cap() {
    let w = world();
    setup(&w).await;
    let c = w
        .customers
        .list(&person("bea"), Some("acme"), None, 10)
        .await
        .expect("list")
        .0
        .remove(0);
    let eur = Currency::parse("EUR").expect("eur");
    let cap = SpendControls {
        overage_enabled: true,
        spend_cap: Some(Money::new(50_000_000, eur)),
        ..SpendControls::default()
    };
    let err = w
        .customers
        .set_spend_controls(&person("bea"), &c.id, cap.clone(), "opt in")
        .await
        .expect_err("no card");
    assert_eq!(err.reason(), "payment_method_required");

    // The billing exporter records the card (looked up by its Stripe id).
    w.customers
        .set_payment_state(
            &Caller::Service("meter-export".into()),
            None,
            Some("cus_1"),
            PaymentState::Card,
            "payment_method.attached",
        )
        .await
        .expect("card");
    let no_cap = SpendControls {
        overage_enabled: true,
        ..SpendControls::default()
    };
    let err = w
        .customers
        .set_spend_controls(&person("bea"), &c.id, no_cap, "opt in")
        .await
        .expect_err("no cap");
    assert_eq!(err.reason(), "spend_cap_required");
    let updated = w
        .customers
        .set_spend_controls(&person("bea"), &c.id, cap, "opt in")
        .await
        .expect("card and cap");
    assert!(updated.spend_controls.overage_enabled);
    // A viewer may not change it; an agent never.
    assert!(
        w.customers
            .set_spend_controls(&person("bill"), &c.id, SpendControls::default(), "x")
            .await
            .is_err()
    );
    assert_eq!(
        w.customers
            .set_spend_controls(&agent(), &c.id, SpendControls::default(), "x")
            .await
            .expect_err("agent")
            .reason(),
        "agent_not_allowed"
    );
}

#[tokio::test]
async fn payment_state_comes_from_the_billing_service_and_keeps_invoice() {
    let w = world();
    setup(&w).await;
    let id = w
        .customers
        .list(&person("mia"), None, None, 10)
        .await
        .expect("list")
        .0[0]
        .id
        .clone();
    // A person (even a meter admin) cannot claim a card.
    let err = w
        .customers
        .set_payment_state(&person("mia"), Some(&id), None, PaymentState::Card, "x")
        .await
        .expect_err("card by hand");
    assert_eq!(err.reason(), "billing_service_only");
    // An operator puts a contract customer on invoice …
    let c = w
        .customers
        .set_payment_state(
            &person("mia"),
            Some(&id),
            None,
            PaymentState::Invoice,
            "contract",
        )
        .await
        .expect("invoice");
    assert_eq!(c.payment_state, PaymentState::Invoice);
    // … and the exporter's "no card" does not take it off.
    let c = w
        .customers
        .set_payment_state(
            &Caller::Service("meter-export".into()),
            Some(&id),
            None,
            PaymentState::None,
            "customer.updated",
        )
        .await
        .expect("ignored");
    assert_eq!(c.payment_state, PaymentState::Invoice);
    // Another service identity is refused.
    let err = w
        .customers
        .set_payment_state(
            &Caller::Service("house".into()),
            Some(&id),
            None,
            PaymentState::Card,
            "x",
        )
        .await
        .expect_err("not the exporter");
    assert!(matches!(err, MeterError::PermissionDenied { .. }));
}

#[tokio::test]
async fn customers_are_idempotent_and_subjects_unique() {
    let w = world();
    setup(&w).await;
    let again = w
        .customers
        .create(
            &person("mia"),
            NewCustomer {
                org: "acme".into(),
                display_name: "Acme".into(),
                subjects: vec!["acme".into()],
                currency: "EUR".into(),
                stripe_customer_id: Some("cus_1".into()),
                idempotency_key: Some("k1".into()),
            },
        )
        .await
        .expect("retry answers the first");
    let (all, _) = w
        .customers
        .list(&person("mia"), None, None, 10)
        .await
        .expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, again.id);
    let err = w
        .customers
        .create(
            &person("mia"),
            NewCustomer {
                org: "acme".into(),
                subjects: vec!["acme".into()],
                ..NewCustomer::default()
            },
        )
        .await
        .expect_err("subject taken");
    assert!(matches!(err, MeterError::AlreadyExists { .. }));
    // One audit event for the one customer.
    assert_eq!(
        w.audit
            .events()
            .iter()
            .filter(|a| a.action == "meter.customer.create")
            .count(),
        1
    );
}
