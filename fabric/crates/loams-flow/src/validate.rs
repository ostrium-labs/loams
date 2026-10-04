//! What a manifest is checked against: `connectors/schema/connector.schema.json`,
//! CN1 plan Task 1's six semantic rules, §33 §4 rule 1's refusal, and D359's
//! licence gate.
//!
//! The order is the design's: **the JSON Schema first**, then the rules the schema
//! cannot express. Every problem is collected rather than reported one at a time, so
//! a manifest that is wrong in five ways says so in one run.
//!
//! * [`schema_violations`] is the schema alone, which the loader calls first so a
//!   malformed manifest is reported as malformed before it reaches into a `$ref`.
//! * [`validate_manifest`] is the schema plus the rules, and
//!   [`validate_manifest_with_config_schema`] adds CN1's secret rule, which needs
//!   the resolved instance-config schema that only the loader has.
//! * [`check_use`] is §33 §4 rule 1: a route that uses an undeclared capability is
//!   refused by connector, capability and manifest version.
//! * [`LicenceGate`] is D359: AGPL, BSL, SSPL, ELv2, `NOASSERTION` and unknown ids
//!   are refused for anything Loams ships or runs by default, and the refused set is
//!   read from `connectors/licences.toml` rather than written here a second time.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::manifest::{ConnectorSpec, Delivery, ManifestError, ManifestErrors, Ordering, Priority};

/// §33 §4 rule 1's refusal, which names the connector, the capability and the
/// manifest version — the three things a user needs to know which manifest to look
/// at and which major it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{connector} (manifest specVersion {spec_version}) does not declare the capability {capability}"
)]
pub struct CapabilityError {
    /// The connector's registry id.
    pub connector: String,
    /// The capability the route uses and the manifest does not declare, named as the
    /// manifest's own key (`cdc`, `upsert`, `delivery.sink`).
    pub capability: String,
    /// The manifest's `specVersion`, which §33 §4 rule 4 makes the thing a route pins.
    pub spec_version: String,
}

/// The manifest schema compiled once.
///
/// `jsonschema::validator_for` compiles the whole document, so a `Registry` holds one
/// of these and hands it to 200 manifests rather than compiling it 200 times. The
/// schema's path is a parameter and not a constant so a test can point at a fixture;
/// [`default_manifest_schema_path`] is the repository-relative default.
#[derive(Debug)]
pub struct ManifestSchema {
    validator: jsonschema::Validator,
}

impl ManifestSchema {
    /// The compiled schema at `path`.
    pub fn load(path: &Path) -> Result<Self, SchemaError> {
        let name = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|error| SchemaError::Unreadable {
            path: name.clone(),
            message: error.to_string(),
        })?;
        let document: Value =
            serde_json::from_str(&text).map_err(|error| SchemaError::NotJson {
                path: name.clone(),
                message: error.to_string(),
            })?;
        let validator =
            jsonschema::validator_for(&document).map_err(|error| SchemaError::NotASchema {
                path: name,
                message: error.to_string(),
            })?;
        Ok(Self { validator })
    }

    /// The compiled schema at [`default_manifest_schema_path`].
    pub fn embedded() -> Result<Self, SchemaError> {
        Self::load(&default_manifest_schema_path())
    }

    /// The compiled validator, which [`validate_manifest`] takes.
    pub fn validator(&self) -> &jsonschema::Validator {
        &self.validator
    }
}

/// Why a manifest schema could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    /// The file is not there or not readable.
    #[error("{path}: cannot read the manifest schema: {message}")]
    Unreadable {
        /// The schema's path.
        path: String,
        /// The I/O error.
        message: String,
    },
    /// The file is not JSON.
    #[error("{path}: the manifest schema is not valid JSON: {message}")]
    NotJson {
        /// The schema's path.
        path: String,
        /// The parse error.
        message: String,
    },
    /// The JSON is not a usable JSON Schema.
    #[error("{path}: the manifest schema does not compile: {message}")]
    NotASchema {
        /// The schema's path.
        path: String,
        /// The compiler's error.
        message: String,
    },
}

