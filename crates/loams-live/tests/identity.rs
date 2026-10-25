//! LV1 plan Task 16: app end users through OIDC (design §45 §5.3), through a
//! real server. A stand-in issuer serves OIDC discovery and a JWKS with a
//! generated Ed25519 key. A test guard stands in for the engine's
//! authentication layer: `x-test-kind: anonymous` is a request the layer
//! left to the app (no engine credential, or a JWT from an issuer the
//! engine does not take), as `api::live::caller` marks it; `dev` is the
//! developer on loopback; no header is a bare server.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use buffa::MessageField;
use connectrpc::ConnectError;
use connectrpc::client::{CallOptions, ClientConfig, HttpClient};
use loams_live::apps::TrustedIssuer;
use loams_live::auth::{Caller, CallerKind, EndUsers};
use loams_live::session::{ClientState, QueryResult, SessionConfig};
use loams_live::system::QUERY;
use loams_live::testing::TestStore;
use loams_live::{APP_HEADER, LiveHandle, LiveServer, LiveValue, live_test, pb};
use serde_json::json;
use tokio_util::sync::CancellationToken;

type Client = pb::LiveServiceClient<HttpClient>;
type Admin = pb::LiveAdminServiceClient<HttpClient>;
type Watch = connectrpc::client::ServerStream<
    <HttpClient as connectrpc::client::ClientTransport>::ResponseBody,
    pb::__buffa::view::TransitionView<'static>,
>;

/// The app the suite's end users belong to.
const APP: &str = "shop";

#[path = "support/oidc.rs"]
mod oidc;
use oidc::{Issuer, Key};

// ---- the test authentication layer and the server ----

async fn test_layer(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let kind = request
        .headers()
        .get("x-test-kind")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let caller = match kind.as_deref() {
        Some("anonymous") => Some(Caller::anonymous()),
        Some("dev") => Some(Caller::new("dev-loopback", CallerKind::DevLoopback)),
        Some("api_key") => Some(Caller::new("apikey:01", CallerKind::ApiKey)),
        _ => None,
    };
    if let Some(caller) = caller {
        request.extensions_mut().insert(caller);
    }
    next.run(request).await
}

fn bundle() -> Vec<u8> {
    br#"
import { query, mutation, internalQuery } from "loams:server";
export const me = {
  whoami: query(async (ctx) => await ctx.auth.getUserIdentity()),
  note: mutation(async (ctx) => {
    const who = await ctx.auth.getUserIdentity();
    const body = who === null ? "anonymous" : who.tokenIdentifier;
    await ctx.db.insert("notes", { body });
    return body;
  }),
  audit: internalQuery(async () => null),
};
"#
    .to_vec()
}

/// A server for app `main`, behind the test layer, with the app `shop`
/// trusting `issuer` (anonymous calls as `anonymous` says) and the bundle
/// deployed to it.
async fn server(store: &TestStore, issuer: &Issuer, anonymous: bool) -> LiveHandle {
    server_trusting(store, issuer.trusted_pb(), anonymous).await
}

async fn server_trusting(
    store: &TestStore,
    issuer: pb::TrustedIssuer,
    anonymous: bool,
) -> LiveHandle {
    server_with(store, issuer, anonymous, SessionConfig::default()).await
}

async fn server_with(
    store: &TestStore,
    issuer: pb::TrustedIssuer,
    anonymous: bool,
    session: SessionConfig,
) -> LiveHandle {
    let mut config = store.live_config("main");
    config.session = session;
    config.listen = "127.0.0.1:0".parse().expect("an address");
    let loader = loams_live_js::JsLoader::new(loams_live_js::JsConfig {
        contexts: 1,
        ..loams_live_js::JsConfig::default()
    })
    .expect("an in-process loader");
    let guard: loams_live::service::Guard =
        Arc::new(|router: axum::Router| router.layer(axum::middleware::from_fn(test_layer)));
    let handle = LiveServer::start_guarded(
        config,
        loams_live::deploy::Runtime {
            bundles: loams_store::Store::in_memory(),
            loader: Arc::new(loader),
        },
        Some(guard),
        CancellationToken::new(),
    )
    .await
    .expect("the server starts");
    let a = admin(&handle);
    a.create_app(pb::CreateAppRequest {
        name: APP.into(),
        namespace: "shop/prod".into(),
        issuers: vec![issuer],
        allow_anonymous: anonymous,
        ..Default::default()
    })
    .await
    .expect("CreateApp");
    a.deploy_with_options(
        pb::DeployRequest {
            bundle: bundle(),
            ..Default::default()
        },
        CallOptions::default().with_header(APP_HEADER, APP),
    )
    .await
    .expect("Deploy");
    handle
}

