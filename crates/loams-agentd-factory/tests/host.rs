//! The factory host (configure, test, list, remove, query): the TypeScript host's
//! behaviour, and no secret in any reply or error.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loams_agentd_factory::{
    ConfigStore, FactoryHost, MemorySecrets, OpError, SecretStore, Upstream, UpstreamRequest,
    UpstreamResponse,
};
use loams_agentd_proto::factory::{FactoryConfigureParams, FactoryHealth, FactoryQueryParams};
use serde_json::json;

const SECRET: &str = "tok-S3CRET/+x";

/// Answers every request with `status` and `body`, and records the requests.
#[derive(Debug)]
struct Fixed {
    status: Mutex<u16>,
    body: Mutex<String>,
    sent: Mutex<Vec<UpstreamRequest>>,
}

impl Fixed {
    fn new(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(status),
            body: Mutex::new(body.to_owned()),
            sent: Mutex::new(Vec::new()),
        })
    }
    fn answer(&self, status: u16, body: &str) {
        *self.status.lock().unwrap() = status;
        *self.body.lock().unwrap() = body.to_owned();
    }
}

#[async_trait]
impl Upstream for Fixed {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, OpError> {
        self.sent.lock().unwrap().push(request);
        let status = *self.status.lock().unwrap();
        if status == 0 {
            return Err(OpError::network("connection refused"));
        }
        Ok(UpstreamResponse {
            status,
            headers: vec![],
            body: self.body.lock().unwrap().clone().into_bytes(),
        })
    }
}

fn host(upstream: Arc<Fixed>) -> (FactoryHost, Arc<MemorySecrets>) {
    let secrets = Arc::new(MemorySecrets::default());
    let host = FactoryHost::new(ConfigStore::open(None), secrets.clone(), upstream);
    (host, secrets)
}

