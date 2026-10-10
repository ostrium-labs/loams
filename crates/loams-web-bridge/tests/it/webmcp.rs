//! The WebMCP conformance suite (AP1d Task 3; design §42 §5, D568, D570, D635).
//!
//! The point of these tests is the **absent** case, so they are written to run
//! the same contract against both providers *and* against a page with no
//! `document.modelContext` — which is what Safari is, permanently, since
//! WebKit's standards position on WebMCP is closed and `oppose`. A bridge that
//! only works when the draft shipped would be a bridge nobody can rely on.

use std::collections::HashMap;
use std::sync::Arc;

use loams_web_bridge::testing::{
    FakeCdp, FakeDriver, FakeModelContext, FakePage, FakeTransport, FakeWebmcpTool,
    ax_tree_with_button, evaluate_json, login_page, read_page,
};
use loams_web_bridge::{
    Availability, BridgeError, BrowserProvider, BrowserRunProvider, CallWebmcpToolRequest,
    DriverAction, DriverPage, ListWebmcpToolsRequest, MapSecretResolver, OpenRequest, PageDriver,
    PageRef, PageState, ProviderKind, SecretRef, WebBridgeConfig, WebmcpAbsent, WebmcpToolName,
    local_provider,
};
use serde_json::{Value, json};

const URL: &str = "https://example.com/login";

fn url(raw: &str) -> url::Url {
    url::Url::parse(raw).unwrap_or_else(|_| panic!("{raw} parses"))
}

fn resolver() -> Arc<MapSecretResolver> {
    Arc::new(MapSecretResolver::new().with(
        SecretRef::parse("env:cloudflare#api_token").unwrap_or_else(|_| panic!("ref")),
        "canary-token-4f8b2c19d7e3",
    ))
}

/// The tools a page registers for these tests: one read-only search, one
/// mutating action, and one that answers with far too much text.
fn tools() -> Vec<FakeWebmcpTool> {
    vec![
        FakeWebmcpTool::new("find_items")
            .titled("Find items")
            .described("Search the catalogue")
            .requiring(&["query"])
            .read_only(),
        FakeWebmcpTool::new("add_to_cart")
            .titled("Add to cart")
            .described("Put the sku in the basket"),
        FakeWebmcpTool::new("dump_report").described("Everything, at length"),
    ]
}

/// A context with the three tools, answers, and a call that changes the page.
fn webmcp_present() -> FakeModelContext {
    let mut context = FakeModelContext::with_tools(tools()).answering("find_items", "3 items");
    context = context
        .answering("add_to_cart", "added sku-1")
        .answering("dump_report", &"y".repeat(50_000));
    context.mutates = true;
    context
}

fn page_with(context: FakeModelContext) -> FakePage {
    FakePage {
        webmcp: context,
        ..login_page(URL)
    }
}

