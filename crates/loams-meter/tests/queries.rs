//! `MeterService.QueryMeter` (ME1 Task 8): who may query, and the Connect
//! face over the same House query as OpenMeter's
//! `GET /api/v1/meters/{slug}/query`, against a scripted House. The House
//! SQL itself runs on a real House in
//! `fabric/crates/loams-house-worker/tests/meter.rs`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use loams_authz::{BuiltinAuthorizer, Subject, Tuple};
use loams_meter::audit::MemAudit;
use loams_meter::authz::{Caller, MeterAuthz};
use loams_meter::connect::meters;
use loams_meter::customers::{Customers, NewCustomer};
use loams_meter::error::MeterError;
use loams_meter::queries::MeterQueries;
use loams_meter::store::{MemStore, MeterStore};
use loams_meter_core::{Aggregation, JsonPath, Meter, Slug, UtcNanos, WindowSize};
use loams_meter_house::query::MeterQuery;
use loams_meter_house::{HouseError, HouseSql};
use tower::ServiceExt;

const NOW: UtcNanos = UtcNanos(1_792_404_000_000_000_000);
/// 2026-10-19T09:00Z and 10:00Z.
const H09: i64 = 1_792_400_400;
const H10: i64 = 1_792_404_000;

/// Answers the query's reads: closed to 09:58, a closed minute window and a
/// raw tail; records every statement.
#[derive(Debug, Default)]
struct Scripted {
    seen: Mutex<Vec<String>>,
}

impl HouseSql for Scripted {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.seen.lock().expect("lock").push(sql.to_string());
        // Rows are per subject only when the query names subjects.
        let subject = if sql.contains("subject IN (") {
            "\tacme"
        } else {
            ""
        };
        Ok(if sql.contains("_meter.close_runs") {
            format!("{H10}\t{}\n", H10 - 120)
        } else if sql.contains("_meter.windows") {
            format!("{H09}\t{H10}{subject}\t7\t1840\n")
        } else {
            format!("{H09}\t{H10}{subject}\t7\t999.5\n")
        })
    }
}

struct World {
    queries: MeterQueries<Scripted>,
    house: Arc<Scripted>,
}

async fn world() -> World {
    let fga = BuiltinAuthorizer::new();
    fga.write(&[
        Tuple::new("platform:default", "platform", "org:acme"),
        Tuple::new("platform:default", "platform", "org:zeta"),
        Tuple::new("user:mia", "meter_admin", "platform:default"),
        Tuple::new("user:bill", "billing_viewer", "org:acme"),
        Tuple::new("user:zed", "billing_viewer", "org:zeta"),
    ])
    .expect("tuples");
    let authz = MeterAuthz::new(Arc::new(fga), "meter-export");
    let store = Arc::new(MemStore::new());
    let clock: loams_meter::customers::Clock = Arc::new(|| NOW);
    let customers = Customers::new(
        store.clone(),
        authz.clone(),
        Arc::new(MemAudit::new()),
        clock,
    );
    customers
        .create(
            &Caller::Principal(Subject::user("mia")),
            NewCustomer {
                org: "acme".into(),
                display_name: "Acme".into(),
                subjects: vec!["acme".into()],
                currency: "EUR".into(),
                stripe_customer_id: None,
                idempotency_key: None,
            },
        )
        .await
        .expect("customer");
    store
        .put_meter(Meter {
            slug: Slug::new("house_cpu_ms").expect("slug"),
            event_type: "io.loams.dev.usage.house.query.v1".into(),
            aggregation: Aggregation::Sum,
            value_property: Some(JsonPath::parse("$.cpu_ms").expect("path")),
            group_by: BTreeMap::from([(
                "namespace".to_string(),
                JsonPath::parse("$.namespace").expect("path"),
            )]),
            event_from: None,
        })
        .await
        .expect("meter");
    let house = Arc::new(Scripted::default());
    World {
        queries: MeterQueries::new(store, authz, house.clone()),
        house,
    }
}

fn person(id: &str) -> Caller {
    Caller::Principal(Subject::user(id))
}

fn hour_query(subjects: &[&str]) -> MeterQuery {
    MeterQuery {
        from: UtcNanos::from_secs(H09),
        to: UtcNanos::from_secs(H10),
        window_size: Some(WindowSize::Hour),
        subjects: subjects.iter().map(|s| s.to_string()).collect(),
        group_by: vec!["namespace".into()],
        filter_group_by: BTreeMap::new(),
    }
}

fn reason(e: &MeterError) -> &'static str {
    match e {
        MeterError::PermissionDenied { reason, .. } => reason,
        MeterError::InvalidArgument { .. } => "invalid",
        MeterError::NotFound { .. } => "not_found",
        _ => "other",
    }
}

#[tokio::test]
async fn query_meter_answers_admins_and_billing_viewers_of_the_subject() {
    let w = world().await;
    // A meter admin, and a billing viewer of acme naming acme's subject:
    // closed minutes plus the raw tail, provisional past as_of.
    for caller in [person("mia"), person("bill"), Caller::Operator] {
        let answered = w
            .queries
            .query(&caller, "house_cpu_ms", hour_query(&["acme"]))
            .await
            .expect("answered");
        assert_eq!(answered.group_by, ["namespace"]);
        assert_eq!(answered.answer.as_of, Some(H10 - 120));
        assert_eq!(answered.answer.rows.len(), 1);
        let row = &answered.answer.rows[0];
        assert_eq!(row.value, 2_839_500_000_000);
        assert_eq!(row.subject.as_deref(), Some("acme"));
        assert!(row.provisional);
    }
    // A meter admin may query every subject.
    w.queries
        .query(&person("mia"), "house_cpu_ms", hour_query(&[]))
        .await
        .expect("every subject");
}

