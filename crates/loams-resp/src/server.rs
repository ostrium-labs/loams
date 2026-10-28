// ported-from: https://github.com/tidb-incubator/tidis
// revision: 555856206f248737b12d3f148b10f08a604bc821
// path: src/server.rs, src/connection.rs
// licence: Apache-2.0
// ported-from: https://github.com/tokio-rs/mini-redis
// revision: (as carried by Tidis at the revision above)
// path: src/server.rs, src/connection.rs
// licence: MIT, Copyright (c) 2020 Tokio Contributors
// Modified by Ostrium Labs for Loams.

//! The listener and the connection loop (plan KV2 Task 2).
//!
//! One task per connection, admitted by a semaphore of `max_clients`
//! permits, as Tidis's (and mini-redis's) `Listener` does. The connection
//! reads into a buffer, takes every whole request off it, runs each in
//! order, encodes the replies into one write buffer and flushes it once
//! per read, so a pipelined batch costs one write. A protocol error is
//! replied as `-ERR Protocol error: …` and closes the connection, as
//! Valkey does.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use bytes::{Bytes, BytesMut};
use loams_kv::Store;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};

use crate::cmd::{self, Flow};
use crate::config::RespConfig;
use crate::frame::{Decoder, Proto, Value};

/// What every connection shares.
#[derive(Debug)]
pub struct Shared {
    pub store: Store,
    pub password: Option<String>,
    pub databases: u16,
    pub config: RespConfig,
    pub started: Instant,
    pub clients: Mutex<BTreeMap<u64, ClientEntry>>,
    pub next_id: AtomicU64,
    pub stats: Stats,
    /// `CONFIG SET` values, echoed by `CONFIG GET` (report §1.2).
    pub params: Mutex<HashMap<String, String>>,
    pub listen: SocketAddr,
}

/// Server-wide counters for `INFO stats`.
#[derive(Debug, Default)]
pub struct Stats {
    pub connections: AtomicU64,
    pub commands: AtomicU64,
    pub rejected: AtomicU64,
}

/// A connected client, as `CLIENT LIST` shows it and `CLIENT KILL` finds it.
#[derive(Debug)]
pub struct ClientEntry {
    pub addr: SocketAddr,
    pub name: Option<Bytes>,
    pub db: u8,
    pub connected: Instant,
    pub last_command: Option<String>,
    pub lib_name: Option<String>,
    pub lib_ver: Option<String>,
    pub kill: watch::Sender<bool>,
}

/// One connection's state.
#[derive(Debug)]
pub struct Conn {
    pub id: u64,
    pub addr: SocketAddr,
    pub proto: Proto,
    pub db: u8,
    pub name: Option<Bytes>,
    pub authenticated: bool,
    /// `CLIENT REPLY OFF` / `SKIP`.
    pub reply: ReplyMode,
    /// Open `SCAN`-family cursors.
    pub cursors: crate::cursor::Cursors,
}

/// `CLIENT REPLY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    On,
    Off,
    /// Skip the next command's reply.
    Skip,
}

impl Shared {
    /// The shared state for `config`, on `store`.
    pub fn new(config: RespConfig, store: Store, listen: SocketAddr) -> Result<Self, String> {
        Ok(Shared {
            password: config.password()?,
            databases: config.databases,
            store,
            started: Instant::now(),
            clients: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            stats: Stats::default(),
            params: Mutex::new(HashMap::new()),
            config,
            listen,
        })
    }

    /// Updates a client's entry, if it is still registered.
    pub fn with_client<T>(&self, id: u64, f: impl FnOnce(&mut ClientEntry) -> T) -> Option<T> {
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.get_mut(&id).map(f)
    }
}

/// A bound gateway.
#[derive(Debug)]
pub struct Server {
    shared: Arc<Shared>,
    listener: TcpListener,
}

