//! The authentication layer: one axum middleware for every HTTP surface
//! (the main port's Connect, gRPC, gRPC-Web and REST, the Qdrant and
//! Elasticsearch gateways, and Flight SQL through tonic's axum routes).
//!
//! A refusal is written in the protocol the caller speaks: gRPC and gRPC-Web
//! get a trailers-only answer (`grpc-status` 16, 7 or 14), Connect gets its
//! JSON error with HTTP 401/403/503, anything else a JSON body with the same
//! status and `WWW-Authenticate: Bearer`. Every refusal carries the reason in
//! `loams-auth-reason`.
//!
//! The console ships with the deployment and is served by the engine itself
//! (`crate::console`), so its calls are same-origin. CORS is deny by
//! default: only the engine's own origin (the request's `Host`) and the
//! extra origins in `LOAMS_CORS_ORIGINS` (none by default) get it. The
//! preflight is answered here (it carries no credential), and every answer
//! to an allowed origin, refusals included, carries
//! `Access-Control-Allow-Origin`.
//!
//! Loams Live's app end users browse from the apps' own pages (LV1 plan
//! Task 18, design §45 §6.4): on a router that serves Live, a `LiveService`
//! call from an origin one of the active apps lists
//! ([`AuthLayerState::with_app_origins`]) gets CORS too, preflight included
//! (a preflight cannot name the app), and Live then holds the actual request
//! to the addressed app's own list. A request on an origin the engine itself
//! allows is marked [`EngineOrigin`]. `Access-Control-Allow-Credentials` is
//! never sent.
//!
//! Tailscale identity headers are stripped on every listener but the
//! `tailscale serve` target ([`AuthLayerState::tailscale_target`]). There
//! the identity is ambient (like a cookie), so CORS never allows any origin:
//! only `LOAMS_CORS_ORIGINS` (none by default) and the target's own host,
//! and a request from any other origin is refused outright.
//!
//! The connect socket (`--connect-socket`, CC1 ruling R1.1,
//! [`AuthLayerState::connect_socket`]) is a Unix socket the engine shares
//! with `loams-fabric connect` only. Whoever reaches it proves nothing by
//! reaching it, so it takes an API key and nothing else, whatever
//! `LOAMS_AUTH_MODE` says: Tailscale identity headers are stripped and never
//! read, sessions and JWTs are refused, it answers no CORS, and only
//! `/health` is public.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use base64::Engine as _;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};

use super::{
    Auth, AuthError, AuthErrorKind, Credential, SessionCredential, api_key, directory, reason,
    tailscale,
};

/// The response header naming why a request was refused.
pub const REASON_HEADER: &str = "loams-auth-reason";

/// Query parameters that would carry a credential (refused: URLs are logged).
const QUERY_CREDENTIALS: &[&str] = &["access_token", "token", "api_key", "apikey", "api-key"];

/// The main port's unauthenticated paths: [`super::PUBLIC_RPCS`] plus the
/// HTTP probes (`main_public_paths_are_the_public_rpcs_and_the_probes`).
pub const MAIN_PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/ready",
    "/grpc.health.v1.Health/Check",
    "/grpc.health.v1.Health/Watch",
    "/loams.instance.v1.InstanceService/GetInstance",
    "/loams.auth.v1.AuthService/ListProviders",
    "/loams.auth.v1.AuthService/CompleteSetup",
    "/loams.auth.v1.AuthService/CreateSession",
    "/loams.auth.v1.AuthService/DeleteSession",
    // Stripe's payment-state webhooks (D2207): Stripe has no Loams
    // credential; the route verifies `Stripe-Signature` itself.
    #[cfg(feature = "meter-export-stripe")]
    "/v1/meter/webhooks/stripe",
];

/// The connect socket's unauthenticated path: its health probe.
pub const CONNECT_SOCKET_PUBLIC_PATHS: &[&str] = &["/health"];

