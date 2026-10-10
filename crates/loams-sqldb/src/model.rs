//! Shared model (plan SQ1 "Shared contracts"): classes, branch ids and the
//! endpoints a `tidb-server` pool is rendered against.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A value the model refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what}: {value:?} {why}")]
pub struct ModelError {
    what: &'static str,
    value: String,
    why: &'static str,
}

impl ModelError {
    fn new(what: &'static str, value: &str, why: &'static str) -> Self {
        Self {
            what,
            value: value.to_owned(),
            why,
        }
    }
}

/// A compute class (§47 §15, D734; ruling R2.1). Sizes one `tidb-server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    /// 0.25 vCPU, 0.75 GiB.
    Xs,
    /// 0.5 vCPU, 1 GiB.
    S,
    /// 1 vCPU, 2 GiB.
    M,
    /// 2 vCPU, 4 GiB.
    L,
    /// 4 vCPU, 8 GiB.
    Xl,
    /// 8 vCPU, 16 GiB.
    #[serde(rename = "2xl")]
    Xxl,
}

impl Class {
    /// Every class, smallest first.
    pub const ALL: [Class; 6] = [
        Class::Xs,
        Class::S,
        Class::M,
        Class::L,
        Class::Xl,
        Class::Xxl,
    ];

    /// The class name used in the API and in labels.
    pub fn name(self) -> &'static str {
        match self {
            Class::Xs => "xs",
            Class::S => "s",
            Class::M => "m",
            Class::L => "l",
            Class::Xl => "xl",
            Class::Xxl => "2xl",
        }
    }

    /// CPU per `tidb-server`, in thousandths of a vCPU.
    pub fn vcpu_millis(self) -> u32 {
        match self {
            Class::Xs => 250,
            Class::S => 500,
            Class::M => 1_000,
            Class::L => 2_000,
            Class::Xl => 4_000,
            Class::Xxl => 8_000,
        }
    }

    /// The memory limit of one `tidb-server` (container or pod), in MiB.
    /// `xs` is 0.75 GiB (R2.1: TiDB peaked at 493 MiB under load, R1.5).
    pub fn memory_mib(self) -> u64 {
        match self {
            Class::Xs => 768,
            Class::S => 1_024,
            Class::M => 2_048,
            Class::L => 4_096,
            Class::Xl => 8_192,
            Class::Xxl => 16_384,
        }
    }

    /// The memory limit in bytes.
    pub fn memory_bytes(self) -> u64 {
        self.memory_mib() << 20
    }

    /// What `tidb_server_memory_limit = '80%'` comes to, whole MiB rounded
    /// down (R2.1; the class table). TiDB computes it from its cgroup limit.
    pub fn server_memory_limit_mib(self) -> u64 {
        self.memory_mib() * 4 / 5
    }

    /// `tidb_mem_quota_query`: 40 % of the memory limit, whole MiB rounded
    /// down, in bytes (R2.1; Task 24 tunes it).
    pub fn mem_quota_query_bytes(self) -> u64 {
        (self.memory_mib() * 2 / 5) << 20
    }

    /// Connections the gate admits per database (§47 §15).
    pub fn gate_connections(self) -> u32 {
        match self {
            Class::Xs => 100,
            Class::S => 200,
            Class::M => 500,
            Class::L => 1_000,
            Class::Xl => 2_000,
            Class::Xxl => 4_000,
        }
    }

    /// Pods per pool, minimum and maximum (§47 §15).
    pub fn pods(self) -> (u32, u32) {
        match self {
            Class::Xs | Class::S | Class::M => (0, 1),
            Class::L => (0, 2),
            Class::Xl => (1, 4),
            Class::Xxl => (1, 8),
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Class {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Class::ALL
            .into_iter()
            .find(|c| c.name() == s)
            .ok_or_else(|| ModelError::new("class", s, "is not xs, s, m, l, xl or 2xl"))
    }
}

/// A branch id, `br_` + 16 characters of `[0-9a-z]`. It is also the branch's
/// PD keyspace name (PD allows `^[-A-Za-z0-9_]{1,20}$`), so a `tidb-server`
/// is never rendered without `keyspace-name` (D260).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BranchId(String);

impl BranchId {
    /// Parses and checks a branch id.
    pub fn parse(s: &str) -> Result<Self, ModelError> {
        let ok = s.len() == 19
            && s.starts_with("br_")
            && s.bytes()
                .skip(3)
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(ModelError::new(
                "branch id",
                s,
                "is not br_ + 16 of [0-9a-z]",
            ))
        }
    }

    /// The id, which is also the keyspace name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for BranchId {
    type Error = ModelError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<BranchId> for String {
    fn from(b: BranchId) -> Self {
        b.0
    }
}

/// An IPv4 or IPv6 network, host bits cleared. `0.0.0.0/0`, `::/0` and `*`
/// are refused: TiDB must take PROXY headers from the gate's network only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl FromStr for Cidr {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |why| ModelError::new("network", s, why);
        let (a, p) = s
            .split_once('/')
            .ok_or_else(|| err("is not <address>/<prefix>"))?;
        let addr: IpAddr = a.parse().map_err(|_| err("has no valid address"))?;
        let prefix: u8 = p.parse().map_err(|_| err("has no valid prefix"))?;
        let addr = match addr {
            IpAddr::V4(v4) if prefix <= 32 => {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                IpAddr::V4((u32::from(v4) & mask).into())
            }
            IpAddr::V6(v6) if prefix <= 128 => {
                let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
                IpAddr::V6((u128::from(v6) & mask).into())
            }
            _ => return Err(err("has a prefix longer than the address")),
        };
        if prefix == 0 {
            return Err(err("admits every address"));
        }
        Ok(Self { addr, prefix })
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// What a pool's `tidb.toml` points at: PD, the gate's networks (PROXY
/// protocol) and whether TiDB ↔ PD/TiKV traffic uses TLS. Certificate and
/// key files sit at fixed paths in the container ([`crate::render`]); this
/// type holds no secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pd: Vec<String>,
    gate_networks: Vec<Cidr>,
    cluster_tls: bool,
}

