//! The admin listener (design §57 §5): `/healthz`, `/readyz` and
//! `/v1/routes`. HTTP/1.1 (pipelining) or HTTP/2 cleartext (prior knowledge)
//! on the pod network; never on the Service.

use std::convert::Infallible;
use std::future::Future;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::listeners::SharedStatus;
use crate::reload::RouteRx;

/// What the admin routes read.
#[derive(Debug, Clone)]
pub struct State {
    pub routes: RouteRx,
    pub listeners: SharedStatus,
}

/// Serves until `shutdown` resolves.
pub async fn serve(listener: TcpListener, state: State, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        let (stream, _) = tokio::select! {
            _ = &mut shutdown => return,
            a = listener.accept() => match a {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(error = %e, "admin accept failed");
                    // A persistent error (EMFILE) must not spin a core.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
            },
        };
        // Small replies on keep-alive connections: never wait for Nagle.
        let _ = stream.set_nodelay(true);
        let state = state.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                let state = state.clone();
                async move { Ok::<_, Infallible>(handle(&state, &req)) }
            });
            // HTTP/1.1 with pipelining, or HTTP/2 (prior knowledge, h2c)
            // with many streams per connection: the bytes decide. Pipelined
            // HTTP/1.1 responses are flushed together rather than one write
            // per response (BQ1-gw, "pipelining in every point").
            let mut builder =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
            builder.http1().pipeline_flush(true);
            let _ = builder
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

fn handle(state: &State, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    if req.method() != Method::GET {
        return reply(StatusCode::METHOD_NOT_ALLOWED, "GET only\n".into());
    }
    match req.uri().path() {
        "/healthz" => reply(StatusCode::OK, "ok\n".into()),
        "/readyz" => readyz(state),
        "/v1/routes" => routes(state),
        _ => reply(StatusCode::NOT_FOUND, "not found\n".into()),
    }
}

/// Ready once a route table is loaded. Dedicated ports that failed to bind
/// are reported, but the shared port still serves, so the replica stays
/// ready.
fn readyz(state: &State) -> Response<Full<Bytes>> {
    if state.routes.borrow().is_none() {
        return reply(StatusCode::SERVICE_UNAVAILABLE, "no routes loaded\n".into());
    }
    let failures = status_failures(state);
    if failures.is_empty() {
        reply(StatusCode::OK, "ready\n".into())
    } else {
        reply(
            StatusCode::OK,
            format!("ready, degraded: dedicated ports not bound: {failures:?}\n"),
        )
    }
}

/// The generation and counts: no hostnames, no upstreams.
fn routes(state: &State) -> Response<Full<Bytes>> {
    let table = state.routes.borrow().clone();
    let (bound, failures) = {
        let s = state
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            s.dedicated_bound.iter().copied().collect::<Vec<_>>(),
            s.bind_failures.keys().copied().collect::<Vec<_>>(),
        )
    };
    let body = serde_json::json!({
        "generation": table.as_ref().map(|t| t.generation.clone()),
        "pg_namespaces": table.as_ref().map_or(0, |t| t.pg.namespaces.len()),
        "dedicated_ports_bound": bound,
        "dedicated_ports_failed": failures,
    });
    let mut r = reply(StatusCode::OK, format!("{body}\n"));
    r.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    r
}

fn status_failures(state: &State) -> Vec<u16> {
    state
        .listeners
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .bind_failures
        .keys()
        .copied()
        .collect()
}

fn reply(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(body)));
    *r.status_mut() = status;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serving() -> (std::net::SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let addr = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
        let (_tx, routes) = crate::reload::channel();
        let state = State {
            routes,
            listeners: SharedStatus::default(),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            // Keep the routes sender alive for the server's life.
            let _tx = _tx;
            serve(listener, state, async move {
                let _ = stopped.await;
            })
            .await;
        });
        (addr, stop)
    }

    #[tokio::test]
    async fn pipelined_requests_on_one_connection_are_all_answered_in_order() {
        let (addr, _stop) = serving().await;
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let request = "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n".repeat(8)
            + "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n";
        stream
            .write_all(request.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let mut seen = String::new();
        let mut chunk = [0u8; 4096];
        while seen.matches("HTTP/1.1 ").count() < 9 || !seen.ends_with("not found\n") {
            let n =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut chunk))
                    .await
                    .unwrap_or_else(|_| panic!("stalled after: {seen}"))
                    .unwrap_or_else(|e| panic!("{e}"));
            assert!(n > 0, "closed after: {seen}");
            seen.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
        assert_eq!(seen.matches("HTTP/1.1 200 OK").count(), 8, "{seen}");
        assert!(
            seen.rfind("HTTP/1.1 404").unwrap_or(0) > seen.rfind("HTTP/1.1 200").unwrap_or(0),
            "answered in order: {seen}"
        );
    }

    #[tokio::test]
    async fn an_http2_prior_knowledge_client_is_served() {
        let (addr, _stop) = serving().await;
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        // The HTTP/2 connection preface, then an empty SETTINGS frame.
        stream
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let mut head = [0u8; 9];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read_exact(&mut head),
        )
        .await
        .unwrap_or_else(|_| panic!("no HTTP/2 answer"))
        .unwrap_or_else(|e| panic!("{e}"));
        // The server's first frame is its SETTINGS (type 4).
        assert_eq!(head[3], 0x04, "a SETTINGS frame: {head:?}");
    }
}
