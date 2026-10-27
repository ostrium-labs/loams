//! The identity sync against a fake of Zitadel's documented admin and management APIs
//! (`ListEvents`, `ListUserGrants`), with in-memory tuples, links and state.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use loams_authz::Tuple;
use loams_identity_sync::memory::{MemoryPrincipals, MemoryState, MemoryTuples};
use loams_identity_sync::reconcile::reconcile;
use loams_identity_sync::{Config, IdentitySync, Kind, StateStore, Tuples};
use loams_zitadel::Zitadel;
use serde_json::{Value, json};
use tokio::sync::Notify;

const PROJECT: &str = "p-loams";
const ISSUER: &str = "https://auth.example";
const ORG: &str = "o1";

#[derive(Default)]
struct Fake {
    /// `(type, aggregate type, aggregate id, owner, sequence, creation date, payload)`.
    events: Vec<Value>,
    /// The listing `ListUserGrants` answers, per org.
    grants: Vec<Value>,
    /// The `from` of every `ListEvents` call.
    froms: Vec<Option<String>>,
}

type Shared = Arc<Mutex<Fake>>;

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer pat-1")
}

async fn list_events(State(f): State<Shared>, h: HeaderMap, body: String) -> (StatusCode, String) {
    if !authorized(&h) {
        return (StatusCode::UNAUTHORIZED, "{}".into());
    }
    let req: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(req["asc"], true);
    let types: Vec<String> = serde_json::from_value(req["eventTypes"].clone()).unwrap();
    let limit = req["limit"].as_u64().unwrap() as usize;
    let from = req.get("from").and_then(Value::as_str).map(str::to_owned);
    let mut f = f.lock().unwrap();
    f.froms.push(from.clone());
    let from = from.map(|s| {
        time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339).unwrap()
    });
    let events: Vec<Value> = f
        .events
        .iter()
        .filter(|e| {
            types
                .iter()
                .any(|t| t == e["type"]["type"].as_str().unwrap())
        })
        .filter(|e| {
            let at = time::OffsetDateTime::parse(
                e["creationDate"].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap();
            // Strict, as Zitadel's is.
            from.is_none_or(|from| at > from)
        })
        .take(limit)
        .cloned()
        .collect();
    (StatusCode::OK, json!({"events": events}).to_string())
}

async fn list_grants(State(f): State<Shared>, h: HeaderMap, body: String) -> (StatusCode, String) {
    if !authorized(&h) {
        return (StatusCode::UNAUTHORIZED, "{}".into());
    }
    let org = h
        .get("x-zitadel-orgid")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let req: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(req["queries"][0]["projectIdQuery"]["projectId"], PROJECT);
    let f = f.lock().unwrap();
    let result: Vec<&Value> = f
        .grants
        .iter()
        .filter(|g| g["orgId"] == org.as_str())
        .collect();
    (StatusCode::OK, json!({"result": result}).to_string())
}

struct World {
    fake: Shared,
    sync: Arc<IdentitySync>,
    tuples: Arc<MemoryTuples>,
    links: Arc<MemoryPrincipals>,
    state: Arc<MemoryState>,
    clock: u32,
}

impl World {
    async fn new() -> Self {
        Self::with_batch(200).await
    }

    async fn with_batch(batch: u32) -> Self {
        let fake: Shared = Arc::default();
        let app = Router::new()
            .route("/admin/v1/events/_search", post(list_events))
            .route("/management/v1/users/grants/_search", post(list_grants))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let zitadel =
            Zitadel::new(&format!("http://{addr}"), "pat-1", reqwest::Client::new()).unwrap();
        let tuples = Arc::new(MemoryTuples::default());
        let links = Arc::new(MemoryPrincipals::default());
        let state = Arc::new(MemoryState::default());
        let mut config = Config::new(PROJECT, ISSUER);
        config.batch = batch;
        config.poll = Duration::from_millis(50);
        let sync = Arc::new(IdentitySync::new(
            zitadel,
            tuples.clone(),
            links.clone(),
            state.clone(),
            config,
        ));
        Self {
            fake,
            sync,
            tuples,
            links,
            state,
            clock: 0,
        }
    }

