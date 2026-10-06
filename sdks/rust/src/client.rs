//! The `Loams` object: one client with namespaced modules (design §44 §7.1).
//!
//! ```rust,ignore
//! let loams = Loams::connect(TransportOptions::new("https://acme.loams.dev"))?
//!     .with_auth(Arc::new(ApiKey::new(std::env::var("LOAMS_API_KEY")?)?));
//! let info = loams.instance().get_instance(GetInstanceRequest::default(), CallOptions::new()).await?;
//! let mut transitions = loams.live().watch(request, CallOptions::new()).await?;
//! while let Some(transition) = transitions.next().await { let t = transition?; }
//! ```
//!
//! What is generated and what is hand-written, once more, because it decides
//! where a change goes. The **module surface** is generated: [`crate::facade`]
//! has one module per annotated service and one call per `FacadeOptions` entry,
//! with the retry class, the streaming shape and the RPC path read off the
//! generated `Spec`s. The **runtime** behind those calls is hand-written, once, in
//! the other modules of this crate: transport, credentials, retry, errors,
//! tokens, consistency, pagination, streams. This file is the thin join.
//!
//! ## Why the modules are methods rather than a table
//!
//! Every other SDK's facade is a **table** of `(module, call)` names, because
//! JavaScript and Python have no compile-time knowledge of the generated types.
//! Rust does: `loams.instance().get_instance(..)` is checked by `rustc` against
//! `InstanceServiceClient`, and a call that does not exist is a compile error
//! rather than a runtime one. So the SDK exposes typed methods *and* keeps the
//! generated table — [`crate::facade::MODULES`] — for the three things a table is
//! actually for: the catalogue, [`System::guard`], and the by-name lookup
//! [`Loams::binding`].

use std::sync::{Arc, Mutex};

use buffa::view::OwnedView;
use connectrpc::client::{ClientConfig, ClientTransport, HttpClient, UnaryResponse};
use connectrpc::http_body::Body;
use futures::Stream;
use loams_live_proto::loams::live::v1::{
    DeployRequest, DeployResponse, LiveServiceClient, ModifyQuerySetRequest,
    ModifyQuerySetResponse, MutateRequest, MutateResponse, QueryRequest, QueryResponse, Transition,
    TransitionView, WatchRequest,
};
use loams_proto::loams::instance::v1::{
    GetInstanceRequest, GetInstanceResponse, InstanceServiceClient, WhoAmIRequest, WhoAmIResponse,
};

use crate::call::{CallOptionsOverrides, RetryPlan, call_with_retry};
use crate::error::LoamsError;
use crate::facade::{self, CallBinding};
use crate::request::{Consistency, ConsistencySession};
use crate::retry::{DEFAULT_MAX_RETRIES, RetryClass};
use crate::streams::{self, Opened, WatchOptions};
use crate::system::System;
use crate::token::TokenSource;
use crate::transport::{TransportOptions, transport};

/// The parts of a client a call does not need, kept together so every module
/// method builds its retry plan the same way.
#[derive(Debug, Clone)]
pub(crate) struct Runtime {
    pub(crate) token_source: Option<Arc<dyn TokenSource>>,
    pub(crate) max_retries: u32,
    pub(crate) consistency: Option<Arc<ConsistencySession>>,
    pub(crate) default_timeout: Option<std::time::Duration>,
    pub(crate) catalogue_cache: Arc<Mutex<crate::system::Cache>>,
}

impl Runtime {
    /// The retry plan one call runs under, with everything the runtime owns
    /// already resolved (R1, R2, R4).
    ///
    /// `retry_safe` is `None` here: the class comes from the **binding**, and
    /// [`Inner::unary`] fills it in once it knows whether the request ended up
    /// keyed.
    pub(crate) fn plan(
        &self,
        overrides: &CallOptionsOverrides,
        binding: &'static CallBinding,
        keyed: bool,
    ) -> RetryPlan {
        let session = overrides
            .session
            .clone()
            .or_else(|| self.consistency.clone());
        let consistency = Consistency::resolve(overrides.consistency.as_ref(), session.as_deref());
        RetryPlan {
            // D610: a `Safe` call always retries; a `Manual` one retries once it
            // carries an idempotency key, and `keyed` is `true` exactly then.
            retry_safe: overrides
                .retry_safe
                .unwrap_or(binding.retry_class() == RetryClass::Safe || keyed),
            max_retries: overrides.max_retries.unwrap_or(self.max_retries),
            token_source: self.token_source.clone(),
            headers: overrides.headers.clone(),
            consistency: consistency.header_value(),
            session: session.clone(),
            timeout: overrides.timeout.or(self.default_timeout),
            random: RetryPlan::jitter(),
        }
    }
}

