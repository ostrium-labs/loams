//! The Obscura provider: one CDP driver for every place a browser runs
//! (D2110, D2111).
//!
//! Obscura (Apache-2.0, pinned at [`crate::config::PINNED_OBSCURA_VERSION`])
//! is the one engine. Where it runs is the only thing that differs, and it is
//! an [`EngineEndpoint`]:
//!
//! * [`ServiceEndpoint`] — the deployment's `browser` service (compose profile
//!   `browser`, or the Kubernetes manifests under deploy/browser), reached on
//!   the private network with the engine's bearer token;
//! * the local launcher (`launch::LocalEngine`) — an `obscura serve` process
//!   on loopback that agentd or Desktop starts on the user's machine.
//!
//! Everything else is this file, so the two cannot drift (D2112):
//!
//! * **Isolation.** Each page gets its own browser context
//!   (`Target.createBrowserContext`), so two agents on one shared service never
//!   share cookies or storage, and closing a page disposes its context rather
//!   than closing the browser everyone else is using.
//! * **Network policy.** The egress policy is checked before every
//!   navigation, on the URL the page ends up at (a redirect is refused after
//!   the fact by sending the page to `about:blank`), and on every request the
//!   page makes while the connection delivers events: `Fetch.enable` pauses
//!   each request and the provider continues or fails it. The engine's own
//!   SSRF guard (private addresses refused after DNS resolution) is the layer
//!   underneath.
//! * **Credential boundary.** A secret fill needs `allow_secret_fills` and a
//!   listed host, is resolved at the moment of the fill, and is registered for
//!   scrubbing before it reaches the socket. The engine's token is a secret
//!   too, resolved per connection and never logged.
//! * **Budgets.** Concurrent pages, a per-call timeout, a per-page lifetime and
//!   an optional daily ceiling.
//! * **Recording.** An action log per page when it is switched on.

pub mod launch;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

use crate::artifact;
use crate::cdp::budget::BrowserBudget;
use crate::cdp::{CdpClient, CdpEvent, CdpTransport, RequestHeaders, ax};
use crate::config::{
    BudgetConfig, CredentialPolicy, Engine, ProviderKind, RecordingConfig, ServiceConfig,
};
use crate::egress::EgressPolicy;
use crate::error::BridgeError;
use crate::page::{PageState, SnapshotCache, outcome};
use crate::provider::{BrowserProvider, Capabilities, OpenRequest, PageRef};
use crate::record::ActionLog;
use crate::secret::{SecretRef, SecretResolver, SecretValue};
use crate::tool::{
    ActionOutcome, ChangeSummary, Extracted, FillValue, FindQuery, Navigation, Screenshot,
    Snapshot, SnapshotMode, SnapshotNode, SnapshotRequest, Uid, WaitCondition, WaitOutcome,
    WaitRequest,
};
use crate::webmcp::{
    CallWebmcpToolRequest, ListWebmcpToolsRequest, WebmcpCallOutcome, WebmcpListing, WebmcpRequest,
    call_from_evaluation, listing_from_evaluation,
};

/// The script that reads the page's URL, title and visible text in one call.
const READ_PAGE: &str = "JSON.stringify({u: location.href, t: document.title, \
                          x: document.body ? document.body.innerText : ''})";

/// The script that clicks a resolved element.
const CLICK_ELEMENT: &str = "function() { this.scrollIntoView({block: 'center'}); this.click(); \
                             return true; }";

/// The script that types into a resolved element the way a framework expects:
/// through the prototype's setter, then `input` and `change`.
const FILL_ELEMENT: &str = "function(v) { const el = this; const proto = \
                             el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : \
                             HTMLInputElement.prototype; const setter = \
                             Object.getOwnPropertyDescriptor(proto, 'value').set; \
                             setter.call(el, v); el.dispatchEvent(new Event('input', \
                             {bubbles: true})); el.dispatchEvent(new Event('change', \
                             {bubbles: true})); return true; }";

