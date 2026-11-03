//! CRD generation, defaults and schema rules of `loams-operator-api`.

use std::path::PathBuf;
use std::time::Duration;

use loams_operator_api::{
    ExposureMode, IdentityProvider, InstallExposureMode, LoamsClusterSpec, LoamsInstallSpec,
    all_crds,
};
use serde_json::Value;

fn from_yaml<T: serde::de::DeserializeOwned>(s: &str) -> T {
    serde_yaml_ng::from_str(s).expect("valid yaml")
}

fn minimal_home() -> LoamsClusterSpec {
    from_yaml(
        r#"
role: home
release: 0.1.4
marketplace:
  index: https://ostrium-labs.github.io/loams-marketplace/index.v2.json
base:
  access:
    org: acme
    identity:
      externalDomain: id.example.internal
      database: { secretRef: { name: identity-db } }
    openfga:
      database: { secretRef: { name: openfga-db } }
"#,
    )
}

fn crds_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/loams-operator/crds")
}

#[test]
fn crds_match_checked_in() {
    let crds = all_crds();
    assert_eq!(crds.len(), 3);
    for crd in &crds {
        let name = crd.metadata.name.clone().expect("crd name");
        let path = crds_dir().join(format!("{name}.yaml"));
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; run crdgen", path.display()));
        let fresh = serde_yaml_ng::to_string(crd).expect("serialise crd");
        assert_eq!(on_disk, fresh, "{name} drifted; run crdgen");
    }
    let files = std::fs::read_dir(crds_dir())
        .expect("crds dir")
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        })
        .count();
    assert_eq!(files, crds.len(), "stale CRD file in the crds directory");
}

#[test]
fn every_kind_is_namespaced_v1alpha1_loams_dev() {
    for c in all_crds() {
        assert_eq!(c.spec.group, "loams.dev");
        assert_eq!(c.spec.scope, "Namespaced");
        assert_eq!(c.spec.versions[0].name, "v1alpha1");
        let sub = c.spec.versions[0]
            .subresources
            .as_ref()
            .expect("subresources");
        assert!(sub.status.is_some(), "status subresource");
    }
}

#[test]
fn names_and_short_names() {
    let got: Vec<(String, String, Vec<String>)> = all_crds()
        .into_iter()
        .map(|c| {
            (
                c.spec.names.kind,
                c.spec.names.plural,
                c.spec.names.short_names.unwrap_or_default(),
            )
        })
        .collect();
    let want = [
        ("LoamsCluster", "loamsclusters", "lc"),
        ("LoamsInstall", "loamsinstalls", "li"),
        ("StorageBinding", "storagebindings", "sb"),
    ];
    for (kind, plural, short) in want {
        assert!(
            got.iter()
                .any(|(k, p, s)| k == kind && p == plural && s == &[short.to_string()]),
            "{kind} missing or wrong: {got:?}"
        );
    }
}

#[test]
fn printer_columns_ready_and_summary() {
    for c in all_crds() {
        let cols = c.spec.versions[0]
            .additional_printer_columns
            .clone()
            .unwrap_or_default();
        let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Ready", "Summary"], "{}", c.spec.names.kind);
        assert!(cols[0].json_path.contains("Ready"));
        assert_eq!(cols[1].json_path, ".status.summary");
    }
}

#[test]
fn install_defaults() {
    let s: LoamsInstallSpec = from_yaml("cluster: home\nlisting: {id: a/b, version: 1.0.0}");
    assert_eq!(s.exposure.mode, InstallExposureMode::None);
    assert!(!s.sandbox.allow_without_gvisor);
    assert_eq!(s.project, "default");
    assert!(s.storage.is_empty());
    assert!(s.approved_cluster_scoped.is_none());
}

#[test]
fn cluster_defaults() {
    let s = minimal_home();
    assert_eq!(s.exposure.mode, ExposureMode::Tailnet);
    assert_eq!(s.marketplace.refresh, Duration::from_secs(300));
    assert_eq!(s.base.access.identity.provider, IdentityProvider::Zitadel);
}

#[test]
fn marketplace_refresh_is_a_duration_string() {
    let mut s = minimal_home();
    s.marketplace.refresh = Duration::from_secs(90);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["marketplace"]["refresh"], "1m 30s");
    let back: LoamsClusterSpec = serde_json::from_value(v).unwrap();
    assert_eq!(back.marketplace.refresh, Duration::from_secs(90));
    let five: LoamsClusterSpec =
        from_yaml(&serde_yaml_ng::to_string(&serde_json::to_value(minimal_home()).unwrap()).unwrap());
    assert_eq!(
        serde_json::to_value(&five).unwrap()["marketplace"]["refresh"],
        "5m"
    );
}

#[test]
fn unknown_cluster_exposure_is_rejected_by_the_type() {
    let r: Result<LoamsClusterSpec, _> = serde_yaml_ng::from_str(
        "role: home\nrelease: 0.1.4\nexposure: {mode: public}\nmarketplace: {index: 'https://x/'}\nbase: {access: {org: a, identity: {externalDomain: d, database: {secretRef: {name: s}}}, openfga: {database: {secretRef: {name: s}}}}}",
    );
    assert!(r.is_err());
}

/// Walks a schema; a property named like a secret value is allowed only
/// inside `SecretRef`.
fn walk(v: &Value, path: &str, in_secret_ref: bool, bad: &mut Vec<String>) {
    const FORBIDDEN: [&str; 4] = ["password", "token", "key", "dsn"];
    let Value::Object(map) = v else {
        if let Value::Array(a) = v {
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{path}[{i}]"), in_secret_ref, bad);
            }
        }
        return;
    };
    if let Some(Value::Object(props)) = map.get("properties") {
        // The SecretRef object is the one with a `name` and an optional `key`.
        let is_secret_ref =
            props.contains_key("name") && props.contains_key("key") && props.len() == 2;
        for (name, sub) in props {
            let p = format!("{path}.{name}");
            if !is_secret_ref && !in_secret_ref && FORBIDDEN.contains(&name.to_lowercase().as_str())
            {
                bad.push(p.clone());
            }
            walk(sub, &p, is_secret_ref || in_secret_ref, bad);
        }
    }
    for (k, sub) in map {
        if k != "properties" {
            walk(sub, &format!("{path}.{k}"), in_secret_ref, bad);
        }
    }
}

#[test]
fn no_secret_value_fields() {
    let mut bad = Vec::new();
    for c in all_crds() {
        let v = serde_json::to_value(&c).unwrap();
        walk(&v, &c.spec.names.kind, false, &mut bad);
    }
    assert!(bad.is_empty(), "secret-like value fields: {bad:?}");
}

#[test]
fn cel_rules_present() {
    // The it_kind_cel tests prove the API server enforces these; this one
    // proves they are in the generated schema, with the messages the tests
    // look for.
    let text: String = all_crds()
        .iter()
        .map(|c| serde_yaml_ng::to_string(c).unwrap())
        .collect();
    for msg in [
        "listing.id is immutable",
        "kind is immutable",
        "kind object needs an object block",
        "set only one of workloadIdentity and secretRef",
    ] {
        assert!(text.contains(msg), "missing CEL message: {msg}");
    }
}
