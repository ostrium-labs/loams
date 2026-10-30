//! The scheduled exports (D2403): the mapping and its defaults, Stripe's
//! periods, Lago's windows and OpenMeter's raw events, run through
//! `ExportSchedule` against a fake House, a fake stream and local
//! stand-ins for the providers' APIs.

#![cfg(any(
    feature = "export-stripe",
    feature = "export-lago",
    feature = "export-openmeter",
    feature = "export-webhook"
))]

use std::collections::BTreeMap;

use loams_meter::export::{ExportMapping, period_hours_sql, stripe_periods};
use loams_meter::store::{Customer, PaymentState, Records, SpendControls};
use loams_meter_core::{
    Aggregation, Amount, Currency, Entitlement, EntitlementId, EntitlementKind, EntitlementSource,
    Feature, FeatureKey, Interval, JsonPath, Meter, Mode, Slug, SubjectKey, Unit, UsagePeriod,
    UtcNanos,
};

const TYPE: &str = "io.loams.dev.usage.house.query.v1";
/// 2026-10-19T00:00:00Z.
const DAY0: i64 = 1_792_368_000;

fn meter(slug: &str, aggregation: Aggregation) -> Meter {
    Meter {
        slug: Slug::new(slug).expect("slug"),
        event_type: TYPE.into(),
        aggregation,
        value_property: (aggregation != Aggregation::Count)
            .then(|| JsonPath::parse("$.cpu_ms").expect("path")),
        group_by: BTreeMap::from([(
            "namespace".to_string(),
            JsonPath::parse("$.namespace").expect("path"),
        )]),
        event_from: None,
    }
}

fn feature(key: &str, meter: &str) -> Feature {
    Feature {
        key: FeatureKey::new(key).expect("feature"),
        meter: Some(Slug::new(meter).expect("slug")),
        filter: BTreeMap::from([("namespace".to_string(), "7".to_string())]),
        unit: Unit::Ms,
        archived: false,
    }
}

fn entitlement(id: &str, subject: &str, feature: &str) -> Entitlement {
    Entitlement {
        id: EntitlementId::new(id).expect("id"),
        subject: SubjectKey::new(subject).expect("subject"),
        feature: FeatureKey::new(feature).expect("feature"),
        kind: EntitlementKind::Metered {
            usage_period: UsagePeriod {
                interval: Interval::Day,
                anchor: UtcNanos(DAY0 * UtcNanos::SECOND),
            },
            mode: Mode::HardLimit,
            issue_after_reset: Some(Amount::from_units(1000)),
            preserve_overage_at_reset: false,
        },
        created_at: UtcNanos(DAY0 * UtcNanos::SECOND),
        source: EntitlementSource::Manual { by: "test".into() },
    }
}

fn customer(id: &str, subject: &str, stripe: Option<&str>) -> Customer {
    Customer {
        id: id.into(),
        org: "acme".into(),
        display_name: String::new(),
        subjects: vec![SubjectKey::new(subject).expect("subject")],
        payment_state: PaymentState::Card,
        spend_controls: SpendControls::default(),
        stripe_customer_id: stripe.map(str::to_string),
        currency: Currency::EUR,
        plan: None,
        created_at: UtcNanos(0),
        updated_at: UtcNanos(0),
        version: 1,
    }
}

fn records() -> Records {
    Records {
        customers: vec![
            customer("cust_a", "org_a", Some("cus_A")),
            // No Stripe id: never in Stripe's export.
            customer("cust_b", "org_b", None),
        ],
        entitlements: vec![
            entitlement("e1", "org_a", "house_cpu"),
            entitlement("e2", "org_a", "house_users"),
            entitlement("e3", "org_b", "house_cpu"),
            entitlement("e4", "org_a", "house_queries"),
        ],
        grants: Vec::new(),
        meters: vec![
            meter("house_cpu_ms", Aggregation::Sum),
            // UNIQUE_COUNT hours do not add up to a period: not exported.
            meter("house_active_users", Aggregation::UniqueCount),
            meter("house_query_count", Aggregation::Count),
        ],
        features: vec![
            feature("house_cpu", "house_cpu_ms"),
            feature("house_users", "house_active_users"),
            feature("house_queries", "house_query_count"),
        ],
    }
}

