//! The compose runtime: one compose project `rw-<key>` per instance, one
//! service per replica (each advertises its own service name), on the
//! Loams stack's network so meta nodes reach Loams Postgres and Hummock
//! reaches the bucket. Nothing publishes a host port: clients come through
//! loams-gateway.
//!
//! The project is written as JSON (a YAML subset compose reads) next to
//! `risingwave.toml` and one `<role>.secrets.env` per role (mode 0600), so
//! each role sees only its own credentials.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Runtime, RuntimeError, run};
use crate::model::InstanceKey;
use crate::render::{CONFIG_PATH, EnvValue, HostStyle, RenderedCluster, RenderedRole};

/// Runs instances' clusters with `podman compose` (or `docker compose`).
#[derive(Debug, Clone)]
pub struct ComposeRuntime {
    /// Each instance's files go in `<dir>/rw-<key>/`.
    pub dir: PathBuf,
    /// `podman`.
    pub program: String,
    /// `["compose"]`.
    pub base_args: Vec<String>,
    /// The external network of the Loams stack (`loams_default`).
    pub network: String,
}

impl ComposeRuntime {
    pub fn new(dir: impl Into<PathBuf>, network: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            program: "podman".into(),
            base_args: vec!["compose".into()],
            network: network.into(),
        }
    }

    fn project(key: &InstanceKey) -> String {
        format!("rw-{key}")
    }

    fn project_dir(&self, key: &InstanceKey) -> PathBuf {
        self.dir.join(Self::project(key))
    }

    fn args(&self, key: &InstanceKey, rest: &[&str]) -> Vec<String> {
        let dir = self.project_dir(key);
        let mut args = self.base_args.clone();
        args.extend([
            "--project-name".to_owned(),
            Self::project(key),
            "--file".to_owned(),
            dir.join("compose.json").display().to_string(),
        ]);
        args.extend(rest.iter().map(|s| (*s).to_owned()));
        args
    }
}

/// The compose project of a rendered cluster.
pub fn compose_project(cluster: &RenderedCluster, network: &str) -> Value {
    let hosts = HostStyle::Compose;
    let mut services = serde_json::Map::new();
    for role in &cluster.roles {
        for i in 0..role.replicas {
            let host = hosts.replica_host(&cluster.key, role.role, i);
            services.insert(host.clone(), service(role, &host));
        }
    }
    json!({
        "name": format!("rw-{}", cluster.key),
        "services": services,
        "networks": {
            "loams": { "external": true, "name": network }
        }
    })
}

fn service(role: &RenderedRole, host: &str) -> Value {
    let mut environment = BTreeMap::new();
    for (k, v) in &role.env {
        match v {
            EnvValue::Plain(s) => {
                environment.insert(k.clone(), s.clone());
            }
            EnvValue::Advertise(port) => {
                environment.insert(k.clone(), format!("{host}:{port}"));
            }
            // In the role's env file.
            EnvValue::Secret(_) => {}
        }
    }
    let cpus = f64::from(role.resources.cpu_milli) / 1000.0;
    json!({
        "image": role.image,
        "command": role.args,
        "hostname": host,
        "environment": environment,
        "env_file": [format!("{}.secrets.env", role.role)],
        "volumes": [format!("./risingwave.toml:{CONFIG_PATH}:ro")],
        "networks": ["loams"],
        "restart": "unless-stopped",
        "cpus": cpus,
        "mem_limit": role.resources.memory_bytes,
        "labels": role.labels,
        "cap_drop": ["ALL"],
        "security_opt": ["no-new-privileges:true"],
    })
}

/// A role's `KEY=value` env file: only the secrets its env names.
pub fn secrets_env(cluster: &RenderedCluster, role: &RenderedRole) -> Result<String, RuntimeError> {
    let mut out = String::new();
    for (k, v) in &role.env {
        if let EnvValue::Secret(key) = v {
            let secret = cluster
                .secrets
                .get(key)
                .ok_or_else(|| RuntimeError::Render(format!("no secret {key} for {k}")))?;
            let value = secret.expose();
            if value.contains(['\n', '\r']) {
                return Err(RuntimeError::Render(format!("secret {key} spans lines")));
            }
            out.push_str(&format!("{k}={value}\n"));
        }
    }
    Ok(out)
}

async fn write_file(path: &Path, contents: &str, private: bool) -> Result<(), RuntimeError> {
    let io = |e: std::io::Error| RuntimeError::Io(format!("{}: {e}", path.display()));
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, contents).await.map_err(io)?;
    if private {
        make_private(&tmp).await.map_err(io)?;
    }
    tokio::fs::rename(&tmp, path).await.map_err(io)
}

