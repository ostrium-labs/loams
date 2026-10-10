//! The mock's OAuth surface: pairing, the token endpoint, a fake Authentik
//! and the test controls the phone flows need (design §37 §7.2, AP0).
//!
//! The console seed answers `/api/v1/oauth/token` and `/.well-known/*` with
//! canned bodies, which is enough to draw the console's consent screens but not
//! to pair a phone: a token that was never issued cannot be presented to an
//! RPC, and a code that was never recorded cannot be redeemed. This module
//! makes both real against the mock's own state, so the desktop, the phone
//! apps and the SDKs can run a full sign-in against one address.
//!
//! What is real: pairing codes with the 5-minute TTL and single use, the user
//! code and its failure limit, all three grants (pairing, RFC 8693 exchange,
//! refresh), the device each issuance registers, PKCE S256 verification in the
//! fake Authentik, and the issued tokens, which are the same
//! `mock-access-<principal>` tokens [`crate::auth`] accepts.
//!
//! What is not: DPoP is recorded, not enforced; there is no revocation
//! endpoint; and the JWTs are opaque to the mock's own RPC layer rather than
//! signed, so only this process can read them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use connectrpc::RequestContext;
use rand::Rng;
use serde_json::{Value, json};

use crate::proto::loams::instance::v1::PrincipalKind;
use crate::seed::ts;
use crate::store::{PAIRING, Pairing, Store};

/// The grant a phone redeems a scanned QR code with (§37 §7.2.2).
pub(crate) const GRANT_PAIRING: &str = "urn:loams:params:oauth:grant-type:pairing";
/// RFC 8693 token exchange, for a phone that signed in through the IdP first.
pub(crate) const GRANT_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// The fake Authentik lives under this prefix so browser sign-in can run end to
/// end against the mock: authorize (redirects straight back with a code),
/// token (checks PKCE S256) and discovery.
pub(crate) const AUTHENTIK: &str = "/mock/authentik/application/o/loams/";

/// How many wrong user codes burn a pairing (§37 §7.2.1).
const USER_CODE_FAILURES: u32 = 5;

/// The shared state of this module: the mock's store.
#[derive(Clone)]
pub(crate) struct Oauth {
    pub(crate) store: Arc<Store>,
    /// What the phone is told to reach the mock as, when the loopback address
    /// is not what a device can use (`--public-url`, for example
    /// `http://10.0.2.2:8084` for the Android emulator).
    pub(crate) public_url: Option<String>,
}

/// One push a gateway would have received, for `GET /mock/push-log`.
#[derive(Debug, Clone)]
pub(crate) struct PushRecord {
    pub(crate) seq: u64,
    pub(crate) target_id: String,
    pub(crate) device_id: String,
    pub(crate) provider: String,
    pub(crate) token: String,
    pub(crate) app_id: String,
    /// The device sent an HPKE public key, so the payload would be sealed
    /// rather than plaintext (§37 §7.4).
    pub(crate) sealed: bool,
}

/// Starts a pairing for `user_id` and returns it with its user code.
///
/// Both the `CreatePairing` RPC and `GET /mock/pairing` call this, so a code
/// from either is redeemable at the token endpoint.
pub(crate) fn new_pairing(
    store: &Arc<Store>,
    user_id: &str,
    issuer: &str,
) -> (Pairing, String, String) {
    let mut rng = rand::rng();
    // 128 random bits in Crockford base32, as the QR payload's `code`.
    let code = ulid::Ulid::from(rng.random::<u128>()).to_string();
    let user_code = format!("{:08}", rng.random_range(0..100_000_000u32));
    let pairing = Pairing {
        user_id: user_id.to_owned(),
        expires_at: SystemTime::now() + PAIRING,
        used: false,
        failures: 0,
    };
    let mut state = store.lock();
    state.pairings.insert(code.clone(), pairing.clone());
    state.user_codes.insert(user_code.clone(), code.clone());
    tracing::info!(issuer, user_id, "pairing started");
    (pairing, code, user_code)
}