    /// Appends an event one second after the last (or at the same second with `same`).
    fn event(&mut self, kind: &str, agg: (&str, &str), seq: u64, payload: Value, same: bool) {
        if !same {
            self.clock += 1;
        }
        let at = format!(
            "2026-10-26T09:{:02}:{:02}.123456Z",
            self.clock / 60,
            self.clock % 60
        );
        self.fake.lock().unwrap().events.push(json!({
            "aggregate": {"id": agg.1, "type": {"type": agg.0}, "resourceOwner": ORG},
            "sequence": seq.to_string(),
            "creationDate": at,
            "payload": payload,
            "type": {"type": kind},
        }));
    }

    fn grant(&mut self, id: &str, seq: u64, kind: &str, payload: Value) {
        self.event(kind, ("usergrant", id), seq, payload, false);
    }

    fn tuples(&self) -> Vec<String> {
        self.tuples.all()
    }
}

fn org_tuple(user: &str, role: &str) -> String {
    format!("user:{user} {role} org:{ORG}")
}

#[tokio::test]
async fn grant_roles_map_to_org_relations() {
    let mut w = World::new().await;
    w.grant(
        "g1",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u1", "roleKeys": ["member"]}),
    );
    w.sync.step().await.unwrap();
    assert_eq!(w.tuples(), vec![org_tuple("u1", "member")]);
    w.grant(
        "g1",
        2,
        "user.grant.changed",
        json!({"userId": "u1", "roleKeys": ["admin", "member", "billing"]}),
    );
    w.sync.step().await.unwrap();
    assert_eq!(
        w.tuples(),
        vec![org_tuple("u1", "admin"), org_tuple("u1", "member")]
    );
    w.grant(
        "g1",
        3,
        "user.grant.changed",
        json!({"userId": "u1", "roleKeys": ["owner"]}),
    );
    w.sync.step().await.unwrap();
    assert_eq!(w.tuples(), vec![org_tuple("u1", "owner")]);
    w.grant("g1", 4, "user.grant.deactivated", Value::Null);
    w.sync.step().await.unwrap();
    assert!(w.tuples().is_empty(), "an inactive grant confers nothing");
    w.grant("g1", 5, "user.grant.reactivated", Value::Null);
    w.sync.step().await.unwrap();
    assert_eq!(w.tuples(), vec![org_tuple("u1", "owner")]);
    w.grant(
        "g1",
        6,
        "user.grant.removed",
        json!({"projectId": PROJECT, "userId": "u1"}),
    );
    w.sync.step().await.unwrap();
    assert!(w.tuples().is_empty());
}

#[tokio::test]
async fn a_grant_on_another_project_is_not_a_membership() {
    let mut w = World::new().await;
    w.grant(
        "g9",
        1,
        "user.grant.added",
        json!({"projectId": "p-other", "userId": "u1", "roleKeys": ["owner"]}),
    );
    w.grant(
        "g9",
        2,
        "user.grant.changed",
        json!({"userId": "u1", "roleKeys": ["owner"]}),
    );
    w.sync.step().await.unwrap();
    assert!(w.tuples().is_empty());
}

#[tokio::test]
async fn org_manager_is_not_a_member() {
    let mut w = World::new().await;
    w.event(
        "org.member.added",
        ("org", ORG),
        5,
        json!({"roles": ["ORG_OWNER"], "userId": "u7"}),
        false,
    );
    let s = w.sync.step().await.unwrap();
    assert_eq!(s.events, 0, "the sync does not even read org.member.*");
    assert!(w.tuples().is_empty());
}

