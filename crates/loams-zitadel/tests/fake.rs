//! `loams-zitadel` against a fake of Zitadel's documented v2 user API (request shapes,
//! idempotency, the order of revocation, and the undo of a half-made agent).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use loams_authz::Tuple;
use loams_zitadel::agents::{
    AgentLink, Agents, LinkStore, MemoryLinks, NewAgent, SessionGrant, TupleStore,
};
use loams_zitadel::{UserState, Zitadel};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    /// Every call: method, path, body.
    calls: Vec<(String, String, Value)>,
    users: HashMap<String, &'static str>,
    next: u32,
    fail_add_key: bool,
}

type Shared = Arc<Mutex<Fake>>;

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer pat-1")
}

async fn create(State(f): State<Shared>, h: HeaderMap, body: String) -> (StatusCode, String) {
    if !authorized(&h) {
        return (StatusCode::UNAUTHORIZED, "{}".into());
    }
    let mut f = f.lock().unwrap();
    f.next += 1;
    let id = format!("3000{}", f.next);
    f.users.insert(id.clone(), "USER_STATE_ACTIVE");
    f.calls.push((
        "POST".into(),
        "/v2/users/new".into(),
        serde_json::from_str(&body).unwrap(),
    ));
    (
        StatusCode::OK,
        json!({"id": id, "creationDate": "2026-10-23T00:00:00Z"}).to_string(),
    )
}

async fn add_key(
    State(f): State<Shared>,
    Path(id): Path<String>,
    body: String,
) -> (StatusCode, String) {
    let mut f = f.lock().unwrap();
    f.calls.push((
        "POST".into(),
        format!("/v2/users/{id}/keys"),
        serde_json::from_str(&body).unwrap(),
    ));
    if f.fail_add_key {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"code": 13, "message": "boom"}).to_string(),
        );
    }
    let file =
        json!({"type": "serviceaccount", "keyId": "k1", "key": "PEM", "userId": id}).to_string();
    use base64::Engine as _;
    let content = base64::engine::general_purpose::STANDARD.encode(file);
    (
        StatusCode::OK,
        json!({"keyId": "k1", "keyContent": content}).to_string(),
    )
}

async fn state_change(
    State(f): State<Shared>,
    Path((id, verb)): Path<(String, String)>,
) -> (StatusCode, String) {
    let mut f = f.lock().unwrap();
    f.calls
        .push(("POST".into(), format!("/v2/users/{id}/{verb}"), Value::Null));
    let (from, to) = match verb.as_str() {
        "deactivate" => ("USER_STATE_ACTIVE", "USER_STATE_INACTIVE"),
        "reactivate" => ("USER_STATE_INACTIVE", "USER_STATE_ACTIVE"),
        _ => return (StatusCode::NOT_FOUND, "{}".into()),
    };
    match f.users.get_mut(&id) {
        None => (
            StatusCode::NOT_FOUND,
            json!({"message": "not found"}).to_string(),
        ),
        // Zitadel refuses a second deactivation (documented); the fake does too.
        Some(s) if *s != from => (
            StatusCode::BAD_REQUEST,
            json!({"code": 9, "message": "user is not in the expected state"}).to_string(),
        ),
        Some(s) => {
            *s = to;
            (StatusCode::OK, json!({"details": {}}).to_string())
        }
    }
}

async fn get_user(State(f): State<Shared>, Path(id): Path<String>) -> (StatusCode, String) {
    let f = f.lock().unwrap();
    match f.users.get(&id) {
        Some(s) => (
            StatusCode::OK,
            json!({"user": {"userId": id, "state": s}}).to_string(),
        ),
        None => (
            StatusCode::NOT_FOUND,
            json!({"message": "not found"}).to_string(),
        ),
    }
}

