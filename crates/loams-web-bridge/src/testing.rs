//! Deterministic drivers for the conformance suite: a fake local browser and a
//! scripted CDP connection.
//!
//! Both record what they were asked, so a test can assert the exact CDP
//! conversation a provider has (module `browser_run`) without a Cloudflare
//! account, and both refuse nothing, so the provider's own policy is what the
//! test is really measuring.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::browser_run::cdp::{CdpConnection, CdpRequest, CdpTransport};
use crate::browser_run::endpoint::RequestHeaders;
use crate::error::BridgeError;
use crate::local::{DriverAction, DriverPage, PageDriver};
use crate::page::PageState;
use crate::provider::OpenRequest;
use crate::tool::{AxNode, WaitRequest};
use crate::webmcp::{
    WebmcpRequest, envelope_absent, envelope_call_error, envelope_call_ok, envelope_call_timeout,
    envelope_call_unknown, envelope_tools,
};

/// A page of the fake local browser.
#[derive(Clone, Default)]
pub struct FakePage {
    /// The URL the driver reports.
    pub url: String,
    /// The title the driver reports.
    pub title: String,
    /// The accessibility tree's roots.
    pub nodes: Vec<AxNode>,
    /// The visible text.
    pub text: String,
    /// The bytes a screenshot returns.
    pub screenshot: Vec<u8>,
    /// The page's `document.modelContext`, absent unless a fixture sets one.
    pub webmcp: FakeModelContext,
}

/// A page's WebMCP surface, for the deterministic driver (AP1d Task 3).
///
/// The default is `Default::default()`, which is a page with **no**
/// `document.modelContext` — the state Safari is permanently in, so it is the
/// default a test gets rather than something it has to opt out of.
#[derive(Clone, Debug, Default)]
pub struct FakeModelContext {
    /// Whether the page exposes `document.modelContext` at all.
    pub present: bool,
    /// Why it is unusable when `present` is false: `not-exposed`,
    /// `insecure-context`, or an error name such as `NotAllowedError`.
    pub reason: String,
    /// The registered tools, in registration order.
    pub tools: Vec<FakeWebmcpTool>,
    /// What each tool returns, by name.
    pub results: HashMap<String, String>,
    /// Whether a call rejects instead of returning.
    pub refuses: Option<String>,
    /// Whether a call never answers.
    pub hangs: bool,
    /// Whether a successful call changes the page, so a change summary exists.
    pub mutates: bool,
}

impl FakeModelContext {
    /// A context that is not there, which is the default.
    pub fn absent() -> Self {
        Self::default()
    }

    /// A context with `tools` registered.
    pub fn with_tools(tools: Vec<FakeWebmcpTool>) -> Self {
        Self {
            present: true,
            tools,
            ..Self::default()
        }
    }

    /// The page refuses to answer at all, for this reason.
    pub fn refused(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
            ..Self::default()
        }
    }

    /// What the tool named `name` answers.
    pub fn answering(self, name: &str, text: &str) -> Self {
        let mut context = self;
        context.results.insert(name.to_string(), text.to_string());
        context
    }

    /// The envelope a browser would return for a listing.
    pub fn listing_envelope(&self, filter: Option<&str>) -> Value {
        if !self.present || !self.reason.is_empty() {
            let reason = if self.reason.is_empty() {
                "not-exposed"
            } else {
                self.reason.as_str()
            };
            return envelope_absent(reason);
        }
        let wanted = filter.unwrap_or_default().to_lowercase();
        let tools: Vec<Value> = self
            .tools
            .iter()
            .filter(|tool| wanted.is_empty() || tool.name.to_lowercase().contains(&wanted))
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "title": tool.title,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                    "annotations": tool.annotations,
                })
            })
            .collect();
        envelope_tools(&tools)
    }

    /// The envelope a browser would return for a call to `name`.
    ///
    /// A page with no usable `document.modelContext` answers the call script the
    /// way it answers the listing script, because both are the same fact: the
    /// API is not there. A *tool's* refusal is a different envelope, which is
    /// what keeps the two apart.
    pub fn call_envelope(&self, name: &str) -> Value {
        if !self.present || !self.reason.is_empty() {
            return envelope_absent(if self.reason.is_empty() {
                "not-exposed"
            } else {
                self.reason.as_str()
            });
        }
        let names: Vec<String> = self.tools.iter().map(|tool| tool.name.clone()).collect();
        if !names.iter().any(|registered| registered == name) {
            return envelope_call_unknown(&names);
        }
        if let Some(reason) = &self.refuses {
            return envelope_call_error(reason);
        }
        if self.hangs {
            return envelope_call_timeout();
        }
        envelope_call_ok(self.results.get(name).map_or("ok", String::as_str))
    }
}

