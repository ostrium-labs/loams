//! CT1 Task 2 (design §83): one axum router over HTTP/3 (quinn + h3) and
//! HTTP/2 (hyper + tokio-rustls), both with mTLS and SPIFFE URI SANs from a
//! throwaway CA, measured briefly on loopback. A spike: it answers "does the
//! stack fit together, and what breaks", not "which is faster" (Task 11).
//!
//! Run: `cargo run --release` (prints JSON lines and a summary).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Path};
use axum::routing::{get, post};
use bytes::{Buf, Bytes};
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::RootCertStore;
use rustls::server::WebPkiClientVerifier;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tower::ServiceExt;

const CLUSTER: &str = "spike";
const CHUNK: usize = 64 * 1024;
static ZEROS: [u8; CHUNK] = [0u8; CHUNK];

/// The verified peer's SPIFFE ID, put in each request's extensions.
#[derive(Clone, Debug)]
struct PeerId(String);

struct Leaf {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

struct Pki {
    ca: CertificateDer<'static>,
    server: Leaf,
    client: Leaf,
    /// A client from another CA, which both listeners must refuse.
    stranger: Leaf,
}

fn ca_and_leaves(ids: &[&str]) -> Result<(CertificateDer<'static>, Vec<Leaf>)> {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
        KeyUsagePurpose, SanType,
    };
    let ca_key = KeyPair::generate()?; // ECDSA P-256 (D2306)
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_key)?;
    let issuer = Issuer::new(ca_params, &ca_key);
    let mut leaves = Vec::new();
    for id in ids {
        let key = KeyPair::generate()?;
        let mut p = CertificateParams::new(vec!["localhost".to_string()])?;
        p.subject_alt_names
            .push(SanType::URI(format!("spiffe://{CLUSTER}/{id}").as_str().try_into()?));
        p.subject_alt_names
            .push(SanType::IpAddress("127.0.0.1".parse()?));
        p.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let cert = p.signed_by(&key, &issuer)?;
        leaves.push(Leaf {
            chain: vec![cert.der().clone()],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        });
    }
    Ok((ca_cert.der().clone(), leaves))
}

fn pki() -> Result<Pki> {
    let (ca, mut leaves) = ca_and_leaves(&["node/1", "role/gateway"])?;
    let client = leaves.pop().context("client")?;
    let server = leaves.pop().context("server")?;
    let (_, mut other) = ca_and_leaves(&["role/gateway"])?;
    let stranger = other.pop().context("stranger")?;
    Ok(Pki { ca, server, client, stranger })
}

fn roots(ca: &CertificateDer<'static>) -> Result<Arc<RootCertStore>> {
    let mut r = RootCertStore::empty();
    r.add(ca.clone())?;
    Ok(Arc::new(r))
}

fn clone_leaf(l: &Leaf) -> Leaf {
    Leaf { chain: l.chain.clone(), key: l.key.clone_key() }
}

fn server_tls(pki: &Pki, alpn: &[u8]) -> Result<rustls::ServerConfig> {
    let verifier = WebPkiClientVerifier::builder(roots(&pki.ca)?).build()?;
    let leaf = clone_leaf(&pki.server);
    let mut cfg = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_client_cert_verifier(verifier)
        .with_single_cert(leaf.chain, leaf.key)?;
    cfg.alpn_protocols = vec![alpn.to_vec()];
    cfg.max_early_data_size = 0; // 0-RTT off (D2318)
    Ok(cfg)
}

fn client_tls(ca: &CertificateDer<'static>, leaf: &Leaf, alpn: &[u8]) -> Result<rustls::ClientConfig> {
    let leaf = clone_leaf(leaf);
    let mut cfg = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(roots(ca)?)
        .with_client_auth_cert(leaf.chain, leaf.key)?;
    cfg.alpn_protocols = vec![alpn.to_vec()];
    cfg.enable_early_data = false;
    Ok(cfg)
}

/// The SPIFFE URI SAN of a verified leaf (the authorization input, D2315).
fn spiffe_of(cert: &CertificateDer<'_>) -> Result<String> {
    let (_, x) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|e| anyhow!("parse peer certificate: {e}"))?;
    let san = x
        .subject_alternative_name()
        .map_err(|e| anyhow!("SAN: {e}"))?
        .context("peer certificate has no SAN")?;
    for name in &san.value.general_names {
        if let x509_parser::extensions::GeneralName::URI(u) = name
            && u.starts_with("spiffe://")
        {
            return Ok((*u).to_string());
        }
    }
    bail!("peer certificate has no SPIFFE URI SAN")
}

fn router() -> Router {
    Router::new()
        .route("/whoami", get(|Extension(PeerId(id)): Extension<PeerId>| async move { id }))
        .route("/echo", post(|body: Bytes| async move { body }))
        .route(
            "/stream/{n}",
            get(|Path(n): Path<usize>| async move {
                let chunks = n / CHUNK;
                let tail = n % CHUNK;
                let it = (0..chunks)
                    .map(|_| Bytes::from_static(&ZEROS))
                    .chain((tail > 0).then(|| Bytes::from_static(&ZEROS[..tail])))
                    .map(Ok::<_, std::io::Error>);
                Body::from_stream(futures::stream::iter(it))
            }),
        )
}

