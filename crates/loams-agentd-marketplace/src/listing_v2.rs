//! The typed listing, v1 and v2 (design 86, D2605, D2624; OP2-B Task 2).
//!
//! [`ListingV2::from_json`] tells the two schema versions apart by `$schema`
//! (no `$schema`, or v1's, is v1), validates the document against that
//! schema, and builds one type from either. A v1 listing is an agent-kind
//! listing: its `targets` map onto v2's (`agentd` to cluster and
//! node-laptop, `byoc-server` and `app-byoc` to cluster) and its §73 agent
//! fields stay in [`ListingV2::raw`], the signed document.
//!
//! A `values_schema` is data the operator and the console evaluate at
//! install, so it is held to 64 KiB and to local (`#…`) references only.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::index::canonical;
use crate::listing::{self, Issue, SCHEMA_V2_ID};

/// The most a `values_schema` may take, in canonical JSON bytes.
const VALUES_SCHEMA_MAX_BYTES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListingKind {
    Agent,
    Harness,
    McpServer,
    App,
    Engine,
}

impl ListingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ListingKind::Agent => "agent",
            ListingKind::Harness => "harness",
            ListingKind::McpServer => "mcp-server",
            ListingKind::App => "app",
            ListingKind::Engine => "engine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Cluster,
    NodeLaptop,
    NodeDevice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Package {
    /// `image` has no tag; `digest` is `sha256:<64 hex>`.
    Oci { image: String, digest: String },
    /// `chart` is `oci://…` (manifest digest) or an https `.tgz` (file sha256).
    Helm {
        chart: String,
        version: String,
        digest: String,
    },
    /// An https tarball of the kustomization directory.
    Kustomize { url: String, digest: String },
    /// Node-laptop and node-device only.
    Wasm { url: String, digest: String },
    /// JSON form: `[{triple, url, digest}]`.
    Binary {
        #[serde(with = "binary_targets")]
        targets: Vec<(String, String, String)>,
    },
}

/// `(triple, url, digest)` tuples as `{triple, url, digest}` objects.
mod binary_targets {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Entry {
        triple: String,
        url: String,
        digest: String,
    }

