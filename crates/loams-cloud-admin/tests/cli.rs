//! `loams cloud admin` against a local double of WorkOS and the control plane.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use clap::Parser;
use loams_cloud_admin::auth::{MemoryStore, TokenStore};
use loams_cloud_admin::{
    AdminArgs, CloudArgs, CloudCommand, Env, Error, InviteArgs, admin, batch_ids, install_crypto,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Seen {
    polls: u32,
    authenticate: Vec<Vec<(String, String)>>,
    calls: Vec<(String, Option<String>, Option<String>, Value)>,
}

type Shared = Arc<Mutex<Seen>>;

async fn device(Form(f): Form<Vec<(String, String)>>) -> impl IntoResponse {
    assert!(f.contains(&("client_id".into(), "client_TEST".into())));
    Json(
        json!({ "device_code": "dc_1", "user_code": "ABCD-EFGH", "verification_uri": "https://auth.test/device", "verification_uri_complete": "https://auth.test/device?code=ABCD-EFGH", "expires_in": 300, "interval": 1 }),
    )
}

async fn authenticate(
    State(s): State<Shared>,
    Form(f): Form<Vec<(String, String)>>,
) -> axum::response::Response {
    let mut seen = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    seen.authenticate.push(f.clone());
    let grant = f
        .iter()
        .find(|(k, _)| k == "grant_type")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    if grant == "urn:ietf:params:oauth:grant-type:device_code" {
        seen.polls += 1;
        if seen.polls == 1 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "authorization_pending" })),
            )
                .into_response();
        }
        return Json(json!({ "access_token": "at_device", "refresh_token": "rt_1", "user": { "id": "user_01ADMIN" } })).into_response();
    }
    let refresh = f
        .iter()
        .find(|(k, _)| k == "refresh_token")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    if refresh == "rt_revoked" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    let n = seen.authenticate.len();
    Json(json!({ "access_token": format!("at_org_{n}"), "refresh_token": format!("rt_{n}") }))
        .into_response()
}

fn record(s: &Shared, what: String, h: &HeaderMap, body: Value) {
    let header = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
    s.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .calls
        .push((
            what,
            header("authorization"),
            header("idempotency-key"),
            body,
        ));
}

async fn server() -> (String, Shared) {
    let s: Shared = Arc::default();
    let app = Router::new()
        .route("/user_management/authorize/device", post(device))
        .route("/user_management/authenticate", post(authenticate))
        .route(
            "/cloud/v1/admin/invites",
            post(|State(s): State<Shared>, h: HeaderMap, Json(b): Json<Value>| async move {
                record(&s, "POST invites".into(), &h, b.clone());
                let ids = b["waitlist_ids"].as_array().cloned().unwrap_or_default();
                (StatusCode::ACCEPTED, Json(json!({ "invites": ids.iter().map(|w| json!({ "id": "inv_x", "waitlist_id": w, "state": "queued" })).collect::<Vec<_>>(), "skipped": [], "request_id": "req_1" })))
            })
            .get(|State(s): State<Shared>, h: HeaderMap| async move {
                record(&s, "GET invites".into(), &h, Value::Null);
                Json(json!({ "items": [], "next_cursor": null }))
            }),
        )
        .route(
            "/cloud/v1/admin/invites/{id}/resend",
            post(|State(s): State<Shared>, Path(id): Path<String>, h: HeaderMap, Json(b): Json<Value>| async move {
                record(&s, format!("resend {id}"), &h, b);
                (StatusCode::CONFLICT, Json(json!({ "error": { "code": "RESEND_LIMIT", "message": "an invite is resent at most 3 times", "request_id": "req_9" } })))
            }),
        )
        .route(
            "/cloud/v1/admin/waitlist",
            get(|State(s): State<Shared>, h: HeaderMap, q: axum::extract::RawQuery| async move {
                record(&s, format!("GET waitlist?{}", q.0.unwrap_or_default()), &h, Value::Null);
                (StatusCode::FORBIDDEN, Json(json!({ "error": { "code": "STEP_UP_REQUIRED", "message": "verify a TOTP code first", "request_id": "req_2" } })))
            }),
        )
        .route(
            "/cloud/v1/admin/waitlist/{id}",
            delete(|State(s): State<Shared>, Path(id): Path<String>, h: HeaderMap| async move {
                record(&s, format!("DELETE {id}"), &h, Value::Null);
                StatusCode::NO_CONTENT
            }),
        )
        .route(
            "/cloud/v1/admin/mfa/verify",
            post(|State(s): State<Shared>, h: HeaderMap, Json(b): Json<Value>| async move {
                record(&s, "verify".into(), &h, b);
                Json(json!({ "step_up_until": "2026-11-01T12:15:00.000Z", "request_id": "req_3" }))
            }),
        )
        .with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (format!("http://{addr}"), s)
}

