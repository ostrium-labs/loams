//! The Spark Connect proxy (SP-a D1992–D1995; design 68 §7.4, design 81
//! R8, R15).
//!
//! A tower service for `/spark.connect.SparkConnectService/*` that, per call:
//!
//! 1. takes the credential from the metadata and authenticates it
//!    ([`crate::auth`]): the principal's Loams namespace is the tenant;
//! 2. checks `can_connect`-style access with the [`Authorizer`] (R18:
//!    `can_read` on `environment:<ns>` until `engine` tuples exist);
//! 3. resolves the namespace's instance ([`crate::route`]), refusing a
//!    hostname of another namespace;
//! 4. reads the first request message's `session_id`, applies the session
//!    and operation limits ([`crate::sessions`]) and pins
//!    `user_context.user_id` to the principal ([`crate::wire`]);
//! 5. forwards the call as bytes, with the client's credentials removed and
//!    the backend's own pre-shared token added, and streams the answer back.
//!
//! Every Spark Connect method is forwarded the same way: on the wire a unary
//! call is a stream of one message, so one bidirectional forwarder serves
//! unary, server-streaming (`ExecutePlan`, `ReattachExecute`) and
//! client-streaming (`AddArtifacts`) methods alike. Loams parses no plan.
//!
//! [`SparkConnectProxy::run_sweeper`] releases idle sessions on their
//! backend and reports idle instances to the [`LifecycleHook`].

use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};
use http::uri::PathAndQuery;
use loams_authz::{Authorizer, CheckRequest, ObjectRef};
use prost::Message as _;
use serde::Deserialize;
use tokio::time::Instant;
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::server::{NamedService, StreamingService};
use tonic::{Request, Response, Status, Streaming};

use crate::auth::{self, API_KEY_HEADER, Authenticator, Principal};
use crate::proto::{self, connect};
use crate::route::{Backend, HostPattern, Router};
use crate::sessions::{CallKind, Limits, SessionInfo, SessionTable};
use crate::wire::{self, CloneSessionHeader, CloneSessionRequestHeader, RawCodec};

/// Spark's own default for `spark.connect.grpc.maxInboundMessageSize` is
/// 128 MiB; the proxy allows the same both ways.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

/// The access check the proxy makes after routing (R18).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessCheck {
    /// The relation, `can_read` until the `engine` type has `can_connect`.
    #[serde(default = "default_relation")]
    pub relation: String,
    /// `environment` (object id = the namespace) or `engine` (object id =
    /// the instance id).
    #[serde(default = "default_object_type")]
    pub object_type: String,
}

fn default_relation() -> String {
    "can_read".into()
}
fn default_object_type() -> String {
    "environment".into()
}

impl Default for AccessCheck {
    fn default() -> Self {
        Self {
            relation: default_relation(),
            object_type: default_object_type(),
        }
    }
}

/// The proxy's settings.
#[derive(Debug, Clone)]
pub struct ProxySettings {
    /// How hostnames name instances.
    pub hosts: HostPattern,
    /// Session and operation limits.
    pub limits: Limits,
    /// The access check.
    pub access: AccessCheck,
    /// The largest message either way.
    pub max_message_bytes: usize,
    /// How often the sweeper runs.
    pub sweep_interval: Duration,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            hosts: HostPattern {
                suffix: String::new(),
            },
            limits: Limits::default(),
            access: AccessCheck::default(),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            sweep_interval: Duration::from_secs(30),
        }
    }
}

/// What the proxy tells the lifecycle (D1995). The engine adapter implements
/// it to suspend an idle instance (scale to zero).
#[async_trait]
pub trait LifecycleHook: Send + Sync + 'static {
    /// The instance has had no session and no call for the instance idle
    /// timeout.
    async fn instance_idle(&self, namespace: &str, instance_id: &str);
}

/// A hook that does nothing (compose: the instance always runs).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLifecycle;

#[async_trait]
impl LifecycleHook for NoLifecycle {
    async fn instance_idle(&self, _: &str, _: &str) {}
}

struct Shared {
    auth: Arc<dyn Authenticator>,
    authz: Arc<dyn Authorizer>,
    router: Arc<dyn Router>,
    hook: Arc<dyn LifecycleHook>,
    sessions: Arc<SessionTable>,
    settings: ProxySettings,
}

