//! `loams.admin.v1` on the main port (API2 Task 7, server half), against the
//! real server: a scripted console session answered alike by the server and
//! the console mock, the once-only key secret, org roles, setup, and the
//! audit watch's cursors.

mod console_support;

use std::sync::Arc;
use std::time::Duration;

use console_support::{Client, Engine, SETUP_TOKEN, TestAuthenticator, reason_of};
use futures::StreamExt as _;
use serde_json::{Value, json};

const ORG: &str = "admin.v1.OrgService";
const PROJECTS: &str = "admin.v1.ProjectService";
const AGENTS: &str = "admin.v1.AgentService";
const KEYS: &str = "admin.v1.KeyService";
const AUDIT: &str = "admin.v1.AuditService";
const AUTH: &str = "auth.v1.AuthService";

/// The mock's audit vocabulary (`crates/loams-apps-mock/src/admin/`): what
/// both record for the script's mutations.
const MOCK_ACTIONS: &[&str] = &[
    "org.update",
    "member.remove",
    "invitation.create",
    "team.create",
    "team.member.add",
    "project.create",
    "project.access.grant",
    "environment.create",
    "agent.create",
    "agent.update",
    "agent.suspend",
    "agent.resume",
    "trust_policy.create",
    "service_account.create",
    "api_key.create",
    "api_key.revoke",
    "token.revoke",
];

/// An answer with its volatile parts gone: the outcome, and which
/// top-level fields carry a message or a non-empty string (lists and
/// scalars hold seed data on the mock and real data on the server, ruling
/// 6/7.10).
fn shape(answer: &console_support::Answer) -> (String, Vec<String>) {
    if answer.status != 200 {
        let (code, reason) = answer.failure();
        return (format!("{code}/{reason}"), Vec::new());
    }
    let mut keys: Vec<String> = answer
        .body
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, v)| {
            k.as_str() != "nextPageToken"
                && (v.is_object() || v.as_str().is_some_and(|s| !s.is_empty()))
        })
        .map(|(k, _)| k.clone())
        .collect();
    keys.sort();
    ("ok".to_owned(), keys)
}

/// The cookie attributes a `Set-Cookie` carries, without its value and
/// `Max-Age` (the mock's cookie is a session cookie).
fn cookie_attributes(answer: &console_support::Answer) -> Vec<String> {
    let mut attributes: Vec<String> = answer
        .set_cookies()
        .iter()
        .flat_map(|c| c.split(';').skip(1).map(|a| a.trim().to_owned()))
        .filter(|a| !a.starts_with("Max-Age") || a == "Max-Age=0")
        .collect();
    attributes.sort();
    attributes
}

/// The first item's `field` under `list` in an answer.
fn first(answer: &console_support::Answer, list: &str, field: &str) -> String {
    answer.ok()[list][0][field]
        .as_str()
        .unwrap_or_else(|| panic!("no {list}[0].{field} in {}", answer.body))
        .to_owned()
}