/// The header the engine reads its token from.
pub const AUTHORIZATION_HEADER: &str = "Authorization";

/// Where an Obscura engine is, and how to authenticate to it.
#[async_trait]
pub trait EngineEndpoint: Send + Sync + fmt::Debug {
    /// Which provider kind this endpoint makes.
    fn kind(&self) -> ProviderKind;

    /// The DevTools WebSocket URL and the handshake headers for one new
    /// connection. A local endpoint starts its engine on the first call.
    async fn handshake(&self) -> Result<(String, RequestHeaders), BridgeError>;

    /// Stop what this endpoint started. Called after every page's connection
    /// is closed. A service endpoint owns no process, so the default does
    /// nothing.
    async fn shutdown(&self) {}
}

/// The handshake headers for a bearer token, registered for scrubbing first.
pub fn bearer(token: &SecretValue) -> RequestHeaders {
    crate::redact::register(token.expose());
    RequestHeaders::none().with(AUTHORIZATION_HEADER, format!("Bearer {}", token.expose()))
}

/// The deployment's `browser` service.
#[derive(Debug)]
pub struct ServiceEndpoint {
    url: String,
    token: Option<SecretRef>,
    secrets: Arc<dyn SecretResolver>,
}

impl ServiceEndpoint {
    /// An endpoint for `config`, checked: `ws://` only on the private network,
    /// and a token reference unless the service is on loopback.
    pub fn new(
        config: &ServiceConfig,
        secrets: Arc<dyn SecretResolver>,
    ) -> Result<Self, BridgeError> {
        let url = config.check()?;
        Ok(Self {
            url: url.to_string(),
            token: config.token_ref()?,
            secrets,
        })
    }
}

#[async_trait]
impl EngineEndpoint for ServiceEndpoint {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Service
    }

    async fn handshake(&self) -> Result<(String, RequestHeaders), BridgeError> {
        let headers = match &self.token {
            Some(reference) => bearer(&self.secrets.resolve(reference).await?),
            None => RequestHeaders::none(),
        };
        Ok((self.url.clone(), headers))
    }
}

/// The policies every Obscura provider applies, whatever its endpoint.
#[derive(Debug, Clone)]
pub struct Policies {
    /// What pages may open.
    pub egress: EgressPolicy,
    /// Which secret fills are allowed.
    pub credentials: CredentialPolicy,
    /// How much browser may be used.
    pub budgets: BudgetConfig,
    /// Whether pages are recorded.
    pub recording: RecordingConfig,
    /// Where screenshots and action logs go.
    pub artifacts: PathBuf,
}

/// One open page.
#[derive(Debug)]
struct PageSlot {
    client: Arc<CdpClient>,
    session_id: String,
    target_id: String,
    context_id: String,
    opened_at: Instant,
    /// The concurrency permit, held for as long as the page is open.
    _permit: OwnedSemaphorePermit,
    /// The request-interception task, when the connection delivers events.
    interceptor: Option<tokio::task::JoinHandle<()>>,
    log: Option<ActionLog>,
    state: Mutex<PageState>,
    cache: Mutex<SnapshotCache>,
}

/// The Obscura provider.
#[derive(Debug)]
pub struct ObscuraProvider {
    endpoint: Arc<dyn EngineEndpoint>,
    policies: Policies,
    secrets: Arc<dyn SecretResolver>,
    transport: Arc<dyn CdpTransport>,
    budget: Arc<BrowserBudget>,
    permits: Arc<Semaphore>,
    next_handle: AtomicU64,
    pages: Mutex<HashMap<String, Arc<PageSlot>>>,
}

