//! The daemon's run loop and RPC wiring (design §50 §5.4, §6 and §7).
//!
//! The caller holds `agentd.lock`. In order, [`serve`]:
//! 1. rotates the token;
//! 2. binds the RPC listener on `127.0.0.1:0` (loopback only, an ephemeral port);
//! 3. assembles the sessions engine on `store/`;
//! 4. serves `Hello` and the engine's methods on the listener, behind the
//!    token, `Origin`, `Host` and role checks (design §50 §7, [`rpc_server`]);
//! 5. binds the `loams.agentd.v1` Connect listener on `127.0.0.1:0` behind the
//!    same token, `Host` and `Origin` rules (design §53 §6.2, plan SD1 Task 5);
//! 6. with `serve --remote` or `[remote] enabled = true` only, binds the
//!    remote Connect listener (TLS, OIDC, the console origins, role `studio`;
//!    design §53 §6.3, plan SD1 Task 7). It is off by default;
//! 7. publishes `agentd.json`, with both loopback ports;
//! 8. runs until SIGTERM, Ctrl-C or `Shutdown`, then closes the listeners and
//!    every connection (awaiting their ndjson requests), stops the engine and
//!    removes `agentd.json`.

use std::sync::Arc;

use async_trait::async_trait;
use loams_agentd_connect::{
    Agentd, ConnectListener, HostInfo, LoopbackOptions, RemoteListener, RemoteOptions,
};
use loams_agentd_proto::OkReply;
use loams_agentd_proto::daemon::{DaemonMode, Discovery};
use loams_agentd_proto::rpc::{AGENTD_PROTOCOL, calls};
use loams_agentd_rpc::{
    Connections, Credentials, Identity, RpcError, RpcReply, RpcService, ScopedTokens, Secret,
    WsServer, methods,
};
use loams_agentd_sessions::sessions::McpEndpoint;
use loams_agentd_sessions::{Engine, EngineConfig, EngineProfile, HarnessId};

use crate::config::Config;
use crate::discovery;
use crate::hello::{BUILD_SHA, DaemonInfo, FEATURES, HelloService, VERSION};
use crate::paths::Paths;
use crate::roles::{ChatTree, DaemonPolicy};

/// What [`serve`] runs with.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub paths: Paths,
    pub config: Config,
    pub mode: DaemonMode,
    /// The harness for chats without a config row.
    pub default_harness: HarnessId,
    /// The remote listener ([`crate::remote::options`]); `None`, the
    /// default, leaves it off.
    pub remote: Option<RemoteOptions>,
}

/// Run the daemon until it is asked to stop. See the module docs.
pub async fn serve(options: ServeOptions) -> anyhow::Result<()> {
    let ServeOptions {
        paths,
        config,
        mode,
        default_harness,
        remote,
    } = options;
    // A service starts with the manager's minimal PATH: resolve the login
    // shell's now, so harness CLIs are found (design §50 §5.1). Harness
    // spawns add it to their PATH; it is cached for the process.
    tokio::task::spawn_blocking(
        || match loams_agentd_harness::shell_env::login_shell_path() {
            Some(path) => tracing::info!(
                entries = std::env::split_paths(path).count(),
                "the login shell's PATH is resolved"
            ),
            None => tracing::info!("no login-shell PATH; harnesses use the daemon's PATH"),
        },
    );
    let token = crate::token::rotate(&paths)?;
    // Harness adapters live in the daemon directory, not `~/.loams-desktop`.
    loams_agentd_harness::set_adapters_root(paths.adapters());

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();

    let store = paths.store();
    std::fs::create_dir_all(&store)?;
    // The MCP shim injected into each harness run dials this port with a
    // token scoped to that run. Installed during assembly, before stale runs
    // are resumed, so a resumed run gets the shim too.
    let scoped = Arc::new(ScopedTokens::new());
    let engine_config = EngineConfig {
        data_dir: store.clone(),
        default_harness,
        terminal_shell: None,
        tool_outputs: config.tool_outputs.limits(),
        mcp_endpoint: Some(McpEndpoint {
            port,
            tokens: scoped.clone(),
        }),
        // `[approvals]` (SD1 Task 13): supervised by default, local or remote
        // (threat model I-1), before any stale run is resumed.
        approvals: config.approvals.settings(),
    };
    let runtime = Engine::assemble_runtime(&engine_config, EngineProfile::local(&store)?).await?;
    // `[groups]` (SD1 Task 12): how many run-group members run at once.
    runtime.core().run_groups.set_limits(config.groups.limits());
    // The marketplace (MK1 Task 4b), under the daemon directory.
    crate::marketplace::start(paths.dir(), runtime.core());

    let info = DaemonInfo {
        mode,
        pid: std::process::id(),
        started_at_ms: now_ms(),
    };
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let lifecycle = Lifecycle {
        inner: runtime.core().rpc_service(),
        stop_tx,
    };
    // The Connect surface calls the engine's own dispatch, never the
    // lifecycle: `Shutdown` stays on the ndjson wire.
    let connect = ConnectListener::bind(
        connect_services(runtime.core().rpc_service(), info),
        LoopbackOptions::new(token.clone()),
    )
    .await;
    let connect = match connect {
        Ok(connect) => connect,
        Err(error) => {
            runtime.shutdown().await;
            return Err(anyhow::Error::new(error).context("bind the Connect listener"));
        }
    };
    let connect_port = connect.port();
    // The remote listener serves the same services as the loopback one,
    // behind its own checks; never the ndjson wire or the lifecycle.
    let remote = match remote {
        None => {
            tracing::info!("the remote listener is off");
            None
        }
        Some(options) => {
            match RemoteListener::bind(
                connect_services(runtime.core().rpc_service(), info),
                options,
            )
            .await
            {
                Ok(remote) => Some(remote),
                Err(error) => {
                    connect.stop().await;
                    runtime.shutdown().await;
                    return Err(anyhow::Error::new(error).context("bind the remote listener"));
                }
            }
        }
    };
    let rpc = rpc_server(
        Arc::new(lifecycle),
        Arc::new(runtime.core().workspace.clone()),
        info,
        token,
        scoped,
    );
    let (server_stop, server_stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(rpc.serve_until(listener, async {
        let _ = server_stopped.await;
    }));
    // Stops accepting, ends every connection and awaits their requests, so
    // nothing calls into the engine while it shuts down.
    let stop_server = || async {
        if let Some(remote) = remote {
            remote.stop().await;
        }
        connect.stop().await;
        let _ = server_stop.send(());
        match server.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "the RPC server failed"),
            Err(error) => tracing::warn!(%error, "the RPC server task failed"),
        }
    };

    let published = discovery::publish(
        &paths,
        &Discovery {
            protocol: AGENTD_PROTOCOL,
            version: VERSION.to_string(),
            pid: info.pid,
            port,
            started_at_ms: info.started_at_ms,
            mode,
            connect_port: Some(connect_port),
        },
    );
    if let Err(error) = published {
        stop_server().await;
        runtime.shutdown().await;
        return Err(anyhow::Error::new(error).context("publish agentd.json"));
    }
    tracing::info!(
        port,
        connect_port,
        ?mode,
        pid = info.pid,
        "loams-agentd ready"
    );

    tokio::select! {
        signal = shutdown_signal() => match signal {
            Ok(()) => tracing::info!("stop signal received"),
            Err(error) => tracing::error!(%error, "cannot wait for stop signals; stopping"),
        },
        Some(()) = stop_rx.recv() => tracing::info!("stop requested over the RPC"),
    }
    tracing::info!("shutting down");
    stop_server().await;
    runtime.shutdown().await;
    if let Err(error) = discovery::remove(&paths) {
        tracing::warn!(%error, "agentd.json was not removed");
    }
    Ok(())
}