fn local_over(page: FakePage) -> (Arc<FakeDriver>, Box<dyn BrowserProvider>) {
    let driver = Arc::new(FakeDriver::new(page));
    let mut config = WebBridgeConfig {
        provider: ProviderKind::Local,
        ..WebBridgeConfig::default()
    };
    config.local.artifact_dir = None;
    let provider = local_provider(
        &config,
        Arc::clone(&driver) as Arc<dyn PageDriver>,
        resolver(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    (driver, provider)
}

fn remote_with(expressions: Vec<(&str, Value)>) -> (Arc<FakeCdp>, Box<dyn BrowserProvider>) {
    let mut config = WebBridgeConfig {
        provider: ProviderKind::BrowserRun,
        ..WebBridgeConfig::default()
    };
    config.browser_run.account_id = "0123456789abcdef0123456789abcdef".to_string();
    config.browser_run.token_secret_ref = Some("env:cloudflare#api_token".to_string());
    config.browser_run.plan = loams_web_bridge::Plan::Paid;
    config.browser_run.min_session_interval_ms = Some(0);
    let mut answers: HashMap<String, Value> = HashMap::new();
    answers.insert(
        "Target.createTarget".to_string(),
        json!({ "targetId": "t1" }),
    );
    answers.insert(
        "Target.attachToTarget".to_string(),
        json!({ "sessionId": "s1" }),
    );
    answers.insert(
        "Accessibility.getFullAXTree".to_string(),
        ax_tree_with_button("Sign in", 42),
    );
    answers.insert(
        "Runtime.evaluate".to_string(),
        read_page(URL, "Sign in", "Sign in"),
    );
    let mut scripted = FakeCdp::new(answers);
    for (needle, answer) in expressions {
        scripted = scripted.with_expression(needle, answer);
    }
    let connection = Arc::new(scripted);
    let provider = BrowserRunProvider::new(
        config.browser_run.clone(),
        config.egress().unwrap_or_else(|error| panic!("{error}")),
        config.credentials.clone(),
        resolver(),
        Arc::new(FakeTransport::new(connection.clone())),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    (connection, Box::new(provider))
}

/// The two WebMCP scripts' answers, wired by the marker each script carries.
///
/// The per-name answers come first because the fake matches needles in order,
/// and the tool name is in the call script as a JSON string literal. That is
/// what lets one fixture answer "no such tool" for `remove_item` and a real
/// answer for `add_to_cart`, exactly as the page would.
fn remote_present() -> (Arc<FakeCdp>, Box<dyn BrowserProvider>) {
    let context = webmcp_present();
    remote_with(vec![
        (
            "\"remove_item\"",
            evaluate_json(context.call_envelope("remove_item")),
        ),
        (
            "\"dump_report\"",
            evaluate_json(context.call_envelope("dump_report")),
        ),
        (
            "loamsWebmcpCall",
            evaluate_json(context.call_envelope("add_to_cart")),
        ),
        (
            "loamsWebmcpList",
            evaluate_json(context.listing_envelope(None)),
        ),
    ])
}

async fn open(provider: &dyn BrowserProvider) -> PageRef {
    provider
        .open(OpenRequest::at(url(URL)))
        .await
        .unwrap_or_else(|error| panic!("{} open: {error}", provider.kind().as_str()))
}

/// What both providers must do with a page that has the tools registered.
async fn webmcp_contract(provider: &dyn BrowserProvider) {
    let page = open(provider).await;

    // list_webmcp_tools enumerates what the page registered, with the parts an
    // agent needs to build a call: the name, the description, the hints and the
    // required inputs.
    let listing = provider
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .unwrap_or_else(|error| panic!("list_webmcp_tools: {error}"));
    assert_eq!(listing.availability, Availability::Available);
    let names: Vec<&str> = listing
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["find_items", "add_to_cart", "dump_report"],
        "{names:?}"
    );
    assert_eq!(listing.tools[0].required_inputs(), ["query"]);
    assert!(listing.tools[0].annotations.read_only);
    let rendered = listing.render();
    assert!(rendered.contains("Find items"), "{rendered}");
    assert!(rendered.contains("requires=query"), "{rendered}");

    // A filter narrows, so a large catalogue does not have to be read whole.
    let narrowed = provider
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new().with_filter("CART"))
        .await
        .unwrap_or_else(|error| panic!("a filtered listing: {error}"));
    assert_eq!(narrowed.tools.len(), 1);
    assert_eq!(narrowed.tools[0].name.as_str(), "add_to_cart");

    // A call runs the tool and answers in one line plus a change summary.
    let request = CallWebmcpToolRequest::new("add_to_cart", json!({ "sku": "sku-1" }))
        .unwrap_or_else(|error| panic!("{error}"));
    let outcome = provider
        .call_webmcp_tool(&page, &request)
        .await
        .unwrap_or_else(|error| panic!("call_webmcp_tool: {error}"));
    assert!(outcome.executed, "{outcome:?}");
    assert_eq!(outcome.result.as_deref(), Some("added sku-1"));
    let rendered = outcome.render();
    assert!(
        rendered.starts_with("called add_to_cart via webmcp ("),
        "{rendered}"
    );
    assert!(rendered.contains("\n\nadded sku-1"), "{rendered}");

    // An invalid name never reaches the page: it is refused by the contract
    // itself, and the message names the way on.
    for name in ["", "add to cart", "add/to/cart"] {
        let error = CallWebmcpToolRequest::new(name, json!({}))
            .err()
            .unwrap_or_else(|| panic!("{name:?} is refused"));
        assert!(
            matches!(error, BridgeError::Policy(_)),
            "{name:?}: {error:?}"
        );
        assert!(error.to_string().contains("list_webmcp_tools"), "{error}");
    }

    // A name the page does not have comes back as "not registered", never as a
    // tool that ran and said nothing.
    let unknown = CallWebmcpToolRequest::bare("remove_item").unwrap_or_else(|e| panic!("{e}"));
    let outcome = provider
        .call_webmcp_tool(&page, &unknown)
        .await
        .unwrap_or_else(|error| panic!("a call to an unknown tool: {error}"));
    assert!(!outcome.executed, "{outcome:?}");
    assert!(
        outcome.available.contains(&"add_to_cart".to_string()),
        "{outcome:?}"
    );
    let rendered = outcome.render();
    assert!(rendered.contains("not registered"), "{rendered}");
    assert!(rendered.contains("list_webmcp_tools"), "{rendered}");

    // A large return value is cut at the budget, with the way to continue named.
    let big = CallWebmcpToolRequest::bare("dump_report")
        .unwrap_or_else(|e| panic!("{e}"))
        .with_budget_chars(200);
    let outcome = provider
        .call_webmcp_tool(&page, &big)
        .await
        .unwrap_or_else(|error| panic!("a large answer: {error}"));
    assert!(outcome.truncated, "{outcome:?}");
    assert_eq!(outcome.result.as_ref().map(String::len), Some(200));
    assert!(
        outcome.render().contains("narrow the tool's input"),
        "{}",
        outcome.render()
    );

    // The rest of the toolbox is untouched by any of it.
    let snapshot = provider
        .snapshot(&page, &loams_web_bridge::SnapshotRequest::new())
        .await
        .unwrap_or_else(|error| panic!("take_snapshot: {error}"));
    assert!(!snapshot.nodes.is_empty());
}

/// What both providers must do with a page that has no WebMCP surface.
async fn webmcp_absent_contract(provider: &dyn BrowserProvider, expected: WebmcpAbsent) {
    let page = open(provider).await;

    let listing = provider
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .unwrap_or_else(|error| panic!("an absent listing must not fail: {error}"));
    assert_eq!(listing.availability, Availability::Absent(expected.clone()));
    assert!(listing.tools.is_empty());
    let rendered = listing.render();
    assert!(rendered.contains("no WebMCP tools"), "{rendered}");
    assert!(
        rendered.contains("take_snapshot"),
        "the answer must name the way on: {rendered}"
    );

    let request = CallWebmcpToolRequest::bare("add_to_cart").unwrap_or_else(|e| panic!("{e}"));
    let outcome = provider
        .call_webmcp_tool(&page, &request)
        .await
        .unwrap_or_else(|error| panic!("an absent call must not fail: {error}"));
    assert!(!outcome.executed, "{outcome:?}");
    assert_eq!(outcome.availability, Availability::Absent(expected));
    let rendered = outcome.render();
    assert!(rendered.contains("was not called"), "{rendered}");
    assert!(rendered.contains("take_snapshot"), "{rendered}");

    // A snapshot still works: the toolbox is not a WebMCP toolbox.
    let snapshot = provider
        .snapshot(&page, &loams_web_bridge::SnapshotRequest::new())
        .await
        .unwrap_or_else(|error| panic!("take_snapshot: {error}"));
    assert!(!snapshot.nodes.is_empty(), "the v1 tools still answer");
    provider
        .close(&page)
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
}

#[tokio::test]
async fn the_local_provider_satisfies_the_webmcp_contract() {
    let (_driver, provider) = local_over(page_with(webmcp_present()));
    webmcp_contract(provider.as_ref()).await;
}

#[tokio::test]
async fn the_remote_provider_satisfies_the_same_webmcp_contract() {
    let (_cdp, provider) = remote_present();
    webmcp_contract(provider.as_ref()).await;
}

#[tokio::test]
async fn a_webkit_page_gets_an_answer_not_an_error_from_both_providers() {
    // Safari, and every WebKit engine: no document.modelContext, ever (D635).
    let (_driver, local) = local_over(page_with(FakeModelContext::absent()));
    webmcp_absent_contract(local.as_ref(), WebmcpAbsent::NotExposed).await;

    let (_cdp, remote) = remote_with(vec![
        (
            "loamsWebmcpCall",
            evaluate_json(loams_web_bridge::webmcp::envelope_absent("not-exposed")),
        ),
        (
            "loamsWebmcpList",
            evaluate_json(loams_web_bridge::webmcp::envelope_absent("not-exposed")),
        ),
    ]);
    webmcp_absent_contract(remote.as_ref(), WebmcpAbsent::NotExposed).await;
}

#[tokio::test]
async fn a_call_against_an_absent_api_is_never_reported_as_a_failed_tool() {
    // What the injected call script actually answers on a page with no
    // `document.modelContext`: the absent envelope, not `state: "error"`. A
    // tool that "failed" would tell an agent its call was rejected by the site,
    // which is the opposite of what happened.
    let name = WebmcpToolName::parse("add_to_cart").unwrap_or_else(|e| panic!("{e}"));
    let expression = loams_web_bridge::webmcp::WebmcpRequest::call(
        &CallWebmcpToolRequest::bare("add_to_cart").unwrap_or_else(|e| panic!("{e}")),
    )
    .script();
    assert!(expression.contains("document.modelContext"), "{expression}");
    assert!(expression.contains("supported: false"), "{expression}");
    assert!(
        !expression.contains("state: 'error', reason: 'not-exposed'"),
        "{expression}"
    );
    assert_eq!(name.as_str(), "add_to_cart");
}

#[tokio::test]
async fn a_permissions_policy_refusal_on_a_call_is_typed_not_a_page_refusal() {
    // The real `NotAllowedError` is a DOMException, so it carries a message the
    // page wrote, and only its name is the bridge's to trust.
    let (_cdp, provider) = remote_with(vec![(
        "loamsWebmcpCall",
        evaluate_json(loams_web_bridge::webmcp::envelope_call_error(
            "NotAllowedError: Permission policy 'tools' is disallowed in this document",
        )),
    )]);
    let page = open(provider.as_ref()).await;
    let request = CallWebmcpToolRequest::bare("pay").unwrap_or_else(|e| panic!("{e}"));
    let outcome = provider
        .call_webmcp_tool(&page, &request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!outcome.executed, "{outcome:?}");
    assert_eq!(
        outcome.availability.absent(),
        Some(&WebmcpAbsent::BlockedByPermissionsPolicy),
        "{outcome:?}"
    );
    assert!(
        outcome.render().contains("Permissions Policy"),
        "{}",
        outcome.render()
    );
}

#[tokio::test]
async fn a_page_that_refuses_for_its_own_reasons_says_which() {
    // The `tools` Permissions Policy refusal is the page's configuration, not a
    // browser limitation, and only the page can fix it.
    let (_driver, local) = local_over(page_with(FakeModelContext::refused("NotAllowedError")));
    let page = open(local.as_ref()).await;
    let listing = local
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        listing.availability.absent(),
        Some(&WebmcpAbsent::BlockedByPermissionsPolicy)
    );
    assert!(
        listing.render().contains("Permissions Policy"),
        "{}",
        listing.render()
    );
}

