//! Instance files (D1821): one YAML file per connector instance under
//! `--instances`, checked against the registry before anything starts.
//!
//! ```yaml
//! name: orders-in              # unique per install, [a-z0-9-], at most 63
//! connector: kafka             # a registry manifest id
//! profile: confluent-cloud     # optional target profile of the family
//! direction: source            # source | sink
//! namespace: default           # the engine namespace of the stream
//! namespace_id: 0              # the secret store's namespace (CONNECT_NAMESPACE)
//! stream: orders               # the Loams stream: a source writes it, a sink reads it
//! stream_partitions: 1         # a source creates the stream with this many if missing
//! delivery: exactly_once       # optional; checked against the manifest
//! dead_letter_stream: orders-in-dlq   # default <name>-dlq; "" turns the DLQ off
//! config: { … }                # the family's config; secrets by name only
//! ```
//!
//! The checks: the manifest exists; the direction's runtime is native (a
//! fallback runtime — Camel, Debezium Server — is Kubernetes-only, CN1:
//! `runtime_not_supported_on_vm`); this build has the family; the profile is
//! a target profile of the family; the use is within the manifest's
//! capabilities (`check_use`); and the config, the profile's defaults under
//! it, passes the manifest's config schema (`pipeline` and `rate_limit` are
//! the runtime's and are checked by the SDK).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use loams_connector_sdk::{ConnectorFactory, PipelineConfig, Profile, RateLimit};
use loams_flow::manifest::Choice;
use loams_flow::profile::{ProfileSchema, load_profile_with_schema};
use loams_flow::validate::{Direction, ManifestSchema, Uses, check_use};
use loams_flow::{ConnectorSpec, Delivery, Registry, RuntimeKind};
use serde::Deserialize;
use serde_json::Value;

/// One problem with one instance file. `code` is stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceError {
    pub file: String,
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.code, self.message)
    }
}

/// The codes.
pub mod code {
    pub const INVALID_INSTANCE: &str = "invalid_instance";
    pub const UNKNOWN_CONNECTOR: &str = "unknown_connector";
    pub const RUNTIME_NOT_SUPPORTED_ON_VM: &str = "runtime_not_supported_on_vm";
    pub const NOT_IN_THIS_BUILD: &str = "connector_not_in_this_build";
    pub const PROFILE: &str = "invalid_profile";
    pub const CAPABILITY: &str = "capability_not_declared";
    pub const CONFIG: &str = "invalid_config";
    pub const DUPLICATE: &str = "duplicate_instance";
}

/// The direction of an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Source,
    Sink,
}

impl Dir {
    pub fn as_str(self) -> &'static str {
        match self {
            Dir::Source => "source",
            Dir::Sink => "sink",
        }
    }
}

/// An instance file as written.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceFile {
    pub name: String,
    pub connector: String,
    #[serde(default)]
    pub profile: Option<String>,
    pub direction: Dir,
    pub namespace: String,
    #[serde(default)]
    pub namespace_id: u64,
    pub stream: String,
    #[serde(default)]
    pub stream_partitions: Option<u32>,
    #[serde(default)]
    pub delivery: Option<String>,
    /// `native` (the default) or `fallback`, which is refused here.
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub dead_letter_stream: Option<String>,
    #[serde(default)]
    pub config: Value,
}

/// A checked instance, ready to run.
#[derive(Clone)]
pub struct Instance {
    pub name: String,
    pub connector: String,
    pub direction: Dir,
    pub namespace: String,
    pub namespace_id: u64,
    pub stream: String,
    pub stream_partitions: u32,
    pub dead_letter_stream: Option<String>,
    /// The user's config (the factory merges the profile's defaults).
    pub config: Value,
    pub profile: Option<Profile>,
    pub pipeline: PipelineConfig,
    pub rate_limit: RateLimit,
    pub factory: Arc<dyn ConnectorFactory>,
    /// Where it came from, for messages.
    pub file: String,
}