/// The manifest schema's path in this repository. A default, not a constant: the
/// loader takes the path as a parameter so a test can point at a fixture.
pub fn default_manifest_schema_path() -> PathBuf {
    crate::manifest::repository_root().join("connectors/schema/connector.schema.json")
}

/// The licence gate's input in this repository (design §33 D359).
pub fn default_licences_path() -> PathBuf {
    crate::manifest::repository_root().join("connectors/licences.toml")
}

/// Every violation of `connector.schema.json`, not just the first.
///
/// The JSON Schema is the authority on the manifest's shape (CN1 plan Task 1), so a
/// caller gets all of its complaints in one pass: a hand-written manifest is fixed
/// once, not once per complaint.
pub fn schema_violations(value: &Value, schema: &jsonschema::Validator) -> Vec<ManifestError> {
    schema
        .iter_errors(value)
        .map(|error| {
            let instance = dotted(error.instance_path());
            let path = if instance.is_empty() {
                let schema_path = dotted(error.schema_path());
                if schema_path.is_empty() {
                    "manifest".to_string()
                } else {
                    schema_path
                }
            } else {
                instance
            };
            ManifestError::new(path, error.to_string())
        })
        .collect()
}

/// The manifest at `value`, checked against `connector.schema.json` and CN1 plan
/// Task 1's semantic rules.
///
/// CN1's sixth rule, that `secrets` and the config schema's `writeOnly` properties
/// are exactly equal, needs the resolved instance-config schema, which only
/// [`crate::manifest::load_manifest`] has; this entry point leaves it unchecked and
/// [`validate_manifest_with_config_schema`] is the one that runs it.
pub fn validate_manifest(
    value: &Value,
    schema: &jsonschema::Validator,
) -> Result<ConnectorSpec, Vec<ManifestError>> {
    validate_manifest_with_config_schema(value, schema, None)
}

/// [`validate_manifest`] with the instance-config schema already resolved, so CN1's
/// secret rule runs.
pub fn validate_manifest_with_config_schema(
    value: &Value,
    schema: &jsonschema::Validator,
    config_schema: Option<&Value>,
) -> Result<ConnectorSpec, Vec<ManifestError>> {
    let violations = schema_violations(value, schema);
    if !violations.is_empty() {
        return Err(violations);
    }
    let spec: ConnectorSpec = serde_json::from_value(value.clone())
        .map_err(|error| vec![ManifestError::new("manifest", error.to_string())])?;
    let rules = semantic_rules(&spec, schema, value, config_schema);
    if rules.is_empty() {
        Ok(spec)
    } else {
        Err(rules)
    }
}

/// CN1 plan Task 1's semantic rules, the ones `connector.schema.json` cannot
/// express. Every problem is returned, not just the first.
pub fn semantic_rules(
    spec: &ConnectorSpec,
    schema: &jsonschema::Validator,
    value: &Value,
    config_schema: Option<&Value>,
) -> Vec<ManifestError> {
    let mut errors = Vec::new();
    rule_upsert_declares_a_key(spec, config_schema, &mut errors);
    rule_cdc_implies_streaming_and_position(spec, &mut errors);
    rule_exactly_once_needs_transactional_or_idempotent(spec, &mut errors);
    rule_starred_implies_p1(spec, &mut errors);
    rule_auth_enum_is_enforced(schema, value, &mut errors);
    rule_secrets_match_the_config_schema(spec, config_schema, &mut errors);
    errors
}

