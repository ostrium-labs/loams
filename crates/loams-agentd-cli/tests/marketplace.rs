//! MK1 Task 10: `loams marketplace` against a daemon with the signed fake
//! world (MK1 Task 4b's testkit). Each test starts the sessions engine with a
//! marketplace host, serves it on a loopback WebSocket with an owner token,
//! writes `agentd.json` and `agentd.token` as the daemon does, and runs the
//! command line through discovery, `Hello` and the ndjson methods.
#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::Arc;

use async_trait::async_trait;
use clap::Parser;
use loams_agentd_cli::connect;
use loams_agentd_cli::marketplace::{MarketplaceArgs, execute};
use loams_agentd_cli::term::Terminal;
use loams_agentd_marketplace::MarketplaceHost;
use loams_agentd_marketplace::testkit::{HARNESS_ID, MCP_ID, World};
use loams_agentd_proto::HarnessId;
use loams_agentd_proto::daemon::{DaemonMode, Discovery};
use loams_agentd_proto::marketplace::{
    InstalledListingList, ListingSearchResults, PlanInstallReply,
};
use loams_agentd_rpc::{
    Connections, Credentials, Identity, Policy, RpcClient, RpcError, RpcReply, RpcService,
    ScopedTokens, Secret, WsServer, methods,
};
use loams_agentd_sessions::{ApprovalSettings, EngineCore, HarnessRegistry};
use serde_json::json;

const TOKEN: &str = "cli-test-owner-token-0123456789abcdefghijk";

#[derive(Parser)]
#[command(name = "loams")]
enum Cli {
    Marketplace(MarketplaceArgs),
}

/// Answers `Hello` as the daemon does, with the marketplace feature.
struct WithHello(Arc<dyn RpcService>);

#[async_trait]
impl RpcService for WithHello {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method == methods::HELLO {
            return Ok(RpcReply::Value(json!({
                "protocol": 1, "version": "test", "buildSha": "unknown", "mode": "child",
                "pid": std::process::id(), "startedAtMs": 1, "role": "owner",
                "features": ["marketplace"],
            })));
        }
        self.0.handle(method, params).await
    }
}

struct Open(Arc<dyn RpcService>);

impl Connections for Open {
    fn open(&self, _: &Identity) -> Arc<dyn RpcService> {
        Arc::new(WithHello(self.0.clone()))
    }
}

struct Anything;

#[async_trait]
impl Policy for Anything {
    async fn authorize(
        &self,
        _: &Identity,
        _: &str,
        _: &serde_json::Value,
    ) -> Result<(), RpcError> {
        Ok(())
    }
}

