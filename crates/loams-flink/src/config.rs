//! A tenant's session-cluster spec and its Flink `config.yaml` (D1972,
//! D1974, D1975).
//!
//! The one rendered file serves the JobManager, the TaskManagers and the SQL
//! Gateway. It is Flink 2.x's `config.yaml` in the flattened form Flink
//! accepts (`a.b.c: value`, one per line, sorted), so goldens stay stable.
//! Credentials are never in it: the S3 keys reach Flink through
//! `FLINK_PROPERTIES` from a Secret ([`flink_properties`]), which the
//! image's entrypoint appends at start.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::error::FlinkError;
use crate::storage::{ObjectStore, TenantId};

/// Where the cluster runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// The compose `flink` profile: one tenant, one JobManager, HA off and
    /// the controller's recovery record on S3 instead (D1974).
    Compose,
    /// One namespace per tenant with Kubernetes HA.
    Kubernetes,
}

/// JobManager high availability (D1974).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaMode {
    /// None in Flink: one JobManager. The controller replays jobs from its
    /// recovery record (compose).
    ControllerRecovery,
    /// `high-availability.type: kubernetes` (leader election on ConfigMaps,
    /// metadata in `high-availability.storageDir`).
    Kubernetes,
}

/// A t-shirt size (D1972).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    /// 1 TaskManager × 2 slots, 2 GiB.
    Dev,
    /// 2 × 2 slots, 4 GiB each.
    Small,
    /// 4 × 4 slots, 8 GiB each.
    Medium,
    /// 8 × 4 slots, 16 GiB each.
    Large,
}

/// The resources of a cluster: [`Size::resources`], optionally overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    /// TaskManager replicas.
    pub taskmanagers: u32,
    /// Slots per TaskManager (`taskmanager.numberOfTaskSlots`).
    pub slots_per_taskmanager: u32,
    /// TaskManager process memory, MiB.
    pub taskmanager_memory_mb: u32,
    /// TaskManager CPU, millicores.
    pub taskmanager_cpu_milli: u32,
    /// JobManager process memory, MiB.
    pub jobmanager_memory_mb: u32,
    /// JobManager CPU, millicores.
    pub jobmanager_cpu_milli: u32,
}

impl Size {
    /// The size's resources.
    pub fn resources(self) -> Resources {
        let (taskmanagers, slots, tm_mb, tm_cpu, jm_mb, jm_cpu) = match self {
            Size::Dev => (1, 2, 2048, 1000, 1600, 500),
            Size::Small => (2, 2, 4096, 2000, 2048, 1000),
            Size::Medium => (4, 4, 8192, 4000, 4096, 1000),
            Size::Large => (8, 4, 16384, 4000, 4096, 2000),
        };
        Resources {
            taskmanagers,
            slots_per_taskmanager: slots,
            taskmanager_memory_mb: tm_mb,
            taskmanager_cpu_milli: tm_cpu,
            jobmanager_memory_mb: jm_mb,
            jobmanager_cpu_milli: jm_cpu,
        }
    }
}

impl Resources {
    /// Checks the bounds Flink and the controller can serve.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] naming the field.
    pub fn check(&self) -> Result<(), FlinkError> {
        let bad = |what: &str| Err(FlinkError::Invalid(format!("resources: {what}")));
        if self.taskmanagers > 64 {
            return bad("at most 64 TaskManagers");
        }
        if !(1..=32).contains(&self.slots_per_taskmanager) {
            return bad("1 to 32 slots per TaskManager");
        }
        if self.taskmanager_memory_mb < 1024 {
            return bad("TaskManager memory of at least 1024 MiB");
        }
        if self.jobmanager_memory_mb < 1024 {
            return bad("JobManager memory of at least 1024 MiB");
        }
        if self.taskmanager_cpu_milli == 0 || self.jobmanager_cpu_milli == 0 {
            return bad("CPU above 0");
        }
        Ok(())
    }

    /// Slots in all.
    pub fn total_slots(&self) -> u32 {
        self.taskmanagers.saturating_mul(self.slots_per_taskmanager)
    }
}

