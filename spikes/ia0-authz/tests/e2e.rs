//! IA0 Task 3 and 4 tests against the running stack (`conformance/identity/up.sh` plus the
//! `spike` profile). They skip unless `IA0_E2E=1`, so `cargo test` alone stays offline.
//!
//!   IA0_E2E=1 cargo test --release --manifest-path spikes/ia0-authz/Cargo.toml -- --test-threads 1

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use ia0_authz::config::Config;
use ia0_authz::fga::{self, Consistency, Fga};
use ia0_authz::jwt::Verifier;
use ia0_authz::server::verify_signature;
use ia0_authz::zitadel::{MachineKey, Zitadel};
use sha2::Sha256;

fn enabled() -> Option<Config> {
    if std::env::var("IA0_E2E").as_deref() != Ok("1") {
        eprintln!("skipped: set IA0_E2E=1 with the IA0 stack up");
        return None;
    }
    Some(Config::load().expect("config"))
}

async fn agent_token(cfg: &Config) -> String {
    let key: MachineKey =
        serde_json::from_str(&std::fs::read_to_string(cfg.path("IA0_AGENT_KEY").unwrap()).unwrap())
            .unwrap();
    Zitadel::new(cfg.req("IA0_ISSUER").unwrap())
        .agent_token(&key, cfg.req("IA0_AUDIENCE").unwrap())
        .await
        .unwrap()
        .access_token
}

async fn fga(cfg: &Config) -> Fga {
    let grpc = std::env::var("IA0_FGA_GRPC").unwrap_or_else(|_| "http://127.0.0.1:18082".into());
    Fga::connect(
        &grpc,
        cfg.req("IA0_STORE").unwrap(),
        cfg.req("IA0_MODEL").unwrap(),
        Duration::ZERO,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn rejects_wrong_aud() {
    let Some(cfg) = enabled() else { return };
    let token = agent_token(&cfg).await;
    let v = Verifier::new(cfg.req("IA0_ISSUER").unwrap(), "some-other-project", None);
    let e = v.verify(&token).await.unwrap_err();
    assert!(format!("{e}").contains("Audience"), "{e}");
    let ok = Verifier::new(
        cfg.req("IA0_ISSUER").unwrap(),
        cfg.req("IA0_AUDIENCE").unwrap(),
        None,
    );
    assert!(ok.verify(&token).await.is_ok());
}

#[tokio::test]
async fn rejects_wrong_iss() {
    let Some(cfg) = enabled() else { return };
    let token = agent_token(&cfg).await;
    // Same keys (fetched from the real issuer), but the verifier expects another issuer.
    let issuer = cfg.req("IA0_ISSUER").unwrap();
    let v = Verifier::new(issuer, cfg.req("IA0_AUDIENCE").unwrap(), None)
        .expect_issuer("https://evil.example");
    let e = v.verify(&token).await.unwrap_err();
    assert!(format!("{e:?}").contains("Issuer"), "{e:?}");
}

#[tokio::test]
async fn allow_after_grant_and_deny_after_revoke() {
    let Some(cfg) = enabled() else { return };
    let f = fga(&cfg).await;
    let agent = format!("agent:{}", cfg.req("IA0_AGENT").unwrap());
    f.write(
        vec![fga::tuple(
            "environment:t-env",
            "environment",
            "deployment:t-dep",
        )],
        Vec::new(),
    )
    .await
    .unwrap();
    let check = || {
        f.check(
            &agent,
            "can_view",
            "deployment:t-dep",
            Consistency::HigherConsistency,
            None,
        )
    };
    assert!(!check().await.unwrap().allowed, "deny by default");
    f.write(
        vec![fga::tuple(&agent, "editor", "environment:t-env")],
        Vec::new(),
    )
    .await
    .unwrap();
    assert!(check().await.unwrap().allowed, "allow after grant");
    f.write(
        Vec::new(),
        vec![fga::key(&agent, "editor", "environment:t-env")],
    )
    .await
    .unwrap();
    assert!(
        !check().await.unwrap().allowed,
        "deny after revoke, on the next HIGHER_CONSISTENCY check"
    );
}

#[tokio::test]
async fn idempotent_duplicate_add_and_missing_delete_is_success() {
    let Some(cfg) = enabled() else { return };
    let f = fga(&cfg).await;
    let t = || fga::tuple("user:dup", "member", "org:dup-org");
    f.write(vec![t()], Vec::new()).await.unwrap();
    f.write(vec![t()], Vec::new())
        .await
        .expect("a duplicate write succeeds");
    f.write(
        Vec::new(),
        vec![fga::key("user:dup", "member", "org:dup-org")],
    )
    .await
    .unwrap();
    f.write(
        Vec::new(),
        vec![fga::key("user:dup", "member", "org:dup-org")],
    )
    .await
    .expect("a missing delete succeeds");
}

#[tokio::test]
async fn rejects_bad_signature_at_the_endpoint() {
    let Some(cfg) = enabled() else { return };
    let url = format!(
        "{}/zitadel/events",
        cfg.or("IA0_SPIKE_URL", "http://127.0.0.1:18090")
    );
    let body = r#"{"aggregateID":"x","event_type":"user.deactivated","sequence":1}"#;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"not-the-key").unwrap();
    mac.update(format!("{now}.{body}").as_bytes());
    let sig = format!("t={now},v1={}", hex::encode(mac.finalize().into_bytes()));
    let res = reqwest::Client::new()
        .post(&url)
        .header("ZITADEL-Signature", sig)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = reqwest::Client::new()
        .post(&url)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[test]
fn signature_scheme() {
    let body = br#"{"a":1}"#;
    let mut mac = Hmac::<Sha256>::new_from_slice(b"k").unwrap();
    mac.update(b"1000.");
    mac.update(body);
    let good = format!("t=1000,v1={}", hex::encode(mac.finalize().into_bytes()));
    assert!(verify_signature(&good, body, "k", 1000));
    assert!(
        verify_signature(&good, body, "k", 1299),
        "inside the 5-minute tolerance"
    );
    assert!(
        !verify_signature(&good, body, "k", 1301),
        "replayed after 5 minutes"
    );
    assert!(!verify_signature(&good, body, "other", 1000), "wrong key");
    assert!(
        !verify_signature(&good, br#"{"a":2}"#, "k", 1000),
        "tampered body"
    );
    assert!(!verify_signature("t=1000", body, "k", 1000), "no v1");
}
