//! MK1 Task 4b: the HTTPS fetcher against a local TLS server with a test CA.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use loams_agentd_marketplace::HttpsFetcher;
use loams_agentd_marketplace::market::Fetcher;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves `routes` (path → status and body) over TLS on 127.0.0.1 for a
/// certificate naming `localhost`. Answers the base URL and the CA's DER.
async fn serve(routes: Vec<(&'static str, u16, Vec<u8>)>) -> (String, Vec<u8>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_der = certified.cert.der().to_vec();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert_der.clone().into()], key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let routes = routes.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = tls.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let line = String::from_utf8_lossy(&request);
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, b"not found".to_vec()));
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = tls.write_all(head.as_bytes()).await;
                let _ = tls.write_all(&body).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (format!("https://localhost:{port}"), cert_der)
}

#[tokio::test]
async fn fetches_over_tls_from_a_trusted_server() {
    let (base, ca) = serve(vec![("/index.json", 200, b"{\"ok\":true}".to_vec())]).await;
    let fetcher = HttpsFetcher::with_root_certificate(&ca).unwrap();
    let body = fetcher.get(&format!("{base}/index.json")).await.unwrap();
    assert_eq!(body, b"{\"ok\":true}");
}

#[tokio::test]
async fn an_untrusted_certificate_is_refused() {
    let (base, _ca) = serve(vec![("/index.json", 200, b"{}".to_vec())]).await;
    let fetcher = HttpsFetcher::new().unwrap();
    let error = fetcher
        .get(&format!("{base}/index.json"))
        .await
        .unwrap_err();
    assert!(error.contains("index.json"), "{error}");
}

#[tokio::test]
async fn an_error_status_is_an_error() {
    let (base, ca) = serve(vec![("/gone.json", 410, b"gone".to_vec())]).await;
    let fetcher = HttpsFetcher::with_root_certificate(&ca).unwrap();
    let error = fetcher.get(&format!("{base}/gone.json")).await.unwrap_err();
    assert!(error.starts_with("HTTP 410"), "{error}");
    let error = fetcher
        .get(&format!("{base}/missing.json"))
        .await
        .unwrap_err();
    assert!(error.starts_with("HTTP 404"), "{error}");
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused() {
    let (base, ca) = serve(vec![("/big.json", 200, vec![b'x'; 4096])]).await;
    let fetcher = HttpsFetcher::with_root_certificate(&ca)
        .unwrap()
        .with_max_bytes(1024);
    let error = fetcher.get(&format!("{base}/big.json")).await.unwrap_err();
    assert!(error.contains("larger than 1024"), "{error}");
}

#[tokio::test]
async fn plain_http_is_refused_without_a_request() {
    let fetcher = HttpsFetcher::new().unwrap();
    let error = fetcher
        .get("http://127.0.0.1:9/index.json")
        .await
        .unwrap_err();
    assert!(error.starts_with("refusing a non-https URL"), "{error}");
}