impl Server {
    /// Opens the store and binds the listener.
    pub async fn bind(config: RespConfig) -> Result<Self, String> {
        let store_config = config.store_config()?;
        let store = Store::open(store_config)
            .await
            .map_err(|e| format!("opening the store: {e}"))?;
        Self::bind_on(config, store).await
    }

    /// Binds the listener over an open store (tests).
    pub async fn bind_on(config: RespConfig, store: Store) -> Result<Self, String> {
        let listener = TcpListener::bind(config.listen)
            .await
            .map_err(|e| format!("binding {}: {e}", config.listen))?;
        let listen = listener.local_addr().map_err(|e| e.to_string())?;
        Ok(Server {
            shared: Arc::new(Shared::new(config, store, listen)?),
            listener,
        })
    }

    /// The bound address.
    pub fn local_addr(&self) -> SocketAddr {
        self.shared.listen
    }

    /// The shared state.
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Serves until `shutdown` resolves; live connections are closed.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), String> {
        let limit = Arc::new(Semaphore::new(self.shared.config.max_clients));
        let (stop_tx, stop_rx) = watch::channel(false);
        if self.shared.config.workers {
            tokio::spawn(crate::workers::run(
                self.shared.store.clone(),
                self.shared.config.worker_options(),
                stop_rx.clone(),
            ));
        }
        tokio::pin!(shutdown);
        loop {
            let (mut stream, addr) = tokio::select! {
                () = &mut shutdown => break,
                accepted = self.listener.accept() => match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(error = %e, "resp: accept failed");
                        continue;
                    }
                },
            };
            let shared = self.shared.clone();
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                shared.stats.rejected.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    let _ = stream
                        .write_all(b"-ERR max number of clients reached\r\n")
                        .await;
                });
                continue;
            };
            let _ = stream.set_nodelay(true);
            let stop = stop_rx.clone();
            tokio::spawn(async move {
                serve_stream(shared, stream, addr, stop).await;
                drop(permit);
            });
        }
        let _ = stop_tx.send(true);
        Ok(())
    }
}

/// Serves one client on any byte stream (TCP now, TLS in Task 13).
pub async fn serve_stream<S>(
    shared: Arc<Shared>,
    mut stream: S,
    addr: SocketAddr,
    mut stop: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    let (kill_tx, mut kill) = watch::channel(false);
    {
        let mut clients = shared.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.insert(
            id,
            ClientEntry {
                addr,
                name: None,
                db: 0,
                connected: Instant::now(),
                last_command: None,
                lib_name: None,
                lib_ver: None,
                kill: kill_tx,
            },
        );
    }
    shared.stats.connections.fetch_add(1, Ordering::Relaxed);
    let mut conn = Conn {
        id,
        addr,
        proto: Proto::Resp2,
        db: 0,
        name: None,
        authenticated: shared.password.is_none(),
        reply: ReplyMode::On,
        cursors: crate::cursor::Cursors::default(),
    };
    let mut decoder = Decoder::new(shared.config.limits(), conn.authenticated);
    let mut rbuf = BytesMut::with_capacity(16 * 1024);
    let mut wbuf = BytesMut::with_capacity(16 * 1024);
    let idle = shared.config.idle_timeout();
    'conn: loop {
        // Run every whole request in the buffer.
        loop {
            match decoder.decode(&mut rbuf) {
                Ok(Some(args)) => {
                    let before = conn.reply;
                    let (reply, flow) = cmd::dispatch(&shared, &mut conn, args).await;
                    decoder.set_authenticated(conn.authenticated);
                    if write_reply(before, &mut conn.reply) {
                        reply.encode(conn.proto, &mut wbuf);
                    }
                    if flow == Flow::Close {
                        let _ = stream.write_all(&wbuf).await;
                        break 'conn;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    wbuf.extend_from_slice(b"-ERR ");
                    wbuf.extend_from_slice(e.0.as_bytes());
                    wbuf.extend_from_slice(b"\r\n");
                    let _ = stream.write_all(&wbuf).await;
                    break 'conn;
                }
            }
        }
        if !wbuf.is_empty() {
            if stream.write_all(&wbuf).await.is_err() {
                break;
            }
            wbuf.clear();
        }
        let read = async {
            match idle {
                Some(t) => tokio::time::timeout(t, stream.read_buf(&mut rbuf))
                    .await
                    .unwrap_or(Ok(0)),
                None => stream.read_buf(&mut rbuf).await,
            }
        };
        tokio::select! {
            n = read => match n {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            },
            _ = kill.changed() => break,
            _ = stop.changed() => break,
        }
    }
    let _ = stream.shutdown().await;
    let mut clients = shared.clients.lock().unwrap_or_else(|e| e.into_inner());
    clients.remove(&id);
}