#[derive(Debug, Clone)]
struct Inner<T> {
    transport: T,
    config: ClientConfig,
    endpoint: String,
    runtime: Runtime,
}

/// One SDK, over one instance.
///
/// Generic over the transport, so a caller can supply one — a pinned-certificate
/// connector, a sidecar, a mock in a test — with [`Loams::with_transport`]
/// instead of [`Loams::connect`]. `Loams` is cheap to clone: every clone shares
/// one transport, one configuration and one session consistency store.
#[derive(Debug, Clone)]
pub struct Loams<T> {
    inner: Arc<Inner<T>>,
}

impl Loams<HttpClient> {
    /// Builds a client for an endpoint, over the transport
    /// [`TransportOptions`] names.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the endpoint is not a usable base URL, or
    /// when it is `https://` and the `tls` feature is off.
    pub fn connect(options: TransportOptions) -> Result<Self, LoamsError> {
        let config = options.config()?;
        let transport = transport(&options)?;
        Loams::with_transport_and_config(transport, config, options.endpoint.clone())
    }

    /// Builds a client for an endpoint with the SDK's defaults: no credential, a
    /// retry budget of [`DEFAULT_MAX_RETRIES`], and no session consistency store.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the endpoint is not usable.
    pub fn to(endpoint: &str) -> Result<Self, LoamsError> {
        Loams::connect(TransportOptions::new(endpoint))
    }
}

impl<T: Clone> Loams<T> {
    /// Builds a client over a transport the caller supplies.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the endpoint is not a usable base URL.
    pub fn with_transport(transport: T, options: TransportOptions) -> Result<Self, LoamsError> {
        let config = options.config()?;
        Loams::with_transport_and_config(transport, config, options.endpoint.clone())
    }

    fn with_transport_and_config(
        transport: T,
        config: ClientConfig,
        endpoint: String,
    ) -> Result<Self, LoamsError> {
        let runtime = Runtime {
            token_source: None,
            max_retries: DEFAULT_MAX_RETRIES,
            consistency: None,
            default_timeout: config.default_timeout(),
            catalogue_cache: Arc::new(Mutex::new(crate::system::Cache::default())),
        };
        Ok(Loams {
            inner: Arc::new(Inner {
                transport,
                config,
                endpoint,
                runtime,
            }),
        })
    }

    /// Returns a client that authenticates with `source`.
    #[must_use]
    pub fn with_auth(self, source: Arc<dyn TokenSource>) -> Self {
        let mut runtime = self.inner.runtime.clone();
        runtime.token_source = Some(source);
        self.with_runtime(runtime)
    }

    /// Returns a client with a different retry budget.
    #[must_use]
    pub fn max_retries(self, retries: u32) -> Self {
        let mut runtime = self.inner.runtime.clone();
        runtime.max_retries = retries;
        self.with_runtime(runtime)
    }

    /// Returns a client holding a session consistency token across calls (D609).
    ///
    /// Off by default: every read is then `STRONG` on its own, which is correct
    /// but does not give read-your-writes across calls.
    #[must_use]
    pub fn session_consistency(self, on: bool) -> Self {
        let mut runtime = self.inner.runtime.clone();
        runtime.consistency = on.then(|| Arc::new(ConsistencySession::new()));
        self.with_runtime(runtime)
    }

    fn with_runtime(self, runtime: Runtime) -> Self {
        let inner = Inner {
            transport: self.inner.transport.clone(),
            config: self.inner.config.clone(),
            endpoint: self.inner.endpoint.clone(),
            runtime,
        };
        Loams {
            inner: Arc::new(inner),
        }
    }

    /// The endpoint this client talks to.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    /// The session consistency store, when the client has one (D609).
    #[must_use]
    pub fn consistency(&self) -> Option<&Arc<ConsistencySession>> {
        self.inner.runtime.consistency.as_ref()
    }

