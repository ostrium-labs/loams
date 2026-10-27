//! IA4 Task 1 against a running Zitadel and OpenFGA (the IA0 stack, `conformance/identity`),
//! when `LOAMS_IDENTITY_ZITADEL_URL` is set; skipped otherwise. CI: the Identity workflow's
//! `loams-zitadel` job.
//!
//! - `LOAMS_IDENTITY_ZITADEL_URL`: the instance (`http://localhost:18080`), also the issuer
//!   unless `LOAMS_IDENTITY_ZITADEL_ISSUER` says otherwise;
//! - `LOAMS_IDENTITY_ZITADEL_TOKEN_FILE` (or `_TOKEN`): an admin machine user's PAT;
//! - `LOAMS_IDENTITY_ZITADEL_ORG`, `LOAMS_IDENTITY_ZITADEL_PROJECT`: the org and the Loams
//!   project (the audience);
//! - `LOAMS_AUTHZ_OPENFGA_{GRPC,STORE,MODEL}`: OpenFGA with model v1.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use loams_authz::{Authorizer, CheckRequest, Consistency, OpenFgaAuthorizer, Subject, Tuple};
use loams_zitadel::agents::{Agents, MemoryLinks, NewAgent};
use loams_zitadel::token::{MachineKey, project_audience_scope};
use loams_zitadel::{UserState, Zitadel};

struct Live {
    agents: Agents<OpenFgaAuthorizer, MemoryLinks>,
    fga: OpenFgaAuthorizer,
    issuer: String,
    project: String,
    run: String,
}

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

async fn live() -> Option<Live> {
    let Some(url) = var("LOAMS_IDENTITY_ZITADEL_URL") else {
        eprintln!("skipped: LOAMS_IDENTITY_ZITADEL_URL is not set");
        return None;
    };
    let token = match var("LOAMS_IDENTITY_ZITADEL_TOKEN_FILE") {
        Some(path) => std::fs::read_to_string(path).unwrap().trim().to_owned(),
        None => var("LOAMS_IDENTITY_ZITADEL_TOKEN").expect("LOAMS_IDENTITY_ZITADEL_TOKEN(_FILE)"),
    };
    let issuer = var("LOAMS_IDENTITY_ZITADEL_ISSUER").unwrap_or_else(|| url.clone());
    let fga = OpenFgaAuthorizer::connect(
        &var("LOAMS_AUTHZ_OPENFGA_GRPC").expect("LOAMS_AUTHZ_OPENFGA_GRPC"),
        &var("LOAMS_AUTHZ_OPENFGA_STORE").expect("LOAMS_AUTHZ_OPENFGA_STORE"),
        &var("LOAMS_AUTHZ_OPENFGA_MODEL").expect("LOAMS_AUTHZ_OPENFGA_MODEL"),
        None,
    )
    .await
    .unwrap();
    let z = Zitadel::new(&url, token, reqwest::Client::new()).unwrap();
    let run = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    Some(Live {
        agents: Agents::new(
            z,
            issuer.clone(),
            var("LOAMS_IDENTITY_ZITADEL_ORG").expect("LOAMS_IDENTITY_ZITADEL_ORG"),
            fga.clone(),
            MemoryLinks::default(),
        )
        .with_key_ttl(Duration::from_secs(3600)),
        fga,
        issuer,
        project: var("LOAMS_IDENTITY_ZITADEL_PROJECT").expect("LOAMS_IDENTITY_ZITADEL_PROJECT"),
        run,
    })
}

