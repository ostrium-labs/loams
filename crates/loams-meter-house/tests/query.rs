//! `QueryMeter` and OpenMeter's query path (ME1 Task 8) against a scripted
//! House. The SQL runs on a real House in
//! `fabric/crates/loams-house-worker/tests/meter.rs`
//! (`query_marks_provisional_open_window`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use loams_meter_core::{Aggregation, JsonPath, Meter, Slug, UtcNanos, WindowSize};
use loams_meter_house::query::{
    MeterCatalog, QueryState, handle_query, meter_def, parse_query, query_spec,
};
use loams_meter_house::{HouseError, HouseSql, query_meter};

fn cpu_meter() -> Meter {
    Meter {
        slug: Slug::new("house_cpu_ms").expect("slug"),
        event_type: "io.loams.dev.usage.house.query.v1".to_string(),
        aggregation: Aggregation::Sum,
        value_property: Some(JsonPath::parse("$.cpu_ms").expect("path")),
        group_by: BTreeMap::from([(
            "namespace".to_string(),
            JsonPath::parse("$.namespace").expect("path"),
        )]),
        event_from: None,
    }
}

/// Answers the query's reads: the close run, the closed minutes, the raw tail.
struct Scripted {
    seen: Mutex<Vec<String>>,
}

impl HouseSql for Scripted {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.seen.lock().expect("lock").push(sql.to_string());
        // 2026-10-19: 09:00 = 1792400400, closed to 09:58 = 1792403880.
        Ok(if sql.contains("_meter.close_runs") {
            "1792404000\t1792403880\n".to_string()
        } else if sql.contains("_meter.windows") {
            "1792400400\t1792404000\torg_42\t7\t1840\n".to_string()
        } else {
            "1792400400\t1792404000\torg_42\t7\t999\n".to_string()
        })
    }
}

struct Catalog;

impl MeterCatalog for Catalog {
    fn meter(&self, slug: &str) -> Option<Meter> {
        (slug == "house_cpu_ms").then(cpu_meter)
    }
}

#[test]
fn parses_openmeter_parameters() {
    let q = parse_query(
        "from=2026-10-19T09%3A00%3A00Z&to=2026-10-19T10:00:00Z&windowSize=HOUR&windowTimeZone=UTC\
         &subject=org_42&subject=org_7&groupBy=namespace&groupBy=subject\
         &filterGroupBy%5Bnamespace%5D=7",
    )
    .expect("parses");
    assert_eq!(
        q.from,
        UtcNanos::from_rfc3339("2026-10-19T09:00:00Z").expect("t")
    );
    assert_eq!(q.window_size, Some(WindowSize::Hour));
    assert_eq!(q.subjects, ["org_42", "org_7"]);
    assert_eq!(q.group_by, ["namespace", "subject"]);
    assert_eq!(q.filter_group_by["namespace"], ["7"]);
    let spec = query_spec(&cpu_meter(), &q).expect("spec");
    assert!(spec.by_subject);
    assert_eq!(spec.group_by, ["namespace"]);
    assert_eq!((spec.from, spec.to), (1_792_400_400, 1_792_404_000));

    for bad in [
        "to=2026-10-19T10:00:00Z",
        "from=2026-10-19T10:00:00Z&to=2026-10-19T09:00:00Z",
        "from=2026-10-19T09:00:00Z&windowSize=WEEK",
        "from=2026-10-19T09:00:00Z&windowTimeZone=Europe/Paris",
        "from=2026-10-19T09:00:00Z&color=red",
    ] {
        assert!(parse_query(bad).is_err(), "{bad}");
    }
    let q = parse_query("from=2026-10-19T09:00:00Z&to=2026-10-19T10:00:00Z&groupBy=model")
        .expect("parses");
    assert!(query_spec(&cpu_meter(), &q).is_err());
}

#[test]
fn converts_me_core_meters() {
    let def = meter_def(&cpu_meter());
    assert_eq!(def.slug, "house_cpu_ms");
    assert_eq!(def.value_property.as_deref(), Some("$.cpu_ms"));
    assert_eq!(
        def.group_by,
        [("namespace".to_string(), "$.namespace".to_string())]
    );
    assert_eq!(def.aggregation, loams_meter_house::Aggregation::Sum);
}

#[tokio::test]
async fn query_combines_closed_minutes_and_raw_tail() {
    let house = Scripted {
        seen: Mutex::new(Vec::new()),
    };
    let q = parse_query(
        "from=2026-10-19T09:00:00Z&to=2026-10-19T10:00:00Z&windowSize=HOUR&subject=org_42\
         &groupBy=namespace",
    )
    .expect("parses");
    let answer = query_meter(&house, &cpu_meter(), &q).await.expect("answer");
    assert_eq!(answer.as_of, Some(1_792_403_880));
    assert_eq!(answer.rows.len(), 1);
    assert_eq!(answer.rows[0].value, 2_839_000_000_000);
    assert!(answer.rows[0].provisional);
    assert_eq!(house.seen.lock().expect("lock").len(), 3);
}

#[tokio::test]
async fn openmeter_route_answers_and_refuses() {
    let state = Arc::new(QueryState {
        house: Scripted {
            seen: Mutex::new(Vec::new()),
        },
        meters: Catalog,
    });
    let ok = handle_query(
        &state,
        "house_cpu_ms",
        "from=2026-10-19T09:00:00Z&to=2026-10-19T10:00:00Z&windowSize=HOUR&subject=org_42\
         &groupBy=namespace",
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), 1 << 20)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["windowSize"], "HOUR");
    assert_eq!(body["data"][0]["value"], 2839);
    assert_eq!(body["data"][0]["subject"], "org_42");
    assert_eq!(body["data"][0]["groupBy"]["namespace"], "7");
    assert_eq!(body["data"][0]["provisional"], true);
    assert_eq!(
        UtcNanos::from_rfc3339(body["as_of"].as_str().expect("as_of")).expect("time"),
        UtcNanos::from_secs(1_792_403_880)
    );
    assert_eq!(
        UtcNanos::from_rfc3339(body["data"][0]["windowEnd"].as_str().expect("end")).expect("t"),
        UtcNanos::from_secs(1_792_404_000)
    );

    let missing = handle_query(&state, "nope", "from=2026-10-19T09:00:00Z").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let bad = handle_query(&state, "house_cpu_ms", "windowSize=HOUR").await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    // The router itself builds (the role mounts it).
    let _router = loams_meter_house::openmeter_router(state);
}