/// One console session, step by step, against `client`. Returns every
/// answer by step name, and the audit log's `(id, action)`s (newest first).
async fn script(
    mut client: Client,
) -> (
    Vec<(&'static str, console_support::Answer)>,
    Vec<(String, String)>,
) {
    let mut out = Vec::new();
    let setup = client.setup(SETUP_TOKEN).await;
    out.push(("CompleteSetup", setup));
    let call = |client: &Client, service: &'static str, method: &'static str, body: Value| {
        let client = client.clone();
        async move { client.call(service, method, body).await }
    };
    out.push((
        "GetSession",
        call(&client, AUTH, "GetSession", json!({})).await,
    ));
    out.push((
        "ListProviders",
        call(&client, AUTH, "ListProviders", json!({})).await,
    ));
    out.push((
        "CreateProject",
        call(
            &client,
            PROJECTS,
            "CreateProject",
            json!({"name": "Docs search", "slug": "docs-search"}),
        )
        .await,
    ));
    out.push((
        "CreateEnvironment",
        call(
            &client,
            PROJECTS,
            "CreateEnvironment",
            json!({"project": "docs-search", "name": "Production", "slug": "production"}),
        )
        .await,
    ));
    out.push((
        "CreateTeam",
        call(&client, ORG, "CreateTeam", json!({"name": "Platform"})).await,
    ));
    let teams = call(&client, ORG, "ListTeams", json!({})).await;
    let team = first(&teams, "teams", "id");
    out.push(("ListTeams", teams));
    out.push((
        "GetTeam",
        call(&client, ORG, "GetTeam", json!({"team": team})).await,
    ));
    let members = call(&client, ORG, "ListMembers", json!({})).await;
    let user = members.ok()["members"][0]["user"]["id"]
        .as_str()
        .expect("test fixture")
        .to_owned();
    out.push(("ListMembers", members));
    out.push((
        "AddTeamMember",
        call(
            &client,
            ORG,
            "AddTeamMember",
            json!({"team": team, "user": user}),
        )
        .await,
    ));
    out.push((
        "GrantAccess",
        call(
            &client,
            PROJECTS,
            "GrantAccess",
            json!({"project": "docs-search", "subject": {"kind": "SUBJECT_KIND_USER", "id": user},
                   "role": "PROJECT_ROLE_DEVELOPER"}),
        )
        .await,
    ));
    out.push((
        "ListAccess",
        call(
            &client,
            PROJECTS,
            "ListAccess",
            json!({"project": "docs-search"}),
        )
        .await,
    ));
    out.push((
        "CreateInvitation",
        call(
            &client,
            ORG,
            "CreateInvitation",
            json!({"email": "new@example.test", "role": "ORG_ROLE_MEMBER"}),
        )
        .await,
    ));
    out.push((
        "ListInvitations",
        call(&client, ORG, "ListInvitations", json!({})).await,
    ));
    out.push(("GetOrg", call(&client, ORG, "GetOrg", json!({})).await));
    out.push((
        "UpdateOrg",
        call(
            &client,
            ORG,
            "UpdateOrg",
            json!({"org": {"name": "Acme Inc"}, "updateMask": "name"}),
        )
        .await,
    ));
    out.push((
        "ListProjects",
        call(&client, PROJECTS, "ListProjects", json!({})).await,
    ));
    out.push((
        "GetProject",
        call(
            &client,
            PROJECTS,
            "GetProject",
            json!({"project": "docs-search"}),
        )
        .await,
    ));
    out.push((
        "ListEnvironments",
        call(
            &client,
            PROJECTS,
            "ListEnvironments",
            json!({"project": "docs-search"}),
        )
        .await,
    ));
    let env = json!({"project": "docs-search", "environment": "production"});
    out.push((
        "GetEnvironment",
        call(&client, PROJECTS, "GetEnvironment", env.clone()).await,
    ));
    out.push((
        "GetEnvironmentActivity",
        call(&client, PROJECTS, "GetEnvironmentActivity", env.clone()).await,
    ));
    out.push((
        "CreateAgent",
        call(
            &client,
            AGENTS,
            "CreateAgent",
            json!({"project": "docs-search", "name": "Release notes",
                   "policy": [{"environment": "production", "actions": ["collections:read"]}]}),
        )
        .await,
    ));
    let agents = call(
        &client,
        AGENTS,
        "ListAgents",
        json!({"project": "docs-search"}),
    )
    .await;
    let agent = first(&agents, "agents", "id");
    out.push(("ListAgents", agents));
    out.push((
        "GetAgent",
        call(&client, AGENTS, "GetAgent", json!({"agent": agent})).await,
    ));
    out.push((
        "UpdateAgent",
        call(
            &client,
            AGENTS,
            "UpdateAgent",
            json!({"agent": {"id": agent, "description": "Writes the notes"}, "updateMask": "description"}),
        )
        .await,
    ));
    out.push((
        "SuspendAgent",
        call(&client, AGENTS, "SuspendAgent", json!({"agent": agent})).await,
    ));
    out.push((
        "ResumeAgent",
        call(&client, AGENTS, "ResumeAgent", json!({"agent": agent})).await,
    ));
    out.push((
        "ListAgentTokens",
        call(&client, AGENTS, "ListAgentTokens", json!({"agent": agent})).await,
    ));
    out.push((
        "CreateTrustPolicy",
        call(
            &client,
            AGENTS,
            "CreateTrustPolicy",
            json!({"agent": agent, "name": "CI", "issuer": "https://ci.example.test",
                   "subjectPattern": "repo:acme/*", "environments": ["production"]}),
        )
        .await,
    ));
    out.push((
        "ListTrustPolicies",
        call(
            &client,
            AGENTS,
            "ListTrustPolicies",
            json!({"agent": agent}),
        )
        .await,
    ));
    out.push((
        "CreateServiceAccount",
        call(
            &client,
            AGENTS,
            "CreateServiceAccount",
            json!({"project": "docs-search", "name": "ci"}),
        )
        .await,
    ));
    out.push((
        "ListServiceAccounts",
        call(
            &client,
            AGENTS,
            "ListServiceAccounts",
            json!({"project": "docs-search"}),
        )
        .await,
    ));
    let key_request = json!({"project": "docs-search", "environment": "production",
                             "name": "ci", "idempotencyKey": "script-1"});
    out.push((
        "CreateKey",
        call(&client, KEYS, "CreateKey", key_request.clone()).await,
    ));
    out.push((
        "CreateKey (replay)",
        call(&client, KEYS, "CreateKey", key_request).await,
    ));
    out.push((
        "CreateKey (no key)",
        call(
            &client,
            KEYS,
            "CreateKey",
            json!({"project": "docs-search", "environment": "production", "name": "ci"}),
        )
        .await,
    ));
    let keys = call(&client, KEYS, "ListKeys", env.clone()).await;
    let key = first(&keys, "keys", "id");
    out.push(("ListKeys", keys));
    out.push((
        "RevokeKey",
        call(&client, KEYS, "RevokeKey", json!({"key": key})).await,
    ));
    out.push((
        "RevokeToken",
        call(&client, KEYS, "RevokeToken", json!({"jti": "tok_9f2c"})).await,
    ));
    out.push((
        "GetProject (unknown)",
        call(&client, PROJECTS, "GetProject", json!({"project": "nope"})).await,
    ));
    out.push((
        "DecideConsent (deny)",
        call(
            &client,
            AUTH,
            "DecideConsent",
            json!({"decision": "CONSENT_DECISION_DENY", "redirectUri": "https://app.example.test/cb",
                   "state": "s1"}),
        )
        .await,
    ));
    let audit = call(&client, AUDIT, "ListAuditEvents", json!({})).await;
    let events: Vec<(String, String)> = audit.ok()["events"]
        .as_array()
        .expect("test fixture")
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap_or_default().to_owned(),
                e["action"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    out.push(("ListAuditEvents", audit));
    out.push((
        "DeleteSession",
        call(&client, AUTH, "DeleteSession", json!({})).await,
    ));
    (out, events)
}

/// The console mock and the server answer a scripted console session alike:
/// the same outcome and reason at every step, the same resource fields, the
/// same cookies, and the same audit trail for the session's mutations. The
/// server also keeps what it was told.
#[tokio::test]
async fn mock_and_server_answer_alike() {
    let mock = loams_apps_mock::serve(loams_apps_mock::MockConfig {
        listen: console_support::LOOPBACK0,
        ..loams_apps_mock::MockConfig::default()
    })
    .await
    .expect("the mock");
    let engine = Engine::dev().await;
    let (mock_steps, mock_audit) = script(Client::new(&mock.url())).await;
    let (server_steps, server_audit) = script(engine.client()).await;
    assert_eq!(mock_steps.len(), server_steps.len());
    for ((step, on_mock), (_, on_server)) in mock_steps.iter().zip(&server_steps) {
        assert_eq!(
            shape(on_mock),
            shape(on_server),
            "{step}: mock {} / server {}",
            on_mock.body,
            on_server.body
        );
        assert_eq!(
            cookie_attributes(on_mock),
            cookie_attributes(on_server),
            "{step}: cookies"
        );
    }
    // The mock's Connect events (`evt_c…`, above its seed's) are the
    // script's mutations; the server records the same ones, plus its own
    // sign-in and setup events outside the mock's vocabulary.
    let mock_connect: Vec<&str> = mock_audit
        .iter()
        .filter(|(id, _)| id.starts_with("evt_c"))
        .map(|(_, action)| action.as_str())
        .collect();
    let server_mutations: Vec<&str> = server_audit
        .iter()
        .map(|(_, action)| action.as_str())
        .filter(|action| MOCK_ACTIONS.contains(action))
        .collect();
    assert_eq!(server_mutations, mock_connect);
    assert_eq!(server_mutations.len(), 16, "{server_mutations:?}");
    // What the server kept (the mock keeps none of it).
    let step = |name: &str| &server_steps.iter().find(|(s, _)| *s == name).unwrap().1;
    assert_eq!(
        first(step("ListProjects"), "projects", "slug"),
        "docs-search"
    );
    assert_eq!(
        step("ListProjects").body["projects"][0]["environmentCount"],
        1
    );
    assert_eq!(step("GetOrg").body["org"]["name"], "Acme");
    assert_eq!(step("UpdateOrg").body["org"]["name"], "Acme Inc");
    assert_eq!(
        step("GetTeam").body["members"].as_array().map(Vec::len),
        None
    );
    assert_eq!(
        step("ListAccess").body["grants"][0]["subject"]["name"],
        "Ada Admin"
    );
    assert_eq!(
        step("ListServiceAccounts").body["serviceAccounts"][0]["name"],
        "ci"
    );
    assert_eq!(step("ListKeys").body["keys"].as_array().unwrap().len(), 1);
    mock.stop().await;
    engine.stop().await;
}

/// `CreateKey` answers the secret once per `idempotency_key`; a replay is
/// `secret_already_issued`, and the replay made no second key.
#[tokio::test]
async fn create_key_returns_secret_once() {
    let engine = Engine::dev().await;
    let mut owner = engine.client();
    owner.setup(SETUP_TOKEN).await;
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "Web"}))
        .await
        .ok();
    owner
        .call(
            PROJECTS,
            "CreateEnvironment",
            json!({"project": "web", "name": "Prod"}),
        )
        .await
        .ok();
    let request = json!({"project": "web", "environment": "prod", "name": "deploy",
                         "idempotencyKey": "once-1", "scopes": ["collections:read"],
                         "expiresInDays": 30});
    let created = owner.call(KEYS, "CreateKey", request.clone()).await;
    let secret = created.ok()["secret"].as_str().unwrap();
    assert!(secret.starts_with("lk_"));
    assert_eq!(created.ok()["key"]["prefix"], secret[..11]);
    assert!(created.ok()["key"]["expiresAt"].is_string());
    let replay = owner.call(KEYS, "CreateKey", request).await;
    assert_eq!(
        replay.failure(),
        ("failed_precondition".into(), "secret_already_issued".into())
    );
    assert!(!replay.body.to_string().contains(secret));
    let keys = owner
        .call(
            KEYS,
            "ListKeys",
            json!({"project": "web", "environment": "prod"}),
        )
        .await;
    let listed = keys.ok()["keys"].as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        !keys.body.to_string().contains(secret),
        "lists carry no secret"
    );
    // A new idempotency key makes a new key.
    owner
        .call(
            KEYS,
            "CreateKey",
            json!({"project": "web", "environment": "prod", "name": "deploy",
                   "idempotencyKey": "once-2"}),
        )
        .await
        .ok();
    engine.stop().await;
}

