//! MK1 Task 4b: `MarketplaceService`'s ndjson handlers over the engine, with
//! the testkit world (a signed fake index, fake npm, stand-in provenance),
//! and MK1 Task 6's registrations: an installed ACP harness in
//! `ListHarnesses`, an installed MCP server on every run.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use loams_agentd_harness::{Harness, HarnessError, RunControls};
use loams_agentd_marketplace::MarketplaceHost;
use loams_agentd_marketplace::service::mcp_name;
use loams_agentd_marketplace::testkit::{HARNESS_ID, MCP_ID, OFFICIAL_BASE, World};
use loams_agentd_proto::marketplace::{
    AvailableUpdateList, InstalledListingList, InstalledListingReply, ListingDetail,
    ListingSearchResults, MarketplaceSource, MarketplaceSourceList, PlanInstallReply,
};
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessCatalog, HarnessId, Model, ReasoningLevel, RunRequest,
    SandboxLevel, SteeringMode,
};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::{ApprovalSettings, EngineCore, HarnessRegistry};
use serde_json::json;

struct Capture(Arc<Mutex<Vec<RunRequest>>>);

#[async_trait]
impl Harness for Capture {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Capture"
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
        self.0.lock().unwrap().push(request);
        Ok(futures::stream::iter(vec![Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        })])
        .boxed())
    }
}

struct Engine {
    _dir: tempfile::TempDir,
    core: EngineCore,
    client: RpcClient,
    world: World,
    requests: Arc<Mutex<Vec<RunRequest>>>,
}

async fn engine() -> Engine {
    let dir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture(requests.clone())));
    let core = EngineCore::assemble_with_approvals(
        &dir.path().join("engine"),
        Arc::new(registry),
        HarnessId::Mock,
        ApprovalSettings::full_access_default(),
    )
    .unwrap();
    let world = World::new(&dir.path().join("agentd"));
    let host = MarketplaceHost::open(world.deps()).unwrap();
    assert!(host.refresh().await.is_empty());
    core.attach_marketplace(host);
    let client = loams_agentd_rpc::memory_client(core.rpc_service());
    Engine {
        _dir: dir,
        core,
        client,
        world,
        requests,
    }
}

impl Engine {
    async fn plan(&self, id: &str, version: Option<&str>) -> PlanInstallReply {
        self.client
            .call_as(
                methods::PLAN_INSTALL,
                json!({ "id": id, "version": version }),
            )
            .await
            .unwrap()
    }

    async fn install(&self, id: &str, version: Option<&str>, secrets: serde_json::Value) {
        let plan = self.plan(id, version).await;
        let reply: InstalledListingReply = self
            .client
            .call_as(
                methods::INSTALL_LISTING,
                json!({ "id": id, "version": version, "planDigest": plan.plan.digest, "secrets": secrets }),
            )
            .await
            .unwrap();
        assert_eq!(reply.installed.id, id);
    }

