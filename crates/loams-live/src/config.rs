//! [`LiveConfig`]: one Loams Live app on TiKV.

use loams_tikv::TikvConfig;

use crate::{Limits, LiveError, catalog};

/// The prefix of a Live app's keyspace: app `chat` lives in `loams_live_chat`
/// (R1 plan Ruling 7).
pub const KEYSPACE_PREFIX: &str = "loams_live_";

/// The journal shard count a new app gets (R1 plan Ruling 4, raised from 16
/// to 64 by the owner, row T11-1; stored per app, row T10-1).
pub const DEFAULT_JOURNAL_SHARDS: u16 = 64;

/// One Live app: its TiKV handle configuration (keyspace and root prefix,
/// R1 plan Ruling 1), its limits and its journal shard count.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    /// The app's name; the keyspace it derived is `loams_live_<app>`.
    pub app: String,
    /// The TiKV handle: the PD endpoints, the keyspace and the root prefix.
    pub tikv: TikvConfig,
    /// The per-document and per-write limits R1 fixes.
    pub limits: Limits,
    /// How many journal shards the app's commit journal has.
    pub journal_shards: u16,
}

impl LiveConfig {
    /// App `app` on the cluster whose PD endpoints are `pd`, in the keyspace
    /// `loams_live_<app>`, with R1's default limits.
    pub fn new(pd: Vec<String>, app: &str) -> Result<Self, LiveError> {
        catalog::check_name("app", app)?;
        Ok(LiveConfig {
            app: app.to_string(),
            tikv: TikvConfig::new(pd, keyspace_of(app)),
            limits: Limits::default(),
            journal_shards: DEFAULT_JOURNAL_SHARDS,
        })
    }
}

/// The keyspace of app `app`.
pub fn keyspace_of(app: &str) -> String {
    format!("{KEYSPACE_PREFIX}{app}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MAX_NAME_BYTES;

    #[test]
    fn new_derives_the_keyspace_and_defaults() {
        let config = LiveConfig::new(vec!["127.0.0.1:2379".into()], "chat")
            .expect("chat is a valid app name");
        assert_eq!(config.app, "chat");
        assert_eq!(config.tikv.keyspace, "loams_live_chat");
        assert_eq!(config.tikv.pd, ["127.0.0.1:2379".to_string()]);
        assert_eq!(config.journal_shards, DEFAULT_JOURNAL_SHARDS);
        assert_eq!(config.limits, Limits::default());
    }

    #[test]
    fn keyspace_of_prefixes_the_app_name() {
        assert_eq!(keyspace_of("chat"), "loams_live_chat");
        assert_eq!(keyspace_of("orders"), "loams_live_orders");
    }

    #[test]
    fn new_refuses_a_bad_app_name() {
        for bad in ["", "1chat", "chat-app", "chat app"] {
            let err = LiveConfig::new(vec!["127.0.0.1:2379".into()], bad)
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
            LiveConfig::new(Vec::new(), &longest)
                .expect("64 bytes is allowed")
                .tikv
                .keyspace,
            format!("{KEYSPACE_PREFIX}{longest}")
        );
        let too_long = "a".repeat(MAX_NAME_BYTES + 1);
        assert!(matches!(
            LiveConfig::new(Vec::new(), &too_long),
            Err(LiveError::InvalidArgument(_))
        ));
    }
}