impl Live {
    /// A project `p<run>` with environments `dev<run>` and `prod<run>`, administered by
    /// `owner<run>`, and an agent of it owned by them.
    async fn seeded(&self, name: &str) -> (String, MachineKey) {
        let r = &self.run;
        self.fga
            .write(
                &[
                    Tuple::new(
                        format!("project:p{r}"),
                        "project",
                        format!("environment:dev{r}"),
                    ),
                    Tuple::new(
                        format!("project:p{r}"),
                        "project",
                        format!("environment:prod{r}"),
                    ),
                    Tuple::new(format!("user:owner{r}"), "admin", format!("project:p{r}")),
                ],
                &[],
            )
            .await
            .unwrap();
        let agent = self
            .agents
            .create(&NewAgent {
                username: &format!("{name}-{r}"),
                name,
                description: None,
                owner: &format!("owner{r}"),
                project: &format!("p{r}"),
            })
            .await
            .unwrap();
        let key = MachineKey::from_json(agent.key.key_file.as_deref().unwrap()).unwrap();
        assert_eq!(key.user_id, agent.id);
        (agent.id, key)
    }

    async fn can(&self, agent: &str, relation: &str, env: &str) -> bool {
        self.fga
            .check(
                CheckRequest::new(
                    Subject::agent(agent),
                    relation,
                    format!("environment:{env}{}", self.run).parse().unwrap(),
                )
                .consistency(Consistency::HigherConsistency),
            )
            .await
            .unwrap()
            .allowed
    }

    async fn token(&self, key: &MachineKey) -> loams_zitadel::Result<String> {
        key.access_token(
            &reqwest::Client::new(),
            &self.issuer,
            &["openid", &project_audience_scope(&self.project)],
        )
        .await
        .map(|t| t.access_token)
    }
}

#[tokio::test]
async fn create_agent_has_no_grant() {
    let Some(l) = live().await else { return };
    let (agent, key) = l.seeded("no-grant").await;
    for rel in ["viewer", "editor", "deployer"] {
        assert!(!l.can(&agent, rel, "dev").await, "{rel} on dev");
        assert!(!l.can(&agent, rel, "prod").await, "{rel} on prod");
    }
    assert_eq!(
        l.agents.zitadel().user_state(&agent).await.unwrap(),
        UserState::Active
    );
    let token = l.token(&key).await.expect("an agent gets a token");
    assert_eq!(token.split('.').count(), 3, "JWT access tokens");
}

#[tokio::test]
async fn grant_reaches_only_that_environment() {
    let Some(l) = live().await else { return };
    let (agent, _) = l.seeded("granted").await;
    l.agents
        .grant(&agent, "editor", &format!("dev{}", l.run), None)
        .await
        .unwrap();
    assert!(l.can(&agent, "editor", "dev").await);
    assert!(l.can(&agent, "viewer", "dev").await);
    assert!(!l.can(&agent, "deployer", "dev").await);
    assert!(!l.can(&agent, "viewer", "prod").await);
}

#[tokio::test]
async fn revoke_denies_within_seconds() {
    let Some(l) = live().await else { return };
    let (agent, _) = l.seeded("revoked").await;
    let grant = l
        .agents
        .grant(&agent, "editor", &format!("dev{}", l.run), None)
        .await
        .unwrap();
    assert!(l.can(&agent, "editor", "dev").await);
    let started = Instant::now();
    l.agents.revoke(&agent, &[grant]).await.unwrap();
    assert!(l.agents.links().get(&l.issuer, &agent).unwrap().disabled);
    assert!(!l.can(&agent, "editor", "dev").await);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(2), "revocation took {took:?}");
    assert_eq!(
        l.agents.zitadel().user_state(&agent).await.unwrap(),
        UserState::Inactive
    );
}

#[tokio::test]
async fn deactivated_agent_gets_no_token() {
    let Some(l) = live().await else { return };
    let (agent, key) = l.seeded("deactivated").await;
    l.token(&key).await.expect("before");
    l.agents.revoke(&agent, &[]).await.unwrap();
    let err = l
        .token(&key)
        .await
        .expect_err("a deactivated machine user gets no token");
    assert!(matches!(err, loams_zitadel::Error::Status { .. }), "{err}");
    l.agents.reinstate(&agent).await.unwrap();
    l.token(&key).await.expect("after reinstating");
    l.agents.zitadel().delete_user(&agent).await.unwrap();
}