/// The service names and ports the roles reach each other on (the same in
/// compose and in a tenant's namespace).
pub mod addr {
    /// The JobManager's Service (and compose service) name.
    pub const JOBMANAGER: &str = "flink-jobmanager";
    /// The SQL Gateway's Service (and compose service) name.
    pub const SQL_GATEWAY: &str = "flink-sql-gateway";
    /// JobManager RPC.
    pub const RPC_PORT: u16 = 6123;
    /// TaskManager RPC.
    pub const TM_RPC_PORT: u16 = 6122;
    /// Blob server.
    pub const BLOB_PORT: u16 = 6124;
    /// JobManager REST.
    pub const REST_PORT: u16 = 8081;
    /// SQL Gateway REST.
    pub const SQL_GATEWAY_PORT: u16 = 8083;
    /// Prometheus metrics, every role.
    pub const METRICS_PORT: u16 = 9249;
}

/// Keys the cluster owns: a spec's `extra` and a session's `SET` may not
/// change them (D1977). Matched as exact keys or, ending with `.`, as
/// prefixes.
pub const PROTECTED_KEYS: &[&str] = &[
    "blob.server.port",
    "execution.checkpointing.dir",
    "execution.checkpointing.savepoint-dir",
    "execution.checkpointing.externalized-checkpoint-retention",
    "execution.state-recovery.path",
    "execution.savepoint.path",
    "fs.",
    "high-availability.",
    "jobmanager.",
    "kubernetes.",
    "metrics.",
    "rest.",
    "s3.",
    "sql-gateway.",
    "state.backend.type",
    "state.checkpoints.dir",
    "state.savepoints.dir",
    "taskmanager.",
    "web.",
];

/// Whether `key` is cluster-owned ([`PROTECTED_KEYS`]), case-insensitively.
pub fn is_protected(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    PROTECTED_KEYS.iter().any(|p| {
        if p.ends_with('.') {
            key.starts_with(p)
        } else {
            key == *p
        }
    })
}

/// A tenant's session cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterSpec {
    /// The tenant.
    pub tenant: TenantId,
    /// Where it runs.
    pub platform: Platform,
    /// The image (pinned by digest).
    pub image: String,
    /// The Flink version the image carries (for upgrade checks).
    pub flink_version: String,
    /// The size the resources came from.
    pub size: Size,
    /// The resources.
    pub resources: Resources,
    /// Where state goes.
    pub storage: ObjectStore,
    /// JobManager HA.
    pub ha: HaMode,
    /// Checkpoint interval, seconds.
    pub checkpoint_interval_secs: u32,
    /// Retained checkpoints.
    pub checkpoints_retained: u32,
    /// Extra Flink options (not [`PROTECTED_KEYS`]).
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl ClusterSpec {
    /// The defaults for `tenant` on `platform` at `size`: the pinned image,
    /// RustFS bucket `flink`, HA by platform, checkpoints every 60 s, 3
    /// retained.
    pub fn new(tenant: TenantId, platform: Platform, size: Size) -> Self {
        Self {
            tenant,
            platform,
            image: crate::DEFAULT_IMAGE.to_owned(),
            flink_version: crate::FLINK_VERSION.to_owned(),
            size,
            resources: size.resources(),
            storage: ObjectStore::default(),
            ha: match platform {
                Platform::Compose => HaMode::ControllerRecovery,
                Platform::Kubernetes => HaMode::Kubernetes,
            },
            checkpoint_interval_secs: 60,
            checkpoints_retained: 3,
            extra: BTreeMap::new(),
        }
    }

    /// Checks everything [`render`] relies on.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] naming the field.
    pub fn check(&self) -> Result<(), FlinkError> {
        self.resources.check()?;
        self.storage.check()?;
        if self.platform == Platform::Compose && self.ha == HaMode::Kubernetes {
            return Err(FlinkError::Invalid(
                "Kubernetes HA needs the Kubernetes platform".to_owned(),
            ));
        }
        if self.image.is_empty() || self.image.contains(char::is_whitespace) {
            return Err(FlinkError::Invalid(format!("image {:?}", self.image)));
        }
        if !(1..=3600).contains(&self.checkpoint_interval_secs) {
            return Err(FlinkError::Invalid(
                "checkpoint interval of 1 to 3600 s".to_owned(),
            ));
        }
        if !(1..=100).contains(&self.checkpoints_retained) {
            return Err(FlinkError::Invalid(
                "1 to 100 retained checkpoints".to_owned(),
            ));
        }
        for (k, v) in &self.extra {
            check_key(k)?;
            if is_protected(k) {
                return Err(FlinkError::Invalid(format!(
                    "{k} is set by the controller and cannot be overridden"
                )));
            }
            if v.contains(['\n', '\r']) {
                return Err(FlinkError::Invalid(format!("{k}: a value is one line")));
            }
        }
        Ok(())
    }
}