fn config(handle: &LiveHandle) -> ClientConfig {
    ClientConfig::new(format!("http://{}", handle.addr).parse().expect("a uri"))
}

fn client(handle: &LiveHandle) -> Client {
    pb::LiveServiceClient::new(HttpClient::plaintext(), config(handle))
}

fn admin(handle: &LiveHandle) -> Admin {
    pb::LiveAdminServiceClient::new(HttpClient::plaintext(), config(handle))
}

/// The options of a call to `shop` as `kind` with `bearer`.
fn who(kind: Option<&str>, bearer: Option<&str>) -> CallOptions {
    let mut o = CallOptions::default().with_header(APP_HEADER, APP);
    if let Some(kind) = kind {
        o = o.with_header("x-test-kind", kind);
    }
    if let Some(token) = bearer {
        o = o.with_header("authorization", format!("Bearer {token}"));
    }
    o
}

/// `allowed`, or the refusal's Connect code and `ErrorInfo.reason`.
fn outcome<T>(result: Result<T, ConnectError>) -> String {
    use buffa::Message as _;
    let Err(e) = result else {
        return "allowed".to_string();
    };
    let reason = e
        .details
        .iter()
        .find(|d| d.type_url.ends_with("loams.errors.v1.ErrorInfo"))
        .and_then(|d| d.value.as_deref())
        .and_then(|v| {
            data_encoding::BASE64_NOPAD
                .decode(v.trim_end_matches('=').as_bytes())
                .ok()
        })
        .and_then(|b| loams_proto::loams::errors::v1::ErrorInfo::decode_from_slice(&b).ok())
        .map(|i| i.reason)
        .unwrap_or_default();
    format!("{:?} {reason}", e.code).to_lowercase()
}

async fn query(c: &Client, function: &str, o: CallOptions) -> Result<LiveValue, ConnectError> {
    let args = match function {
        QUERY => LiveValue::Object([("table".to_string(), LiveValue::Str("notes".into()))].into()),
        _ => LiveValue::Object(Default::default()),
    };
    let r = c
        .query_with_options(
            pb::QueryRequest {
                function: function.into(),
                args: MessageField::some(args.to_proto()),
                ..Default::default()
            },
            o,
        )
        .await?
        .into_owned();
    Ok(LiveValue::from_proto(r.result.into_option().expect("a result")).expect("a value"))
}

async fn mutate(c: &Client, function: &str, o: CallOptions) -> Result<LiveValue, ConnectError> {
    let r = c
        .mutate_with_options(
            pb::MutateRequest {
                function: function.into(),
                args: MessageField::some(LiveValue::Object(Default::default()).to_proto()),
                ..Default::default()
            },
            o,
        )
        .await?
        .into_owned();
    Ok(LiveValue::from_proto(r.result.into_option().expect("a result")).expect("a value"))
}

fn field<'a>(v: &'a LiveValue, name: &str) -> &'a LiveValue {
    match v {
        LiveValue::Object(fields) => fields
            .get(name)
            .unwrap_or_else(|| panic!("no field {name} in {v:?}")),
        other => panic!("not an object: {other:?}"),
    }
}

// ---- the tests ----

/// An end user with a token from the app's issuer calls its public
/// functions, a query and a mutation.
async fn end_user_token_from_configured_issuer_calls_public_function(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, false).await;
    let c = client(&handle);
    let token = issuer.token("ada");
    let me = query(&c, "me:whoami", who(Some("anonymous"), Some(&token)))
        .await
        .expect("an end user calls a public query");
    assert_eq!(field(&me, "subject"), &LiveValue::Str("ada".into()));
    let written = mutate(&c, "me:note", who(Some("anonymous"), Some(&token)))
        .await
        .expect("an end user calls a public mutation");
    assert_eq!(
        written,
        LiveValue::Str(format!("{}|ada", issuer.url)),
        "the mutation ran as the end user"
    );
    handle.stop().await;
}
live_test!(end_user_token_from_configured_issuer_calls_public_function);

