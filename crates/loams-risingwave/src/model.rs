//! The cluster model (D1950): an instance key, the four node roles, sizes, the
//! per-role topology a size resolves to, and engine versions.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// Why a model value was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error(
        "instance key {0:?} is not [a-z0-9-]{{1,40}} starting with a letter or digit and not ending with '-'"
    )]
    InstanceKey(String),
    #[error("config {key}: {msg}")]
    Config { key: String, msg: String },
    #[error("version {0:?} is not vMAJOR.MINOR.PATCH")]
    Version(String),
    #[error("size {0:?} is not one of xs, s, m, l, xl")]
    Size(String),
    #[error("{role}: {msg}")]
    Topology { role: Role, msg: String },
    #[error("upgrade {from} -> {to}: {msg}")]
    Upgrade {
        from: EngineVersion,
        to: EngineVersion,
        msg: String,
    },
}

/// The runtime key of a managed instance: the 26-character lowercase
/// base32 part of its id `eng_<26>` (ENG81 R2, D1903). Every runtime name
/// derives from it (`rw-<key>-meta`, the meta database `rw_<key>`), so it is
/// a DNS label and, with `-` mapped to `_`, part of SQL identifiers. Tests
/// and the single-tenant compose stack may use any `[a-z0-9-]{1,40}` label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstanceKey(String);

impl InstanceKey {
    /// The longest key: `rw-<key>-compactor-NN` stays a DNS label (63).
    pub const MAX_LEN: usize = 40;

    pub fn new(id: impl Into<String>) -> Result<Self, ModelError> {
        let id = id.into();
        let bytes = id.as_bytes();
        let ok = !bytes.is_empty()
            && bytes.len() <= Self::MAX_LEN
            && bytes[0].is_ascii_alphanumeric()
            && !id.ends_with('-')
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
        if ok {
            Ok(Self(id))
        } else {
            Err(ModelError::InstanceKey(id))
        }
    }

    /// The key of an instance id `eng_<26 lowercase base32>`.
    pub fn from_instance_id(id: &str) -> Result<Self, ModelError> {
        let rest = id
            .strip_prefix("eng_")
            .ok_or_else(|| ModelError::InstanceKey(id.to_owned()))?;
        if rest.len() != 26 {
            return Err(ModelError::InstanceKey(id.to_owned()));
        }
        Self::new(rest.to_ascii_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key as part of a SQL identifier: `-` becomes `_`. Keys hold no
    /// `_`, so the mapping is one to one.
    pub fn sql_part(&self) -> String {
        self.0.replace('-', "_")
    }
}

impl fmt::Display for InstanceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for InstanceKey {
    type Error = ModelError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<InstanceKey> for String {
    fn from(value: InstanceKey) -> Self {
        value.0
    }
}

/// What the adapter needs to know of a managed instance (ENG-UI's
/// `Instance`, ENG81 R2): its id, the key derived from it, its name and
/// Loams namespace (the tenancy unit), and its state prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceRef {
    /// `eng_<26>`.
    pub id: String,
    pub key: InstanceKey,
    /// The instance's DNS-label name, unique per namespace.
    pub name: String,
    /// The Loams namespace (OpenFGA `environment`).
    pub namespace: String,
    /// `engines/<ns>/<id>/`.
    pub state_prefix: String,
}

impl InstanceRef {
    pub fn new(
        id: &str,
        name: &str,
        namespace: &str,
        state_prefix: &str,
    ) -> Result<Self, ModelError> {
        Ok(Self {
            id: id.to_owned(),
            key: InstanceKey::from_instance_id(id)?,
            name: name.to_owned(),
            namespace: namespace.to_owned(),
            state_prefix: state_prefix.to_owned(),
        })
    }
}

/// A RisingWave node role. The order of [`Role::ROLLOUT`] is the order of
/// every start and upgrade (meta first, frontend last), and the reverse is
/// the order of every stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Meta,
    Compactor,
    Compute,
    Frontend,
}

impl Role {
    /// Start and upgrade order (D1955).
    pub const ROLLOUT: [Role; 4] = [Role::Meta, Role::Compactor, Role::Compute, Role::Frontend];