/// The path prefixes whose callers may be an app's own end users rather
/// than this engine's principals (LV1 plan Task 16, design §45 §5.3):
/// Loams Live's `LiveService`. A request on them that the layer cannot admit
/// only because it has no credential or carries a JWT from an issuer (or for
/// an audience) the engine does not take is let through without a
/// [`Principal`](super::Principal), marked [`LeftToApp`]; Live verifies it
/// against the addressed app's trusted issuers, or admits it anonymously
/// where the app allows that, and refuses it otherwise.
pub const APP_END_USER_PATHS: &[&str] = &["/loams.live.v1.LiveService/"];

/// The extension of a request the layer left to the addressed app's own
/// authentication ([`APP_END_USER_PATHS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeftToApp;

/// The extension of a request whose `Origin` the engine's own CORS allows
/// (`LOAMS_CORS_ORIGINS`, the console's) or is the engine's own (the
/// request's `Host`), as opposed to one only a Live app lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineOrigin;

/// The browser origins Loams Live's active apps list (LV1 plan Task 18):
/// whether one is among them.
#[derive(Clone)]
pub struct AppOriginSet(Arc<dyn Fn(&str) -> bool + Send + Sync>);

impl AppOriginSet {
    pub fn new(contains: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(contains))
    }

    fn contains(&self, origin: &str) -> bool {
        (self.0)(origin)
    }
}

impl std::fmt::Debug for AppOriginSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppOriginSet").finish_non_exhaustive()
    }
}

/// The one cookie-authenticated RPC that needs no CSRF token: it is a read,
/// and its answer is how the console learns the token.
const GET_SESSION: &str = "/loams.auth.v1.AuthService/GetSession";

const ALLOW_METHODS: &str = "GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS";
const ALLOW_HEADERS: &str = "authorization, content-type, accept, connect-protocol-version, \
     connect-timeout-ms, connect-content-encoding, connect-accept-encoding, grpc-timeout, \
     x-grpc-web, x-user-agent, loams-consistency-token, loams-hot, idempotency-key, \
     x-csrf-token, loam-session-id, loams-live-app, loams-request-id";
const EXPOSE_HEADERS: &str = "grpc-status, grpc-message, grpc-status-details-bin, \
     loams-consistency-token, loams-request-id, loams-auth-reason, www-authenticate, \
     retry-after";

/// The allowed browser origins beside the engine's own: deny by default.
#[derive(Clone, Debug, Default)]
pub struct Cors {
    origins: Arc<BTreeSet<String>>,
}

impl Cors {
    /// Exactly `origins` (lowercased, without a trailing `/`), and no other.
    pub fn new(origins: BTreeSet<String>) -> Self {
        Self {
            origins: Arc::new(
                origins
                    .into_iter()
                    .map(|o| o.trim_end_matches('/').to_ascii_lowercase())
                    .collect(),
            ),
        }
    }

    pub(crate) fn allows(&self, origin: &str) -> bool {
        self.origins
            .contains(&origin.trim_end_matches('/').to_ascii_lowercase())
    }
}

/// The middleware's state: the authenticators and the paths it lets through.
#[derive(Clone, Debug)]
pub struct AuthLayerState {
    auth: Arc<Auth>,
    public_paths: &'static [&'static str],
    /// The `tailscale serve` target: Tailscale identity headers count.
    trust_tailscale: bool,
    /// Path prefixes left to an app's own authentication when the engine
    /// cannot admit the caller ([`APP_END_USER_PATHS`]).
    app_end_user_paths: &'static [&'static str],
    /// The origins Live's apps list, for [`APP_END_USER_PATHS`] (Task 18).
    app_origins: Option<AppOriginSet>,
    /// The connect socket: an API key is the only credential, in every mode.
    api_key_only: bool,
}

