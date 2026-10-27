//! One conformance suite, run against every provider.
//!
//! This is the point of the abstraction: the same assertions hold whichever
//! provider a deployment selects — Obscura launched locally, Obscura as the
//! deployment's `browser` service, or a host's own page driver — and none can
//! drift from the others without a test failing (D2112).

use std::collections::HashMap;
use std::sync::Arc;

use loams_web_bridge::testing::{FakeCdp, FakeDriver, login_page, obscura_answers, obscura_over};
use loams_web_bridge::{
    BrowserProvider, MapSecretResolver, OpenRequest, PageRef, ProviderKind, SecretRef,
    SnapshotRequest, WaitCondition, WaitRequest, WebBridgeConfig, local_provider,
};

const URL: &str = "https://example.com/login";

fn url(raw: &str) -> url::Url {
    url::Url::parse(raw).unwrap_or_else(|_| panic!("{raw} parses"))
}

fn resolver() -> Arc<MapSecretResolver> {
    Arc::new(MapSecretResolver::new().with(
        SecretRef::parse("env:browser-token").unwrap_or_else(|_| panic!("ref")),
        "canary-token-4f8b2c19d7e3",
    ))
}

/// The Obscura provider of `kind` over a scripted engine.
fn obscura(kind: ProviderKind) -> Box<dyn BrowserProvider> {
    let config = WebBridgeConfig {
        provider: kind,
        ..WebBridgeConfig::default()
    };
    Box::new(obscura_over(
        kind,
        &config,
        Arc::new(FakeCdp::new(obscura_answers(URL)).with_events()),
        resolver(),
    ))
}

/// The host-driver seam over the deterministic driver.
fn local() -> Box<dyn BrowserProvider> {
    let driver = Arc::new(FakeDriver::new(login_page(URL)));
    let mut config = WebBridgeConfig {
        provider: ProviderKind::Local,
        ..WebBridgeConfig::default()
    };
    config.artifact_dir = None;
    local_provider(&config, driver, resolver()).unwrap_or_else(|error| panic!("{error}"))
}

async fn contract(provider: &dyn BrowserProvider) {
    // A handle names its provider.
    let page = provider
        .open(OpenRequest::at(url(URL)))
        .await
        .unwrap_or_else(|error| panic!("{} open: {error}", provider.kind().as_str()));
    assert_eq!(page.provider(), provider.kind());

    // A snapshot is text with uids, and it carries the page's shape.
    let snapshot = provider
        .snapshot(&page, &SnapshotRequest::new())
        .await
        .unwrap_or_else(|error| panic!("snapshot: {error}"));
    assert!(!snapshot.nodes.is_empty(), "a snapshot has nodes");
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.uid.as_str().starts_with('s')),
        "every uid names its snapshot"
    );
    let rendered = snapshot.render(SnapshotRequest::new().budget());
    assert!(!rendered.contains("hunter2"), "a password never renders");

    // A uid from an older snapshot is a self-healing error, whichever
    // provider minted it.
    let first_uid = snapshot.nodes[1].uid.clone();
    provider
        .snapshot(&page, &SnapshotRequest::new())
        .await
        .unwrap_or_else(|error| panic!("a second snapshot: {error}"));
    let error = provider
        .click(&page, &first_uid)
        .await
        .err()
        .unwrap_or_else(|| panic!("a stale uid is refused"));
    assert!(error.to_string().contains("take_snapshot"), "{error}");

    // find answers with matches, not with the page.
    let found = provider
        .find(&page, &loams_web_bridge::FindQuery::text("Sign in"))
        .await
        .unwrap_or_else(|error| panic!("find: {error}"));
    assert!(!found.is_empty(), "find returns matches");

    // An action answers in one line plus a change summary.
    let current = provider
        .snapshot(&page, &SnapshotRequest::new())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let uid = current.nodes[1].uid.clone();
    let outcome = provider
        .click(&page, &uid)
        .await
        .unwrap_or_else(|error| panic!("click: {error}"));
    assert!(outcome.message.contains("clicked"), "{}", outcome.message);
    assert!(!outcome.render().is_empty());

    // A wait for text that is on the page succeeds, and one that never will
    // does not lie about having succeeded.
    let outcome = provider
        .wait_for(
            &page,
            &WaitRequest::new(WaitCondition::Text("Sign in".to_string())).with_timeout_ms(50),
        )
        .await
        .unwrap_or_else(|error| panic!("wait_for: {error}"));
    assert!(outcome.matched, "{outcome:?}");
    let missed = provider
        .wait_for(
            &page,
            &WaitRequest::new(WaitCondition::Text("never appears".to_string())).with_timeout_ms(1),
        )
        .await
        .unwrap_or_else(|error| panic!("wait_for: {error}"));
    assert!(!missed.matched);
    assert!(missed.note.contains("take_snapshot"), "{missed:?}");

    // Extraction answers with the page's text.
    let extracted = provider
        .extract(&page)
        .await
        .unwrap_or_else(|error| panic!("extract: {error}"));
    assert!(!extracted.title.is_empty(), "a title");

    // A private address is refused by both providers, in the same way.
    for blocked in [
        "http://127.0.0.1/",
        "http://169.254.169.254/latest/meta-data/",
        "file:///etc/passwd",
    ] {
        let error = provider
            .navigate(&page, &url(blocked))
            .await
            .err()
            .unwrap_or_else(|| panic!("{blocked} is refused"));
        assert!(
            matches!(error, loams_web_bridge::BridgeError::EgressDenied(_)),
            "{blocked}: {error:?}"
        );
    }

    // A closed handle is an error, and closing twice is not.
    provider
        .close(&page)
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
    provider
        .close(&page)
        .await
        .unwrap_or_else(|error| panic!("closing twice is quiet: {error}"));
    assert!(
        provider
            .snapshot(&page, &SnapshotRequest::new())
            .await
            .is_err(),
        "a closed handle cannot be snapshotted"
    );
}

