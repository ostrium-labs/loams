//! The Spark batch job spec (D2000): what a user submits.
//!
//! The JSON shape is the contract with the Python SDK
//! (`loams.spark.submit_job`) and the HTTP facade; keep the serde names stable.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::JobsError;

/// The longest job name: it becomes part of a pod name (`<name>-<run>-<n>`,
/// a DNS label of at most 63 characters, with a 26-character ULID).
pub const MAX_NAME_LEN: usize = 30;

/// The most attempts a job may ask for.
pub const MAX_ATTEMPTS: u32 = 10;

/// The most executors a job may ask for (the namespace's quota applies too).
pub const MAX_EXECUTORS: u32 = 256;

/// One Spark batch job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SparkJobSpec {
    /// `[a-z][a-z0-9-]*`, at most [`MAX_NAME_LEN`] characters.
    pub name: String,
    /// What runs.
    pub app: SparkApp,
    /// The application's arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Spark conf (`spark.*` keys only; see [`RESERVED_CONF`]).
    #[serde(default)]
    pub conf: BTreeMap<String, String>,
    /// Driver and executor sizes.
    #[serde(default)]
    pub resources: Resources,
    /// Attempts and backoff.
    #[serde(default)]
    pub retry: RetryPolicy,
    /// Where it runs; `auto` picks from the app kind.
    #[serde(default)]
    pub backend: Backend,
    /// The driver image, when not the backend's default.
    #[serde(default)]
    pub image: Option<String>,
    /// Extra environment for the driver (not secrets: use the namespace's
    /// secrets for those).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Kill an attempt after this many seconds.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// The application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SparkApp {
    /// A PySpark script, with extra Python files (`--py-files`).
    Pyspark {
        /// `s3://…` (or a local path for the local runner).
        file: String,
        /// Extra `.py`, `.zip` or `.egg` files.
        #[serde(default)]
        py_files: Vec<String>,
    },
    /// A JAR and its main class.
    Jar {
        /// `s3://…` (or a local path for the local runner).
        file: String,
        /// The fully qualified main class.
        main_class: String,
        /// Extra jars (`--jars`).
        #[serde(default)]
        jars: Vec<String>,
    },
    /// A file of Spark SQL statements, separated by `;`.
    Sql {
        /// `s3://…` (or a local path for the local runner).
        file: String,
    },
}

impl SparkApp {
    /// The main file.
    pub fn file(&self) -> &str {
        match self {
            Self::Pyspark { file, .. } | Self::Jar { file, .. } | Self::Sql { file } => file,
        }
    }

    /// `pyspark`, `jar` or `sql`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Pyspark { .. } => "pyspark",
            Self::Jar { .. } => "jar",
            Self::Sql { .. } => "sql",
        }
    }
}

/// Where a job runs (D2001).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// `connect` for PySpark and SQL, `kubernetes` for a JAR.
    #[default]
    Auto,
    /// A short-lived driver against the namespace's Spark Connect endpoint
    /// (Sail by default; design 26 §9.2).
    Connect,
    /// Apache Spark on Kubernetes: a driver pod running `spark-submit` in
    /// client mode, which starts its own executors (design 26 §9.4).
    Kubernetes,
    /// A child process on this host (`loams dev`, compose; D2002).
    Local,
}

impl Backend {
    /// `auto` resolved for an app.
    pub fn resolve(self, app: &SparkApp) -> Self {
        match (self, app) {
            (Self::Auto, SparkApp::Jar { .. }) => Self::Kubernetes,
            (Self::Auto, _) => Self::Connect,
            (other, _) => other,
        }
    }

    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Connect => "connect",
            Self::Kubernetes => "kubernetes",
            Self::Local => "local",
        }
    }
}

/// Driver and executor sizes. Memory is Spark's notation (`512m`, `2g`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    /// Driver cores.
    #[serde(default = "one")]
    pub driver_cores: u32,
    /// Driver memory.
    #[serde(default = "one_gig")]
    pub driver_memory: String,
    /// Cores per executor.
    #[serde(default = "one")]
    pub executor_cores: u32,
    /// Memory per executor.
    #[serde(default = "one_gig")]
    pub executor_memory: String,
    /// Executors (Kubernetes backend only; Spark Connect servers size
    /// themselves).
    #[serde(default = "one")]
    pub executors: u32,
}

