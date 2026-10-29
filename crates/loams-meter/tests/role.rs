//! The role put together (role wiring, part 1): records in versioned
//! documents, the operator's standing, and balances refused until the
//! counters run.

use std::sync::Arc;

use loams_authz::BuiltinAuthorizer;
use loams_meter::audit::MemAudit;
use loams_meter::authz::Caller;
use loams_meter::customers::NewCustomer;
use loams_meter::notify::notifier::DocNotifications;
use loams_meter::notify::webhook::{Outcome, Transport};
use loams_meter::role::{CountersNotRunning, MeterRole, NoHouseYet, RoleParts};
use loams_meter::store::MemDoc;
use loams_meter_core::UtcNanos;

/// No network in these tests.
#[derive(Debug)]
struct NoNet;

#[async_trait::async_trait]
impl Transport for NoNet {
    async fn post(&self, _: &str, _: &[(&str, String)], _: &[u8]) -> Outcome {
        Outcome::Failed("no network in tests".into())
    }
}

fn role(records: Arc<MemDoc>) -> MeterRole {
    MeterRole::new(RoleParts {
        records,
        notifications: Arc::new(DocNotifications::new(Arc::new(MemDoc::default()))),
        authorizer: Arc::new(BuiltinAuthorizer::new()),
        audit: Arc::new(MemAudit::new()),
        transport: Arc::new(NoNet),
        mailer: None,
        console: None,
        random: Arc::new(|| [5; 32]),
        clock: Arc::new(|| UtcNanos(1_700_000_000_000_000_000)),
        values: Arc::new(CountersNotRunning),
        usage: Arc::new(NoHouseYet),
        house: Arc::new(loams_meter::house::RoleHouse::Absent),
    })
}

const FREE: &str = r#"
plan: free
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
    metered: { period: { interval: month }, mode: hard_limit, issue_after_reset: 100 }
"#;

#[tokio::test]
async fn the_operator_runs_the_meter_and_records_outlive_the_instance() {
    let records = Arc::new(MemDoc::default());
    let r = role(records.clone());
    let c = r
        .customers
        .create(
            &Caller::Operator,
            NewCustomer {
                org: "acme".into(),
                subjects: vec!["acme".into()],
                ..NewCustomer::default()
            },
        )
        .await
        .expect("the operator creates a customer");
    r.plans
        .apply(&Caller::Operator, FREE, "bootstrap")
        .await
        .expect("apply");
    r.plans
        .assign(&Caller::Operator, &c.id, "free", 0, "bootstrap")
        .await
        .expect("assign");

    // Balances wait for the counters (part 2): refused, not guessed.
    let err = r
        .entitlements
        .value(&Caller::Operator, "acme", "house_cpu")
        .await
        .expect_err("no counters yet");
    assert_eq!(err.reason(), "meter_unavailable");

    // Another instance over the same document sees the records.
    let other = role(records);
    let (list, _) = other
        .entitlements
        .list(&Caller::Operator, Some("acme"), None, None, 10)
        .await
        .expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].feature.as_str(), "house_cpu");

    // While no House answers, every existing meter counts as used.
    let changed = FREE
        .replace("version: 1", "version: 2")
        .replace("aggregation: SUM", "aggregation: MAX");
    let err = other
        .plans
        .apply(&Caller::Operator, &changed, "x")
        .await
        .expect_err("no House to ask");
    assert_eq!(err.reason(), "meter_semantics_immutable");
}