// ---------------------------------------------------------------- HTTP/3

async fn serve_h3(endpoint: quinn::Endpoint, app: Router) {
    while let Some(incoming) = endpoint.accept().await {
        let app = app.clone();
        tokio::spawn(async move {
            let Ok(conn) = incoming.await else { return };
            let Some(id) = conn
                .peer_identity()
                .and_then(|a| a.downcast::<Vec<CertificateDer<'static>>>().ok())
                .and_then(|certs| certs.first().and_then(|c| spiffe_of(c).ok()))
            else {
                conn.close(0u32.into(), b"no identity");
                return;
            };
            let Ok(mut h3c) = h3::server::Connection::<_, Bytes>::new(h3_quinn::Connection::new(conn)).await
            else {
                return;
            };
            loop {
                match h3c.accept().await {
                    Ok(Some(resolver)) => {
                        let app = app.clone();
                        let id = id.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_h3(resolver, app, id).await {
                                eprintln!("h3 request: {e:#}");
                            }
                        });
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        });
    }
}

async fn handle_h3(
    resolver: h3::server::RequestResolver<h3_quinn::Connection, Bytes>,
    app: Router,
    id: String,
) -> Result<()> {
    let (req, mut stream) = resolver.resolve_request().await?;
    let mut body = Vec::new();
    while let Some(mut chunk) = stream.recv_data().await? {
        let n = chunk.remaining();
        body.extend_from_slice(&chunk.copy_to_bytes(n));
    }
    let (parts, ()) = req.into_parts();
    let mut req = Request::from_parts(parts, Body::from(body));
    req.extensions_mut().insert(PeerId(id));
    let resp = app.oneshot(req).await?;
    let (parts, mut body) = resp.into_parts();
    stream.send_response(Response::from_parts(parts, ())).await?;
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame?.into_data() {
            stream.send_data(data).await?;
        }
    }
    stream.finish().await?;
    Ok(())
}

struct H3Client {
    send: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    _endpoint: quinn::Endpoint,
}

async fn h3_connect(addr: SocketAddr, cfg: rustls::ClientConfig) -> Result<H3Client> {
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    let quic = quinn::crypto::rustls::QuicClientConfig::try_from(cfg)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quic)));
    let conn = endpoint.connect(addr, "localhost")?.await?;
    let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn)).await?;
    tokio::spawn(async move {
        let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });
    Ok(H3Client { send, _endpoint: endpoint })
}

impl H3Client {
    async fn call(&self, req: Request<()>, body: Option<Bytes>) -> Result<(u16, usize, Bytes)> {
        let mut s = self.send.clone().send_request(req).await?;
        if let Some(b) = body {
            s.send_data(b).await?;
        }
        s.finish().await?;
        let resp = s.recv_response().await?;
        let mut n = 0usize;
        let mut first = Vec::new();
        while let Some(mut c) = s.recv_data().await? {
            let len = c.remaining();
            if first.len() < 256 {
                first.extend_from_slice(&c.copy_to_bytes(len));
            } else {
                c.advance(len);
            }
            n += len;
        }
        Ok((resp.status().as_u16(), n, Bytes::from(first)))
    }
}

// ---------------------------------------------------------------- HTTP/2

async fn serve_h2(listener: tokio::net::TcpListener, tls: rustls::ServerConfig, app: Router) {
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    loop {
        let Ok((tcp, _)) = listener.accept().await else { continue };
        let _ = tcp.set_nodelay(true);
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(tcp).await else { return };
            let Some(id) = tls
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|c| c.first())
                .and_then(|c| spiffe_of(c).ok())
            else {
                return;
            };
            let svc = hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| {
                let mut req = req.map(Body::new);
                req.extensions_mut().insert(PeerId(id.clone()));
                app.clone().oneshot(req)
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .max_concurrent_streams(256)
                .serve_connection(TokioIo::new(tls), svc)
                .await;
        });
    }
}

#[derive(Clone)]
struct H2Client {
    send: hyper::client::conn::http2::SendRequest<Full<Bytes>>,
}

async fn h2_connect(addr: SocketAddr, cfg: rustls::ClientConfig) -> Result<H2Client> {
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    tcp.set_nodelay(true)?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from("localhost")?, tcp)
        .await?;
    let (send, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls)).await?;
    tokio::spawn(conn);
    Ok(H2Client { send })
}

impl H2Client {
    async fn call(&self, req: Request<()>, body: Option<Bytes>) -> Result<(u16, usize, Bytes)> {
        let (parts, ()) = req.into_parts();
        let req = Request::from_parts(parts, Full::new(body.unwrap_or_default()));
        let resp = self.send.clone().send_request(req).await?;
        let status = resp.status().as_u16();
        let mut body = resp.into_body();
        let mut n = 0usize;
        let mut first = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(d) = frame?.into_data() {
                if first.len() < 256 {
                    first.extend_from_slice(&d);
                }
                n += d.len();
            }
        }
        Ok((status, n, Bytes::from(first)))
    }
}