    pub fn name(self) -> &'static str {
        match self {
            Role::Meta => "meta",
            Role::Compactor => "compactor",
            Role::Compute => "compute",
            Role::Frontend => "frontend",
        }
    }

    /// The `risingwave` subcommand that runs the role.
    pub fn subcommand(self) -> &'static str {
        match self {
            Role::Meta => "meta-node",
            Role::Compactor => "compactor-node",
            Role::Compute => "compute-node",
            Role::Frontend => "frontend-node",
        }
    }

    /// The role's main listener (RPC, or PG wire for the frontend).
    pub fn port(self) -> u16 {
        match self {
            Role::Meta => ports::META,
            Role::Compactor => ports::COMPACTOR,
            Role::Compute => ports::COMPUTE,
            Role::Frontend => ports::PGWIRE,
        }
    }

    /// `common.WorkerType` as the meta dashboard's `/api/clusters/{ty}`
    /// takes it.
    pub fn worker_type(self) -> u8 {
        match self {
            Role::Frontend => 1,
            Role::Compute => 2,
            Role::Compactor => 4,
            Role::Meta => 5,
        }
    }

    /// Roles that keep a stable identity (StatefulSet in Kubernetes).
    pub fn is_stateful(self) -> bool {
        matches!(self, Role::Meta | Role::Compute)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The ports RisingWave listens on.
pub mod ports {
    /// The frontend's PG wire.
    pub const PGWIRE: u16 = 4566;
    /// The meta node's gRPC.
    pub const META: u16 = 5690;
    /// The meta node's dashboard and JSON API.
    pub const DASHBOARD: u16 = 5691;
    pub const COMPUTE: u16 = 5688;
    pub const COMPACTOR: u16 = 6660;
    /// The frontend's webhook source listener (not routed by Loams).
    pub const WEBHOOK: u16 = 4560;
    /// Every role's Prometheus listener (D1959; Loams Meter scrapes it).
    pub const METRICS: u16 = 1250;
}

/// CPU and memory for one replica of a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    /// Thousandths of a vCPU.
    pub cpu_milli: u32,
    pub memory_bytes: u64,
}

impl Resources {
    pub const fn new(cpu_milli: u32, memory_bytes: u64) -> Self {
        Self {
            cpu_milli,
            memory_bytes,
        }
    }

    /// Whole vCPUs, at least one: compute's `--parallelism`.
    pub fn vcpus(&self) -> u32 {
        self.cpu_milli.div_ceil(1000).max(1)
    }
}

/// One role's replicas and per-replica resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpec {
    pub replicas: u32,
    pub resources: Resources,
}

/// Size tiers, ENG-UI's `SizeTier` (D2011, ENG81 R19): `xs`..`xl`. A tier
/// fixes every role's layout here; the instance's `EngineSize` then sets
/// the compute nodes (replicas and per-replica shape) and `config` may
/// override the other roles ([`parse_config`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Xs,
    S,
    M,
    L,
    Xl,
}

impl Size {
    pub const ALL: [Size; 5] = [Size::Xs, Size::S, Size::M, Size::L, Size::Xl];