/// An end user never reaches internal functions, `_system:*` or the admin
/// service, whatever the token says.
async fn end_user_cannot_call_internal_or_system_or_admin(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, false).await;
    let (c, a) = (client(&handle), admin(&handle));
    // A token naming every Live scope changes nothing.
    let token = issuer.token_with(&issuer.key, "ada", |claims| {
        claims["scp"] = json!([
            "live:call",
            "live:read",
            "live:write",
            "live:deploy",
            "live:admin"
        ]);
    });
    for kind in ["anonymous", "dev"] {
        let o = || who(Some(kind), Some(&token));
        assert_eq!(
            outcome(query(&c, "me:audit", o()).await),
            "permissiondenied permission_denied",
            "{kind}: an internal function"
        );
        assert_eq!(
            outcome(query(&c, QUERY, o()).await),
            "permissiondenied permission_denied",
            "{kind}: _system:query"
        );
        let deploy = a
            .deploy_with_options(
                pb::DeployRequest {
                    bundle: bundle(),
                    dry_run: true,
                    ..Default::default()
                },
                o(),
            )
            .await;
        assert_eq!(
            outcome(deploy),
            "permissiondenied permission_denied",
            "{kind}: Deploy"
        );
    }
    let update = a
        .update_app_with_options(
            pb::UpdateAppRequest {
                name: APP.into(),
                allow_anonymous: Some(true),
                ..Default::default()
            },
            who(Some("anonymous"), Some(&token)),
        )
        .await;
    assert_eq!(outcome(update), "permissiondenied permission_denied");
    handle.stop().await;
}
live_test!(end_user_cannot_call_internal_or_system_or_admin);

/// A token the app's issuers do not vouch for is `UNAUTHENTICATED`: another
/// issuer, another audience, an expired one, a forged signature.
async fn token_from_unconfigured_issuer_rejected(store: TestStore) {
    let issuer = Issuer::start().await;
    let stranger = Issuer::start().await;
    let handle = server(&store, &issuer, true).await;
    let c = client(&handle);
    let refused = |token: String| {
        let c = &c;
        async move { outcome(query(c, "me:whoami", who(Some("anonymous"), Some(&token))).await) }
    };
    assert_eq!(
        refused(stranger.token("ada")).await,
        "unauthenticated token_wrong_issuer"
    );
    assert_eq!(
        refused(issuer.token_with(&issuer.key, "ada", |c| c["aud"] = json!("elsewhere"))).await,
        "unauthenticated token_wrong_audience"
    );
    assert_eq!(
        refused(issuer.token_with(&issuer.key, "ada", |c| c["exp"] = json!(1_000_000))).await,
        "unauthenticated token_expired"
    );
    // The stranger's key under the issuer's `kid` and `iss`.
    let forged = issuer.token_with(&Key::generate("k1"), "ada", |_| {});
    assert_eq!(refused(forged).await, "unauthenticated token_bad_signature");
    // On a bare server or for the developer, a stranger's token is not the
    // app's end user: the caller stays a platform principal.
    let me = query(
        &c,
        "me:whoami",
        who(Some("dev"), Some(&stranger.token("ada"))),
    )
    .await
    .expect("the developer calls");
    assert_eq!(me, LiveValue::Null);
    handle.stop().await;
}
live_test!(token_from_unconfigured_issuer_rejected);

