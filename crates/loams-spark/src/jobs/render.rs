//! Job spec → `spark-submit` argv and driver pod spec (D2001).
//!
//! Rendering is pure: a [`SparkJobSpec`] and a [`RenderContext`] give the same
//! output every time (conf is sorted), so the pod spec is unit-tested here and
//! `loams spark render` prints it for `kubectl apply`.
//!
//! **Kubernetes backend.** The driver is a pod running `spark-submit` in
//! *client* mode against the in-cluster API server: Spark's Kubernetes
//! scheduler, inside the driver, creates the executor pods, owned by the
//! driver pod (`spark.kubernetes.driver.pod.name`), so deleting the driver
//! removes them. Executors reach the driver at its pod IP on fixed ports. No
//! Spark Operator is needed (design 26 Q104 stays open for ENG81).
//!
//! **Connect backend.** The driver is a small Python pod (the Loams driver
//! image, with `loams[spark]`) that runs `python -m loams.spark.driver`
//! against the namespace's Spark Connect endpoint; the Spark work happens on
//! the Connect server (Sail), not in this pod.
//!
//! Secrets never appear in the rendered objects: the Loams token and the S3
//! keys come from Kubernetes Secrets through `env`, and Spark conf that needs
//! the token references it as `$(LOAMS_TOKEN)`, which the kubelet expands.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::RunId;
use super::spec::{Backend, SparkApp, SparkJobSpec, k8s_memory};
use crate::catalog::{CatalogConfig, CatalogToken, Flavor};

/// Apache Spark's official image (Spark 4.0.1, released 2025-09-06).
pub const DEFAULT_SPARK_IMAGE: &str = "apache/spark:4.0.1-scala2.13-java17-python3-ubuntu";

/// The Loams Connect-mode driver image (Python with `loams[spark]`), versioned
/// with Loams.
pub const DEFAULT_DRIVER_IMAGE: &str = concat!(
    "ghcr.io/ostrium-labs/loams-spark-driver:",
    env!("CARGO_PKG_VERSION")
);

/// The Spark image's user (`spark`, uid 185).
pub const SPARK_UID: i64 = 185;

/// The driver's RPC port, fixed so executors can reach it.
pub const DRIVER_PORT: u16 = 7078;

/// The driver's block manager port.
pub const BLOCK_MANAGER_PORT: u16 = 7079;

/// The Spark UI port.
pub const UI_PORT: u16 = 4040;

/// Where the SQL runner is mounted in a Kubernetes driver.
pub const SQL_RUNNER_PATH: &str = "/opt/loams/sql/runner.py";

/// The ConfigMap that holds the SQL runner (one per Kubernetes namespace,
/// versioned by name so an upgrade never edits one a running job reads).
pub const SQL_RUNNER_CONFIG_MAP: &str = "loams-spark-sql-runner-v1";

/// The SQL runner: runs each statement of a SQL file with `spark.sql`.
pub const SQL_RUNNER: &str = include_str!("sql_runner.py");

/// The env var a driver reads the Loams token from.
pub const TOKEN_ENV: &str = "LOAMS_TOKEN";

/// The env var that carries the driver pod's IP.
pub const POD_IP_ENV: &str = "LOAMS_POD_IP";

/// A key of a Kubernetes Secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    /// The Secret.
    pub name: String,
    /// The key in it.
    pub key: String,
}

/// Where Spark runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Master {
    /// In-cluster Kubernetes (`k8s://https://kubernetes.default.svc:443`).
    Kubernetes,
    /// `local[<threads>]`, for the local runner.
    Local {
        /// Threads; `None` = all cores (`local[*]`).
        threads: Option<u32>,
    },
}

impl Master {
    fn url(&self) -> String {
        match self {
            Self::Kubernetes => "k8s://https://kubernetes.default.svc:443".to_string(),
            Self::Local { threads: None } => "local[*]".to_string(),
            Self::Local { threads: Some(n) } => format!("local[{n}]"),
        }
    }
}