    /// The proto revision this SDK declares (§44 §10.3).
    #[must_use]
    pub fn proto_rev(&self) -> &'static str {
        facade::PROTO_REV
    }

    /// The proto packages this SDK speaks.
    #[must_use]
    pub fn proto_packages(&self) -> &'static [&'static str] {
        facade::PROTO_PACKAGES
    }

    /// The generated module bindings, as the SDK sees them.
    #[must_use]
    pub fn bindings(&self) -> &'static [facade::ModuleBinding] {
        facade::MODULES
    }

    /// The binding a module and call name identify.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] naming the module and call, which is what a
    /// by-name lookup reports when the generator has no such row.
    pub fn binding(&self, module: &str, call: &str) -> Result<&'static CallBinding, LoamsError> {
        crate::binding::binding_of(module, call).ok_or_else(|| {
            LoamsError::internal(format!("loams.{module} has no generated call {call}"))
        })
    }
}

impl<T> Loams<T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    /// `loams.instance` — what this instance is, and who the caller is on it.
    #[must_use]
    pub fn instance(&self) -> InstanceModule<'_, T> {
        InstanceModule {
            client: InstanceServiceClient::new(
                self.inner.transport.clone(),
                self.inner.config.clone(),
            ),
            runtime: &self.inner.runtime,
        }
    }

    /// `loams.live` — the live sync **session** half. Its package is `unstable`,
    /// so the SDK marks it experimental (§44 §10.3).
    #[must_use]
    pub fn live(&self) -> LiveModule<'_, T> {
        LiveModule {
            client: LiveServiceClient::new(self.inner.transport.clone(), self.inner.config.clone()),
            runtime: &self.inner.runtime,
        }
    }

    /// `loams.tables` — the **table** half of the same service (design §44
    /// §7.2). Two facade names for one set of RPCs.
    #[must_use]
    pub fn tables(&self) -> TablesModule<'_, T> {
        TablesModule {
            client: LiveServiceClient::new(self.inner.transport.clone(), self.inner.config.clone()),
            runtime: &self.inner.runtime,
        }
    }

    /// `loams.system` — the module catalogue, feature detection and the version
    /// check (R5, R9).
    #[must_use]
    pub fn system(&self) -> System {
        let transport = self.inner.transport.clone();
        let config = self.inner.config.clone();
        let runtime = self.inner.runtime.clone();
        let cache = self.inner.runtime.catalogue_cache.clone();
        let endpoint = self.inner.endpoint.clone();
        System::new(
            Arc::new(move || {
                let transport = transport.clone();
                let config = config.clone();
                let runtime = runtime.clone();
                Box::pin(async move {
                    let client = InstanceServiceClient::new(transport, config);
                    let binding =
                        crate::binding::binding_of("instance", "get_instance").expect("generated");
                    let rpc = binding.rpc;
                    let plan = runtime.plan(&CallOptionsOverrides::new(), binding, false);
                    call_with_retry(
                        GetInstanceRequest::default(),
                        |request, attempt| {
                            client.get_instance_with_options(request, attempt.options)
                        },
                        &plan,
                        rpc,
                    )
                    .await
                    .map(connectrpc::client::UnaryResponse::into_owned)
                }) as crate::system::GetInstanceFuture
            }),
            endpoint,
            cache,
        )
    }

    /// `loams.stream`: a server stream that reconnects from its cursor (R7).
    ///
    /// The cursor-resuming form of `loams.live().watch(..)`. The stream's own
    /// protocol decides what to do with a cursor, so the caller supplies `resume`
    /// and `cursor`: for `loams.live`, `cursor` reads the `Transition`'s end
    /// `StateVersion` and `resume` builds a `WatchRequest.resume` from it.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the **first** open is refused — which is
    /// what a caller sees today, since every `LiveService` RPC answers
    /// `feature_not_in_variant` in every build variant (R5). Later failures are
    /// items of the stream, because by then the caller has already committed to
    /// iterating it.
    pub async fn watch(
        &self,
        request: WatchRequest,
        options: WatchOptions<WatchRequest, Transition>,
    ) -> Result<impl Stream<Item = Result<Transition, LoamsError>>, LoamsError> {
        let binding = crate::binding::binding_of("live", "watch").expect("generated");
        let rpc = binding.rpc;
        let runtime = self.inner.runtime.clone();
        let client = self.live().client.clone();
        let mut call_options = connectrpc::client::CallOptions::default();
        if let Some(timeout) = runtime.default_timeout {
            call_options = call_options.with_timeout(timeout);
        }
        let opener_options = call_options.clone();
        let opener_client = client.clone();
        let opened = call_with_retry(
            request.clone(),
            |request, attempt| {
                let client = opener_client.clone();
                async move {
                    client
                        .watch_with_options(request, attempt.options)
                        .await
                        .map(opened_transitions)
                }
            },
            &runtime.plan(&CallOptionsOverrides::new(), binding, false),
            rpc,
        )
        .await?;
        // The first open has happened, and its handle is already the one this
        // stream will read first. Handing it to `streams::watch` through a slot
        // means the runtime does not open the stream twice, and a refusal of the
        // first open is the `Result` above rather than the first item.
        let first: Arc<std::sync::Mutex<Option<Opened<Transition>>>> =
            Arc::new(std::sync::Mutex::new(Some(opened)));
        Ok(streams::watch(
            move |request: WatchRequest| {
                let client = client.clone();
                let options = opener_options.clone();
                let first = Arc::clone(&first);
                Box::pin(async move {
                    if let Some(opened) = first.lock().expect("not poisoned").take() {
                        return Ok(opened);
                    }
                    client
                        .watch_with_options(request, options)
                        .await
                        .map(opened_transitions)
                })
                    as std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<Opened<Transition>, connectrpc::ConnectError>,
                                > + Send,
                        >,
                    >
            },
            request,
            options,
            crate::WATCH_RPC,
        ))
    }
}