/// The origin a device should be handed to follow: `--public-url` when given
/// (the emulator reaches the host as `10.0.2.2`), otherwise the authority the
/// client dialled, from its `Host` header.
pub(crate) fn public_base(store: &Store, ctx: &RequestContext) -> Option<String> {
    if let Some(base) = store.base_url() {
        return Some(base);
    }
    ctx.header(http::header::HOST)
        .and_then(|host| host.to_str().ok())
        .map(|authority| format!("http://{authority}"))
}

/// The issuer a device should be told to use.
pub(crate) fn issuer_for(public_url: &Option<String>, fallback: &str) -> String {
    public_url.clone().unwrap_or_else(|| fallback.to_owned())
}

/// Builds the routes. `console_token` is false when the caller wants the
/// console seed's canned token answer instead of this module's.
pub(crate) fn router(oauth: Oauth) -> Router {
    let mut router = Router::new();
    router = router
        .route("/api/v1/oauth/token", post(token).with_state(oauth.clone()))
        .route(
            &format!("{AUTHENTIK}.well-known/openid-configuration"),
            get(authentik_discovery).with_state(oauth.clone()),
        )
        .route(
            &format!("{AUTHENTIK}authorize/"),
            get(authentik_authorize).with_state(oauth.clone()),
        )
        .route(
            &format!("{AUTHENTIK}token/"),
            post(authentik_token).with_state(oauth.clone()),
        )
        .route(&format!("{AUTHENTIK}jwks/"), get(authentik_jwks))
        .route("/healthz", get(|| async { "ok" }))
        // Test controls. Not part of any Loams API.
        .route("/mock/pairing", get(mock_pairing).with_state(oauth.clone()))
        .route(
            "/mock/approvals",
            post(mock_approval).with_state(oauth.clone()),
        )
        .route(
            "/mock/drop-streams",
            post(mock_drop_streams).with_state(oauth.clone()),
        )
        .route("/mock/tick", post(mock_tick).with_state(oauth.clone()))
        .route("/mock/push-log", get(mock_push_log).with_state(oauth));
    router
}

/// `POST /api/v1/oauth/token`, the gateway's token endpoint.
async fn token(State(oauth): State<Oauth>, Form(form): Form<HashMap<String, String>>) -> Response {
    let grant = form
        .get("grant_type")
        .map(String::as_str)
        .unwrap_or_default();
    let (status, body) = match grant {
        GRANT_PAIRING => redeem_pairing(&oauth, &form),
        GRANT_EXCHANGE => exchange(&oauth, &form),
        "refresh_token" => refresh(&oauth, &form),
        other => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "",
            &format!("grant_type {other}"),
        ),
    };
    json_response(status, body)
}

/// An OAuth error body, with the status it is returned with.
fn oauth_error(
    status: StatusCode,
    code: &str,
    reason: &str,
    description: &str,
) -> (StatusCode, Value) {
    (
        status,
        json!({
            "error": code,
            "error_description": description,
            "loams_reason": reason,
        }),
    )
}

fn redeem_pairing(oauth: &Oauth, form: &HashMap<String, String>) -> (StatusCode, Value) {
    let code = form.get("code").cloned();
    let user_code = form.get("user_code").cloned();
    if code.is_some() == user_code.is_some() {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "",
            "send exactly one of code or user_code",
        );
    }
    let now = SystemTime::now();
    let mut state = oauth.store.lock();
    let code = match &user_code {
        Some(typed) => match state.user_codes.get(typed) {
            Some(code) => code.clone(),
            None => {
                // Count the failure against every live pairing (a
                // simplification of AP0's per-instance limit) and burn one
                // after USER_CODE_FAILURES.
                for pairing in state.pairings.values_mut().filter(|p| !p.used) {
                    pairing.failures += 1;
                    if pairing.failures >= USER_CODE_FAILURES {
                        pairing.used = true;
                    }
                }
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "pairing_expired",
                    "unknown or expired user code",
                );
            }
        },
        None => code.expect("one of the two was set"),
    };
    match state.pairings.get_mut(&code) {
        None => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "pairing_expired",
            "unknown or expired pairing code",
        ),
        Some(pairing) if pairing.used => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "pairing_used",
            "this pairing code was already used",
        ),
        Some(pairing) if now > pairing.expires_at => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "pairing_expired",
            "this pairing code expired",
        ),
        Some(pairing) => {
            pairing.used = true;
            let user_id = pairing.user_id.clone();
            drop(state);
            issue(oauth, &user_id, form)
        }
    }
}