#[tokio::test]
async fn resumes_from_stored_position() {
    let mut w = World::new().await;
    w.grant(
        "g1",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u1", "roleKeys": ["member"]}),
    );
    assert_eq!(w.sync.step().await.unwrap().events, 1);
    assert_eq!(w.sync.step().await.unwrap().events, 0, "nothing new");
    // A tuple removed by hand is not re-applied from an old event: the position moved on.
    w.tuples
        .write(
            &[],
            &[Tuple::new("user:u1", "member", format!("org:{ORG}"))],
        )
        .await
        .unwrap();
    w.grant(
        "g2",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u2", "roleKeys": ["admin"]}),
    );
    assert_eq!(w.sync.step().await.unwrap().events, 1);
    assert_eq!(w.tuples(), vec![org_tuple("u2", "admin")]);
    let froms = w.fake.lock().unwrap().froms.clone();
    assert_eq!(froms[0], None);
    assert_eq!(
        froms.last().unwrap().as_deref(),
        Some("2026-10-26T09:00:01.123455Z"),
        "asked from a microsecond before the stored date"
    );
    let state = w.state.load().await.unwrap().unwrap();
    assert_eq!(state.position.seen.len(), 1);
}

#[tokio::test]
async fn reread_dedupes_by_aggregate_sequence() {
    // Three events at one date, read two at a time: the second read starts at that date
    // again and skips the two it applied.
    let mut w = World::with_batch(2).await;
    w.grant(
        "g1",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u1", "roleKeys": ["member"]}),
    );
    w.event(
        "user.grant.added",
        ("usergrant", "g2"),
        1,
        json!({"projectId": PROJECT, "userId": "u2", "roleKeys": ["member"]}),
        true,
    );
    w.event(
        "user.grant.added",
        ("usergrant", "g3"),
        1,
        json!({"projectId": PROJECT, "userId": "u3", "roleKeys": ["member"]}),
        true,
    );
    w.grant(
        "g1",
        2,
        "user.grant.removed",
        json!({"projectId": PROJECT, "userId": "u1"}),
    );
    let s = w.sync.step().await.unwrap();
    assert_eq!(s.events, 4);
    assert_eq!(
        w.tuples(),
        vec![org_tuple("u2", "member"), org_tuple("u3", "member")]
    );
}

#[tokio::test]
async fn deactivation_disables_the_link_then_deletes_every_tuple() {
    let mut w = World::new().await;
    w.grant(
        "g1",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u1", "roleKeys": ["admin"]}),
    );
    w.sync.step().await.unwrap();
    // Tuples the control plane wrote for u1, and someone else's.
    w.tuples
        .write(
            &[
                Tuple::new("user:u1", "editor", "project:p1"),
                Tuple::new("user:u1", "reader", "graph:ns/g"),
                Tuple::new("user:u2", "editor", "project:p1"),
            ],
            &[],
        )
        .await
        .unwrap();
    w.event("user.deactivated", ("user", "u1"), 3, Value::Null, false);
    w.sync.step().await.unwrap();
    assert_eq!(
        w.links.get(ISSUER, "u1"),
        Some((Kind::User, true)),
        "linked and disabled"
    );
    assert_eq!(w.tuples(), vec!["user:u2 editor project:p1".to_owned()]);
    // A grant change while deactivated writes nothing.
    w.grant(
        "g1",
        2,
        "user.grant.changed",
        json!({"userId": "u1", "roleKeys": ["owner"]}),
    );
    w.event("user.locked", ("user", "u1"), 4, Value::Null, false);
    w.event("user.reactivated", ("user", "u1"), 5, Value::Null, false);
    w.sync.step().await.unwrap();
    assert_eq!(
        w.links.get(ISSUER, "u1"),
        Some((Kind::User, true)),
        "still locked"
    );
    assert_eq!(w.tuples().len(), 1);
    w.event("user.unlocked", ("user", "u1"), 6, Value::Null, false);
    w.sync.step().await.unwrap();
    assert_eq!(w.links.get(ISSUER, "u1"), Some((Kind::User, false)));
    assert!(
        w.tuples().contains(&org_tuple("u1", "owner")),
        "memberships come back from the grants"
    );
    assert!(
        !w.tuples().contains(&"user:u1 editor project:p1".to_owned()),
        "other grants do not"
    );
}