/// A member can read but not create a key: `permission_denied`, reason
/// `role_required`, naming the role.
#[tokio::test]
async fn member_cannot_create_key() {
    let engine = Engine::guarded(vec![Arc::new(TestAuthenticator)]).await;
    let mut owner = engine.client();
    owner.setup(SETUP_TOKEN).await;
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "Web"}))
        .await
        .ok();
    owner
        .call(
            PROJECTS,
            "CreateEnvironment",
            json!({"project": "web", "name": "Prod"}),
        )
        .await
        .ok();
    owner
        .call(
            ORG,
            "CreateInvitation",
            json!({"email": "mel@example.test"}),
        )
        .await
        .ok();
    let member = engine
        .client()
        .with_bearer(&TestAuthenticator::token("mel"));
    member
        .call(
            KEYS,
            "ListKeys",
            json!({"project": "web", "environment": "prod"}),
        )
        .await
        .ok();
    let refused = member
        .call(
            KEYS,
            "CreateKey",
            json!({"project": "web", "environment": "prod", "name": "mine",
                   "idempotencyKey": "m-1"}),
        )
        .await;
    assert_eq!(
        refused.failure(),
        ("permission_denied".into(), "role_required".into())
    );
    assert!(refused.body["message"].as_str().unwrap().contains("admin"));
    // Nor any other mutation; and the owner's role is untouched by theirs.
    let refused = member
        .call(PROJECTS, "CreateProject", json!({"name": "Mine"}))
        .await;
    assert_eq!(refused.failure().1, "role_required");
    // Inviting an owner needs an owner.
    owner
        .call(
            ORG,
            "CreateInvitation",
            json!({"email": "adm@example.test", "role": "ORG_ROLE_ADMIN"}),
        )
        .await
        .ok();
    let admin = engine
        .client()
        .with_bearer(&TestAuthenticator::token("adm"));
    let refused = admin
        .call(
            ORG,
            "CreateInvitation",
            json!({"email": "o2@example.test", "role": "ORG_ROLE_OWNER"}),
        )
        .await;
    assert_eq!(refused.failure().1, "role_required");
    // Someone the authenticator admits but nobody invited is not in the org.
    let stranger = engine
        .client()
        .with_bearer(&TestAuthenticator::token("eve"));
    let refused = stranger.call(ORG, "GetOrg", json!({})).await;
    assert_eq!(refused.failure().1, "role_required");
    engine.stop().await;
}

