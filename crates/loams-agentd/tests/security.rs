//! RPC security (design §50 §7, plan DD1 Task 7): the upgrade checks, `Hello`
//! first, roles and scoped run tokens, the limits, and the token canaries.
//!
//! The upgrade tests speak raw HTTP to a real `loams-agentd run`, so they see
//! exactly what a browser, another local user or a stray client would.
#![allow(clippy::unwrap_used)]

mod support;

use std::io::{BufRead, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use loams_agentd::hello::DaemonInfo;
use loams_agentd_harness::{Harness, HarnessError, RunControls};
use loams_agentd_proto::daemon::DaemonMode;
use loams_agentd_proto::rpc::{
    AGENTD_PROTOCOL, ENV_PORT, ENV_RUN_TOKEN, ENV_SESSION_ID, FORBIDDEN_ROLE, UNAUTHENTICATED,
};
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use loams_agentd_rpc::{RpcClient, ScopedTokens, Secret, connect_ws_with_token, methods};
use loams_agentd_sessions::{EngineCore, HarnessRegistry};
use support::{Daemon, Home, block_on, hello_params};

const CHILD: &str = "[daemon]\nmode = \"child\"\n";
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// One HTTP response, as read off the socket.
#[derive(Debug)]
struct Response {
    /// 0 when the server closed the connection without answering.
    status: u16,
    head: String,
    body: Vec<u8>,
}

/// Send `request` and read the response head (and any body the server sends
/// before closing). A 101 is returned as soon as its head arrives.
fn exchange(port: u16, request: &str) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            // A signal is not the server's answer. Once anything in this
            // process handles SIGCHLD (tokio's reaper does), another test's
            // daemon exiting interrupts this recv: with a read timeout set,
            // Linux fails it with EINTR even under SA_RESTART.
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                panic!("no answer within the read timeout; read so far: {bytes:?}")
            }
            // Reset: the server closed without answering.
            Err(_) => break,
        }
        if let Some(end) = find(&bytes, b"\r\n\r\n")
            && bytes.starts_with(b"HTTP/1.1 101")
        {
            bytes.truncate(end + 4);
            break;
        }
    }
    let Some(end) = find(&bytes, b"\r\n\r\n") else {
        return Response {
            status: 0,
            head: String::from_utf8_lossy(&bytes).into_owned(),
            body: Vec::new(),
        };
    };
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    Response {
        status,
        head,
        body: bytes[end + 4..].to_vec(),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A WebSocket upgrade request to `127.0.0.1:<port>` with `extra` headers.
fn upgrade(port: u16, extra: &[(&str, &str)]) -> String {
    upgrade_with_host(port, &format!("127.0.0.1:{port}"), extra)
}

fn upgrade_with_host(_port: u16, host: &str, extra: &[(&str, &str)]) -> String {
    let mut request = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {KEY}\r\n"
    );
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request
}

fn token(home: &Home) -> String {
    std::fs::read_to_string(home.file("agentd.token")).unwrap()
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn assert_refused(response: &Response, status: u16) {
    assert_eq!(response.status, status, "{response:?}");
    assert!(
        response.body.is_empty(),
        "a refusal has no body: {response:?}"
    );
    assert!(
        !response
            .head
            .to_ascii_lowercase()
            .contains("access-control-"),
        "a refusal sends no CORS header: {response:?}"
    );
}

#[test]
fn upgrade_without_token_is_401() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    assert_refused(&exchange(port, &upgrade(port, &[])), 401);
    // The right token is accepted, so the 401 is about the token alone.
    let ok = exchange(
        port,
        &upgrade(port, &[("Authorization", &bearer(&token(&home)))]),
    );
    assert_eq!(ok.status, 101, "{ok:?}");
}

