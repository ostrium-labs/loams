//! Config rendering goldens. The compose golden is the file the compose
//! profile mounts (`deploy/flink/compose/config.yaml`), so the stack and
//! the renderer cannot drift. Set `LOAMS_UPDATE_GOLDENS=1` to rewrite them
//! after a deliberate change.

use std::path::PathBuf;

use loams_flink::TenantId;
use loams_flink::config::{ClusterSpec, Platform, Size, render};

/// `want` is the golden's text (`include_str!`, so Buck sees the file too);
/// `golden` its path from the crate, for rewriting.
fn check(golden: &str, want: &str, spec: &ClusterSpec) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(golden);
    let rendered = render(spec).unwrap().to_yaml();
    if std::env::var_os("LOAMS_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, &rendered).unwrap();
        return;
    }
    assert_eq!(
        rendered,
        want,
        "{} differs from the renderer; rerun with LOAMS_UPDATE_GOLDENS=1 if the change is meant",
        path.display()
    );
}

#[test]
fn compose_config_matches_the_deployed_file() {
    let spec = ClusterSpec::new(
        TenantId::new("local").unwrap(),
        Platform::Compose,
        Size::Dev,
    );
    check(
        "../../deploy/flink/compose/config.yaml",
        include_str!("../../../deploy/flink/compose/config.yaml"),
        &spec,
    );
}

#[test]
fn kubernetes_config_golden() {
    let spec = ClusterSpec::new(
        TenantId::new("acme").unwrap(),
        Platform::Kubernetes,
        Size::Small,
    );
    check(
        "tests/config-goldens/kubernetes-acme-small.yaml",
        include_str!("config-goldens/kubernetes-acme-small.yaml"),
        &spec,
    );
}

#[test]
fn kubernetes_example_matches_the_kustomize_base() {
    let spec = ClusterSpec::new(
        TenantId::new("example").unwrap(),
        Platform::Kubernetes,
        Size::Dev,
    );
    check(
        "../../deploy/flink/k8s/config.yaml",
        include_str!("../../../deploy/flink/k8s/config.yaml"),
        &spec,
    );
}