/// One tool a [`FakeModelContext`] has registered.
#[derive(Clone, Debug, Default)]
pub struct FakeWebmcpTool {
    /// The tool's name.
    pub name: String,
    /// The title, when the page gave one.
    pub title: Option<String>,
    /// The description.
    pub description: String,
    /// The input schema, as the page wrote it.
    pub input_schema: Option<Value>,
    /// The four annotations.
    pub annotations: Option<Value>,
}

impl FakeWebmcpTool {
    /// A tool named `name`.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Self::default()
        }
    }

    /// With a title.
    pub fn titled(mut self, title: &str) -> Self {
        self.title = Some(title.to_string());
        self
    }

    /// With a description.
    pub fn described(mut self, description: &str) -> Self {
        self.description = description.to_string();
        self
    }

    /// With an input schema.
    pub fn requiring(mut self, required: &[&str]) -> Self {
        self.input_schema = Some(serde_json::json!({
            "type": "object",
            "properties": required
                .iter()
                .map(|name| (name.to_string(), serde_json::json!({ "type": "string" })))
                .collect::<serde_json::Map<String, Value>>(),
            "required": required,
        }));
        self
    }

    /// Read-only, per the draft's `readOnlyHint`.
    pub fn read_only(mut self) -> Self {
        self.annotations = Some(serde_json::json!({ "readOnlyHint": true }));
        self
    }
}

/// A local driver that serves [`FakePage`]s and records every action.
#[derive(Debug, Default)]
pub struct FakeDriver {
    pages: Mutex<HashMap<String, FakePage>>,
    /// Every action, in order, with the page's own value elided.
    pub actions: Mutex<Vec<String>>,
    /// Every WebMCP request the driver was asked, in order. Unlike `actions`
    /// this keeps the request whole, because a test asserts that the input
    /// arrived intact: a `fill`'s value is a secret and this is not.
    pub webmcp_requests: Mutex<Vec<WebmcpRequest>>,
    next_id: AtomicU64,
    page: Mutex<FakePage>,
}

/// The standard fixture page: a form with a button, a field and a password.
pub fn login_page(url: &str) -> FakePage {
    FakePage {
        url: url.to_string(),
        title: "Sign in".to_string(),
        nodes: vec![
            AxNode::new("document", "Sign in")
                .with_backend_id(1)
                .with_child(
                    AxNode::new("textbox", "Username")
                        .with_backend_id(2)
                        .with_state("focused"),
                ),
            AxNode::new("button", "Sign in").with_backend_id(3),
            AxNode::new("textbox", "Password")
                .with_value("hunter2")
                .with_backend_id(4)
                .sensitive(),
        ],
        text: "Sign in".to_string(),
        screenshot: b"\x89PNG\r\n\x1a\nfake".to_vec(),
        webmcp: FakeModelContext::absent(),
    }
}

impl FakeDriver {
    /// A driver serving one page.
    pub fn new(page: FakePage) -> Self {
        Self {
            page: Mutex::new(page),
            ..Self::default()
        }
    }

    /// Replace the page the driver serves, as a navigation would.
    pub fn set_page(&self, page: FakePage) {
        *self
            .page
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = page;
    }