#[test]
fn wrong_token_is_401() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let real = token(&home);
    let mut flipped = real.clone().into_bytes();
    flipped[0] = if flipped[0] == b'A' { b'B' } else { b'A' };
    let flipped = String::from_utf8(flipped).unwrap();
    let wrong = [
        bearer(&flipped),
        // Wrong lengths: empty, short, a prefix, one character too many, huge.
        "Bearer ".to_string(),
        "Bearer".to_string(),
        bearer("x"),
        bearer(&real[..real.len() - 1]),
        bearer(&format!("{real}A")),
        bearer(&"A".repeat(4096)),
        // Another scheme, or the token without one.
        format!("Basic {real}"),
        real.clone(),
    ];
    for value in &wrong {
        let response = exchange(port, &upgrade(port, &[("Authorization", value)]));
        assert_refused(&response, 401);
    }
    // Two Authorization headers are refused even when one is right.
    let twice = exchange(
        port,
        &upgrade(
            port,
            &[
                ("Authorization", &bearer(&real)),
                ("Authorization", &bearer(&flipped)),
            ],
        ),
    );
    assert_refused(&twice, 401);
}

#[test]
fn failed_upgrades_are_slowed() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let started = Instant::now();
    for _ in 0..4 {
        let response = exchange(port, &upgrade(port, &[("Authorization", "Bearer nope")]));
        assert_eq!(response.status, 401);
    }
    assert!(
        started.elapsed() >= Duration::from_millis(4 * 250),
        "four failures took {:?}",
        started.elapsed()
    );
}

/// The raw-HTTP helper reads through signals: a SIGCHLD landing on the
/// reading thread mid-recv once read as "closed without answering" (status 0),
/// the CI flake in `failed_upgrades_are_slowed`. Here a handler is installed
/// and the signal aimed at the reader on every read.
#[cfg(unix)]
#[test]
fn exchange_reads_through_signals() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _handler = runtime.block_on(async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()).unwrap()
    });
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // SAFETY: pthread_self has no preconditions.
    #[allow(unsafe_code)]
    let reader = unsafe { libc::pthread_self() } as usize;
    let signaller = std::thread::spawn({
        let done = done.clone();
        move || {
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                // SAFETY: the reader thread outlives this loop: it joins us
                // before it returns.
                #[allow(unsafe_code)]
                unsafe {
                    libc::pthread_kill(reader as libc::pthread_t, libc::SIGCHLD);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let statuses: Vec<u16> = (0..2)
        .map(|_| exchange(port, &upgrade(port, &[("Authorization", "Bearer nope")])).status)
        .collect();
    done.store(true, std::sync::atomic::Ordering::Release);
    signaller.join().unwrap();
    assert_eq!(statuses, vec![401, 401]);
}

#[test]
fn origin_header_is_403() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let auth = bearer(&token(&home));
    for origin in ["https://evil.example", "null", "http://127.0.0.1"] {
        let response = exchange(
            port,
            &upgrade(port, &[("Authorization", &auth), ("Origin", origin)]),
        );
        assert_refused(&response, 403);
    }
}

#[test]
fn foreign_host_is_403() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let auth = bearer(&token(&home));
    for host in [
        format!("evil.example:{port}"),
        format!("localhost:{port}"),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "127.0.0.1".to_string(),
        format!("[::1]:{port}"),
    ] {
        let response = exchange(
            port,
            &upgrade_with_host(port, &host, &[("Authorization", &auth)]),
        );
        assert_refused(&response, 403);
    }
    // No Host at all.
    let request = format!(
        "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: {KEY}\r\nAuthorization: {auth}\r\n\r\n"
    );
    assert_refused(&exchange(port, &request), 403);
}

#[test]
fn options_is_403_without_cors_headers() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let preflight = format!(
        "OPTIONS / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\n\
         Access-Control-Request-Method: GET\r\nAccess-Control-Request-Headers: authorization\r\n\r\n"
    );
    assert_refused(&exchange(port, &preflight), 403);
    // Without Origin too, and with the right token.
    let auth = bearer(&token(&home));
    let options =
        format!("OPTIONS / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: {auth}\r\n\r\n");
    assert_refused(&exchange(port, &options), 403);
}