/// Ruling T5.R1: removing a member revokes the keys that act as them, so a removed admin's
/// key can no longer authenticate; a service account's key they made keeps working.
#[tokio::test]
async fn removing_a_member_revokes_their_keys() {
    let engine = Engine::guarded(vec![Arc::new(TestAuthenticator)]).await;
    let mut owner = engine.client();
    owner.setup(SETUP_TOKEN).await;
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "Web"}))
        .await
        .ok();
    owner
        .call(
            PROJECTS,
            "CreateEnvironment",
            json!({"project": "web", "name": "Prod"}),
        )
        .await
        .ok();
    owner
        .call(
            ORG,
            "CreateInvitation",
            json!({"email": "adm@example.test", "role": "ORG_ROLE_ADMIN"}),
        )
        .await
        .ok();
    let admin = engine
        .client()
        .with_bearer(&TestAuthenticator::token("adm"));
    admin.call(ORG, "GetOrg", json!({})).await.ok();
    let own = admin
        .call(
            KEYS,
            "CreateKey",
            json!({"project": "web", "environment": "prod", "name": "adm-key",
                   "idempotencyKey": "adm-1"}),
        )
        .await;
    let own = engine
        .client()
        .with_bearer(own.ok()["secret"].as_str().unwrap());
    let account = admin
        .call(
            AGENTS,
            "CreateServiceAccount",
            json!({"project": "web", "name": "ci"}),
        )
        .await;
    let account = account.ok()["serviceAccount"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ci = admin
        .call(
            KEYS,
            "CreateKey",
            json!({"project": "web", "environment": "prod", "name": "ci-key",
                   "serviceAccount": account, "idempotencyKey": "adm-2"}),
        )
        .await;
    let ci = engine
        .client()
        .with_bearer(ci.ok()["secret"].as_str().unwrap());
    own.call(ORG, "GetOrg", json!({})).await.ok();
    let members = owner.call(ORG, "ListMembers", json!({})).await;
    let user = members.ok()["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["user"]["email"] == "adm@example.test")
        .map(|m| m["user"]["id"].as_str().unwrap().to_owned())
        .unwrap();
    owner
        .call(ORG, "RemoveMember", json!({"user": user}))
        .await
        .ok();
    let refused = own
        .call(PROJECTS, "CreateProject", json!({"name": "After"}))
        .await;
    assert_eq!(
        refused.failure(),
        ("unauthenticated".into(), "token_revoked".into())
    );
    ci.call(ORG, "GetOrg", json!({})).await.ok();
    let audit = owner.call(AUDIT, "ListAuditEvents", json!({})).await;
    assert!(
        audit.ok()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "api_key.revoke" && e["target"]["name"] == "adm-key"),
        "the revocation is audited"
    );
    engine.stop().await;
}