/// The Spark Connect proxy service. Cheap to clone.
#[derive(Clone)]
pub struct SparkConnectProxy {
    shared: Arc<Shared>,
}

impl fmt::Debug for SparkConnectProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SparkConnectProxy")
            .field("settings", &self.shared.settings)
            .finish_non_exhaustive()
    }
}

impl SparkConnectProxy {
    /// A proxy over the given seams.
    pub fn new(
        auth: Arc<dyn Authenticator>,
        authz: Arc<dyn Authorizer>,
        router: Arc<dyn Router>,
        hook: Arc<dyn LifecycleHook>,
        settings: ProxySettings,
    ) -> Self {
        let sessions = SessionTable::new(settings.limits);
        Self {
            shared: Arc::new(Shared {
                auth,
                authz,
                router,
                hook,
                sessions,
                settings,
            }),
        }
    }

    /// The session table (for `SparkEngine::list_sessions`).
    pub fn sessions(&self) -> &Arc<SessionTable> {
        &self.shared.sessions
    }

    /// The open sessions of a namespace.
    pub fn list_sessions(&self, namespace: &str) -> Vec<SessionInfo> {
        self.shared.sessions.sessions(namespace, Instant::now())
    }

    /// Releases a session on its backend and forgets it (the facade's
    /// `DELETE …/sessions/{s}`).
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` when the namespace has no such session.
    pub async fn close_session(&self, namespace: &str, session_id: &str) -> Result<(), Status> {
        let found: Vec<SessionInfo> = self
            .list_sessions(namespace)
            .into_iter()
            .filter(|s| s.session_id == session_id)
            .collect();
        if found.is_empty() {
            return Err(Status::not_found("no such Spark session"));
        }
        for s in found {
            self.shared
                .release_on_backend(&s.instance_id, &s.session_id, &s.user_id)
                .await;
            self.shared
                .sessions
                .close(namespace, &s.instance_id, &s.session_id);
        }
        Ok(())
    }

    /// One sweep: releases idle sessions, reports idle instances.
    pub async fn sweep(&self) {
        let now = Instant::now();
        for r in self.shared.sessions.reap(now) {
            tracing::info!(
                namespace = %r.namespace,
                instance = %r.instance_id,
                "releasing an idle Spark session"
            );
            self.shared
                .release_on_backend(&r.instance_id, &r.session_id, &r.user_id)
                .await;
        }
        for (namespace, instance_id) in self.shared.sessions.idle_instances(now) {
            tracing::info!(%namespace, instance = %instance_id, "Spark instance idle");
            self.shared
                .hook
                .instance_idle(&namespace, &instance_id)
                .await;
        }
    }

    /// Runs [`Self::sweep`] every `sweep_interval` until `shutdown`.
    pub async fn run_sweeper(self, shutdown: impl std::future::Future<Output = ()> + Send) {
        let mut tick = tokio::time::interval(self.shared.settings.sweep_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => return,
                _ = tick.tick() => self.sweep().await,
            }
        }
    }
}

impl NamedService for SparkConnectProxy {
    const NAME: &'static str = proto::SERVICE;
}

/// What the HTTP layer knew about a call, handed to the forwarder.
#[derive(Debug, Clone)]
struct CallContext {
    method: String,
    authority: Option<String>,
}

impl tower::Service<http::Request<tonic::body::Body>> for SparkConnectProxy {
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: http::Request<tonic::body::Body>) -> Self::Future {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            let Some(method) = req
                .uri()
                .path()
                .strip_prefix(proto::PATH_PREFIX)
                .filter(|m| !m.is_empty() && !m.contains('/'))
                .map(str::to_owned)
            else {
                return Ok(Status::unimplemented("not a Spark Connect method").into_http());
            };
            let authority = req
                .uri()
                .authority()
                .map(|a| a.as_str().to_owned())
                .or_else(|| {
                    req.headers()
                        .get(http::header::HOST)
                        .and_then(|h| h.to_str().ok())
                        .map(str::to_owned)
                });
            req.extensions_mut()
                .insert(CallContext { method, authority });
            let max = shared.settings.max_message_bytes;
            let mut grpc = tonic::server::Grpc::new(RawCodec)
                .max_decoding_message_size(max)
                .max_encoding_message_size(max);
            Ok(grpc.streaming(Forward { shared }, req).await)
        })
    }
}

struct Forward {
    shared: Arc<Shared>,
}

impl StreamingService<Bytes> for Forward {
    type Response = Bytes;
    type ResponseStream = BoxStream<'static, Result<Bytes, Status>>;
    type Future = BoxFuture<'static, Result<Response<Self::ResponseStream>, Status>>;

    fn call(&mut self, request: Request<Streaming<Bytes>>) -> Self::Future {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move { shared.forward(request).await })
    }
}

/// Request headers never forwarded: the client's credentials, and what the
/// gRPC client sets itself.
const DROP_REQUEST: &[&str] = &[
    "authorization",
    API_KEY_HEADER,
    "host",
    "te",
    "content-type",
    "content-length",
    "user-agent",
    "grpc-encoding",
    "grpc-accept-encoding",
];

/// Response headers never passed back (the server layer sets them).
const DROP_RESPONSE: &[&str] = &[
    "content-type",
    "content-length",
    "grpc-encoding",
    "grpc-accept-encoding",
    "grpc-status",
    "grpc-message",
];

fn filter(md: MetadataMap, drop: &[&str]) -> MetadataMap {
    let mut headers = md.into_headers();
    for name in drop {
        headers.remove(*name);
    }
    MetadataMap::from_headers(headers)
}

/// The metadata the backend gets: the client's, minus its credentials, plus
/// the backend's pre-shared token.
fn backend_metadata(client: MetadataMap, backend: &Backend) -> Result<MetadataMap, Status> {
    let mut md = filter(client, DROP_REQUEST);
    if let Some(token) = &backend.token {
        let value = MetadataValue::try_from(format!("Bearer {token}"))
            .map_err(|_| Status::internal("the backend token is not a valid header"))?;
        md.insert("authorization", value);
    }
    Ok(md)
}

impl Shared {
    async fn forward(
        self: Arc<Self>,
        request: Request<Streaming<Bytes>>,
    ) -> Result<Response<BoxStream<'static, Result<Bytes, Status>>>, Status> {
        let (metadata, extensions, mut inbound) = request.into_parts();
        let ctx = extensions
            .get::<CallContext>()
            .cloned()
            .ok_or_else(|| Status::internal("call context missing"))?;

        // 1. Who.
        let credential = auth::credential(&metadata)?;
        let principal = self.auth.authenticate(&credential).await?;

        // 2. Where.
        let target = self
            .settings
            .hosts
            .target(&principal.namespace, ctx.authority.as_deref())?;
        let backend = self.router.resolve(&target).await?;

        // 3. May they.
        self.check_access(&principal, &backend).await?;

        // 4. Which session; limits; identity.
        let first = inbound
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("the call carried no request message"))?;
        let header = wire::session_header(&first)?;
        let session_id = header.session_id;
        let user_id = principal.spark_user_id();
        if ctx.method == "CloneSession"
            && let Some(new) = CloneSessionRequestHeader::decode(first.as_ref())
                .map_err(|e| Status::invalid_argument(e.to_string()))?
                .new_session_id
        {
            wire::check_session_id(&new)?;
        }
        let kind = if ctx.method == "ReleaseSession" {
            CallKind::Release
        } else {
            CallKind::Use
        };
        let guard = self.sessions.begin(
            &principal.namespace,
            &backend.instance_id,
            &session_id,
            &user_id,
            kind,
            Instant::now(),
        )?;

        // 5. Forward.
        let first = wire::with_user_id(&first, &user_id);
        let rest_user = user_id.clone();
        let rest = inbound
            .take_while(|m| std::future::ready(m.is_ok()))
            .filter_map(move |m| {
                std::future::ready(m.ok().map(|b| wire::with_user_id(&b, &rest_user)))
            });
        let outbound = stream::once(std::future::ready(first)).chain(rest);
        let md = backend_metadata(metadata, &backend)?;

        let max = self.settings.max_message_bytes;
        let mut client = tonic::client::Grpc::new(backend.channel.clone())
            .max_decoding_message_size(max)
            .max_encoding_message_size(max);
        client
            .ready()
            .await
            .map_err(|e| Status::unavailable(format!("Spark backend unavailable: {e}")))?;
        let path = PathAndQuery::try_from(format!("{}{}", proto::PATH_PREFIX, ctx.method))
            .map_err(|_| Status::invalid_argument("bad method name"))?;
        let response = client
            .streaming(
                Request::from_parts(md, tonic::Extensions::default(), outbound),
                path,
                RawCodec,
            )
            .await?;

        let (resp_md, answers, _) = response.into_parts();
        let after = AfterCall {
            shared: Arc::clone(&self),
            method: ctx.method,
            namespace: principal.namespace.clone(),
            instance_id: backend.instance_id.clone(),
            session_id,
            user_id,
        };
        let answers = answers.map(move |item| {
            // The guard lives as long as the stream: the call is in flight
            // until the last answer or a cancel.
            let _in_flight = &guard;
            if let Ok(bytes) = &item {
                after.on_answer(bytes)?;
            }
            item
        });
        Ok(Response::from_parts(
            filter(resp_md, DROP_RESPONSE),
            answers.boxed(),
            tonic::Extensions::default(),
        ))
    }

    async fn check_access(&self, principal: &Principal, backend: &Backend) -> Result<(), Status> {
        let object_id = if self.settings.access.object_type == "environment" {
            principal.namespace.clone()
        } else {
            backend.instance_id.clone()
        };
        let req = CheckRequest::new(
            principal.subject.clone(),
            self.settings.access.relation.clone(),
            ObjectRef::new(self.settings.access.object_type.clone(), object_id),
        );
        match self.authz.check(req).await {
            Ok(d) if d.allowed => Ok(()),
            Ok(_) => Err(Status::permission_denied(
                "this principal may not use Spark in this namespace",
            )),
            Err(e) => {
                tracing::warn!(error = %e, "authorizer unavailable; denying");
                Err(Status::unavailable("authorization is unavailable"))
            }
        }
    }

    /// Sends `ReleaseSession` to an instance (best effort: a backend that is
    /// gone has no session to keep).
    async fn release_on_backend(&self, instance_id: &str, session_id: &str, user_id: &str) {
        let Some(backend) = self.router.by_instance(instance_id).await else {
            return;
        };
        let mut req = Request::new(connect::ReleaseSessionRequest {
            session_id: session_id.to_owned(),
            user_context: Some(connect::UserContext {
                user_id: user_id.to_owned(),
                user_name: String::new(),
                extensions: Vec::new(),
            }),
            client_type: Some("loams-spark-proxy".into()),
            allow_reconnect: false,
        });
        if let Some(token) = &backend.token
            && let Ok(v) = MetadataValue::try_from(format!("Bearer {token}"))
        {
            req.metadata_mut().insert("authorization", v);
        }
        req.set_timeout(Duration::from_secs(10));
        let mut client =
            connect::spark_connect_service_client::SparkConnectServiceClient::new(backend.channel);
        if let Err(e) = client.release_session(req).await {
            tracing::warn!(instance = %instance_id, code = ?e.code(), "ReleaseSession failed");
        }
    }
}

/// Bookkeeping from answers: a released session is forgotten, a clone is
/// counted.
struct AfterCall {
    shared: Arc<Shared>,
    method: String,
    namespace: String,
    instance_id: String,
    session_id: String,
    user_id: String,
}

impl AfterCall {
    fn on_answer(&self, bytes: &Bytes) -> Result<(), Status> {
        match self.method.as_str() {
            "ReleaseSession" => {
                self.shared
                    .sessions
                    .close(&self.namespace, &self.instance_id, &self.session_id);
                Ok(())
            }
            "CloneSession" => {
                let clone = CloneSessionHeader::decode(bytes.as_ref())
                    .map_err(|e| Status::internal(format!("bad CloneSession answer: {e}")))?;
                if clone.new_session_id.is_empty() {
                    return Ok(());
                }
                let opened = self.shared.sessions.open_clone(
                    &self.namespace,
                    &self.instance_id,
                    &clone.new_session_id,
                    &self.user_id,
                    Instant::now(),
                );
                if let Err(status) = opened {
                    // Over the limit: undo the clone on the backend.
                    let shared = Arc::clone(&self.shared);
                    let (instance, session, user) = (
                        self.instance_id.clone(),
                        clone.new_session_id,
                        self.user_id.clone(),
                    );
                    tokio::spawn(async move {
                        shared.release_on_backend(&instance, &session, &user).await;
                    });
                    return Err(status);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
