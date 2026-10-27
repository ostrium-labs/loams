//! The Helm runtime: one release `rw-<key>` of the chart
//! `deploy/helm/loams-risingwave` per instance, in the instance's Kubernetes
//! namespace. The controller renders the chart's values ([`helm_values`]);
//! the chart only lays them out as StatefulSets (meta, compute),
//! Deployments (compactor, frontend), Services, the ConfigMap, the Secret
//! and the NetworkPolicy.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Runtime, RuntimeError, run};
use crate::model::InstanceKey;
use crate::render::{EnvValue, RenderedCluster, RenderedRole};

/// Runs instances' clusters with `helm`.
#[derive(Debug, Clone)]
pub struct HelmRuntime {
    pub program: String,
    /// A chart path or OCI reference.
    pub chart: String,
    /// The Kubernetes namespace of an instance; `{key}` is replaced by its
    /// key (`eng-{key}`, one namespace per instance, ENG81 R24).
    pub namespace_template: String,
    /// Where values files are written while helm runs (they carry the
    /// credentials and are removed after).
    pub work_dir: PathBuf,
    /// Labels of the pods allowed to reach the frontend (loams-gateway)
    /// and the meta dashboard (the controller).
    pub gateway_pod_labels: serde_json::Map<String, Value>,
    pub controller_pod_labels: serde_json::Map<String, Value>,
    /// The gateway's and controller's namespace.
    pub control_namespace: String,
}

impl HelmRuntime {
    pub fn namespace(&self, key: &InstanceKey) -> String {
        self.namespace_template.replace("{key}", key.as_str())
    }

    pub fn release(key: &InstanceKey) -> String {
        format!("rw-{key}")
    }
}

/// The chart's values for a rendered cluster.
pub fn helm_values(cluster: &RenderedCluster, rt: Option<&HelmRuntime>) -> Value {
    let mut roles = serde_json::Map::new();
    for role in &cluster.roles {
        roles.insert(role.role.name().to_owned(), role_values(role));
    }
    let credentials: serde_json::Map<String, Value> = cluster
        .secrets
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.expose().to_owned())))
        .collect();
    let mut values = json!({
        "instance": cluster.key.as_str(),
        "secretName": cluster.secret_name,
        "credentials": credentials,
        "configName": cluster.config_name,
        "config": cluster.config_toml,
        "roles": roles,
    });
    if let Some(rt) = rt {
        values["networkPolicy"] = json!({
            "enabled": true,
            "controlNamespace": rt.control_namespace,
            "gatewayPodLabels": rt.gateway_pod_labels,
            "controllerPodLabels": rt.controller_pod_labels,
        });
    }
    values
}

fn role_values(role: &RenderedRole) -> Value {
    let env: Vec<Value> = role
        .env
        .iter()
        .map(|(name, v)| match v {
            EnvValue::Plain(s) => json!({ "name": name, "value": s }),
            EnvValue::Secret(key) => json!({ "name": name, "secretKey": key }),
            // The chart defines POD_IP first, so Kubernetes expands it.
            EnvValue::Advertise(port) => {
                json!({ "name": name, "value": format!("$(POD_IP):{port}") })
            }
        })
        .collect();
    let ports: Vec<Value> = role
        .ports
        .iter()
        .map(|p| json!({ "name": p.name, "containerPort": p.port }))
        .collect();
    let memory = role.resources.memory_bytes.to_string();
    json!({
        "name": role.name,
        "image": role.image,
        "replicas": role.replicas,
        "stateful": role.role.is_stateful(),
        "args": role.args,
        "env": env,
        "ports": ports,
        "labels": role.labels,
        "resources": {
            "requests": { "cpu": format!("{}m", role.resources.cpu_milli), "memory": memory },
            "limits": { "memory": memory },
        },
    })
}

