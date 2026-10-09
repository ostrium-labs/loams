//! Local-first startup boundaries: the engine opens the local profile only, and
//! the headless daemon serves and stops on its IPC port. The fork's synced,
//! development and edge-relay cases went with the edge (D781).
#![allow(clippy::unwrap_used)]

use loams_agentd_rpc::{connect_ws, memory_client, methods};
use loams_agentd_sessions::{
    Engine, EngineConfig, EngineInfo, EngineProfile, HarnessId, WorkspaceScope,
};

fn config(data_dir: &std::path::Path) -> EngineConfig {
    EngineConfig {
        data_dir: data_dir.to_path_buf(),
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        terminal_shell: None,
    }
}

async fn wait_until(mut check: impl FnMut() -> bool, message: &str) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{message}");
}

#[tokio::test]
async fn boot_serves_the_local_profile() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let profile = EngineProfile::local(dir.path()).unwrap();
    let runtime = Engine::assemble_runtime(&config, profile).await.unwrap();
    assert_eq!(runtime.workspace_scope(), WorkspaceScope::Local);
    let client = memory_client(runtime.core().rpc_service());
    let info: EngineInfo = client
        .call_as(methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(info.workspace_scope, WorkspaceScope::Local);
    assert!(
        client
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap()
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(dir.path().join("profiles/local").is_dir());
    assert!(!dir.path().join("orgs").exists());
    runtime.shutdown().await;
}

#[tokio::test]
async fn removed_edge_and_sign_in_methods_are_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Engine::assemble_runtime(
        &config(dir.path()),
        EngineProfile::local(dir.path()).unwrap(),
    )
    .await
    .unwrap();
    let client = memory_client(runtime.core().rpc_service());
    for method in [
        "RelayCommand",
        "FocusChat",
        "ProbeSync",
        "SyncStatus",
        "WatchConnectivity",
        "WatchTransfers",
        "WatchDevices",
        "AuthStatus",
        "SignIn",
        "SignInHeadless",
        "CompleteSignIn",
        "SignOut",
        "ListOrgs",
        "CreateOrg",
        "SelectOrg",
        "LocalImportStatus",
        "ImportLocalWorkspace",
    ] {
        let error = client
            .call(method, serde_json::json!({ "chatId": "c" }))
            .await
            .expect_err(method);
        assert!(
            error.to_string().contains("unknown method"),
            "{method}: {error}"
        );
    }
    // The local re-send of dead attempts stays (DD1 fix round 1, I1).
    assert_eq!(
        client
            .call(
                methods::RETRY_DELIVERY,
                serde_json::json!({ "chatId": "c" })
            )
            .await
            .unwrap(),
        serde_json::json!({})
    );
    runtime.shutdown().await;
}

/// Shutdown joins every worker and frees the engine graph.
#[tokio::test]
async fn runtime_shutdown_retires_the_graph() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Engine::assemble_runtime(
        &config(dir.path()),
        EngineProfile::local(dir.path()).unwrap(),
    )
    .await
    .unwrap();
    let retired = runtime.core().doc_host.retirement_probe();
    tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("shutdown never returned — a worker did not join");
    drop(runtime);
    wait_until(
        &*retired,
        "engine graph still reachable after shutdown + drop",
    )
    .await;
}

#[tokio::test]
async fn headless_stop_rpc_drains_the_daemon_and_releases_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut engine_config = config(dir.path());
    engine_config.ipc_port = port;
    let daemon = tokio::spawn(Engine::new(engine_config).run());

    let client = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(client) = connect_ws(&format!("ws://127.0.0.1:{port}")).await {
                break client;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("headless IPC did not start");

    assert_eq!(
        client
            .call(methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .unwrap(),
        serde_json::json!({ "ok": true })
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), daemon)
        .await
        .expect("headless engine did not stop")
        .expect("headless task panicked")
        .expect("headless shutdown failed");

    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("headless IPC port remained occupied after shutdown");
}