#[tokio::test]
async fn a_deactivated_agent_loses_its_tuples() {
    let mut w = World::new().await;
    w.links.put(ISSUER, "a1", Kind::Agent);
    w.tuples
        .write(&[Tuple::new("agent:a1", "editor", "environment:dev")], &[])
        .await
        .unwrap();
    w.event("user.deactivated", ("user", "a1"), 2, Value::Null, false);
    w.sync.step().await.unwrap();
    assert_eq!(
        w.links.get(ISSUER, "a1"),
        Some((Kind::Agent, true)),
        "an agent stays an agent"
    );
    assert!(w.tuples().is_empty());
}

#[tokio::test]
async fn machine_user_links_as_service_account_unless_an_agent() {
    let mut w = World::new().await;
    w.links.put(ISSUER, "a1", Kind::Agent);
    w.event(
        "user.machine.added",
        ("user", "a1"),
        1,
        json!({"userName": "agent-1"}),
        false,
    );
    w.event(
        "user.machine.added",
        ("user", "m1"),
        1,
        json!({"userName": "ci"}),
        false,
    );
    w.sync.step().await.unwrap();
    assert_eq!(w.links.get(ISSUER, "a1"), Some((Kind::Agent, false)));
    assert_eq!(
        w.links.get(ISSUER, "m1"),
        Some((Kind::ServiceAccount, false))
    );
}

#[tokio::test]
async fn lost_webhook_still_applied() {
    // No wake-up call ever comes: the poll reads the event list anyway.
    let mut w = World::new().await;
    let shutdown = Arc::new(Notify::new());
    let task = tokio::spawn(w.sync.clone().run(shutdown.clone()));
    w.grant(
        "g1",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u1", "roleKeys": ["member"]}),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while w.tuples().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "not applied within 5 s"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shutdown.notify_one();
    task.await.unwrap();
}

#[tokio::test]
async fn reconciliation_repairs_only_what_both_sides_agree_on() {
    let mut w = World::new().await;
    w.grant(
        "g1",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u1", "roleKeys": ["admin"]}),
    );
    w.grant(
        "g2",
        1,
        "user.grant.added",
        json!({"projectId": PROJECT, "userId": "u2", "roleKeys": ["member"]}),
    );
    w.sync.step().await.unwrap();
    {
        let mut f = w.fake.lock().unwrap();
        f.grants = vec![
            json!({"id": "g1", "userId": "u1", "orgId": ORG, "projectId": PROJECT, "roleKeys": ["admin"], "state": "USER_GRANT_STATE_ACTIVE"}),
            // The listing lags: u2's grant is not in it yet.
            // u3's grant is listed, but the sync has no event for it.
            json!({"id": "g3", "userId": "u3", "orgId": ORG, "projectId": PROJECT, "roleKeys": ["owner"], "state": "USER_GRANT_STATE_ACTIVE"}),
        ];
    }
    // Drift in OpenFGA: u1's admin tuple lost, a stray owner for u9.
    let org = format!("org:{ORG}");
    w.tuples
        .write(
            &[Tuple::new("user:u9", "owner", &org)],
            &[Tuple::new("user:u1", "admin", &org)],
        )
        .await
        .unwrap();
    let report = reconcile(&w.sync).await.unwrap();
    assert_eq!(report.orgs, 1);
    assert_eq!(report.written, 1, "u1's admin: listed and applied");
    assert_eq!(report.deleted, 1, "u9's owner: neither listed nor applied");
    assert_eq!(
        report.drift, 2,
        "u2 (not listed yet) and u3 (no event) are left alone"
    );
    assert_eq!(
        w.tuples(),
        vec![org_tuple("u1", "admin"), org_tuple("u2", "member")]
    );
}
