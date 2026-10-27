//! The `loams.agentd.v1` services: one implementation, [`Agentd`], of every
//! generated service trait.
//!
//! Each handler maps its request to the ndjson twin's params ([`crate::map`]),
//! calls the engine ([`crate::engine`]) and maps the twin's result to its
//! response. The twin of every method is in [`crate::twins`]. A §53 §6.5 gap
//! would answer `unimplemented` with reason `not_implemented`; since SD1
//! Task 13 every gap is served.
//!
//! The handlers return the concrete response type where the generated traits
//! promise `impl Encodable<…>`, as `loams-apps-mock` does.
#![allow(refining_impl_trait)]

use std::sync::Arc;

use connectrpc::{ConnectError, Router};
use loams_agentd_proto as rs;
use loams_agentd_proto::daemon::DaemonMode;
use loams_agentd_proto::rpc::calls;

use crate::engine::{self, Engine};

mod approvals;
mod attachments;
mod diff;
mod factory;
mod files;
mod git;
mod groups;
mod harness;
mod marketplace;
mod notifications;
mod projects;
mod queue;
mod review;
mod studio;
mod terminals;
mod threads;

/// What `StudioService.GetHost` reports beside the engine's `EngineInfo`:
/// the daemon's `Hello` facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    /// The ndjson protocol (`AGENTD_PROTOCOL`).
    pub protocol: u32,
    pub version: String,
    pub build_sha: String,
    pub mode: DaemonMode,
    pub pid: u32,
    pub started_at_ms: i64,
    /// For example "studio.v1".
    pub features: Vec<String>,
}

/// Every `loams.agentd.v1` service, over one engine.
#[derive(Clone)]
pub struct Agentd {
    engine: Engine,
    host: Arc<HostInfo>,
}

impl std::fmt::Debug for Agentd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agentd")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl Agentd {
    /// The services over `engine`, the daemon's `RpcService`.
    pub fn new(engine: Engine, host: HostInfo) -> Self {
        Self {
            engine,
            host: Arc::new(host),
        }
    }

    /// A Connect router with every service registered.
    pub fn router(self) -> Router {
        use crate::proto::loams::agentd::v1::{
            ApprovalServiceExt, AttachmentServiceExt, DiffServiceExt, FactoryServiceExt,
            FileServiceExt, GitServiceExt, HarnessServiceExt, MarketplaceServiceExt,
            NotificationServiceExt, PreviewServiceExt, ProjectServiceExt, QueueServiceExt,
            ReviewServiceExt, RunGroupServiceExt, StudioServiceExt, TerminalServiceExt,
            ThreadServiceExt,
        };
        let me = Arc::new(self);
        let router = Router::new();
        let router = StudioServiceExt::register(Arc::clone(&me), router);
        let router = ProjectServiceExt::register(Arc::clone(&me), router);
        let router = ThreadServiceExt::register(Arc::clone(&me), router);
        let router = QueueServiceExt::register(Arc::clone(&me), router);
        let router = HarnessServiceExt::register(Arc::clone(&me), router);
        let router = GitServiceExt::register(Arc::clone(&me), router);
        let router = DiffServiceExt::register(Arc::clone(&me), router);
        let router = FileServiceExt::register(Arc::clone(&me), router);
        let router = TerminalServiceExt::register(Arc::clone(&me), router);
        let router = PreviewServiceExt::register(Arc::clone(&me), router);
        let router = ReviewServiceExt::register(Arc::clone(&me), router);
        let router = RunGroupServiceExt::register(Arc::clone(&me), router);
        let router = ApprovalServiceExt::register(Arc::clone(&me), router);
        let router = NotificationServiceExt::register(Arc::clone(&me), router);
        let router = FactoryServiceExt::register(Arc::clone(&me), router);
        let router = MarketplaceServiceExt::register(Arc::clone(&me), router);
        AttachmentServiceExt::register(me, router)
    }

    /// `Mutate` with `params`; every Mutate-backed RPC answers its `ok`.
    async fn mutate(&self, params: rs::MutateParams) -> Result<rs::MutateReply, ConnectError> {
        engine::call::<calls::Mutate>(&*self.engine, &params).await
    }

    /// `QueueCommand` on `chat_id`; answers the command id.
    async fn command(
        &self,
        chat_id: String,
        command: rs::SessionCommandPayload,
    ) -> Result<String, ConnectError> {
        let params = rs::QueueCommandParams { chat_id, command };
        let reply = engine::call::<calls::QueueCommand>(&*self.engine, &params).await?;
        Ok(reply.command_id)
    }
}

/// A unary handler: owned request → `$params` (a `Result<_, MapError>`
/// body, `?` allowed) → method `$method` → `$response`.
macro_rules! unary {
    ($svc:expr, $request:expr, $method:ident, |$req:ident| $params:expr, |$result:ident| $response:expr $(,)?) => {{
        #[allow(unused_variables)]
        let $req = $request.to_owned_message();
        #[allow(clippy::redundant_closure_call)]
        let params: <::loams_agentd_proto::rpc::calls::$method as ::loams_agentd_proto::rpc::Method>::Params =
            (|| -> ::std::result::Result<_, $crate::map::MapError> { Ok($params) })()
                .map_err($crate::error::from_map)?;
        #[allow(unused_variables)]
        let $result = $crate::engine::call::<::loams_agentd_proto::rpc::calls::$method>(
            &*$svc.engine,
            &params,
        )
        .await?;
        ::connectrpc::Response::ok($response)
    }};
}

/// A stream handler: as [`unary`], with `$response` built per item.
macro_rules! stream {
    ($svc:expr, $request:expr, $method:ident, |$req:ident| $params:expr, |$item:ident| $response:expr $(,)?) => {{
        #[allow(unused_variables)]
        let $req = $request.to_owned_message();
        #[allow(clippy::redundant_closure_call)]
        let params: <::loams_agentd_proto::rpc::calls::$method as ::loams_agentd_proto::rpc::Method>::Params =
            (|| -> ::std::result::Result<_, $crate::map::MapError> { Ok($params) })()
                .map_err($crate::error::from_map)?;
        let items = $crate::engine::watch::<::loams_agentd_proto::rpc::calls::$method>(
            &*$svc.engine,
            &params,
        )
        .await?;
        let out: ::connectrpc::ServiceStream<_> = Box::pin(::futures::StreamExt::map(
            items,
            |item| item.map(|$item| $response),
        ));
        ::connectrpc::Response::ok(out)
    }};
}

pub(crate) use {stream, unary};