    async fn harnesses(&self) -> HarnessCatalog {
        self.client
            .call_as(methods::LIST_HARNESSES, json!({}))
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn list_marketplace_sources_answers_the_official_source() {
    let engine = engine().await;
    let list: MarketplaceSourceList = engine
        .client
        .call_as(methods::LIST_MARKETPLACE_SOURCES, json!({}))
        .await
        .unwrap();
    assert_eq!(list.sources.len(), 1);
    let official = &list.sources[0];
    assert!(official.official);
    assert_eq!(official.url, format!("{OFFICIAL_BASE}/index.json"));
    assert!(official.refreshed_ms > 0);
    assert_eq!(official.error, None);
    assert_eq!(official.listings, 8);
}

#[tokio::test]
async fn add_and_remove_marketplace_source() {
    let engine = engine().await;
    let base = "https://example.org/market";
    let identity =
        "https://github.com/example/market/.github/workflows/publish.yml@refs/heads/main";
    engine.world.fetcher.serve(
        base,
        &loams_agentd_marketplace::testkit::index_bytes(),
        br#"{"revocations":[]}"#,
        identity,
    );
    // Signed by another identity than the one it is pinned to: refused.
    let error = engine
        .client
        .call(
            methods::ADD_MARKETPLACE_SOURCE,
            json!({ "name": "example", "url": format!("{base}/index.json"), "identity": "https://github.com/someone/else" }),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().starts_with("untrusted_listing"),
        "{error}"
    );

    let added: MarketplaceSource = engine
        .client
        .call_as(
            methods::ADD_MARKETPLACE_SOURCE,
            json!({ "name": "example", "url": format!("{base}/index.json"), "identity": identity }),
        )
        .await
        .unwrap();
    assert_eq!(added.name, "example");
    assert!(!added.official);
    let list: MarketplaceSourceList = engine
        .client
        .call_as(methods::LIST_MARKETPLACE_SOURCES, json!({}))
        .await
        .unwrap();
    assert_eq!(list.sources.len(), 2);

    engine
        .client
        .call(
            methods::REMOVE_MARKETPLACE_SOURCE,
            json!({ "name": "example" }),
        )
        .await
        .unwrap();
    let error = engine
        .client
        .call(
            methods::REMOVE_MARKETPLACE_SOURCE,
            json!({ "name": "official" }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("bad_params"), "{error}");
}

#[tokio::test]
async fn search_listings_filters_by_kind_and_level() {
    let engine = engine().await;
    let found: ListingSearchResults = engine
        .client
        .call_as(methods::SEARCH_LISTINGS, json!({ "kinds": ["mcp-server"] }))
        .await
        .unwrap();
    let ids: Vec<&str> = found.results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "ostrium/github-mcp-server",
            "ostrium/plane-mcp-server",
            MCP_ID
        ]
    );
    let core: ListingSearchResults = engine
        .client
        .call_as(methods::SEARCH_LISTINGS, json!({ "minLevel": "core" }))
        .await
        .unwrap();
    assert!(core.results.iter().all(|r| r.level.is_some()));
    let error = engine
        .client
        .call(methods::SEARCH_LISTINGS, json!({ "kinds": ["robot"] }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("robot"), "{error}");
}

#[tokio::test]
async fn get_listing_answers_every_version() {
    let engine = engine().await;
    let detail: ListingDetail = engine
        .client
        .call_as(methods::GET_LISTING, json!({ "id": HARNESS_ID }))
        .await
        .unwrap();
    assert_eq!(detail.summary.latest.as_deref(), Some("1.1.0"));
    assert_eq!(detail.listing["version"], "1.1.0");
    let versions: Vec<&str> = detail.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions, ["1.0.0", "1.1.0"]);
    let error = engine
        .client
        .call(methods::GET_LISTING, json!({ "id": "nobody/nothing" }))
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("not_found"), "{error}");
}

#[tokio::test]
async fn plan_install_creates_nothing() {
    let engine = engine().await;
    let plan = engine.plan(MCP_ID, None).await.plan;
    assert_eq!(plan.digest.len(), 64);
    assert_eq!(plan.tools[0].name, "read_note");
    assert_eq!(plan.secrets[0].name, "token");
    assert!(plan.secrets[0].required);
    let installed: InstalledListingList = engine
        .client
        .call_as(methods::LIST_INSTALLED_LISTINGS, json!({}))
        .await
        .unwrap();
    assert!(installed.installed.is_empty());
}

