//! Engine authentication (preview contract, [`loams::auth`]) against the real
//! server: every HTTP surface refuses a request without a credential, an API
//! key and an OIDC token from a stand-in issuer are admitted, and each bad
//! token gets its own reason.

#![cfg(all(feature = "qdrant", feature = "es"))]

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use loams::auth::oidc::{Issuer as TrustedIssuer, OidcConfig};
use loams::auth::tailscale::TailscaleConfig;
use loams::auth::{AuthConfig, AuthMode};
use loams::{Server, ServerConfig};
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::{Value, json};
use tempfile::TempDir;

const AUDIENCE: &str = "loams-console";
/// A console on an origin of its own, listed in `LOAMS_CORS_ORIGINS`.
const CONSOLE: &str = "https://console.example.com";
const LOOPBACK0: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

struct Running {
    server: Server,
    base: String,
    _dir: TempDir,
}

impl Running {
    async fn start(auth: AuthConfig, edit: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let mut config = ServerConfig::new(dir.path());
        config.listen = LOOPBACK0;
        config.log.flush_interval = Duration::from_millis(20);
        config.worker_poll_interval = Duration::from_millis(50);
        config.auth = auth;
        edit(&mut config);
        let server = Server::start(config).await.expect("start");
        let base = format!("http://{}", server.local_addr());
        Self {
            server,
            base,
            _dir: dir,
        }
    }

    /// `WhoAmI` over Connect JSON with `authorization` (if any).
    async fn who_am_i(&self, authorization: Option<&str>) -> (u16, Option<String>, Value) {
        let mut request = reqwest::Client::new()
            .post(format!(
                "{}/loams.instance.v1.InstanceService/WhoAmI",
                self.base
            ))
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .body("{}");
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        let response = request.send().await.expect("call");
        let status = response.status().as_u16();
        let reason = response
            .headers()
            .get("loams-auth-reason")
            .map(|v| v.to_str().expect("test fixture").to_owned());
        let body = response.json().await.unwrap_or(Value::Null);
        (status, reason, body)
    }
}

/// A gRPC unary call with an empty message over h2c: `(grpc-status, reason)`.
async fn grpc_status(
    addr: SocketAddr,
    path: &str,
    authorization: Option<&str>,
) -> (String, String) {
    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .expect("test fixture");
    let mut request = client
        .post(format!("http://{addr}{path}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(vec![0u8, 0, 0, 0, 0]);
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    let response = request.send().await.expect("grpc call");
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|v| v.to_str().expect("test fixture").to_owned())
            .unwrap_or_default()
    };
    (header("grpc-status"), header("loams-auth-reason"))
}

async fn http_get(url: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut request = reqwest::Client::new().get(url);
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = request.send().await.expect("get");
    let reason = response
        .headers()
        .get("loams-auth-reason")
        .map(|v| v.to_str().expect("test fixture").to_owned())
        .unwrap_or_default();
    (response.status().as_u16(), reason)
}

fn keys_file(dir: &TempDir) -> (String, std::path::PathBuf) {
    let (key, line) = loams::auth::api_key::new_key("test");
    let path = dir.path().join("api-keys");
    std::fs::write(&path, format!("# test keys\n{line}\n")).expect("test fixture");
    (key, path)
}

