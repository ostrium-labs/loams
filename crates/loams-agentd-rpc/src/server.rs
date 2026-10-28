//! Server side: the dispatch loop over string frames, and the WebSocket
//! server that authenticates each upgrade before serving it (design §50 §7).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use loams_agentd_proto::rpc::{LIMIT_EXCEEDED, Role, UNAUTHENTICATED};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::{Role as WsRole, WebSocketConfig};

use crate::auth::{Credentials, Identity};
use crate::handshake::{self, Refusal, Upgrade};
use crate::limits::Limits;
use crate::roles::{Gated, Policy};
use crate::{ClientFrame, RpcError, RpcReply, RpcService, ServerFrame};

/// Serve one connection: read client frames from `inbound`, write server frames to `out`.
/// Returns when `inbound` closes, or after a request failed with [`RpcError::Fatal`]
/// (its `{err}` frame is queued on `out` first); in-flight requests are aborted and
/// awaited before it returns. This is the in-process transport: it needs no
/// `Hello` and has no stream limit.
pub async fn serve_connection(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    inbound: mpsc::Receiver<String>,
) {
    serve_connection_until(service, out, inbound, None, ConnectionRules::default()).await;
}

/// What a connection enforces beyond dispatch.
#[derive(Debug, Default)]
struct ConnectionRules {
    /// Any method before a successful `Hello` is refused and ends the connection.
    require_hello: bool,
    /// Streams open at once; one more is refused with `limit_exceeded`.
    streams: Option<Arc<Semaphore>>,
}

/// [`serve_connection`], which also ends when `stop` turns `true`.
async fn serve_connection_until(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    mut inbound: mpsc::Receiver<String>,
    mut stop: Option<watch::Receiver<bool>>,
    rules: ConnectionRules,
) {
    let mut tasks = JoinSet::new();
    let mut running: HashMap<u64, tokio::task::AbortHandle> = HashMap::new();
    // `close_tx` lives as long as this loop, so `close_rx` only yields on a fatal error.
    let (close_tx, mut close_rx) = mpsc::channel::<()>(1);
    let mut said_hello = !rules.require_hello;
    'connection: loop {
        let payload = tokio::select! {
            biased;
            _ = close_rx.recv() => break,
            () = stopped(&mut stop) => break,
            payload = inbound.recv() => match payload {
                Some(payload) => payload,
                None => break,
            },
        };
        // ndjson: a transport may batch several frames per message.
        for line in payload.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let frame: ClientFrame = match serde_json::from_str(line) {
                Ok(frame) => frame,
                Err(err) => {
                    tracing::warn!(error = %err, "rpc: dropping malformed client frame");
                    continue;
                }
            };
            while tasks.try_join_next().is_some() {}
            running.retain(|_, task| !task.is_finished());
            if frame.cancel {
                if let Some(task) = running.remove(&frame.id) {
                    task.abort();
                }
                continue;
            }
            let Some(method) = frame.method else {
                tracing::warn!(id = frame.id, "rpc: frame has neither method nor cancel");
                continue;
            };
            if !said_hello && method != crate::methods::HELLO {
                tracing::warn!(id = frame.id, "rpc: a call before Hello; closing");
                let _ = send(
                    &out,
                    ServerFrame {
                        id: frame.id,
                        err: Some(format!(
                            "{UNAUTHENTICATED}: Hello must be the first call on a connection"
                        )),
                        ..Default::default()
                    },
                )
                .await;
                break 'connection;
            }
            let request = handle_request(
                service.clone(),
                out.clone(),
                close_tx.clone(),
                rules.streams.clone(),
                frame.id,
                method.clone(),
                frame.params,
            );
            if method == crate::methods::HELLO {
                // The one method that can end the connection: nothing after a
                // fatal `Hello` may be dispatched, even in the same payload.
                let answered = request.await;
                if close_rx.try_recv().is_ok() {
                    break 'connection;
                }
                said_hello |= answered;
                continue;
            }
            let task = tasks.spawn(async move {
                request.await;
            });
            running.insert(frame.id, task);
        }
    }
    tasks.shutdown().await;
}

/// Resolves once `stop` is `true` (or its sender is gone); never without one.
async fn stopped(stop: &mut Option<watch::Receiver<bool>>) {
    match stop {
        Some(stop) => {
            let _ = stop.wait_for(|stopped| *stopped).await;
        }
        None => std::future::pending().await,
    }
}