/// `loams.instance`.
#[derive(Clone)]
pub struct InstanceModule<'a, T> {
    client: InstanceServiceClient<T>,
    runtime: &'a Runtime,
}

impl<T> std::fmt::Debug for InstanceModule<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceModule").finish_non_exhaustive()
    }
}

impl<T> InstanceModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    /// `loams.instance.v1.InstanceService/GetInstance`: what this instance is and
    /// how to sign in to it. No auth, and the first thing any client calls.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] carrying the server's `reason` when the call
    /// fails.
    pub async fn get_instance(
        &self,
        request: GetInstanceRequest,
        options: CallOptionsOverrides,
    ) -> Result<GetInstanceResponse, LoamsError> {
        let binding = crate::binding::binding_of("instance", "get_instance").expect("generated");
        let client = &self.client;
        send(
            binding,
            request,
            false,
            options,
            self.runtime,
            |request, opts| client.get_instance_with_options(request, opts),
        )
        .await
    }

    /// `loams.instance.v1.InstanceService/WhoAmI`: the calling principal.
    ///
    /// `not_implemented` on every build today — there is no authentication yet —
    /// so this is here for the contract rather than for a working call.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] with `reason = not_implemented` today.
    pub async fn who_am_i(
        &self,
        request: WhoAmIRequest,
        options: CallOptionsOverrides,
    ) -> Result<WhoAmIResponse, LoamsError> {
        let binding = crate::binding::binding_of("instance", "who_am_i").expect("generated");
        let client = &self.client;
        send(
            binding,
            request,
            false,
            options,
            self.runtime,
            |request, opts| client.who_am_i_with_options(request, opts),
        )
        .await
    }
}

/// `loams.live` — the session half.
#[derive(Clone)]
pub struct LiveModule<'a, T> {
    client: LiveServiceClient<T>,
    runtime: &'a Runtime,
}

impl<T> std::fmt::Debug for LiveModule<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveModule").finish_non_exhaustive()
    }
}

