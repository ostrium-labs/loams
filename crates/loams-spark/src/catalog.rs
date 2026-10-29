//! The Iceberg catalog Spark jobs and sessions use (design 81 SP-b, D2005).
//!
//! Every Spark job and session gets one catalog, named `loams` by default: the
//! namespace's Iceberg REST catalog at Lakekeeper, with **credential vending**
//! (`X-Iceberg-Access-Delegation: vended-credentials`), so Spark never holds
//! long-lived object-store keys for table data. Lakekeeper hands the engine
//! short-lived credentials scoped to the table's location when it loads it
//! (design 17 §6.1, the M4 path).
//!
//! **House external tables.** The House maps a namespace's ClickHouse database
//! `d` onto the Iceberg namespace `ns_<id>.d` (design 49 §5.3), and lists and
//! reads tables another writer made there as *external* tables (§49 §5.2). So a
//! table Spark writes at `loams.ns_<id>.<d>.<t>` is queryable in the House as
//! `<d>.<t>` with no registration step: [`house_table`] builds that name and
//! [`HouseRef`] the House side of it. Spark must never write the keyed tables
//! Loams' links maintain (one writer class, §17 §6.1); those are not under
//! `ns_<id>.<d>` names a job is given.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use url::Url;

/// The default Spark catalog name.
pub const DEFAULT_CATALOG: &str = "loams";

/// The header that asks an Iceberg REST catalog to vend credentials.
pub const ACCESS_DELEGATION_HEADER: &str = "X-Iceberg-Access-Delegation";

/// Its value for vended credentials (Iceberg REST spec).
pub const VENDED_CREDENTIALS: &str = "vended-credentials";

/// Iceberg's version for the JVM Spark runtime jars (Spark 4.1, Scala 2.13):
/// ENG81 ruling R27 (D1931), the same Iceberg as Flink's.
pub const ICEBERG_VERSION: &str = "1.12.0";

/// Hadoop's S3A for `s3a://` app files: Spark 4.1.3's Hadoop (R27).
pub const HADOOP_AWS_VERSION: &str = "3.4.2";

/// The Lakekeeper warehouse every engine uses (ENG81 R30), on the bucket
/// `loams-lake`.
pub const DEFAULT_WAREHOUSE: &str = "loams";

/// Where a catalog configuration is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// A Spark Connect session (Sail or Spark's Connect server): only the
    /// catalog keys; the server has the Iceberg runtime already.
    Connect,
    /// JVM Spark through `spark-submit`: also the Iceberg runtime packages, the
    /// SQL extensions and the S3 file IO.
    Jvm,
}

/// How the Loams token reaches the catalog conf.
#[derive(Clone, PartialEq, Eq)]
pub enum CatalogToken {
    /// No token: a loopback Lakekeeper in `loams dev` that trusts its network.
    None,
    /// A literal token. Never rendered into a pod spec (it would be stored in
    /// the API server); used only for in-process Connect sessions.
    Literal(String),
    /// An environment variable reference, `$(NAME)`. Kept for conf that is
    /// written to a file at start from the env; ENG81 R5 forbids putting a
    /// secret in argv, so the renderer never passes this on the command line
    /// (see `jobs::render::SECRET_PROPERTIES`).
    Env(String),
}

impl fmt::Debug for CatalogToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Literal(_) => f.write_str("Literal(<redacted>)"),
            Self::Env(name) => f.debug_tuple("Env").field(name).finish(),
        }
    }
}

/// The catalog a job or session is wired to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogConfig {
    /// The Spark catalog name (`loams`).
    pub name: String,
    /// Lakekeeper's Iceberg REST base, such as `http://lakekeeper:8181/catalog`.
    pub uri: Url,
    /// The Lakekeeper warehouse of the namespace.
    pub warehouse: String,
    /// The S3 endpoint for the vended credentials (RustFS in compose); `None`
    /// on AWS.
    pub s3_endpoint: Option<String>,
    /// Path-style S3 addressing (RustFS, MinIO).
    pub path_style: bool,
    /// Ask for vended credentials. On by default; off only for a dev catalog
    /// whose bucket the engine already reaches.
    pub vended_credentials: bool,
    /// Make it the session's default catalog (`spark.sql.defaultCatalog`).
    pub default_catalog: bool,
}

