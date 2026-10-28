//! The `resp` module's configuration (plan KV2 Task 2), read from its own
//! file (`loams-gateway --module resp --config resp.toml`, ruling K2-2).
//!
//! ```toml
//! listen = "127.0.0.1:6379"
//! requirepass = "…"            # or requirepass_file = "/run/secrets/…"
//! databases = 16
//! [store]
//! kind = "embedded"            # or "memory", or "tikv"
//! path = "/var/lib/loams/kv.redb"
//! keyspace = "kv"
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use loams_kv::{EmbeddedConfig, StoreConfig, tenant};
use serde::Deserialize;

use crate::frame::Limits;

/// The `resp` module's configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RespConfig {
    /// The listener.
    pub listen: SocketAddr,
    /// The default user's password; none means no authentication, allowed
    /// only on a loopback listener.
    #[serde(default)]
    pub requirepass: Option<String>,
    /// A file holding the password (trailing whitespace trimmed).
    #[serde(default)]
    pub requirepass_file: Option<PathBuf>,
    /// The number of `SELECT` databases (Q1234), 1 to [`MAX_DATABASES`].
    #[serde(default = "default_databases")]
    pub databases: u16,
    /// At most this many client connections.
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
    /// `proto-max-bulk-len`: the largest argument.
    #[serde(default = "default_max_bulk")]
    pub proto_max_bulk_len: usize,
    /// Close a client idle for this many seconds; 0 never does.
    #[serde(default)]
    pub timeout_secs: u64,
    /// `list-max-shift`: `LINSERT` and `LREM` move every element after the
    /// change, so they refuse lists longer than this (plan KV2 Task 8).
    #[serde(default = "default_list_max_shift")]
    pub list_max_shift: usize,
    /// How writes are acknowledged (D1651; ruling K2-3).
    #[serde(default)]
    pub durability: Durability,
    /// Where keys are stored.
    pub store: StoreSection,
}

/// The most `SELECT` databases a gateway serves. A database's keys start
/// with its index byte, beside the keyspace-wide rows that start with `'L'`
/// (0x4C, leases), `'g'` (0x67, the GC queue) and `'j'` (0x6A, the
/// journal) in D1653's layout, so the index stays below all of them
/// (ruling K2-8).
pub const MAX_DATABASES: u16 = 64;
const _: () = assert!(MAX_DATABASES <= 0x4C, "database bytes must sort below 'L'");

fn default_databases() -> u16 {
    16
}

fn default_max_clients() -> usize {
    10_000
}

fn default_list_max_shift() -> usize {
    10_000
}

fn default_max_bulk() -> usize {
    512 * 1024 * 1024
}

/// D1651's durability modes. Only `durable` runs until `loams-kv-hot`
/// (plan KV1) exists; the others are accepted by the parser and refused by
/// [`RespConfig::check`] (ruling K2-3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Durability {
    /// Every write commits before its reply (the default).
    #[default]
    Durable,
    /// Writes go to the hot tier and the store (KV1).
    WriteThrough,
    /// Writes go to the hot tier only (KV1, behind a flag).
    HotOnly,
}

/// The store the gateway keeps its keys in, under the Redis product root
/// (D1645).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum StoreSection {
    /// A redb file of the gateway's own (D1647: one replica).
    Embedded {
        path: PathBuf,
        keyspace: String,
        #[serde(default)]
        root: Option<String>,
    },
    /// An in-memory store, for tests and the dev loop.
    Memory {
        name: String,
        #[serde(default = "default_keyspace")]
        keyspace: String,
    },
    /// A TiKV keyspace (the `tikv` profile). Parsed in every build so the
    /// error names the missing feature; read only with it.
    #[cfg_attr(not(feature = "tikv"), allow(dead_code))]
    Tikv {
        pd: Vec<String>,
        keyspace: String,
        #[serde(default)]
        root: Option<String>,
    },
}

fn default_keyspace() -> String {
    "kv".into()
}

