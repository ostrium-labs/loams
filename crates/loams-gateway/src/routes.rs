//! The route table (design §57 §8.1, D1018): rendered by `pg-control`,
//! validated whole, immutable once built. A reload builds a new table and
//! swaps it in ([`crate::reload`]).

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// One validated routes file.
#[derive(Debug)]
pub struct RouteTable {
    /// `sha256:<hex>` of the file without its `generation` line.
    pub generation: String,
    pub pg: PgRoutes,
    /// Managed RisingWave instances (design 81, D1957).
    pub risingwave: crate::risingwave::RwRoutes,
}

/// The Postgres module's routes.
#[derive(Debug, Default)]
pub struct PgRoutes {
    /// The namespace that owns no-SNI connections on the shared port
    /// (single node only).
    pub default_namespace: Option<Arc<PgNamespace>>,
    pub namespaces: Vec<Arc<PgNamespace>>,
    by_host: HashMap<String, Arc<PgNamespace>>,
    by_port: HashMap<u16, Arc<PgNamespace>>,
}

/// One Loams namespace's front door.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PgNamespace {
    pub name: String,
    /// SNI names, lower-cased; the first is the canonical one.
    pub hostnames: Vec<String>,
    /// The dedicated port, when the namespace asked for one (D1016).
    #[serde(default)]
    pub port: Option<u16>,
    /// PgDog addresses as `host:port`, tried in order (D1017). A host may
    /// resolve to several addresses (a headless Service).
    pub upstreams: Vec<String>,
    /// The name PgDog's certificate is verified against on the terminate
    /// path; the first hostname when absent.
    #[serde(default)]
    pub upstream_server_name: Option<String>,
}

impl PgNamespace {
    /// The name the gateway verifies PgDog's certificate against.
    pub fn upstream_server_name(&self) -> &str {
        self.upstream_server_name
            .as_deref()
            .or_else(|| self.canonical_hostname())
            .unwrap_or_default()
    }

