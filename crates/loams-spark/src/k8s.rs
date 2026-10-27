//! Kubernetes objects for one Spark instance (SP-a D1998; design 81 R3, R8).
//!
//! [`render`] turns a backend and a [`ClusterConf`] into the objects of one
//! instance, all in the instance's Kubernetes namespace and labelled
//! `loams.dev/instance=<id>`:
//!
//! - a ServiceAccount, Role and RoleBinding that let the server create its
//!   own worker/executor pods (Sail `kubernetes-cluster` mode; Spark's
//!   client-mode driver);
//! - a ConfigMap with `spark-defaults.conf` (Apache Spark only);
//! - the server Deployment (one replica; zero when suspended);
//! - a ClusterIP Service for the proxy and, for Apache Spark, a headless
//!   driver Service the executors call back on;
//! - NetworkPolicies: the server's Spark Connect port admits only the proxy;
//!   workers talk only to their own instance.
//!
//! Secrets are referenced, never rendered: `<name>-store` (the instance's
//! prefix-scoped S3 keys, minted by the facade, R5) and, for Apache Spark,
//! `<name>-connect` (`SPARK_CONNECT_AUTHENTICATE_TOKEN`, which the proxy's
//! route also reads).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::backend::{BackendKind, SPARK_CONF_DIR, SparkBackend};
use crate::conf::{ClusterConf, ConfError, Placement};

/// The label every object of an instance carries.
pub const INSTANCE_LABEL: &str = "loams.dev/instance";

/// The proxy pods' label the NetworkPolicy admits.
pub const PROXY_SELECTOR: (&str, &str) = ("app.kubernetes.io/name", "loams-spark-proxy");

/// A DNS-label form of an instance id (`eng_01…` → `eng-01…`).
pub fn object_name(instance_id: &str) -> String {
    instance_id.replace('_', "-").to_ascii_lowercase()
}

/// The Secret holding the instance's S3 keys (`AWS_ACCESS_KEY_ID`,
/// `AWS_SECRET_ACCESS_KEY`).
pub fn store_secret_name(instance_id: &str) -> String {
    format!("{}-store", object_name(instance_id))
}

/// The Secret holding Apache Spark's pre-shared Connect token.
pub fn connect_secret_name(instance_id: &str) -> String {
    format!("{}-connect", object_name(instance_id))
}

/// The server Deployment's name.
pub fn server_name(instance_id: &str) -> String {
    format!("{}-server", object_name(instance_id))
}

/// The in-cluster gRPC URI the proxy routes to.
pub fn backend_uri(backend: &dyn SparkBackend, instance_id: &str, k8s_namespace: &str) -> String {
    format!(
        "http://{}.{k8s_namespace}.svc:{}",
        object_name(instance_id),
        backend.port()
    )
}

/// Rendering options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeOptions {
    /// The image registry, e.g. `ghcr.io/ostrium-labs`.
    pub registry: String,
    /// Server replicas: 1 running, 0 suspended.
    pub replicas: u8,
    /// The server's CPU request, in milli.
    pub server_vcpu_milli: u32,
}

fn labels(instance_id: &str, backend: BackendKind, component: &str) -> Value {
    json!({
        "app.kubernetes.io/name": match backend { BackendKind::Sail => "sail", BackendKind::Spark => "spark" },
        "app.kubernetes.io/component": component,
        "app.kubernetes.io/managed-by": "loams",
        INSTANCE_LABEL: object_name(instance_id),
    })
}

fn env_list(env: &BTreeMap<String, String>) -> Value {
    Value::Array(
        env.iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect(),
    )
}