#[derive(Parser)]
struct Top {
    #[command(subcommand)]
    cmd: Cmd,
}
#[derive(clap::Subcommand)]
enum Cmd {
    Cloud(CloudArgs),
}

fn parse(base: &str, argv: &[&str]) -> AdminArgs {
    let mut full = vec![
        "loams",
        "cloud",
        "admin",
        "--api",
        "",
        "--workos-api",
        base,
        "--workos-client-id",
        "client_TEST",
        "--ostrium-org",
        "org_OSTRIUM",
    ];
    let api = format!("{base}/cloud/v1");
    full[4] = &api;
    full.extend_from_slice(argv);
    let Cmd::Cloud(c) = Top::try_parse_from(full).expect("parses").cmd;
    let CloudCommand::Admin(a) = c.command;
    a
}

struct Out {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn go(
    base: &str,
    store: &MemoryStore,
    argv: &[&str],
    stdin: &str,
) -> (Result<(), Error>, Out) {
    install_crypto();
    let args = parse(base, argv);
    let mut input = Cursor::new(stdin.as_bytes().to_vec());
    let mut out = Out {
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let res = {
        let mut env = Env {
            http: reqwest::Client::new(),
            store,
            stdin: &mut input,
            stdout: &mut out.stdout,
            stderr: &mut out.stderr,
        };
        admin(&args, &mut env).await
    };
    (res, out)
}

#[tokio::test]
async fn login_then_invite_refreshes_into_the_org_and_sends_one_key() {
    let (base, seen) = server().await;
    let store = MemoryStore::default();
    assert!(matches!(
        go(&base, &store, &["waitlist", "list"], "").await.0,
        Err(Error::NotSignedIn)
    ));

    let (res, out) = go(&base, &store, &["login"], "").await;
    res.expect("login");
    assert!(String::from_utf8_lossy(&out.stderr).contains("ABCD-EFGH"));
    assert_eq!(
        store.load("workos:client_TEST").expect("load").as_deref(),
        Some("rt_1")
    );
    assert_eq!(
        seen.lock().expect("lock").polls,
        2,
        "one pending poll, then the token"
    );

    let ids = ["wl_AAAAAAAAAAAAAAAAAAAAAA", "wl_BBBBBBBBBBBBBBBBBBBBBB"];
    let (res, out) = go(
        &base,
        &store,
        &["invite", ids[0], ids[1], ids[0], "--expires-in-days", "7"],
        "",
    )
    .await;
    res.expect("invite");
    let printed: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(printed["invites"].as_array().map(Vec::len), Some(2));

    let s = seen.lock().expect("lock");
    let refresh = s.authenticate.last().expect("a refresh");
    assert!(refresh.contains(&("grant_type".into(), "refresh_token".into())));
    assert!(refresh.contains(&("refresh_token".into(), "rt_1".into())));
    assert!(refresh.contains(&("organization_id".into(), "org_OSTRIUM".into())));
    let (what, auth, key, body) = s.calls.last().expect("a call");
    assert_eq!(what, "POST invites");
    assert_eq!(
        auth.as_deref(),
        Some(format!("Bearer at_org_{}", s.authenticate.len()).as_str())
    );
    assert!(
        key.as_deref()
            .is_some_and(|k| k.starts_with("cli-") && k.len() <= 64)
    );
    assert_eq!(
        body,
        &json!({ "waitlist_ids": ids, "expires_in_days": 7 }),
        "de-duplicated, in order"
    );
    drop(s);
    // The refresh token rotated, and the new one was kept.
    assert_ne!(
        store.load("workos:client_TEST").expect("load").as_deref(),
        Some("rt_1")
    );
}

#[tokio::test]
async fn api_errors_carry_the_code_and_request_id() {
    let (base, seen) = server().await;
    let store = MemoryStore::default();
    store.save("workos:client_TEST", "rt_x").expect("save");
    let (res, _) = go(
        &base,
        &store,
        &["waitlist", "list", "--status", "confirmed", "--limit", "10"],
        "",
    )
    .await;
    let e = res.expect_err("step-up");
    assert_eq!(
        e.to_string(),
        "STEP_UP_REQUIRED: verify a TOTP code first (request req_2)"
    );
    assert!(
        seen.lock()
            .expect("lock")
            .calls
            .last()
            .expect("call")
            .0
            .contains("status=confirmed&limit=10")
    );
    let (res, _) = go(
        &base,
        &store,
        &["invite", "resend", "inv_AAAAAAAAAAAAAAAAAAAAAA", "--extend"],
        "",
    )
    .await;
    assert!(matches!(res, Err(Error::Api { status: 409, ref code, .. }) if code == "RESEND_LIMIT"));
    assert_eq!(
        seen.lock().expect("lock").calls.last().expect("call").3,
        json!({ "extend": true })
    );
}

#[tokio::test]
async fn mfa_verify_reads_the_code_from_stdin() {
    let (base, seen) = server().await;
    let store = MemoryStore::default();
    store.save("workos:client_TEST", "rt_x").expect("save");
    let (res, out) = go(&base, &store, &["mfa", "verify"], "246810\n").await;
    res.expect("verify");
    assert!(String::from_utf8_lossy(&out.stdout).contains("step_up_until"));
    assert_eq!(
        seen.lock().expect("lock").calls.last().expect("call").3,
        json!({ "code": "246810" })
    );
}

#[tokio::test]
async fn a_revoked_session_asks_for_login_and_logout_forgets() {
    let (base, _) = server().await;
    let store = MemoryStore::default();
    store
        .save("workos:client_TEST", "rt_revoked")
        .expect("save");
    let (res, _) = go(&base, &store, &["audit"], "").await;
    assert!(
        res.expect_err("revoked")
            .to_string()
            .contains("loams cloud admin login")
    );
    go(&base, &store, &["logout"], "").await.0.expect("logout");
    assert_eq!(store.load("workos:client_TEST").expect("load"), None);
}

#[tokio::test]
async fn waitlist_delete_checks_the_id_first() {
    let (base, seen) = server().await;
    let store = MemoryStore::default();
    store.save("workos:client_TEST", "rt_x").expect("save");
    assert!(matches!(
        go(
            &base,
            &store,
            &["waitlist", "delete", "someone@example.com"],
            ""
        )
        .await
        .0,
        Err(Error::Usage(_))
    ));
    go(
        &base,
        &store,
        &["waitlist", "delete", "wl_AAAAAAAAAAAAAAAAAAAAAA"],
        "",
    )
    .await
    .0
    .expect("delete");
    assert_eq!(
        seen.lock().expect("lock").calls.last().expect("call").0,
        "DELETE wl_AAAAAAAAAAAAAAAAAAAAAA"
    );
}

#[test]
fn batch_ids_reads_the_list_file_and_checks_the_ids() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("loams-cloud-admin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("ids.txt");
    std::fs::write(
        &file,
        "# first batch\nwl_AAAAAAAAAAAAAAAAAAAAAA\n\nwl_CCCCCCCCCCCCCCCCCCCCCC # a note\n",
    )
    .expect("write");
    let args = InviteArgs {
        command: None,
        waitlist_ids: vec![
            "wl_BBBBBBBBBBBBBBBBBBBBBB".into(),
            "wl_AAAAAAAAAAAAAAAAAAAAAA".into(),
        ],
        from_list: Some(file),
        expires_in_days: None,
    };
    assert_eq!(
        batch_ids(&args).expect("ids"),
        [
            "wl_BBBBBBBBBBBBBBBBBBBBBB",
            "wl_AAAAAAAAAAAAAAAAAAAAAA",
            "wl_CCCCCCCCCCCCCCCCCCCCCC"
        ]
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
    let bad = InviteArgs {
        command: None,
        waitlist_ids: vec!["inv_AAAAAAAAAAAAAAAAAAAAAA".into()],
        from_list: None,
        expires_in_days: None,
    };
    assert!(matches!(batch_ids(&bad), Err(Error::Usage(_))));
    let many = InviteArgs {
        command: None,
        waitlist_ids: (0..51).map(|i| format!("wl_{i:A>22}")).collect(),
        from_list: None,
        expires_in_days: None,
    };
    assert!(
        batch_ids(&many)
            .expect_err("too many")
            .to_string()
            .contains("at most 50")
    );
    let none = InviteArgs {
        command: None,
        waitlist_ids: vec![],
        from_list: None,
        expires_in_days: None,
    };
    assert!(batch_ids(&none).is_err());
}

#[test]
fn invite_subcommands_and_ids_parse_apart() {
    let a = parse(
        "http://x",
        &["invite", "revoke", "inv_AAAAAAAAAAAAAAAAAAAAAA"],
    );
    assert!(matches!(
        a.command,
        loams_cloud_admin::AdminCommand::Invite(InviteArgs {
            command: Some(loams_cloud_admin::InviteCommand::Revoke { .. }),
            ..
        })
    ));
    let b = parse("http://x", &["invite", "wl_AAAAAAAAAAAAAAAAAAAAAA"]);
    assert!(
        matches!(b.command, loams_cloud_admin::AdminCommand::Invite(InviteArgs { command: None, ref waitlist_ids, .. }) if waitlist_ids.len() == 1)
    );
}