    /// The first hostname (validation guarantees one).
    pub fn canonical_hostname(&self) -> Option<&str> {
        self.hostnames.first().map(String::as_str)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutesFile {
    #[serde(default)]
    generation: Option<String>,
    #[serde(default)]
    pg: PgRoutesFile,
    #[serde(default)]
    risingwave: crate::risingwave::RwRoutesFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PgRoutesFile {
    #[serde(default)]
    default_namespace: Option<String>,
    #[serde(default)]
    namespaces: Vec<PgNamespace>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RouteError {
    #[error("routes: {0}")]
    Parse(String),
    #[error("routes: {0}")]
    Invalid(String),
    #[error("routes: no generation line (pg-control always writes one)")]
    MissingGeneration,
    #[error("routes: generation {found} does not match the content ({computed})")]
    Generation { found: String, computed: String },
}

impl RouteTable {
    /// Parses and validates a routes file. `dedicated` is the port range a
    /// route's `port` must fall in. With `require_generation`, a file
    /// without its `generation` line is refused, so a truncated or
    /// hand-edited file never replaces the table.
    pub fn parse(
        text: &str,
        dedicated: &RangeInclusive<u16>,
        require_generation: bool,
    ) -> Result<RouteTable, RouteError> {
        let file: RoutesFile =
            toml::from_str(text).map_err(|e| RouteError::Parse(e.to_string()))?;
        let computed = generation_of(text);
        match file.generation {
            Some(found) if found != computed => {
                return Err(RouteError::Generation { found, computed });
            }
            None if require_generation => return Err(RouteError::MissingGeneration),
            _ => {}
        }
        let pg = PgRoutes::build(file.pg, dedicated)?;
        let risingwave = crate::risingwave::RwRoutes::build(file.risingwave, dedicated)?;
        for h in risingwave.hostnames() {
            if pg.by_host.contains_key(h) {
                return Err(invalid(format!(
                    "hostname {h:?} is both a namespace's and a RisingWave instance's"
                )));
            }
        }
        for port in risingwave.ports() {
            if pg.by_port.contains_key(&port) {
                return Err(invalid(format!(
                    "port {port} is both a namespace's and a RisingWave instance's"
                )));
            }
        }
        Ok(RouteTable {
            generation: computed,
            pg,
            risingwave,
        })
    }

    /// The namespace whose hostnames include `sni` (case-insensitive).
    pub fn by_hostname(&self, sni: &str) -> Option<&Arc<PgNamespace>> {
        self.pg.by_host.get(&sni.to_ascii_lowercase())
    }

    /// The namespace that owns a dedicated port.
    pub fn by_port(&self, port: u16) -> Option<&Arc<PgNamespace>> {
        self.pg.by_port.get(&port)
    }

    /// Every dedicated port in the table.
    pub fn dedicated_ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.pg
            .by_port
            .keys()
            .copied()
            .chain(self.risingwave.ports())
    }
}

impl PgRoutes {
    fn build(file: PgRoutesFile, dedicated: &RangeInclusive<u16>) -> Result<PgRoutes, RouteError> {
        let mut routes = PgRoutes::default();
        let mut names = HashMap::new();
        for mut ns in file.namespaces {
            check_name(&ns.name)?;
            if names.insert(ns.name.clone(), ()).is_some() {
                return Err(invalid(format!("namespace {:?} is listed twice", ns.name)));
            }
            if ns.hostnames.is_empty() {
                return Err(invalid(format!("namespace {:?} has no hostname", ns.name)));
            }
            for h in &mut ns.hostnames {
                *h = h.to_ascii_lowercase();
                check_hostname(h)?;
            }
            if let Some(name) = &ns.upstream_server_name {
                check_hostname(&name.to_ascii_lowercase())?;
            }
            if ns.upstreams.is_empty() {
                return Err(invalid(format!("namespace {:?} has no upstream", ns.name)));
            }
            for u in &ns.upstreams {
                check_upstream(u)?;
            }
            let ns = Arc::new(ns);
            for h in &ns.hostnames {
                if routes.by_host.insert(h.clone(), ns.clone()).is_some() {
                    return Err(invalid(format!("hostname {h:?} is listed twice")));
                }
            }
            if let Some(port) = ns.port {
                if !dedicated.contains(&port) {
                    return Err(invalid(format!(
                        "namespace {:?}: port {port} is outside the dedicated range {}-{}",
                        ns.name,
                        dedicated.start(),
                        dedicated.end()
                    )));
                }
                if routes.by_port.insert(port, ns.clone()).is_some() {
                    return Err(invalid(format!("port {port} is listed twice")));
                }
            }
            routes.namespaces.push(ns);
        }
        if let Some(name) = file.default_namespace {
            let ns = routes
                .namespaces
                .iter()
                .find(|n| n.name == name)
                .cloned()
                .ok_or_else(|| invalid(format!("default_namespace {name:?} is not listed")))?;
            routes.default_namespace = Some(ns);
        }
        Ok(routes)
    }
}

/// `text` with its `generation` line prepended, as `pg-control` writes it.
pub fn with_generation(text: &str) -> String {
    format!("generation = \"{}\"\n{text}", generation_of(text))
}

/// The generation of a routes file: `sha256:<hex>` of its text without the
/// top-level `generation` line, so the renderer can write the line after
/// computing it. `pg-control`'s renderer uses the same rule.
pub fn generation_of(text: &str) -> String {
    let mut hasher = Sha256::new();
    let mut in_root = true;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_root = false;
        }
        if in_root && is_generation_line(trimmed) {
            continue;
        }
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn is_generation_line(trimmed: &str) -> bool {
    trimmed
        .strip_prefix("generation")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn invalid(msg: String) -> RouteError {
    RouteError::Invalid(msg)
}

fn check_name(name: &str) -> Result<(), RouteError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "namespace name {name:?} is not [a-z0-9-]{{1,63}}"
        )))
    }
}

pub(crate) fn check_hostname(host: &str) -> Result<(), RouteError> {
    let ok = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("hostname {host:?} is not a DNS name")))
    }
}

pub(crate) fn check_upstream(upstream: &str) -> Result<(), RouteError> {
    let bad = || invalid(format!("upstream {upstream:?} is not host:port"));
    let (host, port) = upstream.rsplit_once(':').ok_or_else(bad)?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err(bad());
    }
    match port.parse::<u16>() {
        Ok(p) if p > 0 => Ok(()),
        _ => Err(bad()),
    }
}