async fn delete_user(State(f): State<Shared>, Path(id): Path<String>) -> StatusCode {
    let mut f = f.lock().unwrap();
    f.calls
        .push(("DELETE".into(), format!("/v2/users/{id}"), Value::Null));
    if f.users.remove(&id).is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn fake() -> (Zitadel, Shared) {
    let state = Shared::default();
    let app = Router::new()
        .route("/v2/users/new", post(create))
        .route("/v2/users/{id}/keys", post(add_key))
        .route("/v2/users/{id}/{verb}", post(state_change))
        .route("/v2/users/{id}", get(get_user).delete(delete_user))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let z = Zitadel::new(&format!("http://{addr}"), "pat-1", reqwest::Client::new()).unwrap();
    (z, state)
}

/// Records writes; can be told to fail.
#[derive(Default)]
struct Tuples {
    log: Mutex<Vec<(Vec<Tuple>, Vec<Tuple>)>>,
    fail: Mutex<bool>,
}

#[async_trait]
impl TupleStore for Tuples {
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()> {
        if *self.fail.lock().unwrap() {
            return Err(loams_authz::AuthzError::Unavailable("down".into()));
        }
        self.log
            .lock()
            .unwrap()
            .push((writes.to_vec(), deletes.to_vec()));
        Ok(())
    }
}

const ISS: &str = "http://localhost:18080";

fn agents(z: Zitadel) -> Agents<Arc<Tuples>, Arc<MemoryLinks>> {
    Agents::new(
        z,
        ISS,
        "org-1",
        Arc::new(Tuples::default()),
        Arc::new(MemoryLinks::default()),
    )
}

fn new_agent() -> NewAgent<'static> {
    NewAgent {
        username: "build-bot",
        name: "Build bot",
        description: Some("runs CI"),
        owner: "pia",
        project: "p1",
    }
}

