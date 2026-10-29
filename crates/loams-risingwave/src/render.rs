//! The config renderer: one instance's cluster as runtime-neutral workloads
//! (per role: image, replicas, resources, command, environment, ports), the
//! credentials Secret they read, and `risingwave.toml`.
//!
//! Every setting goes through RisingWave's `RW_*` environment variables
//! (the same names its `clap` options read), so credentials are never in a
//! command line. The runtimes ([`crate::runtime`]) turn the result into
//! compose services or Helm values.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backend::{MetaDb, ObjectStore, Secret};
use crate::model::{
    ClusterSpec, EngineVersion, InstanceKey, ModelError, Resources, Role, Topology, ports,
};

/// Where `risingwave.toml` is mounted in every container.
pub const CONFIG_PATH: &str = "/risingwave/config/risingwave.toml";

/// The licence key file meta nodes read (`RW_LICENSE_KEY_PATH`), mounted
/// next to `risingwave.toml` (D1901, ENG81 R4).
///
/// RisingWave v3.x release builds embed a default licence key
/// (`rw-default-all-4-core`, tier `all`) that turns on every Premium
/// feature for clusters of up to 4 cores and 16 GiB, which covers the
/// smaller managed sizes. Loams uses no Premium feature, so every cluster
/// gets an empty key file instead:
///
/// - an empty key is RisingWave's free tier (`rw_license()` reports tier
///   `free`, and `rw_test_feature(..)` errors for every Premium feature);
/// - while a key path is set, `ALTER SYSTEM SET license_key ..` is refused,
///   so not even a superuser can switch the built-in trial back on with
///   `ALTER SYSTEM SET license_key TO DEFAULT`.
///
/// Checked against the v3.1.0 image on 2026-10-29.
pub const LICENSE_KEY_PATH: &str = "/risingwave/config/license.key";

/// The licence key file's contents. Always empty: see [`LICENSE_KEY_PATH`].
pub const LICENSE_KEY_FILE: &str = "";

/// The keys of the credentials Secret.
pub mod secret_keys {
    pub const META_DB_PASSWORD: &str = "meta-db-password";
    pub const S3_ACCESS_KEY_ID: &str = "s3-access-key-id";
    pub const S3_SECRET_ACCESS_KEY: &str = "s3-secret-access-key";
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(
        "the meta store password must be [A-Za-z0-9]+: RisingWave splices it into a URL unencoded"
    )]
    UnsafePassword,
    #[error("no pinned image digest for RisingWave {0}")]
    NoDigest(EngineVersion),
    #[error("invalid input: {0}")]
    Invalid(String),
}

/// Which images clusters run. Images are pinned by digest (D1958).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePolicy {
    pub repository: String,
    /// Image digests by version, `sha256:<hex>`.
    pub digests: BTreeMap<EngineVersion, String>,
    /// Refuse a version without a digest.
    pub require_digest: bool,
}

/// The upstream image.
pub const DEFAULT_REPOSITORY: &str = "docker.io/risingwavelabs/risingwave";

/// v3.1.0's multi-arch index digest on Docker Hub (pushed 2026-09-21).
pub const V3_1_0_DIGEST: &str =
    "sha256:ee2e8e7a10b728d01b0e149d2a4f35864f0f52a0c889fa6cf9bc32c8ff46c602";

impl Default for ImagePolicy {
    fn default() -> Self {
        let mut digests = BTreeMap::new();
        digests.insert(EngineVersion::new(3, 1, 0), V3_1_0_DIGEST.to_owned());
        Self {
            repository: DEFAULT_REPOSITORY.to_owned(),
            digests,
            require_digest: true,
        }
    }
}

impl ImagePolicy {
    /// `repository:tag@digest` (or `repository:tag` when digests are not
    /// required and the version has none).
    pub fn image(&self, version: EngineVersion) -> Result<String, RenderError> {
        match self.digests.get(&version) {
            Some(d) => Ok(format!("{}:{}@{d}", self.repository, version.tag())),
            None if self.require_digest => Err(RenderError::NoDigest(version)),
            None => Ok(format!("{}:{}", self.repository, version.tag())),
        }
    }

    /// Whether clusters may run `version`.
    pub fn allows(&self, version: EngineVersion) -> bool {
        !self.require_digest || self.digests.contains_key(&version)
    }
}

