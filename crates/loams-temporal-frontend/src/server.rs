//! Wiring: the frontend's parts, its background loops, and the gRPC service
//! that answers `UNIMPLEMENTED` (with the matrix link) for every method
//! outside the DX1 set.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use loams_temporal_matching::Matcher;
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_server::WorkflowServiceServer;
use loams_temporal_resonate::{Durability, Relay};
use loams_temporal_schedule::ScheduleStore;
use loams_temporal_store::Store;
use loams_temporal_visibility::VisibilityStore;
use tonic::Status;
use tonic::body::Body;
use tonic::server::NamedService;

use crate::auth::Authenticator;
use crate::capabilities::{IMPLEMENTED, unimplemented_message};
use crate::effects::CALLBACK_GROUP;
use crate::engine::Engine;
use crate::fastpath::FastPath;
use crate::outbox::StoreOutbox;
use crate::service::{Frontend, FrontendConfig, WorkflowServiceImpl};
use crate::token::TokenKeys;

/// How the frontend is put together.
pub struct Parts {
    /// The history store.
    pub store: Arc<dyn Store>,
    /// Resonate (the fake, the interim adapter, or later `resonate-client`).
    pub durability: Arc<dyn Durability>,
    /// Who may call.
    pub auth: Arc<dyn Authenticator>,
    /// The task-token key ring.
    pub tokens: TokenKeys,
    /// This process's Resonate process ID (claims and outbox ownership).
    pub pid: String,
    /// The store's shard count.
    pub shards: u32,
    /// The lease the matcher takes on each task (kept alive while held).
    pub lease: Duration,
    /// The frontend's settings.
    pub config: FrontendConfig,
    /// The optional stores (defaults: in memory).
    pub ext: PartsExt,
}

/// Stores beside the history store; `Default` keeps them in memory.
#[derive(Default)]
pub struct PartsExt {
    /// Temporal Schedules (D2072); `None` is the memory store.
    pub schedules: Option<Arc<dyn ScheduleStore>>,
    /// Visibility (D2075); `None` is the memory store fed from commits.
    pub visibility: Option<Arc<dyn VisibilityStore>>,
}

impl std::fmt::Debug for PartsExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartsExt")
            .field("schedules", &self.schedules.is_some())
            .field("visibility", &self.visibility.is_some())
            .finish()
    }
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

/// A running frontend: the service to mount and the handle to its loops.
pub struct Gateway {
    /// The frontend.
    pub frontend: Arc<Frontend>,
    /// The relay (for its metrics).
    pub relay: Arc<Relay>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway").finish_non_exhaustive()
    }
}