#[tokio::test]
async fn api_keys_guard_every_http_surface() {
    let keys = TempDir::new().unwrap();
    let (key, path) = keys_file(&keys);
    let running = Running::start(AuthConfig::api_keys(&path), |config| {
        config.flight_sql = Some(LOOPBACK0);
        config.qdrant = Some(loams_qdrant::QdrantConfig {
            rest_listen: LOOPBACK0,
            grpc_listen: LOOPBACK0,
            ..loams_qdrant::QdrantConfig::default()
        });
        config.es = Some(loams_es::EsConfig {
            listen: LOOPBACK0,
            ..loams_es::EsConfig::default()
        });
    })
    .await;
    let server = &running.server;
    let bearer = format!("Bearer {key}");
    let qdrant_rest = format!("http://{}/collections", server.qdrant_rest_addr().unwrap());
    let es = format!("http://{}/", server.es_addr().unwrap());
    let rest = format!("{}/v1/namespaces/default/streams", running.base);

    // Without a credential: refused everywhere, in each protocol's shape.
    for url in [&rest, &qdrant_rest, &es] {
        assert_eq!(
            http_get(url, &[]).await,
            (401, "token_missing".into()),
            "{url}"
        );
    }
    let (status, reason, body) = running.who_am_i(None).await;
    assert_eq!((status, reason.as_deref()), (401, Some("token_missing")));
    assert_eq!(body["code"], "unauthenticated", "{body}");
    let main: SocketAddr = server.local_addr();
    for (addr, path) in [
        (main, "/loams.instance.v1.InstanceService/WhoAmI"),
        (
            server.qdrant_grpc_addr().unwrap(),
            "/qdrant.Collections/List",
        ),
        (
            server.flight_sql_addr().unwrap(),
            "/arrow.flight.protocol.FlightService/ListActions",
        ),
    ] {
        assert_eq!(
            grpc_status(addr, path, None).await,
            ("16".into(), "token_missing".into()),
            "{path}"
        );
        let (status, reason) = grpc_status(addr, path, Some(&bearer)).await;
        assert_ne!(status, "16", "{path}");
        assert!(reason.is_empty(), "{path}: {reason}");
    }

    // With the key: admitted (each carrier the gateways' clients use).
    for url in [&rest, &qdrant_rest, &es] {
        assert_eq!(
            http_get(url, &[("authorization", &bearer)]).await.1,
            "",
            "{url}"
        );
    }
    assert_eq!(http_get(&qdrant_rest, &[("api-key", &key)]).await.1, "");
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("loams:{key}"))
    );
    assert_eq!(http_get(&es, &[("authorization", &basic)]).await.1, "");
    let (status, _, body) = running.who_am_i(Some(&bearer)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["principal"]["kind"], "PRINCIPAL_KIND_SERVICE_ACCOUNT",
        "{body}"
    );
    assert_eq!(body["principal"]["displayName"], "test", "{body}");

    // A wrong key, and a key in the URL.
    let mut wrong = key.clone();
    let last = wrong.pop().unwrap();
    wrong.push(if last == 'a' { 'b' } else { 'a' });
    let (status, reason, _) = running.who_am_i(Some(&format!("Bearer {wrong}"))).await;
    assert_eq!((status, reason.as_deref()), (401, Some("api_key_invalid")));
    let (status, reason) = http_get(
        &format!("{rest}?access_token={key}"),
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(
        (status, reason.as_str()),
        (401, "credential_in_query_string")
    );
    // An OIDC token when only API keys are on.
    let (status, reason, _) = running.who_am_i(Some("Bearer a.b.c")).await;
    assert_eq!(
        (status, reason.as_deref()),
        (401, Some("credential_not_accepted"))
    );

    // Probes and GetInstance stay public.
    assert_eq!(
        http_get(&format!("{}/health", running.base), &[]).await.0,
        200
    );
    let instance = reqwest::Client::new()
        .post(format!(
            "{}/loams.instance.v1.InstanceService/GetInstance",
            running.base
        ))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(instance.status(), 200);
    running.server.shutdown().await.unwrap();
}

