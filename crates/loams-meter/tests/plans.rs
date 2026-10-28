//! Plans as YAML (ME1 Task 11): versions and used meters are immutable,
//! and assigning a plan manages the entitlements it creates.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loams_authz::{BuiltinAuthorizer, Subject, Tuple};
use loams_meter::audit::MemAudit;
use loams_meter::authz::{Caller, MeterAuthz};
use loams_meter::customers::{Customers, NewCustomer};
use loams_meter::error::Result;
use loams_meter::plans::{MeterUsage, Plans};
use loams_meter::store::{MemStore, MeterStore};
use loams_meter_core::{
    EntitlementKind, EntitlementSource, FeatureKey, Mode, Slug, SubjectKey, UtcNanos,
};

/// The House, as far as plans care: which meters have events.
#[derive(Debug, Default)]
struct Used(Mutex<HashSet<String>>);

#[async_trait]
impl MeterUsage for Used {
    async fn has_events(&self, slug: &Slug) -> Result<bool> {
        Ok(self.0.lock().expect("lock").contains(slug.as_str()))
    }
}

const V1: &str = r#"
plan: pro
version: 1
meters:
  - slug: house_cpu_ms
    event_type: io.loams.dev.usage.house.cpu.v1
    aggregation: SUM
    value_property: $.cpu_ms
features:
  - key: house_cpu
    meter: house_cpu_ms
    unit: ms
entitlements:
  - feature: house_cpu
    metered: { period: { interval: month }, mode: hard_limit, issue_after_reset: 1000 }
  - feature: byoc
    boolean: true
"#;

struct World {
    plans: Plans,
    customers: Customers,
    store: Arc<MemStore>,
    used: Arc<Used>,
    audit: Arc<MemAudit>,
}

fn world() -> World {
    let fga = BuiltinAuthorizer::new();
    fga.write(&[Tuple::new("user:mia", "meter_admin", "platform:default")])
        .expect("tuples");
    let authz = MeterAuthz::new(Arc::new(fga), "meter-export");
    let store = Arc::new(MemStore::new());
    let audit = Arc::new(MemAudit::new());
    let used = Arc::new(Used::default());
    // 2023-11-14T22:13:20Z: the billing anchor floors it to 22:00.
    let clock: loams_meter::customers::Clock = Arc::new(|| UtcNanos(1_700_000_000_000_000_000));
    World {
        plans: Plans::new(
            store.clone(),
            authz.clone(),
            audit.clone(),
            used.clone(),
            clock.clone(),
        ),
        customers: Customers::new(store.clone(), authz, audit.clone(), clock),
        store,
        used,
        audit,
    }
}

fn mia() -> Caller {
    Caller::Principal(Subject::user("mia"))
}

#[tokio::test]
async fn meter_semantics_immutable_once_used() {
    let w = world();
    let first = w
        .plans
        .apply(&mia(), V1, "gitops abc123")
        .await
        .expect("v1");
    assert!(first.changed);
    // The same file again is a no-op.
    let again = w
        .plans
        .apply(&mia(), V1, "gitops abc123")
        .await
        .expect("again");
    assert!(!again.changed);
    assert_eq!(again.plan.digest, first.plan.digest);

    // While the meter has no events, a new version may still correct it.
    let v2 = V1
        .replace("version: 1", "version: 2")
        .replace("$.cpu_ms", "$.cpu.ms");
    w.plans
        .apply(&mia(), &v2, "fix path")
        .await
        .expect("unused meter may change");

    // Once it has events, changing what it counts is refused, and nothing
    // of that apply is stored.
    w.used.0.lock().expect("lock").insert("house_cpu_ms".into());
    let v3 = v2
        .replace("version: 2", "version: 3")
        .replace("aggregation: SUM", "aggregation: MAX");
    let err = w
        .plans
        .apply(&mia(), &v3, "change aggregation")
        .await
        .expect_err("used meter");
    assert_eq!(err.reason(), "meter_semantics_immutable");
    let m = w
        .store
        .meter(&Slug::new("house_cpu_ms").expect("slug"))
        .await
        .expect("meter")
        .expect("stored");
    assert_eq!(m.aggregation, loams_meter_core::Aggregation::Sum);
    assert!(
        w.store
            .plan(&loams_meter_core::PlanId::new("pro").expect("id"), 3)
            .await
            .expect("plan")
            .is_none()
    );
    // A new slug is the way to count differently.
    let v3_new_slug = v3.replace("house_cpu_ms", "house_cpu_max_ms");
    w.plans
        .apply(&mia(), &v3_new_slug, "new meter")
        .await
        .expect("a new meter slug is fine");
}

