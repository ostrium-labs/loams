//! The Kubernetes objects of one tenant's session cluster (D1972, D1974,
//! D1977), as JSON in apply order. `runtime_kube` applies them with
//! server-side apply; `deploy/flink/k8s/` is the same set for one example
//! tenant, by hand, for operators without the controller.
//!
//! Everything lives in the namespace `loams-flink-<tenant>`:
//!
//! - `flink` ServiceAccount, and a Role for Kubernetes HA's ConfigMaps;
//! - `flink-config` ConfigMap (`config.yaml`, [`crate::config::render`]);
//! - `flink-s3` Secret (`FLINK_PROPERTIES`: the S3 keys);
//! - Deployments `flink-jobmanager`, `flink-taskmanager`,
//!   `flink-sql-gateway`, and their Services;
//! - NetworkPolicies: nothing reaches the cluster but its own pods,
//!   loams-gateway and the engine controller (and the metrics scraper on
//!   9249).
//!
//! The containers start from a shell line that copies the rendered
//! `config.yaml` over the image's (the ConfigMap is read-only, and the
//! image's entrypoint appends `FLINK_PROPERTIES` to the file), then runs
//! the image's own entrypoint.

use serde_json::{Value, json};

use crate::config::{
    ClusterSpec, FlinkConfig, HaMode, S3Credentials, addr, flink_properties, s3_plugin_jar,
};
use crate::error::FlinkError;

/// The field manager of server-side apply.
pub const FIELD_MANAGER: &str = "loams-flink";
/// The label loams-gateway's pods carry (admitted to 8083, D1977).
pub const GATEWAY_POD_LABEL: (&str, &str) = ("app.kubernetes.io/name", "loams-gateway");
/// The label the engine controller's pods carry (admitted to 8081 and 8083).
pub const CONTROLLER_POD_LABEL: (&str, &str) = ("loams.dev/component", "engine-controller");
/// The label of the metrics scraper (admitted to 9249).
pub const METRICS_POD_LABEL: (&str, &str) = ("loams.dev/component", "metrics");
/// The UID the official image's `flink` user has.
pub const FLINK_UID: i64 = 9999;
/// Where the ConfigMap is mounted.
pub const CONFIG_MOUNT: &str = "/etc/loams-flink";

/// A cluster role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The JobManager.
    JobManager,
    /// The TaskManagers.
    TaskManager,
    /// The SQL Gateway.
    SqlGateway,
}

impl Role {
    /// All roles, in start order.
    pub const ALL: [Role; 3] = [Role::JobManager, Role::TaskManager, Role::SqlGateway];

    /// The Deployment (and compose service) name.
    pub fn name(self) -> &'static str {
        match self {
            Role::JobManager => addr::JOBMANAGER,
            Role::TaskManager => "flink-taskmanager",
            Role::SqlGateway => addr::SQL_GATEWAY,
        }
    }

    /// `app.kubernetes.io/component`.
    pub fn component(self) -> &'static str {
        match self {
            Role::JobManager => "jobmanager",
            Role::TaskManager => "taskmanager",
            Role::SqlGateway => "sql-gateway",
        }
    }
}

/// The replicas each role runs with when the cluster is up.
pub fn replicas(spec: &ClusterSpec, role: Role) -> u32 {
    match role {
        Role::JobManager => match spec.ha {
            HaMode::Kubernetes => 2,
            HaMode::ControllerRecovery => 1,
        },
        Role::TaskManager => spec.resources.taskmanagers,
        Role::SqlGateway => 1,
    }
}

/// The labels every object of the tenant carries.
pub fn labels(spec: &ClusterSpec) -> Value {
    json!({
        "app.kubernetes.io/name": "flink",
        "app.kubernetes.io/part-of": "loams",
        "app.kubernetes.io/managed-by": FIELD_MANAGER,
        "loams.dev/tenant": spec.tenant.as_str(),
    })
}

fn role_labels(spec: &ClusterSpec, role: Role) -> Value {
    let mut l = labels(spec);
    if let Some(map) = l.as_object_mut() {
        map.insert(
            "app.kubernetes.io/component".to_owned(),
            Value::String(role.component().to_owned()),
        );
    }
    l
}

fn selector(spec: &ClusterSpec, role: Role) -> Value {
    json!({
        "app.kubernetes.io/name": "flink",
        "loams.dev/tenant": spec.tenant.as_str(),
        "app.kubernetes.io/component": role.component(),
    })
}