struct Daemon {
    dir: tempfile::TempDir,
    _core: EngineCore,
    client: RpcClient,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn daemon() -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble_with_approvals(
        &dir.path().join("engine"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        ApprovalSettings::full_access_default(),
    )
    .unwrap();
    let world = World::new(&dir.path().join("data"));
    let host = MarketplaceHost::open(world.deps()).unwrap();
    assert!(host.refresh().await.is_empty());
    core.attach_marketplace(host);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = WsServer::new(
        Arc::new(Credentials::new(
            Secret::new(TOKEN),
            Arc::new(ScopedTokens::new()),
        )),
        Arc::new(Open(core.rpc_service())),
        Arc::new(Anything),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(server.serve_until(listener, async {
        let _ = stopped.await;
    }));
    let discovery = Discovery {
        protocol: 1,
        version: "test".into(),
        pid: std::process::id(),
        port,
        started_at_ms: 1,
        mode: DaemonMode::Child,
        connect_port: None,
    };
    std::fs::write(
        dir.path().join("agentd.json"),
        serde_json::to_vec(&discovery).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.path().join("agentd.token"), format!("{TOKEN}\n")).unwrap();
    let client = connect::connect(dir.path()).await.unwrap().client;
    Daemon {
        dir,
        _core: core,
        client,
        _stop: stop,
    }
}

/// A scripted person at a terminal.
#[derive(Default)]
struct Script {
    out: Vec<u8>,
    err: Vec<u8>,
    interactive: bool,
    answers: VecDeque<String>,
    secrets: VecDeque<String>,
    questions: Vec<String>,
}

impl Script {
    fn person(answers: &[&str], secrets: &[&str]) -> Self {
        Self {
            interactive: true,
            answers: answers.iter().map(|a| (*a).to_owned()).collect(),
            secrets: secrets.iter().map(|s| (*s).to_owned()).collect(),
            ..Self::default()
        }
    }

    fn out(&self) -> String {
        String::from_utf8(self.out.clone()).unwrap()
    }
}

impl Terminal for Script {
    fn out(&mut self) -> &mut dyn Write {
        &mut self.out
    }
    fn err(&mut self) -> &mut dyn Write {
        &mut self.err
    }
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn ask(&mut self, question: &str) -> io::Result<String> {
        self.questions.push(question.to_owned());
        self.answers
            .pop_front()
            .ok_or_else(|| io::Error::other("no answer scripted"))
    }
    fn ask_secret(&mut self, question: &str) -> io::Result<String> {
        self.questions.push(question.to_owned());
        self.secrets
            .pop_front()
            .ok_or_else(|| io::Error::other("no secret scripted"))
    }
}

impl Daemon {
    /// Runs `loams marketplace <args>` with the daemon directory.
    async fn run(&self, args: &[&str], term: &mut Script) -> anyhow::Result<()> {
        let dir = self.dir.path().to_str().unwrap();
        let argv = ["loams", "marketplace", "--agentd-dir", dir]
            .into_iter()
            .chain(args.iter().copied());
        let Cli::Marketplace(parsed) = Cli::try_parse_from(argv).unwrap();
        // Through the daemon directory, as `run` does.
        let found = connect::connect(parsed.agentd_dir.as_deref().unwrap())
            .await
            .unwrap();
        execute(&parsed, &found.client, term).await
    }

    async fn ok(&self, args: &[&str]) -> String {
        let mut term = Script::default();
        self.run(args, &mut term).await.unwrap();
        term.out()
    }

    async fn installed(&self) -> InstalledListingList {
        self.client
            .call_as(methods::LIST_INSTALLED_LISTINGS, json!({}))
            .await
            .unwrap()
    }

    async fn digest(&self, id: &str, version: Option<&str>) -> String {
        let reply: PlanInstallReply = self
            .client
            .call_as(
                methods::PLAN_INSTALL,
                json!({ "id": id, "version": version }),
            )
            .await
            .unwrap();
        reply.plan.digest
    }
}

#[tokio::test]
async fn search_lists_matching_listings() {
    let daemon = daemon().await;
    let text = daemon.ok(&["search", "--kind", "mcp-server"]).await;
    assert!(text.contains(MCP_ID), "{text}");
    assert!(!text.contains(HARNESS_ID), "{text}");

    let json = daemon.ok(&["search", "notes", "--json"]).await;
    let found: ListingSearchResults = serde_json::from_str(&json).unwrap();
    assert!(found.results.iter().any(|r| r.id == MCP_ID), "{json}");

    let none = daemon.ok(&["search", "no-such-words-anywhere"]).await;
    assert_eq!(none, "No listings match.\n");
}

#[tokio::test]
async fn info_shows_every_version() {
    let daemon = daemon().await;
    let text = daemon.ok(&["info", HARNESS_ID]).await;
    assert!(text.contains("1.0.0"), "{text}");
    assert!(text.contains("1.1.0  latest"), "{text}");
    let error = daemon
        .run(&["info", "nobody/nothing"], &mut Script::default())
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("not_found"), "{error}");
}

#[tokio::test]
async fn dry_run_prints_the_plan_and_installs_nothing() {
    let daemon = daemon().await;
    let text = daemon.ok(&["install", MCP_ID, "--dry-run"]).await;
    let digest = daemon.digest(MCP_ID, None).await;
    for expected in [
        "READ",
        "read_note",
        "It can't:",
        "token (required)",
        "Provenance:  npm-slsa",
        &format!("Plan digest: {digest}"),
        &format!("--yes --plan-digest {digest}"),
        "Nothing was installed.",
    ] {
        assert!(text.contains(expected), "{expected:?} missing from\n{text}");
    }
    let json = daemon.ok(&["install", MCP_ID, "--dry-run", "--json"]).await;
    let reply: PlanInstallReply = serde_json::from_str(&json).unwrap();
    assert_eq!(reply.plan.digest, digest);
    assert!(daemon.installed().await.installed.is_empty());
}

#[tokio::test]
async fn install_shows_the_plan_and_asks_first() {
    let daemon = daemon().await;
    let mut person = Script::person(&["y"], &["s3cret-value"]);
    daemon.run(&["install", MCP_ID], &mut person).await.unwrap();
    let text = person.out();
    assert!(text.contains("read_note"), "{text}");
    assert!(text.contains("Installed testkit/notes-mcp 1.0.0"), "{text}");
    assert!(
        !text.contains("s3cret-value"),
        "the secret is never printed"
    );
    assert!(person.questions[0].contains("with exactly these permissions?"));
    assert!(person.questions[1].starts_with("token ("));
    assert_eq!(daemon.installed().await.installed[0].id, MCP_ID);
}

#[tokio::test]
async fn a_declined_plan_installs_nothing() {
    let daemon = daemon().await;
    let mut person = Script::person(&["n"], &[]);
    let error = daemon
        .run(&["install", MCP_ID], &mut person)
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("cancelled"), "{error}");
    assert_eq!(person.questions.len(), 1, "no secret is asked for");
    assert!(daemon.installed().await.installed.is_empty());
}