    /// The page the driver serves.
    pub fn page(&self) -> FakePage {
        self.page
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The actions recorded so far.
    pub fn actions(&self) -> Vec<String> {
        self.actions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The WebMCP requests recorded so far, in order.
    pub fn webmcp_requests(&self) -> Vec<WebmcpRequest> {
        self.webmcp_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for FakePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakePage")
            .field("url", &self.url)
            .field("title", &self.title)
            .field("nodes", &self.nodes.len())
            .finish()
    }
}

#[async_trait]
impl PageDriver for FakeDriver {
    async fn open(&self, _request: &OpenRequest) -> Result<(DriverPage, PageState), BridgeError> {
        let id = format!("page-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        self.pages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), self.page());
        self.actions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push("open".to_string());
        Ok((DriverPage::new(id), state_of(&self.page())))
    }

    async fn state(&self, page: &DriverPage) -> Result<PageState, BridgeError> {
        let pages = self
            .pages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match pages.get(page.id()) {
            Some(fake) => Ok(state_of(fake)),
            None => Err(BridgeError::unavailable(format!(
                "{} is not open",
                page.id()
            ))),
        }
    }

    async fn act(
        &self,
        page: &DriverPage,
        action: DriverAction,
    ) -> Result<Option<PageState>, BridgeError> {
        // Recorded with the filled value left out on purpose: a test asserts
        // that a secret does not travel through the driver's own log either.
        let described = match &action {
            DriverAction::Navigate(url) => format!("navigate {}", url),
            DriverAction::Click { backend_id } => format!("click {backend_id}"),
            DriverAction::Fill { backend_id, value } => {
                format!("fill {backend_id} ({} bytes)", value.len())
            }
            DriverAction::Wait(WaitRequest { condition, .. }) => format!("wait {condition:?}"),
            DriverAction::Screenshot => "screenshot".to_string(),
            DriverAction::Extract => "extract".to_string(),
            DriverAction::Close => "close".to_string(),
        };
        self.actions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(described);

        if let DriverAction::Navigate(url) = &action {
            let mut fake = self.page();
            fake.url = url.to_string();
            fake.title = format!("Sign in ({})", url.host_str().unwrap_or_default());
            self.set_page(fake);
        }
        let updated = state_of(&self.page());
        if let DriverAction::Screenshot = action {
            let mut updated = updated;
            updated.screenshot = Some((self.page().screenshot.clone(), "png" as &'static str));
            return Ok(Some(updated));
        }
        let pages = self
            .pages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = pages.get(page.id()).cloned().unwrap_or_default();
        drop(pages);
        let mut current = current;
        current.url = updated.url;
        current.title = updated.title;
        current.nodes = updated.nodes;
        current.text = updated.text;
        Ok(Some(state_of(&current)))
    }

    /// Answer a WebMCP question the way the fake page would.
    ///
    /// The page is read out of the per-page entry rather than the template, so
    /// a navigation, or a tool that mutates, is reflected in what the next call
    /// sees.
    async fn webmcp(
        &self,
        page: &DriverPage,
        request: &WebmcpRequest,
    ) -> Result<Value, BridgeError> {
        let mut fake = {
            let pages = self
                .pages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pages
                .get(page.id())
                .cloned()
                .ok_or_else(|| BridgeError::unavailable(format!("{} is not open", page.id())))?
        };
        self.webmcp_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.clone());
        // Recorded the way `fill` is: the name, and the size of the input, so a
        // test asserts a shape rather than a credential's bytes.
        self.actions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.summary());
        let answer = match request {
            WebmcpRequest::List { filter } => fake.webmcp.listing_envelope(filter.as_deref()),
            WebmcpRequest::Call { name, .. } => fake.webmcp.call_envelope(name.as_str()),
        };
        let ran = answer.get("state").and_then(Value::as_str) == Some("ok");
        if ran && fake.webmcp.mutates {
            fake.nodes
                .push(AxNode::new("status", "tool ran").with_backend_id(999));
            let mut pages = self
                .pages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = pages.get_mut(page.id()) {
                entry.nodes = fake.nodes;
            }
        }
        Ok(answer)
    }
}

fn state_of(fake: &FakePage) -> PageState {
    PageState {
        url: fake.url.clone(),
        title: fake.title.clone(),
        nodes: fake.nodes.clone(),
        text: fake.text.clone(),
        screenshot: None,
    }
}

/// A CDP connection that answers from a script and records every request.
#[derive(Debug, Default)]
pub struct FakeCdp {
    /// Answers by method name; a method with no entry returns `{}`.
    pub answers: HashMap<String, Value>,
    /// Answers by a substring of the expression, tried in order before the
    /// per-method answer. The WebMCP scripts and the page-read script are all
    /// `Runtime.evaluate`, so a test that wants different answers for each has
    /// to tell them apart by something in the expression.
    pub expression_answers: Vec<(String, Value)>,
    /// Every request, in order, as `method(session)`.
    pub calls: Mutex<Vec<String>>,
    /// Every `Runtime.evaluate` expression, in order.
    pub expressions: Mutex<Vec<String>>,
    /// When set, `Target.createTarget` fails with this message.
    pub fail_create_target: Option<String>,
}

impl FakeCdp {
    /// A connection that answers `answers`.
    pub fn new(answers: HashMap<String, Value>) -> Self {
        Self {
            answers,
            ..Self::default()
        }
    }

    /// Answer an expression containing `needle` with `answer`.
    pub fn with_expression(mut self, needle: &str, answer: Value) -> Self {
        self.expression_answers.push((needle.to_string(), answer));
        self
    }

    /// The methods called so far.
    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The `Runtime.evaluate` expressions evaluated so far, in order.
    pub fn expressions(&self) -> Vec<String> {
        self.expressions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record(&self, request: &CdpRequest) {
        let session = request.session_id.as_deref().unwrap_or("-");
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{}({session})", request.method));
    }
}

#[async_trait]
impl CdpConnection for FakeCdp {
    async fn send(&self, request: CdpRequest) -> Result<Value, BridgeError> {
        self.record(&request);
        if request.method == "Target.createTarget"
            && let Some(message) = &self.fail_create_target
        {
            return Err(BridgeError::cdp(message.clone()));
        }
        if request.method == "Runtime.evaluate" {
            let expression = request
                .params
                .get("expression")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.expressions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(expression.clone());
            for (needle, answer) in &self.expression_answers {
                if expression.contains(needle.as_str()) {
                    return Ok(answer.clone());
                }
            }
        }
        Ok(self
            .answers
            .get(&request.method)
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())))
    }

    async fn close(&self) -> Result<(), BridgeError> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push("close".to_string());
        Ok(())
    }
}

/// The transport that hands out one [`FakeCdp`] and remembers the handshake.
#[derive(Debug)]
pub struct FakeTransport {
    /// The connection every `connect` returns.
    pub connection: Arc<FakeCdp>,
    /// The endpoints and headers of every handshake.
    pub handshakes: Mutex<Vec<(String, RequestHeaders)>>,
}

impl FakeTransport {
    /// A transport over `connection`.
    pub fn new(connection: Arc<FakeCdp>) -> Self {
        Self {
            connection,
            handshakes: Mutex::new(Vec::new()),
        }
    }