#[async_trait]
impl Runtime for HelmRuntime {
    async fn apply(&self, cluster: &RenderedCluster) -> Result<(), RuntimeError> {
        let values = helm_values(cluster, Some(self));
        let text =
            serde_json::to_string(&values).map_err(|e| RuntimeError::Render(e.to_string()))?;
        tokio::fs::create_dir_all(&self.work_dir)
            .await
            .map_err(|e| RuntimeError::Io(e.to_string()))?;
        let path = self
            .work_dir
            .join(format!("{}.values.json", Self::release(&cluster.key)));
        write_private(&path, &text).await?;
        let args = vec![
            "upgrade".to_owned(),
            "--install".to_owned(),
            Self::release(&cluster.key),
            self.chart.clone(),
            "--namespace".to_owned(),
            self.namespace(&cluster.key),
            "--create-namespace".to_owned(),
            "--values".to_owned(),
            path.display().to_string(),
        ];
        let result = run(&self.program, &args).await;
        let _ = tokio::fs::remove_file(&path).await;
        result.map(|_| ())
    }

    async fn remove(&self, key: &InstanceKey) -> Result<(), RuntimeError> {
        let args = vec![
            "uninstall".to_owned(),
            Self::release(key),
            "--namespace".to_owned(),
            self.namespace(key),
            "--ignore-not-found".to_owned(),
        ];
        run(&self.program, &args).await.map(|_| ())
    }
}

async fn write_private(path: &std::path::Path, text: &str) -> Result<(), RuntimeError> {
    let io = |e: std::io::Error| RuntimeError::Io(format!("{}: {e}", path.display()));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = path.to_owned();
        let text = text.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            f.write_all(text.as_bytes())
        })
        .await
        .map_err(|e| RuntimeError::Io(e.to_string()))?
        .map_err(io)
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(path, text).await.map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{ClusterSpec, Size};
    use crate::render::tests::{key, meta_db, store};
    use crate::render::{HostStyle, ImagePolicy, RenderInput, render, secret_keys};

    fn cluster() -> RenderedCluster {
        let t = key();
        let db = meta_db();
        let s = store();
        let images = ImagePolicy::default();
        let versions = BTreeMap::new();
        let spec = ClusterSpec::new(Size::M);
        let hosts = HostStyle::Kubernetes {
            namespace: "eng-acme".into(),
            cluster_domain: "cluster.local".into(),
        };
        render(&RenderInput {
            key: &t,
            spec: &spec,
            meta_db: &db,
            store: &s,
            images: &images,
            hosts: &hosts,
            role_versions: &versions,
            suspended: false,
        })
        .unwrap()
    }

    #[test]
    fn values_carry_every_role() {
        let v = helm_values(&cluster(), None);
        assert_eq!(v["instance"], "acme");
        assert_eq!(v["secretName"], "rw-acme-credentials");
        assert_eq!(
            v["credentials"][secret_keys::META_DB_PASSWORD],
            "pw0123456789"
        );
        for role in ["meta", "compactor", "compute", "frontend"] {
            assert!(v["roles"][role].is_object(), "{role}");
        }
        assert_eq!(v["roles"]["meta"]["stateful"], true);
        assert_eq!(v["roles"]["frontend"]["stateful"], false);
        assert_eq!(v["roles"]["compute"]["replicas"], 2);
        assert_eq!(
            v["roles"]["compute"]["resources"]["requests"]["cpu"],
            "2000m"
        );
        let env = v["roles"]["meta"]["env"].as_array().unwrap();
        let get = |name: &str| env.iter().find(|e| e["name"] == name).cloned().unwrap();
        assert_eq!(get("RW_ADVERTISE_ADDR")["value"], "$(POD_IP):5690");
        assert_eq!(
            get("RW_SQL_PASSWORD")["secretKey"],
            secret_keys::META_DB_PASSWORD
        );
        assert!(v.get("networkPolicy").is_none());
    }

    #[test]
    fn namespaces_and_releases() {
        let rt = HelmRuntime {
            program: "helm".into(),
            chart: "deploy/helm/loams-risingwave".into(),
            namespace_template: "eng-{key}".into(),
            work_dir: "/var/lib/loams/helm".into(),
            gateway_pod_labels: serde_json::Map::new(),
            controller_pod_labels: serde_json::Map::new(),
            control_namespace: "loams-system".into(),
        };
        assert_eq!(rt.namespace(&key()), "eng-acme");
        assert_eq!(HelmRuntime::release(&key()), "rw-acme");
        let v = helm_values(&cluster(), Some(&rt));
        assert_eq!(v["networkPolicy"]["controlNamespace"], "loams-system");
    }
}