/// IA2 Task 5 with ruling T5.R2: once an admin is removed from the org, the key they made can
/// neither change nor read anything. `RemoveMember` revokes it (T5.R2); behind that, the admin
/// RPCs check the person's live membership in the model (`can_admin`/`can_view` on the org).
#[tokio::test]
async fn a_removed_admins_key_loses_admin_rpcs() {
    let engine = Engine::guarded(vec![Arc::new(TestAuthenticator)]).await;
    let mut owner = engine.client();
    owner.setup(SETUP_TOKEN).await;
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "Web"}))
        .await
        .ok();
    owner
        .call(
            PROJECTS,
            "CreateEnvironment",
            json!({"project": "web", "name": "Prod"}),
        )
        .await
        .ok();
    owner
        .call(
            ORG,
            "CreateInvitation",
            json!({"email": "adm@example.test", "role": "ORG_ROLE_ADMIN"}),
        )
        .await
        .ok();
    let admin = engine
        .client()
        .with_bearer(&TestAuthenticator::token("adm"));
    admin.call(ORG, "GetOrg", json!({})).await.ok();
    let created = admin
        .call(
            KEYS,
            "CreateKey",
            json!({"project": "web", "environment": "prod", "name": "adm-key",
                   "idempotencyKey": "adm-1"}),
        )
        .await;
    let secret = created.ok()["secret"].as_str().unwrap().to_owned();
    let key = engine.client().with_bearer(&secret);
    key.call(PROJECTS, "CreateProject", json!({"name": "Before"}))
        .await
        .ok();
    let members = owner.call(ORG, "ListMembers", json!({})).await;
    let user = members.ok()["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["user"]["email"] == "adm@example.test")
        .map(|m| m["user"]["id"].as_str().unwrap().to_owned())
        .unwrap();
    owner
        .call(ORG, "RemoveMember", json!({"user": user}))
        .await
        .ok();
    let refused = key
        .call(PROJECTS, "CreateProject", json!({"name": "After"}))
        .await;
    assert_eq!(
        refused.failure(),
        ("unauthenticated".into(), "token_revoked".into())
    );
    let refused = key.call(ORG, "GetOrg", json!({})).await;
    assert_eq!(refused.failure().1, "token_revoked", "nor read");
    engine.stop().await;
}