    pub fn name(self) -> &'static str {
        match self {
            Size::Xs => "xs",
            Size::S => "s",
            Size::M => "m",
            Size::L => "l",
            Size::Xl => "xl",
        }
    }

    /// The tier's default for a role. Compute's per-replica shape is the
    /// tier's (xs = 500m/1 GiB, s = 1/4, m = 2/8, l = 4/16, xl = 8/32).
    pub fn role(self, role: Role) -> RoleSpec {
        let (replicas, cpu_milli, memory_bytes) = match (self, role) {
            (Size::Xs, Role::Meta) => (1, 500, GIB),
            (Size::Xs, Role::Frontend) => (1, 500, GIB),
            (Size::Xs, Role::Compute) => (1, 500, GIB),
            (Size::Xs, Role::Compactor) => (1, 500, GIB),

            (Size::S, Role::Meta) => (1, 1000, 2 * GIB),
            (Size::S, Role::Frontend) => (1, 1000, 2 * GIB),
            (Size::S, Role::Compute) => (1, 1000, 4 * GIB),
            (Size::S, Role::Compactor) => (1, 1000, 2 * GIB),

            (Size::M, Role::Meta) => (1, 1000, 2 * GIB),
            (Size::M, Role::Frontend) => (1, 2000, 4 * GIB),
            (Size::M, Role::Compute) => (2, 2000, 8 * GIB),
            (Size::M, Role::Compactor) => (1, 2000, 4 * GIB),

            (Size::L, Role::Meta) => (1, 2000, 4 * GIB),
            (Size::L, Role::Frontend) => (2, 2000, 4 * GIB),
            (Size::L, Role::Compute) => (2, 4000, 16 * GIB),
            (Size::L, Role::Compactor) => (2, 2000, 8 * GIB),

            (Size::Xl, Role::Meta) => (3, 2000, 4 * GIB),
            (Size::Xl, Role::Frontend) => (2, 4000, 8 * GIB),
            (Size::Xl, Role::Compute) => (4, 8000, 32 * GIB),
            (Size::Xl, Role::Compactor) => (2, 4000, 8 * GIB),
        };
        RoleSpec {
            replicas,
            resources: Resources::new(cpu_milli, memory_bytes),
        }
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Size {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Size::ALL
            .into_iter()
            .find(|size| size.name() == s)
            .ok_or_else(|| ModelError::Size(s.to_owned()))
    }
}

/// A change to one role of a size's default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleOverride {
    #[serde(default)]
    pub replicas: Option<u32>,
    #[serde(default)]
    pub cpu_milli: Option<u32>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
}

/// Per-role overrides of a size.
pub type RoleOverrides = BTreeMap<Role, RoleOverride>;

/// What every role runs: a size with its overrides applied, validated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    roles: BTreeMap<Role, RoleSpec>,
}

impl Topology {
    /// The most replicas a role may have.
    pub const MAX_REPLICAS: u32 = 64;

    pub fn resolve(size: Size, overrides: &RoleOverrides) -> Result<Self, ModelError> {
        let mut roles = BTreeMap::new();
        for role in Role::ROLLOUT {
            let mut spec = size.role(role);
            if let Some(o) = overrides.get(&role) {
                if let Some(r) = o.replicas {
                    spec.replicas = r;
                }
                if let Some(c) = o.cpu_milli {
                    spec.resources.cpu_milli = c;
                }
                if let Some(m) = o.memory_bytes {
                    spec.resources.memory_bytes = m;
                }
            }
            check_role(role, &spec)?;
            roles.insert(role, spec);
        }
        Ok(Self { roles })
    }

    pub fn role(&self, role: Role) -> RoleSpec {
        self.roles[&role]
    }

    pub fn iter(&self) -> impl Iterator<Item = (Role, RoleSpec)> + '_ {
        Role::ROLLOUT.into_iter().map(|r| (r, self.roles[&r]))
    }

    /// Total vCPU thousandths and memory bytes of the running cluster
    /// (metering input).
    pub fn totals(&self) -> Resources {
        let mut cpu = 0u32;
        let mut mem = 0u64;
        for spec in self.roles.values() {
            cpu = cpu.saturating_add(spec.resources.cpu_milli.saturating_mul(spec.replicas));
            mem = mem.saturating_add(
                spec.resources
                    .memory_bytes
                    .saturating_mul(spec.replicas as u64),
            );
        }
        Resources::new(cpu, mem)
    }
}

fn check_role(role: Role, spec: &RoleSpec) -> Result<(), ModelError> {
    let err = |msg: String| ModelError::Topology { role, msg };
    let min_replicas = 1;
    let max_replicas = if role == Role::Meta {
        3
    } else {
        Topology::MAX_REPLICAS
    };
    if spec.replicas < min_replicas || spec.replicas > max_replicas {
        return Err(err(format!(
            "replicas {} outside {min_replicas}..={max_replicas}",
            spec.replicas
        )));
    }
    if spec.resources.cpu_milli < 250 {
        return Err(err(format!(
            "cpu_milli {} below 250",
            spec.resources.cpu_milli
        )));
    }
    let min_mem = match role {
        Role::Compute => GIB,
        _ => 512 * MIB,
    };
    if spec.resources.memory_bytes < min_mem {
        return Err(err(format!(
            "memory_bytes {} below {min_mem}",
            spec.resources.memory_bytes
        )));
    }
    Ok(())
}