/// Off mode is loopback-only (preview contract); with auth on, a public
/// address starts.
#[tokio::test]
async fn off_mode_on_a_public_address_refuses_to_start() {
    for listen in ["0.0.0.0:0", "[::]:0"] {
        let dir = TempDir::new().unwrap();
        let mut config = ServerConfig::new(dir.path());
        config.listen = listen.parse().unwrap();
        let err = Server::start(config)
            .await
            .expect_err("refused")
            .to_string();
        assert!(err.contains("loopback"), "{listen}: {err}");
    }
    let dir = TempDir::new().unwrap();
    let mut config = ServerConfig::new(dir.path());
    config.flight_sql = Some("0.0.0.0:0".parse().unwrap());
    let err = Server::start(config)
        .await
        .expect_err("refused")
        .to_string();
    assert!(err.contains("--flight-sql-listen"), "{err}");

    let keys = TempDir::new().unwrap();
    let (_, path) = keys_file(&keys);
    let dir = TempDir::new().unwrap();
    let mut config = ServerConfig::new(dir.path());
    config.listen = "0.0.0.0:0".parse().unwrap();
    config.auth = AuthConfig::api_keys(path);
    let server = Server::start(config).await.expect("auth on: starts");
    server.shutdown().await.unwrap();
}

/// A stand-in OIDC issuer: discovery and a JWKS with one Ed25519 key.
struct Issuer {
    url: String,
    signer: EncodingKey,
    other: EncodingKey,
    task: tokio::task::JoinHandle<()>,
}

fn ed25519() -> (EncodingKey, String) {
    let pkcs8 =
        Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).expect("test fixture");
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("test fixture");
    let x = URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
    (EncodingKey::from_ed_der(pkcs8.as_ref()), x)
}

impl Issuer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind(LOOPBACK0)
            .await
            .expect("test fixture");
        let url = format!(
            "http://{}/application/o/loams/",
            listener.local_addr().expect("test fixture")
        );
        let (signer, x) = ed25519();
        let (other, _) = ed25519();
        let jwks = json!({"keys": [{"kty": "OKP", "crv": "Ed25519", "x": x, "kid": "k1", "alg": "EdDSA", "use": "sig"}]});
        let discovery = json!({"issuer": url, "jwks_uri": format!("{url}jwks/")});
        let app = axum::Router::new()
            .route(
                "/application/o/loams/.well-known/openid-configuration",
                axum::routing::get(move || async move { axum::Json(discovery) }),
            )
            .route(
                "/application/o/loams/jwks/",
                axum::routing::get(move || async move { axum::Json(jwks) }),
            );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("test fixture");
        });
        Self {
            url,
            signer,
            other,
            task,
        }
    }

    fn token(&self, edit: impl FnOnce(&mut Value, &mut Header) -> bool) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test fixture")
            .as_secs();
        let mut claims = json!({
            "iss": self.url, "aud": AUDIENCE, "sub": "user-1", "exp": now + 300, "iat": now,
            "email": "me@example.com", "email_verified": true, "name": "Me",
        });
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("k1".into());
        let use_other = edit(&mut claims, &mut header);
        let key = if use_other { &self.other } else { &self.signer };
        jsonwebtoken::encode(&header, &claims, key).expect("test fixture")
    }
}

