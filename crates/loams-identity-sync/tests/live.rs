//! IA3 against a running Zitadel and OpenFGA (the IA0 stack, `conformance/identity`), when
//! `LOAMS_IDENTITY_ZITADEL_URL` is set; skipped otherwise. The same variables as
//! `loams-zitadel`'s live test (CI: the Identity workflow).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use loams_authz::OpenFgaAuthorizer;
use loams_identity_sync::memory::{MemoryPrincipals, MemoryState};
use loams_identity_sync::reconcile::reconcile;
use loams_identity_sync::{Config, IdentitySync, Kind};
use loams_zitadel::Zitadel;
use serde_json::{Value, json};

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// A management API call in `org` (roles and grants are not in `loams-zitadel`'s client).
async fn manage(
    url: &str,
    token: &str,
    org: &str,
    method: reqwest::Method,
    path: &str,
    body: Value,
) -> Value {
    let res = reqwest::Client::new()
        .request(method, format!("{url}{path}"))
        .bearer_auth(token)
        .header("x-zitadel-orgid", org)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = res.status();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    // A role that exists already is fine.
    assert!(
        status.is_success() || status.as_u16() == 409,
        "{path}: {status} {body}"
    );
    body
}

#[tokio::test]
async fn membership_follows_user_grants_and_deactivation() {
    let Some(url) = var("LOAMS_IDENTITY_ZITADEL_URL") else {
        eprintln!("skipped: LOAMS_IDENTITY_ZITADEL_URL is not set");
        return;
    };
    let token = match var("LOAMS_IDENTITY_ZITADEL_TOKEN_FILE") {
        Some(path) => std::fs::read_to_string(path).unwrap().trim().to_owned(),
        None => var("LOAMS_IDENTITY_ZITADEL_TOKEN").expect("LOAMS_IDENTITY_ZITADEL_TOKEN(_FILE)"),
    };
    let org = var("LOAMS_IDENTITY_ZITADEL_ORG").expect("LOAMS_IDENTITY_ZITADEL_ORG");
    let project = var("LOAMS_IDENTITY_ZITADEL_PROJECT").expect("LOAMS_IDENTITY_ZITADEL_PROJECT");
    let issuer = var("LOAMS_IDENTITY_ZITADEL_ISSUER").unwrap_or_else(|| url.clone());
    let fga = Arc::new(
        OpenFgaAuthorizer::connect(
            &var("LOAMS_AUTHZ_OPENFGA_GRPC").expect("LOAMS_AUTHZ_OPENFGA_GRPC"),
            &var("LOAMS_AUTHZ_OPENFGA_STORE").expect("LOAMS_AUTHZ_OPENFGA_STORE"),
            &var("LOAMS_AUTHZ_OPENFGA_MODEL").expect("LOAMS_AUTHZ_OPENFGA_MODEL"),
            None,
        )
        .await
        .unwrap(),
    );
    let zitadel = Zitadel::new(&url, token.clone(), reqwest::Client::new()).unwrap();
    let links = Arc::new(MemoryPrincipals::default());
    let state = Arc::new(MemoryState::default());
    let sync = IdentitySync::new(
        zitadel.clone(),
        fga.clone(),
        links.clone(),
        state,
        Config::new(&project, &issuer),
    );

    for role in ["member", "admin", "owner"] {
        manage(
            &url,
            &token,
            &org,
            reqwest::Method::POST,
            &format!("/management/v1/projects/{project}/roles"),
            json!({"roleKey": role, "displayName": role}),
        )
        .await;
    }
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user = zitadel
        .create_machine_user(&org, &format!("ia3-{run}"), "IA3 live", None)
        .await
        .unwrap();
    let object = format!("org:{org}");
    let roles = || {
        let fga = fga.clone();
        let user = format!("user:{user}");
        let object = object.clone();
        async move {
            let mut out: Vec<String> = fga
                .read(Some(&user), None, &object)
                .await
                .unwrap()
                .into_iter()
                .map(|t| t.relation)
                .collect();
            out.sort();
            out
        }
    };
    // Steps until `want` holds, as the poll would; answers how long it took.
    let until = |want: Vec<&'static str>| {
        let sync = &sync;
        let roles = &roles;
        async move {
            let start = Instant::now();
            loop {
                sync.step().await.unwrap();
                if roles().await == want {
                    return start.elapsed();
                }
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "{want:?} not reached"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };

    let grant = manage(
        &url,
        &token,
        &org,
        reqwest::Method::POST,
        &format!("/management/v1/users/{user}/grants"),
        json!({"projectId": project, "roleKeys": ["member"]}),
    )
    .await;
    let grant = grant["userGrantId"].as_str().unwrap().to_owned();
    let took = until(vec!["member"]).await;
    eprintln!("grant added -> tuple in {took:?}");
    assert_eq!(
        links.get(&issuer, &user),
        Some((Kind::ServiceAccount, false)),
        "a machine user's link"
    );

    manage(
        &url,
        &token,
        &org,
        reqwest::Method::PUT,
        &format!("/management/v1/users/{user}/grants/{grant}"),
        json!({"roleKeys": ["admin", "member"]}),
    )
    .await;
    until(vec!["admin", "member"]).await;

    let report = reconcile(&sync).await.unwrap();
    eprintln!("reconcile: {report:?}");

    zitadel.deactivate(&user).await.unwrap();
    let took = until(vec![]).await;
    eprintln!("deactivated -> tuples gone in {took:?}");
    assert_eq!(
        links.get(&issuer, &user),
        Some((Kind::ServiceAccount, true))
    );
    zitadel.reactivate(&user).await.unwrap();
    until(vec!["admin", "member"]).await;
    assert_eq!(
        links.get(&issuer, &user),
        Some((Kind::ServiceAccount, false))
    );

    manage(
        &url,
        &token,
        &org,
        reqwest::Method::DELETE,
        &format!("/management/v1/users/{user}/grants/{grant}"),
        json!({}),
    )
    .await;
    let took = until(vec![]).await;
    eprintln!("grant removed -> tuples gone in {took:?}");
    zitadel.delete_user(&user).await.unwrap();
    sync.step().await.unwrap();
    assert!(roles().await.is_empty());
}