#[cfg(unix)]
async fn make_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await
}

#[cfg(not(unix))]
async fn make_private(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[async_trait]
impl Runtime for ComposeRuntime {
    async fn apply(&self, cluster: &RenderedCluster) -> Result<(), RuntimeError> {
        let dir = self.project_dir(&cluster.key);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| RuntimeError::Io(format!("{}: {e}", dir.display())))?;
        let project = compose_project(cluster, &self.network);
        let text = serde_json::to_string_pretty(&project)
            .map_err(|e| RuntimeError::Render(e.to_string()))?;
        write_file(&dir.join("risingwave.toml"), &cluster.config_toml, false).await?;
        for role in &cluster.roles {
            let env = secrets_env(cluster, role)?;
            write_file(&dir.join(format!("{}.secrets.env", role.role)), &env, true).await?;
        }
        write_file(&dir.join("compose.json"), &text, false).await?;

        let running = cluster.roles.iter().any(|r| r.replicas > 0);
        let args = if running {
            self.args(&cluster.key, &["up", "--detach", "--remove-orphans"])
        } else {
            // Suspended: no services; stop the project, keep its files.
            self.args(&cluster.key, &["down", "--remove-orphans"])
        };
        run(&self.program, &args).await.map(|_| ())
    }

    async fn remove(&self, key: &InstanceKey) -> Result<(), RuntimeError> {
        let dir = self.project_dir(key);
        if tokio::fs::try_exists(dir.join("compose.json"))
            .await
            .unwrap_or(false)
        {
            run(
                &self.program,
                &self.args(key, &["down", "--remove-orphans"]),
            )
            .await?;
        }
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RuntimeError::Io(format!("{}: {e}", dir.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{ClusterSpec, Role, Size};
    use crate::render::tests::{key, meta_db, store};
    use crate::render::{ImagePolicy, RenderInput, render};

    fn cluster(size: Size, suspended: bool) -> RenderedCluster {
        let t = key();
        let db = meta_db();
        let s = store();
        let images = ImagePolicy::default();
        let versions = BTreeMap::new();
        let spec = ClusterSpec::new(size);
        render(&RenderInput {
            key: &t,
            spec: &spec,
            meta_db: &db,
            store: &s,
            images: &images,
            hosts: &HostStyle::Compose,
            role_versions: &versions,
            suspended,
        })
        .unwrap()
    }

    #[test]
    fn one_service_per_replica() {
        let c = cluster(Size::M, false);
        let p = compose_project(&c, "loams_default");
        let services = p["services"].as_object().unwrap();
        // m: 1 meta, 1 compactor, 2 compute, 1 frontend.
        assert_eq!(services.len(), 5);
        assert!(services.contains_key("rw-acme-compute-1"));
        let compute = &services["rw-acme-compute-1"];
        assert_eq!(
            compute["environment"]["RW_ADVERTISE_ADDR"],
            "rw-acme-compute-1:5688"
        );
        assert_eq!(compute["command"][0], "compute-node");
        assert_eq!(compute["env_file"][0], "compute.secrets.env");
        assert!(
            compute["environment"]
                .get("AWS_SECRET_ACCESS_KEY")
                .is_none()
        );
        assert!(compute.get("ports").is_none(), "no host ports");
        assert_eq!(p["networks"]["loams"]["name"], "loams_default");
        assert_eq!(p["name"], "rw-acme");
    }

    #[test]
    fn suspended_has_no_services() {
        let c = cluster(Size::Xs, true);
        let p = compose_project(&c, "loams_default");
        assert!(p["services"].as_object().unwrap().is_empty());
    }

    #[test]
    fn secrets_go_to_their_role_only() {
        let c = cluster(Size::Xs, false);
        let meta = secrets_env(&c, c.role(Role::Meta)).unwrap();
        assert!(meta.contains("RW_SQL_PASSWORD=pw0123456789\n"));
        assert!(meta.contains("AWS_SECRET_ACCESS_KEY=sk\n"));
        let frontend = secrets_env(&c, c.role(Role::Frontend)).unwrap();
        assert!(!frontend.contains("RW_SQL_PASSWORD"));
        assert!(frontend.contains("AWS_ACCESS_KEY_ID=AKacme\n"));
    }

    #[test]
    fn compose_args() {
        let rt = ComposeRuntime::new("/var/lib/loams/risingwave", "loams_default");
        let args = rt.args(&key(), &["up", "--detach"]);
        assert_eq!(
            args,
            vec![
                "compose",
                "--project-name",
                "rw-acme",
                "--file",
                "/var/lib/loams/risingwave/rw-acme/compose.json",
                "up",
                "--detach"
            ]
        );
    }
}