#[test]
fn the_mapping_has_defaults_and_overrides() {
    let empty = ExportMapping::parse("").expect("empty");
    assert_eq!(empty, ExportMapping::default());
    assert_eq!(
        empty
            .stripe_event_name("house_cpu", "house_cpu_ms")
            .as_deref(),
        Some("house_cpu_ms")
    );
    assert_eq!(
        empty.lago_code("house_cpu_ms").as_deref(),
        Some("house_cpu_ms")
    );
    assert_eq!(empty.lago_subscription("cust_a", "org_a"), "cust_a");

    let m = ExportMapping::parse(
        "stripe:\n  features: [house_cpu]\n  event_names: {house_cpu: loams_cpu}\n\
         lago:\n  meters: [house_cpu_ms]\n  codes: {house_cpu_ms: cpu}\n  \
         subscriptions: {cust_a: sub_a, org_c: sub_c}\n",
    )
    .expect("yaml");
    assert_eq!(
        m.stripe_event_name("house_cpu", "house_cpu_ms").as_deref(),
        Some("loams_cpu")
    );
    assert_eq!(
        m.stripe_event_name("house_queries", "house_query_count"),
        None
    );
    assert_eq!(m.lago_code("house_cpu_ms").as_deref(), Some("cpu"));
    assert_eq!(m.lago_code("house_query_count"), None);
    assert_eq!(m.lago_subscription("cust_a", "org_a"), "sub_a");
    assert_eq!(m.lago_subscription("cust_c", "org_c"), "sub_c");
    // JSON is YAML; unknown keys are refused, not ignored.
    assert!(ExportMapping::parse("{\"lago\": {\"codes\": {}}}").is_ok());
    assert!(ExportMapping::parse("stripe: {event_name: x}").is_err());
}

#[test]
fn stripe_periods_are_the_billable_features_of_customers_with_a_stripe_id() {
    // 2026-10-20T10:00Z: the 20th's period, and the 19th's for 26 hours
    // after it ended.
    let now = DAY0 + 86_400 + 36_000;
    let periods = stripe_periods(&records(), &ExportMapping::default(), now);
    let mut seen: Vec<(String, (i64, i64))> = periods
        .iter()
        .map(|p| (p.event_name.clone(), p.period))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("house_cpu_ms".to_string(), (DAY0, DAY0 + 86_400)),
            (
                "house_cpu_ms".to_string(),
                (DAY0 + 86_400, DAY0 + 2 * 86_400)
            ),
            ("house_query_count".to_string(), (DAY0, DAY0 + 86_400)),
            (
                "house_query_count".to_string(),
                (DAY0 + 86_400, DAY0 + 2 * 86_400)
            ),
        ]
    );
    assert!(periods.iter().all(|p| p.subject == "org_a"
        && p.stripe_customer_id == "cus_A"
        && p.included == loams_meter_export::Quantity::from_units(1000)));
    // A day later the 19th is past its lookback.
    let later = stripe_periods(&records(), &ExportMapping::default(), now + 86_400);
    assert!(later.iter().all(|p| p.period.0 >= DAY0 + 86_400));

    // The hours of a period: up to now, its feature's groups summed.
    let p = &periods
        .iter()
        .find(|p| p.event_name == "house_cpu_ms" && p.period.0 == DAY0 + 86_400)
        .expect("today");
    let sql = period_hours_sql(p, now);
    assert!(sql.contains("_meter.windows FINAL"), "{sql}");
    assert!(sql.contains("meter = 'house_cpu_ms'"), "{sql}");
    assert!(sql.contains("subject = 'org_a'"), "{sql}");
    assert!(
        sql.contains("JSONExtractString(group_key, 'namespace') = '7'"),
        "{sql}"
    );
    assert!(
        sql.contains("window_start < toDateTime(1792490400"),
        "{sql}"
    );
}

#[cfg(all(
    feature = "export-stripe",
    feature = "export-lago",
    feature = "export-openmeter"
))]
mod scheduled {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use loams_meter::export::{ExportMapping, ExportSchedule, Exporters};
    use loams_meter::owner::{OwnerError, PartitionReader};
    use loams_meter::store::{MemStore, MeterStore};
    use loams_meter_core::{Event, UtcNanos};
    use loams_meter_export::lago::{LagoConfig, LagoExporter};
    use loams_meter_export::openmeter::{OpenMeterConfig, OpenMeterExporter};
    use loams_meter_export::stripe::{StripeConfig, StripeExporter};
    use loams_meter_export::{ExportCursors, MemCursors, MemLedger, RetryPolicy};
    use loams_meter_house::{HouseError, HouseSql};