#[tokio::test]
async fn a_plan_version_is_immutable() {
    let w = world();
    w.plans.apply(&mia(), V1, "first").await.expect("v1");
    let err = w
        .plans
        .apply(
            &mia(),
            &V1.replace("boolean: true", "boolean: false"),
            "edit",
        )
        .await
        .expect_err("same version, other content");
    assert_eq!(err.reason(), "plan_version_immutable");
    // Agents never apply plans.
    let err = w
        .plans
        .apply(&Caller::Principal(Subject::agent("a")), V1, "x")
        .await
        .expect_err("agent");
    assert_eq!(err.reason(), "agent_not_allowed");
}

#[tokio::test]
async fn assigning_a_plan_manages_its_entitlements_and_leaves_manual_ones() {
    let w = world();
    w.plans.apply(&mia(), V1, "v1").await.expect("v1");
    let c = w
        .customers
        .create(
            &mia(),
            NewCustomer {
                org: "acme".into(),
                subjects: vec!["acme".into(), "acme_ci".into()],
                currency: "EUR".into(),
                ..NewCustomer::default()
            },
        )
        .await
        .expect("customer");
    let (c, changed) = w
        .plans
        .assign(&mia(), &c.id, "pro", 0, "signed up")
        .await
        .expect("assign latest");
    assert_eq!(changed, 4, "two features for two subjects");
    assert_eq!(
        c.plan.as_ref().map(|(p, v)| (p.as_str(), *v)),
        Some(("pro", 1))
    );
    let acme = SubjectKey::new("acme").expect("subject");
    let cpu = FeatureKey::new("house_cpu").expect("feature");
    let e = w
        .store
        .entitlement_for(&acme, &cpu)
        .await
        .expect("read")
        .expect("created");
    match &e.kind {
        EntitlementKind::Metered {
            usage_period,
            mode: Mode::HardLimit,
            ..
        } => assert_eq!(
            usage_period.anchor,
            UtcNanos(1_700_000_000_000_000_000 - (1_700_000_000_000_000_000 % UtcNanos::HOUR))
        ),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        e.source,
        EntitlementSource::Plan { version: 1, .. }
    ));

    // v2 drops byoc and raises the allowance; a manual byoc entitlement on
    // acme_ci is kept.
    let ci = SubjectKey::new("acme_ci").expect("subject");
    let byoc = FeatureKey::new("byoc").expect("feature");
    let mut manual = w
        .store
        .entitlement_for(&ci, &byoc)
        .await
        .expect("read")
        .expect("plan-made");
    manual.source = EntitlementSource::Manual {
        by: "user:mia".into(),
    };
    w.store
        .update_entitlement(manual)
        .await
        .expect("make manual");
    let v2 = V1
        .replace("version: 1", "version: 2")
        .replace("issue_after_reset: 1000", "issue_after_reset: 5000")
        .replace("  - feature: byoc\n    boolean: true\n", "");
    w.plans.apply(&mia(), &v2, "v2").await.expect("v2");
    let (_, changed) = w
        .plans
        .assign(&mia(), &c.id, "pro", 2, "upgrade")
        .await
        .expect("assign v2");
    assert_eq!(changed, 3, "two allowances raised, acme's byoc removed");
    assert!(
        w.store
            .entitlement_for(&acme, &byoc)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        w.store
            .entitlement_for(&ci, &byoc)
            .await
            .expect("read")
            .is_some(),
        "a manual entitlement stays"
    );

    // Unassigning removes what the plan made.
    let c = w
        .plans
        .unassign(&mia(), &c.id, "churn")
        .await
        .expect("unassign");
    assert!(c.plan.is_none());
    assert!(
        w.store
            .entitlement_for(&acme, &cpu)
            .await
            .expect("read")
            .is_none()
    );
    let actions: Vec<String> = w.audit.events().into_iter().map(|a| a.action).collect();
    assert!(actions.contains(&"meter.plan.assign".to_string()));
    assert!(actions.contains(&"meter.plan.unassign".to_string()));
}