/// A RisingWave release, `vMAJOR.MINOR.PATCH`. Pre-releases and nightlies
/// are refused: managed clusters run releases only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EngineVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl EngineVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The image tag.
    pub fn tag(&self) -> String {
        self.to_string()
    }

    /// Forward only, at most one major at a time (D1955).
    pub fn check_upgrade_to(&self, to: EngineVersion) -> Result<(), ModelError> {
        let err = |msg: &str| ModelError::Upgrade {
            from: *self,
            to,
            msg: msg.to_owned(),
        };
        if to == *self {
            return Err(err("already at this version"));
        }
        if to < *self {
            return Err(err("downgrades are not supported"));
        }
        if to.major > self.major + 1 {
            return Err(err("a major version may not be skipped"));
        }
        Ok(())
    }
}

impl fmt::Display for EngineVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for EngineVersion {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ModelError::Version(s.to_owned());
        let rest = s.strip_prefix('v').ok_or_else(bad)?;
        let mut parts = rest.split('.');
        let mut next = || -> Result<u32, ModelError> {
            let p = parts.next().ok_or_else(bad)?;
            if p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
            {
                return Err(bad());
            }
            p.parse().map_err(|_| bad())
        };
        let v = EngineVersion::new(next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(v)
    }
}

impl TryFrom<String> for EngineVersion {
    type Error = ModelError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<EngineVersion> for String {
    fn from(value: EngineVersion) -> Self {
        value.to_string()
    }
}

/// Addresses of other Loams services the cluster's connectors use. RW-a
/// only carries them (RW-b's connectors read them).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integrations {
    /// The Iggy Kafka gateway, `host:port`.
    #[serde(default)]
    pub kafka_bootstrap: Option<String>,
    /// Lakekeeper's Iceberg REST catalog.
    #[serde(default)]
    pub iceberg_rest_uri: Option<String>,
}

/// The cluster an instance runs: the adapter builds it from the instance
/// (tier, compute size, version, `config`) and renders from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterSpec {
    #[serde(default = "default_version")]
    pub version: EngineVersion,
    #[serde(default = "default_size")]
    pub size: Size,
    #[serde(default)]
    pub overrides: RoleOverrides,
    #[serde(default)]
    pub integrations: Integrations,
    /// Embedded JavaScript and WebAssembly UDFs run inside compute nodes;
    /// off unless asked for (Python embedded UDFs stay off).
    #[serde(default)]
    pub embedded_udfs: bool,
}

impl ClusterSpec {
    pub fn new(size: Size) -> Self {
        Self {
            version: default_version(),
            size,
            overrides: RoleOverrides::new(),
            integrations: Integrations::default(),
            embedded_udfs: false,
        }
    }

    pub fn topology(&self) -> Result<Topology, ModelError> {
        Topology::resolve(self.size, &self.overrides)
    }
}

/// The compute nodes an instance asked for: ENG-UI's `EngineSize`
/// (`replicas` = compute nodes, the shape per replica).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputeSize {
    pub tier: Size,
    pub replicas: u32,
    pub vcpu_milli: u32,
    pub memory_mib: u32,
}