#[tokio::test]
async fn oidc_tokens_are_verified_against_the_issuer() {
    let issuer = Issuer::start().await;
    // A second trusted issuer (the user's own), configured without the
    // trailing slash its tokens carry.
    let own = Issuer::start().await;
    let keys = TempDir::new().unwrap();
    let (key, path) = keys_file(&keys);
    let auth = AuthConfig {
        mode: AuthMode::On {
            api_keys: true,
            oidc: true,
            tailscale: false,
        },
        api_keys_file: Some(path),
        oidc: Some(OidcConfig {
            issuers: vec![
                TrustedIssuer::new(issuer.url.clone()),
                TrustedIssuer::new(own.url.trim_end_matches('/')),
            ],
            audiences: vec![AUDIENCE.into(), "my-console".into()],
            allowed_emails: ["me@example.com".to_owned()].into(),
            ..OidcConfig::default()
        }),
        cors_origins: vec![CONSOLE.into()],
        ..AuthConfig::off()
    };
    let running = Running::start(auth, |_| {}).await;
    let call = |token: String| {
        let running = &running;
        async move { running.who_am_i(Some(&format!("Bearer {token}"))).await }
    };

    let (status, _, body) = call(issuer.token(|_, _| false)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["principal"]["kind"], "PRINCIPAL_KIND_USER", "{body}");
    assert_eq!(body["principal"]["email"], "me@example.com", "{body}");
    // The hosted console in `oidc` mode reads who it is from GetSession
    // with the same bearer (API2 ruling S6/7.13).
    let session: Value = reqwest::Client::new()
        .post(format!(
            "{}/loams.auth.v1.AuthService/GetSession",
            running.base
        ))
        .header("content-type", "application/json")
        .header("connect-protocol-version", "1")
        .header(
            "authorization",
            format!("Bearer {}", issuer.token(|_, _| false)),
        )
        .body("{}")
        .send()
        .await
        .expect("call")
        .json()
        .await
        .expect("json");
    assert_eq!(
        session["session"]["user"]["email"], "me@example.com",
        "{session}"
    );
    // The API key still works beside OIDC.
    assert_eq!(call(key).await.0, 200);
    // The second issuer, for the second audience; its key does not verify
    // the first issuer's tokens.
    let (status, _, body) = call(own.token(|c, _| {
        c["aud"] = json!("my-console");
        false
    }))
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, reason, _) = call(own.token(|c, _| {
        c["iss"] = json!(issuer.url);
        false
    }))
    .await;
    assert_eq!(
        (status, reason.as_deref()),
        (401, Some("token_bad_signature"))
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let cases: Vec<(&str, u16, String)> = vec![
        ("no token", 401, String::new()),
        ("bad signature", 401, issuer.token(|_, _| true)),
        (
            "expired",
            401,
            issuer.token(|c, _| {
                c["exp"] = json!(now - 600);
                false
            }),
        ),
        (
            "wrong audience",
            401,
            issuer.token(|c, _| {
                c["aud"] = json!("someone-else");
                false
            }),
        ),
        (
            "wrong issuer",
            401,
            issuer.token(|c, _| {
                c["iss"] = json!("https://evil.example/");
                false
            }),
        ),
        (
            "wrong key",
            401,
            issuer.token(|_, h| {
                h.kid = Some("k2".into());
                true
            }),
        ),
        (
            "not allowed",
            403,
            issuer.token(|c, _| {
                c["email"] = json!("stranger@example.com");
                false
            }),
        ),
        (
            "unverified email",
            403,
            issuer.token(|c, _| {
                c["email_verified"] = json!(false);
                false
            }),
        ),
    ];
    let expected = [
        "token_missing",
        "token_bad_signature",
        "token_expired",
        "token_wrong_audience",
        "token_wrong_issuer",
        "token_unknown_key",
        "principal_not_allowed",
        "principal_not_allowed",
    ];
    for ((name, status, token), reason) in cases.into_iter().zip(expected) {
        let (got, got_reason, body) = if token.is_empty() {
            running.who_am_i(None).await
        } else {
            call(token).await
        };
        assert_eq!(
            (got, got_reason.as_deref()),
            (status, Some(reason)),
            "{name}: {body}"
        );
    }
    // HS256 (a forged token keyed with public material) is refused outright.
    let forged = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &json!({"iss": issuer.url, "aud": AUDIENCE, "sub": "x", "exp": now + 60}),
        &EncodingKey::from_secret(b"public"),
    )
    .unwrap();
    let (status, reason, _) = call(forged).await;
    assert_eq!(
        (status, reason.as_deref()),
        (401, Some("token_algorithm_not_allowed"))
    );

    // CORS: the console's origin gets a preflight answer and headers on a
    // refusal; another origin does not.
    let preflight = |origin: &'static str| {
        reqwest::Client::new()
            .request(
                reqwest::Method::OPTIONS,
                format!("{}/loams.instance.v1.InstanceService/WhoAmI", running.base),
            )
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization, content-type",
            )
            .send()
    };
    let ok = preflight(CONSOLE).await.unwrap();
    assert_eq!(ok.status(), 204);
    assert_eq!(ok.headers()["access-control-allow-origin"], CONSOLE);
    assert!(
        ok.headers()["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("authorization")
    );
    let refused = preflight("https://evil.example").await.unwrap();
    assert_eq!(refused.status(), 403);
    assert!(
        !refused
            .headers()
            .contains_key("access-control-allow-origin")
    );
    let refusal = reqwest::Client::new()
        .post(format!(
            "{}/loams.instance.v1.InstanceService/WhoAmI",
            running.base
        ))
        .header("origin", CONSOLE)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(refusal.status(), 401);
    assert_eq!(refusal.headers()["access-control-allow-origin"], CONSOLE);

    running.server.shutdown().await.unwrap();
    issuer.task.abort();
    own.task.abort();
}

