//! The period closer's driver (ME1 Task 7) against a scripted House. The
//! SQL runs on a real House in `fabric/crates/loams-house-worker/tests/meter.rs`
//! (`period_closes_after_grace`).

use std::sync::Mutex;

use loams_meter_house::period::periods_back;
use loams_meter_house::{
    Aggregation, FeatureDef, HouseError, HouseSql, MeterDef, PeriodCloser, PeriodOutcome,
    PeriodSpec,
};

struct Scripted {
    seen: Mutex<Vec<String>>,
}

impl HouseSql for Scripted {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.seen.lock().expect("lock").push(sql.to_string());
        Ok(String::new())
    }
}

#[test]
fn periods_back_lists_oldest_first() {
    let hourly = |n: i64| (n * 3_600, (n + 1) * 3_600);
    assert_eq!(
        periods_back(10, 3, hourly),
        [
            (25_200, 28_800),
            (28_800, 32_400),
            (32_400, 36_000),
            (36_000, 39_600)
        ]
    );
    assert_eq!(periods_back(10, -1, hourly), [(36_000, 39_600)]);
}

#[tokio::test]
async fn period_closer_closes_due_periods_for_every_spec() {
    let house = Scripted {
        seen: Mutex::new(Vec::new()),
    };
    let feature = FeatureDef {
        key: "house_cpu".to_string(),
        meter: MeterDef {
            slug: "house_cpu_ms".to_string(),
            event_type: "io.loams.dev.usage.house.query.v1".to_string(),
            aggregation: Aggregation::Sum,
            value_property: Some("$.cpu_ms".to_string()),
            group_by: Vec::new(),
            event_from: None,
        },
        filter: Vec::new(),
        byte_hours: false,
    };
    let hourly = |n: i64| (n * 3_600, (n + 1) * 3_600);
    // Now is 10:30 of day 0: 07:00 (ended 08:00) is due; 08:00, 09:00 and 10:00 are not.
    let now_ms = (10 * 3_600 + 1_800) * 1_000;
    let specs = [
        PeriodSpec {
            subject: "org_1".to_string(),
            feature: feature.clone(),
            periods: periods_back(10, 3, hourly),
        },
        PeriodSpec {
            subject: "org_2".to_string(),
            feature,
            periods: periods_back(10, 0, hourly),
        },
    ];
    let results = PeriodCloser::default()
        .run_once(&house, &specs, now_ms)
        .await;
    let outcomes: Vec<_> = results[0]
        .2
        .as_ref()
        .expect("org_1")
        .periods
        .iter()
        .map(|p| p.2)
        .collect();
    assert_eq!(
        outcomes,
        [
            PeriodOutcome::Closed,
            PeriodOutcome::Provisional,
            PeriodOutcome::Provisional,
            PeriodOutcome::Provisional
        ]
    );
    assert_eq!(
        results[1].2.as_ref().expect("org_2").periods,
        [(36_000, 39_600, PeriodOutcome::Provisional)]
    );
    let inserts = house
        .seen
        .lock()
        .expect("lock")
        .iter()
        .filter(|s| s.starts_with("INSERT INTO _meter.period_totals"))
        .count();
    assert_eq!(inserts, 5);
}