/// Before setup every admin RPC is `setup_required`; the last owner cannot
/// be removed; removing a member ends their sessions.
#[tokio::test]
async fn setup_and_membership_rules_rpc() {
    let engine = Engine::dev().await;
    let anonymous = engine.client();
    let early = anonymous.call(ORG, "GetOrg", json!({})).await;
    assert_eq!(
        early.failure(),
        ("failed_precondition".into(), "setup_required".into())
    );
    let mut owner = engine.client();
    owner.setup(SETUP_TOKEN).await;
    let members = owner.call(ORG, "ListMembers", json!({})).await;
    let me = members.ok()["members"][0]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let last = owner.call(ORG, "RemoveMember", json!({"user": me})).await;
    assert_eq!(last.failure().0, "failed_precondition");
    let missing = owner
        .call(ORG, "RemoveMember", json!({"user": "usr_nope"}))
        .await;
    assert_eq!(missing.failure(), ("not_found".into(), "not_found".into()));
    let unknown_mask = owner
        .call(
            ORG,
            "UpdateOrg",
            json!({"org": {"slug": "x"}, "updateMask": "slug"}),
        )
        .await;
    assert_eq!(unknown_mask.failure().0, "invalid_argument");
    let dup = owner
        .call(PROJECTS, "CreateProject", json!({"name": "A"}))
        .await;
    dup.ok();
    let dup = owner
        .call(PROJECTS, "CreateProject", json!({"name": "A"}))
        .await;
    assert_eq!(dup.failure().0, "already_exists");
    engine.stop().await;
}

/// Reads the frames of a Connect server stream as they arrive.
struct Frames {
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
    buffer: Vec<u8>,
}

