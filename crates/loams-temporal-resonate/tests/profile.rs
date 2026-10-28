//! D1591's profile tests: each assumption §74 §5.3 makes about Resonate, run
//! against a real server through [`HttpDurability`] (on `resonate-client`
//! since D2040). Set `LOAMS_TEMPORAL_TEST_RESONATE_URL` to its protocol
//! endpoint (e.g. `http://127.0.0.1:8001`); without it each test prints why
//! and passes. The same scenarios run on the fake in `tests/fake.rs`.
//!
//! The server should be the fork's `loams/main` at f29c04e1 or later, where
//! every store honours `resonate:delay` (D2042). For an older server set
//! `LOAMS_TEMPORAL_TEST_RESONATE_DELAY=ignored`: `profile_delayed_create` is
//! then an expected failure.
#![allow(clippy::unwrap_used)]

mod common;

use common::{Clock, Names};
use loams_temporal_resonate::HttpDurability;

const ENV: &str = "LOAMS_TEMPORAL_TEST_RESONATE_URL";
const DELAY_ENV: &str = "LOAMS_TEMPORAL_TEST_RESONATE_DELAY";

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

#[tokio::test]
async fn profile_delayed_create() {
    let Some((d, c, n)) = server("profile_delayed_create") else {
        return;
    };
    if std::env::var(DELAY_ENV).as_deref() == Ok("ignored") {
        // Expected failure on a server older than the fork's f29c04e1, whose
        // SQL stores ignore `resonate:delay` and dispatch at create (X1.R11
        // (5)). The relay defers delayed creates for it (`DelaySupport::Defer`).
        let run = tokio::spawn(async move { common::delayed_create(&d, &c, &n).await }).await;
        assert!(
            run.is_err(),
            "the server honours resonate:delay: unset {DELAY_ENV}"
        );
        eprintln!("expected failure: the server ignores resonate:delay ({DELAY_ENV}=ignored)");
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