impl CatalogConfig {
    /// A catalog named `loams`, vended credentials on, default catalog.
    pub fn new(uri: Url, warehouse: impl Into<String>) -> Self {
        Self {
            name: DEFAULT_CATALOG.to_string(),
            uri,
            warehouse: warehouse.into(),
            s3_endpoint: None,
            path_style: false,
            vended_credentials: true,
            default_catalog: true,
        }
    }

    /// With an S3-compatible endpoint, path style (RustFS in compose).
    #[must_use]
    pub fn with_s3_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.s3_endpoint = Some(endpoint.into());
        self.path_style = true;
        self
    }

    /// The Spark conf for this catalog. Keys are sorted, so a rendered spec
    /// is stable.
    pub fn spark_conf(&self, flavor: Flavor, token: &CatalogToken) -> BTreeMap<String, String> {
        let p = format!("spark.sql.catalog.{}", self.name);
        let mut conf = BTreeMap::new();
        conf.insert(
            p.clone(),
            "org.apache.iceberg.spark.SparkCatalog".to_string(),
        );
        conf.insert(format!("{p}.type"), "rest".to_string());
        conf.insert(format!("{p}.uri"), self.uri.to_string());
        conf.insert(format!("{p}.warehouse"), self.warehouse.clone());
        if self.vended_credentials {
            conf.insert(
                format!("{p}.header.{ACCESS_DELEGATION_HEADER}"),
                VENDED_CREDENTIALS.to_string(),
            );
        }
        match token {
            CatalogToken::None => {}
            CatalogToken::Literal(t) => {
                conf.insert(format!("{p}.token"), t.clone());
            }
            CatalogToken::Env(name) => {
                conf.insert(format!("{p}.token"), format!("$({name})"));
            }
        }
        if let Some(endpoint) = &self.s3_endpoint {
            conf.insert(format!("{p}.s3.endpoint"), endpoint.clone());
        }
        if self.path_style {
            conf.insert(format!("{p}.s3.path-style-access"), "true".to_string());
        }
        if self.default_catalog {
            conf.insert("spark.sql.defaultCatalog".to_string(), self.name.clone());
        }
        if flavor == Flavor::Jvm {
            conf.insert(
                format!("{p}.io-impl"),
                "org.apache.iceberg.aws.s3.S3FileIO".to_string(),
            );
            conf.insert(
                "spark.sql.extensions".to_string(),
                "org.apache.iceberg.spark.extensions.IcebergSparkSessionExtensions".to_string(),
            );
            conf.insert("spark.jars.packages".to_string(), iceberg_packages());
        }
        conf
    }
}

/// The Maven coordinates JVM Spark 4.1 needs: the Iceberg runtime, its AWS
/// bundle (S3FileIO for vended credentials) and Hadoop's S3A.
pub fn iceberg_packages() -> String {
    format!(
        "org.apache.iceberg:iceberg-spark-runtime-4.1_2.13:{ICEBERG_VERSION},\
         org.apache.iceberg:iceberg-aws-bundle:{ICEBERG_VERSION},\
         org.apache.hadoop:hadoop-aws:{HADOOP_AWS_VERSION}"
    )
}

/// The Iceberg namespace levels of a House database: `ns_<id>`, then the
/// database (`default` is the root, as in §49 §5.3).
pub fn house_namespace(namespace_id: u64, database: &str) -> Vec<String> {
    let root = format!("ns_{namespace_id}");
    if database == "default" {
        vec![root]
    } else {
        vec![root, database.to_string()]
    }
}