/// The shell line a role's container runs (D1974: the JobManager's RPC
/// address is its pod IP under Kubernetes HA).
pub fn launch_script(spec: &ClusterSpec, role: Role) -> String {
    let copy = format!("cp {CONFIG_MOUNT}/config.yaml \"$FLINK_HOME/conf/config.yaml\"");
    match role {
        Role::JobManager => match spec.ha {
            HaMode::Kubernetes => {
                format!("set -eu; {copy}; exec /docker-entrypoint.sh jobmanager \"$POD_IP\"")
            }
            HaMode::ControllerRecovery => {
                format!("set -eu; {copy}; exec /docker-entrypoint.sh jobmanager")
            }
        },
        Role::TaskManager => format!("set -eu; {copy}; exec /docker-entrypoint.sh taskmanager"),
        // The entrypoint has no SQL Gateway command: append FLINK_PROPERTIES
        // and enable the S3 plugin here, as it does for the other roles.
        Role::SqlGateway => format!(
            "set -eu; {copy}; \
             if [ -n \"${{FLINK_PROPERTIES:-}}\" ]; then printf '%s\\n' \"$FLINK_PROPERTIES\" >> \"$FLINK_HOME/conf/config.yaml\"; fi; \
             mkdir -p \"$FLINK_HOME/plugins/s3-fs-presto\"; \
             cp \"$FLINK_HOME/opt/{}\" \"$FLINK_HOME/plugins/s3-fs-presto/\"; \
             exec \"$FLINK_HOME/bin/sql-gateway.sh\" start-foreground",
            s3_plugin_jar(&spec.flink_version)
        ),
    }
}

fn namespace(spec: &ClusterSpec) -> String {
    spec.tenant.cluster_name()
}

fn meta(spec: &ClusterSpec, name: &str, labels: Value) -> Value {
    json!({ "name": name, "namespace": namespace(spec), "labels": labels })
}

fn resources(memory_mb: u32, cpu_milli: u32) -> Value {
    let quantity = json!({ "cpu": format!("{cpu_milli}m"), "memory": format!("{memory_mb}Mi") });
    json!({ "requests": quantity, "limits": { "memory": format!("{memory_mb}Mi") } })
}

fn container_ports(role: Role) -> Value {
    let mut ports = match role {
        Role::JobManager => vec![
            json!({"name": "rpc", "containerPort": addr::RPC_PORT}),
            json!({"name": "blob", "containerPort": addr::BLOB_PORT}),
            json!({"name": "rest", "containerPort": addr::REST_PORT}),
        ],
        Role::TaskManager => vec![json!({"name": "rpc", "containerPort": addr::TM_RPC_PORT})],
        Role::SqlGateway => vec![json!({"name": "sql", "containerPort": addr::SQL_GATEWAY_PORT})],
    };
    ports.push(json!({"name": "metrics", "containerPort": addr::METRICS_PORT}));
    Value::Array(ports)
}

fn probes(role: Role) -> (Value, Value) {
    match role {
        Role::JobManager => (
            json!({"httpGet": {"path": "/overview", "port": addr::REST_PORT}, "periodSeconds": 10, "failureThreshold": 3}),
            json!({"tcpSocket": {"port": addr::RPC_PORT}, "initialDelaySeconds": 30, "periodSeconds": 20}),
        ),
        Role::TaskManager => (
            json!({"tcpSocket": {"port": addr::TM_RPC_PORT}, "periodSeconds": 10}),
            json!({"tcpSocket": {"port": addr::TM_RPC_PORT}, "initialDelaySeconds": 30, "periodSeconds": 20}),
        ),
        Role::SqlGateway => (
            json!({"httpGet": {"path": "/v1/info", "port": addr::SQL_GATEWAY_PORT}, "periodSeconds": 10}),
            json!({"tcpSocket": {"port": addr::SQL_GATEWAY_PORT}, "initialDelaySeconds": 30, "periodSeconds": 20}),
        ),
    }
}

