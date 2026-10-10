//! [`LiveConfig`]: one Loams Live app on a store (embedded or TiKV).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(feature = "tikv")]
use loams_kv::TikvConfig;
use loams_kv::{EmbeddedConfig, StoreConfig};

use crate::session::SessionConfig;
use crate::subs::SubsConfig;
use crate::{Limits, LiveError, catalog};

/// The prefix of a Live app's keyspace: app `chat` lives in `loams_live_chat`
/// (R1 plan Ruling 7).
pub const KEYSPACE_PREFIX: &str = "loams_live_";

/// The journal shard count a new app gets (R1 plan Ruling 4, raised from 16
/// to 64 by the owner, row T11-1; stored per app, row T10-1).
pub const DEFAULT_JOURNAL_SHARDS: u16 = 64;

/// Where Live's embedded store lives under a data directory:
/// `<data_dir>/live/store.redb` (LV1 plan Ruling 4).
pub fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("live").join("store.redb")
}

/// Where the Live sync API listens by default (§20 §7.1).
pub const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 7710);

/// How often the journal janitor runs (it trims entries older than the
/// retention and not held by a checkpoint, and sweeps expired idempotency
/// records).
pub const DEFAULT_JANITOR_INTERVAL: Duration = Duration::from_secs(60);

/// One Live app: its store (an embedded file or a TiKV handle, each with a
/// keyspace and a root prefix, R1 plan Ruling 1; LV1 row T20-9), its limits
/// and its journal shard count; and how this node serves it
/// ([`LiveServer`](crate::LiveServer)).
#[derive(Debug, Clone)]
pub struct LiveConfig {
    /// The app's name; the keyspace it derived is `loams_live_<app>`.
    pub app: String,
    /// The store: an embedded file, or a TiKV handle (the PD endpoints);
    /// each with the keyspace and the root prefix.
    pub store: StoreConfig,
    /// The per-document and per-write limits R1 fixes.
    pub limits: Limits,
    /// How many journal shards the app's commit journal has.
    pub journal_shards: u16,
    /// This node's id: the journal consumer its subscription manager
    /// checkpoints under, and the prefix of its session ids.
    pub node: String,
    /// The sync API's address; loopback only in R1 (D111, [`check_listen`]).
    pub listen: SocketAddr,
    /// The subscription manager (its `consumer` defaults to `node`).
    pub subs: SubsConfig,
    /// Sessions: queue, heartbeat, blocked limit.
    pub session: SessionConfig,
    /// How often the journal janitor runs.
    pub janitor_interval: Duration,
    /// Where deployed functions run (design §45 §3.1, D681; LV1 plan
    /// Task 5): in this process (trusted, single-tenant code only, LV1 row
    /// T3-10) or in sandboxed worker processes.
    pub isolation: Isolation,
    /// Whether this node serves one tenant's code or many; `Multi` needs
    /// [`Isolation::Isolated`] ([`check_isolation`]).
    pub tenancy: Tenancy,
}

/// Where a deployment's functions run (`[live] isolation`, design §45
/// §3.1, D681).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Isolation {
    /// `in_process`: QuickJS runtimes on threads of this process. For code
    /// the operator trusts (desktop, `loams dev`, single-tenant
    /// deployments): a C built-in that ignores the interrupt handler can
    /// hold a thread past the CPU limit (LV1 rows T3-7 and T3-10).
    #[default]
    InProcess,
    /// `isolated`: sandboxed `loams live-worker` processes (seccomp,
    /// landlock, resource limits), killed from outside when a call runs
    /// past its CPU limit. Linux only.
    Isolated,
}

impl Isolation {
    /// The configuration value: `in_process` or `isolated`.
    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::InProcess => "in_process",
            Isolation::Isolated => "isolated",
        }
    }
}

/// Whose code a node serves (`[live] tenancy`, design §45 §3.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Tenancy {
    /// `single`: one operator's own code.
    #[default]
    Single,
    /// `multi`: apps of more than one tenant; needs
    /// [`Isolation::Isolated`].
    Multi,
}

impl Tenancy {
    /// The configuration value: `single` or `multi`.
    pub fn as_str(self) -> &'static str {
        match self {
            Tenancy::Single => "single",
            Tenancy::Multi => "multi",
        }
    }
}

impl LiveConfig {
    /// App `app` on the default store: the embedded store of `data_dir`
    /// ([`store_path`]), in the keyspace `loams_live_<app>`, with R1's
    /// default limits, served on 127.0.0.1:7710 as node `1` (LV1 plan
    /// Task 23).
    pub fn new(data_dir: &Path, app: &str) -> Result<Self, LiveError> {
        catalog::check_name("app", app)?;
        Ok(LiveConfig::with_store(
            app,
            StoreConfig::Embedded(EmbeddedConfig::new(store_path(data_dir), keyspace_of(app))),
        ))
    }

    /// App `app` on the TiKV cluster whose PD endpoints are `pd`, in the
    /// keyspace `loams_live_<app>`, with the defaults of [`LiveConfig::new`]
    /// (feature `tikv`).
    #[cfg(feature = "tikv")]
    pub fn on_tikv(pd: Vec<String>, app: &str) -> Result<Self, LiveError> {
        catalog::check_name("app", app)?;
        Ok(LiveConfig::with_tikv(
            app,
            TikvConfig::new(pd, keyspace_of(app)),
        ))
    }

    /// App `app` on the TiKV handle `tikv` (whose keyspace and root the
    /// caller chose), with the defaults of [`LiveConfig::new`]. The name is
    /// not checked (feature `tikv`).
    #[cfg(feature = "tikv")]
    pub fn with_tikv(app: &str, tikv: TikvConfig) -> Self {
        LiveConfig::with_store(app, StoreConfig::Tikv(tikv))
    }