/// CORS is deny by default (owner, 2026-10-23: the console ships with the
/// deployment): an authenticated engine with no `LOAMS_CORS_ORIGINS` answers
/// no other origin, not even the retired hosted console's; its own origin is
/// same-origin and still gets an answer.
#[tokio::test]
async fn cors_allows_only_the_engines_own_origin_by_default() {
    let keys = TempDir::new().unwrap();
    let (key, path) = keys_file(&keys);
    let running = Running::start(AuthConfig::api_keys(&path), |_| {}).await;
    let url = format!("{}/loams.instance.v1.InstanceService/WhoAmI", running.base);
    let preflight = |origin: String| {
        reqwest::Client::new()
            .request(reqwest::Method::OPTIONS, &url)
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization, content-type",
            )
            .send()
    };
    for foreign in [
        "https://console.loams.dev",
        "https://evil.example",
        "loams-app://console",
    ] {
        let refused = preflight(foreign.into()).await.unwrap();
        assert_eq!(refused.status(), 403, "{foreign}");
        assert!(
            !refused
                .headers()
                .contains_key("access-control-allow-origin"),
            "{foreign}"
        );
        let call = reqwest::Client::new()
            .post(&url)
            .header("origin", foreign)
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert!(
            !call.headers().contains_key("access-control-allow-origin"),
            "{foreign}"
        );
    }
    let own = running.base.clone();
    let ok = preflight(own.clone()).await.unwrap();
    assert_eq!(ok.status(), 204);
    assert_eq!(ok.headers()["access-control-allow-origin"], own.as_str());
    running.server.shutdown().await.unwrap();
}

/// The console ships with the deployment: the engine serves it at `/ui/`
/// on the main port, with no credential (its files are public), while its
/// API calls stay behind authentication.
#[tokio::test]
async fn the_engine_serves_its_console_beside_the_guarded_api() {
    let keys = TempDir::new().unwrap();
    let (_, path) = keys_file(&keys);
    let console = TempDir::new().unwrap();
    std::fs::create_dir_all(console.path().join("assets")).unwrap();
    std::fs::write(
        console.path().join("index.html"),
        "<!doctype html><meta charset=\"utf-8\" /><meta http-equiv=\"Content-Security-Policy\" \
         content=\"default-src 'self'; connect-src 'self'\" /><title>Loams console</title>",
    )
    .unwrap();
    std::fs::write(console.path().join("config.json"), "{}").unwrap();
    std::fs::write(console.path().join("assets/app.js"), "1").unwrap();
    let dir = console.path().to_path_buf();
    let running = Running::start(AuthConfig::api_keys(&path), move |config| {
        config.console_dir = Some(dir);
        config.console = loams::console::ConsoleOptions {
            agentd_origins: vec!["https://agentd.example.ts.net".into()],
        };
    })
    .await;
    let get = |path: &'static str| {
        let url = format!("{}{path}", running.base);
        async move {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(url)
                .send()
                .await
                .unwrap()
        }
    };
    let index = get("/ui/").await;
    assert_eq!(index.status(), 200);
    // The CSP is a header, with the agentd host in connect-src and frame-src
    // (#1201: Studio previews frame it); the page carries no meta CSP that
    // would keep it out.
    assert_eq!(
        index.headers()["content-security-policy"],
        "default-src 'self'; connect-src 'self' https://agentd.example.ts.net; \
         frame-src 'self' https://agentd.example.ts.net; frame-ancestors 'self'"
    );
    let html = index.text().await.unwrap();
    assert!(html.contains("Loams console"), "{html}");
    assert!(!html.contains("Content-Security-Policy"), "{html}");
    assert_eq!(get("/ui/config.json").await.text().await.unwrap(), "{}");
    assert_eq!(get("/ui/assets/app.js").await.status(), 200);
    assert_eq!(get("/ui/projects/p").await.status(), 200);
    let root = get("/").await;
    assert_eq!(root.status(), 307);
    assert_eq!(root.headers()["location"], "/ui/");
    // The API is still guarded.
    let (status, reason, _) = running.who_am_i(None).await;
    assert_eq!((status, reason.as_deref()), (401, Some("token_missing")));
    running.server.shutdown().await.unwrap();

    // Without a build there is no /ui/: the API's own 404 (behind auth).
    let running = Running::start(AuthConfig::api_keys(&path), |_| {}).await;
    let status = reqwest::get(format!("{}/ui/", running.base))
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);
    running.server.shutdown().await.unwrap();
}