    use super::*;

    /// Answers `_meter.windows` reads: the period hours of org_a, and two
    /// closed hour windows for the window exporters.
    #[derive(Debug, Default)]
    struct FakeHouse {
        seen: Mutex<Vec<String>>,
    }

    impl HouseSql for FakeHouse {
        async fn query(&self, sql: &str) -> Result<String, HouseError> {
            self.seen.lock().expect("lock").push(sql.to_string());
            if sql.contains("sum(value), max(close_run)") {
                // 1500 in the first hour (500 over), 200 in the second.
                return Ok(format!(
                    "{}\t1500\t3\n{}\t200\t4\n",
                    DAY0 + 86_400,
                    DAY0 + 86_400 + 3600
                ));
            }
            Ok(format!(
                "house_cpu_ms\torg_a\t{h}\t{{\"namespace\":\"7\"}}\t12\t3\t9\n\
                 house_cpu_ms\torg_z\t{h}\t\t5\t1\t9\n",
                h = DAY0 + 86_400
            ))
        }
    }

    /// Two usage events on partition 3 and one of the meter's own.
    #[derive(Debug)]
    struct FakeStream(Vec<(u64, Event)>);

    #[async_trait]
    impl PartitionReader for FakeStream {
        async fn read(
            &self,
            partition: u32,
            from: u64,
            max: usize,
        ) -> Result<Vec<(u64, Event)>, OwnerError> {
            if partition != 3 {
                return Ok(Vec::new());
            }
            Ok(self
                .0
                .iter()
                .filter(|(o, _)| *o >= from)
                .take(max)
                .cloned()
                .collect())
        }
    }

    fn event(id: &str, ty: &str) -> Event {
        Event {
            id: id.into(),
            source: "loams://test/house".into(),
            r#type: ty.into(),
            subject: SubjectKey::new("org_a").expect("subject"),
            time: UtcNanos((DAY0 + 86_400) * UtcNanos::SECOND),
            tenant: Some(loams_meter_core::TenantId::new("42/7").expect("tenant")),
            lease: Some(loams_meter_core::LeaseId::new("l9k2m4q8x1").expect("lease")),
            data: serde_json::json!({"namespace": "7", "cpu_ms": 5})
                .as_object()
                .expect("object")
                .clone(),
        }
    }

    type Seen = Arc<Mutex<Vec<(String, serde_json::Value, String)>>>;