    pub fn serialize<S: Serializer>(
        targets: &[(String, String, String)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(targets.iter().map(|(triple, url, digest)| Entry {
            triple: triple.clone(),
            url: url.clone(),
            digest: digest.clone(),
        }))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<(String, String, String)>, D::Error> {
        let entries = Vec::<Entry>::deserialize(deserializer)?;
        Ok(entries
            .into_iter()
            .map(|e| (e.triple, e.url, e.digest))
            .collect())
    }
}

/// D2603 names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageKind {
    Object,
    SqlMysql,
    SqlPostgres,
    Kafka,
    Kv,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requires {
    #[serde(default)]
    pub storage: Vec<StorageKind>,
    #[serde(default)]
    pub cpu_millis: Option<u32>,
    #[serde(default)]
    pub memory_mib: Option<u32>,
    #[serde(default)]
    pub gpu: Option<Gpu>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub count: u32,
    pub vendor: GpuVendor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostHint {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub storage_gib: u32,
    pub egress_gib_per_month: u32,
    #[serde(default)]
    pub managed: Vec<StorageKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependsOn {
    pub id: String,
    pub version: semver::VersionReq,
}

/// How an OCI app or engine runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub ports: Vec<Port>,
    #[serde(default)]
    pub health: Option<Health>,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub port: u16,
    pub protocol: PortProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub http_path: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaVersion {
    V1,
    V2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub repository: String,
    pub commit: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceType {
    NpmSlsa,
    PypiPep740,
    Cosign,
    SigstoreBundle,
    GithubAttestation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRef {
    #[serde(rename = "type")]
    pub kind: ProvenanceType,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Why a document is not a listing.
#[derive(Debug, thiserror::Error)]
pub enum ListingError {
    /// The document breaks its schema.
    #[error("the listing breaks its schema: {}", join_issues(.0))]
    Schema(Vec<Issue>),
    /// The `values_schema` is refused (remote `$ref`, over 64 KiB, or not a
    /// schema).
    #[error("values_schema refused: {0}")]
    ValuesSchema(String),
    /// The document passed the schema but does not fit the type.
    #[error("the listing does not fit its type: {0}")]
    Shape(String),
}

fn join_issues(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A listing of either schema version.
pub struct ListingV2 {
    pub schema: SchemaVersion,
    pub id: String,
    pub kind: ListingKind,
    pub version: semver::Version,
    pub title: String,
    pub summary: String,
    pub licence: String,
    pub source: SourceRef,
    pub security_contact: Option<String>,
    pub package: Option<Package>,
    pub provenance: Option<ProvenanceRef>,
    pub requires: Requires,
    pub targets: Vec<Target>,
    pub depends_on: Vec<DependsOn>,
    pub values_schema: Option<Value>,
    pub cost_hint: Option<CostHint>,
    pub run: Option<Run>,
    /// The signed document; §73 agent fields (artifact, runtime, tools,
    /// place…) stay here.
    pub raw: Value,
    /// Compiled from `values_schema` on first use.
    values_validator: OnceLock<jsonschema::Validator>,
}

impl std::fmt::Debug for ListingV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListingV2")
            .field("schema", &self.schema)
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("version", &self.version)
            .field("package", &self.package)
            .field("targets", &self.targets)
            .finish_non_exhaustive()
    }
}

impl Clone for ListingV2 {
    fn clone(&self) -> Self {
        Self {
            schema: self.schema,
            id: self.id.clone(),
            kind: self.kind,
            version: self.version.clone(),
            title: self.title.clone(),
            summary: self.summary.clone(),
            licence: self.licence.clone(),
            source: self.source.clone(),
            security_contact: self.security_contact.clone(),
            package: self.package.clone(),
            provenance: self.provenance.clone(),
            requires: self.requires.clone(),
            targets: self.targets.clone(),
            depends_on: self.depends_on.clone(),
            values_schema: self.values_schema.clone(),
            cost_hint: self.cost_hint.clone(),
            run: self.run.clone(),
            raw: self.raw.clone(),
            values_validator: OnceLock::new(),
        }
    }
}

/// Two listings are equal when their signed documents are.
impl PartialEq for ListingV2 {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

/// The fields both schema versions share, read straight off the document.
#[derive(Deserialize)]
struct Wire {
    id: String,
    kind: ListingKind,
    version: semver::Version,
    title: String,
    summary: String,
    licence: String,
    source: SourceRef,
    #[serde(default)]
    security_contact: Option<String>,
    #[serde(default)]
    provenance: Option<ProvenanceRef>,
}

/// The fields only v2 has.
#[derive(Deserialize)]
struct WireV2 {
    #[serde(default)]
    package: Option<Package>,
    #[serde(default)]
    requires: Requires,
    #[serde(default)]
    targets: Vec<Target>,
    #[serde(default)]
    depends_on: Vec<DependsOn>,
    #[serde(default)]
    values_schema: Option<Value>,
    #[serde(default)]
    cost_hint: Option<CostHint>,
    #[serde(default)]
    run: Option<Run>,
}

impl ListingV2 {
    /// Validates `doc` against its schema (v2 when `$schema` is v2's, else
    /// v1) and reads it.
    pub fn from_json(doc: &Value) -> Result<ListingV2, ListingError> {
        let is_v2 = doc.get("$schema").and_then(Value::as_str) == Some(SCHEMA_V2_ID);
        let (schema, check) = if is_v2 {
            (SchemaVersion::V2, listing::validate_v2(doc))
        } else {
            (SchemaVersion::V1, listing::validate(doc))
        };
        check.map_err(ListingError::Schema)?;

        let wire: Wire = from_value(doc)?;
        let v2: WireV2 = if is_v2 {
            from_value(doc)?
        } else {
            WireV2 {
                package: None,
                requires: Requires::default(),
                targets: v1_targets(doc),
                depends_on: Vec::new(),
                values_schema: None,
                cost_hint: None,
                run: None,
            }
        };

        let values_validator = OnceLock::new();
        if let Some(values_schema) = &v2.values_schema {
            let validator = compile_values_schema(values_schema)?;
            let _ = values_validator.set(validator);
        }

        Ok(ListingV2 {
            schema,
            id: wire.id,
            kind: wire.kind,
            version: wire.version,
            title: wire.title,
            summary: wire.summary,
            licence: wire.licence,
            source: wire.source,
            security_contact: wire.security_contact,
            package: v2.package,
            provenance: wire.provenance,
            requires: v2.requires,
            targets: v2.targets,
            depends_on: v2.depends_on,
            values_schema: v2.values_schema,
            cost_hint: v2.cost_hint,
            run: v2.run,
            raw: doc.clone(),
            values_validator,
        })
    }

    /// The part of `id` before the first `/`.
    pub fn namespace(&self) -> &str {
        self.id.split_once('/').map_or(&self.id, |(ns, _)| ns)
    }

    /// Checks install values against `values_schema`. Without one, only the
    /// empty object is accepted.
    pub fn validate_values(&self, values: &Value) -> Result<(), Vec<Issue>> {
        let Some(values_schema) = &self.values_schema else {
            return if values.as_object().is_some_and(serde_json::Map::is_empty) {
                Ok(())
            } else {
                Err(vec![Issue {
                    instance_path: String::new(),
                    schema_path: String::new(),
                    message: "this listing takes no values (it has no values_schema)".into(),
                }])
            };
        };
        let validator = match self.values_validator.get() {
            Some(validator) => validator,
            None => {
                let compiled = compile_values_schema(values_schema).map_err(|e| {
                    vec![Issue {
                        instance_path: String::new(),
                        schema_path: String::new(),
                        message: e.to_string(),
                    }]
                })?;
                self.values_validator.get_or_init(|| compiled)
            }
        };
        let issues: Vec<Issue> = validator
            .iter_errors(values)
            .map(|error| Issue {
                instance_path: error.instance_path().to_string(),
                schema_path: error.schema_path().to_string(),
                message: error.to_string(),
            })
            .collect();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

fn from_value<T: serde::de::DeserializeOwned>(doc: &Value) -> Result<T, ListingError> {
    serde_json::from_value(doc.clone()).map_err(|e| ListingError::Shape(e.to_string()))
}

/// v1 `targets` onto v2's (D2605, decision 2). A v1 listing without any
/// (harnesses and MCP servers need none) is an agentd listing.
fn v1_targets(doc: &Value) -> Vec<Target> {
    let mut out = Vec::new();
    let mut push = |target: Target| {
        if !out.contains(&target) {
            out.push(target);
        }
    };
    let names: Vec<&str> = doc
        .get("targets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_else(|| vec!["agentd"]);
    for name in names {
        match name {
            "agentd" => {
                push(Target::Cluster);
                push(Target::NodeLaptop);
            }
            "byoc-server" | "app-byoc" => push(Target::Cluster),
            _ => {}
        }
    }
    out
}

/// Refuses remote references and oversize schemas, then compiles.
fn compile_values_schema(schema: &Value) -> Result<jsonschema::Validator, ListingError> {
    if let Some(reference) = remote_ref(schema) {
        return Err(ListingError::ValuesSchema(format!(
            "`{reference}` is not a local reference; only `#…` references are allowed"
        )));
    }
    let size = canonical(schema).len();
    if size > VALUES_SCHEMA_MAX_BYTES {
        return Err(ListingError::ValuesSchema(format!(
            "{size} bytes is over the {VALUES_SCHEMA_MAX_BYTES}-byte limit"
        )));
    }
    jsonschema::draft202012::new(schema).map_err(|e| ListingError::ValuesSchema(e.to_string()))
}

/// The first `$ref` or `$dynamicRef` string anywhere in `value` that does not
/// start with `#`.
fn remote_ref(value: &Value) -> Option<&str> {
    match value {
        Value::Object(map) => map.iter().find_map(|(key, item)| {
            if (key == "$ref" || key == "$dynamicRef")
                && let Some(reference) = item.as_str()
                && !reference.starts_with('#')
            {
                return Some(reference);
            }
            remote_ref(item)
        }),
        Value::Array(items) => items.iter().find_map(remote_ref),
        _ => None,
    }
}
