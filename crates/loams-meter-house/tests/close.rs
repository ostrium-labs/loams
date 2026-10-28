//! The close job's driver (ME1 Task 6) against a scripted House. The SQL
//! itself runs on a real House in
//! `fabric/crates/loams-house-worker/tests/meter.rs` (`close_job_idempotent`,
//! `late_event_recloses_window`, `unique_count_not_rolled_up`).

use std::sync::Mutex;

use loams_meter_house::{Aggregation, CloseError, CloseJob, HouseError, HouseSql, MeterDef};

/// Answers the close job's reads; records every statement.
struct Scripted {
    seen: Mutex<Vec<String>>,
    minutes: &'static str,
}

impl HouseSql for Scripted {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.seen.lock().expect("lock").push(sql.to_string());
        if sql.contains("meter = 'broken'") {
            return Err(HouseError::Query {
                code: 60,
                message: "Table _meter.close_runs does not exist".to_string(),
            });
        }
        if sql.starts_with("SELECT DISTINCT") {
            Ok(self.minutes.to_string())
        } else {
            Ok(String::new())
        }
    }
}

fn meter(slug: &str, aggregation: Aggregation) -> MeterDef {
    MeterDef {
        slug: slug.to_string(),
        event_type: "io.loams.dev.usage.house.query.v1".to_string(),
        aggregation,
        value_property: Some("$.cpu_ms".to_string()),
        group_by: Vec::new(),
        event_from: None,
    }
}

#[tokio::test]
async fn close_job_runs_every_meter_and_isolates_failures() {
    // 08:54 and 09:42 on 2026-10-19: two hours, one day.
    let house = Scripted {
        seen: Mutex::new(Vec::new()),
        minutes: "1792400040\n1792402920\n",
    };
    let meters = [
        meter("house_cpu_ms", Aggregation::Sum),
        meter("broken", Aggregation::Sum),
        meter("house_users", Aggregation::UniqueCount),
    ];
    let results = CloseJob::default()
        .run_once(&house, &meters, 1_792_404_030_000)
        .await;
    assert_eq!(results.len(), 3);
    let ok = results[0].1.as_ref().expect("house_cpu_ms closes");
    assert_eq!((ok.minutes, ok.hours, ok.days), (2, 2, 1));
    assert!(matches!(
        results[1].1,
        Err(CloseError::House(HouseError::Query { code: 60, .. }))
    ));
    assert!(results[2].1.is_ok());
    let seen = house.seen.lock().expect("lock").clone();
    // The broken meter recorded nothing; the others recorded their runs.
    let records: Vec<_> = seen
        .iter()
        .filter(|s| s.starts_with("INSERT INTO _meter.close_runs"))
        .collect();
    assert_eq!(records.len(), 2, "{records:#?}");
    assert!(records.iter().all(|r| !r.contains("'broken'")));
}