/// `ctx.auth.getUserIdentity()` is the end user's identity and claims, and
/// null for a platform principal or an anonymous call.
async fn ctx_auth_returns_identity_claims(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, true).await;
    let c = client(&handle);
    let token = issuer.token("ada");
    for kind in [Some("anonymous"), Some("dev"), None] {
        let me = query(&c, "me:whoami", who(kind, Some(&token)))
            .await
            .expect("whoami");
        assert_eq!(field(&me, "issuer"), &LiveValue::Str(issuer.url.clone()));
        assert_eq!(field(&me, "subject"), &LiveValue::Str("ada".into()));
        assert_eq!(
            field(&me, "tokenIdentifier"),
            &LiveValue::Str(format!("{}|ada", issuer.url))
        );
        assert_eq!(
            field(&me, "email"),
            &LiveValue::Str("ada@example.com".into())
        );
        assert_eq!(field(&me, "name"), &LiveValue::Str("Ada".into()));
        let claims = field(&me, "claims");
        assert_eq!(field(claims, "plan"), &LiveValue::Str("pro".into()));
        assert!(
            matches!(field(claims, "exp"), LiveValue::F64(_)),
            "numbers are JavaScript numbers"
        );
    }
    for (kind, bearer) in [
        (Some("api_key"), Some(token.as_str())),
        (Some("anonymous"), None),
    ] {
        let me = query(&c, "me:whoami", who(kind, bearer))
            .await
            .expect("whoami");
        assert_eq!(me, LiveValue::Null, "{kind:?}");
    }
    handle.stop().await;
}
live_test!(ctx_auth_returns_identity_claims);

/// With no credential, a call is admitted only where the app takes
/// anonymous calls, and then only to public functions.
async fn anonymous_allowed_only_when_configured(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, false).await;
    let (c, a) = (client(&handle), admin(&handle));
    let anonymous = || who(Some("anonymous"), None);
    assert_eq!(
        outcome(query(&c, "me:whoami", anonymous()).await),
        "unauthenticated token_missing"
    );
    a.update_app(pb::UpdateAppRequest {
        name: APP.into(),
        allow_anonymous: Some(true),
        ..Default::default()
    })
    .await
    .expect("UpdateApp");
    assert_eq!(
        query(&c, "me:whoami", anonymous())
            .await
            .expect("anonymous"),
        LiveValue::Null
    );
    assert_eq!(
        mutate(&c, "me:note", anonymous()).await.expect("anonymous"),
        LiveValue::Str("anonymous".into())
    );
    assert_eq!(
        outcome(query(&c, "me:audit", anonymous()).await),
        "permissiondenied permission_denied"
    );
    assert_eq!(
        outcome(query(&c, QUERY, anonymous()).await),
        "permissiondenied permission_denied"
    );
    handle.stop().await;
}
live_test!(anonymous_allowed_only_when_configured);

/// An unknown `kid` fetches the JWKS again at most once per interval, so a
/// flood of tokens with made-up key ids cannot hammer the issuer; a rotated
/// key is picked up once the interval has passed.
#[tokio::test]
async fn jwks_refresh_rate_limited() {
    let issuer = Issuer::start().await;
    let gap = Duration::from_millis(600);
    let users = EndUsers::with_timing(Duration::from_secs(600), gap);
    let issuers = [issuer.trusted()];
    let caller = users
        .verify(&issuer.token("ada"), &issuers)
        .await
        .expect("a valid token");
    assert_eq!(caller.kind, CallerKind::EndUser);
    assert_eq!(issuer.jwks_fetches.load(Ordering::SeqCst), 1);
    // Rotate: the issuer now signs with k2 and publishes it.
    let k2 = Key::generate("k2");
    issuer
        .published
        .lock()
        .expect("test fixture")
        .push(k2.jwk.clone());
    let rotated = issuer.token_with(&k2, "ada", |_| {});
    let unknown = issuer.token_with(&Key::generate("nope"), "ada", |_| {});
    for _ in 0..20 {
        let e = users
            .verify(&unknown, &issuers)
            .await
            .expect_err("unknown kid");
        assert!(format!("{e}").contains("does not publish"), "{e}");
    }
    assert!(
        users.verify(&rotated, &issuers).await.is_err(),
        "too soon: the cached JWKS has no k2"
    );
    assert_eq!(
        issuer.jwks_fetches.load(Ordering::SeqCst),
        1,
        "no fetch within the interval"
    );
    tokio::time::sleep(gap + Duration::from_millis(100)).await;
    users
        .verify(&rotated, &issuers)
        .await
        .expect("the rotated key, fetched once the interval passed");
    for _ in 0..20 {
        let _ = users.verify(&unknown, &issuers).await;
    }
    assert_eq!(issuer.jwks_fetches.load(Ordering::SeqCst), 2);
}

