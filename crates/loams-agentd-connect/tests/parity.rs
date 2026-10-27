//! The parity suite (plan SD1 Task 6, Review Focus 6): every Studio method
//! answers the same over ndjson and over Connect.
//!
//! Two in-process daemons with the Mock harness are set up alike: daemon A is
//! driven over the ndjson wire (`memory_client`, the same framing as the
//! socket) and daemon B over the loopback Connect listener. One scenario runs
//! on both, step by step, so both daemons' states move together. Each step
//! sends the ndjson params (as an ndjson client writes them) to A and the
//! proto JSON request (as a Connect client writes it) to B, maps B's response
//! back to the ndjson result type, and compares the two after normalising what
//! is per-daemon: the data directory, the device id, generated ids and
//! timestamps. A failure compares the Connect code and message.
//!
//! `connect_and_ndjson_agree` has one case per `loams.agentd.v1` method with an
//! ndjson twin (`twins::CONNECT_METHODS`); a later task that serves a §53 §6.5
//! gap adds its case here, and the coverage check fails until it does.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use connectrpc::ConnectError;
use connectrpc::client::{ClientConfig, HttpClient};
use futures::future::BoxFuture;
use loams_agentd_connect::map::{MapError, Wire, from_msg, from_opt_msg};
use loams_agentd_connect::proto::loams::agentd::v1 as pb;
use loams_agentd_connect::twins::{CONNECT_METHODS, Twin};
use loams_agentd_connect::{Agentd, ConnectListener, HostInfo, LoopbackOptions, error};
use loams_agentd_doc::SessionCommandStatus;
use loams_agentd_proto as rs;
use loams_agentd_proto::daemon::DaemonMode;
use loams_agentd_proto::rpc::{Method, calls};
use loams_agentd_proto::transcript::{TranscriptFrame, apply_transcript_frame};
use loams_agentd_rpc::{RpcClient, RpcError, RpcService, Secret, memory_client};
use loams_agentd_sessions::{
    Engine, EngineConfig, EngineProfile, EngineRuntime, HarnessId, TerminalShell,
};
use serde_json::{Value, json};

const TOKEN: &str = "parity-test-token";
const WAIT: Duration = Duration::from_secs(10);
/// How long a stream may stay silent before its first frame counts as none.
const QUIET: Duration = Duration::from_secs(3);
/// How long a settled transcript stays quiet before it counts as final.
const SETTLE: Duration = Duration::from_millis(750);

/// What differs between the two daemons, replaced before comparing.
#[derive(Debug, Default)]
struct Facts {
    root: String,
    repo: String,
    device: String,
    /// Values learnt during the scenario (a queued message's id, a lease, a
    /// terminal id), by name, and the placeholders that replace them.
    learnt: BTreeMap<&'static str, String>,
}

impl Facts {
    fn get(&self, name: &str) -> String {
        self.learnt.get(name).cloned().unwrap_or_default()
    }
}

#[derive(Clone)]
struct Clients {
    studio: pb::StudioServiceClient<HttpClient>,
    projects: pb::ProjectServiceClient<HttpClient>,
    threads: pb::ThreadServiceClient<HttpClient>,
    queue: pb::QueueServiceClient<HttpClient>,
    harness: pb::HarnessServiceClient<HttpClient>,
    git: pb::GitServiceClient<HttpClient>,
    diff: pb::DiffServiceClient<HttpClient>,
    files: pb::FileServiceClient<HttpClient>,
    terminals: pb::TerminalServiceClient<HttpClient>,
    preview: pb::PreviewServiceClient<HttpClient>,
    attachments: pb::AttachmentServiceClient<HttpClient>,
    notifications: pb::NotificationServiceClient<HttpClient>,
    review: pb::ReviewServiceClient<HttpClient>,
    groups: pb::RunGroupServiceClient<HttpClient>,
    approvals: pb::ApprovalServiceClient<HttpClient>,
    market: pb::MarketplaceServiceClient<HttpClient>,
    factory: pb::FactoryServiceClient<HttpClient>,
}

struct Daemon {
    _dir: tempfile::TempDir,
    runtime: EngineRuntime,
    listener: ConnectListener,
    ndjson: RpcClient,
    clients: Clients,
    facts: Facts,
}

fn git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Parity",
            "-c",
            "user.email=parity@example.invalid",
        ])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_DATE", "2026-10-13T09:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-13T09:00:00Z")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

impl Daemon {
    /// Boxed by a plain function: the future is built in this short-lived
    /// frame, not in a slot of the scenario's poll frame.
    fn start() -> BoxFuture<'static, Self> {
        Box::pin(Self::boot())
    }

    async fn boot() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, so a path the daemon resolves reads the same as ours.
        let root = dir.path().canonicalize().unwrap();
        let data = root.join("data");
        let repo = root.join("repo");
        let home = root.join("home");
        for path in [&data, &repo, &home, &repo.join("src")] {
            std::fs::create_dir_all(path).unwrap();
        }
        // The same repository in both daemons, with fixed dates: the commit
        // ids agree too.
        std::fs::write(repo.join("README.md"), "# Parity\n").unwrap();
        std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "initial commit"]);

        let config = EngineConfig {
            data_dir: data.clone(),
            default_harness: HarnessId::Mock,
            terminal_shell: Some(TerminalShell::isolated("/bin/sh", &home)),
            tool_outputs: Default::default(),
            mcp_endpoint: None,
            approvals: Default::default(),
        };
        let runtime = Engine::assemble_runtime(&config, EngineProfile::local(&data).unwrap())
            .await
            .unwrap();
        // The marketplace over the testkit's signed fake index (MK1 Task 4b).
        let world = loams_agentd_marketplace::testkit::World::new(&root.join("agentd"));
        let market = loams_agentd_marketplace::MarketplaceHost::open(world.deps()).unwrap();
        assert!(market.refresh().await.is_empty());
        runtime.core().attach_marketplace(market);
        let engine: Arc<dyn RpcService> = runtime.core().rpc_service();
        let agentd = Agentd::new(
            engine.clone(),
            HostInfo {
                protocol: 1,
                version: "0.0.0-test".into(),
                build_sha: "test".into(),
                mode: DaemonMode::Child,
                pid: 1,
                started_at_ms: 1,
                features: vec!["studio.v1".into()],
            },
        );
        let listener = ConnectListener::bind(
            agentd,
            LoopbackOptions {
                token: Secret::new(TOKEN),
                auth_failure_delay: Duration::from_millis(10),
            },
        )
        .await
        .unwrap();
        let config = ClientConfig::new(
            format!("http://127.0.0.1:{}", listener.port())
                .parse()
                .unwrap(),
        )
        .with_default_header("authorization", format!("Bearer {TOKEN}"));
        let http = HttpClient::plaintext;
        let clients = Clients {
            studio: pb::StudioServiceClient::new(http(), config.clone()),
            projects: pb::ProjectServiceClient::new(http(), config.clone()),
            threads: pb::ThreadServiceClient::new(http(), config.clone()),
            queue: pb::QueueServiceClient::new(http(), config.clone()),
            harness: pb::HarnessServiceClient::new(http(), config.clone()),
            git: pb::GitServiceClient::new(http(), config.clone()),
            diff: pb::DiffServiceClient::new(http(), config.clone()),
            files: pb::FileServiceClient::new(http(), config.clone()),
            terminals: pb::TerminalServiceClient::new(http(), config.clone()),
            preview: pb::PreviewServiceClient::new(http(), config.clone()),
            attachments: pb::AttachmentServiceClient::new(http(), config.clone()),
            notifications: pb::NotificationServiceClient::new(http(), config.clone()),
            review: pb::ReviewServiceClient::new(http(), config.clone()),
            groups: pb::RunGroupServiceClient::new(http(), config.clone()),
            approvals: pb::ApprovalServiceClient::new(http(), config.clone()),
            market: pb::MarketplaceServiceClient::new(http(), config.clone()),
            factory: pb::FactoryServiceClient::new(http(), config),
        };
        let ndjson = memory_client(engine);
        let info: rs::EngineInfo = ndjson
            .call_as(calls::EngineInfo::NAME, json!({}))
            .await
            .unwrap();
        Self {
            facts: Facts {
                root: root.to_string_lossy().into_owned(),
                repo: repo.to_string_lossy().into_owned(),
                device: info.device_id,
                learnt: BTreeMap::new(),
            },
            _dir: dir,
            runtime,
            listener,
            ndjson,
            clients,
        }
    }

    fn stop(self) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.listener.stop().await;
            self.runtime.shutdown().await;
        })
    }
}

/// A failure, as the Connect client sees it: its code and message.
fn failure(code: connectrpc::ErrorCode, message: &str) -> Value {
    json!({ "error": format!("{code:?}"), "message": message })
}

/// An ndjson failure as the Connect error the same failure becomes. The ndjson
/// wire renders `BadParams` as `bad params: …` text, which the Connect
/// surface answers as `invalid_argument` with the bare message.
fn nd_failure(error: RpcError) -> Value {
    let error = match error {
        RpcError::Failed(text) => match text.strip_prefix("bad params: ") {
            Some(message) => RpcError::BadParams(message.to_owned()),
            None => RpcError::Failed(text),
        },
        other => other,
    };
    let error = error::from_rpc(error);
    failure(error.code, error.message.as_deref().unwrap_or_default())
}

fn cx_failure(error: &ConnectError) -> Value {
    failure(error.code, error.message.as_deref().unwrap_or_default())
}

/// A value through the ndjson result type and back, so absent and default
/// fields read alike on both sides.
fn reserialize<T: serde::Serialize + serde::de::DeserializeOwned>(value: Value) -> Value {
    serde_json::to_value(serde_json::from_value::<T>(value).unwrap()).unwrap()
}

fn is_uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Keys whose values are clocks, process ids, terminal bytes, or derived
/// from the checkout's path (a checkout id is a hash of it, and an entry's
/// mutation revision covers it), so they differ between the two daemons.
fn is_volatile(key: &str) -> bool {
    key.ends_with("At")
        || key.ends_with("Ms")
        || matches!(
            key,
            "at" | "pid" | "seq" | "data" | "checkoutId" | "mutationRevision"
        )
}