// ---------------------------------------------------------------- the run

enum Client {
    H3(H3Client),
    H2(H2Client),
}

impl Client {
    async fn call(&self, req: Request<()>, body: Option<Bytes>) -> Result<(u16, usize, Bytes)> {
        match self {
            Client::H3(c) => c.call(req, body).await,
            Client::H2(c) => c.call(req, body).await,
        }
    }
}

fn uri(path: &str) -> String {
    format!("https://localhost{path}")
}

fn pct(sorted: &[Duration], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i].as_secs_f64() * 1e3
}

async fn run(name: &str, connect: impl Fn() -> futures::future::BoxFuture<'static, Result<Client>>) -> Result<()> {
    // Handshake: connect + first request, 20 times.
    let mut hs = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let c = connect().await?;
        let (st, _, who) = c.call(Request::get(uri("/whoami")).body(())?, None).await?;
        hs.push(t.elapsed());
        if st != 200 || who != Bytes::from(format!("spiffe://{CLUSTER}/role/gateway")) {
            bail!("{name}: whoami answered {st} {who:?}");
        }
    }
    hs.sort();
    // Unary 1 KiB echo, 64 in flight, 200 each.
    let c = Arc::new(connect().await?);
    let payload = Bytes::from(vec![7u8; 1024]);
    let t = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let c = c.clone();
        let payload = payload.clone();
        tasks.push(tokio::spawn(async move {
            let mut lat = Vec::with_capacity(200);
            for _ in 0..200 {
                let s = Instant::now();
                let (st, n, _) = c.call(Request::post(uri("/echo")).body(())?, Some(payload.clone())).await?;
                if st != 200 || n != 1024 {
                    bail!("echo answered {st} with {n} bytes");
                }
                lat.push(s.elapsed());
            }
            Ok::<_, anyhow::Error>(lat)
        }));
    }
    let mut lat = Vec::new();
    for t in tasks {
        lat.extend(t.await??);
    }
    let unary_secs = t.elapsed().as_secs_f64();
    lat.sort();
    // One 64 MiB stream, three times; the byte count must be exact (h3 #360).
    let mut gbps = Vec::new();
    for _ in 0..3 {
        let n = 64 * 1024 * 1024;
        let t = Instant::now();
        let (st, got, _) = c.call(Request::get(uri(&format!("/stream/{n}"))).body(())?, None).await?;
        if st != 200 || got != n {
            bail!("{name}: stream answered {st} with {got} of {n} bytes");
        }
        gbps.push(n as f64 * 8.0 / t.elapsed().as_secs_f64() / 1e9);
    }
    println!(
        "{{\"transport\":\"{name}\",\"handshake_p50_ms\":{:.2},\"handshake_p99_ms\":{:.2},\"unary_rps\":{:.0},\"unary_p50_ms\":{:.3},\"unary_p99_ms\":{:.3},\"stream_gbps\":[{:.2},{:.2},{:.2}]}}",
        pct(&hs, 0.5),
        pct(&hs, 0.99),
        lat.len() as f64 / unary_secs,
        pct(&lat, 0.5),
        pct(&lat, 0.99),
        gbps[0],
        gbps[1],
        gbps[2]
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let pki = Arc::new(pki()?);
    let app = router();

    // HTTP/3 on UDP and HTTP/2 on TCP, the same port number (D2330).
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp.local_addr()?;
    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(server_tls(&pki, b"h3")?)?;
    let h3_endpoint = quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(quic)), addr)?;
    tokio::spawn(serve_h3(h3_endpoint, app.clone()));
    tokio::spawn(serve_h2(tcp, server_tls(&pki, b"h2")?, app));
    eprintln!("serving h3 (udp) and h2 (tcp) on {addr}");

    // Refusals first: no client certificate, and a certificate from another CA.
    let stranger_h3 = h3_connect(addr, client_tls(&pki.ca, &pki.stranger, b"h3")?).await;
    let stranger_h3 = match stranger_h3 {
        Err(_) => true,
        Ok(c) => c.call(Request::get(uri("/whoami")).body(())?, None).await.is_err(),
    };
    let stranger_h2 = match h2_connect(addr, client_tls(&pki.ca, &pki.stranger, b"h2")?).await {
        Err(_) => true,
        Ok(c) => c.call(Request::get(uri("/whoami")).body(())?, None).await.is_err(),
    };
    println!("{{\"refuses_foreign_ca\":{{\"h3\":{stranger_h3},\"h2\":{stranger_h2}}}}}");

    let p3 = pki.clone();
    run("h3", move || {
        let p = p3.clone();
        Box::pin(async move { Ok(Client::H3(h3_connect(addr, client_tls(&p.ca, &p.client, b"h3")?).await?)) })
    })
    .await?;
    let p2 = pki.clone();
    run("h2", move || {
        let p = p2.clone();
        Box::pin(async move { Ok(Client::H2(h2_connect(addr, client_tls(&p.ca, &p.client, b"h2")?).await?)) })
    })
    .await?;
    Ok(())
}