/// The Spark name of a table the House reads as `<database>.<table>`:
/// `loams.ns_<id>.<database>.<table>`, each part quoted with backticks when it
/// is not a plain identifier.
pub fn house_table(catalog: &str, namespace_id: u64, database: &str, table: &str) -> String {
    let mut parts = vec![quote_ident(catalog)];
    parts.extend(
        house_namespace(namespace_id, database)
            .iter()
            .map(|p| quote_ident(p)),
    );
    parts.push(quote_ident(table));
    parts.join(".")
}

/// A Spark SQL identifier, backtick-quoted unless it is `[A-Za-z_][A-Za-z0-9_]*`.
pub fn quote_ident(ident: &str) -> String {
    let plain = ident
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        ident.to_string()
    } else {
        format!("`{}`", ident.replace('`', "``"))
    }
}

/// The House side of a table Spark wrote: what to query it as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HouseRef {
    /// The ClickHouse database.
    pub database: String,
    /// The table.
    pub table: String,
}

impl HouseRef {
    /// The ClickHouse name, `` `db`.`t` ``.
    pub fn clickhouse_name(&self) -> String {
        format!(
            "`{}`.`{}`",
            self.database.replace('`', "\\`"),
            self.table.replace('`', "\\`")
        )
    }

    /// A first query to check the table is visible in the House.
    pub fn count_sql(&self) -> String {
        format!("SELECT count() FROM {}", self.clickhouse_name())
    }
}

/// A REST path segment for a namespace: levels joined by the unit separator
/// (0x1F), percent-encoded (Iceberg REST spec).
pub fn rest_namespace_path(levels: &[String]) -> String {
    levels
        .iter()
        .map(|l| encode_segment(l))
        .collect::<Vec<_>>()
        .join("%1F")
}

fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b));
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Credentials Lakekeeper vended for one table.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct VendedCredentials {
    /// The storage prefix they are scoped to (`storage-credentials[].prefix`),
    /// or `None` when they came in the table's `config`.
    pub prefix: Option<String>,
    /// The raw properties (`s3.access-key-id`, `s3.secret-access-key`,
    /// `s3.session-token`, `client.region`, …).
    pub config: BTreeMap<String, String>,
}

impl fmt::Debug for VendedCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<&String> = self.config.keys().collect();
        f.debug_struct("VendedCredentials")
            .field("prefix", &self.prefix)
            .field("keys", &keys)
            .finish()
    }
}

impl VendedCredentials {
    /// Whether an S3 session was vended (an access key and a session token).
    pub fn has_s3_session(&self) -> bool {
        self.config.contains_key("s3.access-key-id") && self.config.contains_key("s3.session-token")
    }
}

#[derive(Debug, Deserialize)]
struct LoadTableResponse {
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default, rename = "storage-credentials")]
    storage_credentials: Vec<StorageCredential>,
}

#[derive(Debug, Deserialize)]
struct StorageCredential {
    prefix: String,
    #[serde(default)]
    config: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ConfigResponse {
    #[serde(default)]
    defaults: BTreeMap<String, String>,
    #[serde(default)]
    overrides: BTreeMap<String, String>,
}

/// Parses a `loadTable` answer into the vended credentials: the
/// `storage-credentials` entry with the longest prefix of `location` when the
/// catalog sent any (Iceberg 1.7+), else the `s3.*` keys of `config`.
pub fn vended_from_load_table(
    body: &[u8],
    location: Option<&str>,
) -> Result<VendedCredentials, CatalogError> {
    let parsed: LoadTableResponse = serde_json::from_slice(body)
        .map_err(|e| CatalogError::Protocol(format!("loadTable answer: {e}")))?;
    let best = parsed
        .storage_credentials
        .into_iter()
        .filter(|c| location.is_none_or(|l| l.starts_with(&c.prefix)))
        .max_by_key(|c| c.prefix.len());
    if let Some(c) = best {
        return Ok(VendedCredentials {
            prefix: Some(c.prefix),
            config: c.config,
        });
    }
    let config = parsed
        .config
        .into_iter()
        .filter(|(k, _)| k.starts_with("s3.") || k.starts_with("client."))
        .collect();
    Ok(VendedCredentials {
        prefix: None,
        config,
    })
}

/// A catalog call failed.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// Lakekeeper could not be reached.
    #[error("the catalog is unavailable: {0}")]
    Unavailable(String),
    /// Lakekeeper answered with an error status.
    #[error("the catalog answered {status}: {message}")]
    Status {
        /// The HTTP status.
        status: u16,
        /// The body, shortened.
        message: String,
    },
    /// The answer was not what the Iceberg REST spec says.
    #[error("{0}")]
    Protocol(String),
}