impl<T> LiveModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    /// `loams.live.v1.LiveService/Watch`: the server stream (R7).
    ///
    /// The returned stream is the **raw** one: the errors are mapped, but there is
    /// no cursor resume. [`Loams::watch`] wraps this with the resume policy, which
    /// is what a caller normally wants; the raw form is here for a caller that
    /// owns its own reconnect logic.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the stream cannot be opened. Every
    /// `LiveService` RPC answers `feature_not_in_variant` today (R5), which is
    /// the interesting case: the refusal arrives inside the Connect streaming
    /// envelope, not as an HTTP status, so a client that reads only status codes
    /// sees a 200 here.
    pub async fn watch(
        &self,
        request: WatchRequest,
        options: CallOptionsOverrides,
    ) -> Result<Opened<Transition>, LoamsError> {
        let binding = crate::binding::binding_of("live", "watch").expect("generated");
        let rpc = binding.rpc;
        let plan = self.runtime.plan(&options, binding, false);
        let client = self.client.clone();
        call_with_retry(
            request,
            |request, attempt| {
                let client = client.clone();
                async move {
                    client
                        .watch_with_options(request, attempt.options)
                        .await
                        .map(opened_transitions)
                }
            },
            &plan,
            rpc,
        )
        .await
    }

    /// `loams.live.v1.LiveService/ModifyQuerySet`: adds and removes queries in an
    /// open session.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] with `reason = feature_not_in_variant` today.
    pub async fn modify_query_set(
        &self,
        request: ModifyQuerySetRequest,
        options: CallOptionsOverrides,
    ) -> Result<ModifyQuerySetResponse, LoamsError> {
        let binding = crate::binding::binding_of("live", "modify_query_set").expect("generated");
        let client = &self.client;
        send(
            binding,
            request,
            false,
            options,
            self.runtime,
            |request, opts| client.modify_query_set_with_options(request, opts),
        )
        .await
    }
}

/// `loams.tables` — the table half of the same service (design §44 §7.2).
#[derive(Clone)]
pub struct TablesModule<'a, T> {
    client: LiveServiceClient<T>,
    runtime: &'a Runtime,
}

impl<T> std::fmt::Debug for TablesModule<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TablesModule").finish_non_exhaustive()
    }
}

impl<T> TablesModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    /// `loams.live.v1.LiveService/Query`: a one-shot query.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] with `reason = feature_not_in_variant` today.
    pub async fn query(
        &self,
        request: QueryRequest,
        options: CallOptionsOverrides,
    ) -> Result<QueryResponse, LoamsError> {
        let binding = crate::binding::binding_of("tables", "query").expect("generated");
        let client = &self.client;
        send(
            binding,
            request,
            false,
            options,
            self.runtime,
            |request, opts| client.query_with_options(request, opts),
        )
        .await
    }

    /// `loams.live.v1.LiveService/Mutate`: runs a mutation.
    ///
    /// The one **keyed** mutation in the API today: `MutateRequest` declares
    /// `idempotency_key`, so the SDK mints one UUIDv7 per logical call and reuses
    /// it on every retry, which is what makes the call retryable at all (R3).
    /// Supply your own with [`CallOptionsOverrides::idempotency_key`] to make the
    /// retry yours.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] with `reason = feature_not_in_variant` today.
    pub async fn mutate(
        &self,
        request: MutateRequest,
        options: CallOptionsOverrides,
    ) -> Result<MutateResponse, LoamsError> {
        let binding = crate::binding::binding_of("tables", "mutate").expect("generated");
        // R3: the key is decided **before** the first attempt, and the request
        // that goes out on every attempt is this one, key included.
        let keyed = crate::request::with_idempotency_key(
            request,
            options.idempotency_key.as_deref(),
            |request: &MutateRequest| request.idempotency_key.clone(),
            |request: &mut MutateRequest, key| request.idempotency_key = Some(key),
        );
        let client = &self.client;
        send(
            binding,
            keyed.request,
            keyed.keyed,
            options,
            self.runtime,
            |request, opts| client.mutate_with_options(request, opts),
        )
        .await
    }

    /// `loams.live.v1.LiveService/Deploy`: admin, deploys a function bundle and a
    /// schema.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] with `reason = feature_not_in_variant` today.
    pub async fn deploy(
        &self,
        request: DeployRequest,
        options: CallOptionsOverrides,
    ) -> Result<DeployResponse, LoamsError> {
        let binding = crate::binding::binding_of("tables", "deploy").expect("generated");
        let client = &self.client;
        send(
            binding,
            request,
            false,
            options,
            self.runtime,
            |request, opts| client.deploy_with_options(request, opts),
        )
        .await
    }
}