    /// The handshakes recorded so far.
    pub fn handshakes(&self) -> Vec<(String, RequestHeaders)> {
        self.handshakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl CdpTransport for FakeTransport {
    async fn connect(
        &self,
        endpoint: &str,
        headers: &RequestHeaders,
    ) -> Result<Arc<dyn CdpConnection>, BridgeError> {
        self.handshakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((endpoint.to_string(), headers.clone()));
        Ok(Arc::clone(&self.connection) as Arc<dyn CdpConnection>)
    }
}

/// A CDP answer for `Accessibility.getFullAXTree` with one button.
pub fn ax_tree_with_button(name: &str, backend_id: i64) -> Value {
    serde_json::json!({
        "nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Example"},
             "childIds": ["2"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": name},
             "backendDOMNodeId": backend_id}
        ]
    })
}

/// A CDP answer for an `Runtime.evaluate` whose expression returned `value` as
/// its JSON string, which is how both the page-read and the WebMCP scripts come
/// back.
pub fn evaluate_json(value: Value) -> Value {
    serde_json::json!({
        "result": { "type": "string", "value": value.to_string() }
    })
}

/// A CDP answer for `Runtime.evaluate` with a URL, title and text.
pub fn read_page(url: &str, title: &str, text: &str) -> Value {
    serde_json::json!({
        "result": {
            "type": "string",
            "value": serde_json::json!({ "u": url, "t": title, "x": text }).to_string()
        }
    })
}
