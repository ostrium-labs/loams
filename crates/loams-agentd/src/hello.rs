//! `Hello`, the handshake (design §50 §5.4).
//!
//! [`HelloService`] answers `Hello` in front of the engine's methods, with the
//! role the connection's token gave it. A client with another protocol gets
//! `protocol_mismatch: …` and its connection is closed ([`RpcError::Fatal`]).
//! That `Hello` comes first on every connection is the RPC server's rule
//! (DD1 Task 7). [`probe`] is the client side, used by a second `run` and by
//! `status` to confirm a daemon found through discovery.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_agentd_proto::daemon::DaemonMode;
use loams_agentd_proto::rpc::{
    AGENTD_PROTOCOL, ClientKind, Hello, HelloReply, PROTOCOL_MISMATCH, Role,
};
use loams_agentd_rpc::{RpcError, RpcReply, RpcService, methods, parse_params};

/// The daemon's version: `LOAMS_AGENTD_VERSION` at build time (the Electron
/// app's version, set by the packaging script), else the crate's (T0-10b).
pub const VERSION: &str = match option_env!("LOAMS_AGENTD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// The commit the daemon was built from (`LOAMS_BUILD_SHA`), else `unknown`.
pub const BUILD_SHA: &str = match option_env!("LOAMS_BUILD_SHA") {
    Some(sha) => sha,
    None => "unknown",
};

/// The daemon's features, in `Hello` and `GetHost` (SD1 Shared contracts):
/// `studio.v1` is the `loams.agentd.v1` Connect surface (SD1 Task 5);
/// `git.write` the worktree list and the git writes (SD1 Task 9);
/// `checkpoints` the per-turn checkpoints and restore (SD1 Task 10);
/// `review` the review comments and `content-search` the content search
/// (SD1 Task 11);
/// `run-groups` the parallel run groups (SD1 Task 12);
/// `notifications` and `usage` the notification feed and per-thread usage
/// (SD1 Task 14);
/// `approvals.supervised` the supervised approvals (SD1 Task 13);
/// `marketplace` the Seed Exchange's `MarketplaceService` (MK1 Task 4b).
pub const FEATURES: &[&str] = &[
    loams_agentd_connect::FEATURE,
    loams_agentd_connect::GIT_WRITE_FEATURE,
    loams_agentd_proto::checkpoints::FEATURE,
    loams_agentd_connect::REVIEW_FEATURE,
    loams_agentd_connect::CONTENT_SEARCH_FEATURE,
    loams_agentd_proto::groups::FEATURE,
    loams_agentd_proto::marketplace::FEATURE,
    loams_agentd_proto::notify::FEATURE,
    loams_agentd_proto::usage::FEATURE,
    loams_agentd_proto::approvals::FEATURE,
];

/// How long [`probe`] waits for a daemon to answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// What this process reports about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonInfo {
    pub mode: DaemonMode,
    pub pid: u32,
    pub started_at_ms: i64,
}

impl DaemonInfo {
    /// `Hello`'s reply to a connection with `role`.
    pub fn reply(&self, role: Role) -> HelloReply {
        HelloReply {
            protocol: AGENTD_PROTOCOL,
            version: VERSION.to_string(),
            build_sha: BUILD_SHA.to_string(),
            mode: self.mode,
            pid: self.pid,
            started_at_ms: self.started_at_ms,
            role,
            features: FEATURES
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
        }
    }
}

/// Serves `Hello` for one connection; every other method goes to `inner`.
pub struct HelloService {
    inner: Arc<dyn RpcService>,
    info: DaemonInfo,
    role: Role,
}

impl HelloService {
    pub fn new(inner: Arc<dyn RpcService>, info: DaemonInfo, role: Role) -> Self {
        Self { inner, info, role }
    }
}

impl std::fmt::Debug for HelloService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelloService")
            .field("info", &self.info)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl RpcService for HelloService {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method != methods::HELLO {
            return self.inner.handle(method, params).await;
        }
        let hello: Hello = parse_params(params)?;
        if hello.protocol != AGENTD_PROTOCOL {
            return Err(RpcError::Fatal(format!(
                "{PROTOCOL_MISMATCH}: this daemon speaks protocol {AGENTD_PROTOCOL}, \
                 the client protocol {}",
                hello.protocol
            )));
        }
        RpcReply::value(&self.info.reply(self.role))
    }
}

/// The CLI's `Hello`.
pub fn cli_hello() -> Hello {
    Hello {
        client: ClientKind::Cli,
        client_version: VERSION.to_string(),
        protocol: AGENTD_PROTOCOL,
    }
}

/// Say `Hello` to the daemon on loopback `port` as the CLI, with the main
/// `token` (the contents of `agentd.token`).
pub async fn probe(port: u16, token: &str) -> Result<HelloReply, RpcError> {
    let params =
        serde_json::to_value(cli_hello()).map_err(|e| RpcError::BadParams(e.to_string()))?;
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let client =
            loams_agentd_rpc::connect_ws_with_token(&format!("ws://127.0.0.1:{port}"), token)
                .await?;
        client.call_as(methods::HELLO, params).await
    })
    .await
    .map_err(|_| RpcError::Transport(format!("no Hello from port {port}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Engine;

    #[async_trait]
    impl RpcService for Engine {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            Ok(RpcReply::Value(serde_json::json!({ "engine": method })))
        }
    }

    fn service() -> Arc<HelloService> {
        service_for(Role::Owner)
    }

    fn service_for(role: Role) -> Arc<HelloService> {
        Arc::new(HelloService::new(
            Arc::new(Engine),
            DaemonInfo {
                mode: DaemonMode::Service,
                pid: 42,
                started_at_ms: 7,
            },
            role,
        ))
    }

    #[tokio::test]
    async fn hello_reports_the_connections_role() {
        let client = loams_agentd_rpc::memory_client(service_for(Role::Mcp));
        let reply: HelloReply = client
            .call_as(
                methods::HELLO,
                serde_json::json!({"client": "mcp", "clientVersion": "1", "protocol": 1}),
            )
            .await
            .expect("hello");
        assert_eq!(reply.role, Role::Mcp);
    }

    #[tokio::test]
    async fn hello_answers_and_other_methods_pass_through() {
        let client = loams_agentd_rpc::memory_client(service());
        let reply: HelloReply = client
            .call_as(
                methods::HELLO,
                serde_json::json!({"client": "electron-main", "clientVersion": "1", "protocol": 1}),
            )
            .await
            .expect("hello");
        assert_eq!(reply.mode, DaemonMode::Service);
        assert_eq!(reply.pid, 42);
        assert_eq!(reply.started_at_ms, 7);
        assert_eq!(reply.version, VERSION);
        assert_eq!(reply.role, Role::Owner);
        assert_eq!(
            client
                .call("ListHarnesses", serde_json::json!({}))
                .await
                .expect("pass through"),
            serde_json::json!({ "engine": "ListHarnesses" })
        );
    }

    #[tokio::test]
    async fn malformed_hello_is_bad_params_and_keeps_the_connection() {
        let client = loams_agentd_rpc::memory_client(service());
        let err = client
            .call(methods::HELLO, serde_json::json!({"client": "browser"}))
            .await
            .expect_err("bad params");
        assert!(err.to_string().starts_with("bad params"), "{err}");
        assert!(
            client
                .call(
                    methods::HELLO,
                    serde_json::json!({"client": "cli", "clientVersion": "1", "protocol": 1})
                )
                .await
                .is_ok()
        );
    }
}
