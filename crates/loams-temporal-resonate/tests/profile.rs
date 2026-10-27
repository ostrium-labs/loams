//! D1591's profile tests: each assumption §74 §5.3 makes about Resonate, run
//! against a real server through the interim adapter. Set
//! `LOAMS_TEMPORAL_TEST_RESONATE_URL` to its protocol endpoint (e.g.
//! `http://127.0.0.1:8001`); without it each test prints why and passes.
//! The same scenarios run on the fake in `tests/fake.rs`. Set
//! `LOAMS_TEMPORAL_TEST_RESONATE_STORE` to the server's store: on the SQL
//! stores `profile_delayed_create` is an expected failure. Postgres refuses
//! IDs with more than one `:` at 31ffb201 (500), so run these on SQLite, the
//! in-memory blob store or TiKV until the fork's fix lands.
#![allow(clippy::unwrap_used)]

mod common;

use common::{Clock, Names};
use loams_temporal_resonate::HttpDurability;

const ENV: &str = "LOAMS_TEMPORAL_TEST_RESONATE_URL";
const STORE_ENV: &str = "LOAMS_TEMPORAL_TEST_RESONATE_STORE";

fn server(name: &str) -> Option<(HttpDurability, Clock, Names)> {
    let Ok(url) = std::env::var(ENV) else {
        eprintln!("skipped {name}: {ENV} is not set (no Resonate server to run against)");
        return None;
    };
    let n = Names::new();
    let d = HttpDurability::new(url, format!("t6-{}", n.u), None).unwrap();
    Some((d, Clock::Wall, n))
}

#[tokio::test]
async fn profile_timer_times_out_and_notifies() {
    let Some((d, c, n)) = server("profile_timer_times_out_and_notifies") else {
        return;
    };
    common::timer_times_out_and_notifies(&d, &c, &n).await;
}

#[tokio::test]
async fn profile_task_lease_expiry_redelivers() {
    let Some((d, c, n)) = server("profile_task_lease_expiry_redelivers") else {
        return;
    };
    let pid = d.pid().to_string();
    common::task_lease_expiry_redelivers(&d, &c, &n, &pid).await;
}

#[tokio::test]
async fn profile_idempotent_create_same_id() {
    let Some((d, c, n)) = server("profile_idempotent_create_same_id") else {
        return;
    };
    common::idempotent_create_same_id(&d, &c, &n).await;
}

#[tokio::test]
async fn profile_idempotent_create_conflicting_param() {
    let Some((d, c, n)) = server("profile_idempotent_create_conflicting_param") else {
        return;
    };
    common::idempotent_create_conflicting_param(&d, &c, &n).await;
}

/// Which store the server runs on (`LOAMS_TEMPORAL_TEST_RESONATE_STORE`:
/// `sqlite`, `postgres`, `mysql`, `blob`, `tikv`; unset means strict).
fn store() -> String {
    std::env::var(STORE_ENV).unwrap_or_default()
}

#[tokio::test]
async fn profile_delayed_create() {
    let Some((d, c, n)) = server("profile_delayed_create") else {
        return;
    };
    if matches!(store().as_str(), "sqlite" | "postgres" | "mysql") {
        // Expected failure: the SQL stores ignore `resonate:delay` at
        // 31ffb201 and dispatch at create (FW7 profile; fork fix coming).
        // The relay defers delayed creates for them (`DelaySupport::Defer`).
        // When this starts passing, remove the expected failure.
        let run = tokio::spawn(async move { common::delayed_create(&d, &c, &n).await }).await;
        assert!(
            run.is_err(),
            "{} now honours resonate:delay: remove this expected failure",
            store()
        );
        eprintln!(
            "expected failure on {}: the SQL stores ignore resonate:delay (31ffb201)",
            store()
        );
        return;
    }
    common::delayed_create(&d, &c, &n).await;
}

#[tokio::test]
async fn profile_listener_delivery() {
    let Some((d, c, n)) = server("profile_listener_delivery") else {
        return;
    };
    common::listener_delivery(&d, &c, &n).await;
}

#[tokio::test]
async fn profile_settle_canceled() {
    let Some((d, c, n)) = server("profile_settle_canceled") else {
        return;
    };
    common::settle_canceled(&d, &c, &n).await;
}
