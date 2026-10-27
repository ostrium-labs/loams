//! The two Spark Connect backends behind one trait (SP-a D1990; design 81
//! R15).
//!
//! - [`Sail`] (default): LakeSail's Rust Spark Connect server, `sail spark
//!   server`. No JVM in the default deployment (D1262, D211).
//! - [`ApacheSpark`] (opt-in): Apache Spark 4.1.3's own Spark Connect server,
//!   `local[*]` in compose, driver plus executors on Kubernetes.
//!
//! A backend says how its server process starts (image, command,
//! environment, config files, port) from a [`ClusterConf`]; the compose file
//! and [`crate::k8s`] use the same answers. The proxy does not care which
//! one it talks to: both speak Spark Connect.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::conf::{self, ClusterConf, ConfError, pins};

/// Which server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// Sail (default).
    #[default]
    Sail,
    /// Apache Spark's Spark Connect server.
    Spark,
}

impl BackendKind {
    /// The backend.
    pub fn backend(self) -> &'static dyn SparkBackend {
        match self {
            Self::Sail => &Sail,
            Self::Spark => &ApacheSpark,
        }
    }

    /// Parses `sail` or `spark` (an instance's `config["backend"]`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sail" => Some(Self::Sail),
            "spark" => Some(Self::Spark),
            _ => None,
        }
    }
}

/// Where the rendered `spark-defaults.conf` is mounted.
pub const SPARK_CONF_DIR: &str = "/opt/loams/spark-conf";

/// A Spark Connect server implementation.
pub trait SparkBackend: Send + Sync + std::fmt::Debug {
    /// Which one.
    fn kind(&self) -> BackendKind;
    /// The upstream version this crate pins.
    fn version(&self) -> &'static str;
    /// The Loams image name (built by `deploy/release/Containerfile.loams-{sail,spark}`).
    fn image(&self, registry: &str) -> String;
    /// The Spark Connect port.
    fn port(&self) -> u16;
    /// The container command.
    fn command(&self) -> Vec<String>;
    /// The non-secret environment.
    ///
    /// # Errors
    ///
    /// [`ConfError`] from rendering.
    fn env(&self, conf: &ClusterConf) -> Result<BTreeMap<String, String>, ConfError>;
    /// Files to mount: file name in [`SPARK_CONF_DIR`] → contents.
    ///
    /// # Errors
    ///
    /// [`ConfError`] from rendering.
    fn config_files(&self, conf: &ClusterConf) -> Result<BTreeMap<String, String>, ConfError>;
    /// Whether the server checks a pre-shared token
    /// (`SPARK_CONNECT_AUTHENTICATE_TOKEN`) that the proxy must present.
    fn has_token_auth(&self) -> bool;
}

/// Sail.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sail;

impl SparkBackend for Sail {
    fn kind(&self) -> BackendKind {
        BackendKind::Sail
    }
    fn version(&self) -> &'static str {
        pins::SAIL_VERSION
    }
    fn image(&self, registry: &str) -> String {
        format!("{registry}/loams-sail:{}", pins::SAIL_VERSION)
    }
    fn port(&self) -> u16 {
        conf::SAIL_PORT
    }
    fn command(&self) -> Vec<String> {
        let mut cmd: Vec<String> = ["sail", "spark", "server", "--ip", "0.0.0.0", "--port"]
            .map(str::to_owned)
            .to_vec();
        cmd.push(conf::SAIL_PORT.to_string());
        cmd
    }
    fn env(&self, conf: &ClusterConf) -> Result<BTreeMap<String, String>, ConfError> {
        let mut env = conf.sail_env()?;
        env.insert("RUST_LOG".into(), "info".into());
        Ok(env)
    }
    fn config_files(&self, _: &ClusterConf) -> Result<BTreeMap<String, String>, ConfError> {
        Ok(BTreeMap::new())
    }
    fn has_token_auth(&self) -> bool {
        // Sail has no authentication: only the proxy can reach it
        // (NetworkPolicy; compose network with no host port).
        false
    }
}

