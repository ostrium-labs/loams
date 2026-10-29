//! BQ1-gw: load generators and stand-ins for measuring loams-gateway.
//!
//! * `mock-upstream`: a stand-in browser engine (answers the WebSocket
//!   handshake with `101`, then echoes), so the browser module's proxy path is
//!   measured without Obscura's own cost.
//! * `upgrade`: handshakes per second through the browser module (connect,
//!   authenticated upgrade, `101`, close): the per-connection L7 cost.
//! * `pingpong`: messages per second over established spliced sessions: the
//!   steady-state L4 proxy cost.
//! * `hold`: opens and holds sessions, for connections per GB of gateway RSS.
//! * `tls-server` / `tls`: TLS 1.3 handshakes per second against an acceptor
//!   built from `loams_gateway::tls::ServerCerts` (the gateway's own rustls
//!   configuration).
//! * `auth`: the browser module's per-connection token check, in-process.
//!
//! Every client command prints one JSON object on stdout.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser)]
#[command(name = "gw-bench")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// A stand-in engine: 101 to any handshake, then echo.
    MockUpstream {
        #[arg(long)]
        listen: SocketAddr,
    },
    /// Authenticated WebSocket handshakes per second through the browser module.
    Upgrade {
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long)]
        token: String,
        #[arg(long, default_value_t = 64)]
        concurrency: usize,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// Echoed messages per second over established spliced sessions.
    Pingpong {
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long)]
        token: String,
        #[arg(long, default_value_t = 64)]
        conns: usize,
        #[arg(long, default_value_t = 64)]
        size: usize,
        /// Messages in flight per session.
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// Open `conns` sessions and hold them for `seconds`.
    Hold {
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long)]
        token: String,
        #[arg(long)]
        conns: usize,
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
    /// A TLS acceptor built from the gateway's ServerCerts.
    TlsServer {
        #[arg(long)]
        listen: SocketAddr,
    },
    /// TLS handshakes per second.
    Tls {
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long, default_value_t = 64)]
        concurrency: usize,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// HTTP/1.1 keep-alive GETs, optionally pipelined: a lean load generator
    /// for when oha is the limit (it records every request's latency in one
    /// place; this keeps per-connection counters and samples latency).
    Http {
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long, default_value = "/healthz")]
        path: String,
        #[arg(long, default_value_t = 256)]
        conns: usize,
        /// Requests in flight per connection.
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// The browser module's token check, per call.
    Auth {
        /// A client tokens file to create and read.
        #[arg(long)]
        file: PathBuf,
        /// How many client digests the file lists.
        #[arg(long, default_value_t = 100)]
        tokens: usize,
        #[arg(long, default_value_t = 200_000)]
        iterations: usize,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::MockUpstream { listen } => mock_upstream(listen).await,
        Command::Upgrade {
            addr,
            token,
            concurrency,
            seconds,
        } => {
            let head = handshake_head(&addr, &token);
            let report = closed_loop(concurrency, seconds, move || {
                let head = head.clone();
                async move {
                    let mut stream = TcpStream::connect(addr).await?;
                    stream.set_nodelay(true)?;
                    stream.write_all(head.as_bytes()).await?;
                    read_101(&mut stream).await?;
                    Ok(())
                }
            })
            .await;
            print(
                json!({ "bench": "browser-upgrade", "concurrency": concurrency, "result": report }),
            );
            Ok(())
        }
        Command::Pingpong {
            addr,
            token,
            conns,
            size,
            depth,
            seconds,
        } => pingpong(addr, &token, conns, size, depth.max(1), seconds).await,
        Command::Hold {
            addr,
            token,
            conns,
            seconds,
        } => hold(addr, &token, conns, seconds).await,
        Command::TlsServer { listen } => tls_server(listen).await,
        Command::Tls {
            addr,
            concurrency,
            seconds,
        } => {
            let connector = tokio_rustls::TlsConnector::from(tls_client_config());
            let name = rustls::pki_types::ServerName::try_from("bench.gateway.test")?.to_owned();
            let report = closed_loop(concurrency, seconds, move || {
                let connector = connector.clone();
                let name = name.clone();
                async move {
                    let stream = TcpStream::connect(addr).await?;
                    stream.set_nodelay(true)?;
                    let mut tls = connector.connect(name, stream).await?;
                    tls.shutdown().await.ok();
                    Ok(())
                }
            })
            .await;
            print(
                json!({ "bench": "tls-handshake", "concurrency": concurrency, "result": report }),
            );
            Ok(())
        }
        Command::Auth {
            file,
            tokens,
            iterations,
        } => auth(&file, tokens, iterations),
        Command::Http {
            addr,
            path,
            conns,
            depth,
            seconds,
        } => http(addr, &path, conns, depth, seconds).await,
    }
}