async fn send(out: &mpsc::Sender<String>, frame: ServerFrame) -> Result<(), RpcError> {
    match serde_json::to_string(&frame) {
        Ok(json) => out.send(json).await.map_err(|_| RpcError::Closed),
        Err(err) => {
            tracing::error!(error = %err, "rpc: failed to serialize server frame");
            Err(RpcError::Closed)
        }
    }
}

/// Run one request to its end. `true` when the service answered rather than
/// failed (for `Hello`: the handshake succeeded).
async fn handle_request(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    close: mpsc::Sender<()>,
    streams: Option<Arc<Semaphore>>,
    id: u64,
    method: String,
    params: serde_json::Value,
) -> bool {
    match service.handle(&method, params).await {
        Ok(RpcReply::Value(value)) => {
            let _ = send(
                &out,
                ServerFrame {
                    id,
                    ok: Some(value),
                    ..Default::default()
                },
            )
            .await;
            true
        }
        Ok(RpcReply::Stream(mut stream)) => {
            // Held while the stream runs.
            let _permit = match streams.map(Semaphore::try_acquire_owned) {
                Some(Err(_)) => {
                    let _ = send(
                        &out,
                        ServerFrame {
                            id,
                            err: Some(format!(
                                "{LIMIT_EXCEEDED}: too many streams open on this connection"
                            )),
                            ..Default::default()
                        },
                    )
                    .await;
                    return false;
                }
                Some(Ok(permit)) => Some(permit),
                None => None,
            };
            // Only the versioned checkout-PR stream uses an explicit readiness
            // frame. Sending it for legacy streams would make older clients remove
            // their pending stream as if it were a unary response.
            if method == crate::methods::WATCH_CHECKOUT_CHANGE_REQUEST
                && send(
                    &out,
                    ServerFrame {
                        id,
                        ok: Some(serde_json::json!({ "stream": true })),
                        ..Default::default()
                    },
                )
                .await
                .is_err()
            {
                return true;
            }
            while let Some(item) = stream.next().await {
                if send(
                    &out,
                    ServerFrame {
                        id,
                        item: Some(item),
                        ..Default::default()
                    },
                )
                .await
                .is_err()
                {
                    return true; // connection gone
                }
            }
            let _ = send(
                &out,
                ServerFrame {
                    id,
                    done: true,
                    ..Default::default()
                },
            )
            .await;
            true
        }
        Err(err) => {
            let fatal = matches!(err, RpcError::Fatal(_));
            let _ = send(
                &out,
                ServerFrame {
                    id,
                    err: Some(err.to_string()),
                    ..Default::default()
                },
            )
            .await;
            if fatal {
                let _ = close.try_send(());
            }
            false
        }
    }
}

/// Builds the service one authenticated connection is served by (it learns
/// the connection's identity, for `Hello`'s `role`).
pub trait Connections: Send + Sync + 'static {
    fn open(&self, identity: &Identity) -> Arc<dyn RpcService>;
}

/// The loopback WebSocket RPC server (design §50 §7). Each upgrade is checked
/// before anything is served (`handshake.rs`), then authenticated; each call
/// passes the [`Policy`], and `Hello` must come first.
pub struct WsServer {
    credentials: Arc<Credentials>,
    connections: Arc<dyn Connections>,
    policy: Arc<dyn Policy>,
    limits: Limits,
}

impl std::fmt::Debug for WsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsServer")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl WsServer {
    pub fn new(
        credentials: Arc<Credentials>,
        connections: Arc<dyn Connections>,
        policy: Arc<dyn Policy>,
    ) -> Self {
        Self {
            credentials,
            connections,
            policy,
            limits: Limits::default(),
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Accept connections on `listener` until `stop` resolves. Then stop
    /// accepting, end every open connection (its in-flight requests are
    /// aborted and awaited) and return, so the caller can tear down what the
    /// services use. A listener that is not on loopback is refused.
    pub async fn serve_until(
        self,
        listener: TcpListener,
        stop: impl std::future::Future<Output = ()>,
    ) -> std::io::Result<()> {
        let local = listener.local_addr()?;
        if !local.ip().is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("the RPC serves loopback only, not {local}"),
            ));
        }
        let shared = Arc::new(Shared {
            port: local.port(),
            owner_slots: Arc::new(Semaphore::new(self.limits.max_owner_connections)),
            pending_upgrades: Arc::new(Semaphore::new(self.limits.max_pending_upgrades)),
            auth_gate: tokio::sync::Mutex::new(()),
            server: self,
        });
        let (stop_tx, stop_rx) = watch::channel(false);
        let mut connections = JoinSet::new();
        tokio::pin!(stop);
        loop {
            tokio::select! {
                biased;
                () = &mut stop => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        connections.spawn(serve_socket(shared.clone(), stream, peer, stop_rx.clone()));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "rpc: accept failed");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                },
            }
        }
        drop(listener);
        let _ = stop_tx.send(true);
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}