impl fmt::Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the config's values (secret names only, but still).
        f.debug_struct("Instance")
            .field("name", &self.name)
            .field("connector", &self.connector)
            .field("direction", &self.direction)
            .field("stream", &format!("{}/{}", self.namespace, self.stream))
            .finish_non_exhaustive()
    }
}

/// Makes a family's factory for a manifest (and its profile).
pub type FactoryFn =
    Arc<dyn Fn(&str, Option<&Profile>) -> Result<Arc<dyn ConnectorFactory>, String> + Send + Sync>;

/// The families this build runs, by manifest id, and each family's profile
/// module (`kafka` for the Kafka family's target profiles).
#[derive(Clone, Default)]
pub struct Factories {
    by_id: BTreeMap<String, (Option<&'static str>, FactoryFn)>,
}

impl fmt::Debug for Factories {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.by_id.keys()).finish()
    }
}

impl Factories {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register manifest `id`, its family's profile `module`, and how to make
    /// its factory.
    pub fn register(
        &mut self,
        id: &str,
        module: Option<&'static str>,
        make: impl Fn(&str, Option<&Profile>) -> Result<Arc<dyn ConnectorFactory>, String>
        + Send
        + Sync
        + 'static,
    ) {
        self.by_id.insert(id.to_owned(), (module, Arc::new(make)));
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.by_id.keys().map(String::as_str)
    }

    fn get(&self, id: &str) -> Option<&(Option<&'static str>, FactoryFn)> {
        self.by_id.get(id)
    }

    /// The families compiled into this binary (`conn-*` features).
    pub fn built_in() -> Self {
        #[allow(unused_mut)]
        let mut f = Self::new();
        #[cfg(feature = "conn-kafka")]
        {
            use loams_connector_kafka::Kafka;
            for id in ["kafka", "redpanda", "azure-event-hubs"] {
                f.register(
                    id,
                    Some(loams_connector_kafka::kafka::MODULE),
                    |id, profile| {
                        let k = match profile {
                            Some(p) => Kafka::for_profile(p).map_err(|e| e.message)?,
                            None if id == "kafka" => Kafka::new(),
                            None => return Err(format!("{id} runs only with its profile")),
                        };
                        Ok(Arc::new(k) as Arc<dyn ConnectorFactory>)
                    },
                );
            }
        }
        #[cfg(feature = "conn-postgres")]
        {
            use loams_connector_postgres::{PostgresFactory, Target};
            for t in [
                Target::Postgres,
                Target::Aurora,
                Target::Cockroach,
                Target::Pgvector,
            ] {
                f.register(t.manifest_id(), Some("postgres"), move |_, _| {
                    Ok(Arc::new(PostgresFactory::new(t)) as Arc<dyn ConnectorFactory>)
                });
            }
        }
        #[cfg(feature = "conn-mysql")]
        {
            use loams_connector_mysql::{MysqlFactory, Target};
            // `aurora` is the Postgres family's registration: one manifest id,
            // two engines by profile; Aurora MySQL joins when an instance
            // needs it.
            for t in [Target::Mysql, Target::Mariadb] {
                f.register(t.manifest_id(), Some("mysql"), move |_, _| {
                    Ok(Arc::new(MysqlFactory::new(t)) as Arc<dyn ConnectorFactory>)
                });
            }
        }
        f
    }
}

/// The registry, the profiles and their schemas, from a `connectors/`
/// directory (`--connectors-dir`; the image ships the repository's).
pub struct Catalog {
    registry: Registry,
    profiles_dir: PathBuf,
    profile_schema: ProfileSchema,
    /// Manifest ids accepted without a registry entry (tests only).
    unregistered: BTreeSet<String>,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog")
            .field("manifests", &self.registry.len())
            .field("profiles_dir", &self.profiles_dir)
            .finish_non_exhaustive()
    }
}