/// A minimal Iceberg REST client for what jobs need from Lakekeeper: the
/// warehouse prefix and a table's vended credentials (`loams spark catalog
/// check`, and the runner's preflight before it starts a driver).
#[derive(Debug, Clone)]
pub struct LakekeeperClient {
    http: reqwest::Client,
    config: CatalogConfig,
    token: Option<String>,
}

impl LakekeeperClient {
    /// A client for `config`, authenticating with `token` when given.
    pub fn new(config: CatalogConfig, token: Option<String>) -> Result<Self, CatalogError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("loams-spark/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CatalogError::Unavailable(e.to_string()))?;
        Ok(Self {
            http,
            config,
            token,
        })
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.config.uri.as_str().trim_end_matches('/'),
            path
        )
    }

    async fn get(&self, url: String, vend: bool) -> Result<Vec<u8>, CatalogError> {
        let mut req = self.http.get(url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        if vend && self.config.vended_credentials {
            req = req.header(ACCESS_DELEGATION_HEADER, VENDED_CREDENTIALS);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| CatalogError::Unavailable(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| CatalogError::Unavailable(e.to_string()))?;
        if !status.is_success() {
            let mut message = String::from_utf8_lossy(&body).into_owned();
            message.truncate(400);
            return Err(CatalogError::Status {
                status: status.as_u16(),
                message,
            });
        }
        Ok(body.to_vec())
    }

    /// The warehouse's REST prefix (`GET /v1/config?warehouse=…`,
    /// `overrides.prefix`, else `defaults.prefix`, else none).
    pub async fn prefix(&self) -> Result<Option<String>, CatalogError> {
        let mut url = Url::parse(&self.url("v1/config"))
            .map_err(|e| CatalogError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("warehouse", &self.config.warehouse);
        let body = self.get(url.to_string(), false).await?;
        let parsed: ConfigResponse = serde_json::from_slice(&body)
            .map_err(|e| CatalogError::Protocol(format!("config answer: {e}")))?;
        Ok(parsed
            .overrides
            .get("prefix")
            .or_else(|| parsed.defaults.get("prefix"))
            .cloned())
    }

    /// Loads a table with vended credentials.
    pub async fn vended_credentials(
        &self,
        namespace: &[String],
        table: &str,
    ) -> Result<VendedCredentials, CatalogError> {
        let prefix = self.prefix().await?;
        let ns = rest_namespace_path(namespace);
        let path = match prefix {
            Some(p) => format!(
                "v1/{}/namespaces/{ns}/tables/{}",
                encode_segment(&p),
                encode_segment(table)
            ),
            None => format!("v1/namespaces/{ns}/tables/{}", encode_segment(table)),
        };
        let body = self.get(self.url(&path), true).await?;
        vended_from_load_table(&body, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CatalogConfig {
        CatalogConfig::new(
            Url::parse("http://lakekeeper:8181/catalog").expect("url"),
            "ns-42",
        )
    }

    #[test]
    fn connect_conf_has_the_catalog_and_vending_only() {
        let conf = config().spark_conf(Flavor::Connect, &CatalogToken::None);
        assert_eq!(
            conf.get("spark.sql.catalog.loams").map(String::as_str),
            Some("org.apache.iceberg.spark.SparkCatalog")
        );
        assert_eq!(
            conf.get("spark.sql.catalog.loams.type").map(String::as_str),
            Some("rest")
        );
        assert_eq!(
            conf.get("spark.sql.catalog.loams.uri").map(String::as_str),
            Some("http://lakekeeper:8181/catalog")
        );
        assert_eq!(
            conf.get("spark.sql.catalog.loams.header.X-Iceberg-Access-Delegation")
                .map(String::as_str),
            Some("vended-credentials")
        );
        assert_eq!(
            conf.get("spark.sql.defaultCatalog").map(String::as_str),
            Some("loams")
        );
        assert!(!conf.contains_key("spark.jars.packages"));
        assert!(!conf.contains_key("spark.sql.catalog.loams.token"));
    }

    #[test]
    fn jvm_conf_adds_runtime_and_env_token_reference() {
        let conf = config()
            .with_s3_endpoint("http://rustfs:9000")
            .spark_conf(Flavor::Jvm, &CatalogToken::Env("LOAMS_TOKEN".into()));
        assert_eq!(
            conf.get("spark.sql.catalog.loams.token")
                .map(String::as_str),
            Some("$(LOAMS_TOKEN)")
        );
        assert_eq!(
            conf.get("spark.sql.catalog.loams.s3.path-style-access")
                .map(String::as_str),
            Some("true")
        );
        assert!(
            conf["spark.jars.packages"].contains("iceberg-spark-runtime-4.1_2.13:1.12.0"),
            "{conf:?}"
        );
        assert_eq!(
            conf.get("spark.sql.extensions").map(String::as_str),
            Some("org.apache.iceberg.spark.extensions.IcebergSparkSessionExtensions")
        );
    }

    #[test]
    fn literal_token_is_redacted_in_debug() {
        let t = CatalogToken::Literal("secret".into());
        assert!(!format!("{t:?}").contains("secret"));
    }

    #[test]
    fn house_names() {
        assert_eq!(
            house_table("loams", 42, "analytics", "events"),
            "loams.ns_42.analytics.events"
        );
        assert_eq!(house_table("loams", 42, "default", "e"), "loams.ns_42.e");
        assert_eq!(
            house_table("loams", 7, "my-db", "a`b"),
            "loams.ns_7.`my-db`.`a``b`"
        );
        let r = HouseRef {
            database: "analytics".into(),
            table: "events".into(),
        };
        assert_eq!(r.count_sql(), "SELECT count() FROM `analytics`.`events`");
    }

    #[test]
    fn rest_namespace_uses_unit_separator() {
        assert_eq!(
            rest_namespace_path(&["ns_42".into(), "a b".into()]),
            "ns_42%1Fa%20b"
        );
    }

    #[test]
    fn vended_prefers_longest_storage_credential_prefix() {
        let body = br#"{
            "metadata-location": "s3://b/ns/42/t/metadata/1.json",
            "metadata": {},
            "config": {"s3.access-key-id": "OLD"},
            "storage-credentials": [
                {"prefix": "s3://b/", "config": {"s3.access-key-id": "WIDE"}},
                {"prefix": "s3://b/ns/42/", "config": {"s3.access-key-id": "AK", "s3.session-token": "ST"}}
            ]
        }"#;
        let v = vended_from_load_table(body, Some("s3://b/ns/42/t")).expect("parse");
        assert_eq!(v.prefix.as_deref(), Some("s3://b/ns/42/"));
        assert!(v.has_s3_session());
        assert!(!format!("{v:?}").contains("ST"));
    }

    #[test]
    fn vended_falls_back_to_config() {
        let body =
            br#"{"config": {"s3.access-key-id": "AK", "s3.session-token": "ST", "other": "x"}}"#;
        let v = vended_from_load_table(body, None).expect("parse");
        assert_eq!(v.prefix, None);
        assert!(v.has_s3_session());
        assert!(!v.config.contains_key("other"));
    }
}