/// The `config` keys an instance may set (ENG81 R19):
///
/// - `meta.replicas` (1 or 3), `frontend.replicas`, `compactor.replicas`;
/// - `<role>.vcpu_milli` and `<role>.memory_mib` for meta, frontend and
///   compactor (compute comes from the instance's size);
/// - `embedded_udfs` (`true`/`false`, default `false`).
///
/// Anything else is refused, so a typo never passes silently.
pub fn parse_config(
    config: &BTreeMap<String, String>,
) -> Result<(RoleOverrides, bool), ModelError> {
    let mut overrides = RoleOverrides::new();
    let mut udfs = false;
    for (key, value) in config {
        let bad = |msg: &str| ModelError::Config {
            key: key.clone(),
            msg: msg.to_owned(),
        };
        if key == "embedded_udfs" {
            udfs = match value.as_str() {
                "true" => true,
                "false" => false,
                _ => return Err(bad("is true or false")),
            };
            continue;
        }
        let (role, field) = key.split_once('.').ok_or_else(|| bad("unknown key"))?;
        let role = match role {
            "meta" => Role::Meta,
            "frontend" => Role::Frontend,
            "compactor" => Role::Compactor,
            "compute" => return Err(bad("compute nodes follow the instance's size")),
            _ => return Err(bad("unknown key")),
        };
        let number: u64 = value
            .parse()
            .map_err(|_| bad("is a non-negative integer"))?;
        let small = |n: u64| u32::try_from(n).map_err(|_| bad("is too large"));
        let o = overrides.entry(role).or_default();
        match field {
            "replicas" => o.replicas = Some(small(number)?),
            "vcpu_milli" => o.cpu_milli = Some(small(number)?),
            "memory_mib" => {
                o.memory_bytes = Some(number.checked_mul(MIB).ok_or_else(|| bad("is too large"))?)
            }
            _ => return Err(bad("unknown key")),
        }
    }
    Ok((overrides, udfs))
}

impl ClusterSpec {
    /// The cluster of an instance: its tier's layout, its compute size, its
    /// version (the default when `None`) and its `config`.
    pub fn from_parts(
        compute: ComputeSize,
        version: Option<&str>,
        config: &BTreeMap<String, String>,
    ) -> Result<Self, ModelError> {
        let (mut overrides, embedded_udfs) = parse_config(config)?;
        overrides.insert(
            Role::Compute,
            RoleOverride {
                replicas: Some(compute.replicas),
                cpu_milli: Some(compute.vcpu_milli),
                memory_bytes: Some(u64::from(compute.memory_mib) * MIB),
            },
        );
        let spec = Self {
            version: match version {
                Some(v) => v.parse()?,
                None => DEFAULT_VERSION,
            },
            size: compute.tier,
            overrides,
            integrations: Integrations::default(),
            embedded_udfs,
        };
        spec.topology()?;
        Ok(spec)
    }
}

/// The release new clusters get (D1958): v3.1.0, published 2026-09-21.
pub const DEFAULT_VERSION: EngineVersion = EngineVersion::new(3, 1, 0);

fn default_version() -> EngineVersion {
    DEFAULT_VERSION
}