fn exchange(oauth: &Oauth, form: &HashMap<String, String>) -> (StatusCode, Value) {
    // The subject token is what the fake Authentik's token endpoint returned.
    if !form
        .get("subject_token")
        .is_some_and(|token| token.starts_with("mock-authentik-"))
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "",
            "subject_token is not a token from the mock Authentik",
        );
    }
    // The fake Authentik signs in whoever the IdP would have: the first seeded
    // person, which is Dana.
    let user_id = oauth
        .store
        .seed
        .principals
        .iter()
        .find(|p| p.kind.as_known() == Some(PrincipalKind::PRINCIPAL_KIND_USER))
        .map(|p| p.id.clone())
        .unwrap_or_else(|| "usr_omar".to_owned());
    issue(oauth, &user_id, form)
}

/// Registers the device the form describes and returns its tokens.
fn issue(oauth: &Oauth, user_id: &str, form: &HashMap<String, String>) -> (StatusCode, Value) {
    // DPoP is recorded but not enforced: the mock's RPC layer authenticates
    // the bearer token alone.
    let dpop = form.contains_key("dpop");
    let mut rng = rand::rng();
    let id = format!("dev_{}", ulid::Ulid::from(rng.random::<u128>()));
    let platform = if form.get("platform").is_some_and(|p| p == "ios") {
        crate::proto::loams::devices::v1::Platform::PLATFORM_IOS
    } else {
        crate::proto::loams::devices::v1::Platform::PLATFORM_ANDROID
    };
    let now = SystemTime::now();
    let device = crate::proto::loams::devices::v1::Device {
        id: id.clone(),
        name: form.get("device_name").cloned().unwrap_or_default(),
        platform: platform.into(),
        model: form.get("model").cloned().unwrap_or_default(),
        app_version: form.get("app_version").cloned().unwrap_or_default(),
        created_at: ts(now),
        last_seen_at: ts(now),
        decision_key_thumbprint: form
            .get("decision_jwk")
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|jwk| jwk.get("x").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default(),
        ..Default::default()
    };
    let refresh = format!("mock-refresh-{}", ulid::Ulid::from(rng.random::<u128>()));
    let access = access_token(user_id, &mut rng);
    {
        let mut state = oauth.store.lock();
        state
            .devices
            .insert(id.clone(), (user_id.to_owned(), device));
        state.refresh_tokens.insert(refresh.clone(), id.clone());
        // The token resolves to this device, so `WhoAmI` names it.
        state.issued.insert(access.clone(), id.clone());
    }
    tracing::info!(device = %id, user_id, dpop, "device paired");
    (
        StatusCode::OK,
        json!({
            "access_token": access,
            "token_type": "DPoP",
            "expires_in": 3600,
            "refresh_token": refresh,
            "device_id": id,
        }),
    )
}

/// An access token for `user_id` that is unique per call.
///
/// The token has to be unique per *device*, not just per principal: `Store::issued`
/// maps a token to the device it was issued for, so one token string per principal
/// means a second paired device silently takes over the first device's identity.
fn access_token(user_id: &str, rng: &mut impl Rng) -> String {
    format!(
        "mock-access-{user_id}-{}",
        ulid::Ulid::from(rng.random::<u128>())
    )
}

fn refresh(oauth: &Oauth, form: &HashMap<String, String>) -> (StatusCode, Value) {
    let old = form.get("refresh_token").cloned().unwrap_or_default();
    let mut rng = rand::rng();
    let mut state = oauth.store.lock();
    let Some(id) = state.refresh_tokens.remove(&old) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "",
            "unknown refresh token",
        );
    };
    let owner = state
        .devices
        .get(&id)
        .map(|(owner, _)| owner.clone())
        .unwrap_or_default();
    let next = format!("mock-refresh-{}", ulid::Ulid::from(rng.random::<u128>()));
    // Unique per refresh, so an old access token cannot start resolving to the
    // device of whichever refresh happened most recently.
    let access = access_token(&owner, &mut rng);
    state.refresh_tokens.insert(next.clone(), id.clone());
    state.issued.insert(access.clone(), id.clone());
    drop(state);
    (
        StatusCode::OK,
        json!({
            "access_token": access,
            "token_type": "DPoP",
            "expires_in": 3600,
            "refresh_token": next,
            "device_id": id,
        }),
    )
}