impl ObscuraProvider {
    /// A provider over `endpoint`.
    pub fn new(
        endpoint: Arc<dyn EngineEndpoint>,
        policies: Policies,
        secrets: Arc<dyn SecretResolver>,
        transport: Arc<dyn CdpTransport>,
    ) -> Self {
        let budget = Arc::new(match policies.budgets.daily_budget() {
            Some(limit) => BrowserBudget::new(u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)),
            None => BrowserBudget::unlimited(),
        });
        let permits = Arc::new(Semaphore::new(policies.budgets.max_concurrent_sessions));
        Self {
            endpoint,
            policies,
            secrets,
            transport,
            budget,
            permits,
            next_handle: AtomicU64::new(1),
            pages: Mutex::new(HashMap::new()),
        }
    }

    /// The daily budget guard, so a host can show what has been spent today.
    pub fn budget(&self) -> &BrowserBudget {
        &self.budget
    }

    /// The policies this provider applies.
    pub fn policies(&self) -> &Policies {
        &self.policies
    }

    /// Close every open page, then stop the endpoint, in that order.
    ///
    /// The order matters for Obscura: an engine merges a connection's state
    /// only when that connection closes, and SIGTERM saves only what was
    /// merged, so closing the sockets first is what a graceful stop means.
    async fn close_all_then_stop(&self) {
        let open: Vec<String> = self
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        for id in open {
            let page = PageRef::new(self.kind_of(), id);
            if let Err(error) = BrowserProvider::close(self, &page).await {
                tracing::debug!(%error, "closing a page at shutdown failed");
            }
        }
        self.endpoint.shutdown().await;
    }

    fn kind_of(&self) -> ProviderKind {
        self.endpoint.kind()
    }

    fn slot(&self, page: &PageRef) -> Result<Arc<PageSlot>, BridgeError> {
        if page.provider() != self.kind_of() {
            return Err(BridgeError::policy(format!(
                "this handle belongs to the {} provider",
                page.provider().as_str()
            )));
        }
        let slot = self
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(page.id())
            .cloned()
            .ok_or_else(|| {
                BridgeError::unavailable(format!("page {} is not open; open it again", page.id()))
            })?;
        if slot.opened_at.elapsed() > self.policies.budgets.max_session() {
            return Err(BridgeError::unavailable(format!(
                "page {} has been open longer than budgets.max_session_ms ({} ms); close it and \
                 open a new one",
                page.id(),
                self.policies.budgets.max_session_ms
            )));
        }
        Ok(slot)
    }

    fn insert(&self, slot: PageSlot, id: String) -> PageRef {
        let handle = PageRef::new(self.kind_of(), id.clone());
        self.pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::new(slot));
        handle
    }

    /// Whether a secret may be typed into this page (D567, D2112).
    fn fill_allowed(&self, url: &str) -> Result<(), BridgeError> {
        if !self.policies.credentials.allow_secret_fills {
            return Err(BridgeError::policy(
                "secret fills are off; set credentials.allow-secret-fills and list the host in \
                 credentials.allowed-fill-hosts",
            ));
        }
        let host = Url::parse(url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        if !self.policies.credentials.allows_fill_on(&host) {
            return Err(BridgeError::policy(format!(
                "{host} is not in credentials.allowed-fill-hosts"
            )));
        }
        Ok(())
    }

    fn record<T>(
        slot: &PageSlot,
        tool: &str,
        target: &str,
        started: Instant,
        result: &Result<T, BridgeError>,
    ) {
        if let Some(log) = &slot.log {
            log.record(tool, target, result.as_ref().err(), started.elapsed());
        }
    }

    async fn read_state(client: &CdpClient, session_id: &str) -> Result<PageState, BridgeError> {
        let tree = client
            .call_page(
                session_id,
                "Accessibility.getFullAXTree",
                json!({ "depth": -1 }),
            )
            .await?;
        let nodes = ax::parse_ax_tree(&tree)?;
        let read = client
            .call_page(
                session_id,
                "Runtime.evaluate",
                json!({ "expression": READ_PAGE, "returnByValue": true }),
            )
            .await?;
        let (url, title, text) = parse_read(&read);
        Ok(PageState {
            url,
            title,
            nodes,
            text,
            screenshot: None,
        })
    }

    /// Read the page again and make it the slot's current snapshot.
    async fn resnapshot(slot: &PageSlot, verbose: bool) -> Result<Snapshot, BridgeError> {
        let state = Self::read_state(&slot.client, &slot.session_id).await?;
        let snapshot = {
            let mut cache = slot.cache.lock().unwrap_or_else(PoisonError::into_inner);
            cache.build(&state, verbose)
        };
        *slot.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
        Ok(snapshot)
    }

    async fn call_element(
        slot: &PageSlot,
        backend_id: i64,
        function: &str,
        arguments: Vec<Value>,
    ) -> Result<(), BridgeError> {
        let resolved = slot
            .client
            .call_page(
                &slot.session_id,
                "DOM.resolveNode",
                json!({ "backendNodeId": backend_id }),
            )
            .await?;
        let object_id = resolved
            .get("object")
            .and_then(|object| object.get("objectId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BridgeError::unavailable(format!(
                    "element {backend_id} is not in the page any more; take_snapshot again"
                ))
            })?
            .to_string();

        let result = slot
            .client
            .call_page(
                &slot.session_id,
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": function,
                    "arguments": arguments,
                    "returnByValue": true
                }),
            )
            .await;
        // Release the handle whether or not the call worked, so the session's
        // object group does not grow for its whole life.
        let _ = slot
            .client
            .call_page(
                &slot.session_id,
                "Runtime.releaseObject",
                json!({ "objectId": object_id }),
            )
            .await;
        result.map(|_| ())
    }

    /// Navigate and hold the result to the policy: the URL asked for before,
    /// and the URL the page ended up at after.
    async fn go(&self, slot: &PageSlot, url: &Url) -> Result<Navigation, BridgeError> {
        self.policies.egress.check(url)?;
        let result = slot
            .client
            .call_page(
                &slot.session_id,
                "Page.navigate",
                json!({ "url": url.as_str() }),
            )
            .await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            return Err(BridgeError::cdp(format!(
                "Page.navigate: {error} ({})",
                crate::tool::redact_url(url.as_str())
            )));
        }
        let state = Self::read_state(&slot.client, &slot.session_id).await?;
        if let Ok(landed) = Url::parse(&state.url)
            && landed.scheme() != "about"
            && let Err(refused) = self.policies.egress.check(&landed)
        {
            // A redirect took the page somewhere the policy refuses. Leave
            // nothing of it on screen for the next snapshot to read.
            let _ = slot
                .client
                .call_page(
                    &slot.session_id,
                    "Page.navigate",
                    json!({ "url": "about:blank" }),
                )
                .await;
            return Err(BridgeError::EgressDenied(refused));
        }
        let change = {
            let mut cache = slot.cache.lock().unwrap_or_else(PoisonError::into_inner);
            *slot.state.lock().unwrap_or_else(PoisonError::into_inner) = state.clone();
            cache.build(&state, false);
            cache.change()
        };
        Ok(Navigation {
            url: crate::tool::redact_url(&state.url),
            title: state.title.clone(),
            change,
        })
    }

    /// Pause every request the page makes and let through only what the
    /// egress policy allows.
    async fn intercept(
        &self,
        client: &Arc<CdpClient>,
        session_id: &str,
    ) -> Result<Option<tokio::task::JoinHandle<()>>, BridgeError> {
        let Some(events) = client.take_events() else {
            return Ok(None);
        };
        client
            .call_page(
                session_id,
                "Fetch.enable",
                json!({ "patterns": [{ "urlPattern": "*" }] }),
            )
            .await?;
        Ok(Some(tokio::spawn(answer_paused_requests(
            events,
            Arc::clone(client),
            session_id.to_string(),
            self.policies.egress.clone(),
        ))))
    }

    /// Evaluate a WebMCP script in the page and return the JSON it answered.
    ///
    /// `awaitPromise` is what makes `getTools()`'s promise a value here. The
    /// answer is parsed rather than passed on as a string because the page is
    /// untrusted and the parser checks the shape (D509), and the client
    /// timeout bounds a browser that never answers at all.
    async fn evaluate_webmcp(
        slot: &PageSlot,
        request: &WebmcpRequest,
    ) -> Result<Value, BridgeError> {
        tracing::debug!(tool = %request.summary(), "webmcp");
        let result = slot
            .client
            .call_page(
                &slot.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": request.script(),
                    "returnByValue": true,
                    "awaitPromise": true,
                    "timeout": request.timeout_ms(),
                }),
            )
            .await?;
        // A thrown script is reported from the protocol's own
        // `exceptionDetails`, which sits *beside* `result`. It has to be read
        // before `result.value`, or a thrown script would be reported as a
        // missing answer.
        crate::webmcp::thrown_in_page(&result)?;
        let value = result
            .get("result")
            .and_then(|inner| inner.get("value"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BridgeError::unavailable(
                    "the browser returned no WebMCP answer; the page may have replaced the \
                     bridge's script",
                )
            })?;
        serde_json::from_str(value).map_err(|error| {
            BridgeError::unavailable(format!(
                "the page's WebMCP answer was not JSON ({error}); this page may have replaced \
                 the bridge's script"
            ))
        })
    }
}