impl AuthLayerState {
    pub fn new(auth: Arc<Auth>, public_paths: &'static [&'static str]) -> Self {
        Self {
            auth,
            public_paths,
            trust_tailscale: false,
            app_end_user_paths: &[],
            app_origins: None,
            api_key_only: false,
        }
    }

    /// The layer of the `LOAMS_TAILSCALE_LISTEN` listener: the only one on
    /// which `Tailscale-User-Login` names the caller.
    pub fn tailscale_target(auth: Arc<Auth>, public_paths: &'static [&'static str]) -> Self {
        Self {
            auth,
            public_paths,
            trust_tailscale: true,
            app_end_user_paths: &[],
            app_origins: None,
            api_key_only: false,
        }
    }

    /// The layer of the connect socket (`--connect-socket`): an API key and
    /// nothing else, also when authentication is off; Tailscale headers are
    /// stripped unread; `/health` is the one public path; no CORS.
    pub fn connect_socket(auth: Arc<Auth>) -> Self {
        Self {
            auth,
            public_paths: CONNECT_SOCKET_PUBLIC_PATHS,
            trust_tailscale: false,
            app_end_user_paths: &[],
            app_origins: None,
            api_key_only: true,
        }
    }

    /// This layer, leaving [`APP_END_USER_PATHS`] to the addressed Live
    /// app when it cannot admit a caller (LV1 plan Task 16): for a router
    /// that serves Loams Live, never for one that answers
    /// `feature_not_in_variant` for it.
    pub fn leaving_to_apps(mut self) -> Self {
        self.app_end_user_paths = APP_END_USER_PATHS;
        self
    }

    /// This layer, answering CORS on [`APP_END_USER_PATHS`] for the origins
    /// Live's active apps list too (LV1 plan Task 18). Not on the
    /// `tailscale serve` target, whose identity is ambient: there only the
    /// engine's own origins are allowed.
    pub fn with_app_origins(mut self, origins: AppOriginSet) -> Self {
        if !self.trust_tailscale {
            self.app_origins = Some(origins);
        }
        self
    }

    /// Whether an app lists `origin` for a request on `path`.
    fn app_allows(&self, path: &str, origin: &str) -> bool {
        self.app_origins.as_ref().is_some_and(|set| {
            self.app_end_user_paths.iter().any(|p| path.starts_with(p)) && set.contains(origin)
        })
    }

    /// `router` behind the layer (every route and the fallback).
    pub fn wrap(&self, router: Router) -> Router {
        router.layer(middleware::from_fn_with_state(self.clone(), guard))
    }

    /// tonic `routes` behind the layer.
    pub fn wrap_grpc(&self, routes: tonic::service::Routes) -> tonic::service::Routes {
        self.wrap(routes.into_axum_router()).into()
    }
}

