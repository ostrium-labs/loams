//! Cluster configuration rendering (SP-a D1996; design 81 R6, R9, R16).
//!
//! One [`ClusterConf`] describes an instance's data plane: the Iceberg
//! catalog `loams` at Lakekeeper, Loams object storage, the instance's state
//! prefix and its compute shape. It renders to
//!
//! - [`ClusterConf::spark_defaults`]: Apache Spark's `spark-defaults.conf`
//!   for the Spark Connect server (catalog, S3A, event logs, warehouse,
//!   master, executors), and
//! - [`ClusterConf::sail_env`]: Sail's environment (`SAIL_*` and the object
//!   store's `AWS_*`).
//!
//! **No secret is ever rendered.** S3 keys reach the processes as
//! `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` from a Secret (R5), the Spark
//! Connect pre-shared token as `SPARK_CONNECT_AUTHENTICATE_TOKEN`, and a
//! Lakekeeper token as a mounted file. SP-b's `catalog.rs` renders the
//! client/job side with the same names, so a job's conf equals what the
//! server already has.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// The Spark catalog name Loams uses everywhere (R9).
pub const CATALOG: &str = "loams";

/// The engines bucket (R6).
pub const ENGINES_BUCKET: &str = "loams-engines";

/// Apache Spark Connect's default gRPC port.
pub const SPARK_CONNECT_PORT: u16 = 15002;

/// Sail's Spark Connect port in its own manifests.
pub const SAIL_PORT: u16 = 50051;

/// The pins this crate renders (SP-a D1990; all at least 14 days old on
/// 2026-10-27).
pub mod pins {
    /// Apache Spark, the opt-in backend.
    pub const SPARK_VERSION: &str = "4.1.3";
    /// The official image, by digest.
    pub const SPARK_IMAGE: &str = "docker.io/apache/spark:4.1.3-scala2.13-java21-python3-ubuntu@sha256:bf9d035a7c32a8ca46aa58d6348182ffd7d2dff6409206ecfbb3915ff1fef211";
    /// Iceberg, whose 1.12.0 ships a runtime for Spark 4.1 (none for 4.2).
    pub const ICEBERG_VERSION: &str = "1.12.0";
    /// Hadoop of Spark 4.1.3, for `hadoop-aws`.
    pub const HADOOP_VERSION: &str = "3.4.2";
    /// The Maven coordinates the Loams Spark image bakes in.
    pub const SPARK_PACKAGES: &[&str] = &[
        "org.apache.iceberg:iceberg-spark-runtime-4.1_2.13:1.12.0",
        "org.apache.iceberg:iceberg-aws-bundle:1.12.0",
        "org.apache.hadoop:hadoop-aws:3.4.2",
    ];
    /// Sail, the default backend.
    pub const SAIL_VERSION: &str = "0.7.2";
}

/// Loams object storage as the engines see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectStore {
    /// The S3 endpoint, e.g. `http://rustfs:9000`; `None` for AWS S3.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// The region.
    #[serde(default = "default_region")]
    pub region: String,
    /// Path-style requests (RustFS, MinIO).
    #[serde(default = "yes")]
    pub path_style: bool,
    /// The engines bucket.
    #[serde(default = "default_bucket")]
    pub bucket: String,
}

fn default_region() -> String {
    "us-east-1".into()
}
fn default_bucket() -> String {
    ENGINES_BUCKET.into()
}
fn yes() -> bool {
    true
}

impl Default for ObjectStore {
    fn default() -> Self {
        Self {
            endpoint: Some("http://rustfs:9000".into()),
            region: default_region(),
            path_style: true,
            bucket: default_bucket(),
        }
    }
}