/// An issuer that is down is not asked again within the interval, however
/// many tokens arrive: they are `UNAVAILABLE` (`auth_unavailable`) until it
/// answers.
#[tokio::test]
async fn jwks_fetch_failures_are_rate_limited_too() {
    let issuer = Issuer::start().await;
    issuer.down.store(true, Ordering::SeqCst);
    let gap = Duration::from_millis(600);
    let users = EndUsers::with_timing(Duration::from_secs(600), gap);
    let issuers = [issuer.trusted()];
    let token = issuer.token("ada");
    for _ in 0..10 {
        let e = users
            .verify(&token, &issuers)
            .await
            .expect_err("the issuer is down");
        assert_eq!(e.reason(), Some("auth_unavailable"), "{e}");
    }
    assert_eq!(issuer.jwks_fetches.load(Ordering::SeqCst), 1);
    issuer.down.store(false, Ordering::SeqCst);
    tokio::time::sleep(gap + Duration::from_millis(100)).await;
    users
        .verify(&token, &issuers)
        .await
        .expect("the issuer is back");
    assert_eq!(issuer.jwks_fetches.load(Ordering::SeqCst), 2);
}

/// Against a real OIDC issuer (Zitadel, or any issuer the install trusts):
/// a token it issued calls a public function. Skipped unless
/// `LOAMS_TEST_OIDC_ISSUER`, `LOAMS_TEST_OIDC_AUDIENCE` and
/// `LOAMS_TEST_OIDC_TOKEN` name the issuer, the app's client id and a
/// fresh access token.
async fn oidc_end_user_e2e(store: TestStore) {
    let vars = (
        std::env::var("LOAMS_TEST_OIDC_ISSUER"),
        std::env::var("LOAMS_TEST_OIDC_AUDIENCE"),
        std::env::var("LOAMS_TEST_OIDC_TOKEN"),
    );
    let (Ok(issuer), Ok(audience), Ok(token)) = vars else {
        eprintln!("skipped: oidc_end_user_e2e needs LOAMS_TEST_OIDC_ISSUER, _AUDIENCE and _TOKEN");
        return;
    };
    let trusted = TrustedIssuer {
        issuer,
        audiences: vec![audience],
        jwks_uri: None,
    };
    let handle = server_trusting(&store, trusted.to_proto(), false).await;
    let me = query(
        &client(&handle),
        "me:whoami",
        who(Some("anonymous"), Some(&token)),
    )
    .await
    .expect("an issuer's token calls a public function");
    assert!(matches!(field(&me, "subject"), LiveValue::Str(s) if !s.is_empty()));
    handle.stop().await;
}
live_test!(oidc_end_user_e2e);

// ---- session identity (LV1 plan Task 17) ----

/// A Watch of `me:whoami` (query 1) as `o`'s caller.
async fn watch_whoami(c: &Client, o: CallOptions) -> Watch {
    let set = pb::QuerySet {
        version: 1,
        queries: vec![pb::QuerySpec {
            query_id: 1,
            function: "me:whoami".into(),
            args: MessageField::some(LiveValue::Object(Default::default()).to_proto()),
            ..Default::default()
        }],
        ..Default::default()
    };
    c.watch_with_options(
        pb::WatchRequest {
            start: Some(pb::__buffa::oneof::watch_request::Start::Initial(Box::new(
                set,
            ))),
            ..Default::default()
        },
        o,
    )
    .await
    .expect("Watch opens")
}

/// The next message of `w`: a Transition, or the stream's end (`Ok(None)`)
/// or error.
async fn next(w: &mut Watch) -> Result<Option<pb::Transition>, ConnectError> {
    tokio::time::timeout(Duration::from_secs(20), w.message())
        .await
        .expect("a message within 20 s")
        .map(|m| m.map(|t| t.to_owned_message()))
}

/// Applies Transitions until one carries a result for query 1; returns it,
/// the Transition's end identity version and the session id.
async fn whoami(w: &mut Watch, client: &mut ClientState) -> (LiveValue, u64, String) {
    loop {
        let t = next(w).await.expect("the stream").expect("not the end");
        client.apply(&t).expect("in order");
        if t.updates.iter().any(|u| u.query_id == 1) {
            let value = match &client.results[&1] {
                QueryResult::Value(v) => v.clone(),
                QueryResult::Error(e) => panic!("whoami failed: {e:?}"),
            };
            return (value, client.version.identity, t.session_id);
        }
    }
}