/// Apache Spark 4's Spark Connect server.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApacheSpark;

impl SparkBackend for ApacheSpark {
    fn kind(&self) -> BackendKind {
        BackendKind::Spark
    }
    fn version(&self) -> &'static str {
        pins::SPARK_VERSION
    }
    fn image(&self, registry: &str) -> String {
        format!("{registry}/loams-spark:{}", pins::SPARK_VERSION)
    }
    fn port(&self) -> u16 {
        conf::SPARK_CONNECT_PORT
    }
    fn command(&self) -> Vec<String> {
        // In the foreground (SPARK_NO_DAEMONIZE), so the container's main
        // process is the server.
        vec!["/opt/spark/sbin/start-connect-server.sh".into()]
    }
    fn env(&self, _: &ClusterConf) -> Result<BTreeMap<String, String>, ConfError> {
        Ok(BTreeMap::from([
            ("SPARK_NO_DAEMONIZE".into(), "1".into()),
            ("SPARK_CONF_DIR".into(), SPARK_CONF_DIR.into()),
            ("SPARK_LOG_DIR".into(), "/tmp/spark-logs".into()),
            ("HOME".into(), "/tmp".into()),
        ]))
    }
    fn config_files(&self, conf: &ClusterConf) -> Result<BTreeMap<String, String>, ConfError> {
        Ok(BTreeMap::from([(
            "spark-defaults.conf".into(),
            conf.spark_defaults()?.to_file(),
        )]))
    }
    fn has_token_auth(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf::{Catalog, ObjectStore, Placement, Shape};

    fn conf() -> ClusterConf {
        ClusterConf {
            namespace: "local".into(),
            instance_id: "eng_local".into(),
            state_prefix: "engines/local/eng_local/".into(),
            store: ObjectStore::default(),
            catalog: Catalog::default(),
            placement: Placement::Local,
            shape: Shape {
                workers: 1,
                worker_vcpu_milli: 1000,
                worker_memory_mib: 4096,
                driver_memory_mib: 2048,
            },
            proxy_idle_timeout_secs: 1800,
            otlp_endpoint: None,
        }
    }

    #[test]
    fn kinds_parse_and_resolve() {
        assert_eq!(BackendKind::parse("sail"), Some(BackendKind::Sail));
        assert_eq!(BackendKind::parse("spark"), Some(BackendKind::Spark));
        assert_eq!(BackendKind::parse("flink"), None);
        assert_eq!(BackendKind::default().backend().kind(), BackendKind::Sail);
        assert_eq!(BackendKind::Spark.backend().port(), 15002);
        assert_eq!(BackendKind::Sail.backend().port(), 50051);
    }

    #[test]
    fn sail_starts_the_spark_connect_server() {
        let cmd = Sail.command();
        assert_eq!(cmd[..3], ["sail", "spark", "server"]);
        assert!(cmd.contains(&"50051".to_owned()));
        let env = Sail.env(&conf()).unwrap();
        assert_eq!(env["SAIL_MODE"], "local");
        assert!(Sail.config_files(&conf()).unwrap().is_empty());
        assert!(!Sail.has_token_auth());
        assert_eq!(
            Sail.image("ghcr.io/ostrium-labs"),
            "ghcr.io/ostrium-labs/loams-sail:0.7.2"
        );
    }

    #[test]
    fn spark_mounts_its_defaults() {
        let files = ApacheSpark.config_files(&conf()).unwrap();
        assert!(files["spark-defaults.conf"].contains("spark.sql.catalog.loams.type rest"));
        let env = ApacheSpark.env(&conf()).unwrap();
        assert_eq!(env["SPARK_NO_DAEMONIZE"], "1");
        assert_eq!(env["SPARK_CONF_DIR"], SPARK_CONF_DIR);
        assert!(ApacheSpark.has_token_auth());
    }
}