/// The Deployment of `role` with `replicas`.
pub fn deployment(spec: &ClusterSpec, role: Role, replicas: u32) -> Value {
    let r = &spec.resources;
    let (memory, cpu) = match role {
        Role::JobManager => (r.jobmanager_memory_mb, r.jobmanager_cpu_milli),
        Role::TaskManager => (r.taskmanager_memory_mb, r.taskmanager_cpu_milli),
        // The gateway's JVM uses the client defaults; 1 GiB is ample.
        Role::SqlGateway => (1024, 500),
    };
    let mut env = vec![
        json!({"name": "POD_IP", "valueFrom": {"fieldRef": {"apiVersion": "v1", "fieldPath": "status.podIP"}}}),
        json!({"name": "ENABLE_BUILT_IN_PLUGINS", "value": s3_plugin_jar(&spec.flink_version)}),
        json!({"name": "FLINK_PROPERTIES", "valueFrom": {"secretKeyRef": {"name": "flink-s3", "key": "FLINK_PROPERTIES"}}}),
    ];
    if role == Role::SqlGateway {
        env.push(json!({"name": "LOAMS_FLINK_TENANT", "value": spec.tenant.as_str()}));
    }
    let (readiness, liveness) = probes(role);
    // One JobManager without Flink HA: never two at once.
    let strategy = if role == Role::JobManager && spec.ha == HaMode::ControllerRecovery {
        "Recreate"
    } else {
        "RollingUpdate"
    };
    let service_account = match (role, spec.ha) {
        (Role::JobManager | Role::TaskManager, HaMode::Kubernetes) => "flink",
        _ => "default",
    };
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": meta(spec, role.name(), role_labels(spec, role)),
        "spec": {
            "replicas": replicas,
            "selector": { "matchLabels": selector(spec, role) },
            "strategy": { "type": strategy },
            "template": {
                "metadata": { "labels": role_labels(spec, role) },
                "spec": {
                    "serviceAccountName": service_account,
                    "automountServiceAccountToken": service_account == "flink",
                    "securityContext": {
                        "runAsUser": FLINK_UID,
                        "runAsGroup": FLINK_UID,
                        "fsGroup": FLINK_UID,
                        "runAsNonRoot": true,
                        "seccompProfile": {"type": "RuntimeDefault"},
                    },
                    "containers": [{
                        "name": role.component(),
                        "image": spec.image,
                        "command": ["/bin/sh", "-c", launch_script(spec, role)],
                        "env": env,
                        "ports": container_ports(role),
                        "resources": resources(memory, cpu),
                        "readinessProbe": readiness,
                        "livenessProbe": liveness,
                        "securityContext": {
                            "allowPrivilegeEscalation": false,
                            "capabilities": {"drop": ["ALL"]},
                        },
                        "volumeMounts": [{"name": "config", "mountPath": CONFIG_MOUNT, "readOnly": true}],
                    }],
                    "volumes": [{"name": "config", "configMap": {"name": "flink-config"}}],
                },
            },
        },
    })
}

fn service(spec: &ClusterSpec, role: Role) -> Value {
    let ports = match role {
        Role::JobManager => json!([
            {"name": "rpc", "port": addr::RPC_PORT, "targetPort": addr::RPC_PORT},
            {"name": "blob", "port": addr::BLOB_PORT, "targetPort": addr::BLOB_PORT},
            {"name": "rest", "port": addr::REST_PORT, "targetPort": addr::REST_PORT},
        ]),
        Role::SqlGateway => json!([
            {"name": "sql", "port": addr::SQL_GATEWAY_PORT, "targetPort": addr::SQL_GATEWAY_PORT},
        ]),
        Role::TaskManager => json!([
            {"name": "rpc", "port": addr::TM_RPC_PORT, "targetPort": addr::TM_RPC_PORT},
        ]),
    };
    json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": meta(spec, role.name(), role_labels(spec, role)),
        "spec": {
            "type": "ClusterIP",
            "selector": selector(spec, role),
            "ports": ports,
        },
    })
}

fn peer(label: (&str, &str)) -> Value {
    let mut match_labels = serde_json::Map::new();
    match_labels.insert(label.0.to_owned(), Value::String(label.1.to_owned()));
    json!({
        "namespaceSelector": {},
        "podSelector": { "matchLabels": match_labels },
    })
}