fn print(value: serde_json::Value) {
    println!("{value}");
}

fn handshake_head(addr: &SocketAddr, token: &str) -> String {
    format!(
        "GET /devtools/browser HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n"
    )
}

/// Read up to the end of a response head and require `101`.
async fn read_101(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(128);
    let mut chunk = [0u8; 256];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::other("closed before the response"));
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    if buffer.starts_with(b"HTTP/1.1 101") {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "not 101: {}",
            String::from_utf8_lossy(&buffer[..buffer.len().min(40)])
        )))
    }
}

/// Run `op` from `concurrency` workers back to back for `seconds`.
async fn closed_loop<F, Fut>(concurrency: usize, seconds: u64, op: F) -> serde_json::Value
where
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = std::io::Result<()>> + Send,
{
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let op = op.clone();
        let stop = Arc::clone(&stop);
        workers.push(tokio::spawn(async move {
            let mut latencies: Vec<u32> = Vec::with_capacity(1 << 16);
            let mut errors = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                match op().await {
                    Ok(()) => latencies.push(micros(t.elapsed())),
                    Err(_) => errors += 1,
                }
            }
            (latencies, errors)
        }));
    }
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    stop.store(true, Ordering::Relaxed);
    let mut all = Vec::new();
    let mut errors = 0;
    for worker in workers {
        if let Ok((latencies, e)) = worker.await {
            all.extend(latencies);
            errors += e;
        }
    }
    summarize(all, errors, started.elapsed())
}

fn micros(duration: Duration) -> u32 {
    u32::try_from(duration.as_micros()).unwrap_or(u32::MAX)
}

fn summarize(mut latencies: Vec<u32>, errors: u64, elapsed: Duration) -> serde_json::Value {
    latencies.sort_unstable();
    let at = |q: f64| -> u32 {
        if latencies.is_empty() {
            return 0;
        }
        let index = ((latencies.len() as f64 - 1.0) * q).round() as usize;
        latencies[index.min(latencies.len() - 1)]
    };
    json!({
        "ops": latencies.len(),
        "errors": errors,
        "seconds": elapsed.as_secs_f64(),
        "ops_per_sec": latencies.len() as f64 / elapsed.as_secs_f64(),
        "p50_us": at(0.50),
        "p99_us": at(0.99),
        "p999_us": at(0.999),
        "max_us": latencies.last().copied().unwrap_or(0),
    })
}

async fn mock_upstream(listen: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(listen).await?;
    eprintln!("mock-upstream listening on {}", listener.local_addr()?);
    loop {
        let (mut stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let _ = stream.set_nodelay(true);
            let mut buffer = Vec::with_capacity(1024);
            let mut chunk = [0u8; 1024];
            // The head first.
            loop {
                let Ok(n) = stream.read(&mut chunk).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    buffer.drain(..end + 4);
                    break;
                }
            }
            if stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            if !buffer.is_empty() && stream.write_all(&buffer).await.is_err() {
                return;
            }
            let mut echo = [0u8; 16 * 1024];
            loop {
                match stream.read(&mut echo).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        if stream.write_all(&echo[..n]).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
    }
}

async fn open_session(addr: SocketAddr, head: &str) -> std::io::Result<TcpStream> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    stream.write_all(head.as_bytes()).await?;
    read_101(&mut stream).await?;
    Ok(stream)
}

