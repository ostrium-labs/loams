//! DX1 Task 9's tests for the standalone gateway (the `loams-temporal`
//! binary and `loams-gateway`'s `temporal` module share `standalone`).

use loams_temporal_frontend::standalone::{Config, check, dev_config, run_on};
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_client::WorkflowServiceClient;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    DescribeNamespaceRequest, GetSystemInfoRequest,
};

#[test]
fn config_rejects_dev_settings_on_a_public_listener() {
    let mut c = dev_config();
    assert!(check(&c).is_ok());
    c.listen = "0.0.0.0:7233".parse().expect("addr");
    assert!(check(&c).unwrap_err().contains("loopback"));
    let mut c = dev_config();
    c.insecure_dev = false;
    assert!(check(&c).unwrap_err().contains("insecure_dev"));
}

#[test]
fn example_config_parses_and_needs_no_dev_mode() {
    let path = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set"))
        .join("../../deploy/release/temporal-gateway.toml.example");
    let text = std::fs::read_to_string(&path).expect("example config");
    let c: Config = toml::from_str(&text).expect("parses");
    assert!(!c.insecure_dev);
    assert!(check(&c).is_ok());
}

#[test]
fn standalone_binary_has_no_loams_deps() {
    // D1539: nothing Loams-internal besides the gateway's own crates.
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set"));
    for krate in [
        "loams-temporal-frontend",
        "loams-temporal-store",
        "loams-temporal-resonate",
        "loams-temporal-matching",
        "loams-temporal-statemachine",
        "loams-temporal-history",
        "loams-temporal-proto",
    ] {
        let text = std::fs::read_to_string(root.join("..").join(krate).join("Cargo.toml"))
            .expect("Cargo.toml");
        for line in text.lines().map(str::trim) {
            let name = line.split(['=', '.', ' ']).next().unwrap_or("");
            if name.starts_with("loams-") && !name.starts_with("loams-temporal-") {
                panic!("{krate} depends on {name}");
            }
        }
    }
}

#[test]
fn relay_metrics_have_no_tenant_labels() {
    let text = loams_temporal_resonate::RelayMetrics::default().render();
    assert!(text.contains("loams_temporal_outbox"), "{text}");
    for label in ["namespace", "tenant", "workflow", "run"] {
        assert!(!text.contains(&format!("{label}=")), "{text}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_serves_get_system_info_and_the_bootstrap_namespace() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(run_on(dev_config(), listener, async {
        let _ = rx.await;
    }));
    let mut client = loop {
        match WorkflowServiceClient::connect(format!("http://{addr}")).await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    };
    let info = client
        .get_system_info(GetSystemInfoRequest {})
        .await
        .expect("info")
        .into_inner();
    assert!(info.capabilities.expect("caps").sdk_metadata);
    let ns = client
        .describe_namespace(DescribeNamespaceRequest {
            namespace: "default".into(),
            ..Default::default()
        })
        .await
        .expect("default namespace")
        .into_inner();
    assert_eq!(ns.namespace_info.expect("info").name, "default");
    let _ = tx.send(());
    server.await.expect("join").expect("clean shutdown");
}
