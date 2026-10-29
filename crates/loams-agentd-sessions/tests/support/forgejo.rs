//! A fake Forgejo on loopback (SD1 Task 9, ruling T9-4; shared with Task
//! 11's review tests): it records every request and answers each with what
//! the test's `respond` says. No real service is ever called.

use std::sync::{Arc, Mutex};

/// One request the fake received.
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: String,
    pub authorization: String,
    pub body: serde_json::Value,
}

/// What the fake answers `(method, path)` with: a status and a JSON body.
pub type Respond = fn(&str, &str) -> (u16, serde_json::Value);

/// Start the fake; answers its base URL (`http://127.0.0.1:<port>`) and the
/// requests it records.
pub async fn fake_forgejo(respond: Respond) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    use axum::Json;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::IntoResponse;

    type Seen = Arc<Mutex<Vec<Recorded>>>;
    let seen: Seen = Arc::default();
    let app = axum::Router::new()
        .fallback(
            move |State(seen): State<Seen>,
                  method: Method,
                  uri: Uri,
                  headers: HeaderMap,
                  body: Bytes| async move {
                let authorization = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                seen.lock().unwrap().push(Recorded {
                    method: method.to_string(),
                    path: uri.path().to_owned(),
                    query: uri.query().unwrap_or_default().to_owned(),
                    authorization,
                    body: serde_json::from_slice(&body).unwrap_or_default(),
                });
                let (status, reply) = respond(method.as_str(), uri.path());
                let status =
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                // A JSON string is served as plain text (a CI log, FT0 Task 5).
                match reply {
                    serde_json::Value::String(text) => (status, text).into_response(),
                    reply => (status, Json(reply)).into_response(),
                }
            },
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, seen)
}
