//! The role's House side (ME1 role wiring, part 2b): meter usage from
//! `_meter.events`, the close lap's statements, period specs and the
//! OpenMeter query route's authentication.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use loams_authz::BuiltinAuthorizer;
use loams_meter::authz::MeterAuthz;
use loams_meter::house::{
    HouseSchedule, HouseUsage, RoleHouse, has_events_sql, meter_defs, period_specs, query_router,
};
use loams_meter::plans::MeterUsage;
use loams_meter::queries::MeterQueries;
use loams_meter::store::{MemStore, MeterStore, Records};
use loams_meter_core::{
    Aggregation, Entitlement, EntitlementId, EntitlementKind, EntitlementSource, Feature,
    FeatureKey, Interval, JsonPath, Meter, Mode, Slug, SubjectKey, Unit, UsagePeriod, UtcNanos,
};
use loams_meter_house::{HouseError, HouseSql};
use tower::ServiceExt;

const TYPE: &str = "io.loams.dev.usage.house.cpu.v1";

/// A House that records every statement and answers `answer`.
#[derive(Debug, Default)]
struct FakeHouse {
    seen: Mutex<Vec<String>>,
    answer: String,
}

impl HouseSql for FakeHouse {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.seen.lock().expect("lock").push(sql.to_string());
        Ok(self.answer.clone())
    }
}

fn meter() -> Meter {
    Meter {
        slug: Slug::new("house_cpu_ms").expect("slug"),
        event_type: TYPE.into(),
        aggregation: Aggregation::Sum,
        value_property: Some(JsonPath::parse("$.cpu_ms").expect("path")),
        group_by: BTreeMap::new(),
        event_from: None,
    }
}

fn records(interval: Interval) -> Records {
    Records {
        customers: Vec::new(),
        entitlements: vec![Entitlement {
            id: EntitlementId::new("ent1").expect("id"),
            subject: SubjectKey::new("org_a").expect("subject"),
            feature: FeatureKey::new("house_cpu").expect("feature"),
            kind: EntitlementKind::Metered {
                usage_period: UsagePeriod {
                    interval,
                    anchor: UtcNanos(0),
                },
                mode: Mode::HardLimit,
                issue_after_reset: None,
                preserve_overage_at_reset: false,
            },
            created_at: UtcNanos(0),
            source: EntitlementSource::Manual { by: "test".into() },
        }],
        grants: Vec::new(),
        meters: vec![meter()],
        features: vec![Feature {
            key: FeatureKey::new("house_cpu").expect("feature"),
            meter: Some(Slug::new("house_cpu_ms").expect("slug")),
            filter: BTreeMap::new(),
            unit: Unit::Ms,
            archived: false,
        }],
    }
}

#[tokio::test]
async fn a_meter_has_events_when_the_house_finds_one() {
    let store = Arc::new(MemStore::new());
    store.put_meter(meter()).await.expect("meter");
    let slug = Slug::new("house_cpu_ms").expect("slug");

    let used = HouseUsage {
        house: Arc::new(FakeHouse {
            answer: "1\n".into(),
            ..FakeHouse::default()
        }),
        store: store.clone(),
    };
    assert!(used.has_events(&slug).await.expect("an answer"));
    let sql = used.house.seen.lock().expect("lock")[0].clone();
    assert!(sql.contains("_meter.events"), "{sql}");
    assert!(sql.contains(TYPE), "{sql}");

    let unused = HouseUsage {
        house: Arc::new(FakeHouse::default()),
        store: store.clone(),
    };
    assert!(!unused.has_events(&slug).await.expect("an answer"));

    let unknown = Slug::new("nobody_uses_this").expect("slug");
    assert!(!unused.has_events(&unknown).await.expect("an answer"));
}

#[tokio::test]
async fn without_a_house_meter_usage_is_unavailable_never_a_guess() {
    let store = Arc::new(MemStore::new());
    store.put_meter(meter()).await.expect("meter");
    let usage = HouseUsage {
        house: Arc::new(RoleHouse::Absent),
        store,
    };
    let answer = usage
        .has_events(&Slug::new("house_cpu_ms").expect("slug"))
        .await;
    assert!(
        matches!(answer, Err(loams_meter::error::MeterError::Unavailable(_))),
        "{answer:?}"
    );
}

#[test]
fn the_has_events_query_is_one_row_at_most() {
    let defs = meter_defs(&records(Interval::Month));
    assert_eq!(defs.len(), 1);
    let sql = has_events_sql(&defs[0]);
    assert!(sql.ends_with("LIMIT 1"), "{sql}");
}

#[test]
fn period_specs_cover_every_metered_entitlement() {
    let now = UtcNanos(10 * UtcNanos::DAY);
    let daily = period_specs(&records(Interval::Day), now);
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].subject, "org_a");
    assert_eq!(daily[0].feature.key, "house_cpu");
    assert_eq!(daily[0].periods.len(), 3, "the current day and two before");
    let hourly = period_specs(&records(Interval::Hour), now);
    assert_eq!(hourly[0].periods.len(), 5, "the current hour and four before");
}

#[tokio::test]
async fn a_close_lap_runs_the_close_job_and_the_period_close() {
    let store = Arc::new(MemStore::new());
    store.put_meter(meter()).await.expect("meter");
    let house = Arc::new(FakeHouse::default());
    let schedule = HouseSchedule::new(
        house.clone(),
        store as Arc<dyn MeterStore>,
        Arc::new(|| UtcNanos(10 * UtcNanos::DAY)),
    );
    // The fake House answers nothing, so the meter's run stops at its first
    // answer; what matters here is that the lap asked.
    let _ = schedule.lap().await.expect("records read");
    let seen = house.seen.lock().expect("lock");
    assert!(!seen.is_empty(), "the close job asked the House");
    assert!(seen.iter().all(|s| s.contains("_meter.")), "{seen:?}");
}

#[tokio::test]
async fn the_openmeter_query_route_needs_a_caller() {
    let store = Arc::new(MemStore::new());
    let queries = MeterQueries::new(
        store,
        MeterAuthz::new(Arc::new(BuiltinAuthorizer::new()), "meter-export"),
        Arc::new(RoleHouse::Absent),
    );
    let response = query_router(queries)
        .oneshot(
            Request::get("/api/v1/meters/house_cpu_ms/query")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