fn subject(v: &LiveValue) -> Option<String> {
    match v {
        LiveValue::Null => None,
        v => match field(v, "subject") {
            LiveValue::Str(s) => Some(s.clone()),
            other => panic!("a subject: {other:?}"),
        },
    }
}

async fn authenticate(c: &Client, session: &str, o: CallOptions) -> Result<u64, ConnectError> {
    Ok(c.authenticate_with_options(
        pb::AuthenticateRequest {
            session_id: session.into(),
            ..Default::default()
        },
        o,
    )
    .await?
    .into_owned()
    .identity)
}

/// The app `shop`'s subscription counters.
async fn shop_stats(handle: &LiveHandle) -> loams_live::SubsStats {
    let shop = handle
        .engines()
        .resolve(Some(APP))
        .await
        .expect("shop runs");
    shop.subs().stats()
}

/// `Authenticate` with the same caller (a refreshed token) moves the expiry
/// only: no rerun, no new identity version.
async fn authenticate_same_subject_extends_without_rerun(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, false).await;
    let c = client(&handle);
    let first = issuer.token("ada");
    let mut w = watch_whoami(&c, who(Some("anonymous"), Some(&first))).await;
    let mut state = ClientState::new();
    let (me, identity, session) = whoami(&mut w, &mut state).await;
    assert_eq!((subject(&me).as_deref(), identity), (Some("ada"), 0));
    let before = shop_stats(&handle).await.reruns;
    let refreshed = issuer.token_with(&issuer.key, "ada", |c| c["iat"] = json!(1));
    let version = authenticate(&c, &session, who(Some("anonymous"), Some(&refreshed)))
        .await
        .expect("Authenticate");
    assert_eq!(version, 0, "the same subject keeps the identity version");
    assert_eq!(shop_stats(&handle).await.reruns, before, "nothing reran");
    handle.stop().await;
}
live_test!(authenticate_same_subject_extends_without_rerun);

/// `Authenticate` with another caller bumps the identity version and reruns
/// every query as the new caller; a sign-out to anonymous does the same.
async fn authenticate_new_subject_bumps_identity_and_reruns(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, true).await;
    let c = client(&handle);
    let mut w = watch_whoami(&c, who(Some("anonymous"), Some(&issuer.token("ada")))).await;
    let mut state = ClientState::new();
    let (me, _, session) = whoami(&mut w, &mut state).await;
    assert_eq!(subject(&me).as_deref(), Some("ada"));
    let version = authenticate(
        &c,
        &session,
        who(Some("anonymous"), Some(&issuer.token("bob"))),
    )
    .await
    .expect("Authenticate as bob");
    assert_eq!(version, 1);
    let (me, identity, _) = whoami(&mut w, &mut state).await;
    assert_eq!((subject(&me).as_deref(), identity), (Some("bob"), 1));
    // Signing out: anonymous (the app takes anonymous calls).
    let version = authenticate(&c, &session, who(Some("anonymous"), None))
        .await
        .expect("sign out");
    assert_eq!(version, 2);
    let (me, identity, _) = whoami(&mut w, &mut state).await;
    assert_eq!((me, identity), (LiveValue::Null, 2));
    // A token that does not verify changes nothing.
    let e = authenticate(&c, &session, who(Some("anonymous"), Some("e30.e30.sig")))
        .await
        .expect_err("a bad token");
    assert_eq!(e.code, connectrpc::ErrorCode::Unauthenticated, "{e:?}");
    // An unknown session.
    let e = authenticate(&c, "nobody", who(Some("anonymous"), None))
        .await
        .expect_err("no such session");
    assert_eq!(e.code, connectrpc::ErrorCode::NotFound, "{e:?}");
    handle.stop().await;
}
live_test!(authenticate_new_subject_bumps_identity_and_reruns);