/// **Rule 1 — a sink with `upsert: true` declares a key in its config schema.**
///
/// §33 §4's rule 1 refuses a route that upserts through a sink that does not declare
/// `upsert`, and `connector.schema.json`'s `sink.upsert` description says the sink
/// "upserts by a key it declares in its config schema". Declaring the capability and
/// giving no way to name the key would make the capability unusable, so the two must
/// come together. The property is looked for as `key`: the three ★ manifests that
/// declare `upsert` — `postgresql`, `mysql` and `elasticsearch` — all spell it
/// `key`, and each says in its description that it is required *because* the
/// manifest declares the capability.
fn rule_upsert_declares_a_key(
    spec: &ConnectorSpec,
    config_schema: Option<&Value>,
    errors: &mut Vec<ManifestError>,
) {
    let Some(_sink) = spec.capabilities.sink.as_ref().filter(|sink| sink.upsert) else {
        return;
    };
    let Some(config) = config_schema else {
        return;
    };
    if config["properties"]["key"].is_null() {
        errors.push(ManifestError::new(
            "capabilities.sink.upsert",
            format!(
                "{}: the sink declares upsert, so its config schema must declare a `key` \
                 property, and {} has none",
                spec.id, spec.config_ref.reference
            ),
        ));
    }
}

/// **Rule 2 — `cdc: true` implies `source.streaming` and a non-empty `source.position`.**
///
/// CN1 plan Task 1 states it; the reasoning is §33 §6 and Ruling 4. A change stream
/// is delivered continuously and its position is what makes a restart resume rather
/// than replay, so a connector that declares `cdc` without either has declared a
/// capability it cannot honour.
fn rule_cdc_implies_streaming_and_position(spec: &ConnectorSpec, errors: &mut Vec<ManifestError>) {
    let Some(source) = spec.capabilities.source.as_ref() else {
        return;
    };
    if !source.cdc {
        return;
    }
    if !source.streaming {
        errors.push(ManifestError::new(
            "capabilities.source.streaming",
            format!(
                "{}: source.cdc is true, which implies source.streaming (CN1 Task 1's \
                 semantic rules; §33 §7)",
                spec.id
            ),
        ));
    }
    if source.position.trim().is_empty() {
        errors.push(ManifestError::new(
            "capabilities.source.position",
            format!(
                "{}: source.cdc is true, so the source must name what it checkpoints \
                 (CN1 Task 1's semantic rules; §33 §6, Ruling 4)",
                spec.id
            ),
        ));
    }
}

/// **Rule 3 — `delivery.sink = exactly_once` requires `sink.transactional` or
/// `sink.idempotent`.**
///
/// CN1 plan Task 1 states it. §33 §2.2 says `exactly_once` is a claim only where the
/// external system deduplicates, so the manifest has to say which mechanism makes it
/// true: a transaction that spans the batch, or the sink's own dedup.
fn rule_exactly_once_needs_transactional_or_idempotent(
    spec: &ConnectorSpec,
    errors: &mut Vec<ManifestError>,
) {
    if spec.capabilities.delivery.sink != Delivery::ExactlyOnce {
        return;
    }
    let covered = spec
        .capabilities
        .sink
        .as_ref()
        .is_some_and(|sink| sink.transactional || sink.idempotent);
    if !covered {
        errors.push(ManifestError::new(
            "capabilities.delivery.sink",
            format!(
                "{}: delivery.sink is exactly_once, which needs sink.transactional or \
                 sink.idempotent (CN1 Task 1's semantic rules; §33 §2.2)",
                spec.id
            ),
        ));
    }
}

/// **Rule 4 — `starred` implies `priority = P1`.**
///
/// §33 §8 and D358 make P1 the ★ set of CN1, so a starred row that is not P1 says
/// the rollout phase and the star disagree. The Camel runtime row is P1 and *not*
/// starred, which is the other direction and legal.
fn rule_starred_implies_p1(spec: &ConnectorSpec, errors: &mut Vec<ManifestError>) {
    if spec.starred && spec.priority != Priority::P1 {
        errors.push(ManifestError::new(
            "priority",
            format!(
                "{}: starred is true, which implies priority P1 (CN1 Task 1's semantic \
                 rules; §33 §8, D358), but priority is {}",
                spec.id, spec.priority
            ),
        ));
    }
}