#[tokio::test]
async fn the_host_driver_seam_satisfies_the_contract() {
    contract(local().as_ref()).await;
}

#[tokio::test]
async fn obscura_launched_locally_satisfies_the_contract() {
    contract(obscura(ProviderKind::Local).as_ref()).await;
}

#[tokio::test]
async fn obscura_as_the_browser_service_satisfies_the_same_contract() {
    contract(obscura(ProviderKind::Service).as_ref()).await;
}

#[tokio::test]
async fn local_and_service_differ_only_in_where_the_credentials_live() {
    let local = obscura(ProviderKind::Local);
    let service = obscura(ProviderKind::Service);

    assert_eq!(local.engine(), Some(loams_web_bridge::Engine::Obscura));
    assert_eq!(service.engine(), Some(loams_web_bridge::Engine::Obscura));

    let local_caps = local.capabilities();
    let service_caps = service.capabilities();
    assert!(local_caps.credentials_stay_local);
    assert!(!service_caps.credentials_stay_local);
    assert_eq!(
        loams_web_bridge::Capabilities {
            credentials_stay_local: false,
            ..local_caps
        },
        service_caps,
        "everything else is the same"
    );

    // Both are swappable behind the one trait.
    let providers: Vec<Arc<dyn BrowserProvider>> = vec![Arc::from(local), Arc::from(service)];
    for provider in providers {
        assert!(provider.capabilities().core);
    }
}

#[tokio::test]
async fn a_handle_is_opaque_and_says_only_what_it_must() {
    let page = PageRef::new(ProviderKind::Service, "service-1");
    let rendered = format!("{page:?}");
    assert!(rendered.contains("service-1"));
    assert!(!rendered.contains("session"), "{rendered}");
    assert!(!rendered.contains("token"), "{rendered}");
}

#[tokio::test]
async fn both_kinds_honour_an_allow_list_they_were_given() {
    for kind in [ProviderKind::Local, ProviderKind::Service] {
        let mut config = WebBridgeConfig {
            provider: kind,
            ..WebBridgeConfig::default()
        };
        config.allowed_domains = vec!["*.example.com".to_string()];
        let provider = obscura_over(
            kind,
            &config,
            Arc::new(FakeCdp::new(obscura_answers(
                "https://app.example.com/login",
            ))),
            resolver(),
        );
        let page = provider
            .open(OpenRequest::at(url("https://app.example.com/login")))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let error = provider
            .navigate(&page, &url("https://elsewhere.test/"))
            .await
            .err()
            .unwrap_or_else(|| panic!("off the allow list"));
        assert!(error.to_string().contains("allow list"), "{error}");
    }
}