/// The objects of one instance, in apply order.
///
/// # Errors
///
/// [`ConfError`] when the conf cannot render or is not a Kubernetes one.
pub fn render(
    backend: &dyn SparkBackend,
    conf: &ClusterConf,
    opts: &KubeOptions,
) -> Result<Vec<Value>, ConfError> {
    let Placement::Kubernetes(kube) = &conf.placement else {
        return Err(ConfError::Invalid(
            "Kubernetes objects need a Kubernetes placement".into(),
        ));
    };
    let ns = kube.namespace.as_str();
    let id = conf.instance_id.as_str();
    let name = object_name(id);
    let kind = backend.kind();
    let server_labels = labels(id, kind, "server");
    let mut out = Vec::new();

    out.push(json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": { "name": kube.service_account, "namespace": ns, "labels": server_labels },
        "automountServiceAccountToken": true,
    }));
    out.push(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "Role",
        "metadata": { "name": format!("{name}-workers"), "namespace": ns, "labels": server_labels },
        "rules": [
            { "apiGroups": [""], "resources": ["pods"],
              "verbs": ["create", "delete", "deletecollection", "get", "list", "watch", "patch"] },
            { "apiGroups": [""], "resources": ["configmaps", "services", "persistentvolumeclaims"],
              "verbs": ["create", "delete", "deletecollection", "get", "list", "watch"] },
        ],
    }));
    out.push(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": { "name": format!("{name}-workers"), "namespace": ns, "labels": server_labels },
        "subjects": [{ "kind": "ServiceAccount", "name": kube.service_account, "namespace": ns }],
        "roleRef": { "apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": format!("{name}-workers") },
    }));

    let files = backend.config_files(conf)?;
    let mut volumes = Vec::new();
    let mut mounts = Vec::new();
    if !files.is_empty() {
        out.push(json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": { "name": format!("{name}-conf"), "namespace": ns, "labels": server_labels },
            "data": files,
        }));
        volumes.push(json!({ "name": "conf", "configMap": { "name": format!("{name}-conf") } }));
        mounts.push(json!({ "name": "conf", "mountPath": SPARK_CONF_DIR, "readOnly": true }));
    }
    volumes.push(json!({ "name": "tmp", "emptyDir": {} }));
    mounts.push(json!({ "name": "tmp", "mountPath": "/tmp" }));

    let mut env = env_list(&backend.env(conf)?);
    if let Value::Array(items) = &mut env {
        // Sail's driver tells workers where it is.
        if kind == BackendKind::Sail {
            items.push(json!({ "name": "SAIL_CLUSTER__DRIVER_EXTERNAL_HOST",
                               "valueFrom": { "fieldRef": { "fieldPath": "status.podIP" } } }));
            items.push(json!({ "name": "SAIL_KUBERNETES__DRIVER_POD_NAME",
                               "valueFrom": { "fieldRef": { "fieldPath": "metadata.name" } } }));
        }
    }
    let mut env_from = vec![json!({ "secretRef": { "name": store_secret_name(id) } })];
    if backend.has_token_auth() {
        env_from.push(json!({ "secretRef": { "name": connect_secret_name(id) } }));
    }
    let port = backend.port();
    let mut ports = vec![json!({ "name": "connect", "containerPort": port })];
    if kind == BackendKind::Spark {
        ports.push(json!({ "name": "driver", "containerPort": 7078 }));
        ports.push(json!({ "name": "blockmanager", "containerPort": 7079 }));
        ports.push(json!({ "name": "ui", "containerPort": 4040 }));
    }
    let memory = format!("{}Mi", conf.shape.driver_memory_mib);
    out.push(json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": { "name": server_name(id), "namespace": ns, "labels": server_labels },
        "spec": {
            // One server per instance: a Spark session lives in one process.
            "replicas": opts.replicas,
            "strategy": { "type": "Recreate" },
            "selector": { "matchLabels": server_labels },
            "template": {
                "metadata": { "labels": server_labels },
                "spec": {
                    "serviceAccountName": kube.service_account,
                    "enableServiceLinks": false,
                    "securityContext": { "runAsNonRoot": true, "fsGroup": 185,
                                         "seccompProfile": { "type": "RuntimeDefault" } },
                    "containers": [{
                        "name": "server",
                        "image": kube.image,
                        "imagePullPolicy": "IfNotPresent",
                        "command": backend.command(),
                        "env": env,
                        "envFrom": env_from,
                        "ports": ports,
                        "readinessProbe": { "tcpSocket": { "port": port },
                                            "periodSeconds": 5, "failureThreshold": 3 },
                        "livenessProbe": { "tcpSocket": { "port": port },
                                           "initialDelaySeconds": 60, "periodSeconds": 20 },
                        "resources": {
                            "requests": { "cpu": format!("{}m", opts.server_vcpu_milli), "memory": memory },
                            "limits": { "memory": memory },
                        },
                        "volumeMounts": mounts,
                        "securityContext": { "allowPrivilegeEscalation": false,
                                             "capabilities": { "drop": ["ALL"] } },
                    }],
                    "volumes": volumes,
                },
            },
        },
    }));
    out.push(json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": { "name": name, "namespace": ns, "labels": server_labels },
        "spec": {
            "type": "ClusterIP",
            "selector": server_labels,
            "ports": [{ "name": "connect", "port": port, "targetPort": port, "appProtocol": "kubernetes.io/h2c" }],
        },
    }));
    if kind == BackendKind::Spark {
        out.push(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": { "name": kube.driver_service, "namespace": ns, "labels": server_labels },
            "spec": {
                "clusterIP": "None",
                "selector": server_labels,
                "ports": [
                    { "name": "driver", "port": 7078, "targetPort": 7078 },
                    { "name": "blockmanager", "port": 7079, "targetPort": 7079 },
                ],
            },
        }));
    }

    // Network: the proxy reaches the Connect port; the instance's own pods
    // reach each other; nothing else gets in.
    let same_instance = json!({ "podSelector": { "matchLabels": { INSTANCE_LABEL: name } } });
    out.push(json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": format!("{name}-ingress"), "namespace": ns, "labels": server_labels },
        "spec": {
            "podSelector": { "matchLabels": { INSTANCE_LABEL: name } },
            "policyTypes": ["Ingress"],
            "ingress": [
                {
                    "from": [{ "namespaceSelector": {},
                               "podSelector": { "matchLabels": { PROXY_SELECTOR.0: PROXY_SELECTOR.1 } } }],
                    "ports": [{ "protocol": "TCP", "port": port }],
                },
                { "from": [same_instance] },
            ],
        },
    }));
    Ok(out)
}