/// **Rule 5 — every auth method is one of Appendix A's legend's slugs.**
///
/// The JSON Schema's `auth.items.enum` is what enforces this, and
/// `AuthMethod`'s variants are pinned to that list by `enum_slugs_match_the_json_schema`.
/// The rule therefore asserts the enforcement rather than assuming it: the schema is
/// probed with a value no legend defines, and a schema that accepts it would let any
/// string into a manifest's `auth` unchecked.
fn rule_auth_enum_is_enforced(
    schema: &jsonschema::Validator,
    value: &Value,
    errors: &mut Vec<ManifestError>,
) {
    let mut probe = value.clone();
    probe["auth"] = Value::Array(vec![Value::String(AUTH_PROBE_SLUG.to_string())]);
    let enforced = schema
        .iter_errors(&probe)
        .any(|error| dotted(error.instance_path()).starts_with("auth"));
    if !enforced {
        errors.push(ManifestError::new(
            "auth",
            "the manifest schema does not enforce an enum on auth, so a manifest's auth \
             methods are unchecked (CN1 Task 1's semantic rules; Appendix A's legend)",
        ));
    }
}

/// A slug no legend defines, used only to probe the schema's `auth` enum.
const AUTH_PROBE_SLUG: &str = "loams-not-an-auth-method";

/// **Rule 6 — `secrets` and the config schema's `writeOnly` properties are exactly
/// equal.**
///
/// CN1 plan Task 1 asks for one direction: every name in `secrets` names a property
/// marked `"writeOnly": true`. CN1's execution ruling on that rule makes it an
/// equality in both directions, because a `writeOnly` property the manifest does not
/// name is a secret field no secret store entry resolves, and a `secrets` name with
/// no `writeOnly` property behind it names nothing.
///
/// The 21 ★ manifests write dotted JSON paths (`sasl.password`,
/// `s3.access_key_id`), so the walk follows nested `properties` objects and reports
/// the full path.
fn rule_secrets_match_the_config_schema(
    spec: &ConnectorSpec,
    config_schema: Option<&Value>,
    errors: &mut Vec<ManifestError>,
) {
    let Some(config) = config_schema else {
        return;
    };
    let write_only = write_only_paths(config);
    let declared: BTreeSet<String> = spec.secrets.iter().cloned().collect();
    for secret in &spec.secrets {
        if !write_only.contains(secret) {
            errors.push(ManifestError::new(
                format!("secrets.{secret}"),
                format!(
                    "{}: {} has no property at this path marked \"writeOnly\": true, so the \
                     name resolves to nothing (CN1 Task 1's semantic rules; D189)",
                    spec.id, spec.config_ref.reference
                ),
            ));
        }
    }
    for path in write_only.difference(&declared) {
        let path = path.as_str();
        errors.push(ManifestError::new(
            format!("secrets.{path}"),
            format!(
                "{}: {} marks {path:?} \"writeOnly\", so it is a secret field and belongs in \
                 the manifest's secrets list (CN1 Task 1's semantic rules; D189)",
                spec.id, spec.config_ref.reference
            ),
        ));
    }
}

/// Every `"writeOnly": true` property of a config schema, as dotted paths.
///
/// Nested objects are walked because a manifest's `secrets` are dotted paths; an
/// array's `items` are not, because no manifest names a secret inside a list (the
/// 21 ★ schemas put every secret under a named object or at the top level).
fn write_only_paths(schema: &Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    collect_write_only(schema, "", &mut found);
    found
}

fn collect_write_only(node: &Value, prefix: &str, found: &mut BTreeSet<String>) {
    let Some(properties) = node.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, child) in properties {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if child.get("writeOnly").and_then(Value::as_bool) == Some(true) {
            found.insert(path.clone());
        }
        collect_write_only(child, &path, found);
    }
}