/// Lakekeeper, the one table catalog (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// The Iceberg REST URI, e.g. `http://lakekeeper:8181/catalog`.
    pub uri: String,
    /// The warehouse name.
    #[serde(default = "default_warehouse")]
    pub warehouse: String,
    /// `X-Iceberg-Access-Delegation`; `None` sends none (static S3 keys).
    #[serde(default = "default_delegation")]
    pub access_delegation: Option<String>,
    /// OAuth2 client credentials (k8s, Zitadel). The secret is read from the
    /// environment variable named, never rendered.
    #[serde(default)]
    pub oauth2: Option<OAuth2>,
    /// A file holding a bearer token for the catalog (Sail reads it on each
    /// request; a sidecar refreshes it).
    #[serde(default)]
    pub token_file: Option<String>,
}

fn default_warehouse() -> String {
    "loams".into()
}
fn default_delegation() -> Option<String> {
    Some("vended-credentials".into())
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            uri: "http://lakekeeper:8181/catalog".into(),
            warehouse: default_warehouse(),
            access_delegation: default_delegation(),
            oauth2: None,
            token_file: None,
        }
    }
}

/// OAuth2 client credentials for the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth2 {
    /// The token endpoint.
    pub server_uri: String,
    /// The scope, e.g. `lakekeeper`.
    #[serde(default)]
    pub scope: Option<String>,
    /// The environment variable holding `client_id:client_secret`.
    pub credential_env: String,
}

/// Where the processes run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// One process (compose): Spark `local[*]`, Sail `local`.
    Local,
    /// Kubernetes: Spark driver + executors, Sail `kubernetes-cluster`.
    Kubernetes(KubePlacement),
}

/// The Kubernetes side of a cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubePlacement {
    /// The Kubernetes namespace of the instance.
    pub namespace: String,
    /// The worker/executor image (the server's own image).
    pub image: String,
    /// The service account that may create worker pods.
    pub service_account: String,
    /// The headless service name of the driver (Spark executors call back).
    pub driver_service: String,
}

/// The compute shape of one instance (from ENG-UI's size tier, R19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// Executors / Sail workers at most.
    pub workers: u16,
    /// vCPU per worker, in milli.
    pub worker_vcpu_milli: u32,
    /// Memory per worker, MiB.
    pub worker_memory_mib: u32,
    /// The driver/server's memory, MiB.
    pub driver_memory_mib: u32,
}

/// Everything the renderers need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConf {
    /// The Loams namespace (R2).
    pub namespace: String,
    /// The instance id, `eng_<26>`.
    pub instance_id: String,
    /// `engines/<ns>/<instance-id>/` (R2, R6).
    pub state_prefix: String,
    /// Object storage.
    pub store: ObjectStore,
    /// The catalog.
    pub catalog: Catalog,
    /// Where the processes run.
    pub placement: Placement,
    /// The compute shape.
    pub shape: Shape,
    /// The proxy's idle timeout in seconds; the backend keeps sessions a
    /// little longer so the proxy releases them first.
    pub proxy_idle_timeout_secs: u64,
    /// An OTLP endpoint for Sail's telemetry (and Spark's, later).
    pub otlp_endpoint: Option<String>,
}

/// A rendering refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfError {
    /// A value that would break the file format or escape its field.
    #[error("{0}")]
    Invalid(String),
}

fn check_value(key: &str, value: &str) -> Result<(), ConfError> {
    if value.contains(['\n', '\r', '\0']) {
        return Err(ConfError::Invalid(format!("{key}: value has a line break")));
    }
    Ok(())
}