/// The objects as one multi-document YAML-compatible JSON stream (`kubectl
/// apply -f -` accepts a JSON `List`).
pub fn as_list(objects: Vec<Value>) -> Value {
    json!({ "apiVersion": "v1", "kind": "List", "items": objects })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ApacheSpark, Sail};
    use crate::conf::{Catalog, KubePlacement, ObjectStore, Shape};

    fn conf() -> ClusterConf {
        ClusterConf {
            namespace: "ns-a".into(),
            instance_id: "eng_01jabc".into(),
            state_prefix: "engines/ns-a/eng_01jabc/".into(),
            store: ObjectStore::default(),
            catalog: Catalog::default(),
            placement: Placement::Kubernetes(KubePlacement {
                namespace: "loams-ns-a".into(),
                image: "ghcr.io/ostrium-labs/loams-sail:0.7.2".into(),
                service_account: "eng-01jabc-spark".into(),
                driver_service: "eng-01jabc-driver".into(),
            }),
            shape: Shape {
                workers: 2,
                worker_vcpu_milli: 1000,
                worker_memory_mib: 4096,
                driver_memory_mib: 2048,
            },
            proxy_idle_timeout_secs: 1800,
            otlp_endpoint: None,
        }
    }

    fn opts() -> KubeOptions {
        KubeOptions {
            registry: "ghcr.io/ostrium-labs".into(),
            replicas: 1,
            server_vcpu_milli: 500,
        }
    }

    fn by_kind<'a>(objs: &'a [Value], kind: &str) -> Vec<&'a Value> {
        objs.iter().filter(|o| o["kind"] == kind).collect()
    }

    #[test]
    fn sail_objects() {
        let objs = render(&Sail, &conf(), &opts()).unwrap();
        assert!(by_kind(&objs, "ConfigMap").is_empty());
        let dep = by_kind(&objs, "Deployment")[0];
        assert_eq!(dep["metadata"]["name"], "eng-01jabc-server");
        assert_eq!(dep["spec"]["replicas"], 1);
        let c = &dep["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(c["command"][0], "sail");
        assert_eq!(c["envFrom"].as_array().unwrap().len(), 1);
        assert_eq!(c["envFrom"][0]["secretRef"]["name"], "eng-01jabc-store");
        let env = c["env"].as_array().unwrap();
        assert!(
            env.iter()
                .any(|e| e["name"] == "SAIL_MODE" && e["value"] == "kubernetes-cluster")
        );
        assert!(
            env.iter()
                .any(|e| e["name"] == "SAIL_CLUSTER__DRIVER_EXTERNAL_HOST")
        );
        assert!(!env.iter().any(|e| e["name"] == "AWS_SECRET_ACCESS_KEY"));
        assert_eq!(by_kind(&objs, "Service").len(), 1);
        assert_eq!(
            backend_uri(&Sail, "eng_01jabc", "loams-ns-a"),
            "http://eng-01jabc.loams-ns-a.svc:50051"
        );
    }

    #[test]
    fn spark_objects() {
        let objs = render(&ApacheSpark, &conf(), &opts()).unwrap();
        let cm = by_kind(&objs, "ConfigMap")[0];
        assert!(
            cm["data"]["spark-defaults.conf"]
                .as_str()
                .unwrap()
                .contains("spark.master k8s://https://kubernetes.default.svc")
        );
        let services = by_kind(&objs, "Service");
        assert_eq!(services.len(), 2);
        assert_eq!(services[1]["spec"]["clusterIP"], "None");
        let c = &by_kind(&objs, "Deployment")[0]["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(c["envFrom"][1]["secretRef"]["name"], "eng-01jabc-connect");
        assert_eq!(c["volumeMounts"][0]["mountPath"], SPARK_CONF_DIR);
    }

    #[test]
    fn network_policy_admits_only_the_proxy_on_connect() {
        let objs = render(&Sail, &conf(), &opts()).unwrap();
        let np = by_kind(&objs, "NetworkPolicy")[0];
        let first = &np["spec"]["ingress"][0];
        assert_eq!(first["ports"][0]["port"], 50051);
        assert_eq!(
            first["from"][0]["podSelector"]["matchLabels"]["app.kubernetes.io/name"],
            "loams-spark-proxy"
        );
    }

    #[test]
    fn suspended_has_zero_replicas_and_local_is_refused() {
        let mut o = opts();
        o.replicas = 0;
        let objs = render(&Sail, &conf(), &o).unwrap();
        assert_eq!(by_kind(&objs, "Deployment")[0]["spec"]["replicas"], 0);
        let mut c = conf();
        c.placement = Placement::Local;
        assert!(render(&Sail, &c, &opts()).is_err());
    }
}