#[test]
fn non_upgrade_requests_are_403() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let auth = bearer(&token(&home));
    let requests = [
        format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: {auth}\r\n\r\n"),
        format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: {auth}\r\n\
             Content-Length: 2\r\n\r\n{{}}"
        ),
        // An upgrade to something else, and a WebSocket version we do not speak.
        format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: h2c\r\nConnection: Upgrade\r\n\
             Authorization: {auth}\r\n\r\n"
        ),
        upgrade(port, &[("Authorization", &auth)])
            .replace("Sec-WebSocket-Version: 13", "Sec-WebSocket-Version: 8"),
        // Not HTTP at all.
        "\u{16}\u{3}\u{1}\u{0}\u{a5}\u{1}\u{0}\u{0}\u{a1}\u{3}\u{3}\r\n\r\n".to_string(),
    ];
    for request in &requests {
        assert_refused(&exchange(port, request), 403);
    }
}

#[test]
fn a_method_before_hello_is_refused() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let url = daemon.url();
    let token = daemon.token.clone();
    block_on(async move {
        let client = connect_ws_with_token(&url, &token).await.unwrap();
        let err = client
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .expect_err("a call before Hello");
        assert!(err.to_string().starts_with(UNAUTHENTICATED), "{err}");
        let next = tokio::time::timeout(
            Duration::from_secs(5),
            client.call(methods::HELLO, hello_params(AGENTD_PROTOCOL)),
        )
        .await
        .expect("the closed connection answers at once");
        assert!(next.is_err(), "the connection stayed open: {next:?}");
    });
    // Hello first, then the same call works.
    block_on(async {
        let client = daemon.owner().await.unwrap();
        client
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap();
    });
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
fn frame_limit_enforced() {
    const MIB: usize = 1024 * 1024;
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    block_on(async {
        let client = daemon.owner().await.unwrap();
        // A large frame under the limit is served.
        let pad = "x".repeat(MIB);
        client
            .call(methods::LOCAL_DEVICE, serde_json::json!({ "pad": pad }))
            .await
            .expect("a 1 MiB frame is fine");
        // One over 16 MiB ends the connection.
        let pad = "x".repeat(16 * MIB);
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            client.call(methods::LOCAL_DEVICE, serde_json::json!({ "pad": pad })),
        )
        .await
        .expect("an oversized frame ends the connection");
        assert!(result.is_err(), "{result:?}");
        // The daemon still serves others.
        daemon.owner().await.expect("a new connection");
    });
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
fn token_never_logged() {
    let home = Home::new(CHILD);
    let daemon = Daemon::start(&home, &["--child"]);
    let port = daemon.discovery.port;
    let real = token(&home);
    let near_miss = format!("{}x", &real[..real.len() - 1]);
    let other = "A".repeat(43);
    for value in [bearer(&near_miss), bearer(&other), format!("Basic {real}")] {
        assert_eq!(
            exchange(port, &upgrade(port, &[("Authorization", &value)])).status,
            401
        );
    }
    // Refused for other reasons while carrying the real token.
    let origin = upgrade(
        port,
        &[("Authorization", &bearer(&real)), ("Origin", "null")],
    );
    assert_eq!(exchange(port, &origin).status, 403);
    // A good connection: its replies do not carry the token either.
    let replies = block_on(async {
        let client = daemon.owner().await.unwrap();
        let hello = client
            .call(methods::HELLO, hello_params(AGENTD_PROTOCOL))
            .await
            .unwrap();
        let info = client
            .call(methods::ENGINE_INFO, serde_json::json!({}))
            .await
            .unwrap();
        let err = client
            .call("NoSuchMethod", serde_json::json!({ "x": 1 }))
            .await
            .unwrap_err();
        format!("{hello} {info} {err}")
    });
    // The CLI's own client (the `status` probe) at TRACE: tungstenite logs
    // the upgrade request it sends, which carries the token.
    let status = home
        .command(&["status"])
        .env("RUST_LOG", "trace")
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(daemon.stop().code(), Some(0));

    let output = home.output();
    assert!(
        output.contains("without a valid token"),
        "the refusals were logged:\n{output}"
    );
    assert!(
        home.file("logs/agentd.log").is_file(),
        "the daemon's own log is logs/agentd.log"
    );
    for secret in [&real, &near_miss, &other] {
        assert!(
            !output.contains(secret.as_str()),
            "a token was logged:\n{output}"
        );
        assert!(
            !replies.contains(secret.as_str()),
            "a token was in a reply: {replies}"
        );
    }
}