#[tokio::test]
async fn without_a_terminal_install_needs_the_reviewed_digest() {
    let daemon = daemon().await;
    let error = daemon
        .run(&["install", HARNESS_ID], &mut Script::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("--dry-run"), "{error}");
    // `--yes` alone is a usage error: never a blind yes.
    assert!(Cli::try_parse_from(["loams", "marketplace", "install", HARNESS_ID, "--yes"]).is_err());
    let error = daemon
        .run(
            &[
                "install",
                HARNESS_ID,
                "--yes",
                "--plan-digest",
                &"0".repeat(64),
            ],
            &mut Script::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("plan_changed"), "{error}");
    assert!(daemon.installed().await.installed.is_empty());

    let digest = daemon.digest(HARNESS_ID, None).await;
    let text = daemon
        .ok(&["install", HARNESS_ID, "--yes", "--plan-digest", &digest])
        .await;
    assert!(text.contains("Installed testkit/echo-acp 1.1.0"), "{text}");
}

#[tokio::test]
async fn secrets_come_from_files_never_the_command_line() {
    let daemon = daemon().await;
    let digest = daemon.digest(MCP_ID, None).await;
    let error = daemon
        .run(
            &[
                "install",
                MCP_ID,
                "--yes",
                "--plan-digest",
                &digest,
                "--secret",
                "token=s3cret",
            ],
            &mut Script::default(),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("never from the command line"),
        "{error}"
    );
    let error = daemon
        .run(
            &[
                "install",
                MCP_ID,
                "--yes",
                "--plan-digest",
                &digest,
                "--secret",
                "other=@x",
            ],
            &mut Script::default(),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("asks for no secret other"),
        "{error}"
    );
    // The required secret is missing: the daemon refuses.
    assert!(
        daemon
            .run(
                &["install", MCP_ID, "--yes", "--plan-digest", &digest],
                &mut Script::default()
            )
            .await
            .is_err()
    );

    let file = daemon.dir.path().join("token.txt");
    std::fs::write(&file, "s3cret\n").unwrap();
    let secret = format!("token=@{}", file.display());
    let json = daemon
        .ok(&[
            "install",
            MCP_ID,
            "--yes",
            "--plan-digest",
            &digest,
            "--secret",
            &secret,
            "--json",
        ])
        .await;
    let reply: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(reply["installed"]["id"], MCP_ID);
}