async fn guard(State(state): State<AuthLayerState>, mut request: Request, next: Next) -> Response {
    // Read (on the tailscale serve target only), then strip everywhere: no
    // handler or proxy behind this layer ever sees them.
    let tailnet_user = state
        .trust_tailscale
        .then(|| tailscale::identity(request.headers()))
        .flatten();
    tailscale::strip(request.headers_mut());
    if state.api_key_only {
        return api_key_only(&state, request, next).await;
    }
    let cors = match state.trust_tailscale {
        true => state.auth.tailscale_cors(),
        false => state.auth.cors(),
    };
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // The engine's own origin (a page it serves itself, or the desktop
    // loading the console from it) is the engine's as much as a configured
    // console origin is: same-origin, so a browser applies no CORS to it
    // anyway. Marking it lets Live admit a platform principal from it
    // (0.1.0 release e2e, B11) without widening CORS for any other origin.
    let engine_origin = origin
        .as_deref()
        .filter(|o| cors.allows(o) || same_host(o, request.headers()));
    if engine_origin.is_some() {
        request.extensions_mut().insert(EngineOrigin);
    }
    let allowed_origin = engine_origin
        .or_else(|| {
            origin
                .as_deref()
                .filter(|o| state.app_allows(request.uri().path(), o))
        })
        .map(str::to_owned);
    // On the serve target, a foreign page must not ride the ambient tailnet
    // identity, not even with a request that skips the preflight.
    if state.trust_tailscale
        && let Some(o) = origin.as_deref()
        && allowed_origin.is_none()
        && !same_host(o, request.headers())
    {
        return refuse(
            request.headers(),
            &AuthError::forbidden(
                reason::CORS_ORIGIN_NOT_ALLOWED,
                "this origin is not in LOAMS_CORS_ORIGINS",
            ),
        );
    }
    let with_cors = |mut response: Response| {
        if let Some(origin) = &allowed_origin {
            add_cors(response.headers_mut(), origin);
        }
        response
    };
    if request.method() == Method::OPTIONS
        && origin.is_some()
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        return match &allowed_origin {
            Some(origin) => preflight(origin, request.headers()),
            None => refuse(
                request.headers(),
                &AuthError::forbidden(
                    reason::CORS_ORIGIN_NOT_ALLOWED,
                    "this origin is not in LOAMS_CORS_ORIGINS",
                ),
            ),
        };
    }
    if !state.auth.enforced() {
        // Loopback only: everyone here is the developer, unless the call
        // carries a console session that is still good, which names its
        // person (so the console's sign-in works in `loams dev` too).
        let principal = match request_credential(&request) {
            Ok(credential @ Credential::Session(_)) => {
                state.auth.authenticate(&credential).await.ok()
            }
            _ => None,
        };
        request
            .extensions_mut()
            .insert(principal.unwrap_or_else(Auth::dev_loopback));
        return with_cors(next.run(request).await);
    }
    if credential_in_query(request.uri().query()) {
        let err = AuthError::unauthenticated(
            reason::CREDENTIAL_IN_QUERY_STRING,
            "credentials in the URL are refused; send Authorization: Bearer",
        );
        return with_cors(refuse(request.headers(), &err));
    }
    if state.public_paths.contains(&request.uri().path()) {
        return with_cors(next.run(request).await);
    }
    let credential = request_credential(&request);
    let jwt = matches!(credential, Ok(Credential::Jwt(_)));
    let result = match (credential, &tailnet_user, state.auth.tailscale()) {
        // An explicit credential wins; otherwise the tailnet identity.
        (Err(err), Some((login, name)), Some(ts)) if err.reason == reason::TOKEN_MISSING => {
            ts.admit(login, name.as_deref())
        }
        (Ok(credential), _, _) => state.auth.authenticate(&credential).await,
        (Err(err), _, _) => Err(err),
    };
    match result {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            with_cors(next.run(request).await)
        }
        Err(err) if state.left_to_app(request.uri().path(), &err, jwt) => {
            request.extensions_mut().insert(LeftToApp);
            with_cors(next.run(request).await)
        }
        Err(err) => {
            tracing::debug!(
                reason = err.reason,
                path = request.uri().path(),
                "request refused"
            );
            with_cors(refuse(request.headers(), &err))
        }
    }
}

/// The connect socket's guard: an API key, or the refusal. Runs after the
/// Tailscale headers are stripped, never reads them, and adds no CORS.
async fn api_key_only(state: &AuthLayerState, mut request: Request, next: Next) -> Response {
    if state.public_paths.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    if credential_in_query(request.uri().query()) {
        let err = AuthError::unauthenticated(
            reason::CREDENTIAL_IN_QUERY_STRING,
            "credentials in the URL are refused; send Authorization: Bearer",
        );
        return refuse(request.headers(), &err);
    }
    let result = match request_credential(&request) {
        Ok(credential @ Credential::ApiKey(_)) => state.auth.authenticate(&credential).await,
        Ok(_) => Err(AuthError::unauthenticated(
            reason::CREDENTIAL_NOT_ACCEPTED,
            "the connect socket takes an API key only",
        )),
        Err(err) => Err(err),
    };
    match result {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(err) => {
            tracing::debug!(
                reason = err.reason,
                path = request.uri().path(),
                "connect socket request refused"
            );
            refuse(request.headers(), &err)
        }
    }
}

impl AuthLayerState {
    /// Whether a request on `path` refused with `err` goes to the app
    /// instead ([`APP_END_USER_PATHS`]): no credential, or a JWT (`jwt`)
    /// the engine does not take at all or from that issuer or for that
    /// audience. An expired or forged token of the engine's own issuer, and
    /// any API key or session the engine refuses, stay refused here.
    fn left_to_app(&self, path: &str, err: &AuthError, jwt: bool) -> bool {
        self.app_end_user_paths.iter().any(|p| path.starts_with(p))
            && match err.reason {
                reason::TOKEN_MISSING => !jwt,
                reason::TOKEN_WRONG_ISSUER
                | reason::TOKEN_WRONG_AUDIENCE
                | reason::CREDENTIAL_NOT_ACCEPTED => jwt,
                _ => false,
            }
    }
}