    /// App `app` on the store `store` (whose keyspace and root the caller
    /// chose), with the defaults of [`LiveConfig::new`]. The name is not
    /// checked.
    pub fn with_store(app: &str, store: StoreConfig) -> Self {
        LiveConfig {
            app: app.to_string(),
            store,
            limits: Limits::default(),
            journal_shards: DEFAULT_JOURNAL_SHARDS,
            node: "1".to_string(),
            listen: DEFAULT_LISTEN,
            subs: SubsConfig::default(),
            session: SessionConfig::default(),
            janitor_interval: DEFAULT_JANITOR_INTERVAL,
            isolation: Isolation::default(),
            tenancy: Tenancy::default(),
        }
    }
}

/// The keyspace of app `app`.
pub fn keyspace_of(app: &str) -> String {
    format!("{KEYSPACE_PREFIX}{app}")
}

/// Refuses a non-loopback listen address (127.0.0.0/8, `::1` and their
/// IPv4-mapped forms pass): `Mutate`, the `_system:*` writes and `Deploy`
/// have no authentication in R1 (D111, §20 §7.1).
pub fn check_listen(addr: SocketAddr) -> Result<(), LiveError> {
    if addr.ip().to_canonical().is_loopback() {
        Ok(())
    } else {
        Err(LiveError::NotLoopback(addr))
    }
}

/// Refuses `tenancy = "multi"` unless functions run `isolated`, and
/// `isolated` off Linux, where the worker sandbox does not exist (design
/// §45 §3.1, D681; LV1 plan Task 5).
pub fn check_isolation(config: &LiveConfig) -> Result<(), LiveError> {
    let isolated_here = config.isolation == Isolation::Isolated && cfg!(target_os = "linux");
    if config.tenancy == Tenancy::Multi && !isolated_here {
        return Err(LiveError::IsolationRequired);
    }
    if config.isolation == Isolation::Isolated && !cfg!(target_os = "linux") {
        return Err(LiveError::IsolationUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MAX_NAME_BYTES;

    #[test]
    fn new_defaults_to_the_embedded_store_of_the_data_dir() {
        let config = LiveConfig::new(Path::new("/data"), "chat").expect("chat is a valid app name");
        assert_eq!(config.app, "chat");
        assert_eq!(config.store.keyspace(), "loams_live_chat");
        let StoreConfig::Embedded(store) = &config.store else {
            panic!("new configures the embedded store (LV1 plan Task 23)");
        };
        assert_eq!(store.path, Path::new("/data/live/store.redb"));
        assert!(store.root.is_empty());
        assert_eq!(config.journal_shards, DEFAULT_JOURNAL_SHARDS);
        assert_eq!(config.limits, Limits::default());
    }

    #[cfg(feature = "tikv")]
    #[test]
    fn on_tikv_derives_the_keyspace() {
        let config = LiveConfig::on_tikv(vec!["127.0.0.1:2379".into()], "chat")
            .expect("chat is a valid app name");
        let StoreConfig::Tikv(tikv) = &config.store else {
            panic!("on_tikv configures TiKV");
        };
        assert_eq!(tikv.keyspace, "loams_live_chat");
        assert_eq!(tikv.pd, ["127.0.0.1:2379".to_string()]);
        assert!(LiveConfig::on_tikv(Vec::new(), "1bad").is_err());
    }

    #[test]
    fn with_store_keeps_the_store_and_with_tikv_wraps_it() {
        let embedded = StoreConfig::Embedded(loams_kv::EmbeddedConfig::new(
            "/data/live/store.redb",
            keyspace_of("chat"),
        ));
        let config = LiveConfig::with_store("chat", embedded);
        assert!(
            matches!(&config.store, StoreConfig::Embedded(e) if e.keyspace == "loams_live_chat")
        );
        assert_eq!(config.journal_shards, DEFAULT_JOURNAL_SHARDS);
        #[cfg(feature = "tikv")]
        {
            let tikv = LiveConfig::with_tikv("chat", TikvConfig::new(vec!["pd:2379".into()], "ks"));
            assert!(matches!(&tikv.store, StoreConfig::Tikv(t) if t.keyspace == "ks"));
        }
    }

    #[test]
    fn keyspace_of_prefixes_the_app_name() {
        assert_eq!(keyspace_of("chat"), "loams_live_chat");
        assert_eq!(keyspace_of("orders"), "loams_live_orders");
    }

    #[test]
    fn new_refuses_a_bad_app_name() {
        for bad in ["", "1chat", "chat-app", "chat app"] {
            let err = LiveConfig::new(Path::new("/data"), bad)
                .expect_err("a name must start with a letter");
            assert!(
                matches!(err, LiveError::InvalidArgument(_)),
                "{bad:?}: {err}"
            );
        }
    }

    #[test]
    fn new_accepts_the_longest_name_and_refuses_one_byte_more() {
        let longest = "a".repeat(MAX_NAME_BYTES);
        assert_eq!(
            LiveConfig::new(Path::new("/data"), &longest)
                .expect("64 bytes is allowed")
                .store
                .keyspace(),
            format!("{KEYSPACE_PREFIX}{longest}")
        );
        let too_long = "a".repeat(MAX_NAME_BYTES + 1);
        assert!(matches!(
            LiveConfig::new(Path::new("/data"), &too_long),
            Err(LiveError::InvalidArgument(_))
        ));
    }
}