/// The daemon's RPC server (design §50 §7): the main `token` (role `owner`)
/// and the `scoped` run tokens (role `mcp`), the role policy over `chats`,
/// and `Hello` in front of `engine`.
pub fn rpc_server(
    engine: Arc<dyn RpcService>,
    chats: Arc<dyn ChatTree>,
    info: DaemonInfo,
    token: Secret,
    scoped: Arc<ScopedTokens>,
) -> WsServer {
    WsServer::new(
        Arc::new(Credentials::new(token, scoped)),
        Arc::new(DaemonConnections { engine, info }),
        Arc::new(DaemonPolicy::new(chats)),
    )
}

/// `loams.agentd.v1` over `engine` (plan SD1 Task 5), reporting `info` and
/// the daemon's build in `GetHost`.
pub fn connect_services(engine: Arc<dyn RpcService>, info: DaemonInfo) -> Agentd {
    Agentd::new(
        engine,
        HostInfo {
            protocol: AGENTD_PROTOCOL,
            version: VERSION.to_string(),
            build_sha: BUILD_SHA.to_string(),
            mode: info.mode,
            pid: info.pid,
            started_at_ms: info.started_at_ms,
            features: FEATURES
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
        },
    )
}

/// Each connection gets its own `Hello`, which reports the connection's role.
struct DaemonConnections {
    engine: Arc<dyn RpcService>,
    info: DaemonInfo,
}

impl Connections for DaemonConnections {
    fn open(&self, identity: &Identity) -> Arc<dyn RpcService> {
        Arc::new(HelloService::new(
            self.engine.clone(),
            self.info,
            identity.role,
        ))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// SIGTERM or Ctrl-C. systemd and launchd stop with SIGTERM; without catching
/// it the daemon would die mid-write and every stop would take the
/// crash-recovery path instead of the clean one.
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = sigterm.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

/// `Shutdown` stops the daemon; every other method goes to the engine.
/// (The fork's headless `StopEngine`, renamed by Shared contracts, ruling
/// T0-1; the name `StopEngine` is Task 14's, for the supervised engine.)
struct Lifecycle {
    inner: Arc<dyn RpcService>,
    stop_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

#[async_trait]
impl RpcService for Lifecycle {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method != methods::SHUTDOWN {
            return self.inner.handle(method, params).await;
        }
        loams_agentd_rpc::params_of::<calls::Shutdown>(params)?;
        let stop_tx = self.stop_tx.clone();
        // Let the reply reach the client before the server is aborted.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = stop_tx.send(());
        });
        loams_agentd_rpc::reply::<calls::Shutdown, _>(&OkReply::OK)
    }
}
