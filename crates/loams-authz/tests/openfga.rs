//! `OpenFgaAuthorizer` against a running OpenFGA with model v1 (`deploy/openfga/fga.mod`).
//!
//! Skipped unless `LOAMS_AUTHZ_OPENFGA_GRPC`, `LOAMS_AUTHZ_OPENFGA_STORE` and
//! `LOAMS_AUTHZ_OPENFGA_MODEL` are set; `conformance/identity/up.sh` starts one and writes the
//! store and model ids to `.env.ia0` (`IA0_FGA_GRPC`, `IA0_STORE`, `IA0_MODEL`).

#![allow(clippy::unwrap_used)]

use loams_authz::{
    Authorizer, AuthzError, CachedAuthorizer, CheckRequest, Consistency, ObjectRef,
    OpenFgaAuthorizer, Subject, Tuple,
};

async fn connect() -> Option<OpenFgaAuthorizer> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(url), Some(store), Some(model)) = (
        var("LOAMS_AUTHZ_OPENFGA_GRPC"),
        var("LOAMS_AUTHZ_OPENFGA_STORE"),
        var("LOAMS_AUTHZ_OPENFGA_MODEL"),
    ) else {
        eprintln!("skipped: LOAMS_AUTHZ_OPENFGA_{{GRPC,STORE,MODEL}} are not set");
        return None;
    };
    Some(
        OpenFgaAuthorizer::connect(
            &url,
            &store,
            &model,
            var("LOAMS_AUTHZ_OPENFGA_KEY").as_deref(),
        )
        .await
        .expect("connect"),
    )
}