/// What the renderer needs besides the spec: who, where, with which images
/// and secrets.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// The Loams namespace (tenant) id.
    pub namespace_id: u64,
    /// The run.
    pub run_id: RunId,
    /// The attempt, 1-based.
    pub attempt: u32,
    /// The Kubernetes namespace the namespace's Spark pods go in.
    pub k8s_namespace: String,
    /// The driver's service account (it may create and delete pods in
    /// `k8s_namespace`, nothing else).
    pub service_account: String,
    /// The JVM Spark image (driver and executors).
    pub spark_image: String,
    /// The Connect-mode driver image.
    pub driver_image: String,
    /// The namespace's Spark Connect endpoint, `sc://…` (no token in it).
    pub connect_remote: Option<String>,
    /// The Loams HTTP endpoint (for the SDK in the driver).
    pub loams_endpoint: Option<String>,
    /// The Iceberg catalog.
    pub catalog: Option<CatalogConfig>,
    /// The Secret key holding the Loams token for this job's principal.
    pub token_secret: Option<SecretRef>,
    /// A Secret with `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` for the
    /// job's files (`s3a://` reads of the app), and its endpoint.
    pub s3: Option<S3Access>,
}

/// Object-store access for the app's own files (not table data: tables get
/// vended credentials from the catalog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Access {
    /// A Secret with `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`.
    pub secret: String,
    /// The S3 endpoint (RustFS), `None` for AWS.
    pub endpoint: Option<String>,
    /// Path-style addressing.
    pub path_style: bool,
}

impl RenderContext {
    /// A context with the default images and no catalog or secrets.
    pub fn new(namespace_id: u64, run_id: RunId, attempt: u32) -> Self {
        Self {
            namespace_id,
            run_id,
            attempt,
            k8s_namespace: format!("loams-ns-{namespace_id}"),
            service_account: "loams-spark-driver".to_string(),
            spark_image: DEFAULT_SPARK_IMAGE.to_string(),
            driver_image: DEFAULT_DRIVER_IMAGE.to_string(),
            connect_remote: None,
            loams_endpoint: None,
            catalog: None,
            token_secret: None,
            s3: None,
        }
    }

    /// The driver pod's name: `spark-<job>-<run suffix>-<attempt>`, a DNS
    /// label (at most 6 + 30 + 1 + 10 + 1 + 2 = 50 characters).
    pub fn pod_name(&self, spec: &SparkJobSpec) -> String {
        format!(
            "spark-{}-{}-{}",
            spec.name,
            self.run_id.short(),
            self.attempt
        )
    }

    fn token(&self) -> CatalogToken {
        if self.token_secret.is_some() {
            CatalogToken::Env(TOKEN_ENV.to_string())
        } else {
            CatalogToken::None
        }
    }
}

/// The rendered Kubernetes objects of one attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedJob {
    /// The driver pod.
    pub pod: Value,
    /// The SQL runner's ConfigMap, for SQL jobs on the Kubernetes backend
    /// (create it if missing; it is shared and immutable).
    pub config_map: Option<Value>,
    /// The driver container's argv.
    pub argv: Vec<String>,
}

/// `s3://` → `s3a://` for JVM Spark (Hadoop's S3A file system).
pub fn jvm_uri(file: &str) -> String {
    match file.strip_prefix("s3://") {
        Some(rest) => format!("s3a://{rest}"),
        None => file.to_string(),
    }
}

/// Conf keys whose values are comma lists: the job's entries are appended to
/// Loams' instead of replacing them.
const LIST_CONF: &[&str] = &["spark.jars.packages", "spark.jars", "spark.sql.extensions"];