fn one() -> u32 {
    1
}

fn one_gig() -> String {
    "1g".to_string()
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            driver_cores: 1,
            driver_memory: one_gig(),
            executor_cores: 1,
            executor_memory: one_gig(),
            executors: 1,
        }
    }
}

/// Attempts and the wait between them (D2003).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Attempts in all, 1 to [`MAX_ATTEMPTS`]; 1 = no retry.
    #[serde(default = "one")]
    pub max_attempts: u32,
    /// The wait before the next attempt.
    #[serde(default)]
    pub backoff: Backoff,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff: Backoff::default(),
        }
    }
}

/// The wait before attempt `n + 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Backoff {
    /// The same wait every time.
    Fixed {
        /// Seconds.
        secs: u64,
    },
    /// `base * 2^(n-1)`, capped.
    Exponential {
        /// The first wait, in seconds.
        base_secs: u64,
        /// The cap, in seconds.
        max_secs: u64,
    },
}

impl Default for Backoff {
    fn default() -> Self {
        Self::Exponential {
            base_secs: 30,
            max_secs: 600,
        }
    }
}

impl Backoff {
    /// The wait after failed attempt `attempt` (1-based).
    pub fn delay(&self, attempt: u32) -> Duration {
        match *self {
            Self::Fixed { secs } => Duration::from_secs(secs),
            Self::Exponential {
                base_secs,
                max_secs,
            } => {
                let shift = attempt.saturating_sub(1).min(30);
                let secs = base_secs.saturating_mul(1u64 << shift);
                Duration::from_secs(secs.min(max_secs))
            }
        }
    }
}

/// Conf keys Loams sets and a job may not: the master and deploy mode, where
/// the pods go and as whom, the Connect endpoint, and the managed catalog.
pub const RESERVED_CONF: &[&str] = &[
    "spark.master",
    "spark.remote",
    "spark.submit.deployMode",
    "spark.kubernetes.namespace",
    "spark.kubernetes.driver.pod.name",
    "spark.kubernetes.authenticate.",
    "spark.kubernetes.container.image",
    "spark.kubernetes.executor.podTemplateFile",
    "spark.kubernetes.driver.podTemplateFile",
    "spark.driver.host",
    "spark.driver.bindAddress",
    "spark.sql.catalog.loams",
];

fn reserved(key: &str) -> bool {
    RESERVED_CONF.iter().any(|r| {
        if r.ends_with('.') {
            key.starts_with(r)
        } else {
            key == *r || key.starts_with(&format!("{r}."))
        }
    })
}

/// A Spark memory string: digits and an optional `k`, `m`, `g` or `t`.
pub fn valid_memory(s: &str) -> bool {
    let digits = s.trim_end_matches(['k', 'm', 'g', 't', 'K', 'M', 'G', 'T']);
    let suffix = s.len() - digits.len();
    suffix <= 1 && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && digits != "0"
}

/// Kubernetes' quantity for a Spark memory string (`2g` → `2Gi`).
pub fn k8s_memory(s: &str) -> String {
    let (digits, unit) = match s.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => (&s[..s.len() - 1], c.to_ascii_lowercase()),
        _ => (s, 'b'),
    };
    match unit {
        'k' => format!("{digits}Ki"),
        'm' => format!("{digits}Mi"),
        'g' => format!("{digits}Gi"),
        't' => format!("{digits}Ti"),
        _ => digits.to_string(),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_class(class: &str) -> bool {
    !class.is_empty()
        && class.split('.').all(|part| {
            part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        })
}

fn valid_file(file: &str) -> bool {
    !file.trim().is_empty() && !file.contains(['\n', '\r', '\0'])
}

fn valid_env_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.starts_with("LOAMS_")
        && !name.starts_with("SPARK_")
        && !name.starts_with("AWS_")
}