fn check_key(k: &str) -> Result<(), FlinkError> {
    let ok = !k.is_empty()
        && k.len() <= 200
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && !k.starts_with('.')
        && !k.ends_with('.');
    if ok {
        Ok(())
    } else {
        Err(FlinkError::Invalid(format!("option key {k:?}")))
    }
}

/// A rendered configuration: keys to values, sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlinkConfig {
    entries: BTreeMap<String, String>,
}

impl FlinkConfig {
    /// The value of `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Every entry, sorted by key.
    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }

    fn set(&mut self, key: &str, value: impl ToString) {
        self.entries.insert(key.to_owned(), value.to_string());
    }

    /// The `config.yaml` text: a header comment, then `key: value` lines in
    /// key order, values quoted where YAML needs it.
    pub fn to_yaml(&self) -> String {
        let mut out = String::from(HEADER);
        for (k, v) in &self.entries {
            // Writing to a String cannot fail.
            let _ = writeln!(out, "{k}: {}", yaml_scalar(v));
        }
        out
    }
}

/// The header of every rendered `config.yaml`.
pub const HEADER: &str = "# Flink config.yaml rendered by loams-flink (design 81, D1972-D1975).\n\
# Do not edit: the controller re-renders it. Credentials come from FLINK_PROPERTIES.\n";