fn default_size() -> Size {
    Size::Xs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_keys() {
        for ok in ["acme", "a", "p-01", "0abc", &"x".repeat(40)] {
            assert!(InstanceKey::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-a", "a-", "A", "a_b", "a.b", &"x".repeat(41)] {
            assert!(InstanceKey::new(bad).is_err(), "{bad}");
        }
        assert_eq!(InstanceKey::new("p-01").unwrap().sql_part(), "p_01");
        let key = InstanceKey::from_instance_id("eng_01jb2v3c4d5e6f7g8h9j0k1m2n").unwrap();
        assert_eq!(key.as_str(), "01jb2v3c4d5e6f7g8h9j0k1m2n");
        assert!(InstanceKey::from_instance_id("eng_short").is_err());
        assert!(InstanceKey::from_instance_id("01jb2v3c4d5e6f7g8h9j0k1m2n").is_err());
    }

    #[test]
    fn versions_parse_and_print() {
        let v: EngineVersion = "v3.1.0".parse().unwrap();
        assert_eq!(v, EngineVersion::new(3, 1, 0));
        assert_eq!(v.to_string(), "v3.1.0");
        for bad in [
            "3.1.0",
            "v3.1",
            "v3.1.0.1",
            "v3.1.0-rc.1",
            "v03.1.0",
            "v3..0",
            "nightly",
        ] {
            assert!(bad.parse::<EngineVersion>().is_err(), "{bad}");
        }
    }

    #[test]
    fn upgrade_rules() {
        let v = EngineVersion::new(3, 1, 0);
        assert!(v.check_upgrade_to(EngineVersion::new(3, 1, 1)).is_ok());
        assert!(v.check_upgrade_to(EngineVersion::new(3, 2, 0)).is_ok());
        assert!(v.check_upgrade_to(EngineVersion::new(4, 0, 0)).is_ok());
        assert!(v.check_upgrade_to(EngineVersion::new(5, 0, 0)).is_err());
        assert!(v.check_upgrade_to(EngineVersion::new(3, 0, 5)).is_err());
        assert!(v.check_upgrade_to(v).is_err());
    }

    #[test]
    fn every_size_resolves() {
        for size in Size::ALL {
            let t = Topology::resolve(size, &RoleOverrides::new()).unwrap();
            assert_eq!(t.iter().count(), 4);
            assert_eq!(size.name().parse::<Size>().unwrap(), size);
        }
        let large = Topology::resolve(Size::Xl, &RoleOverrides::new()).unwrap();
        assert_eq!(large.role(Role::Meta).replicas, 3);
    }

    #[test]
    fn overrides_apply_and_are_checked() {
        let mut o = RoleOverrides::new();
        o.insert(
            Role::Compute,
            RoleOverride {
                replicas: Some(3),
                ..Default::default()
            },
        );
        let t = Topology::resolve(Size::Xs, &o).unwrap();
        assert_eq!(t.role(Role::Compute).replicas, 3);
        assert_eq!(
            t.role(Role::Compute).resources,
            Size::Xs.role(Role::Compute).resources
        );

        o.insert(
            Role::Meta,
            RoleOverride {
                replicas: Some(4),
                ..Default::default()
            },
        );
        assert!(Topology::resolve(Size::Xs, &o).is_err());

        let mut o = RoleOverrides::new();
        o.insert(
            Role::Compute,
            RoleOverride {
                memory_bytes: Some(512 * MIB),
                ..Default::default()
            },
        );
        assert!(matches!(
            Topology::resolve(Size::Xs, &o),
            Err(ModelError::Topology {
                role: Role::Compute,
                ..
            })
        ));
    }

    #[test]
    fn totals_sum_replicas() {
        let t = Topology::resolve(Size::Xs, &RoleOverrides::new()).unwrap();
        assert_eq!(t.totals(), Resources::new(2000, 4 * GIB));
    }

    #[test]
    fn spec_defaults_from_json() {
        let spec: ClusterSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec, ClusterSpec::new(Size::Xs));
        let spec: ClusterSpec = serde_json::from_str(
            r#"{"version":"v3.0.5","size":"s","overrides":{"compute":{"replicas":3}}}"#,
        )
        .unwrap();
        assert_eq!(spec.version, EngineVersion::new(3, 0, 5));
        assert_eq!(spec.topology().unwrap().role(Role::Compute).replicas, 3);
    }

    fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn config_keys() {
        let (o, udfs) = parse_config(&config(&[
            ("meta.replicas", "3"),
            ("frontend.memory_mib", "2048"),
            ("embedded_udfs", "true"),
        ]))
        .unwrap();
        assert!(udfs);
        assert_eq!(o[&Role::Meta].replicas, Some(3));
        assert_eq!(o[&Role::Frontend].memory_bytes, Some(2 * GIB));
        for bad in [
            ("compute.replicas", "2"),
            ("meta.colour", "1"),
            ("nope", "1"),
            ("meta.replicas", "-1"),
            ("embedded_udfs", "yes"),
        ] {
            assert!(parse_config(&config(&[bad])).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn spec_from_instance_parts() {
        let compute = ComputeSize {
            tier: Size::M,
            replicas: 3,
            vcpu_milli: 2000,
            memory_mib: 8192,
        };
        let spec =
            ClusterSpec::from_parts(compute, None, &config(&[("meta.replicas", "3")])).unwrap();
        assert_eq!(spec.version, DEFAULT_VERSION);
        let t = spec.topology().unwrap();
        assert_eq!(t.role(Role::Compute).replicas, 3);
        assert_eq!(
            t.role(Role::Compute).resources,
            Resources::new(2000, 8 * GIB)
        );
        assert_eq!(t.role(Role::Meta).replicas, 3);
        assert_eq!(t.role(Role::Frontend), Size::M.role(Role::Frontend));
        assert!(ClusterSpec::from_parts(compute, Some("v3.1"), &BTreeMap::new()).is_err());
        let tiny = ComputeSize {
            memory_mib: 256,
            ..compute
        };
        assert!(ClusterSpec::from_parts(tiny, None, &BTreeMap::new()).is_err());
    }
}
