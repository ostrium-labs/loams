//! The meter's House schema, applied through [`HouseSql`] (ME1 Task 5). The
//! statements themselves run against a real House in
//! `fabric/crates/loams-house-worker/tests/meter.rs`.

use std::sync::Mutex;

use loams_meter_house::{HouseError, HouseSql, ensure_schema, sql};

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<String>>,
    fail_on: Option<&'static str>,
}

impl HouseSql for Recorder {
    async fn query(&self, sql: &str) -> Result<String, HouseError> {
        self.seen.lock().expect("lock").push(sql.to_string());
        match self.fail_on {
            Some(needle) if sql.contains(needle) => Err(HouseError::Query {
                code: 62,
                message: "Syntax error".to_string(),
            }),
            _ => Ok(String::new()),
        }
    }
}

#[tokio::test]
async fn schema_applied_in_order_and_again() {
    let house = Recorder::default();
    ensure_schema(&house).await.expect("first");
    ensure_schema(&house).await.expect("again");
    let seen = house.seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 2 * sql::schema().len());
    assert_eq!(seen[..seen.len() / 2], sql::schema()[..]);
    assert_eq!(seen[0], "CREATE DATABASE IF NOT EXISTS _meter");
}

#[tokio::test]
async fn schema_stops_at_the_first_error() {
    let house = Recorder {
        fail_on: Some("events_q"),
        ..Recorder::default()
    };
    let err = ensure_schema(&house).await.expect_err("refused");
    assert!(matches!(err, HouseError::Query { code: 62, .. }), "{err}");
    let seen = house.seen.lock().expect("lock").clone();
    assert!(seen.last().expect("one").contains("events_q"));
    assert!(!seen.iter().any(|s| s.contains("events_mv")));
}