#[tokio::test]
async fn a_driver_that_cannot_evaluate_degrades_to_an_answer() {
    // A host that has not implemented the WebMCP seam at all. The trait's
    // default refuses, and the tool turns that refusal into one sentence rather
    // than a failed session.
    #[derive(Debug)]
    struct PlainDriver;

    #[async_trait::async_trait]
    impl PageDriver for PlainDriver {
        async fn open(
            &self,
            _request: &OpenRequest,
        ) -> Result<(DriverPage, PageState), BridgeError> {
            Ok((
                DriverPage::new("plain-1"),
                PageState {
                    url: URL.to_string(),
                    title: "Sign in".to_string(),
                    ..PageState::default()
                },
            ))
        }

        async fn state(&self, _page: &DriverPage) -> Result<PageState, BridgeError> {
            Ok(PageState {
                url: URL.to_string(),
                title: "Sign in".to_string(),
                ..PageState::default()
            })
        }

        async fn act(
            &self,
            _page: &DriverPage,
            _action: DriverAction,
        ) -> Result<Option<PageState>, BridgeError> {
            Ok(None)
        }
    }

    let mut config = WebBridgeConfig {
        provider: ProviderKind::Local,
        ..WebBridgeConfig::default()
    };
    config.local.artifact_dir = None;
    let provider = local_provider(&config, Arc::new(PlainDriver), resolver())
        .unwrap_or_else(|error| panic!("{error}"));

    let page = open(provider.as_ref()).await;
    let listing = provider
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        listing.availability.absent(),
        Some(&WebmcpAbsent::DriverCannotEvaluate)
    );
    assert!(
        listing.render().contains("cannot evaluate a script"),
        "{}",
        listing.render()
    );
    let request = CallWebmcpToolRequest::bare("pay").unwrap_or_else(|e| panic!("{e}"));
    let outcome = provider
        .call_webmcp_tool(&page, &request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!outcome.executed);
}