/// A JSON Pointer as a dotted path, the way the manifest itself spells it
/// (`capabilities.source.position`).
fn dotted(pointer: &impl fmt::Display) -> String {
    pointer
        .to_string()
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

/// Every error, one per line, under no heading: what a `Display` impl that has to
/// render "all of them" needs.
pub fn render_errors(errors: &[ManifestError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which way a route uses a connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// The connector is the route's `from`.
    #[default]
    Source,
    /// The connector is the route's `to`.
    Sink,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Source => "source",
            Self::Sink => "sink",
        })
    }
}

/// The source mode a route uses (§33 D353's modes: streaming, batch, CDC, webhook).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Continuous, low-latency delivery.
    Streaming,
    /// Bulk or scheduled transfer.
    Batch,
    /// Row-level change capture (§33 §7, D357).
    Cdc,
    /// Webhooks received as a source.
    Webhook,
}

impl Mode {
    /// The capability name §33 §4 rule 1's refusal names.
    pub const fn capability(self) -> &'static str {
        match self {
            Self::Streaming => "source.streaming",
            Self::Batch => "source.batch",
            Self::Cdc => "cdc",
            Self::Webhook => "webhook",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.capability())
    }
}

/// What a route needs of its sink beyond the direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkRequirement {
    /// The route upserts, so the sink must declare `upsert`.
    Upsert,
    /// The route applies deletes.
    Delete,
    /// The route writes inside the target's transaction.
    Transactional,
}

impl SinkRequirement {
    /// The capability name §33 §4 rule 1's refusal names.
    pub const fn capability(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
            Self::Transactional => "transactional",
        }
    }
}

impl fmt::Display for SinkRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.capability())
    }
}

/// What a route wants of one connector, which the manifest has to declare all of.
///
/// §33 §4 rule 1: "ValidateRoute refuses a route whose `from` connector lacks the
/// source mode it uses (for example `cdc` on a polling source), whose `to` connector
/// lacks `upsert` when the route upserts, or whose delivery asks for more than the
/// connector declares."
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Uses {
    /// Which way the connector is used.
    pub direction: Direction,
    /// The source mode, when the direction is `Source`.
    pub mode: Option<Mode>,
    /// What the sink must offer, when the direction is `Sink`.
    pub sink_requirements: Vec<SinkRequirement>,
    /// The delivery guarantee the route relies on, per direction.
    pub delivery: Option<Delivery>,
    /// The ordering the route relies on.
    pub ordering: Option<Ordering>,
    /// Whether the route carries Arrow batches, which D356 makes the only bulk path.
    pub arrow_bulk: bool,
}

/// The strength of a delivery guarantee: `at_least_once` is above `at_most_once`,
/// and `exactly_once` is above both.
fn delivery_strength(delivery: Delivery) -> u8 {
    match delivery {
        Delivery::AtMostOnce => 0,
        Delivery::AtLeastOnce => 1,
        Delivery::ExactlyOnce => 2,
    }
}

/// How much ordering a value promises, in the order §33 §4's `ordering` comment
/// lists them. A route that relies on per-partition order needs at least
/// `per_partition`, as that comment says.
fn ordering_strength(ordering: Ordering) -> u8 {
    match ordering {
        Ordering::None => 0,
        Ordering::PerKey => 1,
        Ordering::PerPartition => 2,
        Ordering::Total => 3,
    }
}