impl Catalog {
    pub fn load(connectors_dir: &Path) -> Result<Self, String> {
        let schema = ManifestSchema::load(&connectors_dir.join("schema/connector.schema.json"))
            .map_err(|e| format!("{}: {e}", connectors_dir.display()))?;
        let registry =
            Registry::load_compiled(&connectors_dir.join("registry"), connectors_dir, &schema)
                .map_err(|e| format!("the connector registry: {e}"))?;
        let profile_schema =
            ProfileSchema::load(&connectors_dir.join("schema/profile.schema.json"))
                .map_err(|e| format!("the profile schema: {e}"))?;
        Ok(Self {
            registry,
            profiles_dir: connectors_dir.join("profiles"),
            profile_schema,
            unregistered: BTreeSet::new(),
        })
    }

    /// Accept `id` with no manifest (the SDK's reference connector in
    /// tests): no runtime, capability or schema check.
    pub fn allow_unregistered(mut self, id: &str) -> Self {
        self.unregistered.insert(id.to_owned());
        self
    }

    fn profile(&self, id: &str) -> Result<Profile, String> {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!("profile {id:?} is not a profile id"));
        }
        load_profile_with_schema(
            &self.profiles_dir.join(format!("{id}.yaml")),
            &self.profile_schema,
        )
        .map_err(|e| e.to_string())
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// Every `*.yaml`/`*.yml` in `dir`, checked. All problems are reported.
pub fn load_dir(
    dir: &Path,
    catalog: &Catalog,
    factories: &Factories,
) -> Result<Vec<Instance>, Vec<InstanceError>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return Err(vec![InstanceError {
                file: dir.display().to_string(),
                code: code::INVALID_INSTANCE,
                message: format!("cannot read the instances directory: {e}"),
            }]);
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut names = BTreeSet::new();
    for path in paths {
        let file = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                errors.push(InstanceError {
                    file,
                    code: code::INVALID_INSTANCE,
                    message: e.to_string(),
                });
                continue;
            }
        };
        match parse(&file, &text, catalog, factories) {
            Ok(i) => {
                if !names.insert(i.name.clone()) {
                    errors.push(InstanceError {
                        file,
                        code: code::DUPLICATE,
                        message: format!("another file already defines instance {:?}", i.name),
                    });
                } else {
                    out.push(i);
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// One instance file's text, checked.
pub fn parse(
    file: &str,
    text: &str,
    catalog: &Catalog,
    factories: &Factories,
) -> Result<Instance, InstanceError> {
    let err = |code: &'static str, message: String| InstanceError {
        file: file.to_owned(),
        code,
        message,
    };
    let f: InstanceFile =
        serde_norway::from_str(text).map_err(|e| err(code::INVALID_INSTANCE, e.to_string()))?;
    if !valid_name(&f.name) {
        return Err(err(
            code::INVALID_INSTANCE,
            format!("name {:?}: 1-63 characters of [a-z0-9-]", f.name),
        ));
    }
    if f.namespace.is_empty() || f.stream.is_empty() {
        return Err(err(
            code::INVALID_INSTANCE,
            "namespace and stream are required".to_owned(),
        ));
    }
    if !f.config.is_object() && !f.config.is_null() {
        return Err(err(
            code::INVALID_INSTANCE,
            "config must be a map".to_owned(),
        ));
    }
    let config = if f.config.is_null() {
        Value::Object(Default::default())
    } else {
        f.config.clone()
    };
    match f.runtime.as_deref() {
        None | Some("native") => {}
        Some("fallback") => {
            return Err(err(
                code::RUNTIME_NOT_SUPPORTED_ON_VM,
                format!(
                    "{}: fallback runtimes (Camel, Debezium Server) run on Kubernetes only; \
                     loams-fabric connect runs native connectors",
                    f.connector
                ),
            ));
        }
        Some(other) => {
            return Err(err(
                code::INVALID_INSTANCE,
                format!("runtime {other:?}: expected native"),
            ));
        }
    }
    let direction = match f.direction {
        Dir::Source => Direction::Source,
        Dir::Sink => Direction::Sink,
    };
    let delivery = match &f.delivery {
        Some(d) => Some(
            serde_json::from_value::<Delivery>(Value::String(d.clone()))
                .map_err(|_| err(code::INVALID_INSTANCE, format!("delivery {d:?} is unknown")))?,
        ),
        None => None,
    };

    let spec: Option<&ConnectorSpec> = catalog.registry.get(&f.connector);
    let profile_id = f
        .profile
        .clone()
        .or_else(|| spec.and_then(|s| s.runtime.profile.clone()));
    if spec.is_none() && !catalog.unregistered.contains(&f.connector) {
        return Err(err(
            code::UNKNOWN_CONNECTOR,
            format!("no manifest {:?} in the registry", f.connector),
        ));
    }
    if let Some(spec) = spec {
        match spec.runtime.for_direction(direction, Choice::Default) {
            Some((RuntimeKind::Native, _)) => {}
            Some((kind, _)) => {
                return Err(err(
                    code::RUNTIME_NOT_SUPPORTED_ON_VM,
                    format!(
                        "{} runs on the {kind} runtime as a {}, which is Kubernetes-only; \
                         loams-fabric connect runs native connectors",
                        f.connector,
                        f.direction.as_str()
                    ),
                ));
            }
            None => {
                return Err(err(
                    code::CAPABILITY,
                    format!("{} has no {} runtime", f.connector, f.direction.as_str()),
                ));
            }
        }
        check_use(
            spec,
            &Uses {
                direction,
                delivery,
                ..Uses::default()
            },
        )
        .map_err(|e| err(code::CAPABILITY, e.to_string()))?;
    }
    let Some((module, make)) = factories.get(&f.connector) else {
        return Err(err(
            code::NOT_IN_THIS_BUILD,
            format!(
                "{} is native, but this loams-fabric was built without its family \
                 (it has: {})",
                f.connector,
                factories.ids().collect::<Vec<_>>().join(", ")
            ),
        ));
    };
    let profile = match &profile_id {
        Some(id) => {
            let p = catalog.profile(id).map_err(|e| err(code::PROFILE, e))?;
            if p.kind != loams_connector_sdk::ProfileKind::Target {
                return Err(err(
                    code::PROFILE,
                    format!("profile {id} is not a target profile"),
                ));
            }
            if module.is_some_and(|m| m != p.module) {
                return Err(err(
                    code::PROFILE,
                    format!(
                        "profile {id} is for the {} family, not {}'s",
                        p.module, f.connector
                    ),
                ));
            }
            Some(p)
        }
        None => None,
    };
    let factory = make(&f.connector, profile.as_ref()).map_err(|e| err(code::PROFILE, e))?;

    let pipeline = PipelineConfig::default()
        .with_overrides(&config)
        .map_err(|e| err(code::CONFIG, e.message))?;
    let rate_limit = RateLimit::from_config(&config).map_err(|e| err(code::CONFIG, e.message))?;
    if let Some(spec) = spec
        && spec.config_schema.is_object()
    {
        let mut checked = match &profile {
            Some(p) => p.config_with_defaults(&config),
            None => config.clone(),
        };
        if let Some(o) = checked.as_object_mut() {
            o.remove("pipeline");
            o.remove("rate_limit");
        }
        let validator = jsonschema::validator_for(&spec.config_schema)
            .map_err(|e| err(code::CONFIG, format!("the manifest's config schema: {e}")))?;
        let problems: Vec<String> = validator
            .iter_errors(&checked)
            .map(|e| format!("config{}: {e}", e.instance_path()))
            .collect();
        if !problems.is_empty() {
            return Err(err(code::CONFIG, problems.join("; ")));
        }
    }
    let dead_letter_stream = match f.dead_letter_stream {
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s),
        None => Some(format!("{}-dlq", f.name)),
    };
    Ok(Instance {
        name: f.name,
        connector: f.connector,
        direction: f.direction,
        namespace: f.namespace,
        namespace_id: f.namespace_id,
        stream: f.stream,
        stream_partitions: f.stream_partitions.unwrap_or(1).max(1),
        dead_letter_stream,
        config,
        profile,
        pipeline,
        rate_limit,
        factory,
        file: file.to_owned(),
    })
}