/// Whether `origin` is the request's own host (a page the engine serves).
fn same_host(origin: &str, headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin_host = origin
        .split_once("://")
        .map(|(_, rest)| rest.trim_end_matches('/'));
    matches!((host, origin_host), (Some(h), Some(o)) if h.eq_ignore_ascii_case(o))
}

fn credential_in_query(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        q.split('&').any(|pair| {
            let name = pair.split('=').next().unwrap_or_default();
            QUERY_CREDENTIALS
                .iter()
                .any(|c| name.eq_ignore_ascii_case(c))
        })
    })
}

/// The request's credential: [`credential`]'s headers, then the console's
/// session cookie. A cookie-authenticated call that is not a `GET` (nor
/// `GetSession`) must carry the session's `X-CSRF-Token`.
pub(crate) fn request_credential(request: &Request) -> Result<Credential, AuthError> {
    match credential(request.headers()) {
        Err(err) if err.reason == reason::TOKEN_MISSING => {
            let Some(token) = directory::session_cookie(request.headers()) else {
                return Err(err);
            };
            let csrf = (request.method() != Method::GET && request.uri().path() != GET_SESSION)
                .then(|| {
                    request
                        .headers()
                        .get(directory::CSRF_HEADER)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                });
            Ok(Credential::Session(SessionCredential { token, csrf }))
        }
        other => other,
    }
}

/// A bearer, Basic password, `ApiKey` or `api-key` value as a credential.
fn classify(value: &str) -> Result<Credential, AuthError> {
    let value = value.trim();
    if directory::looks_like_session(value) {
        Ok(Credential::Session(SessionCredential {
            token: value.to_owned(),
            csrf: None,
        }))
    } else if value.starts_with(directory::SESSION_PREFIX) {
        Err(AuthError::unauthenticated(
            reason::TOKEN_MALFORMED,
            "a session token is ls_<43 characters>",
        ))
    } else if api_key::looks_like_key(value) {
        Ok(Credential::ApiKey(value.to_owned()))
    } else if value.starts_with(api_key::PREFIX) {
        Err(AuthError::unauthenticated(
            reason::TOKEN_MALFORMED,
            "an API key is lk_<26-char id>_<32 characters>",
        ))
    } else if value.split('.').count() == 3 {
        Ok(Credential::Jwt(value.to_owned()))
    } else {
        Err(AuthError::unauthenticated(
            reason::TOKEN_MALFORMED,
            "the credential is neither an API key nor a JWT",
        ))
    }
}

fn malformed(message: &str) -> AuthError {
    AuthError::unauthenticated(reason::TOKEN_MALFORMED, message)
}