/// A session whose token expires (plus the leeway) without an
/// `Authenticate` is closed with `token_expired`.
async fn expired_session_closed_with_token_expired(store: TestStore) {
    let issuer = Issuer::start().await;
    let session = SessionConfig {
        expiry_leeway: Duration::from_millis(200),
        ..SessionConfig::default()
    };
    let handle = server_with(&store, issuer.trusted_pb(), false, session).await;
    let c = client(&handle);
    // Valid for verification (60 s leeway), but already past its `exp`.
    let stale = issuer.token_with(&issuer.key, "ada", |claims| {
        let now = claims["iat"].as_u64().expect("iat");
        claims["exp"] = json!(now - 1);
    });
    let mut w = watch_whoami(&c, who(Some("anonymous"), Some(&stale))).await;
    let mut state = ClientState::new();
    whoami(&mut w, &mut state).await;
    let closed = loop {
        match next(&mut w).await {
            Ok(Some(t)) => state.apply(&t).expect("in order"),
            Ok(None) => panic!("the stream ended without an error"),
            Err(e) => break e,
        };
    };
    assert_eq!(outcome::<()>(Err(closed)), "unauthenticated token_expired");
    handle.stop().await;
}
live_test!(expired_session_closed_with_token_expired);

/// Revoking a credential closes its sessions with `token_revoked`; others
/// stay open.
async fn revoked_key_closes_sessions(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, false).await;
    let c = client(&handle);
    let mut revoked = watch_whoami(&c, who(Some("api_key"), None)).await;
    let mut kept = watch_whoami(&c, who(Some("anonymous"), Some(&issuer.token("ada")))).await;
    let (mut a, mut b) = (ClientState::new(), ClientState::new());
    whoami(&mut revoked, &mut a).await;
    whoami(&mut kept, &mut b).await;
    handle.routes().revoke("apikey:01");
    let closed = loop {
        match next(&mut revoked).await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the stream ended without an error"),
            Err(e) => break e,
        }
    };
    assert_eq!(outcome::<()>(Err(closed)), "unauthenticated token_revoked");
    let shop = handle
        .engines()
        .resolve(Some(APP))
        .await
        .expect("shop runs");
    // The revoked session's task sends the error, then leaves the table: the
    // client can see the error first, so wait for the table to catch up.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while shop.sessions().len() > 1 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(shop.sessions().len(), 1, "the other session stays");
    handle.stop().await;
}
live_test!(revoked_key_closes_sessions);

/// A stream is rotated after the session's maximum age (24 h by default)
/// with `session_rotate`, and the client resumes.
async fn stream_rotates_after_24h(store: TestStore) {
    assert_eq!(
        SessionConfig::default().max_age,
        Duration::from_secs(24 * 60 * 60)
    );
    let issuer = Issuer::start().await;
    let session = SessionConfig {
        max_age: Duration::from_millis(400),
        ..SessionConfig::default()
    };
    let handle = server_with(&store, issuer.trusted_pb(), false, session).await;
    let c = client(&handle);
    let mut w = watch_whoami(&c, who(Some("api_key"), None)).await;
    let mut state = ClientState::new();
    whoami(&mut w, &mut state).await;
    let closed = loop {
        match next(&mut w).await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the stream ended without an error"),
            Err(e) => break e,
        }
    };
    assert_eq!(outcome::<()>(Err(closed)), "unavailable session_rotate");
    handle.stop().await;
}
live_test!(stream_rotates_after_24h);

/// Two end users watching one `ctx.auth`-dependent query each see their own
/// result, and the manager holds one subscription per user.
async fn results_never_shared_across_identities(store: TestStore) {
    let issuer = Issuer::start().await;
    let handle = server(&store, &issuer, false).await;
    let c = client(&handle);
    let mut ada = watch_whoami(&c, who(Some("anonymous"), Some(&issuer.token("ada")))).await;
    let mut bob = watch_whoami(&c, who(Some("anonymous"), Some(&issuer.token("bob")))).await;
    let (mut a, mut b) = (ClientState::new(), ClientState::new());
    let (me_a, ..) = whoami(&mut ada, &mut a).await;
    let (me_b, ..) = whoami(&mut bob, &mut b).await;
    assert_eq!(subject(&me_a).as_deref(), Some("ada"));
    assert_eq!(subject(&me_b).as_deref(), Some("bob"));
    assert_eq!(shop_stats(&handle).await.subscriptions, 2);
    handle.stop().await;
}
live_test!(results_never_shared_across_identities);