fn merge_conf(base: &mut BTreeMap<String, String>, extra: &BTreeMap<String, String>) {
    for (k, v) in extra {
        match base.get_mut(k) {
            Some(existing) if LIST_CONF.contains(&k.as_str()) => {
                existing.push(',');
                existing.push_str(v);
            }
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Spark's memory overhead for a container: `max(384 MiB, 10 %)`, added to
/// the heap for the pod's memory request and limit.
pub fn memory_with_overhead_mib(spark_memory: &str) -> u64 {
    let mib = memory_mib(spark_memory);
    mib + (mib / 10).max(384)
}

fn memory_mib(s: &str) -> u64 {
    let (digits, unit) = match s.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => (&s[..s.len() - 1], c.to_ascii_lowercase()),
        _ => (s, 'b'),
    };
    let n: u64 = digits.parse().unwrap_or(0);
    match unit {
        'k' => n.div_ceil(1024),
        'm' => n,
        'g' => n.saturating_mul(1024),
        't' => n.saturating_mul(1024 * 1024),
        _ => n.div_ceil(1024 * 1024),
    }
}

/// The full Spark conf of a JVM run (Kubernetes or local master), sorted.
pub fn spark_conf(
    spec: &SparkJobSpec,
    ctx: &RenderContext,
    master: &Master,
) -> BTreeMap<String, String> {
    let mut conf = BTreeMap::new();
    let mut set = |k: &str, v: String| {
        conf.insert(k.to_string(), v);
    };
    set("spark.app.name", spec.name.clone());
    set("spark.loams.run", ctx.run_id.to_string());
    set("spark.loams.attempt", ctx.attempt.to_string());
    set("spark.loams.namespace", ctx.namespace_id.to_string());
    if *master == Master::Kubernetes {
        let pod = ctx.pod_name(spec);
        let sa = "/var/run/secrets/kubernetes.io/serviceaccount";
        set("spark.kubernetes.namespace", ctx.k8s_namespace.clone());
        set("spark.kubernetes.container.image", ctx.spark_image.clone());
        set("spark.kubernetes.driver.pod.name", pod.clone());
        set("spark.kubernetes.executor.podNamePrefix", pod);
        set(
            "spark.kubernetes.authenticate.caCertFile",
            format!("{sa}/ca.crt"),
        );
        set(
            "spark.kubernetes.authenticate.oauthTokenFile",
            format!("{sa}/token"),
        );
        set(
            "spark.kubernetes.authenticate.executor.serviceAccountName",
            "default".to_string(),
        );
        set("spark.driver.host", format!("$({POD_IP_ENV})"));
        set("spark.driver.bindAddress", "0.0.0.0".to_string());
        set("spark.driver.port", DRIVER_PORT.to_string());
        set(
            "spark.driver.blockManager.port",
            BLOCK_MANAGER_PORT.to_string(),
        );
        set(
            "spark.executor.instances",
            spec.resources.executors.to_string(),
        );
        set(
            "spark.kubernetes.executor.request.cores",
            spec.resources.executor_cores.to_string(),
        );
        for (k, v) in labels(spec, ctx) {
            set(&format!("spark.kubernetes.executor.label.{k}"), v);
        }
        set(
            "spark.kubernetes.executor.deleteOnTermination",
            "true".to_string(),
        );
    }
    set(
        "spark.executor.cores",
        spec.resources.executor_cores.to_string(),
    );
    set(
        "spark.executor.memory",
        spec.resources.executor_memory.clone(),
    );
    if let Some(s3) = &ctx.s3 {
        if let Some(endpoint) = &s3.endpoint {
            set("spark.hadoop.fs.s3a.endpoint", endpoint.clone());
        }
        if s3.path_style {
            set("spark.hadoop.fs.s3a.path.style.access", "true".to_string());
        }
    }
    if let Some(catalog) = &ctx.catalog {
        merge_conf(&mut conf, &catalog.spark_conf(Flavor::Jvm, &ctx.token()));
    }
    merge_conf(&mut conf, &spec.conf);
    conf
}

/// The `spark-submit` argv of a JVM run. `spark_submit` is the binary
/// (`/opt/spark/bin/spark-submit` in the image, `$SPARK_HOME/bin/spark-submit`
/// locally) and `sql_runner` the path of [`SQL_RUNNER`] for SQL jobs.
pub fn spark_submit_argv(
    spec: &SparkJobSpec,
    ctx: &RenderContext,
    master: &Master,
    spark_submit: &str,
    sql_runner: &str,
) -> Vec<String> {
    let mut argv = vec![
        spark_submit.to_string(),
        "--master".to_string(),
        master.url(),
        "--deploy-mode".to_string(),
        "client".to_string(),
        "--name".to_string(),
        spec.name.clone(),
        "--driver-memory".to_string(),
        spec.resources.driver_memory.clone(),
    ];
    for (k, v) in spark_conf(spec, ctx, master) {
        argv.push("--conf".to_string());
        argv.push(format!("{k}={v}"));
    }
    match &spec.app {
        SparkApp::Jar {
            file,
            main_class,
            jars,
        } => {
            argv.push("--class".to_string());
            argv.push(main_class.clone());
            if !jars.is_empty() {
                argv.push("--jars".to_string());
                argv.push(
                    jars.iter()
                        .map(|j| jvm_uri(j))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            argv.push(jvm_uri(file));
        }
        SparkApp::Pyspark { file, py_files } => {
            if !py_files.is_empty() {
                argv.push("--py-files".to_string());
                argv.push(
                    py_files
                        .iter()
                        .map(|f| jvm_uri(f))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            argv.push(jvm_uri(file));
        }
        SparkApp::Sql { file } => {
            argv.push(sql_runner.to_string());
            argv.push(jvm_uri(file));
        }
    }
    argv.extend(spec.args.iter().cloned());
    argv
}

/// The argv of a Connect-mode driver: `python -m loams.spark.driver`.
pub fn connect_driver_argv(spec: &SparkJobSpec, python: &str) -> Vec<String> {
    let mut argv = vec![
        python.to_string(),
        "-m".to_string(),
        "loams.spark.driver".to_string(),
        "--app".to_string(),
        spec.app.file().to_string(),
    ];
    match &spec.app {
        SparkApp::Sql { .. } => argv.push("--sql".to_string()),
        SparkApp::Pyspark { py_files, .. } => {
            for f in py_files {
                argv.push("--py-file".to_string());
                argv.push(f.clone());
            }
        }
        SparkApp::Jar { .. } => {}
    }
    argv.push("--".to_string());
    argv.extend(spec.args.iter().cloned());
    argv
}

/// The environment of a Connect-mode driver (besides the token and the
/// user's own `env`): the endpoint, the catalog and the job's conf, which the
/// SDK reads in `loams.spark.session()`.
pub fn connect_driver_env(spec: &SparkJobSpec, ctx: &RenderContext) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if let Some(remote) = &ctx.connect_remote {
        env.insert("LOAMS_SPARK_REMOTE".to_string(), remote.clone());
    }
    if let Some(endpoint) = &ctx.loams_endpoint {
        env.insert("LOAMS_ENDPOINT".to_string(), endpoint.clone());
    }
    env.insert("LOAMS_NAMESPACE".to_string(), ctx.namespace_id.to_string());
    if let Some(c) = &ctx.catalog {
        env.insert("LOAMS_ICEBERG_URI".to_string(), c.uri.to_string());
        env.insert("LOAMS_ICEBERG_WAREHOUSE".to_string(), c.warehouse.clone());
    }
    let mut conf = spec.conf.clone();
    conf.insert("spark.app.name".to_string(), spec.name.clone());
    if let Ok(json) = serde_json::to_string(&conf) {
        env.insert("LOAMS_SPARK_CONF".to_string(), json);
    }
    env.insert("LOAMS_SPARK_RUN".to_string(), ctx.run_id.to_string());
    env.insert("LOAMS_SPARK_ATTEMPT".to_string(), ctx.attempt.to_string());
    env
}

fn labels(spec: &SparkJobSpec, ctx: &RenderContext) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "app.kubernetes.io/name".to_string(),
            "loams-spark".to_string(),
        ),
        (
            "app.kubernetes.io/managed-by".to_string(),
            "loams".to_string(),
        ),
        (
            "loams.dev/namespace".to_string(),
            ctx.namespace_id.to_string(),
        ),
        ("loams.dev/spark-job".to_string(), spec.name.clone()),
        ("loams.dev/spark-run".to_string(), ctx.run_id.to_string()),
        (
            "loams.dev/spark-attempt".to_string(),
            ctx.attempt.to_string(),
        ),
    ])
}

fn env_list(plain: &BTreeMap<String, String>) -> Vec<Value> {
    plain
        .iter()
        .map(|(k, v)| json!({ "name": k, "value": v }))
        .collect()
}

fn container_security() -> Value {
    json!({
        "allowPrivilegeEscalation": false,
        "capabilities": { "drop": ["ALL"] },
        "runAsNonRoot": true,
        "runAsUser": SPARK_UID,
        "seccompProfile": { "type": "RuntimeDefault" }
    })
}

/// The SQL runner's ConfigMap.
pub fn sql_runner_config_map(k8s_namespace: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {
            "name": SQL_RUNNER_CONFIG_MAP,
            "namespace": k8s_namespace,
            "labels": {
                "app.kubernetes.io/name": "loams-spark",
                "app.kubernetes.io/managed-by": "loams"
            }
        },
        "immutable": true,
        "data": { "runner.py": SQL_RUNNER }
    })
}