impl Frames {
    async fn open(client: &Client, base: &str, body: Value) -> Self {
        let mut frame = vec![0u8];
        let json = body.to_string();
        frame.extend_from_slice(
            &u32::try_from(json.len())
                .expect("test fixture")
                .to_be_bytes(),
        );
        frame.extend_from_slice(json.as_bytes());
        let (token, csrf) = client.session.clone().expect("test fixture");
        let response = reqwest::Client::new()
            .post(format!(
                "{base}/loams.admin.v1.AuditService/WatchAuditEvents"
            ))
            .header("content-type", "application/connect+json")
            .header("connect-protocol-version", "1")
            .header("cookie", format!("loams_session={token}"))
            .header("x-csrf-token", csrf)
            .body(frame)
            .send()
            .await
            .expect("test fixture");
        assert_eq!(response.status().as_u16(), 200);
        Self {
            stream: Box::pin(response.bytes_stream()),
            buffer: Vec::new(),
        }
    }

    /// The next message (an end-of-stream frame is an error).
    async fn next(&mut self) -> Value {
        loop {
            if self.buffer.len() >= 5 {
                let len = u32::from_be_bytes(self.buffer[1..5].try_into().expect("test fixture"))
                    as usize;
                if self.buffer.len() >= 5 + len {
                    let flags = self.buffer[0];
                    let message: Value =
                        serde_json::from_slice(&self.buffer[5..5 + len]).expect("test fixture");
                    self.buffer.drain(..5 + len);
                    assert_eq!(flags & 2, 0, "the stream ended: {message}");
                    return message;
                }
            }
            let chunk = tokio::time::timeout(Duration::from_secs(10), self.stream.next())
                .await
                .expect("a frame within 10 s")
                .expect("the stream is open")
                .expect("test fixture");
            self.buffer.extend_from_slice(&chunk);
        }
    }
}

/// The watch starts with a snapshot and its cursor, streams each mutation as
/// an `append`, replays what a resumed cursor missed, and resets with a
/// fresh snapshot for a cursor it does not hold.
#[tokio::test]
async fn watch_audit_resumes_from_cursor() {
    let engine = Engine::dev().await;
    let mut owner = engine.client();
    owner.setup(SETUP_TOKEN).await;
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "One"}))
        .await
        .ok();
    let mut watch = Frames::open(&owner, &engine.base, json!({"snapshotSize": 10})).await;
    let snapshot = watch.next().await;
    let events = snapshot["snapshot"]["events"].as_array().unwrap();
    assert_eq!(events[0]["action"], "project.create");
    let cursor = snapshot["cursor"].as_str().unwrap().to_owned();
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "Two"}))
        .await
        .ok();
    let appended = watch.next().await;
    assert_eq!(appended["append"]["action"], "project.create");
    assert_eq!(appended["append"]["target"]["name"], "Two");
    assert_ne!(appended["cursor"], cursor.as_str());
    drop(watch);
    owner
        .call(PROJECTS, "CreateProject", json!({"name": "Three"}))
        .await
        .ok();
    // Resumed from the snapshot's cursor: the two it missed, in order, and
    // no snapshot.
    let mut resumed = Frames::open(&owner, &engine.base, json!({"resumeCursor": cursor})).await;
    let missed: Vec<String> = [resumed.next().await, resumed.next().await]
        .iter()
        .map(|m| {
            assert!(m.get("snapshot").is_none(), "{m}");
            m["append"]["target"]["name"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(missed, ["Two", "Three"]);
    drop(resumed);
    let mut reset = Frames::open(&owner, &engine.base, json!({"resumeCursor": "a999999"})).await;
    let fresh = reset.next().await;
    assert_eq!(fresh["snapshotReset"], true);
    assert!(fresh["snapshot"].is_object());
    // A filter on another project's events selects none of them.
    drop(reset);
    let mut filtered = Frames::open(
        &owner,
        &engine.base,
        json!({"project": "nope", "snapshotSize": 5}),
    )
    .await;
    let empty = filtered.next().await;
    assert!(empty["snapshot"].get("events").is_none(), "{empty}");
    let _ = reason_of;
    engine.stop().await;
}