pub(crate) struct Shared {
    server: WsServer,
    port: u16,
    owner_slots: Arc<Semaphore>,
    /// Connections still before their token check.
    pending_upgrades: Arc<Semaphore>,
    /// Serializes token checks, so a failure's delay holds back the next attempt.
    auth_gate: tokio::sync::Mutex<()>,
}

/// One accepted socket: check and authenticate the upgrade, then serve it.
async fn serve_socket<S>(
    shared: Arc<Shared>,
    mut stream: S,
    peer: SocketAddr,
    mut stop: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if !peer.ip().is_loopback() {
        // The listener is loopback-only; this is defence in depth.
        tracing::warn!("rpc: dropping a connection from a non-loopback peer");
        return;
    }
    let limits = shared.server.limits;
    // Held until the token is checked: unauthenticated connections are
    // bounded, and so is the queue behind a failure's delay.
    let Ok(pending) = shared.pending_upgrades.clone().try_acquire_owned() else {
        tracing::warn!(
            peer_port = peer.port(),
            "rpc: too many unauthenticated connections; closing"
        );
        return;
    };
    let checked = tokio::select! {
        checked = read_upgrade(&mut stream, shared.port, &limits) => checked,
        _ = stop.wait_for(|stopped| *stopped) => return,
    };
    let upgrade = match checked {
        Ok(upgrade) => Some(upgrade),
        Err(Some(Refusal::Unauthorized)) => None,
        Err(Some(Refusal::Forbidden)) => {
            tracing::debug!(peer_port = peer.port(), "rpc: refused a request (403)");
            refuse(&mut stream, handshake::FORBIDDEN).await;
            return;
        }
        Err(None) => return,
    };
    let identity = {
        let Ok(_gate) =
            tokio::time::timeout(limits.handshake_timeout, shared.auth_gate.lock()).await
        else {
            return;
        };
        let identity = upgrade.as_ref().and_then(|upgrade| {
            shared
                .server
                .credentials
                .authenticate(&upgrade.authorization)
        });
        if identity.is_none() {
            tracing::warn!(
                peer_port = peer.port(),
                "rpc: refused an upgrade without a valid token (401)"
            );
            tokio::time::sleep(limits.auth_failure_delay).await;
        }
        identity
    };
    let (Some(identity), Some(upgrade)) = (identity, upgrade) else {
        refuse(&mut stream, handshake::UNAUTHORIZED).await;
        return;
    };
    drop(pending);
    let _owner_slot = match identity.role {
        Role::Owner => match shared.owner_slots.clone().try_acquire_owned() {
            Ok(slot) => Some(slot),
            Err(_) => {
                tracing::warn!(
                    peer_port = peer.port(),
                    "rpc: refused an owner connection over the limit (429)"
                );
                refuse(&mut stream, handshake::TOO_MANY).await;
                return;
            }
        },
        Role::Mcp => None,
    };
    let accept = handshake::accept_response(&upgrade.key);
    if stream.write_all(accept.as_bytes()).await.is_err() || stream.flush().await.is_err() {
        return;
    }
    tracing::debug!(peer_port = peer.port(), role = ?identity.role, "rpc: connection accepted");
    let config = WebSocketConfig::default()
        .max_message_size(Some(limits.max_frame_bytes))
        .max_frame_size(Some(limits.max_frame_bytes));
    let ws = WebSocketStream::from_raw_socket(stream, WsRole::Server, Some(config)).await;

    let server = &shared.server;
    let service: Arc<dyn RpcService> = Arc::new(Gated::new(
        identity.clone(),
        server.credentials.clone(),
        server.policy.clone(),
        server.connections.open(&identity),
    ));
    // The connection ends with the server, or when its run token is revoked.
    let (end_tx, end_rx) = watch::channel(false);
    let credentials = server.credentials.clone();
    let ender = tokio::spawn(async move {
        let mut revocations = credentials.scoped().revocations();
        loop {
            tokio::select! {
                _ = stop.wait_for(|stopped| *stopped) => break,
                changed = revocations.changed() => {
                    if changed.is_err() || !credentials.still_valid(&identity) {
                        break;
                    }
                }
            }
        }
        let _ = end_tx.send(true);
    });
    serve_websocket(ws, service, end_rx, limits.max_streams).await;
    ender.abort();
}

