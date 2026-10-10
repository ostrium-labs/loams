//! The console REST surface, served beside the app protos on one listener
//! (design §37 §12, AP0 Task 5).
//!
//! The contract is `api/console/openapi.json` and the data is
//! `loams-console-mock`'s seed; this module turns the same
//! [`routes`](loams_console_mock::routes) list that crate's `httpmock` server
//! used into real axum routes, so one process answers the app protos over
//! Connect, gRPC and gRPC-Web *and* the console's `/api/v1/*` REST.
//!
//! It keeps the mock's semantics rather than growing a real store: a `POST`
//! answers with a plausible created object and the next `GET` does not include
//! it. `httpmock` matched one concrete path plus optional query constraints and
//! took the first match; here the same answers are grouped by path and the
//! first constraint-satisfying one wins, which is what keeps
//! `GET /api/v1/audit?actor=…` and `?project=…` distinct.
//!
//! The two paths this serves that are not `/api/v1/*` are the engine's
//! `GET /health`, `GET /ready` and the seeded collection reads under
//! `/v1/namespaces/{ns}/collections`, so a console served from the same origin
//! has something to render.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use axum::routing::any;
use loams_console_mock::{Route, load_seed, routes};

/// The paths this module answers itself, which are therefore dropped from the
/// seed's route list.
///
/// `POST /api/v1/oauth/token` is the important one: the seed's example answer
/// is a fixed token that no RPC accepts, and the console only draws its token
/// card from it, which [`crate::oauth`] now answers for real.
const OWNED: [&str; 1] = ["/api/v1/oauth/token"];

/// One prepared answer.
#[derive(Debug)]
struct Prepared {
    method: Method,
    /// Query parameters that must match for this answer to be chosen.
    query: Vec<(String, String)>,
    status: StatusCode,
    /// A serialized JSON body, or `None` for an empty one.
    body: Option<String>,
    location: Option<String>,
}

impl Prepared {
    /// Whether this answer is the one the request asks for.
    fn matches(&self, method: &Method, query: &HashMap<String, String>) -> bool {
        self.method == *method
            && self
                .query
                .iter()
                .all(|(key, value)| query.get(key) == Some(value))
    }

    fn response(&self) -> Response {
        let mut response = Response::new(match &self.body {
            Some(body) => Body::from(body.clone()),
            None => Body::empty(),
        });
        *response.status_mut() = self.status;
        if let Some(location) = self
            .location
            .as_deref()
            .and_then(|value| HeaderValue::from_str(value).ok())
        {
            response.headers_mut().insert(header::LOCATION, location);
        }
        if self.body.is_some() {
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
        response
    }
}

/// The console answers, keyed by the path axum routes them on.
#[derive(Debug)]
pub(crate) struct Console {
    /// The path, then the answers for it in first-match-wins order. The key
    /// is the contract's template (`/v1/namespaces/{ns}/collections`) when the
    /// seed has no concrete id for it, which axum 0.8 reads as a capture.
    by_path: HashMap<String, Vec<Prepared>>,
}

impl Console {
    /// Loads the shared console seed and expands it against `now`.
    ///
    /// `signed_in` false answers `GET /api/v1/session` with 401, which is how
    /// the console's sign-in and setup screens are built.
    pub(crate) fn load(signed_in: bool) -> Result<Self> {
        let seed = load_seed(SystemTime::now()).context("loading the console seed")?;
        Self::from_routes(routes(&seed, signed_in)?)
    }

    /// Prepares an already-expanded route list.
    pub(crate) fn from_routes(prepared: Vec<Route>) -> Result<Self> {
        let mut by_path: HashMap<String, Vec<Prepared>> = HashMap::new();
        for route in prepared {
            if OWNED.contains(&route.path.as_str()) {
                continue;
            }
            let method = Method::from_bytes(route.method.as_bytes())
                .with_context(|| format!("bad method {}", route.method))?;
            let status = StatusCode::from_u16(route.status)
                .with_context(|| format!("bad status {} on {}", route.status, route.path))?;
            by_path.entry(route.path).or_default().push(Prepared {
                method,
                query: route.query,
                status,
                body: route.body.map(|body| body.to_string()),
                location: route.location,
            });
        }
        Ok(Self { by_path })
    }

    /// How many concrete paths are served.
    pub(crate) fn paths(&self) -> usize {
        self.by_path.len()
    }
}

/// The console router: one route per path, so Connect keeps the fallback and
/// the two surfaces never shadow each other.
pub(crate) fn router(console: Console) -> Router {
    let mut router = Router::new();
    for (path, candidates) in console.by_path {
        let template = path.clone();
        let answers = Arc::new(candidates);
        router = router.route(
            &path,
            any(move |request: Request| {
                let answers = Arc::clone(&answers);
                let template = template.clone();
                async move { answer(&answers, &template, request) }
            }),
        );
    }
    router
}

fn answer(answers: &[Prepared], template: &str, request: Request) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let query = uri
        .query()
        .map(|raw| {
            url::form_urlencoded::parse(raw.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<HashMap<String, String>>()
        })
        .unwrap_or_default();

    // An unconstrained answer is the fallback for its path, so a filtered
    // `?actor=` or `?project=` request is matched before it.
    match answers
        .iter()
        .find(|answer| answer.matches(&method, &query))
    {
        Some(answer) => answer.response(),
        // httpmock answered an unmatched request the same way: nothing matched,
        // so 404 whether the path or the method was wrong.
        None => {
            let mut response = Response::new(Body::from(
                serde_json::json!({ "error": "not_found", "template": template }).to_string(),
            ));
            *response.status_mut() = StatusCode::NOT_FOUND;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
    }
}