/// The request's credential: `Authorization` (Bearer, Basic with the key or
/// token as the password, Elasticsearch's `ApiKey`) or Qdrant's `api-key`.
pub(crate) fn credential(headers: &HeaderMap) -> Result<Credential, AuthError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    if let Some(value) = values.next() {
        if values.next().is_some() {
            return Err(malformed("more than one Authorization header"));
        }
        let value = value
            .to_str()
            .map_err(|_| malformed("the Authorization header is not text"))?;
        let (scheme, rest) = value.trim().split_once(' ').unwrap_or((value, ""));
        let rest = rest.trim();
        return match scheme.to_ascii_lowercase().as_str() {
            "bearer" => classify(rest),
            "basic" => {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(rest)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .ok_or_else(|| malformed("the Basic credential is not base64 text"))?;
                let (_, password) = decoded
                    .split_once(':')
                    .ok_or_else(|| malformed("the Basic credential has no password"))?;
                classify(password)
            }
            "apikey" => {
                if api_key::looks_like_key(rest) {
                    return classify(rest);
                }
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(rest)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .ok_or_else(|| malformed("the ApiKey credential is not an API key"))?;
                classify(decoded.split_once(':').map_or(decoded.as_str(), |(_, k)| k))
            }
            _ => Err(malformed(
                "unsupported Authorization scheme; use Bearer, Basic or ApiKey",
            )),
        };
    }
    if let Some(value) = headers.get("api-key") {
        let value = value
            .to_str()
            .map_err(|_| malformed("the api-key header is not text"))?;
        return classify(value);
    }
    // Cloudflare Access in front of the engine sends its JWT here.
    if let Some(value) = headers.get("cf-access-jwt-assertion") {
        let value = value
            .to_str()
            .map_err(|_| malformed("the Cf-Access-Jwt-Assertion header is not text"))?;
        return match classify(value)? {
            jwt @ Credential::Jwt(_) => Ok(jwt),
            Credential::ApiKey(_) | Credential::Session(_) => {
                Err(malformed("Cf-Access-Jwt-Assertion is not a JWT"))
            }
        };
    }
    Err(AuthError::unauthenticated(
        reason::TOKEN_MISSING,
        "this engine needs a credential: Authorization: Bearer <API key or OIDC token>",
    ))
}

fn add_cors(headers: &mut HeaderMap, origin: &str) {
    if let Ok(origin) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(EXPOSE_HEADERS),
    );
}

fn preflight(origin: &str, request: &HeaderMap) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    let headers = response.headers_mut();
    add_cors(headers, origin);
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(ALLOW_METHODS),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(ALLOW_HEADERS),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    // Chrome's Private Network Access: the console is public, the engine on
    // a tailnet address.
    if request
        .get("access-control-request-private-network")
        .is_some_and(|v| v == "true")
    {
        headers.insert(
            HeaderName::from_static("access-control-allow-private-network"),
            HeaderValue::from_static("true"),
        );
    }
    response
}