/// An OIDC allowlist is required: a shared issuer signs tokens for everyone.
#[tokio::test]
async fn oidc_without_an_allowlist_refuses_to_start() {
    let dir = TempDir::new().unwrap();
    let mut config = ServerConfig::new(dir.path());
    config.listen = LOOPBACK0;
    config.auth = AuthConfig {
        mode: AuthMode::On {
            api_keys: false,
            oidc: true,
            tailscale: false,
        },
        oidc: Some(OidcConfig {
            issuers: vec![TrustedIssuer::new("https://auth.loams.dev")],
            audiences: vec![AUDIENCE.into()],
            ..OidcConfig::default()
        }),
        ..AuthConfig::off()
    };
    let err = Server::start(config)
        .await
        .expect_err("refused")
        .to_string();
    assert!(err.contains("allowlist"), "{err}");
}

/// The console's tailnet identity check (API2 ruling 10.4): a
/// credential-less Connect GET of `AuthService/GetSession` from the console
/// the engine serves (its own origin), on the `tailscale serve` target, names
/// the tailnet user; the main port, which strips the identity headers,
/// answers 401.
#[tokio::test]
async fn tailscale_get_session_names_the_tailnet_user() {
    let auth = AuthConfig {
        mode: AuthMode::On {
            api_keys: false,
            oidc: false,
            tailscale: true,
        },
        tailscale: Some(TailscaleConfig {
            listen: LOOPBACK0,
            allowed_users: ["me@example.com".to_owned()].into(),
        }),
        ..AuthConfig::off()
    };
    let running = Running::start(auth, |_| {}).await;
    let target = running.server.tailscale_addr().expect("tailscale target");
    let get_session = |addr: SocketAddr| async move {
        reqwest::Client::new()
            .get(format!(
                "http://{addr}/loams.auth.v1.AuthService/GetSession\
                 ?connect=v1&encoding=json&message=%7B%7D"
            ))
            .header("accept", "application/json")
            .header("origin", format!("http://{addr}"))
            .header("tailscale-user-login", "me@example.com")
            .header("tailscale-user-name", "Me")
            .send()
            .await
            .expect("call")
    };
    let response = get_session(target).await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        format!("http://{target}").as_str()
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["session"]["user"]["email"], "me@example.com", "{body}");
    assert_eq!(body["session"]["user"]["name"], "Me", "{body}");
    assert!(body["session"].get("csrfToken").is_none(), "{body}");
    let main = get_session(running.server.local_addr()).await;
    assert_eq!(main.status().as_u16(), 401);
    running.server.shutdown().await.unwrap();
}

