//! MK1 Task 4b: the marketplace as agentd runs it, over the testkit world
//! (a signed fake index, fake npm, stand-in provenance).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use loams_agentd_marketplace::market::Query;
use loams_agentd_marketplace::service::{
    InstallRequest, MarketplaceHost, REFRESH_INTERVAL, ServiceError, mcp_name, secret_key,
};
use loams_agentd_marketplace::testkit::{HARNESS_ID, MCP_ID, OFFICIAL_BASE, World};
use serde_json::json;

async fn host(dir: &std::path::Path) -> (World, MarketplaceHost) {
    let world = World::new(dir);
    let host = MarketplaceHost::open(world.deps()).unwrap();
    let failed = host.refresh().await;
    assert!(failed.is_empty(), "{failed:?}");
    (world, host)
}

async fn install(
    host: &MarketplaceHost,
    id: &str,
    version: Option<&str>,
    secrets: &[(&str, &str)],
) -> Result<loams_agentd_marketplace::install::Installed, ServiceError> {
    let (_, digest, _) = host.plan_install(id, version, None).await.unwrap();
    host.install(InstallRequest {
        id: id.into(),
        version: version.map(str::to_owned),
        source: None,
        plan_digest: digest,
        config: BTreeMap::new(),
        secrets: secrets
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    })
    .await
}

#[tokio::test]
async fn search_finds_the_testkit_listings() {
    let dir = tempfile::tempdir().unwrap();
    let (_world, host) = host(dir.path()).await;
    let found = host
        .search(&Query {
            text: "testkit".into(),
            ..Query::default()
        })
        .await;
    let ids: Vec<&str> = found.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [HARNESS_ID, MCP_ID]);
}

#[tokio::test]
async fn an_acp_harness_installs_and_registers() {
    let dir = tempfile::tempdir().unwrap();
    let (_world, host) = host(dir.path()).await;
    let mut changes = host.subscribe();
    let installed = install(&host, HARNESS_ID, None, &[]).await.unwrap();
    assert_eq!(installed.version, "1.1.0");
    assert!(changes.has_changed().unwrap());
    changes.mark_unchanged();

    let registrations = host.registrations();
    assert_eq!(registrations.harnesses.len(), 1);
    let harness = &registrations.harnesses[0];
    assert_eq!(harness.listing_id, HARNESS_ID);
    assert_eq!(harness.program, "/usr/bin/node");
    assert!(harness.args[0].ends_with("node_modules/echo-acp/bin/echo.js"));
    assert_eq!(harness.args[1], "--acp");

    host.uninstall(HARNESS_ID).unwrap();
    assert!(changes.has_changed().unwrap());
    assert!(host.registrations().harnesses.is_empty());
}

#[tokio::test]
async fn install_needs_the_confirmed_plan() {
    let dir = tempfile::tempdir().unwrap();
    let (_world, host) = host(dir.path()).await;
    let error = host
        .install(InstallRequest {
            id: HARNESS_ID.into(),
            plan_digest: "00".repeat(32),
            ..InstallRequest::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(error, ServiceError::PlanChanged { .. }), "{error}");
    assert_eq!(error.reason(), Some("plan_changed"));
    assert!(error.with_reason().starts_with("plan_changed: "));
    assert!(host.layout().list().is_empty(), "nothing installed");
}

#[tokio::test]
async fn an_mcp_server_needs_its_secret_and_runs_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let (world, host) = host(dir.path()).await;
    let error = install(&host, MCP_ID, None, &[]).await.unwrap_err();
    assert!(matches!(error, ServiceError::MissingSecret(ref s) if s == "token"));

    install(&host, MCP_ID, None, &[("token", "s3cret")])
        .await
        .unwrap();
    let servers = host.registrations().mcp_servers;
    assert_eq!(servers.len(), 1);
    let server = &servers[0];
    assert_eq!(server.name, mcp_name(MCP_ID));
    assert_eq!(server.command, "/usr/bin/node");
    assert!(server.args[0].ends_with("node_modules/notes-mcp/bin/notes.js"));
    assert_eq!(server.args[1], "--stdio");
    assert_eq!(
        server.env.get("NOTES_TOKEN").map(String::as_str),
        Some("s3cret")
    );
    // The secret is in the vault, never in the install's files.
    let on_disk =
        std::fs::read_to_string(dir.path().join("agents").join(MCP_ID).join("config.json"))
            .unwrap();
    assert!(!on_disk.contains("s3cret"));
    assert!(!format!("{server:?}").contains("s3cret"));

    host.uninstall(MCP_ID).unwrap();
    assert!(host.registrations().mcp_servers.is_empty());
    assert!(
        world
            .deps()
            .vault
            .get(&secret_key(MCP_ID, "token"))
            .is_none()
    );
}