/// The per-daemon parts of a value replaced by placeholders.
fn normalize(value: &mut Value, facts: &Facts, also: &[&str], uuids: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            let mut out = text.replace(&facts.root, "<root>");
            if !facts.device.is_empty() {
                out = out.replace(&facts.device, "<device>");
            }
            for (name, learnt) in &facts.learnt {
                if !learnt.is_empty() {
                    out = out.replace(learnt.as_str(), &format!("<{name}>"));
                }
            }
            if is_uuid(&out) {
                let n = uuids.iter().position(|u| *u == out).unwrap_or_else(|| {
                    uuids.push(out.clone());
                    uuids.len() - 1
                });
                out = format!("<uuid-{n}>");
            }
            *text = out;
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| normalize(item, facts, also, uuids)),
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if (is_volatile(key) && !item.is_null()) || also.contains(&key.as_str()) {
                    *item = json!("<volatile>");
                } else {
                    normalize(item, facts, also, uuids);
                }
            }
        }
        _ => {}
    }
}

/// The two daemons and the tally.
struct Suite {
    a: Daemon,
    b: Daemon,
    covered: BTreeSet<String>,
    mismatches: Vec<String>,
    /// More volatile keys for the next step only.
    also: &'static [&'static str],
}

impl Suite {
    /// Compares one step; returns each side's raw result (`Null` on failure)
    /// so a later step can use what the daemon generated.
    fn agree(
        &mut self,
        path: &str,
        nd: Result<Value, RpcError>,
        cx: Result<Value, ConnectError>,
    ) -> (Value, Value) {
        self.covered.insert(path.to_owned());
        let raw_a = nd.as_ref().cloned().unwrap_or(Value::Null);
        let raw_b = cx.as_ref().cloned().unwrap_or(Value::Null);
        let mut left = nd.unwrap_or_else(nd_failure);
        let mut right = cx.unwrap_or_else(|e| cx_failure(&e));
        let also = std::mem::take(&mut self.also);
        normalize(&mut left, &self.a.facts, also, &mut Vec::new());
        normalize(&mut right, &self.b.facts, also, &mut Vec::new());
        if left != right {
            self.mismatches
                .push(format!("{path}\n  ndjson:  {left}\n  connect: {right}"));
        }
        (raw_a, raw_b)
    }

    /// Remembers a value each daemon generated, so later params can name it
    /// and the comparison treats the two as the same.
    fn learn(&mut self, name: &'static str, a: &Value, b: &Value) {
        let text = |v: &Value| v.as_str().map(str::to_owned).unwrap_or_default();
        self.a.facts.learnt.insert(name, text(a));
        self.b.facts.learnt.insert(name, text(b));
    }
}

/// One unary step: ndjson params (`|d|` is the daemon's facts) to daemon A,
/// the proto JSON request to `client.rpc` on daemon B, whose response `|r|`
/// maps back to the ndjson result (`?` allowed).
macro_rules! unary {
    ($s:ident, $path:literal, $method:ident, |$d:ident| $nd:expr, $client:ident . $rpc:ident ($cx:expr), |$r:ident| $back:expr $(,)?) => {{
        type M = calls::$method;
        let nd = {
            #[allow(unused_variables)]
            let $d = &$s.a.facts;
            let params: <M as Method>::Params =
                serde_json::from_value($nd).expect(concat!($path, ": ndjson params"));
            $s.a.ndjson
                .call(<M as Method>::NAME, serde_json::to_value(&params).unwrap())
                .await
                .map(reserialize::<<M as Method>::Result>)
        };
        let cx = {
            #[allow(unused_variables)]
            let $d = &$s.b.facts;
            let request =
                serde_json::from_value($cx).expect(concat!($path, ": proto JSON request"));
            match $s.b.clients.$client.$rpc(request).await {
                Ok(response) => {
                    #[allow(unused_mut, unused_variables)]
                    let mut $r = response.into_owned();
                    #[allow(clippy::redundant_closure_call)]
                    let back: Result<<M as Method>::Result, MapError> = (|| Ok($back))();
                    Ok(serde_json::to_value(back.expect(concat!($path, ": maps back"))).unwrap())
                }
                Err(error) => Err(error),
            }
        };
        $s.agree($path, nd, cx)
    }};
}