/// A TOML basic string (Sail parses its list settings as TOML).
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl ClusterConf {
    /// `s3a://<bucket>/<state_prefix><sub>`.
    pub fn s3a(&self, sub: &str) -> String {
        format!("s3a://{}/{}{sub}", self.store.bucket, self.state_prefix)
    }

    /// `s3://<bucket>/<state_prefix><sub>` (Sail's spelling).
    pub fn s3(&self, sub: &str) -> String {
        format!("s3://{}/{}{sub}", self.store.bucket, self.state_prefix)
    }

    fn check(&self) -> Result<(), ConfError> {
        if !self.state_prefix.ends_with('/') || self.state_prefix.starts_with('/') {
            return Err(ConfError::Invalid(
                "state_prefix must be relative and end with '/'".into(),
            ));
        }
        if !self
            .instance_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ConfError::Invalid(format!(
                "instance id {:?} has characters a pod name cannot",
                self.instance_id
            )));
        }
        Ok(())
    }

    /// Apache Spark's `spark-defaults.conf` entries for the Spark Connect
    /// server.
    ///
    /// # Errors
    ///
    /// [`ConfError`] for a malformed prefix or a value with a line break.
    pub fn spark_defaults(&self) -> Result<SparkDefaults, ConfError> {
        self.check()?;
        let mut c = BTreeMap::<String, String>::new();
        let mut set = |k: &str, v: String| {
            c.insert(k.to_owned(), v);
        };

        // The Iceberg catalog `loams` at Lakekeeper (R9).
        let cat = format!("spark.sql.catalog.{CATALOG}");
        set(&cat, "org.apache.iceberg.spark.SparkCatalog".into());
        set(&format!("{cat}.type"), "rest".into());
        set(&format!("{cat}.uri"), self.catalog.uri.clone());
        set(&format!("{cat}.warehouse"), self.catalog.warehouse.clone());
        set(
            &format!("{cat}.io-impl"),
            "org.apache.iceberg.aws.s3.S3FileIO".into(),
        );
        if let Some(d) = &self.catalog.access_delegation {
            set(
                &format!("{cat}.header.X-Iceberg-Access-Delegation"),
                d.clone(),
            );
        }
        if let Some(o) = &self.catalog.oauth2 {
            set(&format!("{cat}.oauth2-server-uri"), o.server_uri.clone());
            // Iceberg reads `credential` from the conf; the server's start
            // script fills it from the environment (never this file).
            set(
                &format!("{cat}.credential"),
                format!("${{env:{}}}", o.credential_env),
            );
            if let Some(s) = &o.scope {
                set(&format!("{cat}.scope"), s.clone());
            }
        }
        if let Some(ep) = &self.store.endpoint {
            set(&format!("{cat}.s3.endpoint"), ep.clone());
        }
        set(
            &format!("{cat}.s3.path-style-access"),
            self.store.path_style.to_string(),
        );
        set(&format!("{cat}.client.region"), self.store.region.clone());
        set(
            "spark.sql.extensions",
            "org.apache.iceberg.spark.extensions.IcebergSparkSessionExtensions".into(),
        );
        set("spark.sql.defaultCatalog", CATALOG.into());

        // S3A on Loams object storage; credentials from the environment only.
        if let Some(ep) = &self.store.endpoint {
            set("spark.hadoop.fs.s3a.endpoint", ep.clone());
            set(
                "spark.hadoop.fs.s3a.connection.ssl.enabled",
                ep.starts_with("https://").to_string(),
            );
        }
        set(
            "spark.hadoop.fs.s3a.endpoint.region",
            self.store.region.clone(),
        );
        set(
            "spark.hadoop.fs.s3a.path.style.access",
            self.store.path_style.to_string(),
        );
        set(
            "spark.hadoop.fs.s3a.aws.credentials.provider",
            "software.amazon.awssdk.auth.credentials.EnvironmentVariableCredentialsProvider".into(),
        );
        set(
            "spark.hadoop.fs.s3a.impl",
            "org.apache.hadoop.fs.s3a.S3AFileSystem".into(),
        );
        // RustFS does not implement the bucket probe the S3A default does.
        set("spark.hadoop.fs.s3a.bucket.probe", "0".into());

        // Event logs and scratch under the instance's prefix (R6, R16).
        set("spark.eventLog.enabled", "true".into());
        set("spark.eventLog.dir", self.s3a("eventlog/"));
        set("spark.eventLog.rolling.enabled", "true".into());
        set("spark.eventLog.rolling.maxFileSize", "128m".into());
        set("spark.eventLog.compress", "true".into());
        set("spark.sql.warehouse.dir", self.s3a("scratch/warehouse/"));
        set("spark.sql.catalogImplementation", "in-memory".into());

        // The Spark Connect server.
        set(
            "spark.connect.grpc.binding.port",
            SPARK_CONNECT_PORT.to_string(),
        );
        set(
            "spark.connect.grpc.maxInboundMessageSize",
            crate::proxy::DEFAULT_MAX_MESSAGE_BYTES.to_string(),
        );
        set(
            "spark.connect.session.manager.defaultSessionTimeout",
            format!("{}s", self.proxy_idle_timeout_secs + 300),
        );
        set("spark.ui.killEnabled", "false".into());
        set(
            "spark.driver.memory",
            format!("{}m", self.shape.driver_memory_mib),
        );

        match &self.placement {
            Placement::Local => {
                set("spark.master", "local[*]".into());
            }
            Placement::Kubernetes(k) => {
                set(
                    "spark.master",
                    "k8s://https://kubernetes.default.svc".into(),
                );
                set("spark.submit.deployMode", "client".into());
                set("spark.kubernetes.namespace", k.namespace.clone());
                set("spark.kubernetes.container.image", k.image.clone());
                set(
                    "spark.kubernetes.authenticate.driver.serviceAccountName",
                    k.service_account.clone(),
                );
                set(
                    "spark.kubernetes.executor.podNamePrefix",
                    self.instance_id.replace('_', "-"),
                );
                set(
                    "spark.kubernetes.executor.label.loams.dev/instance",
                    self.instance_id.replace('_', "-"),
                );
                set(
                    "spark.kubernetes.executor.label.loams.dev/role",
                    "spark-executor".into(),
                );
                set(
                    "spark.driver.host",
                    format!("{}.{}.svc", k.driver_service, k.namespace),
                );
                set("spark.driver.bindAddress", "0.0.0.0".into());
                set("spark.driver.port", "7078".into());
                set("spark.driver.blockManager.port", "7079".into());
                let cores = self.shape.worker_vcpu_milli.div_ceil(1000).max(1);
                set("spark.executor.cores", cores.to_string());
                set(
                    "spark.kubernetes.executor.request.cores",
                    format!("{}m", self.shape.worker_vcpu_milli),
                );
                set(
                    "spark.kubernetes.executor.limit.cores",
                    format!("{}m", self.shape.worker_vcpu_milli),
                );
                // Spark adds ~10% overhead on top of executor memory.
                let heap = (u64::from(self.shape.worker_memory_mib) * 10 / 11).max(512);
                set("spark.executor.memory", format!("{heap}m"));
                set("spark.dynamicAllocation.enabled", "true".into());
                set(
                    "spark.dynamicAllocation.shuffleTracking.enabled",
                    "true".into(),
                );
                set("spark.dynamicAllocation.minExecutors", "0".into());
                set(
                    "spark.dynamicAllocation.maxExecutors",
                    self.shape.workers.to_string(),
                );
                set("spark.dynamicAllocation.executorIdleTimeout", "60s".into());
                set("spark.kubernetes.local.dirs.tmpfs", "false".into());
            }
        }
        for (k, v) in &c {
            check_value(k, v)?;
        }
        Ok(SparkDefaults(c))
    }

    /// Sail's environment (no secrets: the S3 keys come from a Secret as
    /// `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`).
    ///
    /// # Errors
    ///
    /// [`ConfError`] for a malformed prefix or a value with a line break.
    pub fn sail_env(&self) -> Result<BTreeMap<String, String>, ConfError> {
        self.check()?;
        let mut e = BTreeMap::<String, String>::new();
        let mut set = |k: &str, v: String| {
            e.insert(k.to_owned(), v);
        };
        // The catalog list is TOML (Sail's config format).
        let mut catalog = format!(
            "[{{type={}, name={}, uri={}, warehouse={}",
            toml_str("iceberg-rest"),
            toml_str(CATALOG),
            toml_str(&self.catalog.uri),
            toml_str(&self.catalog.warehouse),
        );
        if let Some(f) = &self.catalog.token_file {
            let _ = write!(catalog, ", bearer_access_token_file={}", toml_str(f));
        }
        catalog.push_str("}]");
        set("SAIL_CATALOG__LIST", catalog);
        set("SAIL_CATALOG__DEFAULT_CATALOG", CATALOG.into());
        set(
            "SAIL_SPARK__SESSION_TIMEOUT_SECS",
            (self.proxy_idle_timeout_secs + 300).to_string(),
        );
        set(
            "SAIL_EXECUTION__CHECKPOINT__PATH",
            self.s3("scratch/checkpoints/"),
        );
        // Object storage (object_store's AWS settings).
        if let Some(ep) = &self.store.endpoint {
            set("AWS_ENDPOINT", ep.clone());
            set("AWS_ALLOW_HTTP", ep.starts_with("http://").to_string());
        }
        set("AWS_REGION", self.store.region.clone());
        set(
            "AWS_VIRTUAL_HOSTED_STYLE_REQUEST",
            (!self.store.path_style).to_string(),
        );
        // Bounded memory: 80% of the server's (local) or a worker's memory.
        let pool_mib = match &self.placement {
            Placement::Local => self.shape.driver_memory_mib,
            Placement::Kubernetes(_) => self.shape.worker_memory_mib,
        };
        set("SAIL_RUNTIME__MEMORY_POOL__TYPE", "fair".into());
        set(
            "SAIL_RUNTIME__MEMORY_POOL__FAIR__MAX_SIZE",
            (u64::from(pool_mib) * 1024 * 1024 * 4 / 5).to_string(),
        );
        if let Some(otlp) = &self.otlp_endpoint {
            set("SAIL_TELEMETRY__EXPORTER__OTLP__ENDPOINT", otlp.clone());
            set("SAIL_TELEMETRY__EXPORT_TRACES", "true".into());
            set("SAIL_TELEMETRY__EXPORT_METRICS", "true".into());
            set("SAIL_TELEMETRY__EXPORT_LOGS", "true".into());
        }
        match &self.placement {
            Placement::Local => set("SAIL_MODE", "local".into()),
            Placement::Kubernetes(k) => {
                set("SAIL_MODE", "kubernetes-cluster".into());
                set("SAIL_CLUSTER__DRIVER_LISTEN_HOST", "0.0.0.0".into());
                set("SAIL_KUBERNETES__IMAGE", k.image.clone());
                set("SAIL_KUBERNETES__NAMESPACE", k.namespace.clone());
                set(
                    "SAIL_KUBERNETES__WORKER_SERVICE_ACCOUNT_NAME",
                    k.service_account.clone(),
                );
                set(
                    "SAIL_KUBERNETES__WORKER_POD_NAME_PREFIX",
                    format!("{}-worker-", self.instance_id.replace('_', "-")),
                );
                set(
                    "SAIL_CLUSTER__WORKER_MAX_COUNT",
                    self.shape.workers.to_string(),
                );
                set(
                    "SAIL_CLUSTER__WORKER_TASK_SLOTS",
                    (self.shape.worker_vcpu_milli.div_ceil(1000).max(1)).to_string(),
                );
                // Shuffle through the instance's prefix, so a lost worker
                // does not lose a stage (Sail 0.7 blocking shuffle).
                set("SAIL_CLUSTER__SHUFFLE_BACKEND__TYPE", "storage".into());
                set(
                    "SAIL_CLUSTER__SHUFFLE_BACKEND__STORAGE__PATH",
                    self.s3("scratch/shuffle/"),
                );
                set(
                    "SAIL_KUBERNETES__WORKER_POD_TEMPLATE",
                    self.sail_worker_template().to_string(),
                );
            }
        }
        for (k, v) in &e {
            check_value(k, v)?;
        }
        Ok(e)
    }

    /// The `PodTemplateSpec` Sail merges into its worker pods: resources,
    /// labels and the S3 credentials Secret.
    pub fn sail_worker_template(&self) -> serde_json::Value {
        let label = self.instance_id.replace('_', "-");
        serde_json::json!({
            "metadata": {
                "labels": {
                    "app.kubernetes.io/name": "sail",
                    "app.kubernetes.io/component": "worker",
                    "loams.dev/instance": label,
                    "loams.dev/role": "sail-worker",
                }
            },
            "spec": {
                "automountServiceAccountToken": false,
                "containers": [{
                    "name": "worker",
                    "resources": {
                        "requests": {
                            "cpu": format!("{}m", self.shape.worker_vcpu_milli),
                            "memory": format!("{}Mi", self.shape.worker_memory_mib),
                        },
                        "limits": {
                            "memory": format!("{}Mi", self.shape.worker_memory_mib),
                        }
                    },
                    "envFrom": [{ "secretRef": { "name": crate::k8s::store_secret_name(&self.instance_id) } }],
                    "securityContext": {
                        "allowPrivilegeEscalation": false,
                        "runAsNonRoot": true,
                        "capabilities": { "drop": ["ALL"] }
                    }
                }]
            }
        })
    }
}