/// How workloads find each other: compose service names, or Kubernetes
/// Service DNS names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum HostStyle {
    /// One compose service per replica, `rw-<key>-<role>-<n>`.
    Compose,
    /// StatefulSets with headless Services for meta and compute,
    /// Deployments for the rest, each behind a Service `rw-<key>-<role>`.
    Kubernetes {
        namespace: String,
        #[serde(default = "default_cluster_domain")]
        cluster_domain: String,
    },
}

fn default_cluster_domain() -> String {
    "cluster.local".to_owned()
}

impl HostStyle {
    /// The name every object of a role is called by.
    pub fn workload_name(key: &InstanceKey, role: Role) -> String {
        format!("rw-{key}-{role}")
    }

    /// One replica's address.
    pub fn replica_host(&self, key: &InstanceKey, role: Role, index: u32) -> String {
        let name = Self::workload_name(key, role);
        match self {
            HostStyle::Compose => format!("{name}-{index}"),
            HostStyle::Kubernetes {
                namespace,
                cluster_domain,
            } => format!("{name}-{index}.{name}-headless.{namespace}.svc.{cluster_domain}"),
        }
    }

    /// The role's load-balanced address.
    pub fn service_host(&self, key: &InstanceKey, role: Role) -> String {
        match self {
            HostStyle::Compose => self.replica_host(key, role, 0),
            HostStyle::Kubernetes {
                namespace,
                cluster_domain,
            } => format!(
                "{}.{namespace}.svc.{cluster_domain}",
                Self::workload_name(key, role)
            ),
        }
    }
}

/// One environment variable's value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "from", content = "value")]
pub enum EnvValue {
    Plain(String),
    /// A key of the cluster's credentials Secret.
    Secret(String),
    /// The replica's own address and this port; the runtime fills it in
    /// (the replica's host in compose, `$(POD_IP)` in Kubernetes).
    Advertise(u16),
}

/// A named port of a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub port: u16,
}

/// One role's workload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedRole {
    pub role: Role,
    /// `rw-<key>-<role>`.
    pub name: String,
    pub image: String,
    pub version: EngineVersion,
    pub replicas: u32,
    pub resources: Resources,
    /// The container's command after the image's entrypoint (`risingwave`).
    pub args: Vec<String>,
    pub env: BTreeMap<String, EnvValue>,
    pub ports: Vec<Port>,
    pub labels: BTreeMap<String, String>,
}

/// A instance's whole cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedCluster {
    pub key: InstanceKey,
    /// The Secret every role reads its credentials from.
    pub secret_name: String,
    pub secrets: BTreeMap<String, Secret>,
    /// The ConfigMap (or file) mounted at [`CONFIG_PATH`].
    pub config_name: String,
    pub config_toml: String,
    /// In [`Role::ROLLOUT`] order.
    pub roles: Vec<RenderedRole>,
}

impl RenderedCluster {
    pub fn role(&self, role: Role) -> &RenderedRole {
        self.roles
            .iter()
            .find(|r| r.role == role)
            .expect("every role is rendered")
    }
}

/// Everything the renderer reads.
#[derive(Debug, Clone)]
pub struct RenderInput<'a> {
    pub key: &'a InstanceKey,
    pub spec: &'a ClusterSpec,
    pub meta_db: &'a MetaDb,
    pub store: &'a ObjectStore,
    pub images: &'a ImagePolicy,
    pub hosts: &'a HostStyle,
    /// Roles that run another version than `spec.version` (a rolling
    /// upgrade in progress).
    pub role_versions: &'a BTreeMap<Role, EngineVersion>,
    /// Every role at zero replicas (D1954).
    pub suspended: bool,
}