// ── Scoped run tokens, against an in-process engine and the daemon's wiring ──

const OWNER: &str = "owner-token-for-the-in-process-security-tests";

/// A harness whose runs wait for [`Engine::release`] before finishing.
struct Hold {
    requests: Arc<Mutex<Vec<RunRequest>>>,
    release: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl Harness for Hold {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Hold"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.requests.lock().unwrap().push(request);
        let release = self.release.clone();
        Ok(futures::stream::once(async {
            Ok(AgentEvent::TextDelta {
                text: "working".into(),
            })
        })
        .chain(futures::stream::once(async move {
            if let Ok(permit) = release.acquire_owned().await {
                permit.forget();
            }
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })
        }))
        .boxed())
    }
}

/// The sessions engine served by `loams_agentd::server::rpc_server`.
struct Engine {
    core: EngineCore,
    port: u16,
    scoped: Arc<ScopedTokens>,
    requests: Arc<Mutex<Vec<RunRequest>>>,
    release: Arc<tokio::sync::Semaphore>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
    root: tempfile::TempDir,
}

impl Engine {
    async fn start() -> Self {
        let root = tempfile::Builder::new()
            .prefix("agentd-security-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(Hold {
            requests: requests.clone(),
            release: release.clone(),
        }));
        let core = EngineCore::assemble(
            &root.path().join("store"),
            Arc::new(registry),
            HarnessId::Mock,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let scoped = Arc::new(ScopedTokens::new());
        core.sessions.set_mcp_endpoint(port, scoped.clone());
        let server = loams_agentd::server::rpc_server(
            core.rpc_service(),
            Arc::new(core.workspace.clone()),
            DaemonInfo {
                mode: DaemonMode::Child,
                pid: std::process::id(),
                started_at_ms: 1,
            },
            Secret::new(OWNER),
            scoped.clone(),
        );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(server.serve_until(listener, async {
            let _ = stopped.await;
        }));
        let engine = Self {
            core,
            port,
            scoped,
            requests,
            release,
            stop: Some(stop),
            server,
            root,
        };
        // Two top-level chats, and a side chat under `main`.
        let owner = engine.dial(OWNER).await.unwrap();
        for (chat, parent) in [("main", None), ("other", None), ("side", Some("main"))] {
            let mut params = serde_json::json!({
                "op": "createChat",
                "chatId": chat,
                "deviceId": engine.core.device_id,
                "cwd": engine.root.path().to_string_lossy(),
            });
            if let Some(parent) = parent {
                params["parentChatId"] = serde_json::json!(parent);
            }
            owner.call(methods::MUTATE, params).await.unwrap();
        }
        // The holding harness cannot ask for approvals, and a thread with no
        // mode is supervised (threat model I-1, SD1 T30-19): the top-level
        // chats ask for full access, as a client must; `side` inherits.
        for chat in ["main", "other"] {
            owner
                .call(
                    methods::SET_THREAD_APPROVAL_MODE,
                    serde_json::json!({ "chatId": chat, "mode": "full-access" }),
                )
                .await
                .unwrap();
        }
        engine
    }

    fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// Connect with `token` and say `Hello`.
    async fn dial(&self, token: &str) -> Result<RpcClient, loams_agentd_rpc::RpcError> {
        let client = connect_ws_with_token(&self.url(), token).await?;
        let reply = client
            .call(methods::HELLO, hello_params(AGENTD_PROTOCOL))
            .await?;
        let expected = if token == OWNER { "owner" } else { "mcp" };
        assert_eq!(reply["role"], expected);
        Ok(client)
    }

    /// Start a run of `chat` on the holding harness and return its request.
    async fn run(&self, chat: &str) -> RunRequest {
        let before = self.requests.lock().unwrap().len();
        self.core
            .sessions
            .dispatch(
                chat,
                HarnessId::Mock,
                RunRequest {
                    mcp_servers: Vec::new(),
                    mcp: None,
                    prompt: "hello".into(),
                    harness: Some(HarnessId::Mock),
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: self.root.path().to_string_lossy().into_owned(),
                    sandbox: SandboxLevel::WorkspaceWrite,
                    auto_approve: true,
                    resume: None,
                    attachments: vec![],
                    worktree: None,
                },
                Some(format!("{chat}-user")),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(request) = self.requests.lock().unwrap().get(before).cloned() {
                    return request;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the harness was started")
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        (&mut self.server).await.unwrap().unwrap();
        self.core.shutdown().await;
    }
}

fn is_forbidden(result: Result<serde_json::Value, loams_agentd_rpc::RpcError>) -> bool {
    match result {
        Ok(_) => false,
        Err(error) => error.to_string().starts_with(FORBIDDEN_ROLE),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_token_cannot_resolve_approval() {
    let engine = Engine::start().await;
    let run = engine.scoped.issue("main").unwrap();
    let mcp = engine.dial(run.token.expose()).await.unwrap();
    let approval = serde_json::json!({ "sessionId": "main", "callId": "c1", "decision": "once" });
    assert!(is_forbidden(
        mcp.call("ResolveApproval", approval.clone()).await
    ));
    // Nor any other owner method, on its own session.
    for (method, params) in [
        (methods::SHUTDOWN, serde_json::json!({})),
        (
            methods::WRITE_WORKSPACE_FILE,
            serde_json::json!({ "chatId": "main", "path": "x", "content": "y" }),
        ),
        (
            methods::OPEN_TERMINAL,
            serde_json::json!({ "chatId": "main" }),
        ),
        (
            methods::FETCH_TOOL_BLOB,
            serde_json::json!({ "blobRef": "main/p" }),
        ),
        (
            methods::MUTATE,
            serde_json::json!({ "op": "deleteChat", "chatId": "main" }),
        ),
    ] {
        assert!(is_forbidden(mcp.call(method, params).await), "{method}");
    }
    // The refusals are about the role: the owner gets past the check.
    let owner = engine.dial(OWNER).await.unwrap();
    let err = owner.call("ResolveApproval", approval).await.unwrap_err();
    assert!(!err.to_string().starts_with(FORBIDDEN_ROLE), "{err}");
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_token_bound_to_session() {
    let engine = Engine::start().await;
    let run = engine.scoped.issue("main").unwrap();
    let mcp = engine.dial(run.token.expose()).await.unwrap();
    let watch = |chat: &str| serde_json::json!({ "chatId": chat });
    // Its own session and the side chat under it.
    for chat in ["main", "side"] {
        let mut stream = mcp
            .subscribe_scoped(methods::WATCH_DOC_MESSAGES, watch(chat))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), stream.recv())
                .await
                .unwrap()
                .is_some(),
            "{chat}"
        );
    }
    // Not another session, whatever the method.
    assert!(
        mcp.subscribe_checked(methods::WATCH_DOC_MESSAGES, watch("other"))
            .await
            .is_err_and(|e| e.to_string().starts_with(FORBIDDEN_ROLE))
    );
    for (method, params) in [
        (
            methods::QUEUE_MESSAGE,
            serde_json::json!({ "chatId": "other", "text": "hi" }),
        ),
        (
            methods::QUEUE_COMMAND,
            serde_json::json!({ "chatId": "other", "command": { "type": "interrupt" } }),
        ),
        (
            methods::MUTATE,
            serde_json::json!({ "op": "renameChat", "chatId": "other", "title": "x" }),
        ),
        (
            methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": "new-1", "parentChatId": "other" }),
        ),
    ] {
        assert!(is_forbidden(mcp.call(method, params).await), "{method}");
    }
    // A new side chat under its session is its own to drive.
    mcp.call(
        methods::MUTATE,
        serde_json::json!({
            "op": "createChat", "chatId": "child", "parentChatId": "main",
            "deviceId": engine.core.device_id,
        }),
    )
    .await
    .unwrap();
    mcp.call(
        methods::MUTATE,
        serde_json::json!({ "op": "renameChat", "chatId": "child", "title": "Child" }),
    )
    .await
    .unwrap();
    // A token for `other` reaches `other` but not `main`.
    let other = engine.scoped.issue("other").unwrap();
    let other = engine.dial(other.token.expose()).await.unwrap();
    assert!(is_forbidden(
        other
            .call(
                methods::MUTATE,
                serde_json::json!({ "op": "renameChat", "chatId": "main", "title": "x" }),
            )
            .await
    ));
    other
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "renameChat", "chatId": "other", "title": "Other" }),
        )
        .await
        .unwrap();
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_shim_env_has_no_main_token() {
    let engine = Engine::start().await;
    let request = engine.run("main").await;
    let mcp = request.mcp.expect("the run carries the MCP shim");
    assert_eq!(mcp.args, ["mcp"]);
    let mut keys: Vec<&str> = mcp.env.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = vec![ENV_PORT, ENV_RUN_TOKEN, ENV_SESSION_ID];
    expected.sort_unstable();
    assert_eq!(keys, expected, "only the three scoped keys");
    assert_eq!(mcp.env[ENV_PORT], engine.port.to_string());
    assert_eq!(mcp.env[ENV_SESSION_ID], "main");
    assert_ne!(mcp.env[ENV_RUN_TOKEN], OWNER);
    assert!(
        mcp.env.values().all(|value| !value.contains(OWNER)),
        "the main token is in the shim's environment"
    );
    assert!(
        !format!("{mcp:?}").contains(&mcp.env[ENV_RUN_TOKEN]),
        "Debug shows the run token"
    );
    engine.release.add_permits(1);
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_token_dies_with_run() {
    let engine = Engine::start().await;
    let request = engine.run("main").await;
    let token = request.mcp.expect("the MCP shim").env[ENV_RUN_TOKEN].clone();
    // While the run lives, its token is an `mcp` connection on its session.
    let client = engine.dial(&token).await.unwrap();
    client
        .call(methods::LIST_HARNESSES, serde_json::json!({}))
        .await
        .unwrap();

    // The real shim, as the harness would start it, reaches the daemon.
    let shim = shim_call(&engine, &token, "whoami", serde_json::json!({}));
    assert_eq!(shim["result"]["isError"], false, "{shim}");
    let text = shim["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"localDeviceId\""), "{text}");
    // And its tools are bound to the session too.
    let refused = shim_call(
        &engine,
        &token,
        "interrupt_chat",
        serde_json::json!({ "chat": "other" }),
    );
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let text = refused["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(FORBIDDEN_ROLE), "{text}");

    // The run ends: its open connection closes and the token is refused.
    engine.release.add_permits(1);
    let closed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if client
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(closed.is_ok(), "the run's connection outlived the run");
    let again = connect_ws_with_token(&engine.url(), &token).await;
    assert!(
        again.is_err_and(|e| e.to_string().contains("401")),
        "a dead run's token still authenticates"
    );
    engine.shutdown().await;
}

/// Start `loams-agentd mcp` with the run's three keys, call one tool and
/// return the JSON-RPC response.
fn shim_call(
    engine: &Engine,
    token: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let home = engine.root.path().join("shim-home");
    std::fs::create_dir_all(&home).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_loams-agentd"))
        .arg("mcp")
        .env_remove("LOAMS_AGENTD_CONFIG")
        .env("HOME", &home)
        .env(ENV_RUN_TOKEN, token)
        .env(ENV_PORT, engine.port.to_string())
        .env(ENV_SESSION_ID, "main")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let requests = [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "test", "version": "1" } } }),
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } }),
    ];
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let response = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = rx.recv_timeout(left).expect("the shim answered");
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 2 {
            break value;
        }
    };
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    response
}