/// The single-tenant compose instance (design 81 R3: compose is single
/// tenant by construction): namespace `local`, Lakekeeper and RustFS on the
/// compose network, `local[*]` / Sail `local`.
pub fn compose_conf(namespace: &str, instance_id: &str) -> ClusterConf {
    ClusterConf {
        namespace: namespace.to_owned(),
        instance_id: instance_id.to_owned(),
        state_prefix: format!("engines/{namespace}/{instance_id}/"),
        store: ObjectStore::default(),
        catalog: Catalog {
            // Lakekeeper has auth off on the internal `lake` network in
            // compose (R9); S3 keys are the compose RustFS keys from env.
            access_delegation: None,
            ..Catalog::default()
        },
        placement: Placement::Local,
        shape: Shape {
            workers: 1,
            worker_vcpu_milli: 2000,
            worker_memory_mib: 4096,
            driver_memory_mib: 2048,
        },
        proxy_idle_timeout_secs: 1800,
        otlp_endpoint: None,
    }
}

/// An environment as POSIX shell `export K='v'` lines, for an entrypoint to
/// source (values are single-quoted; `'` becomes `'\''`).
pub fn shell_env(env: &BTreeMap<String, String>) -> String {
    let mut out = String::from("# Rendered by loams-spark (SP-a D1996). No secrets.\n");
    for (k, v) in env {
        let _ = writeln!(out, "export {k}='{}'", v.replace('\'', "'\\''"));
    }
    out
}