impl SparkJobSpec {
    /// A spec for `app` with every default.
    pub fn new(name: impl Into<String>, app: SparkApp) -> Self {
        Self {
            name: name.into(),
            app,
            args: Vec::new(),
            conf: BTreeMap::new(),
            resources: Resources::default(),
            retry: RetryPolicy::default(),
            backend: Backend::Auto,
            image: None,
            env: BTreeMap::new(),
            timeout_secs: None,
        }
    }

    /// Checks everything a renderer relies on; the error names the field.
    pub fn validate(&self) -> Result<(), JobsError> {
        let bad = |m: String| Err(JobsError::InvalidSpec(m));
        if !valid_name(&self.name) {
            return bad(format!(
                "name {:?}: use [a-z][a-z0-9-]*, at most {MAX_NAME_LEN} characters",
                self.name
            ));
        }
        if !valid_file(self.app.file()) {
            return bad("app.file is empty or has a line break".into());
        }
        match &self.app {
            SparkApp::Pyspark { py_files, .. } => {
                if let Some(f) = py_files.iter().find(|f| !valid_file(f)) {
                    return bad(format!("app.py_files: {f:?} is not a file"));
                }
            }
            SparkApp::Jar {
                main_class, jars, ..
            } => {
                if !valid_class(main_class) {
                    return bad(format!("app.main_class {main_class:?} is not a class name"));
                }
                if let Some(f) = jars.iter().find(|f| !valid_file(f)) {
                    return bad(format!("app.jars: {f:?} is not a file"));
                }
            }
            SparkApp::Sql { .. } => {}
        }
        if self.backend.resolve(&self.app) == Backend::Connect
            && matches!(self.app, SparkApp::Jar { .. })
        {
            return bad(
                "a JAR cannot run over Spark Connect: use backend kubernetes or local".into(),
            );
        }
        for key in self.conf.keys() {
            if !key.starts_with("spark.") {
                return bad(format!("conf {key:?}: only spark.* keys"));
            }
            if reserved(key) {
                return bad(format!("conf {key:?} is set by Loams"));
            }
        }
        let r = &self.resources;
        if r.driver_cores == 0 || r.executor_cores == 0 {
            return bad("resources: cores must be at least 1".into());
        }
        if r.executors == 0 || r.executors > MAX_EXECUTORS {
            return bad(format!("resources.executors must be 1 to {MAX_EXECUTORS}"));
        }
        for (field, m) in [
            ("driver_memory", &r.driver_memory),
            ("executor_memory", &r.executor_memory),
        ] {
            if !valid_memory(m) {
                return bad(format!(
                    "resources.{field} {m:?}: use Spark's notation, like 2g"
                ));
            }
        }
        if self.retry.max_attempts == 0 || self.retry.max_attempts > MAX_ATTEMPTS {
            return bad(format!("retry.max_attempts must be 1 to {MAX_ATTEMPTS}"));
        }
        if let Some(name) = self.env.keys().find(|k| !valid_env_name(k)) {
            return bad(format!(
                "env {name:?}: a plain name, not LOAMS_*, SPARK_* or AWS_*"
            ));
        }
        if self.timeout_secs == Some(0) {
            return bad("timeout_secs must be positive".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py() -> SparkJobSpec {
        SparkJobSpec::new(
            "nightly",
            SparkApp::Pyspark {
                file: "s3://b/jobs/etl.py".into(),
                py_files: vec![],
            },
        )
    }

    #[test]
    fn json_shape_matches_the_python_sdk() {
        let json = serde_json::json!({
            "name": "etl",
            "app": {"kind": "jar", "file": "s3://b/a.jar", "main_class": "com.x.Main"},
            "args": ["--day", "2026-10-27"],
            "conf": {"spark.sql.shuffle.partitions": "8"},
            "resources": {"driver_cores": 1, "driver_memory": "1g", "executor_cores": 2,
                          "executor_memory": "4g", "executors": 3},
            "retry": {"max_attempts": 3, "backoff": {"kind": "exponential", "base_secs": 10, "max_secs": 600}},
            "backend": "auto",
            "image": null
        });
        let spec: SparkJobSpec = serde_json::from_value(json).expect("parse");
        assert_eq!(spec.app.kind(), "jar");
        assert_eq!(spec.backend.resolve(&spec.app), Backend::Kubernetes);
        assert_eq!(spec.resources.executors, 3);
        spec.validate().expect("valid");
    }

    #[test]
    fn unknown_fields_are_refused() {
        let json =
            serde_json::json!({"name": "x", "app": {"kind": "sql", "file": "a.sql"}, "nope": 1});
        assert!(serde_json::from_value::<SparkJobSpec>(json).is_err());
    }

    #[test]
    fn auto_backend() {
        assert_eq!(py().backend.resolve(&py().app), Backend::Connect);
        let sql = SparkApp::Sql {
            file: "a.sql".into(),
        };
        assert_eq!(Backend::Auto.resolve(&sql), Backend::Connect);
        assert_eq!(Backend::Local.resolve(&sql), Backend::Local);
    }

    #[test]
    fn validation() {
        py().validate().expect("valid");
        let mut s = py();
        s.name = "Bad_Name".into();
        assert!(s.validate().is_err());
        let mut s = py();
        s.name = "a".repeat(MAX_NAME_LEN + 1);
        assert!(s.validate().is_err());
        let mut s = py();
        s.conf.insert("spark.master".into(), "local".into());
        assert!(s.validate().is_err());
        let mut s = py();
        s.conf.insert(
            "spark.kubernetes.authenticate.driver.serviceAccountName".into(),
            "x".into(),
        );
        assert!(s.validate().is_err());
        let mut s = py();
        s.conf
            .insert("spark.sql.catalog.loams.uri".into(), "x".into());
        assert!(s.validate().is_err());
        let mut s = py();
        s.conf
            .insert("spark.sql.catalog.other.uri".into(), "x".into());
        s.validate().expect("another catalog is the job's own");
        let mut s = py();
        s.conf.insert("hadoop.x".into(), "1".into());
        assert!(s.validate().is_err());
        let mut s = py();
        s.resources.executor_memory = "4 GB".into();
        assert!(s.validate().is_err());
        let mut s = py();
        s.retry.max_attempts = 0;
        assert!(s.validate().is_err());
        let mut s = py();
        s.env.insert("AWS_SECRET_ACCESS_KEY".into(), "x".into());
        assert!(s.validate().is_err());
        let mut s = py();
        s.backend = Backend::Connect;
        s.app = SparkApp::Jar {
            file: "a.jar".into(),
            main_class: "Main".into(),
            jars: vec![],
        };
        assert!(s.validate().is_err());
        s.app = SparkApp::Jar {
            file: "a.jar".into(),
            main_class: "com..Main".into(),
            jars: vec![],
        };
        s.backend = Backend::Kubernetes;
        assert!(s.validate().is_err());
    }

    #[test]
    fn memory() {
        assert!(valid_memory("512m"));
        assert!(valid_memory("2g"));
        assert!(valid_memory("1024"));
        assert!(!valid_memory("2gb"));
        assert!(!valid_memory("g"));
        assert!(!valid_memory("0"));
        assert_eq!(k8s_memory("2g"), "2Gi");
        assert_eq!(k8s_memory("512m"), "512Mi");
        assert_eq!(k8s_memory("1024"), "1024");
    }

    #[test]
    fn backoff() {
        let b = Backoff::Exponential {
            base_secs: 10,
            max_secs: 60,
        };
        assert_eq!(b.delay(1), Duration::from_secs(10));
        assert_eq!(b.delay(2), Duration::from_secs(20));
        assert_eq!(b.delay(3), Duration::from_secs(40));
        assert_eq!(b.delay(4), Duration::from_secs(60));
        assert_eq!(b.delay(40), Duration::from_secs(60));
        assert_eq!(Backoff::Fixed { secs: 5 }.delay(9), Duration::from_secs(5));
    }
}