impl Endpoints {
    /// `pd`: PD client addresses as `host:port`; `gate_networks`: the
    /// networks `loams-sqlgate` connects from. Both non-empty.
    pub fn new(pd: Vec<String>, gate_networks: Vec<Cidr>) -> Result<Self, ModelError> {
        if pd.is_empty() {
            return Err(ModelError::new("pd", "", "needs at least one address"));
        }
        for a in &pd {
            check_host_port(a)?;
        }
        if gate_networks.is_empty() {
            return Err(ModelError::new(
                "gate networks",
                "",
                "needs at least one network",
            ));
        }
        Ok(Self {
            pd,
            gate_networks,
            cluster_tls: false,
        })
    }

    /// TLS from TiDB to PD and TiKV (`cluster-ssl-*`).
    #[must_use]
    pub fn with_cluster_tls(mut self, on: bool) -> Self {
        self.cluster_tls = on;
        self
    }

    /// PD client addresses.
    pub fn pd(&self) -> &[String] {
        &self.pd
    }

    /// The gate's networks.
    pub fn gate_networks(&self) -> &[Cidr] {
        &self.gate_networks
    }

    /// Whether TiDB ↔ PD/TiKV uses TLS.
    pub fn cluster_tls(&self) -> bool {
        self.cluster_tls
    }
}

fn check_host_port(a: &str) -> Result<(), ModelError> {
    let err = |why| ModelError::new("pd address", a, why);
    let (host, port) = a.rsplit_once(':').ok_or_else(|| err("is not host:port"))?;
    let host_ok = !host.is_empty()
        && host.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'[' | b']' | b':')
        });
    if !host_ok {
        return Err(err("has an invalid host"));
    }
    match port.parse::<u16>() {
        Ok(p) if p > 0 => Ok(()),
        _ => Err(err("has an invalid port")),
    }
}