impl Gateway {
    /// Builds the frontend and starts its loops (relay, lease keeper,
    /// callbacks) on the current runtime until `shutdown` fires.
    pub async fn start(
        parts: Parts,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<Gateway, String> {
        let raw = parts.durability.clone();
        let matcher = Matcher::new(
            raw.clone(),
            parts.pid.clone(),
            parts.lease.as_millis() as i64,
        );
        let fast: Arc<dyn Durability> = Arc::new(FastPath::new(raw.clone(), &matcher));
        let clock_source = raw.clone();
        let engine = Engine::new(parts.store.clone(), Arc::new(move || clock_source.now_ms()));
        let mut frontend = Frontend::new(
            engine,
            matcher.clone(),
            raw.clone(),
            parts.auth,
            parts.tokens,
            parts.config,
        );
        if let Some(s) = parts.ext.schedules {
            frontend = frontend.with_schedule_store(s);
        }
        if let Some(v) = parts.ext.visibility {
            frontend = frontend.with_visibility_store(v);
        }
        frontend.attach_visibility();
        let frontend = Arc::new(frontend);
        let outbox = Arc::new(StoreOutbox::new(
            parts.store,
            parts.shards,
            parts.pid.clone(),
            Duration::from_secs(30),
        ));
        let relay = Arc::new(Relay::new(outbox, fast));

        let stop = |mut rx: tokio::sync::watch::Receiver<bool>| async move {
            let _ = rx.wait_for(|s| *s).await;
        };
        let r = relay.clone();
        let sd = shutdown.clone();
        tokio::spawn(async move { r.run(stop(sd)).await });
        let m = matcher.clone();
        let sd = shutdown.clone();
        tokio::spawn(async move { m.run_leases(stop(sd)).await });

        // Schedules: the overdue sweep, and passes when a scheduled run
        // closes (D2071).
        let f = frontend.clone();
        let mut sd = shutdown.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(crate::schedules::SWEEP_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = sd.wait_for(|s| *s) => return,
                    _ = tick.tick() => f.sweep_schedules().await,
                }
            }
        });
        let f = frontend.clone();
        let mut closed = frontend.engine.closed_runs();
        let mut sd = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sd.wait_for(|s| *s) => return,
                    r = closed.recv() => match r {
                        Ok(key) => {
                            let f = f.clone();
                            tokio::spawn(async move { f.on_run_closed(key).await });
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(missed = n, "schedule close notifications lagged; the re-check timers cover them");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        });
        // Children, continue-as-new and external requests (D2087).
        let (ftx, mut frx) = tokio::sync::mpsc::unbounded_channel();
        frontend.engine.set_followups(ftx);
        let f = frontend.clone();
        let mut sd = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sd.wait_for(|s| *s) => return,
                    m = frx.recv() => match m {
                        Some(fu) => {
                            let f = f.clone();
                            tokio::spawn(async move { f.on_follow_up(fu).await });
                        }
                        None => return,
                    }
                }
            }
        });

        let mut callbacks = raw
            .subscribe(CALLBACK_GROUP)
            .await
            .map_err(|e| format!("subscribing to {CALLBACK_GROUP}: {e}"))?;
        let f = frontend.clone();
        let mut sd = shutdown;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sd.wait_for(|s| *s) => return,
                    d = callbacks.recv() => match d {
                        Some(d) => {
                            let f = f.clone();
                            tokio::spawn(async move { f.on_delivery(d).await });
                        }
                        None => return,
                    }
                }
            }
        });
        Ok(Gateway { frontend, relay })
    }

    /// The gRPC service to mount (`WorkflowService` behind
    /// [`ImplementedOnly`]), with Temporal's 4 MiB message limit (X1.R7).
    pub fn service(&self) -> ImplementedOnly<WorkflowServiceServer<WorkflowServiceImpl>> {
        let limit = loams_temporal_history::Limits::default().grpc_message;
        ImplementedOnly(
            WorkflowServiceServer::new(WorkflowServiceImpl(self.frontend.clone()))
                // The Go SDK sends gzip; answer in kind when asked.
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
                .send_compressed(tonic::codec::CompressionEncoding::Gzip)
                .max_decoding_message_size(limit)
                .max_encoding_message_size(64 * 1024 * 1024),
        )
    }

    /// The `OperatorService` to mount beside [`Gateway::service`] (D2052):
    /// the search-attribute listing, `UNIMPLEMENTED` for the rest.
    pub fn operator_service(
        &self,
    ) -> crate::operator::OperatorImplementedOnly<
        loams_temporal_proto::temporal::api::operatorservice::v1::operator_service_server::OperatorServiceServer<
            crate::operator::OperatorServiceImpl,
        >,
    >{
        crate::operator::operator_service(self.frontend.clone())
    }
}

/// Answers `UNIMPLEMENTED` with the matrix link for every `WorkflowService`
/// method outside [`IMPLEMENTED`], and passes the rest through.
#[derive(Debug, Clone)]
pub struct ImplementedOnly<S>(pub S);

const PREFIX: &str = "/temporal.api.workflowservice.v1.WorkflowService/";

impl<S> tower::Service<http::Request<Body>> for ImplementedOnly<S>
where
    S: tower::Service<http::Request<Body>, Response = http::Response<Body>, Error = Infallible>
        + Send,
    S::Future: Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<Body>) -> Self::Future {
        if let Some(method) = req.uri().path().strip_prefix(PREFIX)
            && !IMPLEMENTED.contains(&method)
        {
            let status = Status::unimplemented(unimplemented_message(method));
            return Box::pin(async move { Ok(status.into_http()) });
        }
        Box::pin(self.0.call(req))
    }
}

impl<S> NamedService for ImplementedOnly<S> {
    const NAME: &'static str = "temporal.api.workflowservice.v1.WorkflowService";
}