/// One unary call through the runtime, returning the owned message.
///
/// The shared tail of every module method: the retry loop, the owned-message
/// conversion, and nothing else. The generated client's return type is
/// `UnaryResponse<OwnedView<View>>`, so this is where the zero-copy view becomes
/// a message a caller can store.
///
/// `keyed` says whether the request already carries this call's idempotency key,
/// which is what promotes a mutation to `RetryClass::Safe` for the retry loop
/// (R3, D610). The key is decided **before** the first attempt and lives inside
/// the request, so every attempt carries the identical one.
async fn send<Req, V, F, Fut>(
    binding: &'static CallBinding,
    request: Req,
    keyed: bool,
    options: CallOptionsOverrides,
    runtime: &Runtime,
    mut send_one: F,
) -> Result<V::Owned, LoamsError>
where
    Req: Clone,
    V: buffa::view::MessageView<'static>,
    F: FnMut(Req, connectrpc::client::CallOptions) -> Fut,
    Fut:
        std::future::Future<Output = Result<UnaryResponse<OwnedView<V>>, connectrpc::ConnectError>>,
{
    let rpc = binding.rpc;
    let plan = runtime.plan(&options, binding, keyed);
    let response = call_with_retry(
        request,
        |request, attempt| send_one(request, attempt.options),
        &plan,
        rpc,
    )
    .await?;
    Ok(response.into_owned())
}

/// Turns connect-rust's stream handle into the [`Opened`] form the runtime
/// resumes, yielding owned messages.
fn opened_transitions<B>(
    stream: connectrpc::client::ServerStream<B, TransitionView<'static>>,
) -> Opened<Transition>
where
    B: Body<Data = bytes::Bytes> + Unpin + Send + 'static,
    B::Error: std::fmt::Display,
{
    Box::pin(futures::stream::unfold(Some(stream), |maybe| async move {
        let mut stream = maybe?;
        match stream.message::<Transition>().await {
            Ok(Some(message)) => Some((Ok(message.to_owned_message()), Some(stream))),
            // A clean end is `None` from `unfold`: the stream is over, not failed.
            Ok(None) => None,
            // A terminal error ends the stream: the handle is dropped, so the
            // runtime's re-open is the only way to continue (R7).
            Err(error) => Some((Err(error), None)),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportOptions;

    fn client() -> Loams<HttpClient> {
        Loams::with_transport(
            HttpClient::plaintext(),
            TransportOptions::new("http://127.0.0.1:8080"),
        )
        .expect("a base URL")
    }

    #[test]
    fn the_client_reports_the_proto_revision_and_its_packages() {
        // A client over a transport that is never used: the constructor is the
        // only thing under test, and it must not need a server.
        let loams = client();
        assert_eq!(loams.proto_rev(), facade::PROTO_REV);
        assert!(loams.proto_packages().contains(&"loams.instance.v1"));
        assert_eq!(loams.endpoint(), "http://127.0.0.1:8080");
        assert!(loams.consistency().is_none(), "off by default (D609)");
        assert_eq!(loams.inner.runtime.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(loams.bindings().len(), 3, "instance, live and tables");
    }

    #[test]
    fn a_session_consistency_store_is_built_only_when_asked_for() {
        let loams = client();
        assert!(
            loams
                .clone()
                .session_consistency(false)
                .consistency()
                .is_none()
        );
        let with_session = loams.clone().session_consistency(true);
        assert!(with_session.consistency().is_some());
        // The original is untouched: a clone is a value, not a mutation.
        assert!(loams.consistency().is_none());
        assert_eq!(loams.clone().max_retries(0).inner.runtime.max_retries, 0);
        assert_eq!(loams.inner.runtime.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn a_cloned_client_shares_one_session_store() {
        let loams = client().session_consistency(true);
        let clone = loams.clone();
        loams.consistency().expect("a session").record(Some("v1:a"));
        assert_eq!(
            clone
                .consistency()
                .expect("the same session")
                .current()
                .as_deref(),
            Some("v1:a")
        );
    }

    #[test]
    fn a_call_the_generator_has_no_row_for_is_reported_by_name() {
        let loams = client();
        let binding = loams
            .binding("instance", "get_instance")
            .expect("generated");
        assert_eq!(binding.rpc, "loams.instance.v1.InstanceService/GetInstance");
        let missing = loams
            .binding("collections", "list_collections")
            .unwrap_err();
        assert!(
            missing.to_string().contains("no generated call"),
            "{missing}"
        );
        assert!(loams.binding("nope", "nope").is_err());
    }
}