/// `value` as a YAML scalar: plain when it is safe, else single-quoted
/// (`'` doubled).
pub fn yaml_scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b':' | b'-' | b'=' | b'+')
        })
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'/')
        && !value.ends_with(':');
    if plain {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

/// Renders `spec`'s `config.yaml` entries.
///
/// # Errors
///
/// [`FlinkError::Invalid`] from [`ClusterSpec::check`].
pub fn render(spec: &ClusterSpec) -> Result<FlinkConfig, FlinkError> {
    spec.check()?;
    let paths = spec.storage.tenant(&spec.tenant);
    let r = &spec.resources;
    let mut c = FlinkConfig::default();

    // Addresses: every role binds all interfaces and finds the JobManager
    // by its Service name.
    c.set("jobmanager.rpc.address", addr::JOBMANAGER);
    c.set("jobmanager.rpc.port", addr::RPC_PORT);
    c.set("jobmanager.bind-host", "0.0.0.0");
    c.set("taskmanager.bind-host", "0.0.0.0");
    c.set("taskmanager.rpc.port", addr::TM_RPC_PORT);
    c.set("blob.server.port", addr::BLOB_PORT);
    c.set("rest.address", addr::JOBMANAGER);
    c.set("rest.port", addr::REST_PORT);
    c.set("rest.bind-address", "0.0.0.0");
    // No jar uploads or runs through the JobManager's REST API: SQL comes in
    // through the gateway only (D1977).
    c.set("web.submit.enable", "false");
    c.set("web.cancel.enable", "true");

    // Resources.
    c.set(
        "jobmanager.memory.process.size",
        format!("{}m", r.jobmanager_memory_mb),
    );
    c.set(
        "taskmanager.memory.process.size",
        format!("{}m", r.taskmanager_memory_mb),
    );
    c.set("taskmanager.numberOfTaskSlots", r.slots_per_taskmanager);
    c.set("parallelism.default", 1);

    // State: RocksDB, incremental, retained checkpoints on the tenant's
    // prefix (D1975). The 1.x keys are written too; Flink 2.x reads the
    // new ones.
    c.set("state.backend.type", "rocksdb");
    c.set("execution.checkpointing.incremental", "true");
    c.set(
        "execution.checkpointing.interval",
        format!("{} s", spec.checkpoint_interval_secs),
    );
    c.set("execution.checkpointing.mode", "EXACTLY_ONCE");
    c.set(
        "execution.checkpointing.externalized-checkpoint-retention",
        "RETAIN_ON_CANCELLATION",
    );
    c.set(
        "execution.checkpointing.num-retained",
        spec.checkpoints_retained,
    );
    c.set("execution.checkpointing.dir", paths.checkpoints());
    c.set("execution.checkpointing.savepoint-dir", paths.savepoints());
    c.set("state.checkpoints.dir", paths.checkpoints());
    c.set("state.savepoints.dir", paths.savepoints());
    c.set("restart-strategy.type", "exponential-delay");

    // S3 through the bundled Presto file system (checkpoints).
    if let Some(endpoint) = &spec.storage.endpoint {
        c.set("s3.endpoint", endpoint);
    }
    c.set(
        "s3.path.style.access",
        if spec.storage.path_style {
            "true"
        } else {
            "false"
        },
    );

    // HA (D1974).
    match spec.ha {
        HaMode::ControllerRecovery => c.set("high-availability.type", "NONE"),
        HaMode::Kubernetes => {
            c.set("high-availability.type", "kubernetes");
            c.set("high-availability.storageDir", paths.ha());
            c.set("kubernetes.cluster-id", spec.tenant.cluster_name());
            c.set("kubernetes.namespace", spec.tenant.cluster_name());
        }
    }

    // The SQL Gateway: its REST endpoint, and sessions per tenant.
    c.set("sql-gateway.endpoint.type", "rest");
    c.set("sql-gateway.endpoint.rest.address", addr::SQL_GATEWAY);
    c.set("sql-gateway.endpoint.rest.bind-address", "0.0.0.0");
    c.set("sql-gateway.endpoint.rest.port", addr::SQL_GATEWAY_PORT);
    c.set("sql-gateway.session.idle-timeout", "30 min");
    c.set("sql-gateway.session.check-interval", "1 min");
    c.set("sql-gateway.session.max-num", 1000);

    // Metrics for Loams Meter.
    c.set(
        "metrics.reporter.prom.factory.class",
        "org.apache.flink.metrics.prometheus.PrometheusReporterFactory",
    );
    c.set("metrics.reporter.prom.port", addr::METRICS_PORT);

    for (k, v) in &spec.extra {
        c.set(k, v);
    }
    Ok(c)
}

/// The S3 credentials Flink needs, never rendered into `config.yaml`.
#[derive(Clone, PartialEq, Eq)]
pub struct S3Credentials {
    /// The access key id.
    pub access_key: String,
    /// The secret key.
    pub secret_key: String,
}

impl std::fmt::Debug for S3Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key", &self.access_key)
            .field("secret_key", &"<redacted>")
            .finish()
    }
}

/// The `FLINK_PROPERTIES` value carrying `creds` (the official image's
/// entrypoint appends it to `config.yaml` at start).
///
/// # Errors
///
/// [`FlinkError::Invalid`] for a key with a line break.
pub fn flink_properties(creds: &S3Credentials) -> Result<String, FlinkError> {
    if creds.access_key.contains(['\n', '\r']) || creds.secret_key.contains(['\n', '\r']) {
        return Err(FlinkError::Invalid(
            "S3 credentials are one line each".to_owned(),
        ));
    }
    Ok(format!(
        "s3.access-key: {}\ns3.secret-key: {}\n",
        yaml_scalar(&creds.access_key),
        yaml_scalar(&creds.secret_key)
    ))
}