fn configure(app: &str, url: &str, fields: &[(&str, &str)]) -> FactoryConfigureParams {
    FactoryConfigureParams {
        app: app.into(),
        url: url.into(),
        fields: fields
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[tokio::test]
async fn lists_eight_apps_unconfigured() {
    let (host, _) = host(Fixed::new(200, "{}"));
    let list = host.list().await;
    assert_eq!(list.apps.len(), 8);
    assert!(
        list.apps
            .iter()
            .all(|a| a.health == FactoryHealth::Unconfigured)
    );
    let plane = list.apps.iter().find(|a| a.id == "plane").unwrap();
    assert_eq!(plane.label, "Plane (ItsAPlan)");
    assert!(
        plane
            .credential_fields
            .iter()
            .any(|f| f.key == "appUrl" && !f.secret)
    );
    let signoz = list.apps.iter().find(|a| a.id == "signoz").unwrap();
    assert_eq!(signoz.label, "SigNoz");
    assert!(!signoz.has_panels);
    // SigNoz on the House is a preview until it passes the House gate (D1571, D1666).
    assert_eq!(
        signoz.preview.as_deref(),
        Some("Preview: SigNoz on Loams House")
    );
    assert_eq!(list.apps.iter().filter(|a| a.preview.is_some()).count(), 1);
}

#[tokio::test]
async fn configure_tests_and_never_replies_a_secret() {
    let up = Fixed::new(200, r#"{"version":"16.0.5"}"#);
    let (host, secrets) = host(up.clone());
    let reply = host
        .configure(&configure(
            "forgejo",
            "https://f.example",
            &[("token", SECRET)],
        ))
        .await;
    assert!(reply.error.is_none());
    let app = reply.app.unwrap();
    assert_eq!(app.health, FactoryHealth::Ok);
    assert_eq!(app.url.as_deref(), Some("https://f.example"));
    assert!(!serde_json::to_string(&app).unwrap().contains("S3CRET"));
    assert_eq!(
        secrets.get("factory:forgejo:token").unwrap().as_deref(),
        Some(SECRET)
    );
    // Studio's pull requests read Forgejo's URL beside its token.
    assert_eq!(
        secrets.get("factory:forgejo:url").unwrap().as_deref(),
        Some("https://f.example")
    );
    let sent = up.sent.lock().unwrap();
    assert_eq!(sent[0].url, "https://f.example/api/v1/version");
    assert!(
        sent[0]
            .headers
            .contains(&("authorization".into(), format!("token {SECRET}")))
    );
}

#[tokio::test]
async fn health_maps_status_codes() {
    let up = Fixed::new(200, r#"{"version":"x"}"#);
    let (host, _) = host(up.clone());
    for (status, health) in [
        (401, FactoryHealth::AuthFailed),
        (403, FactoryHealth::AuthFailed),
        (500, FactoryHealth::Unreachable),
        (0, FactoryHealth::Unreachable),
        (200, FactoryHealth::Ok),
    ] {
        up.answer(status, r#"{"version":"x"}"#);
        let reply = host
            .configure(&configure(
                "forgejo",
                "https://f.example",
                &[("token", "t")],
            ))
            .await;
        assert_eq!(reply.app.unwrap().health, health, "status {status}");
    }
}

#[tokio::test]
async fn configure_validates_urls_and_fields() {
    let (host, _) = host(Fixed::new(200, "{}"));
    let r = host
        .configure(&configure("forgejo", "ftp://x", &[("token", "t")]))
        .await;
    assert_eq!(r.error.unwrap().code, "invalid_url");
    let r = host
        .configure(&configure("forgejo", "https://u:p@x", &[("token", "t")]))
        .await;
    assert_eq!(r.error.unwrap().code, "invalid_url");
    let r = host
        .configure(&configure("forgejo", "https://x", &[]))
        .await;
    assert_eq!(r.error.unwrap().message, "Access token is required");
    let r = host
        .configure(&configure(
            "plane",
            "https://api.x",
            &[("apiKey", "k"), ("appUrl", "javascript:alert(1)")],
        ))
        .await;
    assert_eq!(r.error.unwrap().code, "invalid_url");
    let r = host.configure(&configure("nope", "https://x", &[])).await;
    assert_eq!(r.error.unwrap().code, "unknown_app");
}

#[tokio::test]
async fn reconfigure_keeps_secrets_on_the_same_origin_only() {
    let (host, _) = host(Fixed::new(200, r#"{"version":"x"}"#));
    host.configure(&configure(
        "forgejo",
        "https://f.example",
        &[("token", "t")],
    ))
    .await;
    let same = host
        .configure(&configure("forgejo", "https://f.example/sub", &[]))
        .await;
    assert!(same.error.is_none());
    let other = host
        .configure(&configure("forgejo", "https://g.example", &[]))
        .await;
    let e = other.error.unwrap();
    assert_eq!(e.code, "bad_fields");
    assert!(e.message.contains("different origin"));
}

#[tokio::test]
async fn query_checks_app_op_params_and_configuration() {
    let (host, _) = host(Fixed::new(200, "[]"));
    let q = |app: &str, op: &str, params| FactoryQueryParams {
        app: app.into(),
        op: op.into(),
        params,
    };
    assert_eq!(
        host.query(&q("nope", "repos", json!({})))
            .await
            .error
            .unwrap()
            .code,
        "unknown_app"
    );
    assert_eq!(
        host.query(&q("forgejo", "deleteRepo", json!({})))
            .await
            .error
            .unwrap()
            .code,
        "unknown_op"
    );
    assert_eq!(
        host.query(&q("forgejo", "repos", json!({"evil": 1})))
            .await
            .error
            .unwrap()
            .code,
        "bad_params"
    );
    assert_eq!(
        host.query(&q("forgejo", "repos", json!({})))
            .await
            .error
            .unwrap()
            .code,
        "unconfigured"
    );
}

#[tokio::test]
async fn query_errors_are_scrubbed_of_every_secret_form() {
    let up = Fixed::new(200, r#"{"version":"x"}"#);
    let (host, _) = host(up.clone());
    host.configure(&configure(
        "zulip",
        "https://z.example",
        &[("email", "bot@example.com"), ("apiKey", SECRET)],
    ))
    .await;
    let basic = base64_basic("bot@example.com", SECRET);
    up.answer(500, &format!("boom {SECRET} {basic} tok-S3CRET%2F%2Bx"));
    let reply = host
        .query(&FactoryQueryParams {
            app: "zulip".into(),
            op: "streams".into(),
            params: json!({}),
        })
        .await;
    let e = reply.error.unwrap();
    assert_eq!(e.code, "upstream_error");
    assert!(!e.message.contains("S3CRET"), "{}", e.message);
    assert!(!e.message.contains(&basic), "{}", e.message);
    assert!(e.message.contains("[redacted]"));
}

#[tokio::test]
async fn remove_forgets_the_app_and_its_secrets() {
    let (host, secrets) = host(Fixed::new(200, r#"{"version":"x"}"#));
    host.configure(&configure(
        "forgejo",
        "https://f.example",
        &[("token", SECRET)],
    ))
    .await;
    host.remove("forgejo").unwrap();
    assert!(secrets.get("factory:forgejo:token").unwrap().is_none());
    assert!(secrets.get("factory:forgejo:url").unwrap().is_none());
    let list = host.list().await;
    assert_eq!(list.apps[0].health, FactoryHealth::Unconfigured);
}

#[tokio::test]
async fn a_secret_the_store_lost_is_locked() {
    let (host, secrets) = host(Fixed::new(200, r#"{"version":"x"}"#));
    host.configure(&configure(
        "forgejo",
        "https://f.example",
        &[("token", SECRET)],
    ))
    .await;
    secrets.delete("factory:forgejo:token").unwrap();
    let list = host.list().await;
    assert_eq!(list.apps[0].health, FactoryHealth::Locked);
}

#[tokio::test]
async fn signoz_health_is_its_healthz() {
    let up = Fixed::new(200, r#"{"status":"ok"}"#);
    let (host, _) = host(up.clone());
    let r = host
        .configure(&configure(
            "signoz",
            "http://signoz.example:8080/services/",
            &[],
        ))
        .await;
    assert_eq!(r.app.unwrap().health, FactoryHealth::Ok);
    assert_eq!(
        up.sent.lock().unwrap()[0].url,
        "http://signoz.example:8080/api/v2/healthz"
    );
    up.answer(502, "bad gateway");
    let r = host
        .configure(&configure(
            "signoz",
            "http://signoz.example:8080/services/",
            &[],
        ))
        .await;
    assert_eq!(r.app.unwrap().health, FactoryHealth::Unreachable);
}

fn base64_basic(user: &str, pass: &str) -> String {
    use std::fmt::Write as _;
    // A tiny standard base64 for the test, so it shares no code with the redactor.
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{user}:{pass}").into_bytes();
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let _ = write!(out, "{}", T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