#[tokio::test]
async fn query_meter_refuses_others() {
    let w = world().await;
    let before = w.house.seen.lock().expect("lock").len();
    let cases = [
        // A billing viewer of another org.
        (
            person("zed"),
            hour_query(&["acme"]),
            "billing_viewer_required",
        ),
        // Nobody in particular.
        (
            person("eve"),
            hour_query(&["acme"]),
            "billing_viewer_required",
        ),
        // A billing viewer who names no subject.
        (person("bill"), hour_query(&[]), "subjects_required"),
        // A subject with no customer.
        (
            person("bill"),
            hour_query(&["ghost"]),
            "billing_viewer_required",
        ),
        // A service identity.
        (
            Caller::Service("pg-control".into()),
            hour_query(&["acme"]),
            "service_not_allowed",
        ),
    ];
    for (caller, query, expected) in cases {
        let err = w
            .queries
            .query(&caller, "house_cpu_ms", query)
            .await
            .expect_err("refused");
        assert_eq!(reason(&err), expected, "{caller:?}: {err}");
    }
    // Refused before the House was asked anything.
    assert_eq!(w.house.seen.lock().expect("lock").len(), before);

    let mut backwards = hour_query(&["acme"]);
    backwards.to = backwards.from;
    let err = w
        .queries
        .query(&person("mia"), "house_cpu_ms", backwards)
        .await
        .expect_err("empty range");
    assert_eq!(reason(&err), "invalid");
    let err = w
        .queries
        .query(&person("mia"), "nope", hour_query(&["acme"]))
        .await
        .expect_err("no meter");
    assert_eq!(reason(&err), "not_found");
    let mut wrong = hour_query(&["acme"]);
    wrong.group_by = vec!["model".into()];
    let err = w
        .queries
        .query(&person("mia"), "house_cpu_ms", wrong)
        .await
        .expect_err("not a dimension");
    assert_eq!(reason(&err), "invalid");
}

/// The Connect face: `POST /loams.meter.v1.MeterService/QueryMeter` in
/// Connect's JSON, with the caller the role's transport layer would add.
#[tokio::test]
async fn query_meter_over_connect() {
    let w = world().await;
    let router = meters::register(connectrpc::Router::new(), w.queries.clone());
    let service = connectrpc::ConnectRpcService::new(router);
    let body = serde_json::json!({
        "meter": "house_cpu_ms",
        "from": "2026-10-19T09:00:00Z",
        "to": "2026-10-19T10:00:00Z",
        "windowSize": "WINDOW_SIZE_HOUR",
        "subjects": ["acme"],
        "groupBy": ["namespace"],
    });
    let call = |caller: Option<Caller>| {
        let mut request = Request::post("/loams.meter.v1.MeterService/QueryMeter")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        if let Some(caller) = caller {
            request.extensions_mut().insert(caller);
        }
        service.clone().oneshot(request)
    };

    let response = call(Some(person("bill"))).await.expect("served");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(Body::new(response.into_body()), 1 << 20)
        .await
        .expect("body");
    let answer: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(answer["windowSize"], "WINDOW_SIZE_HOUR");
    assert_eq!(answer["rows"][0]["value"], "2839.5");
    assert_eq!(answer["rows"][0]["subject"], "acme");
    assert_eq!(answer["rows"][0]["groupBy"][0]["dimension"], "namespace");
    assert_eq!(answer["rows"][0]["groupBy"][0]["value"], "7");
    assert_eq!(answer["rows"][0]["provisional"], true);
    assert_eq!(answer["asOf"], "2026-10-19T09:58:00Z");

    // No caller: unauthenticated; a billing viewer of another org: denied.
    let response = call(None).await.expect("served");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = call(Some(person("zed"))).await.expect("served");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// The request and response mappings.
#[test]
fn query_meter_messages_map() {
    use loams_proto::loams::meter::v1 as pb;
    let mut r = pb::QueryMeterRequest {
        meter: "house_cpu_ms".into(),
        window_time_zone: "Europe/Paris".into(),
        ..Default::default()
    };
    assert!(meters::query_in(&r).is_err(), "from is required");
    r.from = loams_meter::connect::timestamp(UtcNanos::from_secs(H09)).into();
    assert!(meters::query_in(&r).is_err(), "only UTC windows");
    r.window_time_zone = "UTC".into();
    r.filters.push(pb::DimensionFilter {
        dimension: "namespace".into(),
        values: vec!["7".into(), "8".into()],
        ..Default::default()
    });
    let q = meters::query_in(&r).expect("query");
    assert_eq!(q.from, UtcNanos::from_secs(H09));
    assert!(q.to > q.from, "to defaults to now");
    assert_eq!(q.window_size, None);
    assert_eq!(q.filter_group_by["namespace"], ["7", "8"]);
}