/// Read the request head and run the checks that need no credentials.
/// `Err(None)`: the peer went away, failed or took too long (no answer).
async fn read_upgrade<S: AsyncRead + Unpin>(
    stream: &mut S,
    port: u16,
    limits: &Limits,
) -> Result<Upgrade, Option<Refusal>> {
    let read = async {
        let mut head = Vec::with_capacity(1024);
        let mut chunk = [0u8; 2048];
        loop {
            let n = stream.read(&mut chunk).await.map_err(|_| None)?;
            if n == 0 {
                return Err(None);
            }
            head.extend_from_slice(&chunk[..n]);
            match handshake::check(&head, port) {
                Ok(Some(upgrade)) => return Ok(upgrade),
                Ok(None) if head.len() < limits.max_request_head => {}
                Ok(None) => return Err(Some(Refusal::Forbidden)),
                Err(refusal) => return Err(Some(refusal)),
            }
        }
    };
    tokio::time::timeout(limits.handshake_timeout, read)
        .await
        .unwrap_or(Err(None))
}

async fn refuse<S: AsyncWrite + Unpin>(stream: &mut S, response: &str) {
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Pump an upgraded socket through the dispatch loop.
async fn serve_websocket<S>(
    ws: WebSocketStream<S>,
    service: Arc<dyn RpcService>,
    end: watch::Receiver<bool>,
    max_streams: usize,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut ws_stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<String>(256);

    // Pump: socket <-> string channels. Ends when either side closes, or on a
    // read error (a message over the frame limit is one).
    let pump = tokio::spawn(async move {
        loop {
            tokio::select! {
                frame = out_rx.recv() => match frame {
                    Some(text) => {
                        if sink.send(WsMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                },
                message = ws_stream.next() => match message {
                    Some(Ok(WsMessage::Text(text))) => {
                        if in_tx.send(text.to_string()).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        tracing::debug!(%error, "rpc: closing the connection on a read error");
                        break;
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong/binary — ignored
                },
            }
        }
    });

    serve_connection_until(
        service,
        out_tx,
        in_rx,
        Some(end),
        ConnectionRules {
            require_hello: true,
            streams: Some(Arc::new(Semaphore::new(max_streams))),
        },
    )
    .await;
    // Let the pump flush what is queued (a fatal error's frame) and send Close:
    // it ends once every sender is dropped. A peer that stopped reading cannot
    // hold the task for longer than the grace period.
    let mut pump = pump;
    if tokio::time::timeout(CLOSE_GRACE, &mut pump).await.is_err() {
        pump.abort();
    }
}

/// How long a closing connection may take to flush its last frames.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(test)]
pub(crate) mod testing {
    //! Drives [`serve_socket`] over an in-memory duplex with a chosen peer.
    use super::*;

    pub(crate) fn shared(server: WsServer, port: u16) -> Arc<Shared> {
        Arc::new(Shared {
            port,
            owner_slots: Arc::new(Semaphore::new(server.limits.max_owner_connections)),
            pending_upgrades: Arc::new(Semaphore::new(server.limits.max_pending_upgrades)),
            auth_gate: tokio::sync::Mutex::new(()),
            server,
        })
    }

    pub(crate) async fn serve_duplex(
        shared: Arc<Shared>,
        stream: tokio::io::DuplexStream,
        peer: SocketAddr,
    ) {
        let (_stop_tx, stop_rx) = watch::channel(false);
        serve_socket(shared, stream, peer, stop_rx).await;
    }
}
