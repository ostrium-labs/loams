//! `NeonClients`, the `NeonRead` adapter over `loams-postgres`'s clients,
//! against stand-in components answering `loams-postgres`'s recorded
//! fixtures.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use loams_pg_control::neon::{
    Component, Lsn, LsnAtTime, NeonClient, NeonClients, NeonRead, TenantId, TimelineId, WalClient,
};
use loams_pg_control::service::Reason;
use loams_postgres::pageserver::NeonEndpoints;

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../loams-postgres/tests/fixtures")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<String>>>);

/// A timeline id the stand-in pageserver does not know.
const MISSING: &str = "6d697373696e676d697373696e676d69";

/// Answers by path: the LSN of a time (with a lease), a timeline, and a 404
/// for [`MISSING`] or anything else.
async fn answer(State(seen): State<Seen>, req: Request) -> impl IntoResponse {
    let pq = req
        .uri()
        .path_and_query()
        .map(ToString::to_string)
        .unwrap_or_default();
    seen.0.lock().expect("lock").push(pq.clone());
    let path = req.uri().path();
    let json = [("content-type", "application/json")];
    let (status, body) = if path.ends_with("/get_lsn_by_timestamp") {
        (
            StatusCode::OK,
            r#"{"lsn":"0/169AD58","kind":"present","valid_until":"2026-10-09T03:41:46.000Z"}"#
                .to_string(),
        )
    } else if path.contains("/timeline/") && !path.contains(MISSING) {
        (StatusCode::OK, fixture("timeline_get.response.json"))
    } else {
        (StatusCode::NOT_FOUND, fixture("not_found.response.json"))
    };
    (status, json, body)
}

/// `loams-wal`: every request answers the recorded status.
async fn wal_answer(State(seen): State<Seen>, req: Request) -> impl IntoResponse {
    let pq = req
        .uri()
        .path_and_query()
        .map(ToString::to_string)
        .unwrap_or_default();
    seen.0.lock().expect("lock").push(pq);
    let json = [("content-type", "application/json")];
    (
        StatusCode::OK,
        json,
        fixture("wal_timeline_status.response.json"),
    )
}

async fn serve(seen: Seen, wal: bool) -> url::Url {
    let app = if wal {
        axum::Router::new().fallback(wal_answer).with_state(seen)
    } else {
        axum::Router::new().fallback(answer).with_state(seen)
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}").parse().expect("a url")
}

const T: &str = "4c6f616d734e656f6e54656e616e7431";
const TL: &str = "4c6f616d734e656f6e54696d656c6e31";

#[tokio::test]
async fn neon_clients_read_through_loams_postgres() {
    let (pages, wal) = (Seen::default(), Seen::default());
    let clients = NeonClients {
        storage: NeonClient::new(
            NeonEndpoints {
                pageserver: serve(pages.clone(), false).await,
                storcon: None,
            },
            None,
        )
        .expect("a client"),
        wal: WalClient::new(serve(wal.clone(), true).await, None).expect("a client"),
    };
    let (t, tl): (TenantId, TimelineId) = (T.parse().expect("t"), TL.parse().expect("tl"));

    let view = clients.timeline(t, tl).await.expect("a timeline");
    assert_eq!(
        view.last_record_lsn,
        "0/14E8F98".parse::<Lsn>().expect("lsn")
    );
    assert_eq!(
        view.min_readable_lsn,
        "0/14E8F20".parse::<Lsn>().expect("lsn")
    );

    // 2026-10-09T02:41:46Z, asked with a lease.
    let at = clients
        .lsn_by_timestamp(t, tl, 1_791_513_706_000)
        .await
        .expect("an LSN");
    assert_eq!(at, LsnAtTime::Present(Lsn(0x0169_AD58)));
    let asked = pages
        .0
        .lock()
        .expect("lock")
        .last()
        .cloned()
        .expect("a call");
    assert_eq!(
        asked,
        format!(
            "/v1/tenant/{T}/timeline/{TL}/get_lsn_by_timestamp?timestamp=2026-10-09T02%3A41%3A46Z&with_lease=true"
        )
    );

    let heads = clients.wal_heads(t, tl).await.expect("heads");
    assert_eq!(heads.commit_lsn, "0/14E8F98".parse::<Lsn>().expect("lsn"));
    assert_eq!(
        wal.0.lock().expect("lock").last().map(String::as_str),
        Some(format!("/v1/tenant/{T}/timeline/{TL}").as_str())
    );

    // A refusal arrives as a typed reason and component.
    let missing: TimelineId = MISSING.parse().expect("tl");
    let e = clients.timeline(t, missing).await.expect_err("a 404");
    assert_eq!(e.reason, Reason::NotFound);
    assert_eq!(e.component, Some(Component::Pageserver));
}