/// Answer each `Fetch.requestPaused` for one page: continue what the policy
/// allows, fail the rest as blocked by the client. Ends when the connection
/// closes.
async fn answer_paused_requests(
    mut events: tokio::sync::mpsc::UnboundedReceiver<CdpEvent>,
    client: Arc<CdpClient>,
    session_id: String,
    egress: EgressPolicy,
) {
    while let Some(event) = events.recv().await {
        if event.method != "Fetch.requestPaused" {
            continue;
        }
        if event
            .session_id
            .as_deref()
            .is_some_and(|session| session != session_id)
        {
            continue;
        }
        let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) else {
            continue;
        };
        let target = event
            .params
            .get("request")
            .and_then(|request| request.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let allowed = Url::parse(target)
            .map(|url| egress.check_request(&url).is_ok())
            .unwrap_or(false);
        let answer = if allowed {
            client
                .call_page(
                    &session_id,
                    "Fetch.continueRequest",
                    json!({ "requestId": request_id }),
                )
                .await
        } else {
            tracing::debug!(url = %crate::tool::redact_url(target), "browser request refused by the egress policy");
            client
                .call_page(
                    &session_id,
                    "Fetch.failRequest",
                    json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
                )
                .await
        };
        if let Err(error) = answer {
            tracing::debug!(%error, "answering a paused request failed");
        }
    }
}