#[tokio::test]
async fn install_listing_needs_the_confirmed_plan() {
    let engine = engine().await;
    let error = engine
        .client
        .call(
            methods::INSTALL_LISTING,
            json!({ "id": HARNESS_ID, "planDigest": "00".repeat(32) }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("plan_changed"), "{error}");
}

#[tokio::test]
async fn harness_descriptor_appears_in_list_harnesses() {
    let engine = engine().await;
    let before = engine.harnesses().await;
    assert!(before.0.iter().all(|d| d.id != HarnessId::Marketplace));

    engine.install(HARNESS_ID, None, json!({})).await;
    let after = engine.harnesses().await;
    let entry = after
        .0
        .iter()
        .find(|d| d.id == HarnessId::Marketplace)
        .expect("the installed harness is listed");
    assert_eq!(entry.marketplace_listing.as_deref(), Some(HARNESS_ID));
    assert_eq!(entry.name, format!("Testkit {HARNESS_ID}"));
    assert_eq!(entry.enabled, Some(true));
    assert!(!entry.can_install);
    // The registry resolves it to an ACP harness launched from the install.
    let harness = engine
        .core
        .registry
        .resolve_marketplace(HARNESS_ID)
        .unwrap();
    assert_eq!(harness.id(), HarnessId::Marketplace);

    engine
        .client
        .call(methods::UNINSTALL_LISTING, json!({ "id": HARNESS_ID }))
        .await
        .unwrap();
    let gone = engine.harnesses().await;
    assert!(gone.0.iter().all(|d| d.id != HarnessId::Marketplace));
    assert!(
        engine
            .core
            .registry
            .resolve_marketplace(HARNESS_ID)
            .is_err()
    );
}

#[tokio::test]
async fn mcp_server_available_to_threads() {
    let engine = engine().await;
    engine
        .install(MCP_ID, None, json!({ "token": "s3cret" }))
        .await;
    engine
        .core
        .workspace
        .create_chat(
            "t",
            None,
            Some(&engine.core.device_id),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    engine
        .core
        .sessions
        .dispatch(
            "t",
            HarnessId::Mock,
            RunRequest {
                mcp: None,
                mcp_servers: vec![],
                prompt: "Read my notes".into(),
                harness: Some(HarnessId::Mock),
                model: None,
                reasoning: None,
                model_options: Default::default(),
                cwd: "/tmp".into(),
                sandbox: SandboxLevel::WorkspaceWrite,
                auto_approve: true,
                resume: None,
                attachments: vec![],
                worktree: None,
            },
            Some("t-user".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while engine.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let request = engine.requests.lock().unwrap()[0].clone();
    let server = request
        .mcp_servers
        .iter()
        .find(|s| s.name == mcp_name(MCP_ID))
        .expect("the run carries the installed MCP server");
    assert_eq!(server.command, "/usr/bin/node");
    assert!(server.args[0].ends_with("node_modules/notes-mcp/bin/notes.js"));
    assert_eq!(server.args[1], "--stdio");
    assert_eq!(server.env["NOTES_TOKEN"], "s3cret");
}

#[tokio::test]
async fn updates_are_listed_and_a_widening_one_needs_consent() {
    let engine = engine().await;
    engine.install(HARNESS_ID, Some("1.0.0"), json!({})).await;
    let installed: InstalledListingList = engine
        .client
        .call_as(methods::LIST_INSTALLED_LISTINGS, json!({}))
        .await
        .unwrap();
    assert_eq!(installed.installed[0].version, "1.0.0");
    assert_eq!(installed.installed[0].update.as_deref(), Some("1.1.0"));
    assert_eq!(installed.installed[0].kind, "harness");

    let updates: AvailableUpdateList = engine
        .client
        .call_as(methods::CHECK_LISTING_UPDATES, json!({}))
        .await
        .unwrap();
    assert_eq!(updates.updates.len(), 1);
    assert!(updates.updates[0].widens);

    let error = engine
        .client
        .call(methods::UPDATE_LISTING, json!({ "id": HARNESS_ID }))
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("consent_required"), "{error}");
    let plan = engine.plan(HARNESS_ID, None).await;
    let updated: InstalledListingReply = engine
        .client
        .call_as(
            methods::UPDATE_LISTING,
            json!({ "id": HARNESS_ID, "planDigest": plan.plan.digest }),
        )
        .await
        .unwrap();
    assert_eq!(updated.installed.version, "1.1.0");
    assert_eq!(updated.installed.update, None);
}

#[tokio::test]
async fn without_a_marketplace_the_methods_are_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble_with_approvals(
        dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        ApprovalSettings::full_access_default(),
    )
    .unwrap();
    let client = loams_agentd_rpc::memory_client(core.rpc_service());
    let error = client
        .call(methods::LIST_MARKETPLACE_SOURCES, json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("engine_not_ready"), "{error}");
}
