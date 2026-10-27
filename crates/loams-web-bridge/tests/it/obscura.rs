//! What the Obscura provider does on the wire, the same for `local` and
//! `service` (D2111, D2112): a browser context per page, a bearer token on
//! every connection, request interception under the egress policy, a redirect
//! refused after the fact, budgets, and the action log.

use std::sync::Arc;
use std::time::Duration;

use loams_web_bridge::cdp::CdpEvent;
use loams_web_bridge::testing::{FakeCdp, FakeTransport, obscura_answers, read_page};
use loams_web_bridge::{
    BridgeError, BrowserProvider, MapSecretResolver, ObscuraProvider, OpenRequest, ProviderKind,
    SecretRef, ServiceEndpoint, WebBridgeConfig,
};
use serde_json::json;

const URL: &str = "https://example.com/login";
const TOKEN: &str = "canary-service-token-0f1e2d3c4b5a69788796a5b4c3d2e1f0";

fn url(raw: &str) -> url::Url {
    url::Url::parse(raw).unwrap_or_else(|_| panic!("{raw} parses"))
}

fn resolver() -> Arc<MapSecretResolver> {
    Arc::new(MapSecretResolver::new().with(
        SecretRef::parse("env:browser-token").unwrap_or_else(|_| panic!("ref")),
        TOKEN,
    ))
}

fn service_config() -> WebBridgeConfig {
    let mut config = WebBridgeConfig {
        provider: ProviderKind::Service,
        ..WebBridgeConfig::default()
    };
    config.service.token_secret_ref = Some("env:browser-token".to_string());
    config
}

fn service(config: &WebBridgeConfig, cdp: Arc<FakeCdp>) -> (Arc<FakeTransport>, ObscuraProvider) {
    let transport = Arc::new(FakeTransport::new(cdp));
    let endpoint =
        ServiceEndpoint::new(&config.service, resolver()).unwrap_or_else(|error| panic!("{error}"));
    let provider = ObscuraProvider::new(
        Arc::new(endpoint),
        loams_web_bridge::policies(config).unwrap_or_else(|error| panic!("{error}")),
        resolver(),
        transport.clone(),
    );
    (transport, provider)
}