/// A unique prefix per run, so reruns against one store do not see each other's tuples.
fn run_id() -> String {
    format!(
        "t{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn tree(r: &str, deployments: usize) -> Vec<Tuple> {
    let mut t = vec![
        Tuple::new(format!("org:{r}-org"), "org", format!("project:{r}-p")),
        Tuple::new(
            format!("project:{r}-p"),
            "project",
            format!("environment:{r}-dev"),
        ),
        Tuple::new(
            format!("project:{r}-p"),
            "project",
            format!("environment:{r}-prod"),
        ),
        Tuple::new(
            format!("environment:{r}-prod"),
            "environment",
            format!("deployment:{r}-prod-web"),
        ),
    ];
    for i in 0..deployments {
        t.push(Tuple::new(
            format!("environment:{r}-dev"),
            "environment",
            format!("deployment:{r}-d{i}"),
        ));
    }
    t
}

fn can(subject: &Subject, relation: &str, object: &str) -> CheckRequest {
    CheckRequest::new(subject.clone(), relation, object.parse().unwrap())
}

#[tokio::test]
async fn openfga_deny_by_default_then_allow_after_grant_then_deny_after_revoke() {
    let Some(fga) = connect().await else { return };
    let r = run_id();
    fga.write(&tree(&r, 1), &[]).await.unwrap();
    let agent = Subject::agent(format!("{r}-a"));
    let dep = format!("deployment:{r}-d0");
    let strong = |rel| can(&agent, rel, &dep).consistency(Consistency::HigherConsistency);
    let d = fga.check(strong("can_view")).await.unwrap();
    assert!(!d.allowed, "deny by default");
    assert_eq!(d.model_id.as_deref(), Some(fga.model_id()));

    let grant = Tuple::new(agent.fga_user(), "editor", format!("environment:{r}-dev"));
    fga.write_strict(std::slice::from_ref(&grant))
        .await
        .unwrap();
    assert!(fga.check(strong("can_view")).await.unwrap().allowed);
    assert!(fga.check(strong("can_operate")).await.unwrap().allowed);
    assert!(
        !fga.check(strong("can_destroy")).await.unwrap().allowed,
        "editor is not deployer"
    );
    let prod = can(&agent, "can_view", &format!("deployment:{r}-prod-web"))
        .consistency(Consistency::HigherConsistency);
    assert!(
        !fga.check(prod).await.unwrap().allowed,
        "an ungranted environment"
    );

    fga.write(&[], std::slice::from_ref(&grant)).await.unwrap();
    assert!(
        !fga.check(strong("can_view")).await.unwrap().allowed,
        "deny after revoke"
    );
    fga.write(&[], &[grant])
        .await
        .expect("a missing delete succeeds");
}

#[tokio::test]
async fn openfga_refuses_agent_on_people_only_relations() {
    let Some(fga) = connect().await else { return };
    let r = run_id();
    let agent = Subject::agent(format!("{r}-a")).fga_user();
    for (rel, obj) in [
        ("billing_admin", format!("org:{r}-org")),
        ("meter_admin", "platform:default".into()),
        ("operator", "platform:default".into()),
    ] {
        let e = fga
            .write_strict(&[Tuple::new(&agent, rel, obj)])
            .await
            .unwrap_err();
        assert!(
            matches!(&e, AuthzError::Invalid(m) if m.contains("type")),
            "{rel}: {e}"
        );
    }
}

#[tokio::test]
async fn openfga_batch_check_over_50() {
    let Some(fga) = connect().await else { return };
    let r = run_id();
    fga.write(&tree(&r, 60), &[]).await.unwrap();
    let agent = Subject::agent(format!("{r}-a"));
    fga.write(
        &[Tuple::new(
            agent.fga_user(),
            "viewer",
            format!("environment:{r}-dev"),
        )],
        &[],
    )
    .await
    .unwrap();
    let mut reqs: Vec<CheckRequest> = (0..60)
        .map(|i| can(&agent, "can_view", &format!("deployment:{r}-d{i}")))
        .collect();
    reqs.push(can(&agent, "can_view", &format!("deployment:{r}-prod-web")));
    let ds = fga.batch_check(reqs).await.unwrap();
    assert_eq!(ds.len(), 61);
    assert!(ds[..60].iter().all(|d| d.allowed));
    assert!(!ds[60].allowed, "order is kept across chunks");
}

#[tokio::test]
async fn openfga_list_objects() {
    let Some(fga) = connect().await else { return };
    let r = run_id();
    fga.write(&tree(&r, 2), &[]).await.unwrap();
    let agent = Subject::agent(format!("{r}-a"));
    fga.write(
        &[Tuple::new(
            agent.fga_user(),
            "editor",
            format!("environment:{r}-dev"),
        )],
        &[],
    )
    .await
    .unwrap();
    let got = fga
        .list_objects(&agent, "can_view", "deployment")
        .await
        .unwrap();
    let want: Vec<ObjectRef> = (0..2)
        .map(|i| ObjectRef::new("deployment", format!("{r}-d{i}")))
        .collect();
    assert_eq!(got, want);
}

#[tokio::test]
async fn openfga_agent_session_condition() {
    let Some(fga) = connect().await else { return };
    let r = run_id();
    fga.write(&tree(&r, 1), &[]).await.unwrap();
    let agent = Subject::agent(format!("{r}-a")).with_session("s-1");
    let mut ctx = serde_json::Map::new();
    ctx.insert("allowed_session".into(), "s-1".into());
    ctx.insert("expires".into(), "2099-01-01T00:00:00Z".into());
    fga.write_strict(&[
        Tuple::new(agent.fga_user(), "editor", format!("environment:{r}-dev"))
            .with_condition("agent_session", ctx),
    ])
    .await
    .unwrap();
    // The session comes from the authenticated subject (T3.R1), never from the caller.
    let at = |session: &str, now: &str| {
        can(
            &Subject::agent(format!("{r}-a")).with_session(session),
            "can_view",
            &format!("deployment:{r}-d0"),
        )
        .context("now", now)
    };
    assert!(
        fga.check(at("s-1", "2026-10-21T12:00:00Z"))
            .await
            .unwrap()
            .allowed
    );
    assert!(
        !fga.check(at("s-2", "2026-10-21T12:00:00Z"))
            .await
            .unwrap()
            .allowed,
        "another session"
    );
    assert!(
        !fga.check(at("s-2", "2026-10-21T12:00:00Z").context("session_id", "s-1"))
            .await
            .unwrap()
            .allowed,
        "a caller cannot claim the granted session for another one"
    );
    assert!(
        !fga.check(at("s-1", "2099-01-01T00:00:01Z"))
            .await
            .unwrap()
            .allowed,
        "after expiry"
    );
}

#[tokio::test]
async fn cached_openfga_serves_allows_and_strict_checks_see_revocation() {
    let Some(fga) = connect().await else { return };
    let r = run_id();
    fga.write(&tree(&r, 1), &[]).await.unwrap();
    let agent = Subject::agent(format!("{r}-a"));
    let grant = Tuple::new(agent.fga_user(), "editor", format!("environment:{r}-dev"));
    fga.write(std::slice::from_ref(&grant), &[]).await.unwrap();
    let cached = CachedAuthorizer::new(fga.clone(), std::time::Duration::from_secs(5), 1000);
    let req = || can(&agent, "can_view", &format!("deployment:{r}-d0"));
    assert!(!cached.check(req()).await.unwrap().cached);
    fga.write(&[], &[grant]).await.unwrap();
    assert!(
        cached.check(req()).await.unwrap().cached,
        "a stale allow inside the TTL"
    );
    let strict = cached
        .check(req().consistency(Consistency::HigherConsistency))
        .await
        .unwrap();
    assert!(
        !strict.allowed && !strict.cached,
        "a strict check sees the revocation at once"
    );
}