async fn pingpong(
    addr: SocketAddr,
    token: &str,
    conns: usize,
    size: usize,
    depth: usize,
    seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let head = handshake_head(&addr, token);
    let mut sessions = Vec::with_capacity(conns);
    for _ in 0..conns {
        sessions.push(open_session(addr, &head).await?);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let mut workers = Vec::new();
    for mut stream in sessions {
        let stop = Arc::clone(&stop);
        workers.push(tokio::spawn(async move {
            // `depth` messages written back to back, then their echoes read.
            let message = vec![b'x'; size * depth];
            let mut reply = vec![0u8; size * depth];
            let mut latencies: Vec<u32> = Vec::with_capacity(1 << 16);
            let mut errors = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                let ok = stream.write_all(&message).await.is_ok()
                    && stream.read_exact(&mut reply).await.is_ok();
                if ok {
                    let took = micros(t.elapsed());
                    for _ in 0..depth {
                        latencies.push(took);
                    }
                } else {
                    errors += 1;
                    break;
                }
            }
            (latencies, errors)
        }));
    }
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    stop.store(true, Ordering::Relaxed);
    let mut all = Vec::new();
    let mut errors = 0;
    for worker in workers {
        if let Ok((latencies, e)) = worker.await {
            all.extend(latencies);
            errors += e;
        }
    }
    let report = summarize(all, errors, started.elapsed());
    print(
        json!({ "bench": "browser-pingpong", "conns": conns, "size": size, "depth": depth, "result": report }),
    );
    Ok(())
}

async fn hold(
    addr: SocketAddr,
    token: &str,
    conns: usize,
    seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let head = handshake_head(&addr, token);
    let mut sessions = Vec::with_capacity(conns);
    let mut failed = 0usize;
    for _ in 0..conns {
        match open_session(addr, &head).await {
            Ok(stream) => sessions.push(stream),
            Err(_) => failed += 1,
        }
    }
    print(json!({ "bench": "hold", "held": sessions.len(), "failed": failed }));
    // The orchestrator samples the gateway's RSS now.
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    drop(sessions);
    Ok(())
}

/// A self-signed certificate for `bench.gateway.test`, loaded through the
/// gateway's own `ServerCerts`, so the server side is the gateway's rustls
/// configuration (ring, safe defaults, ALPN `postgresql`).
async fn tls_server(listen: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let generated = rcgen::generate_simple_self_signed(vec!["bench.gateway.test".to_string()])?;
    let mut certs = loams_gateway::tls::ServerCerts::default();
    certs.add(
        &["bench.gateway.test".to_string()],
        generated.cert.pem().as_bytes(),
        generated.signing_key.serialize_pem().as_bytes(),
    )?;
    let config = certs
        .config_for_name("bench.gateway.test")
        .ok_or("no config for the bench name")?;
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let listener = TcpListener::bind(listen).await?;
    eprintln!("tls-server listening on {}", listener.local_addr()?);
    loop {
        let (stream, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let _ = stream.set_nodelay(true);
            if let Ok(mut tls) = acceptor.accept(stream).await {
                let mut byte = [0u8; 1];
                let _ = tls.read(&mut byte).await;
            }
        });
    }
}

/// A client that accepts any certificate (the bench measures handshakes, not
/// trust) and offers ALPN `postgresql`, as the gateway requires.
fn tls_client_config() -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap_or_else(|error| panic!("{error}"))
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"postgresql".to_vec()];
    // A fresh handshake every time: no session resumption.
    config.resumption = rustls::client::Resumption::disabled();
    Arc::new(config)
}