/// The refusal in the caller's protocol.
pub(crate) fn refuse(request: &HeaderMap, err: &AuthError) -> Response {
    let content_type = request
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut response = if content_type.starts_with("application/grpc") {
        grpc_refusal(&content_type, err)
    } else if request.contains_key("connect-protocol-version")
        || content_type.starts_with("application/connect+")
    {
        let code = match err.kind {
            AuthErrorKind::Unauthenticated => "unauthenticated",
            AuthErrorKind::Forbidden => "permission_denied",
            AuthErrorKind::Unavailable => "unavailable",
        };
        // The reason also rides as the `ErrorInfo` every RPC error carries
        // (design §44 §7.4), so an SDK branches on it as on any other.
        let info = loams_proto::loams::errors::v1::ErrorInfo {
            reason: err.reason.to_owned(),
            ..Default::default()
        };
        let value = base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(buffa::Message::encode_to_vec(&info));
        json(
            err.status(),
            &serde_json::json!({
                "code": code,
                "message": format!("{}: {}", err.reason, err.message),
                "details": [{"type": "loams.errors.v1.ErrorInfo", "value": value}],
            }),
        )
    } else {
        json(
            err.status(),
            &serde_json::json!({
                "error": match err.kind {
                    AuthErrorKind::Unauthenticated => "unauthenticated",
                    AuthErrorKind::Forbidden => "permission_denied",
                    AuthErrorKind::Unavailable => "unavailable",
                },
                "reason": err.reason,
                "message": err.message,
            }),
        )
    };
    let headers = response.headers_mut();
    headers.insert(REASON_HEADER, HeaderValue::from_static(err.reason));
    match err.kind {
        AuthErrorKind::Unauthenticated => {
            let challenge = if err.reason == reason::TOKEN_MISSING {
                "Bearer realm=\"loams\"".to_owned()
            } else {
                format!(
                    "Bearer realm=\"loams\", error=\"invalid_token\", error_description=\"{}\"",
                    err.reason
                )
            };
            if let Ok(value) = HeaderValue::from_str(&challenge) {
                headers.insert(header::WWW_AUTHENTICATE, value);
            }
        }
        AuthErrorKind::Unavailable => {
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        AuthErrorKind::Forbidden => {}
    }
    response
}

fn json(status: StatusCode, body: &serde_json::Value) -> Response {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn grpc_refusal(content_type: &str, err: &AuthError) -> Response {
    let status = match err.kind {
        AuthErrorKind::Unauthenticated => "16",
        AuthErrorKind::Forbidden => "7",
        AuthErrorKind::Unavailable => "14",
    };
    let message = format!("{}: {}", err.reason, err.message);
    let mut response = Response::new(Body::empty());
    let headers = response.headers_mut();
    let content_type = if content_type.starts_with("application/grpc-web") {
        content_type
    } else {
        "application/grpc"
    };
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert("grpc-status", HeaderValue::from_static(status));
    if let Ok(value) = HeaderValue::from_str(&percent_encode(&message)) {
        headers.insert("grpc-message", value);
    }
    response
}

/// gRPC's `grpc-message` encoding: printable ASCII except `%` as is.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if (0x20..=0x7e).contains(&byte) && byte != b'%' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(*k, HeaderValue::from_str(v).unwrap());
        }
        map
    }

    const KEY: &str = "lk_01J9Z8Y7X6W5V4T3S2R1Q0P9N8_abcdefghijklmnopqrstuvwxyzABCDEF";

    #[test]
    fn credentials_are_found_in_every_carrier() {
        assert!(api_key::looks_like_key(KEY));
        let bearer = format!("Bearer {KEY}");
        assert_eq!(
            credential(&headers(&[("authorization", &bearer)])).unwrap(),
            Credential::ApiKey(KEY.to_owned())
        );
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("loams:{KEY}"))
        );
        assert_eq!(
            credential(&headers(&[("authorization", &basic)])).unwrap(),
            Credential::ApiKey(KEY.to_owned())
        );
        let es = format!(
            "ApiKey {}",
            base64::engine::general_purpose::STANDARD.encode(format!("id:{KEY}"))
        );
        assert_eq!(
            credential(&headers(&[("authorization", &es)])).unwrap(),
            Credential::ApiKey(KEY.to_owned())
        );
        assert_eq!(
            credential(&headers(&[("api-key", KEY)])).unwrap(),
            Credential::ApiKey(KEY.to_owned())
        );
        assert_eq!(
            credential(&headers(&[("authorization", "Bearer a.b.c")])).unwrap(),
            Credential::Jwt("a.b.c".to_owned())
        );
        assert_eq!(
            credential(&headers(&[("cf-access-jwt-assertion", "a.b.c")])).unwrap(),
            Credential::Jwt("a.b.c".to_owned())
        );
    }

    #[test]
    fn bad_credentials_get_clear_reasons() {
        let reason_of = |h: HeaderMap| credential(&h).unwrap_err().reason;
        assert_eq!(reason_of(headers(&[])), reason::TOKEN_MISSING);
        assert_eq!(
            reason_of(headers(&[("authorization", "Bearer nope")])),
            reason::TOKEN_MALFORMED
        );
        assert_eq!(
            reason_of(headers(&[("authorization", "Digest x")])),
            reason::TOKEN_MALFORMED
        );
        assert_eq!(
            reason_of(headers(&[
                ("authorization", "Bearer a.b.c"),
                ("authorization", "Bearer d.e.f")
            ])),
            reason::TOKEN_MALFORMED
        );
    }

    #[test]
    fn only_live_service_calls_without_an_engine_credential_go_to_the_app() {
        // LV1 plan Task 16: no credential, or a JWT the engine does not
        // take from that issuer or for that audience, on LiveService only.
        let state = AuthLayerState::new(Auth::off(), &[]).leaving_to_apps();
        let query = "/loams.live.v1.LiveService/Query";
        let err = |reason| AuthError::unauthenticated(reason, "x");
        assert!(state.left_to_app(query, &err(reason::TOKEN_MISSING), false));
        for why in [
            reason::TOKEN_WRONG_ISSUER,
            reason::TOKEN_WRONG_AUDIENCE,
            reason::CREDENTIAL_NOT_ACCEPTED,
        ] {
            assert!(state.left_to_app(query, &err(why), true), "{why}");
            assert!(
                !state.left_to_app(query, &err(why), false),
                "{why}: not a JWT"
            );
        }
        for why in [
            reason::TOKEN_EXPIRED,
            reason::TOKEN_BAD_SIGNATURE,
            reason::TOKEN_MALFORMED,
            reason::API_KEY_INVALID,
            reason::PRINCIPAL_NOT_ALLOWED,
        ] {
            assert!(!state.left_to_app(query, &err(why), true), "{why}");
        }
        for path in [
            "/loams.live.v1.LiveAdminService/Deploy",
            "/loams.collections.v1.CollectionService/Get",
        ] {
            assert!(!state.left_to_app(path, &err(reason::TOKEN_MISSING), false));
            assert!(!state.left_to_app(path, &err(reason::TOKEN_WRONG_ISSUER), true));
        }
        // A router that does not serve Live leaves nothing to it.
        let plain = AuthLayerState::new(Auth::off(), &[]);
        assert!(!plain.left_to_app(query, &err(reason::TOKEN_MISSING), false));
    }

    #[test]
    fn main_public_paths_are_the_public_rpcs_and_the_probes() {
        let mut expected: Vec<&str> = super::super::PUBLIC_RPCS.to_vec();
        expected.extend(["/health", "/ready"]);
        #[cfg(feature = "meter-export-stripe")]
        expected.push("/v1/meter/webhooks/stripe");
        expected.sort_unstable();
        let mut actual = MAIN_PUBLIC_PATHS.to_vec();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_session_cookie_needs_csrf_except_on_get_session() {
        let token = directory::new_session_token();
        let request = |method: Method, path: &str| {
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, format!("loams_session={token}"))
                .body(Body::empty())
                .unwrap()
        };
        let csrf_of = |r: Request| match request_credential(&r).unwrap() {
            Credential::Session(s) => s.csrf,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            csrf_of(request(Method::POST, "/loams.admin.v1.OrgService/GetOrg")),
            Some(String::new())
        );
        assert_eq!(csrf_of(request(Method::POST, GET_SESSION)), None);
        assert_eq!(
            csrf_of(request(Method::GET, "/loams.admin.v1.OrgService/GetOrg")),
            None
        );
        // A bearer session needs none, and wins over the cookie.
        let bearer = format!("Bearer {}", directory::new_session_token());
        assert!(matches!(
            credential(&headers(&[("authorization", &bearer)])).unwrap(),
            Credential::Session(SessionCredential { csrf: None, .. })
        ));
        assert_eq!(
            credential(&headers(&[("authorization", "Bearer ls_short")]))
                .unwrap_err()
                .reason,
            reason::TOKEN_MALFORMED
        );
    }

    #[test]
    fn query_string_credentials_are_detected() {
        assert!(credential_in_query(Some("access_token=x")));
        assert!(credential_in_query(Some("a=1&Token=x")));
        assert!(!credential_in_query(Some("tokens=1&limit=2")));
        assert!(!credential_in_query(None));
    }

    #[test]
    fn grpc_refusal_is_trailers_only() {
        let err = AuthError::unauthenticated(reason::TOKEN_MISSING, "100% missing");
        let response = refuse(
            &headers(&[("content-type", "application/grpc+proto")]),
            &err,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["grpc-status"], "16");
        assert_eq!(
            response.headers()["grpc-message"],
            "token_missing: 100%25 missing"
        );
        assert_eq!(response.headers()[REASON_HEADER], "token_missing");
    }

    #[test]
    fn connect_and_rest_refusals_use_http_status() {
        let err = AuthError::forbidden(reason::PRINCIPAL_NOT_ALLOWED, "no");
        let response = refuse(&headers(&[("connect-protocol-version", "1")]), &err);
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let err = AuthError::unauthenticated(reason::TOKEN_EXPIRED, "old");
        let response = refuse(&headers(&[]), &err);
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response.headers()[header::WWW_AUTHENTICATE]
                .to_str()
                .unwrap()
                .contains("token_expired")
        );
    }
}