impl RespConfig {
    /// Parses and checks a configuration file's text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let config: RespConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        config.check()?;
        Ok(config)
    }

    /// The checks a parse cannot express.
    pub fn check(&self) -> Result<(), String> {
        if self.databases == 0 || self.databases > MAX_DATABASES {
            return Err(format!(
                "databases must be 1 to {MAX_DATABASES}, not {}",
                self.databases
            ));
        }
        if self.requirepass.is_some() && self.requirepass_file.is_some() {
            return Err("set requirepass or requirepass_file, not both".into());
        }
        let has_password = self.requirepass.is_some() || self.requirepass_file.is_some();
        if !has_password && !self.listen.ip().is_loopback() {
            return Err(format!(
                "listen {} is not a loopback address: set requirepass (or requirepass_file)",
                self.listen
            ));
        }
        if self.durability != Durability::Durable {
            return Err(format!(
                "durability {:?} requires loams-kv-hot (plan KV1); only \"durable\" runs today",
                self.durability
            ));
        }
        if matches!(self.store, StoreSection::Tikv { .. }) && !cfg!(feature = "tikv") {
            return Err("store kind \"tikv\" needs loams-resp's `tikv` feature".into());
        }
        Ok(())
    }

    /// The password, read from `requirepass_file` when that is set.
    pub fn password(&self) -> Result<Option<String>, String> {
        if let Some(path) = &self.requirepass_file {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            return Ok(Some(text.trim_end().to_owned()));
        }
        Ok(self.requirepass.clone())
    }

    /// The decoder's limits.
    pub fn limits(&self) -> Limits {
        Limits {
            max_bulk: self.proto_max_bulk_len,
            ..Limits::default()
        }
    }

    /// The idle timeout, if any.
    pub fn idle_timeout(&self) -> Option<Duration> {
        (self.timeout_secs > 0).then(|| Duration::from_secs(self.timeout_secs))
    }

    /// The store configuration, under the Redis product root.
    pub fn store_config(&self) -> Result<StoreConfig, String> {
        let base = match &self.store {
            StoreSection::Embedded {
                path,
                keyspace,
                root,
            } => StoreConfig::Embedded(EmbeddedConfig {
                root: root.clone().unwrap_or_default().into_bytes(),
                ..EmbeddedConfig::new(path.clone(), keyspace.clone())
            }),
            StoreSection::Memory { name, keyspace } => {
                StoreConfig::Embedded(EmbeddedConfig::in_memory(name, keyspace.clone()))
            }
            #[cfg(feature = "tikv")]
            StoreSection::Tikv { pd, keyspace, root } => StoreConfig::Tikv(loams_kv::TikvConfig {
                root: root.clone().unwrap_or_default().into_bytes(),
                ..loams_kv::TikvConfig::new(pd.clone(), keyspace.clone())
            }),
            #[cfg(not(feature = "tikv"))]
            StoreSection::Tikv { .. } => {
                return Err("store kind \"tikv\" needs loams-resp's `tikv` feature".into());
            }
        };
        Ok(tenant::product_root(&base, tenant::PRODUCT_REDIS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMORY: &str = "[store]\nkind = \"memory\"\nname = \"t\"\n";

    #[test]
    fn loopback_without_password_is_allowed() {
        let config = RespConfig::from_toml(&format!("listen = \"127.0.0.1:6379\"\n{MEMORY}"))
            .expect("valid");
        assert_eq!(config.databases, 16);
        assert_eq!(config.durability, Durability::Durable);
        assert_eq!(config.store_config().expect("a store").root(), b"R");
    }

    #[test]
    fn public_listener_needs_a_password() {
        let err = RespConfig::from_toml(&format!("listen = \"0.0.0.0:6379\"\n{MEMORY}"))
            .expect_err("refused");
        assert!(err.contains("requirepass"), "{err}");
        RespConfig::from_toml(&format!(
            "listen = \"0.0.0.0:6379\"\nrequirepass = \"x\"\n{MEMORY}"
        ))
        .expect("valid with a password");
    }

    #[test]
    fn durability_modes_parse_and_refuse() {
        for (mode, ok) in [
            ("durable", true),
            ("write-through", false),
            ("hot-only", false),
        ] {
            let text = format!("listen = \"127.0.0.1:6379\"\ndurability = \"{mode}\"\n{MEMORY}");
            let result = RespConfig::from_toml(&text);
            assert_eq!(result.is_ok(), ok, "{mode}: {result:?}");
            if let Err(e) = result {
                assert!(e.contains("loams-kv-hot"), "{e}");
            }
        }
        assert!(
            RespConfig::from_toml(&format!(
                "listen = \"127.0.0.1:6379\"\ndurability = \"fast\"\n{MEMORY}"
            ))
            .is_err()
        );
    }

    #[test]
    fn databases_stay_below_the_keyspace_rows() {
        let with = |n: u16| {
            RespConfig::from_toml(&format!(
                "listen = \"127.0.0.1:6379\"\ndatabases = {n}\n{MEMORY}"
            ))
        };
        assert!(with(0).is_err());
        assert!(with(MAX_DATABASES).is_ok());
        assert!(with(MAX_DATABASES + 1).is_err());
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(
            RespConfig::from_toml(&format!("listen = \"127.0.0.1:6379\"\nport = 1\n{MEMORY}"))
                .is_err()
        );
    }
}