/// The fake Authentik's discovery document, addressed from wherever the request
/// arrived so the phone reaches it through whichever name it dialled.
async fn authentik_discovery(State(oauth): State<Oauth>, headers: HeaderMap) -> Response {
    let base = format!("{}{AUTHENTIK}", dialled_origin(&oauth, &headers));
    json_response(
        StatusCode::OK,
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}authorize/"),
            "token_endpoint": format!("{base}token/"),
            "jwks_uri": format!("{base}jwks/"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["EdDSA"],
            "code_challenge_methods_supported": ["S256"],
        }),
    )
}

/// Stands in for Authentik's login page: it redirects straight back to the app
/// with a code bound to the PKCE challenge.
async fn authentik_authorize(
    State(oauth): State<Oauth>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(redirect) = query.get("redirect_uri") else {
        return plain(StatusCode::BAD_REQUEST, "redirect_uri is required");
    };
    if query.get("code_challenge_method").map(String::as_str) != Some("S256")
        || !query.contains_key("code_challenge")
    {
        return plain(StatusCode::BAD_REQUEST, "PKCE S256 is required");
    }
    let code = format!(
        "mock-authz-{}",
        ulid::Ulid::from(rand::rng().random::<u128>())
    );
    {
        let mut state = oauth.store.lock();
        state
            .authz_codes
            .insert(code.clone(), query["code_challenge"].clone());
    }
    // Set the code and state on the redirect the app gave us.
    let mut location = redirect.clone();
    let separator = if location.contains('?') { '&' } else { '?' };
    location.push(separator);
    location.push_str(&format!(
        "code={code}&state={}",
        query.get("state").map_or("", String::as_str)
    ));
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::BAD_REQUEST.into_response())
}

async fn authentik_token(
    State(oauth): State<Oauth>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let code = form.get("code").cloned().unwrap_or_default();
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    // The code survives a wrong verifier, so a mistyped one can be retried; it
    // is only spent once it matches.
    let challenge = oauth.store.lock().authz_codes.get(&code).cloned();
    if !challenge.is_some_and(|expected| expected == s256(&verifier)) {
        let (status, body) = oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "",
            "unknown code or PKCE verifier mismatch",
        );
        return json_response(status, body);
    }
    oauth.store.lock().authz_codes.remove(&code);
    // No id_token: a real AppAuth validates one (issuer, audience, nonce), and
    // the app only needs the access token to exchange at the gateway.
    let subject = code.strip_prefix("mock-authz-").unwrap_or(&code);
    json_response(
        StatusCode::OK,
        json!({
            "access_token": format!("mock-authentik-{subject}"),
            "token_type": "Bearer",
            "expires_in": 300,
        }),
    )
}

/// `GET /mock/pairing`: start a pairing without an RPC, for tests that only
/// need a token.
async fn mock_pairing(
    State(oauth): State<Oauth>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let user_id = query
        .get("user")
        .cloned()
        .or_else(|| oauth.store.seed.principals.first().map(|p| p.id.clone()))
        .unwrap_or_else(|| "usr_omar".to_owned());
    let instance = &oauth.store.seed.instance;
    let issuer = issuer_for(
        &oauth.public_url,
        query.get("issuer").map_or("", String::as_str),
    );
    let (pairing, code, user_code) = new_pairing(&oauth.store, &user_id, &issuer);
    let exp = pairing
        .expires_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    json_response(
        StatusCode::OK,
        json!({
            "v": 1,
            "kind": "loams-pair",
            "issuer": issuer,
            "instance_id": instance.instance_id,
            // Plain HTTP mock: no TLS pins.
            "spki": Value::Null,
            "jkt": "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs",
            // The code that redeems it, which a phone that scanned the QR already has.
            "code": code,
            "user_code": user_code,
            "exp": exp,
        }),
    )
}

/// `POST /mock/approvals`: a pending approval, as an agent asking for one would
/// create, with a notification.
async fn mock_approval(State(oauth): State<Oauth>) -> Response {
    match crate::services::new_demo_approval(&oauth.store) {
        Some(id) => json_response(StatusCode::OK, json!({ "approval_id": id })),
        None => plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "no seeded principal to ask",
        ),
    }
}