/// §33 §4 rule 1: refuse a route that uses a capability the manifest does not
/// declare, naming the connector, the capability and the manifest version.
///
/// The checks run in the order §33 §4 rule 1 lists them — the source mode, then the
/// sink's requirement, then delivery, then ordering, then the bulk path — so the
/// refusal names the first thing a reader of the rule would expect.
pub fn check_use(spec: &ConnectorSpec, uses: &Uses) -> Result<(), CapabilityError> {
    let refuse = |capability: &str| {
        Err(CapabilityError {
            connector: spec.id.clone(),
            capability: capability.to_string(),
            spec_version: spec.spec_version.to_string(),
        })
    };

    match uses.direction {
        Direction::Source => {
            let Some(source) = spec.capabilities.source.as_ref() else {
                return refuse("source");
            };
            let mode = match uses.mode {
                Some(Mode::Streaming) if !source.streaming => Some("source.streaming"),
                Some(Mode::Batch) if !source.batch => Some("source.batch"),
                Some(Mode::Cdc) if !source.cdc => Some("cdc"),
                Some(Mode::Webhook) if !source.webhook => Some("webhook"),
                _ => None,
            };
            if let Some(capability) = mode {
                return refuse(capability);
            }
            if uses.arrow_bulk && !spec.capabilities.bulk.arrow {
                return refuse("bulk.arrow");
            }
            if let Some(asked) = uses.delivery
                && delivery_strength(asked) > delivery_strength(spec.capabilities.delivery.source)
            {
                return refuse("delivery.source");
            }
        }
        Direction::Sink => {
            let Some(sink) = spec.capabilities.sink.as_ref() else {
                return refuse("sink");
            };
            for requirement in &uses.sink_requirements {
                let declared = match requirement {
                    SinkRequirement::Upsert => sink.upsert,
                    SinkRequirement::Delete => sink.delete,
                    SinkRequirement::Transactional => sink.transactional,
                };
                if !declared {
                    return refuse(requirement.capability());
                }
            }
            if uses.arrow_bulk && !spec.capabilities.bulk.arrow {
                return refuse("bulk.arrow");
            }
            if let Some(asked) = uses.delivery
                && delivery_strength(asked) > delivery_strength(spec.capabilities.delivery.sink)
            {
                return refuse("delivery.sink");
            }
        }
    }

    if let Some(asked) = uses.ordering
        && ordering_strength(asked) > ordering_strength(spec.capabilities.ordering)
    {
        return refuse("ordering");
    }
    Ok(())
}

/// Design §33 D359's licence gate, over the ids `connectors/licences.toml` refuses.
///
/// D359: each manifest names the licence of its runtime component and of every
/// library or driver it loads, and CI refuses AGPL, BSL, SSPL, ELv2 and unlicensed
/// dependencies for anything Loams ships or runs by default. The refused set is read
/// from `connectors/licences.toml`'s `[deny]` rather than written here again, so the
/// gate and `scripts/connectors/gen_registry.py --check` cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenceGate {
    denied: Vec<String>,
}

/// Why the gate could not read its input, or what it refuses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LicenceError {
    /// `connectors/licences.toml` could not be read.
    #[error("{path}: cannot read the licence gate input: {message}")]
    Unreadable {
        /// The file's path.
        path: String,
        /// The I/O error.
        message: String,
    },
    /// The file is not the shape the gate reads.
    #[error("{path}: not a readable licence list: {message}")]
    Syntax {
        /// The file's path.
        path: String,
        /// What the reader could not make of it.
        message: String,
    },
    /// The gate's input is the file `origin` names, which every message repeats so a
    /// log line stands on its own.
    /// A component's own SPDX id is refused (D359).
    #[error("{connector}: licence.component is {id}, which the gate refuses ({origin})")]
    RefusedComponent {
        /// The manifest's id.
        connector: String,
        /// The refused SPDX id.
        id: String,
        /// Where the refused list is written down.
        origin: String,
    },
    /// A loaded dependency's SPDX id is refused (D359).
    #[error("{connector}: licence.dependencies.{name} is {id}, which the gate refuses ({origin})")]
    RefusedDependency {
        /// The manifest's id.
        connector: String,
        /// The dependency's name, as the manifest spells it.
        name: String,
        /// The refused SPDX id.
        id: String,
        /// Where the refused list is written down.
        origin: String,
    },
    /// A `[components.*]` stanza of the gate input carries a refused id, which
    /// `scripts/connectors/gen_registry.py --check` also fails on.
    #[error("{path}: components.{key} carries refused licence {id}")]
    RefusedListedComponent {
        /// The file's path.
        path: String,
        /// The `[components.<key>]` stanza.
        key: String,
        /// The refused SPDX id.
        id: String,
    },
}