/// Read the JSON the [`READ_PAGE`] script returned.
fn parse_read(result: &Value) -> (String, String, String) {
    let value = result
        .get("result")
        .and_then(|inner| inner.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let parsed: Value = serde_json::from_str(value).unwrap_or(Value::Null);
    let text = |key: &str| parsed.get(key).and_then(Value::as_str).unwrap_or_default();
    (
        text("u").to_string(),
        text("t").to_string(),
        text("x").to_string(),
    )
}

fn base64_decode(encoded: &str) -> Result<Vec<u8>, BridgeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| BridgeError::cdp(format!("the screenshot was not base64: {error}")))
}

fn required<'a>(value: &'a Value, field: &str, method: &str) -> Result<&'a str, BridgeError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::cdp(format!("{method} returned no {field}")))
}

#[async_trait]
impl BrowserProvider for ObscuraProvider {
    fn kind(&self) -> ProviderKind {
        self.kind_of()
    }

    fn engine(&self) -> Option<Engine> {
        Some(Engine::Obscura)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            core: true,
            // Obscura is built with its render feature in every image and
            // asset we pin (never the -no-render variants).
            screenshots: true,
            extraction: true,
            // Each page is its own throwaway browser context, on both kinds:
            // state lasts for the page's life and no longer. Obscura never
            // persists a `Target.createBrowserContext` context (only the
            // default context, and only into `--storage-dir`, when its
            // connection closes), so this provider cannot keep a sign-in
            // across pages. Persistence is the sidebar's job
            // (`loams-sidebar-browser`: the default context, a storage dir
            // and the cookie ledger).
            authenticated_sessions: false,
            tabs: true,
            credentials_stay_local: self.kind_of() == ProviderKind::Local,
            // Obscura 0.2.3 acknowledges Browser.setDownloadBehavior but has
            // no download path: no events, nothing written. Downloads are
            // denied on every connection and never reported as done.
            downloads: false,
        }
    }

    async fn open(&self, request: OpenRequest) -> Result<PageRef, BridgeError> {
        if let Some(url) = &request.url {
            self.policies.egress.check(url)?;
        }
        self.budget.check()?;
        let permit = Arc::clone(&self.permits).try_acquire_owned().map_err(|_| {
            BridgeError::SessionLimit {
                max: self.policies.budgets.max_concurrent_sessions,
            }
        })?;

        let (endpoint, headers) = self.endpoint.handshake().await?;
        let connection = self.transport.connect(&endpoint, &headers).await?;
        let client = Arc::new(CdpClient::new(
            connection,
            self.policies.budgets.request_timeout(),
        ));

        // Downloads are denied before the page exists. Obscura 0.2.3 only
        // acknowledges the call (it has no download path), so an engine that
        // refuses it is logged and not fatal: nothing can be downloaded
        // either way, and `Capabilities::downloads` is false.
        if let Err(error) = client
            .call("Browser.setDownloadBehavior", json!({ "behavior": "deny" }))
            .await
        {
            tracing::debug!(%error, "the engine refused Browser.setDownloadBehavior");
        }
        // A throwaway context per page. The engine never persists these
        // (only its default context, on connection close), which is what a
        // shared service wants: one agent's cookies never reach another's.
        let context = client
            .call("Target.createBrowserContext", json!({}))
            .await?;
        let context_id =
            required(&context, "browserContextId", "Target.createBrowserContext")?.to_string();
        let target = client
            .call(
                "Target.createTarget",
                json!({ "url": "about:blank", "browserContextId": context_id }),
            )
            .await?;
        let target_id = required(&target, "targetId", "Target.createTarget")?.to_string();
        let attached = client
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session_id = required(&attached, "sessionId", "Target.attachToTarget")?.to_string();

        for domain in ["Page", "Runtime", "DOM", "Accessibility"] {
            client
                .call_page(&session_id, &format!("{domain}.enable"), json!({}))
                .await?;
        }
        let interceptor = self.intercept(&client, &session_id).await?;

        let id = format!(
            "{}-{}",
            self.kind_of().as_str(),
            self.next_handle.fetch_add(1, Ordering::Relaxed)
        );
        let log = if self.policies.recording.enabled {
            Some(ActionLog::open(&self.policies.artifacts, &id)?)
        } else {
            None
        };
        let slot = PageSlot {
            client,
            session_id,
            target_id,
            context_id,
            opened_at: Instant::now(),
            _permit: permit,
            interceptor,
            log,
            state: Mutex::new(PageState::default()),
            cache: Mutex::new(SnapshotCache::new()),
        };

        let Some(url) = request.url else {
            // A blank page still gets a first read, so a uid exists before the
            // first action.
            let state = Self::read_state(&slot.client, &slot.session_id).await?;
            *slot.state.lock().unwrap_or_else(PoisonError::into_inner) = state.clone();
            slot.cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .build(&state, false);
            return Ok(self.insert(slot, id));
        };

        let handle = self.insert(slot, id);
        if let Err(error) = self.navigate(&handle, &url).await {
            let _ = self.close(&handle).await;
            return Err(error);
        }
        Ok(handle)
    }

    async fn navigate(&self, page: &PageRef, url: &Url) -> Result<Navigation, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let result = self.go(&slot, url).await;
        Self::record(
            &slot,
            "navigate",
            &crate::tool::redact_url(url.as_str()),
            started,
            &result,
        );
        result
    }

    async fn snapshot(
        &self,
        page: &PageRef,
        request: &SnapshotRequest,
    ) -> Result<Snapshot, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let result = Self::resnapshot(&slot, request.verbose).await;
        Self::record(&slot, "take_snapshot", "", started, &result);
        result
    }

    async fn find(
        &self,
        page: &PageRef,
        query: &FindQuery,
    ) -> Result<Vec<SnapshotNode>, BridgeError> {
        let slot = self.slot(page)?;
        slot.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .find(query)
    }

    async fn click(&self, page: &PageRef, uid: &Uid) -> Result<ActionOutcome, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let result: Result<ActionOutcome, BridgeError> = async {
            let node = slot
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .resolve(uid)?;
            Self::call_element(&slot, node.backend_id, CLICK_ELEMENT, Vec::new()).await?;
            Self::resnapshot(&slot, false).await?;
            Ok(outcome(
                format!("clicked {uid} ({})", node.role),
                &slot.cache.lock().unwrap_or_else(PoisonError::into_inner),
                SnapshotMode::Diff,
                &SnapshotRequest::new(),
            ))
        }
        .await;
        Self::record(&slot, "click", uid.as_str(), started, &result);
        result
    }

    async fn fill(
        &self,
        page: &PageRef,
        uid: &Uid,
        value: &FillValue,
    ) -> Result<ActionOutcome, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let result: Result<ActionOutcome, BridgeError> = async {
            let node = slot
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .resolve(uid)?;
            let secret = match value {
                FillValue::Literal(literal) => SecretValue::new(literal),
                FillValue::Secret(reference) => {
                    let url = slot
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .url
                        .clone();
                    self.fill_allowed(&url)?;
                    self.secrets.resolve(reference).await?
                }
            };
            // Register before the value goes anywhere near the socket, so an
            // error from the browser cannot echo it into a log.
            crate::redact::register(secret.expose());
            Self::call_element(
                &slot,
                node.backend_id,
                FILL_ELEMENT,
                vec![json!({ "value": secret.expose() })],
            )
            .await?;
            Self::resnapshot(&slot, false).await?;
            Ok(outcome(
                format!("filled {uid} ({})", node.role),
                &slot.cache.lock().unwrap_or_else(PoisonError::into_inner),
                SnapshotMode::Diff,
                &SnapshotRequest::new(),
            ))
        }
        .await;
        // The uid only: the value is never recorded, not even its length.
        Self::record(&slot, "fill", uid.as_str(), started, &result);
        result
    }

    async fn wait_for(
        &self,
        page: &PageRef,
        request: &WaitRequest,
    ) -> Result<WaitOutcome, BridgeError> {
        let slot = self.slot(page)?;
        let started = Instant::now();
        let deadline = started + Duration::from_millis(request.timeout_ms);
        let mut matched = false;
        loop {
            let holds = match &request.condition {
                WaitCondition::Selector(selector) => {
                    let expression = format!(
                        "Boolean(document.querySelector({}))",
                        serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".to_string())
                    );
                    let result = slot
                        .client
                        .call_page(
                            &slot.session_id,
                            "Runtime.evaluate",
                            json!({ "expression": expression, "returnByValue": true }),
                        )
                        .await?;
                    result
                        .get("result")
                        .and_then(|inner| inner.get("value"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                }
                condition => {
                    let fresh = Self::read_state(&slot.client, &slot.session_id).await?;
                    crate::local::condition_holds(condition, &fresh)
                }
            };
            if holds {
                matched = true;
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(request.poll_interval_ms)).await;
        }
        let answer = WaitOutcome {
            matched,
            waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            note: if matched {
                String::new()
            } else {
                "the condition did not hold before the timeout; take_snapshot to see the page"
                    .to_string()
            },
        };
        Self::record(&slot, "wait_for", "", started, &Ok::<_, BridgeError>(()));
        Ok(answer)
    }

    async fn screenshot(&self, page: &PageRef) -> Result<Screenshot, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let result: Result<Screenshot, BridgeError> = async {
            let shot = slot
                .client
                .call_page(
                    &slot.session_id,
                    "Page.captureScreenshot",
                    json!({ "format": "png", "captureBeyondViewport": false }),
                )
                .await?;
            let encoded = shot
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| BridgeError::cdp("Page.captureScreenshot returned no data"))?;
            let bytes = base64_decode(encoded)?;
            artifact::write(&self.policies.artifacts, "screenshot", "png", &bytes)
        }
        .await;
        Self::record(&slot, "take_screenshot", "", started, &result);
        result
    }

    async fn extract(&self, page: &PageRef) -> Result<Extracted, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let result: Result<Extracted, BridgeError> = async {
            let read = slot
                .client
                .call_page(
                    &slot.session_id,
                    "Runtime.evaluate",
                    json!({ "expression": READ_PAGE, "returnByValue": true }),
                )
                .await?;
            let (url, title, text) = parse_read(&read);
            Ok(artifact::extracted(
                &PageState {
                    url,
                    title,
                    text,
                    ..PageState::default()
                },
                SnapshotRequest::new().budget(),
            ))
        }
        .await;
        Self::record(&slot, "extract", "", started, &result);
        result
    }

    async fn close(&self, page: &PageRef) -> Result<(), BridgeError> {
        let Some(slot) = self
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(page.id())
        else {
            return Ok(());
        };
        self.budget.charge(slot.opened_at.elapsed());
        // The engine may be shared with other agents: close this page's target
        // and dispose of its context, never `Browser.close`. A target that
        // already went away answers with an error, which is not worth
        // reporting; the socket is ours either way.
        let _ = slot
            .client
            .call("Target.closeTarget", json!({ "targetId": slot.target_id }))
            .await;
        let _ = slot
            .client
            .call(
                "Target.disposeBrowserContext",
                json!({ "browserContextId": slot.context_id }),
            )
            .await;
        if let Some(interceptor) = &slot.interceptor {
            interceptor.abort();
        }
        if let Some(log) = &slot.log {
            log.record("close", "", None, slot.opened_at.elapsed());
        }
        slot.client.close().await
    }

    async fn shutdown(&self) {
        self.close_all_then_stop().await;
    }

    async fn list_webmcp_tools(
        &self,
        page: &PageRef,
        request: &ListWebmcpToolsRequest,
    ) -> Result<WebmcpListing, BridgeError> {
        let slot = self.slot(page)?;
        let raw = Self::evaluate_webmcp(&slot, &WebmcpRequest::listing(request)).await;
        listing_from_evaluation(raw, request)
    }

    async fn call_webmcp_tool(
        &self,
        page: &PageRef,
        request: &CallWebmcpToolRequest,
    ) -> Result<WebmcpCallOutcome, BridgeError> {
        let started = Instant::now();
        let slot = self.slot(page)?;
        let raw = Self::evaluate_webmcp(&slot, &WebmcpRequest::call(request)).await;
        let outcome = call_from_evaluation(raw, request, ChangeSummary::default());
        Self::record(
            &slot,
            "call_webmcp_tool",
            request.name.as_str(),
            started,
            &outcome,
        );
        let outcome = outcome?;
        // The tool ran page code, so the change summary is measured, not
        // assumed (D504 rule 3). A read failure here costs the summary and
        // nothing else: the tool already ran.
        if outcome.executed && Self::resnapshot(&slot, false).await.is_ok() {
            let change = slot
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .change();
            return Ok(WebmcpCallOutcome { change, ..outcome });
        }
        Ok(outcome)
    }
}