/// The plugin the image's entrypoint copies from `opt/` into `plugins/`
/// (`ENABLE_BUILT_IN_PLUGINS`): Presto's S3 file system, for checkpoints.
pub fn s3_plugin_jar(flink_version: &str) -> String {
    format!("flink-s3-fs-presto-{flink_version}.jar")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId::new(s).unwrap()
    }

    #[test]
    fn scalars_are_quoted_only_when_needed() {
        assert_eq!(yaml_scalar("rocksdb"), "rocksdb");
        assert_eq!(
            yaml_scalar("s3://flink/flink/t1/checkpoints"),
            "s3://flink/flink/t1/checkpoints"
        );
        assert_eq!(yaml_scalar("60 s"), "'60 s'");
        assert_eq!(yaml_scalar(""), "''");
        assert_eq!(yaml_scalar("it's"), "'it''s'");
        assert_eq!(yaml_scalar("-Xmx1g"), "'-Xmx1g'");
        assert_eq!(yaml_scalar("a: b"), "'a: b'");
        assert_eq!(yaml_scalar("#x"), "'#x'");
        assert_eq!(yaml_scalar("x:"), "'x:'");
    }

    #[test]
    fn protected_keys_match_exactly_or_by_prefix() {
        assert!(is_protected("execution.checkpointing.dir"));
        assert!(is_protected("STATE.CHECKPOINTS.DIR"));
        assert!(is_protected("high-availability.storageDir"));
        assert!(is_protected("s3.secret-key"));
        assert!(!is_protected("table.exec.state.ttl"));
        assert!(!is_protected("execution.checkpointing.interval"));
        assert!(!is_protected("pipeline.name"));
    }

    #[test]
    fn extra_options_cannot_override_the_cluster() {
        let mut spec = ClusterSpec::new(tenant("t1"), Platform::Compose, Size::Dev);
        spec.extra
            .insert("table.exec.state.ttl".into(), "1 h".into());
        assert!(spec.check().is_ok());
        spec.extra
            .insert("state.checkpoints.dir".into(), "s3://x/y".into());
        assert!(spec.check().is_err());
        let mut spec = ClusterSpec::new(tenant("t1"), Platform::Compose, Size::Dev);
        spec.extra.insert("a b".into(), "c".into());
        assert!(spec.check().is_err());
        let mut spec = ClusterSpec::new(tenant("t1"), Platform::Compose, Size::Dev);
        spec.ha = HaMode::Kubernetes;
        assert!(spec.check().is_err());
    }

    #[test]
    fn kubernetes_ha_names_the_tenant_cluster() {
        let spec = ClusterSpec::new(tenant("acme"), Platform::Kubernetes, Size::Small);
        let c = render(&spec).unwrap();
        assert_eq!(c.get("high-availability.type"), Some("kubernetes"));
        assert_eq!(
            c.get("high-availability.storageDir"),
            Some("s3://flink/flink/acme/ha")
        );
        assert_eq!(c.get("kubernetes.cluster-id"), Some("loams-flink-acme"));
        assert_eq!(c.get("taskmanager.numberOfTaskSlots"), Some("2"));
        assert_eq!(c.get("taskmanager.memory.process.size"), Some("4096m"));
    }

    #[test]
    fn compose_has_no_flink_ha_and_no_credentials() {
        let spec = ClusterSpec::new(tenant("local"), Platform::Compose, Size::Dev);
        let yaml = render(&spec).unwrap().to_yaml();
        assert!(yaml.contains("high-availability.type: NONE\n"));
        assert!(!yaml.contains("access-key"));
        assert!(!yaml.contains("secret-key"));
        assert!(yaml.starts_with(HEADER));
    }

    #[test]
    fn credentials_go_to_flink_properties_redacted_in_debug() {
        let creds = S3Credentials {
            access_key: "AK".into(),
            secret_key: "s3cr3t/x+y".into(),
        };
        assert_eq!(
            flink_properties(&creds).unwrap(),
            "s3.access-key: AK\ns3.secret-key: s3cr3t/x+y\n"
        );
        assert!(!format!("{creds:?}").contains("s3cr3t"));
        let bad = S3Credentials {
            access_key: "a\nb".into(),
            secret_key: "c".into(),
        };
        assert!(flink_properties(&bad).is_err());
    }

    #[test]
    fn sizes_are_within_bounds() {
        for size in [Size::Dev, Size::Small, Size::Medium, Size::Large] {
            size.resources().check().unwrap();
        }
        assert_eq!(Size::Medium.resources().total_slots(), 16);
    }
}