#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The browser module's token check: the file read and parse it does on every
/// connection, then the digest and the lookup; and the check alone.
fn auth(
    file: &PathBuf,
    tokens: usize,
    iterations: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut text = String::from("# bench clients\n");
    for index in 0..tokens {
        text.push_str(&loams_gateway::browser::digest(&format!(
            "client-token-{index:032}"
        )));
        text.push('\n');
    }
    std::fs::write(file, &text)?;
    let token = format!("client-token-{:032}", tokens / 2);

    let started = Instant::now();
    let mut hits = 0usize;
    for _ in 0..iterations {
        let allowed = loams_gateway::browser::digests(&std::fs::read_to_string(file)?);
        if allowed.contains(&loams_gateway::browser::digest(&token)) {
            hits += 1;
        }
    }
    let per_connection = started.elapsed();

    let allowed = loams_gateway::browser::digests(&text);
    let started = Instant::now();
    for _ in 0..iterations {
        if allowed.contains(&loams_gateway::browser::digest(&token)) {
            hits += 1;
        }
    }
    let lookup_only = started.elapsed();

    // What the module does since BQ1-gw: stat, then the cached set.
    let watched =
        loams_gateway::browser::WatchedFile::new(file.clone(), loams_gateway::browser::digests);
    let started = Instant::now();
    for _ in 0..iterations {
        if watched
            .get()?
            .contains(&loams_gateway::browser::digest(&token))
        {
            hits += 1;
        }
    }
    let watched_check = started.elapsed();
    std::fs::remove_file(file).ok();

    let ns = |d: Duration| d.as_nanos() as f64 / iterations as f64;
    print(json!({
        "bench": "auth",
        "tokens": tokens,
        "iterations": iterations,
        "hits": hits,
        "read_parse_digest_lookup_ns": ns(per_connection),
        "digest_lookup_ns": ns(lookup_only),
        "stat_cached_digest_lookup_ns": ns(watched_check),
    }));
    Ok(())
}

/// Find one complete HTTP/1.1 response at the start of `buffer`; answers its
/// length, or `None` when more bytes are needed.
fn response_len(buffer: &[u8]) -> Option<usize> {
    let end = buffer.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = std::str::from_utf8(&buffer[..end]).ok()?;
    let body = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    (buffer.len() >= end + body).then_some(end + body)
}

async fn http(
    addr: SocketAddr,
    path: &str,
    conns: usize,
    depth: usize,
    seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n").repeat(depth.max(1));
    let request = Arc::new(request.into_bytes());
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let mut workers = Vec::with_capacity(conns);
    for _ in 0..conns {
        let request = Arc::clone(&request);
        let stop = Arc::clone(&stop);
        let depth = depth.max(1);
        workers.push(tokio::spawn(async move {
            let mut done = 0u64;
            let mut errors = 0u64;
            // Every 64th batch's round trip, so the sample costs nothing.
            let mut samples: Vec<u32> = Vec::with_capacity(4096);
            let Ok(mut stream) = TcpStream::connect(addr).await else {
                return (0, 1, samples);
            };
            let _ = stream.set_nodelay(true);
            let mut buffer: Vec<u8> = Vec::with_capacity(16 * 1024);
            let mut chunk = vec![0u8; 16 * 1024];
            let mut batch = 0u64;
            'outer: while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                if stream.write_all(&request).await.is_err() {
                    errors += 1;
                    break;
                }
                let mut seen = 0;
                while seen < depth {
                    while let Some(len) = response_len(&buffer) {
                        if !buffer.starts_with(b"HTTP/1.1 200") {
                            errors += 1;
                        }
                        buffer.drain(..len);
                        seen += 1;
                        if seen == depth {
                            break;
                        }
                    }
                    if seen == depth {
                        break;
                    }
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => {
                            errors += 1;
                            break 'outer;
                        }
                        Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    }
                }
                done += depth as u64;
                batch += 1;
                if batch % 64 == 0 {
                    samples.push(micros(t.elapsed()));
                }
            }
            (done, errors, samples)
        }));
    }
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    stop.store(true, Ordering::Relaxed);
    let mut total = 0u64;
    let mut errors = 0u64;
    let mut samples = Vec::new();
    for worker in workers {
        if let Ok((d, e, s)) = worker.await {
            total += d;
            errors += e;
            samples.extend(s);
        }
    }
    let elapsed = started.elapsed();
    let mut latency = summarize(samples, 0, elapsed);
    if let Some(object) = latency.as_object_mut() {
        object.remove("ops");
        object.remove("ops_per_sec");
        object.remove("errors");
        object.remove("seconds");
    }
    print(json!({
        "bench": "http-lean",
        "path": path,
        "conns": conns,
        "depth": depth,
        "requests": total,
        "errors": errors,
        "rps": total as f64 / elapsed.as_secs_f64(),
        "batch_round_trip_sampled": latency,
    }));
    Ok(())
}
