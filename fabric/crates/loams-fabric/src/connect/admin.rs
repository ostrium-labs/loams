//! The admin API (D1821): loopback by default (`--admin-listen`), a bearer
//! token from `--admin-token-file` (required on any non-loopback address).
//!
//! ```text
//! GET   /health                                  no token; 200 once instances are loaded
//! GET   /v1/connect/instances                    every instance's status
//! GET   /v1/connect/instances/{id}
//! POST  /v1/connect/instances/{id}:pause
//! POST  /v1/connect/instances/{id}:resume
//! POST  /v1/connect/instances/{id}:restart       (a key rotation's restart)
//! PATCH /v1/connect/instances/{id}               {"rate_limit": {"bytes_per_sec": N}} or null
//! ```

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use loams_connector_sdk::Secret;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util_free::CancelOnDrop;

use super::supervisor::{AdminError, RateLimitBody, Supervisor};

/// The largest request body the admin API reads.
const MAX_BODY: usize = 64 << 10;

/// The running admin API.
#[derive(Debug)]
pub struct Admin {
    addr: SocketAddr,
    _task: CancelOnDrop,
}

impl Admin {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Bind and serve. `token: None` is allowed on loopback only.
pub async fn serve(
    listen: SocketAddr,
    token: Option<Secret>,
    supervisor: Arc<Supervisor>,
) -> Result<Admin, String> {
    if token.is_none() && !listen.ip().is_loopback() {
        return Err(format!(
            "--admin-listen {listen} is not loopback: it needs --admin-token-file"
        ));
    }
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|e| format!("--admin-listen {listen}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let token = token.map(Arc::new);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let supervisor = supervisor.clone();
            let token = token.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req| {
                    let supervisor = supervisor.clone();
                    let token = token.clone();
                    async move { Ok::<_, Infallible>(handle(req, token.as_deref(), &supervisor).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    Ok(Admin {
        addr,
        _task: CancelOnDrop(task),
    })
}

/// Aborts the server task when the [`Admin`] is dropped.
mod tokio_util_free {
    #[derive(Debug)]
    pub struct CancelOnDrop(pub tokio::task::JoinHandle<()>);

    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
}

fn reply(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

fn error(status: StatusCode, code: &str, message: &str) -> Response<Full<Bytes>> {
    reply(status, &json!({"code": code, "message": message}))
}

/// Constant-time comparison of the presented token with the expected one.
fn token_ok(headers: &http::HeaderMap, expected: &Secret) -> bool {
    let Some(presented) = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.as_bytes().strip_prefix(b"Bearer "))
    else {
        return false;
    };
    let want = expected.expose();
    if presented.len() != want.len() {
        return false;
    }
    presented
        .iter()
        .zip(want)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn admin_error(e: AdminError) -> Response<Full<Bytes>> {
    match e {
        AdminError::NotFound(n) => error(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("no instance {n:?}"),
        ),
        AdminError::Invalid(m) => error(StatusCode::BAD_REQUEST, "invalid_argument", &m),
        AdminError::Unavailable(m) => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &m),
    }
}

async fn handle(
    req: Request<Incoming>,
    token: Option<&Secret>,
    supervisor: &Supervisor,
) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_owned();
    if path == "/health" && req.method() == Method::GET {
        return reply(StatusCode::OK, &json!({"status": "ok"}));
    }
    if let Some(t) = token
        && !token_ok(req.headers(), t)
    {
        let mut r = error(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "send Authorization: Bearer <the connect admin token>",
        );
        r.headers_mut().insert(
            http::header::WWW_AUTHENTICATE,
            http::HeaderValue::from_static("Bearer"),
        );
        return r;
    }
    let Some(rest) = path.strip_prefix("/v1/connect/instances") else {
        return error(StatusCode::NOT_FOUND, "not_found", "no such route");
    };
    let method = req.method().clone();
    if rest.is_empty() || rest == "/" {
        return match method {
            Method::GET => reply(StatusCode::OK, &supervisor.list().await),
            _ => error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "GET only",
            ),
        };
    }
    let Some(id) = rest.strip_prefix('/') else {
        return error(StatusCode::NOT_FOUND, "not_found", "no such route");
    };
    let (name, verb) = match id.split_once(':') {
        Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
        None => (id.to_owned(), None),
    };
    let result = match (method, verb.as_deref()) {
        (Method::GET, None) => supervisor.status(&name).await,
        (Method::POST, Some("pause")) => supervisor.pause(&name).await,
        (Method::POST, Some("resume")) => supervisor.resume(&name).await,
        (Method::POST, Some("restart")) => supervisor.restart(&name).await,
        (Method::PATCH, None) => {
            let body = match Limited::new(req.into_body(), MAX_BODY).collect().await {
                Ok(b) => b.to_bytes(),
                Err(_) => {
                    return error(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "invalid_argument",
                        "the body is too large",
                    );
                }
            };
            let v: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        &format!("not JSON: {e}"),
                    );
                }
            };
            let Some(obj) = v.as_object() else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "send an object",
                );
            };
            if let Some(k) = obj.keys().find(|k| k.as_str() != "rate_limit") {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    &format!("{k} cannot be changed here; only rate_limit"),
                );
            }
            let rate = match obj.get("rate_limit") {
                None | Some(Value::Null) => None,
                Some(r) => match serde_json::from_value::<RateLimitBody>(r.clone()) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "invalid_argument",
                            &format!("rate_limit: {e}"),
                        );
                    }
                },
            };
            supervisor.set_rate(&name, rate).await
        }
        _ => {
            return error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "see the connect role's admin API",
            );
        }
    };
    match result {
        Ok(v) => reply(StatusCode::OK, &v),
        Err(e) => admin_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_exactly() {
        let t = Secret::new("s3cret-token");
        let mut h = http::HeaderMap::new();
        assert!(!token_ok(&h, &t));
        h.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer s3cret-token"),
        );
        assert!(token_ok(&h, &t));
        h.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer s3cret-tokeN"),
        );
        assert!(!token_ok(&h, &t));
    }
}
