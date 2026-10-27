//! Maps a measured resource to the tenant it is billed to (design 01 §5.1).
//!
//! MB0 has a static resolver from configuration. The kube resolver (MB2)
//! watches the node's pods and maps the pod UID in the cgroup path
//! ([`crate::cgroup::pod_uid_from_path`]) to the pod's `loams.dev/*` labels.

use std::path::Path;

use serde::Deserialize;

use crate::cgroup::CgroupSample;
use crate::error::MeterError;
use crate::event::{Attribution, Tier};

/// Resolves the tenant of a cgroup. `None` means nobody owns it; the agent
/// then meters it under [`Attribution::platform`].
pub trait TenantResolver: Send + Sync {
    /// The tenant billed for `sample`.
    fn resolve(&self, sample: &CgroupSample) -> Option<Attribution>;
}

/// One static rule: cgroups under `prefix` belong to a tenant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticRule {
    /// A cgroup path relative to the root, matched on whole components.
    pub prefix: String,
    /// The org billed.
    pub org: String,
    /// The namespace billed.
    pub namespace: String,
    /// The function, when fixed.
    #[serde(default)]
    pub function: Option<String>,
    /// The tier, when fixed.
    #[serde(default)]
    pub tier: Option<Tier>,
}

/// A resolver from a fixed list of rules; the longest matching prefix wins.
#[derive(Debug, Clone, Default)]
pub struct StaticResolver {
    rules: Vec<StaticRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StaticConfig {
    rules: Vec<StaticRule>,
}

impl StaticResolver {
    /// A resolver over `rules`.
    pub fn new(mut rules: Vec<StaticRule>) -> Self {
        for rule in &mut rules {
            rule.prefix = rule.prefix.trim_matches('/').to_string();
        }
        // Longest prefix first, so the first match is the most specific.
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.prefix.len()));
        Self { rules }
    }

    /// Parses `{"rules": [{"prefix", "org", "namespace", "function"?,
    /// "tier"?}, …]}`.
    pub fn from_json(json: &str) -> Result<Self, MeterError> {
        let config: StaticConfig =
            serde_json::from_str(json).map_err(|e| MeterError::Config(e.to_string()))?;
        Ok(Self::new(config.rules))
    }

    /// Reads [`StaticResolver::from_json`]'s format from a file.
    pub fn from_file(path: &Path) -> Result<Self, MeterError> {
        let json = std::fs::read_to_string(path).map_err(|e| MeterError::io(path, e))?;
        Self::from_json(&json)
    }
}

impl TenantResolver for StaticResolver {
    fn resolve(&self, sample: &CgroupSample) -> Option<Attribution> {
        self.rules
            .iter()
            .find(|rule| under(&sample.path, &rule.prefix))
            .map(|rule| Attribution {
                org: rule.org.clone(),
                namespace: rule.namespace.clone(),
                function: rule.function.clone(),
                tier: rule.tier,
            })
    }
}

/// Whether `path` is `prefix` or below it, on whole path components.
fn under(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str) -> CgroupSample {
        CgroupSample {
            path: path.into(),
            inode: 1,
            cpu_usec: 0,
            memory_current: None,
            memory_peak: None,
            populated: true,
        }
    }

    #[test]
    fn longest_prefix_wins_on_whole_components() {
        let resolver = StaticResolver::from_json(
            r#"{"rules": [
                {"prefix": "loams.slice", "org": "org_a", "namespace": "default"},
                {"prefix": "/loams.slice/loams-fn1/", "org": "org_a", "namespace": "prod",
                 "function": "fn1", "tier": "t0"}
            ]}"#,
        )
        .unwrap();
        let fn1 = resolver.resolve(&at("loams.slice/loams-fn1")).unwrap();
        assert_eq!(fn1.namespace, "prod");
        assert_eq!(fn1.function.as_deref(), Some("fn1"));
        assert_eq!(fn1.tier, Some(Tier::T0));
        // `loams-fn10` is not under `loams-fn1`.
        let other = resolver.resolve(&at("loams.slice/loams-fn10")).unwrap();
        assert_eq!(other.namespace, "default");
        assert!(resolver.resolve(&at("system.slice/sshd.service")).is_none());
    }

    #[test]
    fn unknown_fields_are_refused() {
        let err = StaticResolver::from_json(
            r#"{"rules": [{"prefix": "a", "org": "o", "namespace": "n", "tenant": "x"}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, MeterError::Config(_)));
    }
}
