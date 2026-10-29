//! Loopback-only, bounded CQL V4 listener.
//!
//! Remote listeners require TLS and authorization integration and are not
//! enabled by this initial server. V5 framing is not yet enabled.
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use cassandra_protocol::frame::{Direction, Envelope, Flags, Opcode, Serialize, message_error::{ErrorBody, ErrorType}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, sync::{Semaphore, watch}, task::{JoinHandle, JoinSet}, time::{Instant, timeout, timeout_at}};
use crate::{frame::{EnvelopeCodec, ProtocolCodec}, handshake::{Authenticator, Handshake, HandshakeError}, query::QueryExecutor, query_protocol::handle_query};

/// Listener limits applied independently to every connection.
#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    /// Largest permitted CQL message body.
    pub max_body_bytes: usize,
    /// Maximum concurrent client connections.
    pub max_connections: usize,
    /// Time from accept through successful authentication.
    pub handshake_timeout: Duration,
    /// Time allowed to read, process, or write an authenticated request.
    pub request_timeout: Duration,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self { max_body_bytes: 1024 * 1024, max_connections: 128, handshake_timeout: Duration::from_secs(10), request_timeout: Duration::from_secs(30) }
    }
}

/// A running loopback CQL listener.
#[derive(Debug)]
pub struct Server { address: SocketAddr, shutdown: watch::Sender<bool>, task: JoinHandle<io::Result<()>> }
impl Server {
    /// Bind a loopback listener and begin accepting authenticated clients.
    pub async fn start(address: SocketAddr, authenticator: Arc<dyn Authenticator>, executor: Arc<dyn QueryExecutor>, config: ServerConfig) -> io::Result<Self> {
        if !address.ip().is_loopback() { return Err(io::Error::new(io::ErrorKind::PermissionDenied,"CQL requires TLS off loopback")); }
        if config.max_body_bytes == 0 || config.max_body_bytes > i32::MAX as usize || config.max_connections == 0 || config.handshake_timeout.is_zero() || config.request_timeout.is_zero() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,"invalid CQL listener limits"));
        }
        let listener = TcpListener::bind(address).await?;
        let address = listener.local_addr()?;
        let (shutdown, mut receiver) = watch::channel(false);
        let task = tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(config.max_connections));
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    changed = receiver.changed() => { if changed.is_err() || *receiver.borrow() { break; } }
                    accepted = listener.accept() => {
                        let (stream, _) = accepted?;
                        if let Ok(permit) = permits.clone().try_acquire_owned() {
                            let auth = authenticator.clone(); let executor = executor.clone();
                            connections.spawn(async move { let _permit = permit; let _ = connection(stream, auth, executor, config).await; });
                        }
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
            Ok(())
        });
        Ok(Self { address, shutdown, task })
    }
    /// Bound loopback address, including the selected ephemeral port.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr { self.address }
    /// Stop the listener and its active connections.
    pub async fn shutdown(self) -> io::Result<()> {
        let _ = self.shutdown.send(true);
        self.task.await.map_err(io::Error::other)?
    }
}

async fn read_frame(stream: &mut TcpStream, codec: &EnvelopeCodec) -> io::Result<Envelope> {
    let mut header = [0_u8; 9];
    stream.read_exact(&mut header).await?;
    // Decoding the header first validates its length before allocation.
    if let Some((frame, _)) = codec.decode(&header).map_err(io::Error::other)? { return Ok(frame); }
    let len = usize::try_from(i32::from_be_bytes(header[5..9].try_into().expect("header length")))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData,"negative CQL length"))?;
    let mut buffer = Vec::with_capacity(9 + len);
    buffer.extend_from_slice(&header);
    buffer.resize(9 + len, 0);
    stream.read_exact(&mut buffer[9..]).await?;
    codec.decode(&buffer).map_err(io::Error::other)?.map(|(frame,_)| frame)
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof,"incomplete CQL frame"))
}

async fn write_frame(stream: &mut TcpStream, codec: &EnvelopeCodec, frame: &Envelope) -> io::Result<()> {
    stream.write_all(&codec.encode(frame).map_err(io::Error::other)?).await
}

fn rejection(request: &Envelope, error: HandshakeError) -> Envelope {
    let ty = match error { HandshakeError::Authentication => ErrorType::Authentication, HandshakeError::Protocol => ErrorType::Protocol };
    let body = ErrorBody { message: error.to_string(), ty }.serialize_to_vec(request.version);
    Envelope::new(request.version, Direction::Response, Flags::empty(), Opcode::Error, request.stream_id, body, None, vec![])
}

async fn connection(mut stream: TcpStream, authenticator: Arc<dyn Authenticator>, executor: Arc<dyn QueryExecutor>, config: ServerConfig) -> io::Result<()> {
    let codec = EnvelopeCodec::new(config.max_body_bytes);
    let deadline = Instant::now() + config.handshake_timeout;
    let mut handshake = Handshake::new(authenticator);
    while handshake.session().is_none() {
        let request = timeout_at(deadline, read_frame(&mut stream, &codec)).await.map_err(io::Error::other)??;
        let response = timeout_at(deadline, handshake.handle(request.clone())).await.map_err(io::Error::other)?;
        match response {
            Ok(response) => timeout_at(deadline, write_frame(&mut stream, &codec, &response)).await.map_err(io::Error::other)??,
            Err(error) => { let _ = timeout_at(deadline, write_frame(&mut stream, &codec, &rejection(&request,error))).await; return Ok(()); }
        }
    }
    let mut session = handshake.session().expect("authenticated above").clone();
    loop {
        let request = match timeout(config.request_timeout, read_frame(&mut stream, &codec)).await {
            Ok(Ok(request)) => request,
            Ok(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Err(error) => return Err(io::Error::other(error)),
        };
        let response = timeout(config.request_timeout, handle_query(executor.as_ref(), &mut session, request)).await.map_err(io::Error::other)?;
        timeout(config.request_timeout, write_frame(&mut stream, &codec, &response)).await.map_err(io::Error::other)??;
    }
}