/// Renders a cluster.
pub fn render(input: &RenderInput<'_>) -> Result<RenderedCluster, RenderError> {
    let topology = input.spec.topology()?;
    if !input.meta_db.password.is_url_safe() {
        return Err(RenderError::UnsafePassword);
    }
    check_input(input)?;

    let key = input.key;
    let secret_name = format!("rw-{key}-credentials");
    let mut secrets = BTreeMap::new();
    secrets.insert(
        secret_keys::META_DB_PASSWORD.to_owned(),
        input.meta_db.password.clone(),
    );
    secrets.insert(
        secret_keys::S3_ACCESS_KEY_ID.to_owned(),
        Secret::new(input.store.access_key_id.clone()),
    );
    secrets.insert(
        secret_keys::S3_SECRET_ACCESS_KEY.to_owned(),
        input.store.secret_access_key.clone(),
    );

    let meta_addr = meta_address(input, &topology);
    let mut roles = Vec::with_capacity(4);
    for (role, spec) in topology.iter() {
        let version = input
            .role_versions
            .get(&role)
            .copied()
            .unwrap_or(input.spec.version);
        let image = input.images.image(version)?;
        let mut env = common_env(input, role);
        match role {
            Role::Meta => meta_env(input, &mut env),
            Role::Compute => {
                plain(&mut env, "RW_META_ADDR", &meta_addr);
                plain(
                    &mut env,
                    "RW_TOTAL_MEMORY_BYTES",
                    &spec.resources.memory_bytes.to_string(),
                );
                plain(
                    &mut env,
                    "RW_PARALLELISM",
                    &spec.resources.vcpus().to_string(),
                );
                plain(&mut env, "RW_COMPUTE_NODE_ROLE", "both");
            }
            Role::Compactor => {
                plain(&mut env, "RW_META_ADDR", &meta_addr);
                plain(
                    &mut env,
                    "RW_COMPACTOR_TOTAL_MEMORY_BYTES",
                    &spec.resources.memory_bytes.to_string(),
                );
            }
            Role::Frontend => {
                plain(&mut env, "RW_META_ADDR", &meta_addr);
                plain(
                    &mut env,
                    "RW_FRONTEND_TOTAL_MEMORY_BYTES",
                    &spec.resources.memory_bytes.to_string(),
                );
                plain(&mut env, "RW_HEALTH_CHECK_LISTENER_ADDR", "0.0.0.0:6786");
                plain(
                    &mut env,
                    "RW_WEBHOOK_LISTEN_ADDR",
                    &format!("0.0.0.0:{}", ports::WEBHOOK),
                );
            }
        }
        let mut ports_list = vec![
            Port {
                name: if role == Role::Frontend {
                    "pgwire".into()
                } else {
                    "rpc".into()
                },
                port: role.port(),
            },
            Port {
                name: "metrics".into(),
                port: ports::METRICS,
            },
        ];
        if role == Role::Meta {
            ports_list.push(Port {
                name: "dashboard".into(),
                port: ports::DASHBOARD,
            });
        }
        let mut labels = BTreeMap::new();
        labels.insert("app.kubernetes.io/name".into(), "risingwave".into());
        labels.insert("app.kubernetes.io/component".into(), role.name().into());
        labels.insert("app.kubernetes.io/managed-by".into(), "loams".into());
        labels.insert("app.kubernetes.io/version".into(), version.tag());
        labels.insert("loams.dev/engine-instance".into(), key.to_string());
        labels.insert("loams.dev/engine".into(), "risingwave".into());

        roles.push(RenderedRole {
            role,
            name: HostStyle::workload_name(key, role),
            image,
            version,
            replicas: if input.suspended { 0 } else { spec.replicas },
            resources: spec.resources,
            args: vec![role.subcommand().to_owned()],
            env,
            ports: ports_list,
            labels,
        });
    }

    Ok(RenderedCluster {
        key: key.clone(),
        secret_name,
        secrets,
        config_name: format!("rw-{key}-config"),
        config_toml: config_toml(input.spec.embedded_udfs),
        roles,
    })
}

fn check_input(input: &RenderInput<'_>) -> Result<(), RenderError> {
    let invalid = |m: &str| Err(RenderError::Invalid(m.to_owned()));
    let db = input.meta_db;
    if db.host.is_empty() || db.port == 0 || db.database.is_empty() || db.username.is_empty() {
        return invalid("the meta store needs a host, port, database and username");
    }
    // The user and database are spliced into the URL too.
    let url_safe = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if !url_safe(&db.username) || !url_safe(&db.database) {
        return invalid("the meta store's username and database must be [A-Za-z0-9_-]");
    }
    let s = input.store;
    if s.bucket.is_empty() || s.region.is_empty() || s.access_key_id.is_empty() {
        return invalid("the object store needs a bucket, region and access key");
    }
    if ObjectStore::prefix_from_state(&s.prefix).as_deref() != Some(s.prefix.as_str()) {
        return invalid(
            "the object store prefix must be the instance's state prefix, without leading or trailing '/'",
        );
    }
    Ok(())
}