/// `spark-defaults.conf` entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SparkDefaults(pub BTreeMap<String, String>);

impl SparkDefaults {
    /// A value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// The file: `key value` lines, sorted, with a header.
    pub fn to_file(&self) -> String {
        let mut out = String::from(
            "# Rendered by loams-spark (design 81 R16, SP-a D1996). Do not edit;\n\
             # secrets come from the environment, never this file.\n",
        );
        for (k, v) in &self.0 {
            let _ = writeln!(out, "{k} {v}");
        }
        out
    }

    /// `--conf k=v` arguments, for `spark-submit`-style launches.
    pub fn to_args(&self) -> Vec<String> {
        self.0
            .iter()
            .flat_map(|(k, v)| ["--conf".to_owned(), format!("{k}={v}")])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn conf(placement: Placement) -> ClusterConf {
        ClusterConf {
            namespace: "ns-a".into(),
            instance_id: "eng_01jabcdefghjkmnpqrstvwxyz0".into(),
            state_prefix: "engines/ns-a/eng_01jabcdefghjkmnpqrstvwxyz0/".into(),
            store: ObjectStore::default(),
            catalog: Catalog::default(),
            placement,
            shape: Shape {
                workers: 4,
                worker_vcpu_milli: 2000,
                worker_memory_mib: 8192,
                driver_memory_mib: 4096,
            },
            proxy_idle_timeout_secs: 1800,
            otlp_endpoint: None,
        }
    }

    fn kube() -> Placement {
        Placement::Kubernetes(KubePlacement {
            namespace: "loams-ns-a".into(),
            image: "ghcr.io/ostrium-labs/loams-spark:4.1.3".into(),
            service_account: "spark".into(),
            driver_service: "eng-01jabcdefghjkmnpqrstvwxyz0-driver".into(),
        })
    }

    #[test]
    fn spark_catalog_is_iceberg_rest_at_lakekeeper() {
        let d = conf(Placement::Local).spark_defaults().unwrap();
        assert_eq!(
            d.get("spark.sql.catalog.loams"),
            Some("org.apache.iceberg.spark.SparkCatalog")
        );
        assert_eq!(d.get("spark.sql.catalog.loams.type"), Some("rest"));
        assert_eq!(
            d.get("spark.sql.catalog.loams.uri"),
            Some("http://lakekeeper:8181/catalog")
        );
        assert_eq!(d.get("spark.sql.catalog.loams.warehouse"), Some("loams"));
        assert_eq!(
            d.get("spark.sql.catalog.loams.header.X-Iceberg-Access-Delegation"),
            Some("vended-credentials")
        );
        assert_eq!(d.get("spark.sql.defaultCatalog"), Some("loams"));
        assert_eq!(d.get("spark.master"), Some("local[*]"));
    }

    #[test]
    fn s3a_and_event_logs_under_the_instance_prefix() {
        let d = conf(Placement::Local).spark_defaults().unwrap();
        assert_eq!(
            d.get("spark.hadoop.fs.s3a.endpoint"),
            Some("http://rustfs:9000")
        );
        assert_eq!(d.get("spark.hadoop.fs.s3a.path.style.access"), Some("true"));
        assert_eq!(
            d.get("spark.hadoop.fs.s3a.connection.ssl.enabled"),
            Some("false")
        );
        assert_eq!(d.get("spark.eventLog.enabled"), Some("true"));
        assert_eq!(
            d.get("spark.eventLog.dir"),
            Some("s3a://loams-engines/engines/ns-a/eng_01jabcdefghjkmnpqrstvwxyz0/eventlog/")
        );
        assert!(
            d.get("spark.sql.warehouse.dir")
                .unwrap()
                .ends_with("/scratch/warehouse/")
        );
    }

    #[test]
    fn no_secret_is_rendered() {
        let mut c = conf(kube());
        c.catalog.oauth2 = Some(OAuth2 {
            server_uri: "https://id.loams.dev/oauth/v2/token".into(),
            scope: Some("lakekeeper".into()),
            credential_env: "LAKEKEEPER_CREDENTIAL".into(),
        });
        let file = c.spark_defaults().unwrap().to_file();
        assert!(file.contains("spark.sql.catalog.loams.credential ${env:LAKEKEEPER_CREDENTIAL}"));
        for needle in ["AWS_SECRET", "AWS_ACCESS_KEY", "rustfsadmin", "password"] {
            assert!(!file.contains(needle), "{needle}");
        }
        let env = c.sail_env().unwrap();
        assert!(!env.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!env.contains_key("AWS_ACCESS_KEY_ID"));
    }

    #[test]
    fn kubernetes_executors() {
        let d = conf(kube()).spark_defaults().unwrap();
        assert_eq!(
            d.get("spark.master"),
            Some("k8s://https://kubernetes.default.svc")
        );
        assert_eq!(d.get("spark.submit.deployMode"), Some("client"));
        assert_eq!(d.get("spark.dynamicAllocation.maxExecutors"), Some("4"));
        assert_eq!(d.get("spark.executor.cores"), Some("2"));
        assert_eq!(d.get("spark.executor.memory"), Some("7447m"));
        assert_eq!(
            d.get("spark.driver.host"),
            Some("eng-01jabcdefghjkmnpqrstvwxyz0-driver.loams-ns-a.svc")
        );
        assert_eq!(
            d.get("spark.kubernetes.executor.podNamePrefix"),
            Some("eng-01jabcdefghjkmnpqrstvwxyz0")
        );
    }

    #[test]
    fn sail_env_local_and_kubernetes() {
        let env = conf(Placement::Local).sail_env().unwrap();
        assert_eq!(env["SAIL_MODE"], "local");
        assert_eq!(
            env["SAIL_CATALOG__LIST"],
            r#"[{type="iceberg-rest", name="loams", uri="http://lakekeeper:8181/catalog", warehouse="loams"}]"#
        );
        assert_eq!(env["SAIL_CATALOG__DEFAULT_CATALOG"], "loams");
        assert_eq!(env["AWS_ENDPOINT"], "http://rustfs:9000");
        assert_eq!(env["AWS_ALLOW_HTTP"], "true");
        assert_eq!(env["SAIL_SPARK__SESSION_TIMEOUT_SECS"], "2100");
        assert_eq!(
            env["SAIL_EXECUTION__CHECKPOINT__PATH"],
            "s3://loams-engines/engines/ns-a/eng_01jabcdefghjkmnpqrstvwxyz0/scratch/checkpoints/"
        );

        let env = conf(kube()).sail_env().unwrap();
        assert_eq!(env["SAIL_MODE"], "kubernetes-cluster");
        assert_eq!(env["SAIL_CLUSTER__WORKER_MAX_COUNT"], "4");
        assert_eq!(env["SAIL_CLUSTER__SHUFFLE_BACKEND__TYPE"], "storage");
        let tpl: serde_json::Value =
            serde_json::from_str(&env["SAIL_KUBERNETES__WORKER_POD_TEMPLATE"]).unwrap();
        assert_eq!(
            tpl["spec"]["containers"][0]["resources"]["requests"]["memory"],
            "8192Mi"
        );
    }

    #[test]
    fn toml_strings_are_escaped() {
        let mut c = conf(Placement::Local);
        c.catalog.uri = r#"http://x/"a"\b"#.into();
        let env = c.sail_env().unwrap();
        assert!(env["SAIL_CATALOG__LIST"].contains(r#"uri="http://x/\"a\"\\b""#));
    }

    #[test]
    fn line_breaks_and_bad_prefixes_are_refused() {
        let mut c = conf(Placement::Local);
        c.catalog.warehouse = "loams\nspark.master yarn".into();
        assert!(c.spark_defaults().is_err());
        let mut c = conf(Placement::Local);
        c.state_prefix = "/abs".into();
        assert!(c.sail_env().is_err());
    }

    #[test]
    fn compose_conf_renders_both_backends() {
        let c = compose_conf("local", "eng_compose");
        let d = c.spark_defaults().unwrap();
        assert_eq!(d.get("spark.master"), Some("local[*]"));
        assert_eq!(
            d.get("spark.eventLog.dir"),
            Some("s3a://loams-engines/engines/local/eng_compose/eventlog/")
        );
        assert!(
            d.get("spark.sql.catalog.loams.header.X-Iceberg-Access-Delegation")
                .is_none()
        );
        let env = shell_env(&c.sail_env().unwrap());
        assert!(env.contains("export SAIL_MODE='local'\n"));
    }

    #[test]
    fn shell_env_quotes() {
        let env = BTreeMap::from([("A".to_owned(), "it's $HOME".to_owned())]);
        assert!(shell_env(&env).ends_with("export A='it'\\''s $HOME'\n"));
    }

    #[test]
    fn file_and_args_forms() {
        let d = conf(Placement::Local).spark_defaults().unwrap();
        let file = d.to_file();
        assert!(file.contains("\nspark.master local[*]\n"));
        let args = d.to_args();
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--conf" && w[1] == "spark.master=local[*]")
        );
    }
}