#[tokio::test]
async fn a_widening_update_shows_the_new_plan_and_asks_again() {
    let daemon = daemon().await;
    let digest = daemon.digest(HARNESS_ID, Some("1.0.0")).await;
    daemon
        .ok(&[
            "install",
            HARNESS_ID,
            "--version",
            "1.0.0",
            "--yes",
            "--plan-digest",
            &digest,
        ])
        .await;
    let list = daemon.ok(&["list"]).await;
    assert!(list.contains("update: 1.1.0"), "{list}");
    let updates = daemon.ok(&["update"]).await;
    assert!(
        updates.contains("1.0.0 → 1.1.0  asks for more"),
        "{updates}"
    );

    // No terminal: refused, with the way to review it.
    let error = daemon
        .run(&["update", HARNESS_ID], &mut Script::default())
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("consent_required"), "{message}");
    assert!(message.contains("--dry-run"), "{message}");

    // Declined: it stays.
    let mut person = Script::person(&["n"], &[]);
    assert!(
        daemon
            .run(&["update", HARNESS_ID], &mut person)
            .await
            .is_err()
    );
    assert_eq!(daemon.installed().await.installed[0].version, "1.0.0");

    let mut person = Script::person(&["yes"], &[]);
    daemon
        .run(&["update", HARNESS_ID], &mut person)
        .await
        .unwrap();
    let text = person.out();
    assert!(text.contains("new host api.example.com"), "{text}");
    assert!(
        text.contains("Reach these hosts: api.example.com."),
        "{text}"
    );
    assert!(
        text.contains("Updated testkit/echo-acp from 1.0.0 to 1.1.0."),
        "{text}"
    );
    assert_eq!(daemon.installed().await.installed[0].version, "1.1.0");
}

#[tokio::test]
async fn a_scripted_update_passes_the_reviewed_digest() {
    let daemon = daemon().await;
    let digest = daemon.digest(HARNESS_ID, Some("1.0.0")).await;
    daemon
        .ok(&[
            "install",
            HARNESS_ID,
            "--version",
            "1.0.0",
            "--yes",
            "--plan-digest",
            &digest,
        ])
        .await;
    let review = daemon.ok(&["update", HARNESS_ID, "--dry-run"]).await;
    let new = daemon.digest(HARNESS_ID, None).await;
    assert!(
        review.contains(&format!("--yes --plan-digest {new}")),
        "{review}"
    );
    daemon
        .ok(&["update", HARNESS_ID, "--yes", "--plan-digest", &new])
        .await;
    assert_eq!(daemon.installed().await.installed[0].version, "1.1.0");
}

#[tokio::test]
async fn uninstall_asks_or_takes_yes() {
    let daemon = daemon().await;
    let digest = daemon.digest(HARNESS_ID, None).await;
    daemon
        .ok(&["install", HARNESS_ID, "--yes", "--plan-digest", &digest])
        .await;
    assert!(
        daemon
            .run(&["uninstall", HARNESS_ID], &mut Script::default())
            .await
            .is_err()
    );
    let mut person = Script::person(&["y"], &[]);
    daemon
        .run(&["uninstall", HARNESS_ID], &mut person)
        .await
        .unwrap();
    assert!(person.out().contains("Its data stays"));
    assert!(daemon.installed().await.installed.is_empty());
    assert_eq!(daemon.ok(&["list"]).await, "Nothing is installed.\n");
}

#[tokio::test]
async fn sources_are_listed_and_the_official_one_stays() {
    let daemon = daemon().await;
    let text = daemon.ok(&["source", "list"]).await;
    assert!(text.starts_with("official (official)"), "{text}");
    assert!(
        text.contains("signed by https://github.com/ostrium-labs/loams-marketplace/"),
        "{text}"
    );
    let error = daemon
        .run(&["source", "remove", "official"], &mut Script::default())
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("bad_params"), "{error}");
    let error = daemon
        .run(
            &[
                "source",
                "add",
                "x",
                "https://example.org/x/index.json",
                "--identity",
                "https://github.com/someone/else",
            ],
            &mut Script::default(),
        )
        .await
        .unwrap_err();
    assert!(!error.to_string().is_empty());
}