fn network_policies(spec: &ClusterSpec) -> Vec<Value> {
    let tenant_pods = json!({"matchLabels": {"loams.dev/tenant": spec.tenant.as_str()}});
    vec![
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta(spec, "flink-default-deny", labels(spec)),
            "spec": { "podSelector": {}, "policyTypes": ["Ingress"] },
        }),
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta(spec, "flink-intra-cluster", labels(spec)),
            "spec": {
                "podSelector": tenant_pods,
                "policyTypes": ["Ingress"],
                "ingress": [{ "from": [{ "podSelector": tenant_pods }] }],
            },
        }),
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta(spec, "flink-sql-gateway-from-loams", labels(spec)),
            "spec": {
                "podSelector": { "matchLabels": selector(spec, Role::SqlGateway) },
                "policyTypes": ["Ingress"],
                "ingress": [{
                    "from": [peer(GATEWAY_POD_LABEL), peer(CONTROLLER_POD_LABEL)],
                    "ports": [{"protocol": "TCP", "port": addr::SQL_GATEWAY_PORT}],
                }],
            },
        }),
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta(spec, "flink-rest-from-controller", labels(spec)),
            "spec": {
                "podSelector": { "matchLabels": selector(spec, Role::JobManager) },
                "policyTypes": ["Ingress"],
                "ingress": [{
                    "from": [peer(CONTROLLER_POD_LABEL)],
                    "ports": [{"protocol": "TCP", "port": addr::REST_PORT}],
                }],
            },
        }),
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta(spec, "flink-metrics", labels(spec)),
            "spec": {
                "podSelector": tenant_pods,
                "policyTypes": ["Ingress"],
                "ingress": [{
                    "from": [peer(METRICS_POD_LABEL)],
                    "ports": [{"protocol": "TCP", "port": addr::METRICS_PORT}],
                }],
            },
        }),
    ]
}

/// Every object of the tenant's cluster, in apply order, with each role at
/// its running replica count. `config` is [`crate::config::render`]'s.
///
/// # Errors
///
/// [`FlinkError::Invalid`] from [`flink_properties`].
pub fn objects(
    spec: &ClusterSpec,
    config: &FlinkConfig,
    creds: &S3Credentials,
) -> Result<Vec<Value>, FlinkError> {
    objects_with_replicas(spec, config, creds, |role| replicas(spec, role))
}

/// [`objects`] with each role's replicas from `replicas_of` (0 for a
/// suspended cluster).
///
/// # Errors
///
/// As [`objects`].
pub fn objects_with_replicas(
    spec: &ClusterSpec,
    config: &FlinkConfig,
    creds: &S3Credentials,
    replicas_of: impl Fn(Role) -> u32,
) -> Result<Vec<Value>, FlinkError> {
    let ns = namespace(spec);
    let mut out = vec![json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": {
            "name": ns,
            "labels": labels(spec),
        },
    })];
    out.push(json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": meta(spec, "flink", labels(spec)),
    }));
    if spec.ha == HaMode::Kubernetes {
        // Kubernetes HA's leader election and job metadata (Flink's docs:
        // ConfigMaps create/get/list/watch/update/patch/delete).
        out.push(json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "Role",
            "metadata": meta(spec, "flink-ha", labels(spec)),
            "rules": [{
                "apiGroups": [""],
                "resources": ["configmaps"],
                "verbs": ["create", "get", "list", "watch", "update", "patch", "delete"],
            }],
        }));
        out.push(json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "RoleBinding",
            "metadata": meta(spec, "flink-ha", labels(spec)),
            "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "flink-ha"},
            "subjects": [{"kind": "ServiceAccount", "name": "flink", "namespace": ns}],
        }));
    }
    out.push(json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": meta(spec, "flink-config", labels(spec)),
        "data": { "config.yaml": config.to_yaml() },
    }));
    out.push(json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": meta(spec, "flink-s3", labels(spec)),
        "type": "Opaque",
        "stringData": { "FLINK_PROPERTIES": flink_properties(creds)? },
    }));
    out.extend(network_policies(spec));
    for role in Role::ALL {
        out.push(service(spec, role));
    }
    for role in Role::ALL {
        out.push(deployment(spec, role, replicas_of(role)));
    }
    Ok(out)
}

/// A merge patch setting a Deployment's replicas.
pub fn scale_patch(replicas: u32) -> Value {
    json!({ "spec": { "replicas": replicas } })
}

/// A hash of the rendered config, put on the pod templates so a config
/// change rolls the pods.
pub fn config_hash(config: &FlinkConfig) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(config.to_yaml().as_bytes());
    hex::encode(&digest[..8])
}