/// Tailscale mode (owner ruling 2026-10-12): `Tailscale-User-Login` names
/// the caller on the `tailscale serve` target only; on the main port it is
/// stripped, so a forged login is no credential at all.
#[tokio::test]
async fn tailscale_identities_count_only_on_the_serve_target() {
    let auth = AuthConfig {
        mode: AuthMode::On {
            api_keys: false,
            oidc: false,
            tailscale: true,
        },
        tailscale: Some(TailscaleConfig {
            listen: LOOPBACK0,
            allowed_users: ["me@example.com".to_owned()].into(),
        }),
        ..AuthConfig::off()
    };
    let running = Running::start(auth, |_| {}).await;
    let target = running.server.tailscale_addr().expect("tailscale target");
    assert_ne!(target, running.server.local_addr());
    let who_am_i = |addr: SocketAddr, login: Option<&'static str>| async move {
        let mut request = reqwest::Client::new()
            .post(format!(
                "http://{addr}/loams.instance.v1.InstanceService/WhoAmI"
            ))
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .body("{}");
        if let Some(login) = login {
            request = request
                .header("tailscale-user-login", login)
                .header("tailscale-user-name", "Me");
        }
        let response = request.send().await.expect("call");
        let status = response.status().as_u16();
        let reason = response
            .headers()
            .get("loams-auth-reason")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        (status, reason, body)
    };
    let main = running.server.local_addr();

    // The main port ignores the headers: 401, as if none were sent.
    let (status, reason, _) = who_am_i(main, Some("me@example.com")).await;
    assert_eq!((status, reason.as_str()), (401, "token_missing"));
    // The serve target maps the login to a principal.
    let (status, _, body) = who_am_i(target, Some("Me@Example.com")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["principal"]["id"], "tailscale:me@example.com",
        "{body}"
    );
    assert_eq!(body["principal"]["kind"], "PRINCIPAL_KIND_USER", "{body}");
    assert_eq!(body["principal"]["displayName"], "Me", "{body}");
    // The allowlist holds there; no login (a tagged node) is no credential.
    let (status, reason, _) = who_am_i(target, Some("you@example.com")).await;
    assert_eq!((status, reason.as_str()), (403, "principal_not_allowed"));
    let (status, reason, _) = who_am_i(target, None).await;
    assert_eq!((status, reason.as_str()), (401, "token_missing"));
    // The identity there is ambient: a foreign page gets no CORS and is
    // refused, the retired hosted console included (no origin is allowed by
    // default); the console the engine serves itself (its own origin) gets
    // both.
    let from = |origin: String| async move {
        reqwest::Client::new()
            .post(format!(
                "http://{target}/loams.instance.v1.InstanceService/WhoAmI"
            ))
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .header("origin", origin)
            .header("tailscale-user-login", "me@example.com")
            .body("{}")
            .send()
            .await
            .expect("call")
    };
    let foreign = from("https://evil.example".into()).await;
    assert_eq!(foreign.status().as_u16(), 403);
    assert!(
        foreign
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    let preflight = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{target}/loams.instance.v1.InstanceService/WhoAmI"),
        )
        .header("origin", "https://evil.example")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert!(
        preflight
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    let hosted = from("https://console.loams.dev".into()).await;
    assert_eq!(hosted.status().as_u16(), 403);
    assert!(
        hosted
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    let own = format!("http://{target}");
    let console = from(own.clone()).await;
    assert_eq!(console.status().as_u16(), 200);
    assert_eq!(
        console.headers()["access-control-allow-origin"],
        own.as_str()
    );
    running.server.shutdown().await.unwrap();

    // The serve target must be loopback.
    let dir = TempDir::new().unwrap();
    let mut config = ServerConfig::new(dir.path());
    config.listen = LOOPBACK0;
    config.auth = AuthConfig {
        mode: AuthMode::On {
            api_keys: false,
            oidc: false,
            tailscale: true,
        },
        tailscale: Some(TailscaleConfig {
            listen: "0.0.0.0:0".parse().unwrap(),
            ..TailscaleConfig::default()
        }),
        ..AuthConfig::off()
    };
    let err = Server::start(config)
        .await
        .expect_err("refused")
        .to_string();
    assert!(err.contains("LOAMS_TAILSCALE_LISTEN"), "{err}");
}