/// Whether a command's reply is written, given the reply mode before it ran
/// and the mode it left (`CLIENT REPLY`): `OFF` and `SKIP` are not replied
/// to; `SKIP` then drops exactly the next command's reply; `ON` is replied
/// to even when it ends `OFF`.
fn write_reply(before: ReplyMode, mode: &mut ReplyMode) -> bool {
    match (before, *mode) {
        // The command after `SKIP`: its reply is dropped, then replies resume.
        (ReplyMode::Skip, ReplyMode::Skip) => {
            *mode = ReplyMode::On;
            false
        }
        (_, ReplyMode::On) => true,
        _ => false,
    }
}

/// Seconds since the Unix epoch, and the microseconds past it.
pub fn unix_now() -> (u64, u32) {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_secs(), d.subsec_micros())
}

/// A client entry in `CLIENT LIST` / `CLIENT INFO` form.
pub fn client_line(id: u64, entry: &ClientEntry) -> String {
    let name = entry
        .name
        .as_ref()
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    format!(
        "id={id} addr={} laddr= fd=0 name={name} age={} idle=0 flags=N db={} sub=0 psub=0 ssub=0 multi=-1 watch=0 qbuf=0 qbuf-free=0 argv-mem=0 multi-mem=0 rbs=0 rbp=0 obl=0 oll=0 omem=0 tot-mem=0 events=r cmd={} user=default redir=-1 resp=2 lib-name={} lib-ver={}",
        entry.addr,
        entry.connected.elapsed().as_secs(),
        entry.db,
        entry.last_command.as_deref().unwrap_or("NULL"),
        entry.lib_name.as_deref().unwrap_or(""),
        entry.lib_ver.as_deref().unwrap_or(""),
    )
}

/// `text` as a verbatim string in RESP3, a bulk string in RESP2 (`INFO`,
/// `CLIENT LIST`).
pub fn verbatim(text: String) -> Value {
    Value::Verbatim("txt".into(), Bytes::from(text))
}

#[cfg(test)]
mod tests {
    use super::{ReplyMode, write_reply};

    #[test]
    fn client_reply_modes() {
        // An ordinary command with replies on.
        let mut mode = ReplyMode::On;
        assert!(write_reply(ReplyMode::On, &mut mode));
        // CLIENT REPLY OFF and SKIP are not replied to.
        let mut mode = ReplyMode::Off;
        assert!(!write_reply(ReplyMode::On, &mut mode));
        let mut mode = ReplyMode::Skip;
        assert!(!write_reply(ReplyMode::On, &mut mode));
        assert_eq!(mode, ReplyMode::Skip);
        // The command after SKIP is dropped, and replies resume.
        assert!(!write_reply(ReplyMode::Skip, &mut mode));
        assert_eq!(mode, ReplyMode::On);
        // CLIENT REPLY ON while off is replied to.
        let mut mode = ReplyMode::On;
        assert!(write_reply(ReplyMode::Off, &mut mode));
        // Commands while off are not.
        let mut mode = ReplyMode::Off;
        assert!(!write_reply(ReplyMode::Off, &mut mode));
    }
}