/// `POST /mock/drop-streams`: make every open watch stream reconnect.
async fn mock_drop_streams(State(oauth): State<Oauth>) -> Response {
    let _ = oauth.store.drop_streams();
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /mock/tick`: advance the running operations and streams.
async fn mock_tick(State(oauth): State<Oauth>) -> Response {
    oauth.store.advance_operations();
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /mock/push-log`: what a push gateway would have received.
async fn mock_push_log(
    State(oauth): State<Oauth>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let after: u64 = query
        .get("after")
        .and_then(|raw| raw.parse().ok())
        .unwrap_or_default();
    let token = query.get("token").map(String::as_str);
    let state = oauth.store.lock();
    let records: Vec<Value> = state
        .push_log
        .iter()
        .filter(|p| p.seq > after && token.is_none_or(|t| p.token == t))
        .map(|p| {
            json!({
                "seq": p.seq,
                "target_id": p.target_id,
                "device_id": p.device_id,
                "provider": p.provider,
                "token": p.token,
                "app_id": p.app_id,
                "sealed": p.sealed,
            })
        })
        .collect();
    json_response(StatusCode::OK, Value::Array(records))
}

/// The base64url, unpadded SHA-256 a PKCE `code_challenge` is.
fn s256(verifier: &str) -> String {
    use sha2::{Digest as _, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The origin to hand a client, from `--public-url` when given (the emulator
/// reaches the host as `10.0.2.2`) and otherwise from the `Host` header.
///
/// The `Host` header, not the request URI: an HTTP/1.1 request line is
/// origin-form, so the URI carries no authority and no port at all.
fn dialled_origin(oauth: &Oauth, headers: &HeaderMap) -> String {
    if let Some(url) = &oauth.public_url {
        return url.trim_end_matches('/').to_owned();
    }
    match headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
    {
        Some(authority) => format!("http://{authority}"),
        None => "http://127.0.0.1".to_owned(),
    }
}

/// The fake Authentik's JWKS. Empty, because the mock does not sign anything:
/// it exists so a client's JWKS fetch succeeds rather than 404ing.
async fn authentik_jwks() -> Response {
    json_response(StatusCode::OK, json!({ "keys": [] }))
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn plain(status: StatusCode, message: &str) -> Response {
    let mut response = Response::new(Body::from(message.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::caller_from_header;

    #[test]
    fn two_devices_of_one_principal_get_distinct_tokens() {
        let (store, oauth) = mock();
        let redeem = |user_code: String| {
            let form = HashMap::from([("user_code".to_owned(), user_code)]);
            redeem_pairing(&oauth, &form).1
        };
        let (_, _, first_code) = new_pairing(&store, "usr_omar", "http://127.0.0.1:8084");
        let (_, _, second_code) = new_pairing(&store, "usr_omar", "http://127.0.0.1:8084");

        let first_body = redeem(first_code);
        let second_body = redeem(second_code);
        let token = |body: &Value| body["access_token"].as_str().unwrap().to_owned();
        let paired_device = |body: &Value| body["device_id"].as_str().unwrap().to_owned();
        let first = token(&first_body);
        let second = token(&second_body);
        let first_device = paired_device(&first_body);
        let second_device = paired_device(&second_body);
        assert_ne!(first, second, "both devices were handed the same token");

        // The point of distinct tokens: each still names its own device.
        let device_of = |token: &str| store.lock().issued.get(token).cloned().unwrap_or_default();
        assert_ne!(device_of(&first), device_of(&second));
        assert_eq!(device_of(&first), first_device);
        assert_eq!(device_of(&second), second_device);
    }

    #[test]
    fn s256_is_the_unpadded_base64url_digest() {
        // RFC 7636 Appendix B.
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    /// A store and an [`Oauth`] over it, as `serve` builds.
    fn mock() -> (Arc<Store>, Oauth) {
        let store = Arc::new(Store::new(
            crate::seed::Seed::demo(),
            std::time::Duration::from_secs(15),
            None,
        ));
        let oauth = Oauth {
            store: Arc::clone(&store),
            public_url: None,
        };
        (store, oauth)
    }

    #[test]
    fn a_pairing_is_redeemable_once() {
        let (store, oauth) = mock();
        let (_, _, user_code) = new_pairing(&store, "usr_omar", "http://127.0.0.1:8084");
        let form = HashMap::from([("user_code".to_owned(), user_code)]);
        let (status, body) = redeem_pairing(&oauth, &form);
        assert_eq!(status, StatusCode::OK);
        // Issued tokens carry a discriminator, so assert the stable part and
        // that the principal still resolves.
        let access = body["access_token"].as_str().unwrap();
        assert!(
            access.starts_with("mock-access-usr_omar-"),
            "unexpected access token {access}"
        );
        let principal =
            caller_from_header(&store.seed, &format!("Bearer {access}"), SystemTime::now());
        assert_eq!(principal.unwrap().principal.id, "usr_omar");
        assert_eq!(
            body["token_type"], "DPoP",
            "a phone presents DPoP, not a bare bearer"
        );
        // The device it registered is listed for its owner.
        let state = store.lock();
        let device_id = body["device_id"].as_str().unwrap();
        assert_eq!(state.devices[device_id].0, "usr_omar");
        drop(state);
        // A second attempt is refused, not silently reissued.
        let (status, body) = redeem_pairing(&oauth, &form);
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");
        assert_eq!(body["loams_reason"], "pairing_used");
    }

    #[test]
    fn wrong_user_codes_are_bounded_then_burn_the_pairing() {
        let (store, oauth) = mock();
        new_pairing(&store, "usr_omar", "http://127.0.0.1:8084");
        let wrong = HashMap::from([("user_code".to_owned(), "00000000".to_owned())]);
        for _ in 0..USER_CODE_FAILURES {
            let (status, _) = redeem_pairing(&oauth, &wrong);
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        assert!(store.lock().pairings.values().all(|p| p.used));
    }

    #[test]
    fn sending_both_a_code_and_a_user_code_is_an_invalid_request() {
        let (store, oauth) = mock();
        new_pairing(&store, "usr_omar", "http://127.0.0.1:8084");
        let form = HashMap::from([
            ("code".to_owned(), "whatever".to_owned()),
            ("user_code".to_owned(), "12345678".to_owned()),
        ]);
        let (status, body) = redeem_pairing(&oauth, &form);
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_request");
    }

    #[test]
    fn an_exchange_needs_an_authentik_token() {
        let (store, oauth) = mock();
        let bad = HashMap::from([("subject_token".to_owned(), "nope".to_owned())]);
        assert_eq!(exchange(&oauth, &bad).0, StatusCode::BAD_REQUEST);
        let good = HashMap::from([("subject_token".to_owned(), "mock-authentik-abc".to_owned())]);
        let (status, body) = exchange(&oauth, &good);
        assert_eq!(status, StatusCode::OK);
        assert!(
            body["access_token"]
                .as_str()
                .unwrap()
                .starts_with("mock-access-")
        );
        // The device the exchange registered is listed, alongside the seeded
        // phone, for its owner.
        let device_id = body["device_id"].as_str().unwrap();
        let state = store.lock();
        assert_eq!(state.devices[device_id].0, "usr_dana");
    }

    #[test]
    fn a_refresh_rotates_and_the_old_token_stops_working() {
        let (_store, oauth) = mock();
        let (_, first) = issue(&oauth, "usr_omar", &HashMap::new());
        let old = first["refresh_token"].as_str().unwrap().to_owned();
        let form = HashMap::from([("refresh_token".to_owned(), old)]);
        let (status, second) = refresh(&oauth, &form);
        assert_eq!(status, StatusCode::OK);
        assert_ne!(
            second["refresh_token"], first["refresh_token"],
            "refresh rotates"
        );
        let (status, _) = refresh(&oauth, &form);
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the old refresh token was consumed"
        );
    }

    #[test]
    fn the_public_url_wins_over_the_seeded_issuer() {
        assert_eq!(
            issuer_for(&Some("http://10.0.2.2:8084".to_owned()), "https://x"),
            "http://10.0.2.2:8084"
        );
        assert_eq!(issuer_for(&None, "https://x"), "https://x");
    }
}