#[tokio::test]
async fn a_remote_mcp_server_installs_nothing_and_waits_for_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let (_world, host) = host(dir.path()).await;
    install(&host, "ostrium/plane-mcp-server", None, &[("api_key", "t")])
        .await
        .unwrap();
    let registrations = host.registrations();
    assert!(registrations.mcp_servers.is_empty());
    assert_eq!(registrations.remote_mcp, ["ostrium/plane-mcp-server"]);
}

#[tokio::test]
async fn a_widening_update_needs_consent() {
    let dir = tempfile::tempdir().unwrap();
    let (_world, host) = host(dir.path()).await;
    install(&host, HARNESS_ID, Some("1.0.0"), &[])
        .await
        .unwrap();

    let updates = host.check_updates().await;
    assert_eq!(updates.len(), 1);
    assert_eq!(
        (updates[0].from.as_str(), updates[0].to.as_str()),
        ("1.0.0", "1.1.0")
    );
    assert_eq!(updates[0].widens, ["new host api.example.com"]);
    let listed = host.list_installed().await;
    assert_eq!(listed[0].update.as_deref(), Some("1.1.0"));

    let error = host.update(HARNESS_ID, None, "").await.unwrap_err();
    assert_eq!(error.reason(), Some("consent_required"), "{error}");

    let (_, digest, _) = host.plan_install(HARNESS_ID, None, None).await.unwrap();
    let updated = host.update(HARNESS_ID, None, &digest).await.unwrap();
    assert_eq!(updated.version, "1.1.0");
    assert!(host.check_updates().await.is_empty());
}

#[tokio::test]
async fn a_disable_revocation_unregisters_on_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let (world, host) = host(dir.path()).await;
    install(&host, HARNESS_ID, None, &[]).await.unwrap();
    assert_eq!(host.registrations().harnesses.len(), 1);

    world.revoke(&json!({ "revocations": [{
        "id": HARNESS_ID, "versions": ">=1.0.0", "severity": "disable",
        "reason": "Sends prompts elsewhere.", "date": "2026-10-25"
    }] }));
    let mut changes = host.subscribe();
    changes.mark_unchanged();
    host.refresh().await;
    assert!(changes.has_changed().unwrap());
    assert!(host.registrations().harnesses.is_empty());
    let listed = host.list_installed().await;
    assert_eq!(
        listed[0].revocation.as_ref().map(|r| r.reason.as_str()),
        Some("Sends prompts elsewhere.")
    );
    // The install and its data stay (D1472).
    assert!(
        dir.path()
            .join("agents")
            .join(HARNESS_ID)
            .join("data")
            .is_dir()
    );
}

#[tokio::test]
async fn revocations_are_checked_at_open_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (world, host) = host(dir.path()).await;
    install(&host, HARNESS_ID, None, &[]).await.unwrap();
    world.revoke(&json!({ "revocations": [{
        "id": HARNESS_ID, "versions": "*", "severity": "disable",
        "reason": "Gone.", "date": "2026-10-25"
    }] }));
    host.refresh().await;
    drop(host);
    // A restart with the network gone still knows the revocation.
    let mut deps = world.deps();
    deps.fetcher = std::sync::Arc::new(loams_agentd_marketplace::testkit::FakeFetcher::default());
    let reopened = MarketplaceHost::open(deps).unwrap();
    assert_eq!(reopened.findings().len(), 1);
    assert!(reopened.registrations().harnesses.is_empty());
}

#[tokio::test(start_paused = true)]
async fn sources_refresh_at_start_and_every_six_hours() {
    let dir = tempfile::tempdir().unwrap();
    let world = World::new(dir.path());
    let host = MarketplaceHost::open(world.deps()).unwrap();
    let index = format!("{OFFICIAL_BASE}/index.json");
    let task = host.spawn_refresh(REFRESH_INTERVAL);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(world.fetcher.calls(&index), 1, "refreshed at start");
    tokio::time::sleep(REFRESH_INTERVAL - Duration::from_secs(2)).await;
    assert_eq!(world.fetcher.calls(&index), 1, "not before six hours");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(world.fetcher.calls(&index), 2, "again after six hours");
    tokio::time::sleep(REFRESH_INTERVAL).await;
    assert_eq!(world.fetcher.calls(&index), 3);
    task.abort();
}