    /// One local server for the three providers: records each request's
    /// path, body (JSON or form) and answers each as accepted.
    async fn providers() -> (String, Seen) {
        // The process's TLS provider, as the loams binary installs it.
        let _ = rustls::crypto::ring::default_provider().install_default();
        use axum::http::{HeaderMap, Uri};
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let app = axum::Router::new().fallback(
            move |uri: Uri, headers: HeaderMap, body: axum::body::Bytes| {
                let s = s.clone();
                async move {
                    let text = String::from_utf8_lossy(&body).to_string();
                    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                    let ct = headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    s.lock().expect("lock").push((
                        uri.path().to_string(),
                        json,
                        format!("{ct}|{text}"),
                    ));
                    if uri.path().contains("meter_events") {
                        return axum::Json(serde_json::json!({
                            "object": "billing.meter_event",
                            "identifier": "x",
                            "event_name": "house_cpu_ms"
                        }))
                        .into_response_parts();
                    }
                    axum::Json(serde_json::json!({})).into_response_parts()
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (format!("http://{addr}"), seen)
    }

    trait IntoParts {
        fn into_response_parts(self) -> axum::response::Response;
    }

    impl<T: axum::response::IntoResponse> IntoParts for T {
        fn into_response_parts(self) -> axum::response::Response {
            self.into_response()
        }
    }

    fn fast() -> RetryPolicy {
        RetryPolicy {
            attempts: 1,
            ..RetryPolicy::default()
        }
    }

    #[tokio::test]
    async fn one_run_exports_stripe_periods_lago_windows_and_openmeter_events() {
        let (base, seen) = providers().await;
        let store = Arc::new(MemStore::new());
        let r = records();
        for m in r.meters {
            store.put_meter(m).await.expect("meter");
        }
        for f in r.features {
            store.put_feature(f).await.expect("feature");
        }
        for c in r.customers {
            store.create_customer(c, None).await.expect("customer");
        }
        for e in r.entitlements {
            store
                .create_entitlement(e, None)
                .await
                .expect("entitlement");
        }
        let mut stripe = StripeConfig::new("sk_test_x");
        stripe.api_base = base.clone();
        stripe.retry = fast();
        let mut lago = LagoConfig::new(base.clone(), "lago_key");
        lago.retry = fast();
        let mut om = OpenMeterConfig::new(base.clone());
        om.retry = fast();
        let cursors: Arc<dyn ExportCursors> = Arc::new(MemCursors::default());
        let ledger = Arc::new(MemLedger::default());
        let mapping = ExportMapping::parse(
            "stripe:\n  features: [house_cpu]\nlago:\n  subscriptions: {cust_a: sub_a}\n",
        )
        .expect("mapping");
        let now = DAY0 + 86_400 + 36_000;
        let schedule = ExportSchedule {
            house: Arc::new(FakeHouse::default()),
            store: store.clone() as Arc<dyn MeterStore>,
            exporters: Exporters {
                stripe: Some(StripeExporter::new(stripe, ledger.clone()).expect("stripe")),
                lago: Some(LagoExporter::new(lago, ledger.clone()).expect("lago")),
                openmeter: Some((
                    OpenMeterExporter::new(om, cursors.clone()).expect("openmeter"),
                    cursors.clone(),
                )),
                mapping,
                ..Exporters::default()
            },
            clock: Arc::new(move || UtcNanos(now * UtcNanos::SECOND)),
            stream: Some(Arc::new(FakeStream(vec![
                (10, event("q-1", TYPE)),
                (11, event("q-2", TYPE)),
                (12, event("t-1", "io.loams.dev.meter.threshold.v1")),
            ]))),
            owned: None,
        };

        // Stripe: org_a's house_cpu for the 19th and the 20th (the hours of
        // both answer the same here): 500 over, then nothing more.
        let sent = schedule.stripe_once().await.expect("stripe");
        assert!(sent >= 1, "{sent}");
        let requests = seen.lock().expect("lock").clone();
        let stripe_bodies: Vec<&String> = requests
            .iter()
            .filter(|(p, _, _)| p.ends_with("/v1/billing/meter_events"))
            .map(|(_, _, b)| b)
            .collect();
        assert!(!stripe_bodies.is_empty(), "{requests:?}");
        assert!(
            stripe_bodies
                .iter()
                .all(|b| b.contains("event_name=house_cpu_ms")
                    && b.contains("payload%5Bstripe_customer_id%5D=cus_A")),
            "{stripe_bodies:?}"
        );
        assert!(
            stripe_bodies
                .iter()
                .any(|b| b.contains("payload%5Bvalue%5D=500")),
            "{stripe_bodies:?}"
        );

        // Lago: org_a's window with its mapped subscription; org_z has no
        // customer and is not sent.
        seen.lock().expect("lock").clear();
        schedule.windows_once().await.expect("windows");
        let requests = seen.lock().expect("lock").clone();
        let lago: Vec<&serde_json::Value> = requests
            .iter()
            .filter(|(p, _, _)| p.ends_with("/api/v1/events/batch"))
            .map(|(_, j, _)| j)
            .collect();
        assert_eq!(lago.len(), 1, "{requests:?}");
        let events = lago[0]["events"].as_array().expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["external_subscription_id"], "sub_a");
        assert_eq!(events[0]["code"], "house_cpu_ms");

        // OpenMeter: the two usage events, without loamslease; the meter's
        // own event moves the cursor but is not sent.
        seen.lock().expect("lock").clear();
        assert_eq!(schedule.openmeter_once().await.expect("openmeter"), 2);
        let requests = seen.lock().expect("lock").clone();
        let om: Vec<&serde_json::Value> = requests
            .iter()
            .filter(|(p, _, _)| p.ends_with("/api/v1/events"))
            .map(|(_, j, _)| j)
            .collect();
        assert_eq!(om.len(), 1, "{requests:?}");
        let batch = om[0].as_array().expect("batch");
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().all(|e| e.get("loamslease").is_none()));
        assert_eq!(batch[0]["tenantid"], "42/7");
        assert_eq!(cursors.get("openmeter", 3).await.expect("cursor"), Some(13));
        // A second run has nothing new.
        seen.lock().expect("lock").clear();
        assert_eq!(schedule.openmeter_once().await.expect("again"), 0);
        assert!(seen.lock().expect("lock").is_empty());
    }
}