#[tokio::test]
async fn create_makes_a_jwt_machine_user_with_a_key_a_link_and_no_grant() {
    let (z, fake) = fake().await;
    let a = agents(z);
    let agent = a.create(&new_agent()).await.unwrap();
    let calls = fake.lock().unwrap().calls.clone();
    assert_eq!(calls[0].1, "/v2/users/new");
    assert_eq!(
        calls[0].2,
        json!({"organizationId": "org-1", "username": "build-bot",
               "machine": {"name": "Build bot", "description": "runs CI",
                           "accessTokenType": "ACCESS_TOKEN_TYPE_JWT"}})
    );
    assert_eq!(calls[1].1, format!("/v2/users/{}/keys", agent.id));
    assert!(
        calls[1].2["expirationDate"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    assert!(
        calls[1].2.get("publicKey").is_none(),
        "Zitadel generates the pair"
    );
    let file = agent.key.key_file.as_deref().unwrap();
    assert!(String::from_utf8_lossy(file).contains("serviceaccount"));
    assert_eq!(
        a.links().get(ISS, &agent.id),
        Some(AgentLink {
            issuer: ISS.into(),
            subject: agent.id.clone(),
            disabled: false
        })
    );
    let log = a.tuples().log.lock().unwrap().clone();
    assert_eq!(log.len(), 1);
    let written: Vec<String> = log[0]
        .0
        .iter()
        .map(|t| format!("{}#{}@{}", t.object, t.relation, t.user))
        .collect();
    assert_eq!(
        written,
        [
            format!("agent:{}#owner@user:pia", agent.id),
            format!("agent:{}#project@project:p1", agent.id)
        ],
        "only its roots: an owner confers nothing, and no grant is made"
    );
}

#[tokio::test]
async fn a_failed_step_removes_the_half_made_agent() {
    let (z, fake) = fake().await;
    fake.lock().unwrap().fail_add_key = true;
    let a = agents(z.clone());
    assert!(a.create(&new_agent()).await.is_err());
    {
        let f = fake.lock().unwrap();
        assert!(f.calls.iter().any(|c| c.0 == "DELETE"), "{:?}", f.calls);
        assert!(f.users.is_empty());
    }
    // A tuple write that fails after the link: the user is removed and the link disabled.
    fake.lock().unwrap().fail_add_key = false;
    *a.tuples().fail.lock().unwrap() = true;
    assert!(a.create(&new_agent()).await.is_err());
    assert!(fake.lock().unwrap().users.is_empty());
    let id = "30002";
    assert!(a.links().get(ISS, id).unwrap().disabled);
}

#[tokio::test]
async fn grants_are_one_environment_and_sessions_carry_their_expiry() {
    let (z, _) = fake().await;
    let a = agents(z);
    let t = a.grant("a1", "editor", "ns-dev", None).await.unwrap();
    assert_eq!(
        (t.user.as_str(), t.relation.as_str(), t.object.as_str()),
        ("agent:a1", "editor", "environment:ns-dev")
    );
    assert!(t.condition.is_none());
    let s = SessionGrant {
        session: "s-1".into(),
        expires: time::macros::datetime!(2026-10-23 12:15 UTC),
    };
    let t = a.grant("a1", "deployer", "ns-dev", Some(&s)).await.unwrap();
    let (name, ctx) = t.condition.unwrap();
    assert_eq!(name, "agent_session");
    assert_eq!(ctx["allowed_session"], "s-1");
    assert_eq!(ctx["expires"], "2026-10-23T12:15:00Z");
    assert!(
        !ctx.contains_key("session_id") && !ctx.contains_key("now"),
        "the request supplies those"
    );
    for bad in ["admin", "owner", "billing_admin", "can_write"] {
        assert!(a.grant("a1", bad, "ns-dev", None).await.is_err(), "{bad}");
    }
}

#[tokio::test]
async fn revoke_disables_the_link_first_and_is_idempotent() {
    let (z, fake) = fake().await;
    let a = agents(z.clone());
    let agent = a.create(&new_agent()).await.unwrap();
    let grant = a.grant(&agent.id, "editor", "ns-dev", None).await.unwrap();
    a.revoke(&agent.id, std::slice::from_ref(&grant))
        .await
        .unwrap();
    assert!(a.links().get(ISS, &agent.id).unwrap().disabled);
    assert_eq!(z.user_state(&agent.id).await.unwrap(), UserState::Inactive);
    let log = a.tuples().log.lock().unwrap().clone();
    assert_eq!(log.last().unwrap().1, vec![grant.clone()]);
    // Again: Zitadel refuses a second deactivation, which is still success.
    a.revoke(&agent.id, std::slice::from_ref(&grant))
        .await
        .unwrap();
    // Even with the tuple store down, the link is disabled and the user deactivated.
    a.reinstate(&agent.id).await.unwrap();
    assert!(!a.links().get(ISS, &agent.id).unwrap().disabled);
    *a.tuples().fail.lock().unwrap() = true;
    assert!(a.revoke(&agent.id, &[grant]).await.is_err());
    assert!(a.links().get(ISS, &agent.id).unwrap().disabled);
    assert_eq!(z.user_state(&agent.id).await.unwrap(), UserState::Inactive);
    assert!(
        fake.lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.1.ends_with("/deactivate"))
            .count()
            >= 3
    );
}

#[tokio::test]
async fn unknown_users_and_bad_ids() {
    let (z, _) = fake().await;
    assert!(z.user_state("999").await.unwrap_err().is_not_found());
    assert!(z.deactivate("999").await.unwrap_err().is_not_found());
    z.delete_user("999")
        .await
        .expect("a missing user is already deleted");
    assert!(z.deactivate("../admin").await.is_err());
    let wrong = Zitadel::new(z.base().as_str(), "nope", reqwest::Client::new()).unwrap();
    let err = wrong
        .create_machine_user("o", "u", "n", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, loams_zitadel::Error::Status { status: 401, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn memory_links_refuse_an_unknown_link() {
    let links = MemoryLinks::default();
    assert!(links.set_disabled(ISS, "x", true).await.is_err());
}