#[tokio::test]
async fn the_input_reaches_the_driver_intact_and_never_the_log() {
    let (driver, provider) = local_over(page_with(webmcp_present()));
    let page = open(provider.as_ref()).await;
    let request = CallWebmcpToolRequest::new(
        "find_items",
        json!({"query": "a \"quoted\" <b> value", "limit": 5}),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    provider
        .call_webmcp_tool(&page, &request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let requests = driver.webmcp_requests();
    let last = requests
        .last()
        .unwrap_or_else(|| panic!("a driver request"));
    let loams_web_bridge::WebmcpRequest::Call { name, input, .. } = last else {
        panic!("a call request, got {last:?}");
    };
    assert_eq!(name.as_str(), "find_items");
    assert_eq!(input["query"], "a \"quoted\" <b> value");
    assert_eq!(input["limit"], 5);

    // The driver's own log records the shape, never the values.
    let actions = driver.actions();
    let logged = actions.last().unwrap_or_else(|| panic!("a logged action"));
    assert!(logged.contains("find_items"), "{logged}");
    assert!(!logged.contains("quoted"), "{logged}");
    assert!(!logged.contains("4111"), "{logged}");
}

#[tokio::test]
async fn the_remote_script_feature_detects_and_marshals_its_input() {
    let (cdp, provider) = remote_present();
    let page = open(provider.as_ref()).await;
    let request = CallWebmcpToolRequest::new("add_to_cart", json!({"sku": "a \"b\" <i>"}))
        .unwrap_or_else(|error| panic!("{error}"));
    provider
        .call_webmcp_tool(&page, &request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    provider
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let expressions = cdp.expressions();
    let call = expressions
        .iter()
        .find(|expression| expression.contains("loamsWebmcpCall"))
        .unwrap_or_else(|| panic!("the call script was evaluated"));
    // Feature detection, never an assumption.
    assert!(call.contains("document.modelContext"), "{call}");
    // The input is handed over as JSON the script parses, so a value can never
    // become code and nothing in it can close a marker.
    assert!(call.contains("JSON.parse"), "{call}");
    assert!(!call.contains("<i>"), "{call}");
    assert!(call.contains("\\u003ci\\u003e"), "{call}");
    assert!(expressions.iter().any(|e| e.contains("loamsWebmcpList")));
}

#[tokio::test]
async fn a_page_that_answers_nonsense_is_an_error_not_a_silent_empty_list() {
    let (_cdp, provider) = remote_with(vec![(
        "loamsWebmcp",
        evaluate_json(json!({"supported": true})),
    )]);
    let page = open(provider.as_ref()).await;
    let error = provider
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .err()
        .unwrap_or_else(|| panic!("a malformed answer is refused"));
    assert!(matches!(error, BridgeError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("replaced"), "{error}");
}

#[tokio::test]
async fn a_tool_name_is_validated_against_the_draft() {
    for name in ["a", "add_to_cart", "a.b-c", "Tool9"] {
        let parsed = WebmcpToolName::parse(name);
        assert!(parsed.is_ok(), "{name} should parse");
    }
    let long = "x".repeat(WebmcpToolName::parse("a").map(|_| ()).map_or(129, |_| 129));
    assert!(WebmcpToolName::parse(&long).is_err());
    for name in ["", "a b", "a/b", "a;b", "tool\nname", "é", "🔍"] {
        assert!(
            WebmcpToolName::parse(name).is_err(),
            "{name:?} should be refused"
        );
    }
}

/// A thrown script is reported from the protocol, not as a missing answer.
///
/// `exceptionDetails` sits *beside* `result`, not inside it. A remote provider
/// that reads `result.value` alone sees an error object with no string value
/// and reports "the browser returned no WebMCP answer" — losing the one fact
/// that matters, which is that the page threw. This is the same drift the local
/// provider does not have, so it is asserted against the remote one.
#[tokio::test]
async fn a_thrown_script_is_reported_and_not_read_as_a_missing_answer() {
    let (_cdp, remote) = remote_with(vec![(
        "loamsWebmcpList",
        json!({
            "result": { "type": "object", "subtype": "error" },
            "exceptionDetails": { "text": "ReferenceError: bridge is not defined" },
        }),
    )]);
    let page = open(remote.as_ref()).await;

    let error = remote
        .list_webmcp_tools(&page, &ListWebmcpToolsRequest::new())
        .await
        .expect_err("a thrown script is not a listing");

    let message = error.to_string();
    assert!(message.contains("threw in the page"), "{message}");
    assert!(message.contains("ReferenceError"), "{message}");
    assert!(
        !message.contains("returned no WebMCP answer"),
        "a thrown script must not be reported as a missing answer: {message}"
    );
}