/// Renders one attempt of `spec` as Kubernetes objects. The spec must be
/// valid ([`SparkJobSpec::validate`]); the backend is resolved here, and
/// `local` renders as `kubernetes` (a local job has no pod).
pub fn render_pod(spec: &SparkJobSpec, ctx: &RenderContext) -> RenderedJob {
    let backend = spec.backend.resolve(&spec.app);
    let name = ctx.pod_name(spec);
    let labels = labels(spec, ctx);
    let r = &spec.resources;

    let mut env: Vec<Value> = Vec::new();
    if let Some(secret) = &ctx.token_secret {
        env.push(json!({
            "name": TOKEN_ENV,
            "valueFrom": { "secretKeyRef": { "name": secret.name, "key": secret.key } }
        }));
    }
    let mut env_from: Vec<Value> = Vec::new();
    if let Some(s3) = &ctx.s3 {
        env_from.push(json!({ "secretRef": { "name": s3.secret } }));
    }

    let (image, argv, ports, automount, mut volumes, mut mounts, config_map, memory_mib, cpu) =
        if backend == Backend::Connect {
            env.extend(env_list(&connect_driver_env(spec, ctx)));
            (
                ctx.driver_image.clone(),
                connect_driver_argv(spec, "python"),
                Vec::new(),
                false,
                Vec::new(),
                Vec::new(),
                None,
                // The Connect driver only submits plans; the work is on the
                // server. Half a GiB is plenty for the Python client.
                512,
                "500m".to_string(),
            )
        } else {
            env.push(json!({
                "name": POD_IP_ENV,
                "valueFrom": { "fieldRef": { "fieldPath": "status.podIP" } }
            }));
            let argv = spark_submit_argv(
                spec,
                ctx,
                &Master::Kubernetes,
                "/opt/spark/bin/spark-submit",
                SQL_RUNNER_PATH,
            );
            let (volumes, mounts, cm) = if matches!(spec.app, SparkApp::Sql { .. }) {
                (
                    vec![json!({
                        "name": "sql-runner",
                        "configMap": { "name": SQL_RUNNER_CONFIG_MAP }
                    })],
                    vec![json!({
                        "name": "sql-runner",
                        "mountPath": "/opt/loams/sql",
                        "readOnly": true
                    })],
                    Some(sql_runner_config_map(&ctx.k8s_namespace)),
                )
            } else {
                (Vec::new(), Vec::new(), None)
            };
            (
                ctx.spark_image.clone(),
                argv,
                vec![
                    json!({ "name": "driver-rpc", "containerPort": DRIVER_PORT }),
                    json!({ "name": "blockmanager", "containerPort": BLOCK_MANAGER_PORT }),
                    json!({ "name": "spark-ui", "containerPort": UI_PORT }),
                ],
                true,
                volumes,
                mounts,
                cm,
                memory_with_overhead_mib(&r.driver_memory),
                r.driver_cores.to_string(),
            )
        };
    env.extend(env_list(&spec.env));
    volumes.push(json!({ "name": "tmp", "emptyDir": {} }));
    mounts.push(json!({ "name": "tmp", "mountPath": "/tmp" }));

    let memory = k8s_memory(&format!("{memory_mib}m"));
    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "serviceAccountName": ctx.service_account,
        "automountServiceAccountToken": automount,
        "enableServiceLinks": false,
        "securityContext": {
            "runAsNonRoot": true,
            "runAsUser": SPARK_UID,
            "fsGroup": SPARK_UID,
            "seccompProfile": { "type": "RuntimeDefault" }
        },
        "containers": [{
            "name": "driver",
            "image": image,
            "imagePullPolicy": "IfNotPresent",
            "args": argv,
            "env": env,
            "envFrom": env_from,
            "ports": ports,
            "resources": {
                "requests": { "cpu": cpu, "memory": memory },
                "limits": { "memory": memory }
            },
            "securityContext": container_security(),
            "volumeMounts": mounts
        }],
        "volumes": volumes
    });
    if let Some(timeout) = spec.timeout_secs {
        pod_spec["activeDeadlineSeconds"] = json!(timeout);
    }
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": name,
            "namespace": ctx.k8s_namespace,
            "labels": labels,
            "annotations": {
                "loams.dev/spark-backend": backend.as_str(),
                "loams.dev/spark-app-kind": spec.app.kind()
            }
        },
        "spec": pod_spec
    });
    RenderedJob {
        pod,
        config_map,
        argv,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::spec::Resources;
    use url::Url;

    fn run() -> RunId {
        RunId("01jbzq3w4y5x6v7t8s9r0pqmnk".into())
    }

    fn ctx() -> RenderContext {
        let mut c = RenderContext::new(42, run(), 1);
        c.catalog = Some(
            CatalogConfig::new(
                Url::parse("http://lakekeeper:8181/catalog").expect("url"),
                "ns-42",
            )
            .with_s3_endpoint("http://rustfs:9000"),
        );
        c.token_secret = Some(SecretRef {
            name: "loams-spark-token".into(),
            key: "token".into(),
        });
        c.s3 = Some(S3Access {
            secret: "loams-spark-s3".into(),
            endpoint: Some("http://rustfs:9000".into()),
            path_style: true,
        });
        c.connect_remote = Some("sc://ns-42.spark.loams.internal:443/;use_ssl=true".into());
        c
    }

    fn jar() -> SparkJobSpec {
        let mut s = SparkJobSpec::new(
            "rollup",
            SparkApp::Jar {
                file: "s3://jobs/rollup.jar".into(),
                main_class: "dev.loams.Rollup".into(),
                jars: vec!["s3://jobs/dep.jar".into()],
            },
        );
        s.args = vec!["--day".into(), "2026-10-27".into()];
        s.resources = Resources {
            driver_cores: 2,
            driver_memory: "2g".into(),
            executor_cores: 4,
            executor_memory: "8g".into(),
            executors: 3,
        };
        s.conf
            .insert("spark.jars.packages".into(), "org.example:x:1.0".into());
        s.timeout_secs = Some(3600);
        s
    }

    fn args_of(pod: &Value) -> Vec<String> {
        pod["spec"]["containers"][0]["args"]
            .as_array()
            .expect("args")
            .iter()
            .map(|a| a.as_str().expect("string").to_string())
            .collect()
    }

    fn conf_of(argv: &[String]) -> BTreeMap<String, String> {
        argv.windows(2)
            .filter(|w| w[0] == "--conf")
            .filter_map(|w| w[1].split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn jar_renders_a_client_mode_driver_pod() {
        let spec = jar();
        spec.validate().expect("valid");
        let out = render_pod(&spec, &ctx());
        let pod = &out.pod;
        assert_eq!(pod["kind"], "Pod");
        assert_eq!(pod["metadata"]["name"], "spark-rollup-8s9r0pqmnk-1");
        assert_eq!(pod["metadata"]["namespace"], "loams-ns-42");
        assert_eq!(pod["metadata"]["labels"]["loams.dev/spark-run"], run().0);
        assert_eq!(
            pod["metadata"]["annotations"]["loams.dev/spark-backend"],
            "kubernetes"
        );
        let spec_json = &pod["spec"];
        assert_eq!(spec_json["restartPolicy"], "Never");
        assert_eq!(spec_json["automountServiceAccountToken"], true);
        assert_eq!(spec_json["activeDeadlineSeconds"], 3600);
        let c = &spec_json["containers"][0];
        assert_eq!(c["image"], DEFAULT_SPARK_IMAGE);
        assert_eq!(c["resources"]["requests"]["cpu"], "2");
        // 2 GiB heap + max(384 MiB, 10 %) = 2432 MiB.
        assert_eq!(c["resources"]["limits"]["memory"], "2432Mi");
        assert_eq!(c["securityContext"]["allowPrivilegeEscalation"], false);

        let argv = args_of(pod);
        assert_eq!(argv, out.argv);
        assert_eq!(argv[0], "/opt/spark/bin/spark-submit");
        assert_eq!(
            &argv[1..5],
            &[
                "--master",
                "k8s://https://kubernetes.default.svc:443",
                "--deploy-mode",
                "client"
            ]
        );
        let class_at = argv.iter().position(|a| a == "--class").expect("--class");
        assert_eq!(argv[class_at + 1], "dev.loams.Rollup");
        assert_eq!(argv[class_at + 2], "--jars");
        assert_eq!(argv[class_at + 3], "s3a://jobs/dep.jar");
        assert_eq!(argv[class_at + 4], "s3a://jobs/rollup.jar");
        assert_eq!(&argv[class_at + 5..], &["--day", "2026-10-27"]);

        let conf = conf_of(&argv);
        assert_eq!(conf["spark.kubernetes.namespace"], "loams-ns-42");
        assert_eq!(
            conf["spark.kubernetes.driver.pod.name"],
            "spark-rollup-8s9r0pqmnk-1"
        );
        assert_eq!(conf["spark.driver.host"], "$(LOAMS_POD_IP)");
        assert_eq!(conf["spark.executor.instances"], "3");
        assert_eq!(conf["spark.executor.memory"], "8g");
        assert_eq!(
            conf["spark.kubernetes.executor.label.loams.dev/spark-run"],
            run().0
        );
        assert_eq!(conf["spark.hadoop.fs.s3a.endpoint"], "http://rustfs:9000");
        assert_eq!(conf["spark.sql.catalog.loams.type"], "rest");
        assert_eq!(conf["spark.sql.catalog.loams.token"], "$(LOAMS_TOKEN)");
        assert_eq!(
            conf["spark.sql.catalog.loams.header.X-Iceberg-Access-Delegation"],
            "vended-credentials"
        );
        let packages = &conf["spark.jars.packages"];
        assert!(packages.starts_with("org.apache.iceberg:iceberg-spark-runtime-4.0_2.13:1.10.0,"));
        assert!(packages.ends_with(",org.example:x:1.0"), "{packages}");
        assert!(out.config_map.is_none());
    }

    #[test]
    fn secrets_come_from_secret_refs_only() {
        let out = render_pod(&jar(), &ctx());
        let text = out.pod.to_string();
        let env = out.pod["spec"]["containers"][0]["env"]
            .as_array()
            .expect("env");
        let token = env
            .iter()
            .find(|e| e["name"] == TOKEN_ENV)
            .expect("token env");
        assert_eq!(
            token["valueFrom"]["secretKeyRef"]["name"],
            "loams-spark-token"
        );
        assert!(token.get("value").is_none());
        let ip = env
            .iter()
            .find(|e| e["name"] == POD_IP_ENV)
            .expect("pod ip");
        assert_eq!(ip["valueFrom"]["fieldRef"]["fieldPath"], "status.podIP");
        assert_eq!(
            out.pod["spec"]["containers"][0]["envFrom"][0]["secretRef"]["name"],
            "loams-spark-s3"
        );
        assert!(!text.contains("AWS_SECRET_ACCESS_KEY\",\"value"));
    }

    #[test]
    fn sql_on_kubernetes_mounts_the_runner() {
        let mut spec = SparkJobSpec::new(
            "daily-sql",
            SparkApp::Sql {
                file: "s3://jobs/daily.sql".into(),
            },
        );
        spec.backend = Backend::Kubernetes;
        spec.validate().expect("valid");
        let out = render_pod(&spec, &ctx());
        let argv = args_of(&out.pod);
        let n = argv.len();
        assert_eq!(&argv[n - 2..], &[SQL_RUNNER_PATH, "s3a://jobs/daily.sql"]);
        let cm = out.config_map.expect("config map");
        assert_eq!(cm["metadata"]["name"], SQL_RUNNER_CONFIG_MAP);
        assert_eq!(cm["immutable"], true);
        assert!(
            cm["data"]["runner.py"]
                .as_str()
                .expect("runner")
                .contains("spark.sql")
        );
        let mounts = &out.pod["spec"]["containers"][0]["volumeMounts"];
        assert_eq!(mounts[0]["mountPath"], "/opt/loams/sql");
        assert_eq!(
            out.pod["spec"]["volumes"][0]["configMap"]["name"],
            SQL_RUNNER_CONFIG_MAP
        );
    }

    #[test]
    fn pyspark_defaults_to_a_connect_driver() {
        let mut spec = SparkJobSpec::new(
            "etl",
            SparkApp::Pyspark {
                file: "s3://jobs/etl.py".into(),
                py_files: vec!["s3://jobs/lib.zip".into()],
            },
        );
        spec.args = vec!["2026-10-27".into()];
        spec.conf
            .insert("spark.sql.shuffle.partitions".into(), "8".into());
        spec.env.insert("MODE".into(), "full".into());
        let out = render_pod(&spec, &ctx());
        let pod = &out.pod;
        assert_eq!(
            pod["metadata"]["annotations"]["loams.dev/spark-backend"],
            "connect"
        );
        assert_eq!(pod["spec"]["automountServiceAccountToken"], false);
        let c = &pod["spec"]["containers"][0];
        assert_eq!(c["image"], DEFAULT_DRIVER_IMAGE);
        assert_eq!(c["ports"].as_array().map(Vec::len), Some(0));
        assert_eq!(
            args_of(pod),
            vec![
                "python",
                "-m",
                "loams.spark.driver",
                "--app",
                "s3://jobs/etl.py",
                "--py-file",
                "s3://jobs/lib.zip",
                "--",
                "2026-10-27"
            ]
        );
        let env: BTreeMap<String, String> = c["env"]
            .as_array()
            .expect("env")
            .iter()
            .filter_map(|e| {
                Some((
                    e["name"].as_str()?.to_string(),
                    e["value"].as_str()?.to_string(),
                ))
            })
            .collect();
        assert_eq!(
            env["LOAMS_SPARK_REMOTE"],
            "sc://ns-42.spark.loams.internal:443/;use_ssl=true"
        );
        assert!(!env["LOAMS_SPARK_REMOTE"].contains("token"));
        assert_eq!(env["LOAMS_ICEBERG_WAREHOUSE"], "ns-42");
        assert_eq!(env["MODE"], "full");
        let conf: BTreeMap<String, String> =
            serde_json::from_str(&env["LOAMS_SPARK_CONF"]).expect("conf json");
        assert_eq!(conf["spark.sql.shuffle.partitions"], "8");
        assert_eq!(conf["spark.app.name"], "etl");
    }

    #[test]
    fn local_master_argv_has_no_kubernetes_conf() {
        let mut spec = jar();
        spec.backend = Backend::Local;
        let mut c = ctx();
        c.catalog = None;
        let argv = spark_submit_argv(
            &spec,
            &c,
            &Master::Local { threads: Some(2) },
            "/opt/spark/bin/spark-submit",
            "/tmp/runner.py",
        );
        assert_eq!(argv[2], "local[2]");
        let conf = conf_of(&argv);
        assert!(
            conf.keys().all(|k| !k.starts_with("spark.kubernetes.")),
            "{conf:?}"
        );
        assert!(!conf.contains_key("spark.driver.host"));
        assert_eq!(conf["spark.jars.packages"], "org.example:x:1.0");
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(render_pod(&jar(), &ctx()), render_pod(&jar(), &ctx()));
    }

    #[test]
    fn memory_overhead() {
        assert_eq!(memory_with_overhead_mib("1g"), 1024 + 384);
        assert_eq!(memory_with_overhead_mib("8g"), 8192 + 819);
        assert_eq!(memory_with_overhead_mib("512m"), 512 + 384);
    }

    #[test]
    fn pod_names_are_dns_labels() {
        let mut spec = jar();
        spec.name = "a".repeat(crate::jobs::spec::MAX_NAME_LEN);
        let mut c = ctx();
        c.attempt = 10;
        let name = c.pod_name(&spec);
        assert!(name.len() <= 63, "{name}");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
    }
}