impl LicenceGate {
    /// The gate over the `[deny]` list in a `connectors/licences.toml`.
    pub fn from_toml(text: &str, origin: &str) -> Result<Self, LicenceError> {
        let flat = parse_licences_toml(text).map_err(|message| LicenceError::Syntax {
            path: origin.to_string(),
            message,
        })?;
        let Some(ids) = flat.get("deny.ids") else {
            return Err(LicenceError::Syntax {
                path: origin.to_string(),
                message: "no [deny] ids list".to_string(),
            });
        };
        let denied = split_toml_array(ids);
        if denied.is_empty() {
            return Err(LicenceError::Syntax {
                path: origin.to_string(),
                message: "[deny] ids is empty, so the gate would refuse nothing (D359)".to_string(),
            });
        }
        Ok(Self { denied })
    }

    /// The gate over `connectors/licences.toml`.
    pub fn load(path: &Path) -> Result<Self, LicenceError> {
        let origin = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|error| LicenceError::Unreadable {
            path: origin.clone(),
            message: error.to_string(),
        })?;
        Self::from_toml(&text, &origin)
    }

    /// The gate over this repository's `connectors/licences.toml`.
    pub fn repository() -> Result<Self, LicenceError> {
        Self::load(&default_licences_path())
    }

    /// The refused ids, in the order `connectors/licences.toml` lists them.
    pub fn denied(&self) -> &[String] {
        &self.denied
    }

    /// Whether `id` is refused.
    pub fn refuses(&self, id: &str) -> bool {
        self.denied.iter().any(|denied| denied == id)
    }

    /// D359 applied to one manifest: its component and every loaded dependency.
    pub fn check_manifest(&self, spec: &ConnectorSpec, origin: &str) -> Result<(), LicenceError> {
        if self.refuses(&spec.licence.component) {
            return Err(LicenceError::RefusedComponent {
                connector: spec.id.clone(),
                id: spec.licence.component.clone(),
                origin: origin.to_string(),
            });
        }
        for (name, id) in &spec.licence.dependencies {
            if self.refuses(id) {
                return Err(LicenceError::RefusedDependency {
                    connector: spec.id.clone(),
                    name: name.clone(),
                    id: id.clone(),
                    origin: origin.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Every `[components.*]` stanza's SPDX id, by stanza key.
    pub fn components(text: &str, origin: &str) -> Result<BTreeMap<String, String>, LicenceError> {
        let flat = parse_licences_toml(text).map_err(|message| LicenceError::Syntax {
            path: origin.to_string(),
            message,
        })?;
        Ok(flat
            .iter()
            .filter_map(|(key, id)| {
                let stanza = key.strip_prefix("components.")?;
                let name = stanza.strip_suffix(".spdx")?;
                Some((name.to_string(), id.clone()))
            })
            .collect())
    }

    /// No `[components.*]` stanza of the gate input may itself carry a refused id:
    /// a component Loams names with an AGPL or BSL id is refused however no manifest
    /// uses it. `scripts/connectors/gen_registry.py --check` asserts the same.
    pub fn check_components(&self, text: &str, origin: &str) -> Result<(), LicenceError> {
        for (key, id) in Self::components(text, origin)? {
            if self.refuses(&id) {
                return Err(LicenceError::RefusedListedComponent {
                    path: origin.to_string(),
                    key,
                    id,
                });
            }
        }
        Ok(())
    }
}

/// The parts of `connectors/licences.toml` the gate reads, as `section.key -> value`.
///
/// A small hand-rolled reader, not a TOML dependency: the gate needs `[deny] ids`
/// and each `[components.*]` stanza's `spdx`, and a crate for those two keys would
/// be a new dependency in a crate whose whole job is to hold the repository's
/// licence decisions (D359). Comments are stripped outside quotes, multi-line arrays
/// are joined, and a double- or single-quoted scalar is unquoted.
pub fn parse_licences_toml(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut flat = BTreeMap::new();
    let mut section = String::new();
    let mut pending: Option<(String, String)> = None;
    for (number, raw) in text.lines().enumerate() {
        let line = strip_toml_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some((_, value)) = pending.as_mut() {
            value.push(' ');
            value.push_str(&line);
            if value.contains(']') {
                let (key, value) = match pending.take() {
                    Some(pending) => pending,
                    None => continue,
                };
                flat.insert(format!("{section}.{key}"), value);
            }
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: no `=` in {line:?}", number + 1));
        };
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if value.starts_with('[') {
            if value.contains(']') {
                flat.insert(format!("{section}.{key}"), value);
            } else {
                pending = Some((key, value));
            }
            continue;
        }
        flat.insert(format!("{section}.{key}"), unquote_toml(&value));
    }
    if pending.is_some() {
        return Err("a `[...]` array is never closed".to_string());
    }
    if flat.is_empty() {
        return Err("no table or key found".to_string());
    }
    Ok(flat)
}

/// A `#` comment ends a line unless it is inside a quoted string.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, character) in line.char_indices() {
        match (quote, character) {
            (None, '"') | (None, '\'') => quote = Some(character),
            (Some(open), character) if open == character => quote = None,
            (None, '#') => return &line[..index],
            _ => {}
        }
    }
    line
}

/// The members of a TOML array, unquoted and in order.
fn split_toml_array(array: &str) -> Vec<String> {
    let inner = array
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    inner
        .split(',')
        .map(|item| unquote_toml(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
}

/// A TOML scalar with its quotes removed; anything else is returned as it stands.
fn unquote_toml(value: &str) -> String {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// Every error of a failed load, ready to print: `Registry::load`'s `Display`.
pub fn render_manifest_errors(files: &[ManifestErrors]) -> String {
    files
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    //! The licence gate's reader is the only hand-rolled parser in the crate, so it
    //! is checked against the shape of `connectors/licences.toml` it must read.

    use super::*;

    const TOML: &str = r#"
# a comment, and one after a value
[deny]
ids = [
  "AGPL-3.0-only",
  "BUSL-1.1",
  "unknown",
]

[components.kafka]
name = "Apache Kafka"   # not a runtime Loams supervises by itself
spdx = "Apache-2.0"
kind = "runtime"
used_by = ["kafka", "kafka-connect"]
"#;

    #[test]
    fn the_licence_reader_reads_deny_and_the_component_ids() {
        let gate = LicenceGate::from_toml(TOML, "licences.toml")
            .unwrap_or_else(|error| panic!("the gate must read the file: {error}"));
        assert_eq!(gate.denied(), ["AGPL-3.0-only", "BUSL-1.1", "unknown"]);
        assert!(gate.refuses("AGPL-3.0-only"));
        assert!(!gate.refuses("Apache-2.0"));
        let components = LicenceGate::components(TOML, "licences.toml")
            .unwrap_or_else(|error| panic!("the components must read: {error}"));
        assert_eq!(
            components.get("kafka").map(String::as_str),
            Some("Apache-2.0"),
            "only spdx is read: {components:?}"
        );
    }

    #[test]
    fn a_gate_with_no_denied_ids_is_refused() {
        let error = LicenceGate::from_toml("[deny]\nids = []\n", "licences.toml").err();
        assert!(
            error.is_some(),
            "an empty [deny] would refuse nothing (D359)"
        );
    }
}
