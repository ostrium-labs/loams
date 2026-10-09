//! Runtime configuration from the environment.

/// Where a local `loams dev` stack serves Connect (`LOAM_URL`, design 30
/// section 8).
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8080";

/// What the Loams integration talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoamsConfig {
    /// The loams server's base URL (Connect over HTTP).
    pub server_url: String,
    /// Run against the in-process mock instead of a real server.
    pub mock: bool,
    /// The Loams Bot A2A endpoint (JSON-RPC over HTTP), when it is not the
    /// server itself.
    pub bot_url: Option<String>,
    /// Overrides the OIDC issuer the instance advertises (development).
    pub oidc_issuer: Option<String>,
}

impl Default for LoamsConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.into(),
            mock: false,
            bot_url: None,
            oidc_issuer: None,
        }
    }
}

impl LoamsConfig {
    /// Reads `LOAMS_URL` (falling back to `LOAM_URL`), `LOAMS_MOCK`,
    /// `LOAMS_BOT_URL` and `LOAMS_OIDC_ISSUER`.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// [`Self::from_env`] over any lookup, so tests need no process state.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |key: &str| {
            get(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let truthy = |key: &str| {
            non_empty(key).is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        };
        Self {
            server_url: non_empty("LOAMS_URL")
                .or_else(|| non_empty("LOAM_URL"))
                .unwrap_or_else(|| DEFAULT_SERVER_URL.into()),
            mock: truthy("LOAMS_MOCK"),
            bot_url: non_empty("LOAMS_BOT_URL"),
            oidc_issuer: non_empty("LOAMS_OIDC_ISSUER"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> LoamsConfig {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        LoamsConfig::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_to_the_local_stack() {
        assert_eq!(cfg(&[]), LoamsConfig::default());
    }

    #[test]
    fn loams_url_wins_over_loam_url() {
        let c = cfg(&[("LOAM_URL", "http://a:1"), ("LOAMS_URL", "http://b:2")]);
        assert_eq!(c.server_url, "http://b:2");
        assert_eq!(cfg(&[("LOAM_URL", "http://a:1")]).server_url, "http://a:1");
    }

    #[test]
    fn mock_is_opt_in_and_blank_values_are_ignored() {
        assert!(cfg(&[("LOAMS_MOCK", "1")]).mock);
        assert!(!cfg(&[("LOAMS_MOCK", "0")]).mock);
        assert_eq!(cfg(&[("LOAMS_URL", "  ")]).server_url, DEFAULT_SERVER_URL);
    }
}