/// Stamps `hash` on every Deployment's pod template
/// (`loams.dev/config-hash`).
pub fn stamp_config_hash(objects: &mut [Value], hash: &str) {
    for obj in objects.iter_mut() {
        if obj.get("kind").and_then(Value::as_str) != Some("Deployment") {
            continue;
        }
        if let Some(template_meta) = obj
            .pointer_mut("/spec/template/metadata")
            .and_then(Value::as_object_mut)
        {
            template_meta.insert(
                "annotations".to_owned(),
                json!({ "loams.dev/config-hash": hash }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Platform, Size, render};
    use crate::storage::TenantId;

    fn spec() -> ClusterSpec {
        ClusterSpec::new(
            TenantId::new("acme").unwrap(),
            Platform::Kubernetes,
            Size::Small,
        )
    }

    fn creds() -> S3Credentials {
        S3Credentials {
            access_key: "AK".into(),
            secret_key: "SK".into(),
        }
    }

    fn kinds(objs: &[Value]) -> Vec<String> {
        objs.iter()
            .map(|o| {
                format!(
                    "{}/{}",
                    o["kind"].as_str().unwrap(),
                    o["metadata"]["name"].as_str().unwrap()
                )
            })
            .collect()
    }

    #[test]
    fn objects_come_in_apply_order() {
        let spec = spec();
        let objs = objects(&spec, &render(&spec).unwrap(), &creds()).unwrap();
        assert_eq!(
            kinds(&objs),
            [
                "Namespace/loams-flink-acme",
                "ServiceAccount/flink",
                "Role/flink-ha",
                "RoleBinding/flink-ha",
                "ConfigMap/flink-config",
                "Secret/flink-s3",
                "NetworkPolicy/flink-default-deny",
                "NetworkPolicy/flink-intra-cluster",
                "NetworkPolicy/flink-sql-gateway-from-loams",
                "NetworkPolicy/flink-rest-from-controller",
                "NetworkPolicy/flink-metrics",
                "Service/flink-jobmanager",
                "Service/flink-taskmanager",
                "Service/flink-sql-gateway",
                "Deployment/flink-jobmanager",
                "Deployment/flink-taskmanager",
                "Deployment/flink-sql-gateway",
            ]
        );
        for o in &objs[1..] {
            assert_eq!(o["metadata"]["namespace"], "loams-flink-acme");
        }
    }

    #[test]
    fn ha_runs_two_jobmanagers_on_their_pod_ip() {
        let spec = spec();
        let jm = deployment(&spec, Role::JobManager, replicas(&spec, Role::JobManager));
        assert_eq!(jm["spec"]["replicas"], 2);
        let cmd = jm["spec"]["template"]["spec"]["containers"][0]["command"][2]
            .as_str()
            .unwrap();
        assert!(cmd.ends_with("jobmanager \"$POD_IP\""), "{cmd}");
        assert_eq!(
            jm["spec"]["template"]["spec"]["serviceAccountName"],
            "flink"
        );
        let tm = deployment(&spec, Role::TaskManager, 2);
        assert_eq!(
            tm["spec"]["template"]["spec"]["containers"][0]["resources"]["limits"]["memory"],
            "4096Mi"
        );
    }

    #[test]
    fn only_gateway_and_controller_reach_the_sql_gateway() {
        let spec = spec();
        let objs = objects(&spec, &render(&spec).unwrap(), &creds()).unwrap();
        let np = objs
            .iter()
            .find(|o| o["metadata"]["name"] == "flink-sql-gateway-from-loams")
            .unwrap();
        let from = np["spec"]["ingress"][0]["from"].as_array().unwrap();
        assert_eq!(
            from[0]["podSelector"]["matchLabels"]["app.kubernetes.io/name"],
            "loams-gateway"
        );
        assert_eq!(
            from[1]["podSelector"]["matchLabels"]["loams.dev/component"],
            "engine-controller"
        );
        assert_eq!(np["spec"]["ingress"][0]["ports"][0]["port"], 8083);
    }

    #[test]
    fn credentials_are_only_in_the_secret() {
        let spec = spec();
        let objs = objects(&spec, &render(&spec).unwrap(), &creds()).unwrap();
        for o in &objs {
            let text = o.to_string();
            if o["kind"] == "Secret" {
                assert!(text.contains("s3.secret-key: SK"));
            } else {
                assert!(!text.contains("s3.secret-key"), "{}", o["kind"]);
            }
        }
    }

    #[test]
    fn config_hash_changes_with_config_and_lands_on_templates() {
        let spec = spec();
        let config = render(&spec).unwrap();
        let mut other = spec.clone();
        other.checkpoint_interval_secs = 30;
        let h1 = config_hash(&config);
        assert_ne!(h1, config_hash(&render(&other).unwrap()));
        let mut objs = objects(&spec, &config, &creds()).unwrap();
        stamp_config_hash(&mut objs, &h1);
        let jm = objs.iter().find(|o| o["kind"] == "Deployment").unwrap();
        assert_eq!(
            jm["spec"]["template"]["metadata"]["annotations"]["loams.dev/config-hash"],
            h1.as_str()
        );
    }
}