/// One stream step: as [`unary`], comparing the first frame.
macro_rules! first {
    ($s:ident, $path:literal, $method:ident, |$d:ident| $nd:expr, $client:ident . $rpc:ident ($cx:expr), |$r:ident| $back:expr $(,)?) => {{
        type M = calls::$method;
        let nd = {
            #[allow(unused_variables)]
            let $d = &$s.a.facts;
            let params: <M as Method>::Params =
                serde_json::from_value($nd).expect(concat!($path, ": ndjson params"));
            match $s
                .a
                .ndjson
                .subscribe_checked(<M as Method>::NAME, serde_json::to_value(&params).unwrap())
                .await
            {
                Ok(mut items) => match tokio::time::timeout(QUIET, items.recv()).await {
                    Ok(Some(item)) => Ok(reserialize::<<M as Method>::Result>(item)),
                    Ok(None) => panic!(concat!($path, ": the ndjson stream ended")),
                    // A stream with nothing to say yet (no pull request).
                    Err(_) => Ok(json!("<no frame>")),
                },
                Err(error) => Err(error),
            }
        };
        let cx = {
            #[allow(unused_variables)]
            let $d = &$s.b.facts;
            let request =
                serde_json::from_value($cx).expect(concat!($path, ": proto JSON request"));
            match $s.b.clients.$client.$rpc(request).await {
                Ok(mut stream) => {
                    match tokio::time::timeout(QUIET, stream.message())
                        .await
                        .unwrap_or(Ok(None))
                    {
                        Ok(Some(view)) => {
                            #[allow(unused_mut, unused_variables)]
                            let mut $r = view.to_owned_message();
                            #[allow(clippy::redundant_closure_call)]
                            let back: Result<
                                <M as Method>::Result,
                                MapError,
                            > = (|| Ok($back))();
                            Ok(
                                serde_json::to_value(back.expect(concat!($path, ": maps back")))
                                    .unwrap(),
                            )
                        }
                        // Silent, or ended: the ndjson side tells which.
                        Ok(None) => Ok(json!("<no frame>")),
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            }
        };
        $s.agree($path, nd, cx)
    }};
}

fn mutated(ok: bool) -> rs::MutateReply {
    rs::MutateReply {
        ok,
        sidebar_preferences: None,
    }
}

fn command(command_id: String) -> rs::QueueCommandReply {
    rs::QueueCommandReply { command_id }
}

fn ok(ok: bool) -> rs::OkReply {
    rs::OkReply { ok }
}

/// The transcript of `chat` once its turn settled: the frames of each wire
/// folded with the daemon's own `apply_transcript_frame`.
async fn settled_transcripts(s: &mut Suite, chat: &str) {
    let mut nd =
        s.a.ndjson
            .subscribe_checked(calls::WatchDocMessages::NAME, json!({ "chatId": chat }))
            .await
            .unwrap();
    let mut cx =
        s.b.clients
            .threads
            .watch_transcript(pb::WatchTranscriptRequest {
                chat_id: chat.into(),
                ..Default::default()
            })
            .await
            .unwrap();
    let done = |entries: &[rs::SessionMessageEntry]| {
        entries.len() >= 2
            && entries
                .last()
                .is_some_and(|e| e.status == Some(rs::MessageStatus::Complete))
    };
    let mut left: Vec<rs::SessionMessageEntry> = Vec::new();
    while !done(&left) {
        let item = tokio::time::timeout(WAIT, nd.recv())
            .await
            .expect("an ndjson transcript frame")
            .expect("the ndjson transcript is open");
        let update: loams_agentd_proto::transcript::TranscriptUpdate =
            serde_json::from_value(item).unwrap();
        apply_transcript_frame(&mut left, update.frame).unwrap();
    }
    // Settled: what follows (the turn's duration) arrives within moments.
    while let Ok(Some(item)) = tokio::time::timeout(SETTLE, nd.recv()).await {
        let update: loams_agentd_proto::transcript::TranscriptUpdate =
            serde_json::from_value(item).unwrap();
        apply_transcript_frame(&mut left, update.frame).unwrap();
    }
    let mut right: Vec<rs::SessionMessageEntry> = Vec::new();
    while !done(&right) {
        let frame = tokio::time::timeout(WAIT, cx.message())
            .await
            .expect("a Connect transcript frame")
            .unwrap()
            .expect("the Connect transcript is open")
            .to_owned_message();
        let update: loams_agentd_proto::transcript::TranscriptUpdate =
            from_msg(frame.update, "update").unwrap();
        let frame: TranscriptFrame = update.frame;
        apply_transcript_frame(&mut right, frame).unwrap();
    }
    while let Ok(Ok(Some(frame))) = tokio::time::timeout(SETTLE, cx.message()).await {
        let frame = frame.to_owned_message();
        let update: loams_agentd_proto::transcript::TranscriptUpdate =
            from_msg(frame.update, "update").unwrap();
        apply_transcript_frame(&mut right, update.frame).unwrap();
    }
    s.agree(
        "/loams.agentd.v1.ThreadService/WatchTranscript",
        Ok(serde_json::to_value(&left).unwrap()),
        Ok(serde_json::to_value(&right).unwrap()),
    );
}

/// Waits until neither daemon has work left on `chat`: every command queued
/// to it is resolved and no turn is in flight.
///
/// `QueueCommand` (Send, Steer, Interrupt, AnswerQuestion) only appends the
/// command to the chat's doc and answers its id; the daemon applies it
/// moments later. A step that reads what the commands did (a turn's
/// checkpoint, whether the thread is running) compares a moment that differs
/// between the two daemons unless both have finished.
async fn settle_thread(s: &Suite, chat: &str) {
    for daemon in [&s.a, &s.b] {
        let core = daemon.runtime.core();
        let handle = core.doc_host.open(chat).unwrap();
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let pending = handle
                .doc()
                .read_commands()
                .unwrap()
                .iter()
                .any(|c| c.status == SessionCommandStatus::Pending);
            if !pending && !core.sessions.turn_in_flight(chat) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{chat}'s commands and turn settle"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The stack the scenario runs on: half a default thread's 2 MiB. Split
/// into sections it needs a little over 0.5 MiB unoptimized; as one
/// `async fn` it needed over 1.5 MiB.
const SCENARIO_STACK: usize = 1 << 20;

/// Set in the child process that runs the scenario.
const CHILD: &str = "LOAMS_AGENTD_PARITY_CHILD";

/// The scenario runs in a child process of this test binary, so the engine's
/// process-wide settings point into a scratch directory: new worktrees go to
/// `LOAMS_DESKTOP_WORKTREES_DIR`, never under the person's home. (Setting the
/// variable here would need `unsafe`, which this crate forbids.)
#[test]
fn connect_and_ndjson_agree() {
    if std::env::var_os(CHILD).is_some() {
        // The scenario's own thread, with half the default stack: a scenario
        // whose frames grow back towards 2 MiB (see `section`) fails here on
        // every run instead of overflowing a default stack now and then.
        let scenario = std::thread::Builder::new()
            .name("parity-scenario".into())
            .stack_size(SCENARIO_STACK)
            .spawn(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(scenario());
            })
            .unwrap();
        if let Err(panic) = scenario.join() {
            std::panic::resume_unwind(panic);
        }
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "connect_and_ndjson_agree",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env(
            "LOAMS_DESKTOP_WORKTREES_DIR",
            scratch.path().join("worktrees"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the parity scenario failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Section `n` of the scenario, boxed; `None` past the last.
///
/// Unoptimized, one `async fn` holding every case keeps all their locals in
/// a single poll frame: 1.5 MB with 124 cases, which left too little of a
/// 2 MiB thread stack for the calls it polls, and the scenario overflowed it
/// now and then (0.1.1 CI). Each section has its own frame, only one is on
/// the stack at a time, and this plain function builds its future, so the
/// scenario's own frame holds one box rather than a slot per section.
fn section(n: usize, s: &mut Suite) -> Option<BoxFuture<'_, ()>> {
    Some(match n {
        0 => Box::pin(studio_and_harness(s)),
        1 => Box::pin(projects(s)),
        2 => Box::pin(threads(s)),
        3 => Box::pin(queue(s)),
        4 => Box::pin(git_and_diff(s)),
        5 => Box::pin(files_previews_attachments(s)),
        6 => Box::pin(terminals(s)),
        7 => Box::pin(review_and_search(s)),
        8 => Box::pin(run_groups(s)),
        9 => Box::pin(approvals_and_teardown(s)),
        10 => Box::pin(marketplace(s)),
        // The software factory (SF1 amendment F2).
        11 => Box::pin(factory_steps(s)),
        _ => return None,
    })
}

async fn scenario() {
    let mut s = Suite {
        a: Daemon::start().await,
        b: Daemon::start().await,
        covered: BTreeSet::new(),
        mismatches: Vec::new(),
        also: &[],
    };

    let mut n = 0;
    while let Some(section) = section(n, &mut s) {
        section.await;
        n += 1;
    }

    // Every method with an ndjson twin has a case.
    let twinned: BTreeSet<String> = CONNECT_METHODS
        .iter()
        .filter(|row| !matches!(row.twin, Twin::Gap { .. }))
        .map(|row| row.path())
        .collect();
    let missing: Vec<String> = twinned.difference(&s.covered).cloned().collect();
    let unknown: Vec<String> = s.covered.difference(&twinned).cloned().collect();
    let Suite {
        a, b, mismatches, ..
    } = s;
    a.stop().await;
    b.stop().await;
    assert!(
        missing.is_empty(),
        "methods without a parity case: {missing:?}"
    );
    assert!(
        unknown.is_empty(),
        "cases for no twinned method: {unknown:?}"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {} methods answer differently:\n{}",
        mismatches.len(),
        twinned.len(),
        mismatches.join("\n")
    );
}

/// The scenario's StudioService and HarnessService.
async fn studio_and_harness(s: &mut Suite) {
    // ---- StudioService ----
    unary!(
        s,
        "/loams.agentd.v1.StudioService/GetHost",
        EngineInfo,
        |d| json!({}),
        studio.get_host(json!({})),
        |r| {
            let host = r
                .host
                .into_option()
                .ok_or_else(|| MapError::missing("host"))?;
            rs::EngineInfo {
                device_id: host.device_id,
                workspace_scope: Wire::from_pb(host.workspace_scope)?,
                capabilities: host.capabilities,
            }
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.StudioService/GetTitleSettings",
        GetTitleSettings,
        |d| json!({}),
        studio.get_title_settings(json!({})),
        |r| from_msg(r.settings, "settings")?
    );
    unary!(
        s,
        "/loams.agentd.v1.StudioService/SetTitleSettings",
        SetTitleSettings,
        |d| json!({ "harness": "mock", "model": "mock-1" }),
        studio.set_title_settings(
            json!({ "settings": { "harness": "HARNESS_ID_MOCK", "model": "mock-1" } })
        ),
        |r| from_msg(r.settings, "settings")?
    );

    // ---- HarnessService ----
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ListHarnesses",
        ListHarnesses,
        |d| json!({}),
        harness.list_harnesses(json!({})),
        |r| rs::HarnessCatalog(Wire::from_pb(r.harnesses)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ListModels",
        ListModels,
        |d| json!({ "harness": "mock" }),
        harness.list_models(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| rs::ModelList(Wire::from_pb(r.models)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ListSkills",
        ListSkills,
        |d| json!({ "harness": "mock", "path": d.repo }),
        harness.list_skills(json!({ "harness": "HARNESS_ID_MOCK", "path": d.repo })),
        |r| rs::SkillList(if r.unavailable {
            None
        } else {
            Some(Wire::from_pb(r.skills)?)
        })
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ListCommands",
        ListCommands,
        |d| json!({ "harness": "mock", "path": d.repo }),
        harness.list_commands(json!({ "harness": "HARNESS_ID_MOCK", "path": d.repo })),
        |r| rs::SlashCommandList(Wire::from_pb(r.commands)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/SetHarnessEnabled",
        SetHarnessEnabled,
        |d| json!({ "harness": "mock", "enabled": true }),
        harness.set_harness_enabled(json!({ "harness": "HARNESS_ID_MOCK", "enabled": true })),
        |r| rs::HarnessCatalog(Wire::from_pb(r.harnesses)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/InstallHarness",
        InstallHarness,
        |d| json!({ "harness": "mock" }),
        harness.install_harness(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| rs::HarnessCatalog(Wire::from_pb(r.harnesses)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/CancelInstall",
        CancelInstall,
        |d| json!({ "harness": "mock" }),
        harness.cancel_install(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| rs::Empty {}
    );
    first!(
        s,
        "/loams.agentd.v1.HarnessService/WatchHarnessUpdates",
        WatchHarnessUpdates,
        |d| json!({}),
        harness.watch_harness_updates(json!({})),
        |r| rs::HarnessUpdateList(Wire::from_pb(r.updates)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/CheckHarnessUpdates",
        CheckHarnessUpdates,
        |d| json!({ "harness": "mock" }),
        harness.check_harness_updates(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| rs::HarnessUpdateList(Wire::from_pb(r.updates)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ApplyHarnessUpdate",
        ApplyHarnessUpdate,
        |d| json!({ "harness": "mock" }),
        harness.apply_harness_update(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| rs::ApplyHarnessUpdateReply {
            ok: r.ok,
            version: r.version
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/CancelHarnessUpdate",
        CancelHarnessUpdate,
        |d| json!({ "harness": "mock" }),
        harness.cancel_harness_update(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| rs::CancelledReply {
            cancelled: r.cancelled
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/DismissHarnessUpdate",
        DismissHarnessUpdate,
        |d| json!({ "harness": "mock", "version": "1.0.0" }),
        harness.dismiss_harness_update(json!({ "harness": "HARNESS_ID_MOCK", "version": "1.0.0" })),
        |r| from_msg(r.status, "status")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ListAgentAccounts",
        ListAgentAccounts,
        |d| json!({ "forceUsage": false }),
        harness.list_agent_accounts(json!({ "forceUsage": false })),
        |r| from_msg(r.snapshot, "snapshot")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ActivateAgentAccount",
        ActivateAgentAccount,
        |d| json!({ "harness": "mock", "accountId": "parity" }),
        harness
            .activate_agent_account(json!({ "harness": "HARNESS_ID_MOCK", "accountId": "parity" })),
        |r| from_msg(r.snapshot, "snapshot")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/ForgetAgentAccount",
        ForgetAgentAccount,
        |d| json!({ "harness": "mock", "accountId": "parity" }),
        harness
            .forget_agent_account(json!({ "harness": "HARNESS_ID_MOCK", "accountId": "parity" })),
        |r| from_msg(r.snapshot, "snapshot")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/StartAgentLogin",
        StartAgentLogin,
        |d| json!({ "harness": "mock" }),
        harness.start_agent_login(json!({ "harness": "HARNESS_ID_MOCK" })),
        |r| from_msg(r.login, "login")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/CompleteAgentLogin",
        CompleteAgentLogin,
        |d| json!({ "loginId": "parity", "code": "0000" }),
        harness.complete_agent_login(json!({ "loginId": "parity", "code": "0000" })),
        |r| from_msg(r.snapshot, "snapshot")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/PollAgentLogin",
        PollAgentLogin,
        |d| json!({ "loginId": "parity" }),
        harness.poll_agent_login(json!({ "loginId": "parity" })),
        |r| from_msg(r.poll, "poll")?
    );
    unary!(
        s,
        "/loams.agentd.v1.HarnessService/CancelAgentLogin",
        CancelAgentLogin,
        |d| json!({ "loginId": "parity" }),
        harness.cancel_agent_login(json!({ "loginId": "parity" })),
        |r| ok(r.ok)
    );
}

/// The scenario's ProjectService.
async fn projects(s: &mut Suite) {
    // ---- ProjectService ----
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/ListRepos",
        ListRepos,
        |d| json!({}),
        projects.list_repos(json!({})),
        |r| rs::RepoList(Wire::from_pb(r.repos)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/AddRepo",
        AddRepo,
        |d| json!({ "path": d.repo }),
        projects.add_repo(json!({ "path": d.repo })),
        |r| from_msg(r.repo, "repo")?
    );
    // Refused before git runs: an option, not a URL.
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/CloneRepo",
        CloneRepo,
        |d| json!({ "url": "--upload-pack=touch parity" }),
        projects.clone_repo(json!({ "url": "--upload-pack=touch parity" })),
        |r| from_msg(r.repo, "repo")?
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/CreateRepo",
        CreateRepo,
        |d| json!({ "name": "scratch" }),
        projects.create_repo(json!({ "name": "scratch" })),
        |r| from_msg(r.repo, "repo")?
    );
    first!(
        s,
        "/loams.agentd.v1.ProjectService/WatchProjects",
        WatchSpaces,
        |d| json!({}),
        projects.watch_projects(json!({})),
        |r| rs::SpaceList(Wire::from_pb(r.spaces)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/CreateProject",
        Mutate,
        |d| json!({ "op": "createSpace", "spaceId": "space-1", "deviceId": d.device,
                    "path": d.repo, "name": "parity", "gitDetected": true }),
        projects.create_project(json!({ "spaceId": "space-1", "deviceId": d.device,
                    "path": d.repo, "name": "parity", "gitDetected": true })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/RenameProject",
        Mutate,
        |d| json!({ "op": "renameSpace", "spaceId": "space-1", "name": "Parity" }),
        projects.rename_project(json!({ "spaceId": "space-1", "name": "Parity" })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/ListProjectActions",
        ListProjectActions,
        |d| json!({ "spaceId": "space-1" }),
        projects.list_project_actions(json!({ "spaceId": "space-1" })),
        |r| from_msg(r.snapshot, "snapshot")?
    );
    unary!(s, "/loams.agentd.v1.ProjectService/UpsertProjectAction", UpsertProjectAction,
        |d| json!({ "spaceId": "space-1", "actionId": "action-1",
                    "action": { "name": "Check", "command": "true", "icon": "test", "runOnWorktreeCreate": false } }),
        projects.upsert_project_action(json!({ "spaceId": "space-1", "actionId": "action-1",
                    "action": { "name": "Check", "command": "true", "icon": "PROJECT_ACTION_ICON_TEST", "runOnWorktreeCreate": false } })),
        |r| from_msg(r.snapshot, "snapshot")?);
    // An action that does not exist: nothing runs.
    unary!(s, "/loams.agentd.v1.ProjectService/RunProjectAction", RunProjectAction,
        |d| json!({ "spaceId": "space-1", "chatId": "chat-1", "actionId": "no-such-action", "cols": 80, "rows": 24 }),
        projects.run_project_action(json!({ "spaceId": "space-1", "chatId": "chat-1", "actionId": "no-such-action", "cols": 80, "rows": 24 })),
        |r| from_msg(r.run, "run")?);
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/TakeProjectActionSetup",
        TakeProjectActionSetup,
        |d| json!({ "chatId": "chat-1", "commandId": "no-such-command" }),
        projects.take_project_action_setup(
            json!({ "chatId": "chat-1", "commandId": "no-such-command" })
        ),
        |r| from_msg(r.setup, "setup")?
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/DeleteProjectAction",
        DeleteProjectAction,
        |d| json!({ "spaceId": "space-1", "actionId": "action-1" }),
        projects.delete_project_action(json!({ "spaceId": "space-1", "actionId": "action-1" })),
        |r| from_msg(r.snapshot, "snapshot")?
    );
}

/// The scenario's ThreadService, through chat-1's turns and checkpoints.
async fn threads(s: &mut Suite) {
    // ---- ThreadService ----
    first!(
        s,
        "/loams.agentd.v1.ThreadService/WatchThreads",
        WatchChats,
        |d| json!({}),
        threads.watch_threads(json!({})),
        |r| rs::ChatList(Wire::from_pb(r.chats)?)
    );
    first!(
        s,
        "/loams.agentd.v1.ThreadService/WatchSessions",
        WatchSessions,
        |d| json!({}),
        threads.watch_sessions(json!({})),
        |r| rs::SessionList(Wire::from_pb(r.sessions)?)
    );
    first!(
        s,
        "/loams.agentd.v1.ThreadService/WatchSidebarPreferences",
        WatchSidebarPreferences,
        |d| json!({}),
        threads.watch_sidebar_preferences(json!({})),
        |r| from_msg(r.preferences, "preferences")?
    );
    unary!(s, "/loams.agentd.v1.ThreadService/CreateThread", Mutate,
        |d| json!({ "op": "createChat", "chatId": "chat-1", "spaceId": "space-1",
                    "config": { "harness": "mock", "sandbox": "workspace-write", "modelOptions": {} } }),
        threads.create_thread(json!({ "chatId": "chat-1", "spaceId": "space-1",
                    "config": { "harness": "HARNESS_ID_MOCK", "sandbox": "SANDBOX_LEVEL_WORKSPACE_WRITE" } })),
        |r| mutated(r.ok));
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/RenameThread",
        Mutate,
        |d| json!({ "op": "renameChat", "chatId": "chat-1", "title": "Parity" }),
        threads.rename_thread(json!({ "chatId": "chat-1", "title": "Parity" })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/SetThreadArchived",
        Mutate,
        |d| json!({ "op": "setChatArchived", "chatId": "chat-1", "archived": false }),
        threads.set_thread_archived(json!({ "chatId": "chat-1", "archived": false })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/ChangeSidebarPin",
        Mutate,
        |d| json!({ "op": "changeSidebarPin", "change": { "action": "pin", "sessionId": "chat-1" } }),
        threads.change_sidebar_pin(json!({ "change": { "pin": { "sessionId": "chat-1" } } })),
        |r| rs::MutateReply {
            ok: r.ok,
            sidebar_preferences: from_opt_msg(r.preferences)?
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/MarkThreadSeen",
        Mutate,
        |d| json!({ "op": "markChatSeen", "chatId": "chat-1", "at": 1_791_797_400_000_i64 }),
        threads.mark_thread_seen(json!({ "chatId": "chat-1", "atMs": "1791797400000" })),
        |r| mutated(r.ok)
    );
    unary!(s, "/loams.agentd.v1.ThreadService/SetThreadConfig", Mutate,
        |d| json!({ "op": "setChatConfig", "chatId": "chat-1",
                    "config": { "harness": "mock", "model": "mock-1", "sandbox": "read-only", "modelOptions": {} } }),
        threads.set_thread_config(json!({ "chatId": "chat-1",
                    "config": { "harness": "HARNESS_ID_MOCK", "model": "mock-1", "sandbox": "SANDBOX_LEVEL_READ_ONLY" } })),
        |r| mutated(r.ok));
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/SetThreadBranch",
        Mutate,
        |d| json!({ "op": "setChatBranch", "chatId": "chat-1", "branch": "main" }),
        threads.set_thread_branch(json!({ "chatId": "chat-1", "branch": "main" })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/SetThreadCwd",
        Mutate,
        |d| json!({ "op": "setChatCwd", "chatId": "chat-1", "cwd": d.repo }),
        threads.set_thread_cwd(json!({ "chatId": "chat-1", "cwd": d.repo })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/ForkThread",
        ForkSideChat,
        |d| json!({ "chatId": "chat-2", "sourceChatId": "chat-1" }),
        threads.fork_thread(json!({ "chatId": "chat-2", "sourceChatId": "chat-1" })),
        |r| from_msg(r.chat, "chat")?
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.ThreadService/Send",
        QueueCommand,
        |d| json!({ "chatId": "chat-1", "command": { "kind": "run", "messageId": "msg-1", "request": {
                    "prompt": "Hello", "harness": "mock", "model": null, "reasoning": null,
                    "modelOptions": {}, "cwd": d.repo, "sandbox": "workspace-write",
                    "autoApprove": false, "resume": null } } }),
        threads.send(
            json!({ "chatId": "chat-1", "messageId": "msg-1", "request": {
                    "prompt": "Hello", "harness": "HARNESS_ID_MOCK", "cwd": d.repo,
                    "sandbox": "SANDBOX_LEVEL_WORKSPACE_WRITE" } })
        ),
        |r| command(r.command_id)
    );
    assert!(
        a["commandId"].is_string() && b["commandId"].is_string(),
        "{a} {b}"
    );
    settled_transcripts(s, "chat-1").await;
    // Usage and notifications (G19, G20; SD1 Task 14): chat-1's turn, once
    // each daemon has recorded its end.
    for daemon in [&s.a, &s.b] {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let usage = daemon
                .ndjson
                .call(calls::GetThreadUsage::NAME, json!({ "chatId": "chat-1" }))
                .await
                .unwrap();
            if usage["turns"].as_array().is_some_and(|t| !t.is_empty()) {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "chat-1's usage");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/GetThreadUsage",
        GetThreadUsage,
        |d| json!({ "chatId": "chat-1" }),
        threads.get_thread_usage(json!({ "chatId": "chat-1" })),
        |r| Wire::from_pb(r)?
    );
    // The oldest notification held: chat-1's turn finished.
    first!(
        s,
        "/loams.agentd.v1.NotificationService/WatchNotifications",
        WatchNotifications,
        |d| json!({}),
        notifications.watch_notifications(json!({})),
        |r| from_msg(r.notification, "notification")?
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/Steer",
        QueueCommand,
        |d| json!({ "chatId": "chat-1", "command": { "kind": "steer", "prompt": "Also this", "messageId": "msg-2" } }),
        threads.steer(json!({ "chatId": "chat-1", "prompt": "Also this", "messageId": "msg-2" })),
        |r| command(r.command_id)
    );
    // chat-1 was idle, so Steer started a second turn. Both daemons finish it
    // before the Interrupt: an Interrupt that reaches a turn before its run
    // starts ends it with no checkpoint, so one daemon would list turn 2's
    // and the other not.
    settle_thread(s, "chat-1").await;
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/Interrupt",
        QueueCommand,
        |d| json!({ "chatId": "chat-1", "command": { "kind": "interrupt" } }),
        threads.interrupt(json!({ "chatId": "chat-1" })),
        |r| command(r.command_id)
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/AnswerQuestion",
        QueueCommand,
        |d| json!({ "chatId": "chat-1", "command": { "kind": "respondInput", "requestId": "no-such-request",
                    "answers": [{ "questionId": "q-1", "labels": ["yes"] }] } }),
        threads.answer_question(json!({ "chatId": "chat-1", "requestId": "no-such-request",
                    "answers": [{ "questionId": "q-1", "labels": ["yes"] }] })),
        |r| command(r.command_id)
    );

    // Checkpoints (G6, G7; SD1 Task 10): the list after chat-1's two turns,
    // and a restore of a turn it never had, so nothing is restored. Both
    // daemons have applied the Interrupt and AnswerQuestion first, or one can
    // refuse the restore as "running" while the other answers
    // `checkpoint_missing`.
    settle_thread(s, "chat-1").await;
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/ListCheckpoints",
        ListCheckpoints,
        |d| json!({ "chatId": "chat-1" }),
        threads.list_checkpoints(json!({ "chatId": "chat-1" })),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/RestoreCheckpoint",
        RestoreCheckpoint,
        |d| json!({ "chatId": "chat-1", "turnId": "no-such-turn", "expectedChecksum": "stale",
                    "scope": "worktree" }),
        threads.restore_checkpoint(json!({ "chatId": "chat-1", "turnId": "no-such-turn",
                    "expectedChecksum": "stale", "scope": "CHECKPOINT_SCOPE_WORKTREE" })),
        |r| Wire::from_pb(r)?
    );
}

/// The scenario's QueueService.
async fn queue(s: &mut Suite) {
    // ---- QueueService ----
    first!(
        s,
        "/loams.agentd.v1.QueueService/WatchQueue",
        WatchQueue,
        |d| json!({ "chatId": "chat-2" }),
        queue.watch_queue(json!({ "chatId": "chat-2" })),
        |r| rs::QueueSnapshot {
            items: Wire::from_pb(r.items)?
        }
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.QueueService/QueueMessage",
        QueueMessage,
        |d| json!({ "chatId": "chat-2", "text": "Later", "attachments": [], "holdForTurnEnd": true }),
        queue.queue_message(json!({ "chatId": "chat-2", "text": "Later", "holdForTurnEnd": true })),
        |r| rs::IdReply { id: r.id }
    );
    s.learn("queued", &a["id"], &b["id"]);
    unary!(
        s,
        "/loams.agentd.v1.QueueService/UpdateQueuedMessage",
        UpdateQueuedMessage,
        |d| json!({ "chatId": "chat-2", "id": d.get("queued"), "text": "Later, edited" }),
        queue.update_queued_message(
            json!({ "chatId": "chat-2", "id": d.get("queued"), "text": "Later, edited" })
        ),
        |r| rs::ChangedReply { changed: r.changed }
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.QueueService/BeginQueuedMessageEdit",
        BeginQueuedMessageEdit,
        |d| json!({ "chatId": "chat-2", "id": d.get("queued"), "editorDeviceId": d.device, "editorInstanceId": "window-1" }),
        queue.begin_queued_message_edit(json!({ "chatId": "chat-2", "id": d.get("queued"),
                    "editorDeviceId": d.device, "editorInstanceId": "window-1" })),
        |r| from_msg(r.outcome, "outcome")?
    );
    s.learn("lease", &a["leaseId"], &b["leaseId"]);
    unary!(
        s,
        "/loams.agentd.v1.QueueService/RenewQueuedMessageEdit",
        RenewQueuedMessageEdit,
        |d| json!({ "chatId": "chat-2", "id": d.get("queued"), "leaseId": d.get("lease") }),
        queue.renew_queued_message_edit(
            json!({ "chatId": "chat-2", "id": d.get("queued"), "leaseId": d.get("lease") })
        ),
        |r| from_msg(r.outcome, "outcome")?
    );
    unary!(
        s,
        "/loams.agentd.v1.QueueService/FinishQueuedMessageEdit",
        FinishQueuedMessageEdit,
        |d| json!({ "chatId": "chat-2", "id": d.get("queued"), "leaseId": d.get("lease"), "action": "cancel" }),
        queue.finish_queued_message_edit(
            json!({ "chatId": "chat-2", "id": d.get("queued"), "leaseId": d.get("lease"),
                    "action": "FINISH_QUEUED_MESSAGE_EDIT_ACTION_CANCEL" })
        ),
        |r| from_msg(r.outcome, "outcome")?
    );
    unary!(
        s,
        "/loams.agentd.v1.QueueService/MoveQueuedMessage",
        MoveQueuedMessage,
        |d| json!({ "chatId": "chat-2", "id": d.get("queued"), "toIndex": 0 }),
        queue.move_queued_message(
            json!({ "chatId": "chat-2", "id": d.get("queued"), "toIndex": "0" })
        ),
        |r| rs::ChangedReply { changed: r.changed }
    );
    unary!(
        s,
        "/loams.agentd.v1.QueueService/SendQueuedMessageNow",
        SendQueuedMessageNow,
        |d| json!({ "chatId": "chat-2", "id": "no-such-message" }),
        queue.send_queued_message_now(json!({ "chatId": "chat-2", "id": "no-such-message" })),
        |r| rs::SentReply { sent: r.sent }
    );
    unary!(
        s,
        "/loams.agentd.v1.QueueService/SteerQueuedMessageNow",
        SteerQueuedMessageNow,
        |d| json!({ "chatId": "chat-2", "id": "no-such-message" }),
        queue.steer_queued_message_now(json!({ "chatId": "chat-2", "id": "no-such-message" })),
        |r| rs::SentReply { sent: r.sent }
    );
    unary!(
        s,
        "/loams.agentd.v1.QueueService/RemoveQueuedMessage",
        RemoveQueuedMessage,
        |d| json!({ "chatId": "chat-2", "id": d.get("queued") }),
        queue.remove_queued_message(json!({ "chatId": "chat-2", "id": d.get("queued") })),
        |r| rs::RemovedReply { removed: r.removed }
    );
    unary!(
        s,
        "/loams.agentd.v1.QueueService/RetryDelivery",
        RetryDelivery,
        |d| json!({ "chatId": "chat-2" }),
        queue.retry_delivery(json!({ "chatId": "chat-2" })),
        |r| rs::Empty {}
    );
}

/// The scenario's GitService and DiffService.
async fn git_and_diff(s: &mut Suite) {
    // ---- GitService ----
    unary!(
        s,
        "/loams.agentd.v1.GitService/ListBranches",
        ListBranches,
        |d| json!({ "repoPath": d.repo }),
        git.list_branches(json!({ "repoPath": d.repo })),
        |r| rs::BranchList(r.branches)
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/ListRefs",
        ListRefs,
        |d| json!({ "repoPath": d.repo }),
        git.list_refs(json!({ "repoPath": d.repo })),
        |r| rs::RepoRefList(Wire::from_pb(r.refs)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/ListGitHistory",
        ListGitHistory,
        |d| json!({ "cwd": d.repo, "cursor": 0, "limit": 10 }),
        git.list_git_history(json!({ "cwd": d.repo, "limit": "10" })),
        |r| from_msg(r.page, "page")?
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/SearchGitHistory",
        SearchGitHistory,
        |d| json!({ "cwd": d.repo, "query": "initial", "cursor": 0, "limit": 10 }),
        git.search_git_history(json!({ "cwd": d.repo, "query": "initial", "limit": "10" })),
        |r| from_msg(r.page, "page")?
    );
    // No authors: nothing is looked up on the network.
    unary!(
        s,
        "/loams.agentd.v1.GitService/ResolveGitAvatars",
        ResolveGitAvatars,
        |d| json!({ "cwd": d.repo, "authors": [], "cursor": 0, "limit": 10 }),
        git.resolve_git_avatars(json!({ "cwd": d.repo, "limit": "10" })),
        |r| rs::GitAvatars(r.avatars.into_iter().collect())
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/FetchAll",
        FetchAll,
        |d| json!({ "repoPath": d.repo }),
        git.fetch_all(json!({ "repoPath": d.repo })),
        |r| ok(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/SwitchRef",
        SwitchRef,
        |d| json!({ "repoPath": d.repo, "refName": "main" }),
        git.switch_ref(json!({ "repoPath": d.repo, "refName": "main" })),
        |r| rs::SwitchRefReply { branch: r.branch }
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.GitService/CreateWorktree",
        CreateWorktree,
        |d| json!({ "repoPath": d.repo, "branch": "parity-wt" }),
        git.create_worktree(json!({ "repoPath": d.repo, "branch": "parity-wt" })),
        |r| from_msg(r.outcome, "outcome")?
    );
    s.learn("checkout", &a["checkoutId"], &b["checkoutId"]);
    let (wt_a, wt_b) = (a["path"].clone(), b["path"].clone());
    s.a.facts
        .learnt
        .insert("worktree", wt_a.as_str().unwrap_or_default().into());
    s.b.facts
        .learnt
        .insert("worktree", wt_b.as_str().unwrap_or_default().into());
    unary!(
        s,
        "/loams.agentd.v1.GitService/DeleteWorktree",
        DeleteWorktree,
        |d| json!({ "repoPath": d.repo, "worktreePath": d.get("worktree") }),
        git.delete_worktree(json!({ "repoPath": d.repo, "worktreePath": d.get("worktree") })),
        |r| ok(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/ListWorktrees",
        ListWorktrees,
        |d| json!({ "repoPath": d.repo }),
        git.list_worktrees(json!({ "repoPath": d.repo })),
        |r| rs::WorktreeList(Wire::from_pb(r.worktrees)?)
    );
    // The git writes (G2–G5, G21), each refused by its precondition: a stale
    // checksum or head, or a repository with no remote. Nothing is written
    // and nothing reaches the network.
    unary!(
        s,
        "/loams.agentd.v1.GitService/CommitWorktree",
        CommitWorktree,
        |d| json!({ "checkoutId": "no-such-checkout", "cwd": d.repo, "message": "parity",
                    "all": true, "expectedChecksum": "stale" }),
        git.commit_worktree(
            json!({ "checkoutId": "no-such-checkout", "cwd": d.repo, "message": "parity",
                    "all": true, "expectedChecksum": "stale" })
        ),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/PushBranch",
        PushBranch,
        |d| json!({ "cwd": d.repo, "branch": "main", "setUpstream": false, "forceWithLease": false }),
        git.push_branch(json!({ "cwd": d.repo, "branch": "main" })),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/CreateChangeRequest",
        CreateChangeRequest,
        |d| json!({ "cwd": d.repo, "provider": "github", "base": "main", "head": "parity-wt",
                    "title": "Parity" }),
        git.create_change_request(
            json!({ "cwd": d.repo, "provider": "CHANGE_REQUEST_PROVIDER_GITHUB",
                    "base": "main", "head": "parity-wt", "title": "Parity" })
        ),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.GitService/MergeBranch",
        MergeBranch,
        |d| json!({ "repoPath": d.repo, "source": "main", "target": "main", "strategy": "merge",
                    "expectedSourceHead": "0000000000000000000000000000000000000000" }),
        git.merge_branch(
            json!({ "repoPath": d.repo, "source": "main", "target": "main",
                    "strategy": "MERGE_STRATEGY_MERGE",
                    "expectedSourceHead": "0000000000000000000000000000000000000000" })
        ),
        |r| Wire::from_pb(r)?
    );
    first!(
        s,
        "/loams.agentd.v1.GitService/WatchStatus",
        WatchWorkspaceGitStatus,
        |d| json!({ "chatId": "chat-1" }),
        git.watch_status(json!({ "target": { "chatId": "chat-1" } })),
        |r| rs::WorkspaceGitStatusFrame {
            status: from_opt_msg(r.status)?
        }
    );
    first!(
        s,
        "/loams.agentd.v1.GitService/WatchChangeRequest",
        WatchCheckoutChangeRequest,
        |d| json!({ "cwd": d.repo, "branch": "main" }),
        git.watch_change_request(json!({ "cwd": d.repo, "branch": "main" })),
        |r| from_msg(r.status, "status")?
    );

    // ---- DiffService ----
    first!(
        s,
        "/loams.agentd.v1.DiffService/WatchCheckoutDiffs",
        WatchCheckoutDiffs,
        |d| json!({}),
        diff.watch_checkout_diffs(json!({})),
        |r| rs::CheckoutDiffList(Wire::from_pb(r.diffs)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.DiffService/GetCheckoutDiff",
        GetCheckoutDiff,
        |d| json!({ "cwd": d.repo, "mode": "branch", "baseRef": "main" }),
        diff.get_checkout_diff(json!({ "cwd": d.repo, "mode": "branch", "baseRef": "main" })),
        |r| from_msg(r.diff, "diff")?
    );
    unary!(s, "/loams.agentd.v1.DiffService/GetCheckoutFileDiffText", GetCheckoutFileDiffText,
        |d| json!({ "checkoutId": "no-such-checkout", "cwd": d.repo, "path": "README.md",
                    "mode": "branch", "baseRef": "main", "diffChecksum": "stale" }),
        diff.get_checkout_file_diff_text(json!({ "checkoutId": "no-such-checkout", "cwd": d.repo,
                    "path": "README.md", "mode": "branch", "baseRef": "main", "diffChecksum": "stale" })),
        |r| from_msg(r.text, "text")?);
    // A stale checksum: nothing is discarded.
    unary!(s, "/loams.agentd.v1.DiffService/Discard", DiscardWorkingTree,
        |d| json!({ "chatId": "chat-1", "checkoutId": "no-such-checkout", "expectedChecksum": "stale" }),
        diff.discard(json!({ "chatId": "chat-1", "checkoutId": "no-such-checkout", "expectedChecksum": "stale" })),
        |r| rs::DiscardWorkingTreeReply { ok: r.ok, checksum: r.checksum });

    unary!(
        s,
        "/loams.agentd.v1.DiffService/DiscardPaths",
        DiscardPaths,
        |d| json!({ "checkoutId": "no-such-checkout", "paths": ["README.md"], "expectedChecksum": "stale" }),
        diff.discard_paths(
            json!({ "checkoutId": "no-such-checkout", "paths": ["README.md"],
                    "expectedChecksum": "stale" })
        ),
        |r| rs::Empty {}
    );
}

/// The scenario's FileService, PreviewService and AttachmentService.
async fn files_previews_attachments(s: &mut Suite) {
    // ---- FileService ----
    unary!(
        s,
        "/loams.agentd.v1.FileService/ListFolders",
        ListFolders,
        |d| json!({ "path": d.root }),
        files.list_folders(json!({ "path": d.root })),
        |r| from_msg(r.listing, "listing")?
    );
    unary!(
        s,
        "/loams.agentd.v1.FileService/ListDrives",
        ListDrives,
        |d| json!({}),
        files.list_drives(json!({})),
        |r| rs::DriveListing {
            drives: Wire::from_pb(r.drives)?
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.FileService/SearchMentions",
        SearchFiles,
        |d| json!({ "query": "main", "chatId": "chat-1" }),
        files.search_mentions(json!({ "query": "main", "chatId": "chat-1" })),
        |r| rs::FileSearchMatchList(Wire::from_pb(r.matches)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.FileService/Search",
        SearchWorkspaceFiles,
        |d| json!({ "chatId": "chat-1", "query": "main", "includeIgnored": false, "limit": 20 }),
        files.search(json!({ "target": { "chatId": "chat-1" }, "query": "main", "limit": 20 })),
        |r| rs::WorkspaceFileSearchMatchList(Wire::from_pb(r.matches)?)
    );
    unary!(
        s,
        "/loams.agentd.v1.FileService/ListDirectory",
        ListWorkspaceDirectory,
        |d| json!({ "chatId": "chat-1", "directory": "", "includeIgnored": false }),
        files.list_directory(json!({ "target": { "chatId": "chat-1" }, "directory": "" })),
        |r| from_msg(r.page, "page")?
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.FileService/ReadFile",
        ReadWorkspaceFile,
        |d| json!({ "chatId": "chat-1", "path": "README.md" }),
        files.read_file(json!({ "target": { "chatId": "chat-1" }, "path": "README.md" })),
        |r| from_msg(r.file, "file")?
    );
    s.learn("file-checkout", &a["checkoutId"], &b["checkoutId"]);
    let hash = a["contentHash"].as_str().unwrap_or_default().to_owned();
    assert_eq!(b["contentHash"].as_str().unwrap_or_default(), hash);
    unary!(
        s,
        "/loams.agentd.v1.FileService/ReadImage",
        ReadWorkspaceImage,
        |d| json!({ "chatId": "chat-1", "path": "README.md", "expectedCheckoutId": d.get("file-checkout"),
                    "expectedContentHash": null, "offset": 0 }),
        files.read_image(
            json!({ "target": { "chatId": "chat-1" }, "path": "README.md",
                    "expectedCheckoutId": d.get("file-checkout") })
        ),
        |r| from_msg(r.chunk, "chunk")?
    );
    unary!(s, "/loams.agentd.v1.FileService/WriteFile", WriteWorkspaceFile,
        |d| json!({ "chatId": "chat-1", "expectedCheckoutId": d.get("file-checkout"), "path": "README.md",
                    "text": "# Parity, written\n", "expectedContentHash": hash, "encoding": "utf8", "lineEnding": "lf" }),
        files.write_file(json!({ "target": { "chatId": "chat-1" }, "expectedCheckoutId": d.get("file-checkout"),
                    "path": "README.md", "text": "# Parity, written\n", "expectedContentHash": hash,
                    "encoding": "WORKSPACE_WRITABLE_ENCODING_UTF8", "lineEnding": "WORKSPACE_WRITABLE_LINE_ENDING_LF" })),
        |r| from_msg(r.outcome, "outcome")?);
    // A stale revision: nothing moves or goes.
    unary!(
        s,
        "/loams.agentd.v1.FileService/MoveEntry",
        MoveWorkspaceEntry,
        |d| json!({ "chatId": "chat-1", "operationId": "op-1", "expectedCheckoutId": d.get("file-checkout"),
                    "sourcePath": "src/main.rs", "destinationPath": "src/lib.rs",
                    "expectedSourceRevision": "stale", "expectedKind": "file" }),
        files.move_entry(
            json!({ "target": { "chatId": "chat-1" }, "operationId": "op-1",
                    "expectedCheckoutId": d.get("file-checkout"), "sourcePath": "src/main.rs",
                    "destinationPath": "src/lib.rs", "expectedSourceRevision": "stale",
                    "expectedKind": "WORKSPACE_ENTRY_KIND_FILE" })
        ),
        |r| from_msg(r.outcome, "outcome")?
    );
    unary!(s, "/loams.agentd.v1.FileService/DeleteEntry", DeleteWorkspaceEntry,
        |d| json!({ "chatId": "chat-1", "operationId": "op-2", "expectedCheckoutId": d.get("file-checkout"),
                    "path": "src", "expectedSourceRevision": "stale", "expectedKind": "directory", "recursive": true }),
        files.delete_entry(json!({ "target": { "chatId": "chat-1" }, "operationId": "op-2",
                    "expectedCheckoutId": d.get("file-checkout"), "path": "src", "expectedSourceRevision": "stale",
                    "expectedKind": "WORKSPACE_ENTRY_KIND_DIRECTORY", "recursive": true })),
        |r| from_msg(r.outcome, "outcome")?);
    first!(
        s,
        "/loams.agentd.v1.FileService/WatchFiles",
        WatchWorkspaceFiles,
        |d| json!({ "chatId": "chat-1" }),
        files.watch_files(json!({ "target": { "chatId": "chat-1" } })),
        |r| from_msg(r.changes, "changes")?
    );

    // ---- PreviewService ----
    // The preview proxy's port is fixed (7331), and both daemons run in this
    // one process: only one of them can listen on it.
    s.also = &["error"];
    first!(
        s,
        "/loams.agentd.v1.PreviewService/WatchPreviews",
        WatchPreviews,
        |d| json!({ "chatId": "chat-1" }),
        preview.watch_previews(json!({ "chatId": "chat-1" })),
        |r| from_msg(r.snapshot, "snapshot")?
    );

    // ---- AttachmentService ----
    unary!(
        s,
        "/loams.agentd.v1.AttachmentService/UploadChunk",
        UploadChunk,
        |d| json!({ "uploadId": "upload-1", "seq": 0, "data": "iVBORw0KGgo=" }),
        attachments
            .upload_chunk(json!({ "uploadId": "upload-1", "seq": "0", "data": "iVBORw0KGgo=" })),
        |r| ok(r.ok)
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.AttachmentService/UploadCommit",
        UploadCommit,
        |d| json!({ "uploadId": "upload-1", "fileName": "shot.png" }),
        attachments.upload_commit(json!({ "uploadId": "upload-1", "fileName": "shot.png" })),
        |r| rs::UploadCommitReply { path: r.path }
    );
    s.learn("upload", &a["path"], &b["path"]);
    unary!(
        s,
        "/loams.agentd.v1.AttachmentService/ReadAttachmentChunk",
        ReadAttachmentChunk,
        |d| json!({ "path": d.get("upload"), "offset": 0 }),
        attachments.read_attachment_chunk(json!({ "path": d.get("upload") })),
        |r| from_msg(r.chunk, "chunk")?
    );
    unary!(
        s,
        "/loams.agentd.v1.AttachmentService/FetchToolBlob",
        FetchToolBlob,
        |d| json!({ "blobRef": "chat-1/no-such-call", "offset": 0 }),
        attachments.fetch_tool_blob(json!({ "blobRef": "chat-1/no-such-call" })),
        |r| from_msg(r.chunk, "chunk")?
    );
}

/// The scenario's TerminalService.
async fn terminals(s: &mut Suite) {
    // ---- TerminalService ----
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.TerminalService/OpenTerminal",
        OpenTerminal,
        |d| json!({ "chatId": "chat-1", "cols": 80, "rows": 24 }),
        terminals.open_terminal(json!({ "chatId": "chat-1", "cols": 80, "rows": 24 })),
        |r| from_msg(r.terminal, "terminal")?
    );
    s.learn("terminal", &a["id"], &b["id"]);
    first!(
        s,
        "/loams.agentd.v1.TerminalService/SubscribeTerminal",
        SubscribeTerminal,
        |d| json!({ "terminalId": d.get("terminal") }),
        terminals.subscribe_terminal(json!({ "terminalId": d.get("terminal") })),
        |r| from_msg(r.event, "event")?
    );
    unary!(
        s,
        "/loams.agentd.v1.TerminalService/WriteTerminal",
        WriteTerminal,
        |d| json!({ "terminalId": d.get("terminal"), "data": "dHJ1ZQo=" }),
        terminals.write_terminal(json!({ "terminalId": d.get("terminal"), "data": "dHJ1ZQo=" })),
        |r| ok(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.TerminalService/ResizeTerminal",
        ResizeTerminal,
        |d| json!({ "terminalId": d.get("terminal"), "cols": 100, "rows": 30 }),
        terminals
            .resize_terminal(json!({ "terminalId": d.get("terminal"), "cols": 100, "rows": 30 })),
        |r| ok(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.TerminalService/CloseTerminal",
        CloseTerminal,
        |d| json!({ "terminalId": d.get("terminal") }),
        terminals.close_terminal(json!({ "terminalId": d.get("terminal") })),
        |r| ok(r.ok)
    );
}

/// The scenario's ReviewService and FileService.SearchContent.
async fn review_and_search(s: &mut Suite) {
    // ---- ReviewService (G8–G11; SD1 Task 11) ----
    // A comment on chat-1's checkout, on the README line WriteFile added.
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.ReviewService/UpsertReviewComment",
        UpsertReviewComment,
        |d| json!({ "checkoutId": d.get("file-checkout"), "path": "README.md", "side": "new",
                    "line": 1, "body": "Say \"why\".", "diffChecksum": "sum-parity" }),
        review.upsert_review_comment(
            json!({ "comment": { "checkoutId": d.get("file-checkout"), "path": "README.md",
                    "side": "DIFF_SIDE_NEW", "line": 1, "body": "Say \"why\".",
                    "diffChecksum": "sum-parity" } })
        ),
        |r| rs::IdReply { id: r.id }
    );
    s.learn("comment", &a["id"], &b["id"]);
    // `outdated` follows each daemon's own checkout-diff sync, which may not
    // have caught WriteFile's edit yet on one of them (seen flaking on dev's
    // case); Task 11's tests cover the flag itself.
    s.also = &["outdated"];
    first!(
        s,
        "/loams.agentd.v1.ReviewService/WatchReviewComments",
        WatchReviewComments,
        |d| json!({ "checkoutId": d.get("file-checkout") }),
        review.watch_review_comments(json!({ "checkoutId": d.get("file-checkout") })),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.ReviewService/ResolveReviewComment",
        ResolveReviewComment,
        |d| json!({ "checkoutId": d.get("file-checkout"), "id": d.get("comment"), "resolved": true }),
        review.resolve_review_comment(
            json!({ "checkoutId": d.get("file-checkout"), "id": d.get("comment"), "resolved": true })
        ),
        |r| rs::IdReply { id: r.id }
    );
    // An id the thread's checkout doesn't have: nothing is queued.
    unary!(
        s,
        "/loams.agentd.v1.ReviewService/SendReviewToThread",
        SendReviewToThread,
        |d| json!({ "chatId": "chat-1", "commentIds": ["no-such-comment"] }),
        review.send_review_to_thread(
            json!({ "chatId": "chat-1", "commentIds": ["no-such-comment"] })
        ),
        |r| command(r.command_id)
    );
    // The repository has no remote: nothing is posted.
    unary!(
        s,
        "/loams.agentd.v1.ReviewService/PostReviewToChangeRequest",
        PostReviewToChangeRequest,
        |d| json!({ "checkoutId": d.get("file-checkout"), "commentIds": [d.get("comment")],
                    "event": "comment" }),
        review.post_review_to_change_request(
            json!({ "checkoutId": d.get("file-checkout"), "commentIds": [d.get("comment")],
                    "event": "REVIEW_EVENT_COMMENT" })
        ),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.ReviewService/DeleteReviewComment",
        DeleteReviewComment,
        |d| json!({ "checkoutId": d.get("file-checkout"), "id": d.get("comment") }),
        review.delete_review_comment(
            json!({ "checkoutId": d.get("file-checkout"), "id": d.get("comment") })
        ),
        |r| rs::IdReply { id: r.id }
    );

    // ---- FileService.SearchContent (G23; SD1 Task 11) ----
    first!(
        s,
        "/loams.agentd.v1.FileService/SearchContent",
        SearchWorkspaceContent,
        |d| json!({ "spaceId": "space-1", "query": "parity, WRITTEN" }),
        files.search_content(json!({ "spaceId": "space-1", "query": "parity, WRITTEN" })),
        |r| Wire::from_pb(r)?
    );
}

/// The scenario's RunGroupService.
async fn run_groups(s: &mut Suite) {
    // ---- RunGroupService (G12–G16; SD1 Task 12) ----
    // Two Mock members in their own worktrees (under the scratch worktree
    // root); the Mock edits nothing here, so the comparison is empty. The
    // worktree names, the members' progress and their stats move on each
    // daemon's own clock.
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.RunGroupService/CreateRunGroup",
        CreateRunGroup,
        |d| json!({ "spaceId": "space-1", "repoPath": d.repo, "base": "main",
                    "prompt": "Parity", "variants": [{ "harness": "mock" }, { "harness": "mock" }],
                    "approvalMode": "full-access" }),
        groups.create_run_group(json!({ "spaceId": "space-1", "repoPath": d.repo, "base": "main",
                    "prompt": "Parity",
                    "variants": [{ "harness": "HARNESS_ID_MOCK" }, { "harness": "HARNESS_ID_MOCK" }],
                    "approvalMode": "APPROVAL_MODE_FULL_ACCESS" })),
        |r| Wire::from_pb(r)?
    );
    s.learn("group", &a["groupId"], &b["groupId"]);
    s.learn("member-1", &a["chatIds"][0], &b["chatIds"][0]);
    s.learn("member-2", &a["chatIds"][1], &b["chatIds"][1]);
    s.also = &[
        "cwd",
        "branch",
        "status",
        "state",
        "filesChanged",
        "additions",
        "deletions",
        "startedAtMs",
    ];
    first!(
        s,
        "/loams.agentd.v1.RunGroupService/WatchRunGroups",
        WatchRunGroups,
        |d| json!({ "spaceId": "space-1" }),
        groups.watch_run_groups(json!({ "spaceId": "space-1" })),
        |r| Wire::from_pb(r)?
    );
    s.also = &["cwd"];
    unary!(
        s,
        "/loams.agentd.v1.RunGroupService/GetRunGroupComparison",
        GetRunGroupComparison,
        |d| json!({ "groupId": d.get("group"), "a": d.get("member-1"), "b": d.get("member-2") }),
        groups.get_run_group_comparison(
            json!({ "groupId": d.get("group"), "a": d.get("member-1"), "b": d.get("member-2") })
        ),
        |r| Wire::from_pb(r)?
    );
    // No check action: refused alike.
    unary!(
        s,
        "/loams.agentd.v1.RunGroupService/RunGroupCheck",
        RunGroupCheck,
        |d| json!({ "groupId": d.get("group") }),
        groups.run_group_check(json!({ "groupId": d.get("group") })),
        |r| Wire::from_pb(r)?
    );
    // `keep` with `force`: no git write, whatever the members' progress.
    unary!(
        s,
        "/loams.agentd.v1.RunGroupService/ResolveRunGroup",
        ResolveRunGroup,
        |d| json!({ "groupId": d.get("group"), "winnerChatId": d.get("member-1"),
                    "action": "keep", "force": true }),
        groups.resolve_run_group(json!({ "groupId": d.get("group"),
                    "winnerChatId": d.get("member-1"), "action": "RESOLVE_ACTION_KEEP",
                    "force": true })),
        |r| Wire::from_pb(r)?
    );
}

/// The scenario's ApprovalService, then tearing down what the scenario made.
async fn approvals_and_teardown(s: &mut Suite) {
    // ---- ApprovalService (G17, G18, G22; SD1 Task 13) ----
    // chat-2 becomes supervised (it runs nothing more); its watch says so,
    // with nothing pending, and answering an approval that does not wait is
    // refused alike (`approval_not_pending`).
    s.also = &[];
    unary!(
        s,
        "/loams.agentd.v1.ApprovalService/SetThreadApprovalMode",
        SetThreadApprovalMode,
        |d| json!({ "chatId": "chat-2", "mode": "supervised" }),
        approvals.set_thread_approval_mode(
            json!({ "chatId": "chat-2", "mode": "APPROVAL_MODE_SUPERVISED" })
        ),
        |r| rs::OkReply::OK
    );
    first!(
        s,
        "/loams.agentd.v1.ApprovalService/WatchApprovals",
        WatchApprovals,
        |d| json!({ "chatId": "chat-2" }),
        approvals.watch_approvals(json!({ "chatId": "chat-2" })),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.ApprovalService/ResolveApproval",
        ResolveApproval,
        |d| json!({ "callId": "call-none", "chatId": "chat-2", "decision": "once" }),
        approvals.resolve_approval(json!({ "callId": "call-none", "chatId": "chat-2",
                    "decision": "APPROVAL_DECISION_ONCE" })),
        |r| rs::OkReply::OK
    );

    // ---- Tear down what the scenario made ----
    unary!(
        s,
        "/loams.agentd.v1.ThreadService/DeleteThread",
        Mutate,
        |d| json!({ "op": "deleteChat", "chatId": "chat-2" }),
        threads.delete_thread(json!({ "chatId": "chat-2" })),
        |r| mutated(r.ok)
    );
    unary!(
        s,
        "/loams.agentd.v1.ProjectService/DeleteProject",
        Mutate,
        |d| json!({ "op": "deleteSpace", "spaceId": "space-1" }),
        projects.delete_project(json!({ "spaceId": "space-1" })),
        |r| mutated(r.ok)
    );
}

/// The scenario's MarketplaceService (MK1 Task 4b), over each daemon's own
/// testkit world.
async fn marketplace(s: &mut Suite) {
    use loams_agentd_marketplace::testkit::HARNESS_ID;
    use loams_agentd_proto::marketplace as rs_market;
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/ListSources",
        ListMarketplaceSources,
        |d| json!({}),
        market.list_sources(json!({})),
        |r| Wire::from_pb(r)?
    );
    // A plain-http index is refused alike.
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/AddSource",
        AddMarketplaceSource,
        |d| json!({ "name": "plain", "url": "http://example.org/index.json", "identity": "x" }),
        market.add_source(
            json!({ "name": "plain", "url": "http://example.org/index.json", "identity": "x" })
        ),
        |r| from_msg(r.source, "source")?
    );
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/RemoveSource",
        RemoveMarketplaceSource,
        |d| json!({ "name": "nowhere" }),
        market.remove_source(json!({ "name": "nowhere" })),
        |r| ok(true)
    );
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/SearchListings",
        SearchListings,
        |d| json!({ "kinds": ["harness"], "tiers": ["community"] }),
        market.search_listings(
            json!({ "kinds": ["KIND_HARNESS"], "tiers": ["LISTING_TIER_COMMUNITY"] })
        ),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/GetListing",
        GetListing,
        |d| json!({ "id": "ostrium/github-mcp-server" }),
        market.get_listing(json!({ "id": "ostrium/github-mcp-server" })),
        |r| Wire::from_pb(r)?
    );
    let (a, b) = unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/PlanInstall",
        PlanInstall,
        |d| json!({ "id": HARNESS_ID, "version": "1.0.0" }),
        market.plan_install(json!({ "id": HARNESS_ID, "version": "1.0.0" })),
        |r| Wire::from_pb(r)?
    );
    s.learn("digest", &a["plan"]["digest"], &b["plan"]["digest"]);
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/Install",
        InstallListing,
        |d| json!({ "id": HARNESS_ID, "version": "1.0.0", "planDigest": d.get("digest") }),
        market.install(
            json!({ "id": HARNESS_ID, "version": "1.0.0", "planDigest": d.get("digest") })
        ),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/ListInstalled",
        ListInstalledListings,
        |d| json!({}),
        market.list_installed(json!({})),
        |r| Wire::from_pb(r)?
    );
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/CheckUpdates",
        CheckListingUpdates,
        |d| json!({}),
        market.check_updates(json!({})),
        |r| Wire::from_pb(r)?
    );
    // 1.1.0 reaches a new host: without its plan's digest, refused alike.
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/Update",
        UpdateListing,
        |d| json!({ "id": HARNESS_ID }),
        market.update(json!({ "id": HARNESS_ID })),
        |r| rs_market::InstalledListingReply {
            installed: from_msg(r.installed, "installed")?
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.MarketplaceService/Uninstall",
        UninstallListing,
        |d| json!({ "id": HARNESS_ID }),
        market.uninstall(json!({ "id": HARNESS_ID })),
        |r| ok(true)
    );
}

fn factory_health(
    h: buffa::EnumValue<pb::FactoryHealth>,
) -> Result<rs::factory::FactoryHealth, MapError> {
    use rs::factory::FactoryHealth as H;
    match h {
        buffa::EnumValue::Known(pb::FactoryHealth::FACTORY_HEALTH_OK) => Ok(H::Ok),
        buffa::EnumValue::Known(pb::FactoryHealth::FACTORY_HEALTH_AUTH_FAILED) => Ok(H::AuthFailed),
        buffa::EnumValue::Known(pb::FactoryHealth::FACTORY_HEALTH_UNREACHABLE) => {
            Ok(H::Unreachable)
        }
        buffa::EnumValue::Known(pb::FactoryHealth::FACTORY_HEALTH_UNCONFIGURED) => {
            Ok(H::Unconfigured)
        }
        buffa::EnumValue::Known(pb::FactoryHealth::FACTORY_HEALTH_LOCKED) => Ok(H::Locked),
        other => Err(MapError::enum_value("FactoryHealth", other.to_i32())),
    }
}

/// A `FactoryApp` back to the ndjson type: an unconfigured app has no `fields`.
fn factory_app(a: pb::FactoryApp) -> Result<rs::factory::FactoryAppInfo, MapError> {
    let configured = a.url.is_some();
    Ok(rs::factory::FactoryAppInfo {
        id: a.id,
        label: a.label,
        url: a.url,
        health: factory_health(a.health)?,
        has_panels: a.has_panels,
        credential_fields: a
            .credential_fields
            .into_iter()
            .map(|f| rs::factory::FactoryCredentialField {
                key: f.key,
                label: f.label,
                secret: f.secret,
            })
            .collect(),
        fields: configured.then(|| a.fields.into_iter().collect()),
        persistent: a.persistent,
        preview: a.preview,
    })
}

fn factory_error(e: pb::FactoryError) -> rs::factory::FactoryError {
    rs::factory::FactoryError {
        code: e.code,
        message: e.message,
    }
}

fn factory_app_reply(
    r: Option<pb::__buffa::oneof::test_factory_app_response::Result>,
) -> Result<rs::factory::FactoryAppReply, MapError> {
    use pb::__buffa::oneof::test_factory_app_response::Result as R;
    match r {
        Some(R::App(a)) => Ok(rs::factory::FactoryAppReply {
            app: Some(factory_app(*a)?),
            error: None,
        }),
        Some(R::Error(e)) => Ok(rs::factory::FactoryAppReply {
            app: None,
            error: Some(factory_error(*e)),
        }),
        None => Err(MapError::empty_oneof("TestFactoryAppResponse.result")),
    }
}

fn factory_query_reply(
    r: Option<pb::__buffa::oneof::query_factory_response::Result>,
) -> Result<rs::factory::FactoryQueryReply, MapError> {
    use pb::__buffa::oneof::query_factory_response::Result as R;
    match r {
        Some(R::ValueJson(v)) => Ok(rs::factory::FactoryQueryReply {
            value: Some(serde_json::from_str(&v).map_err(|e| MapError(e.to_string()))?),
            error: None,
        }),
        Some(R::Error(e)) => Ok(rs::factory::FactoryQueryReply {
            value: None,
            error: Some(factory_error(*e)),
        }),
        None => Err(MapError::empty_oneof("QueryFactoryResponse.result")),
    }
}

/// The factory (SF1 amendment F2): both daemons start with nothing configured.
async fn factory_steps(s: &mut Suite) {
    unary!(
        s,
        "/loams.agentd.v1.FactoryService/ListFactoryApps",
        FactoryList,
        |d| json!({}),
        factory.list_factory_apps(json!({})),
        |r| rs::factory::FactoryAppList {
            apps: r
                .apps
                .into_iter()
                .map(factory_app)
                .collect::<Result<_, _>>()?,
        }
    );
    unary!(
        s,
        "/loams.agentd.v1.FactoryService/TestFactoryApp",
        FactoryTest,
        |d| json!({ "app": "forgejo" }),
        factory.test_factory_app(json!({ "app": "forgejo" })),
        |r| factory_app_reply(r.result)?
    );
    unary!(
        s,
        "/loams.agentd.v1.FactoryService/QueryFactory",
        FactoryQuery,
        |d| json!({ "app": "forgejo", "op": "repos", "params": { "limit": 20 } }),
        factory.query_factory(
            json!({ "app": "forgejo", "op": "repos", "paramsJson": "{\"limit\":20}" })
        ),
        |r| factory_query_reply(r.result)?
    );
    // An unreachable SigNoz: configured, then checked, then forgotten.
    unary!(
        s,
        "/loams.agentd.v1.FactoryService/ConfigureFactoryApp",
        FactoryConfigure,
        |d| json!({ "app": "signoz", "url": "http://127.0.0.1:9/web/", "fields": {} }),
        factory.configure_factory_app(
            json!({ "app": "signoz", "url": "http://127.0.0.1:9/web/", "fields": {} })
        ),
        |r| factory_configure_reply(r.result)?
    );
    unary!(
        s,
        "/loams.agentd.v1.FactoryService/RemoveFactoryApp",
        FactoryRemove,
        |d| json!({ "app": "signoz" }),
        factory.remove_factory_app(json!({ "app": "signoz" })),
        |r| rs::Empty {}
    );
}

fn factory_configure_reply(
    r: Option<pb::__buffa::oneof::configure_factory_app_response::Result>,
) -> Result<rs::factory::FactoryAppReply, MapError> {
    use pb::__buffa::oneof::configure_factory_app_response::Result as R;
    match r {
        Some(R::App(a)) => Ok(rs::factory::FactoryAppReply {
            app: Some(factory_app(*a)?),
            error: None,
        }),
        Some(R::Error(e)) => Ok(rs::factory::FactoryAppReply {
            app: None,
            error: Some(factory_error(*e)),
        }),
        None => Err(MapError::empty_oneof("ConfigureFactoryAppResponse.result")),
    }
}