fn plain(env: &mut BTreeMap<String, EnvValue>, key: &str, value: &str) {
    env.insert(key.to_owned(), EnvValue::Plain(value.to_owned()));
}

fn secret(env: &mut BTreeMap<String, EnvValue>, key: &str, secret_key: &str) {
    env.insert(key.to_owned(), EnvValue::Secret(secret_key.to_owned()));
}

/// The meta address every other role dials: all meta replicas, comma
/// separated (RisingWave's `MetaAddressStrategy::List`).
fn meta_address(input: &RenderInput<'_>, topology: &Topology) -> String {
    (0..topology.role(Role::Meta).replicas)
        .map(|i| {
            format!(
                "http://{}:{}",
                input.hosts.replica_host(input.key, Role::Meta, i),
                ports::META
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn common_env(input: &RenderInput<'_>, role: Role) -> BTreeMap<String, EnvValue> {
    let mut env = BTreeMap::new();
    plain(&mut env, "RW_CONFIG_PATH", CONFIG_PATH);
    plain(&mut env, "ENABLE_TELEMETRY", "false");
    plain(
        &mut env,
        "RW_LISTEN_ADDR",
        &format!("0.0.0.0:{}", role.port()),
    );
    env.insert("RW_ADVERTISE_ADDR".into(), EnvValue::Advertise(role.port()));
    let metrics_key = if role == Role::Meta {
        "RW_PROMETHEUS_HOST"
    } else {
        "RW_PROMETHEUS_LISTENER_ADDR"
    };
    plain(
        &mut env,
        metrics_key,
        &format!("0.0.0.0:{}", ports::METRICS),
    );
    // Every role may touch Hummock; the credentials reach only the instance's
    // prefix (D1952).
    secret(&mut env, "AWS_ACCESS_KEY_ID", secret_keys::S3_ACCESS_KEY_ID);
    secret(
        &mut env,
        "AWS_SECRET_ACCESS_KEY",
        secret_keys::S3_SECRET_ACCESS_KEY,
    );
    plain(&mut env, "AWS_REGION", &input.store.region);
    if let Some(endpoint) = &input.store.endpoint {
        plain(&mut env, "RW_S3_ENDPOINT", endpoint);
    }
    if input.store.force_path_style {
        plain(&mut env, "RW_IS_FORCE_PATH_STYLE", "true");
    }
    env
}

fn meta_env(input: &RenderInput<'_>, env: &mut BTreeMap<String, EnvValue>) {
    let db = input.meta_db;
    let store = input.store;
    plain(
        env,
        "RW_DASHBOARD_HOST",
        &format!("0.0.0.0:{}", ports::DASHBOARD),
    );
    plain(env, "RW_BACKEND", "postgres");
    // The free tier, locked (D1901): see LICENSE_KEY_PATH.
    plain(env, "RW_LICENSE_KEY_PATH", LICENSE_KEY_PATH);
    plain(env, "RW_SQL_ENDPOINT", &db.endpoint());
    plain(env, "RW_SQL_USERNAME", &db.username);
    secret(env, "RW_SQL_PASSWORD", secret_keys::META_DB_PASSWORD);
    plain(env, "RW_SQL_DATABASE", &db.database);
    if let Some(params) = &db.url_params {
        plain(env, "RW_SQL_URL_PARAMS", params);
    }
    plain(env, "RW_STATE_STORE", &store.state_store_url());
    plain(env, "RW_DATA_DIRECTORY", &store.data_directory());
    plain(env, "RW_BACKUP_STORAGE_URL", &store.backup_storage_url());
    plain(
        env,
        "RW_BACKUP_STORAGE_DIRECTORY",
        &store.backup_directory(),
    );
    // The bucket is shared by every instance: RisingWave must not set
    // bucket-wide lifecycle rules.
    plain(env, "RW_DO_NOT_CONFIG_BUCKET_LIFECYCLE", "true");
}

/// `risingwave.toml`: telemetry off, password auth for every network
/// client (no `trust`), no local-filesystem connector, embedded UDFs only
/// when asked for. Every other setting keeps RisingWave's default.
pub fn config_toml(embedded_udfs: bool) -> String {
    format!(
        r#"# Rendered by loams-risingwave. Do not edit: the controller rewrites it.

[server]
telemetry_enabled = false

[frontend]
unsafe_enable_local_fs_connector = false

[[frontend.hba_config.entries]]
connection_type = "Host"
databases = ["all"]
users = ["all"]
addresses = ["0.0.0.0/0", "::/0"]
auth_method = "password"

[udf]
enable_embedded_python_udf = false
enable_embedded_javascript_udf = {embedded_udfs}
enable_embedded_wasm_udf = {embedded_udfs}
"#
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{RoleOverride, RoleOverrides, Size};

    pub(crate) fn key() -> InstanceKey {
        InstanceKey::new("acme").unwrap()
    }

    pub(crate) fn meta_db() -> MetaDb {
        MetaDb {
            host: "compute1".into(),
            port: 55433,
            database: "rw_acme".into(),
            username: "rw_acme".into(),
            password: Secret::new("pw0123456789"),
            url_params: None,
        }
    }

    pub(crate) fn store() -> ObjectStore {
        ObjectStore {
            bucket: "loams-engines".into(),
            endpoint: Some("http://rustfs:9000".into()),
            region: "us-east-1".into(),
            force_path_style: true,
            prefix: "engines/dev/eng_acme".into(),
            access_key_id: "AKacme".into(),
            secret_access_key: Secret::new("sk"),
        }
    }

    fn render_with(
        spec: &ClusterSpec,
        hosts: &HostStyle,
        suspended: bool,
    ) -> Result<RenderedCluster, RenderError> {
        let t = key();
        let db = meta_db();
        let s = store();
        let images = ImagePolicy::default();
        let versions = BTreeMap::new();
        render(&RenderInput {
            key: &t,
            spec,
            meta_db: &db,
            store: &s,
            images: &images,
            hosts,
            role_versions: &versions,
            suspended,
        })
    }

    fn env<'a>(c: &'a RenderedCluster, role: Role, key: &str) -> Option<&'a EnvValue> {
        c.role(role).env.get(key)
    }

    fn p(s: &str) -> Option<EnvValue> {
        Some(EnvValue::Plain(s.to_owned()))
    }

    #[test]
    fn meta_runs_on_postgres_and_hummock_s3() {
        let c = render_with(&ClusterSpec::new(Size::Xs), &HostStyle::Compose, false).unwrap();
        assert_eq!(env(&c, Role::Meta, "RW_BACKEND").cloned(), p("postgres"));
        assert_eq!(
            env(&c, Role::Meta, "RW_SQL_ENDPOINT").cloned(),
            p("compute1:55433")
        );
        assert_eq!(
            env(&c, Role::Meta, "RW_SQL_DATABASE").cloned(),
            p("rw_acme")
        );
        assert_eq!(
            env(&c, Role::Meta, "RW_SQL_PASSWORD").cloned(),
            Some(EnvValue::Secret(secret_keys::META_DB_PASSWORD.into()))
        );
        assert_eq!(
            env(&c, Role::Meta, "RW_STATE_STORE").cloned(),
            p("hummock+s3://loams-engines")
        );
        assert_eq!(
            env(&c, Role::Meta, "RW_DATA_DIRECTORY").cloned(),
            p("engines/dev/eng_acme/hummock")
        );
        assert_eq!(
            env(&c, Role::Meta, "RW_BACKUP_STORAGE_DIRECTORY").cloned(),
            p("engines/dev/eng_acme/backup")
        );
        assert_eq!(
            env(&c, Role::Meta, "RW_DO_NOT_CONFIG_BUCKET_LIFECYCLE").cloned(),
            p("true")
        );
        assert_eq!(
            env(&c, Role::Compute, "RW_S3_ENDPOINT").cloned(),
            p("http://rustfs:9000")
        );
        assert_eq!(
            env(&c, Role::Compactor, "RW_IS_FORCE_PATH_STYLE").cloned(),
            p("true")
        );
        assert_eq!(
            c.secrets[secret_keys::META_DB_PASSWORD].expose(),
            "pw0123456789"
        );
    }

    #[test]
    fn no_credential_is_plain() {
        let c = render_with(&ClusterSpec::new(Size::S), &HostStyle::Compose, false).unwrap();
        for role in &c.roles {
            for (k, v) in &role.env {
                if k.contains("PASSWORD") || k.contains("SECRET") || k == "AWS_ACCESS_KEY_ID" {
                    assert!(
                        matches!(v, EnvValue::Secret(_)),
                        "{k} is not from the Secret"
                    );
                }
                if let EnvValue::Plain(s) = v {
                    assert!(!s.contains("pw0123456789"), "{k} leaks the password");
                }
            }
            assert_eq!(role.args, vec![role.role.subcommand().to_owned()]);
        }
    }

    #[test]
    fn roles_dial_every_meta_replica() {
        let mut spec = ClusterSpec::new(Size::Xl);
        spec.overrides.insert(Role::Meta, RoleOverride::default());
        let hosts = HostStyle::Kubernetes {
            namespace: "eng-acme".into(),
            cluster_domain: "cluster.local".into(),
        };
        let c = render_with(&spec, &hosts, false).unwrap();
        let Some(EnvValue::Plain(addr)) = env(&c, Role::Compute, "RW_META_ADDR") else {
            panic!("no meta address");
        };
        assert_eq!(addr.split(',').count(), 3);
        assert!(addr.starts_with(
            "http://rw-acme-meta-0.rw-acme-meta-headless.eng-acme.svc.cluster.local:5690,"
        ));
        assert_eq!(
            hosts.service_host(&key(), Role::Frontend),
            "rw-acme-frontend.eng-acme.svc.cluster.local"
        );
    }

    #[test]
    fn sizes_set_memory_and_parallelism() {
        let c = render_with(&ClusterSpec::new(Size::S), &HostStyle::Compose, false).unwrap();
        let compute = Size::S.role(Role::Compute);
        assert_eq!(c.role(Role::Compute).replicas, compute.replicas);
        assert_eq!(
            env(&c, Role::Compute, "RW_TOTAL_MEMORY_BYTES").cloned(),
            p(&compute.resources.memory_bytes.to_string())
        );
        assert_eq!(env(&c, Role::Compute, "RW_PARALLELISM").cloned(), p("1"));
        assert_eq!(
            env(&c, Role::Meta, "RW_PROMETHEUS_HOST").cloned(),
            p("0.0.0.0:1250")
        );
        assert_eq!(
            env(&c, Role::Frontend, "RW_PROMETHEUS_LISTENER_ADDR").cloned(),
            p("0.0.0.0:1250")
        );
    }

    #[test]
    fn suspended_is_zero_replicas() {
        let c = render_with(&ClusterSpec::new(Size::M), &HostStyle::Compose, true).unwrap();
        assert!(c.roles.iter().all(|r| r.replicas == 0));
        // The meta address still names the meta replicas the cluster resumes with.
        assert!(matches!(
            env(&c, Role::Frontend, "RW_META_ADDR"),
            Some(EnvValue::Plain(a)) if a == "http://rw-acme-meta-0:5690"
        ));
    }

    #[test]
    fn roles_are_in_rollout_order() {
        let c = render_with(&ClusterSpec::new(Size::Xs), &HostStyle::Compose, false).unwrap();
        let order: Vec<Role> = c.roles.iter().map(|r| r.role).collect();
        assert_eq!(order, Role::ROLLOUT.to_vec());
        assert_eq!(c.role(Role::Meta).ports.len(), 3);
    }

    #[test]
    fn images_are_pinned() {
        let c = render_with(&ClusterSpec::new(Size::Xs), &HostStyle::Compose, false).unwrap();
        assert_eq!(
            c.role(Role::Frontend).image,
            format!("{DEFAULT_REPOSITORY}:v3.1.0@{V3_1_0_DIGEST}")
        );
        let mut spec = ClusterSpec::new(Size::Xs);
        spec.version = EngineVersion::new(3, 0, 5);
        assert_eq!(
            render_with(&spec, &HostStyle::Compose, false),
            Err(RenderError::NoDigest(EngineVersion::new(3, 0, 5)))
        );
    }

    #[test]
    fn rolling_upgrade_renders_mixed_versions() {
        let t = key();
        let db = meta_db();
        let s = store();
        let mut images = ImagePolicy::default();
        images
            .digests
            .insert(EngineVersion::new(3, 1, 1), "sha256:00".into());
        let spec = ClusterSpec::new(Size::Xs);
        let mut versions = BTreeMap::new();
        versions.insert(Role::Meta, EngineVersion::new(3, 1, 1));
        let c = render(&RenderInput {
            key: &t,
            spec: &spec,
            meta_db: &db,
            store: &s,
            images: &images,
            hosts: &HostStyle::Compose,
            role_versions: &versions,
            suspended: false,
        })
        .unwrap();
        assert_eq!(c.role(Role::Meta).version, EngineVersion::new(3, 1, 1));
        assert_eq!(c.role(Role::Compute).version, EngineVersion::new(3, 1, 0));
    }

    #[test]
    fn unsafe_inputs_are_refused() {
        let t = key();
        let mut db = meta_db();
        db.password = Secret::new("p@ss");
        let s = store();
        let images = ImagePolicy::default();
        let versions = BTreeMap::new();
        let spec = ClusterSpec::new(Size::Xs);
        let input = RenderInput {
            key: &t,
            spec: &spec,
            meta_db: &db,
            store: &s,
            images: &images,
            hosts: &HostStyle::Compose,
            role_versions: &versions,
            suspended: false,
        };
        assert_eq!(render(&input), Err(RenderError::UnsafePassword));

        let db = meta_db();
        let mut s2 = store();
        s2.prefix = "engines/../other".into();
        let input = RenderInput {
            meta_db: &db,
            store: &s2,
            ..input
        };
        assert!(matches!(render(&input), Err(RenderError::Invalid(_))));

        let mut bad = ClusterSpec::new(Size::Xs);
        let mut o = RoleOverrides::new();
        o.insert(
            Role::Frontend,
            RoleOverride {
                replicas: Some(0),
                ..Default::default()
            },
        );
        bad.overrides = o;
        assert!(matches!(
            render_with(&bad, &HostStyle::Compose, false),
            Err(RenderError::Model(_))
        ));
    }

    /// D1901: no rendered cluster may run on RisingWave's built-in licence
    /// key. Meta nodes read an empty key file, and nothing else names a key.
    #[test]
    fn licence_key_is_always_the_empty_file() {
        let k8s = HostStyle::Kubernetes {
            namespace: "eng-acme".into(),
            cluster_domain: "cluster.local".into(),
        };
        assert!(LICENSE_KEY_FILE.is_empty());
        assert_eq!(
            std::path::Path::new(LICENSE_KEY_PATH).parent(),
            std::path::Path::new(CONFIG_PATH).parent(),
            "the key file must sit in the mounted config directory"
        );
        for size in [Size::Xs, Size::S, Size::M, Size::Xl] {
            for hosts in [&HostStyle::Compose, &k8s] {
                for suspended in [false, true] {
                    let mut spec = ClusterSpec::new(size);
                    spec.embedded_udfs = suspended;
                    let c = render_with(&spec, hosts, suspended).unwrap();
                    assert_eq!(
                        env(&c, Role::Meta, "RW_LICENSE_KEY_PATH").cloned(),
                        p(LICENSE_KEY_PATH)
                    );
                    for role in &c.roles {
                        assert!(
                            !role.env.contains_key("RW_LICENSE_KEY"),
                            "{} sets a licence key",
                            role.name
                        );
                        if role.role != Role::Meta {
                            assert!(!role.env.contains_key("RW_LICENSE_KEY_PATH"));
                        }
                        assert!(
                            !role.args.iter().any(|a| a.contains("license")),
                            "{} passes a licence flag",
                            role.name
                        );
                    }
                    let v: toml::Value = toml::from_str(&c.config_toml).unwrap();
                    assert!(
                        v.get("system").is_none(),
                        "system params in risingwave.toml"
                    );
                    assert!(!c.config_toml.contains("license"));
                    assert!(c.secrets.keys().all(|k| !k.contains("license")));
                }
            }
        }
    }

    #[test]
    fn config_is_valid_toml_without_trust() {
        for udfs in [false, true] {
            let text = config_toml(udfs);
            let v: toml::Value = toml::from_str(&text).unwrap();
            assert_eq!(v["server"]["telemetry_enabled"].as_bool(), Some(false));
            let entries = v["frontend"]["hba_config"]["entries"].as_array().unwrap();
            assert!(
                entries
                    .iter()
                    .all(|e| e["auth_method"].as_str() == Some("password"))
            );
            assert_eq!(v["udf"]["enable_embedded_wasm_udf"].as_bool(), Some(udfs));
            assert_eq!(
                v["udf"]["enable_embedded_python_udf"].as_bool(),
                Some(false)
            );
        }
    }
}