async fn until(cdp: &FakeCdp, method: &str, count: usize) -> bool {
    for _ in 0..200 {
        let seen = cdp
            .calls()
            .iter()
            .filter(|call| call.starts_with(method))
            .count();
        if seen >= count {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

#[tokio::test]
async fn each_page_is_its_own_browser_context_and_closing_never_closes_the_browser() {
    let cdp = Arc::new(FakeCdp::new(obscura_answers(URL)));
    let (transport, provider) = service(&service_config(), cdp.clone());
    let page = provider
        .open(OpenRequest::at(url(URL)))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    provider
        .close(&page)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let calls = cdp.calls();
    let position = |method: &str| {
        calls
            .iter()
            .position(|call| call.starts_with(method))
            .unwrap_or_else(|| panic!("{method} in {calls:?}"))
    };
    assert!(position("Target.createBrowserContext") < position("Target.createTarget"));
    assert!(position("Target.closeTarget") < position("Target.disposeBrowserContext"));
    assert!(
        !calls.iter().any(|call| call.starts_with("Browser.close")),
        "a shared engine is never closed by one page: {calls:?}"
    );

    // The engine's token rides in the handshake, and nowhere it could be read.
    let handshakes = transport.handshakes();
    assert_eq!(handshakes.len(), 1);
    let (endpoint, headers) = &handshakes[0];
    assert_eq!(endpoint, "ws://browser:9222/devtools/browser");
    assert_eq!(
        headers.get("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(!format!("{headers:?}").contains(TOKEN));
    assert!(!format!("{provider:?}").contains(TOKEN));
}

#[tokio::test]
async fn every_request_the_page_makes_is_held_to_the_egress_policy() {
    let cdp = Arc::new(FakeCdp::new(obscura_answers(URL)).with_events());
    let (_transport, provider) = service(&service_config(), cdp.clone());
    let _page = provider
        .open(OpenRequest::at(url(URL)))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        cdp.calls()
            .iter()
            .any(|call| call.starts_with("Fetch.enable")),
        "interception is on"
    );

    for (id, target) in [
        ("r1", "https://cdn.example.com/app.js"),
        ("r2", "http://169.254.169.254/latest/meta-data/"),
        ("r3", "data:image/png;base64,AAAA"),
        ("r4", "http://10.1.2.3/internal"),
    ] {
        assert!(cdp.emit(CdpEvent {
            method: "Fetch.requestPaused".to_string(),
            params: json!({ "requestId": id, "request": { "url": target } }),
            session_id: Some("s1".to_string()),
        }));
    }
    assert!(
        until(&cdp, "Fetch.continueRequest", 2).await,
        "{:?}",
        cdp.calls()
    );
    assert!(
        until(&cdp, "Fetch.failRequest", 2).await,
        "{:?}",
        cdp.calls()
    );
    let calls = cdp.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("Fetch.continueRequest"))
            .count(),
        2,
        "the public script and the in-memory image: {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("Fetch.failRequest"))
            .count(),
        2,
        "the metadata endpoint and the private address: {calls:?}"
    );
}

#[tokio::test]
async fn a_redirect_to_a_refused_host_is_refused_after_the_fact() {
    let mut answers = obscura_answers(URL);
    // The engine reports that the page ended up on a private address.
    answers.insert(
        "Runtime.evaluate".to_string(),
        read_page("http://10.0.0.5/admin", "Admin", "secret dashboard"),
    );
    let cdp = Arc::new(FakeCdp::new(answers));
    let (_transport, provider) = service(&service_config(), cdp.clone());
    let page = provider
        .open(OpenRequest::blank())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let error = provider
        .navigate(&page, &url("https://example.com/redirects"))
        .await
        .err()
        .unwrap_or_else(|| panic!("the landing page is refused"));
    assert!(matches!(error, BridgeError::EgressDenied(_)), "{error:?}");
    assert_eq!(
        cdp.calls()
            .iter()
            .filter(|call| call.starts_with("Page.navigate"))
            .count(),
        2,
        "the page is sent to about:blank"
    );
}

#[tokio::test]
async fn the_concurrency_budget_is_the_same_on_both_kinds() {
    let mut config = service_config();
    config.budgets.max_concurrent_sessions = 1;
    let cdp = Arc::new(FakeCdp::new(obscura_answers(URL)));
    let (_transport, provider) = service(&config, cdp);
    let first = provider
        .open(OpenRequest::blank())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let error = provider
        .open(OpenRequest::blank())
        .await
        .err()
        .unwrap_or_else(|| panic!("one page at a time"));
    assert!(
        matches!(error, BridgeError::SessionLimit { max: 1 }),
        "{error:?}"
    );
    provider
        .close(&first)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    provider
        .open(OpenRequest::blank())
        .await
        .unwrap_or_else(|error| panic!("a closed page frees its slot: {error}"));
}

#[tokio::test]
async fn recording_writes_an_action_log_without_the_values_typed() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut config = service_config();
    config.recording.enabled = true;
    config.artifact_dir = Some(dir.path().to_path_buf());
    let cdp = Arc::new(FakeCdp::new(obscura_answers(URL)));
    let (_transport, provider) = service(&config, cdp);
    let page = provider
        .open(OpenRequest::at(url(URL)))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let snapshot = provider
        .snapshot(&page, &loams_web_bridge::SnapshotRequest::new())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let uid = snapshot.nodes[1].uid.clone();
    let _ = provider
        .fill(
            &page,
            &uid,
            &loams_web_bridge::FillValue::Literal("typed-canary-51c2".to_string()),
        )
        .await;
    provider
        .close(&page)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let log = dir.path().join(format!("{}.actions.jsonl", page.id()));
    let written = std::fs::read_to_string(&log).unwrap_or_else(|error| panic!("{log:?}: {error}"));
    let tools: Vec<String> = written
        .lines()
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}"));
            value["tool"].as_str().unwrap_or_default().to_string()
        })
        .collect();
    assert_eq!(
        tools,
        ["navigate", "take_snapshot", "fill", "close"],
        "{written}"
    );
    assert!(!written.contains("typed-canary-51c2"), "{written}");
}

#[tokio::test]
async fn recording_is_off_by_default() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut config = service_config();
    config.artifact_dir = Some(dir.path().to_path_buf());
    let (_transport, provider) = service(&config, Arc::new(FakeCdp::new(obscura_answers(URL))));
    let page = provider
        .open(OpenRequest::at(url(URL)))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    provider
        .close(&page)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let entries = std::fs::read_dir(dir.path())
        .unwrap_or_else(|error| panic!("{error}"))
        .count();
    assert_eq!(entries, 0, "nothing is written unless recording is on");
}

#[tokio::test]
async fn a_secret_fill_is_refused_by_default_on_both_kinds() {
    for kind in [ProviderKind::Local, ProviderKind::Service] {
        let config = WebBridgeConfig {
            provider: kind,
            ..WebBridgeConfig::default()
        };
        let provider = loams_web_bridge::testing::obscura_over(
            kind,
            &config,
            Arc::new(FakeCdp::new(obscura_answers(URL))),
            resolver(),
        );
        let page = provider
            .open(OpenRequest::at(url(URL)))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let snapshot = provider
            .snapshot(&page, &loams_web_bridge::SnapshotRequest::new())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let uid = snapshot.nodes[1].uid.clone();
        let reference = SecretRef::parse("env:browser-token").unwrap_or_else(|_| panic!("ref"));
        let error = provider
            .fill(&page, &uid, &loams_web_bridge::FillValue::Secret(reference))
            .await
            .err()
            .unwrap_or_else(|| panic!("{} refuses a secret fill", kind.as_str()));
        assert!(matches!(error, BridgeError::Policy(_)), "{error:?}");
    }
}
