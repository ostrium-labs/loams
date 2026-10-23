//! Revocations (design §73 §10.3, D1472; `schemas/marketplace/revocations.v1.json`):
//! checked when agentd starts and every 6 hours (Q1135). `disable` stops the
//! agent and blocks runs; `warn` shows a banner. User data is never deleted.

use semver::{Version, VersionReq};
use serde::Deserialize;

pub const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Warn,
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub id: String,
    pub versions: String,
    pub severity: Severity,
    pub reason: String,
    pub date: String,
    pub advisory: Option<String>,
    range: VersionReq,
}

impl Revocation {
    /// Whether this revocation covers `id@version`. A version that is not
    /// semantic is treated as covered: revocation fails closed.
    pub fn covers(&self, id: &str, version: &str) -> bool {
        id == self.id
            && Version::parse(version)
                .map(|v| matches_including_prereleases(&self.range, &v))
                .unwrap_or(true)
    }
}

/// `VersionReq::matches` skips prereleases unless a comparator names one; a
/// revocation of `>=0.1.0, <0.2.0` must also cover `0.1.5-rc.1`. So each
/// comparator becomes bounds (Cargo's meaning for each operator and partial
/// version), compared by semantic-version precedence.
fn matches_including_prereleases(range: &VersionReq, version: &Version) -> bool {
    range.comparators.iter().all(|c| {
        let (low, high) = bounds(c);
        let above = match &low {
            None => true,
            Some((bound, true)) => version >= bound,
            Some((bound, false)) => version > bound,
        };
        let below = match &high {
            None => true,
            Some((bound, true)) => version <= bound,
            Some((bound, false)) => version < bound,
        };
        above && below
    })
}

type Bound = Option<(Version, bool)>;

/// A comparator's lower and upper bound; `true` means inclusive.
fn bounds(c: &semver::Comparator) -> (Bound, Bound) {
    use semver::Op;
    let v = |major: u64, minor: u64, patch: u64| Version::new(major, minor, patch);
    let exact = || {
        let mut out = v(c.major, c.minor.unwrap_or(0), c.patch.unwrap_or(0));
        out.pre = c.pre.clone();
        out
    };
    let (major, minor, patch) = (c.major, c.minor, c.patch);
    // The version after the partial one: `1` → 2.0.0, `1.2` → 1.3.0.
    let next_partial = || match (minor, patch) {
        (None, _) => v(major + 1, 0, 0),
        (Some(m), None) => v(major, m + 1, 0),
        (Some(_), Some(_)) => exact(),
    };
    match c.op {
        Op::Exact | Op::Wildcard => match (minor, patch) {
            (Some(_), Some(_)) if c.op == Op::Exact => {
                (Some((exact(), true)), Some((exact(), true)))
            }
            _ => (Some((exact(), true)), Some((next_partial(), false))),
        },
        Op::Greater => match patch {
            Some(_) => (Some((exact(), false)), None),
            None => (Some((next_partial(), true)), None),
        },
        Op::GreaterEq => (Some((exact(), true)), None),
        Op::Less => (None, Some((exact(), false))),
        Op::LessEq => match patch {
            Some(_) => (None, Some((exact(), true))),
            None => (None, Some((next_partial(), false))),
        },
        Op::Tilde => {
            let high = match minor {
                Some(m) => v(major, m + 1, 0),
                None => v(major + 1, 0, 0),
            };
            (Some((exact(), true)), Some((high, false)))
        }
        Op::Caret => {
            let high = match (major, minor, patch) {
                (0, Some(0), Some(p)) => v(0, 0, p + 1),
                (0, Some(m), _) => v(0, m + 1, 0),
                (0, None, _) => v(1, 0, 0),
                (maj, _, _) => v(maj + 1, 0, 0),
            };
            (Some((exact(), true)), Some((high, false)))
        }
        // An operator added to `semver` later: cover every version, so a
        // revocation fails closed.
        _ => (None, None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RevocationsError {
    #[error("revocations.json is not revocations v1: {0}")]
    Shape(String),
    #[error("revocation of {id}: {versions} is not a version range: {error}")]
    Range {
        id: String,
        versions: String,
        error: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawList {
    #[serde(rename = "$schema", default)]
    _schema: Option<String>,
    revocations: Vec<RawRevocation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRevocation {
    id: String,
    versions: String,
    severity: Severity,
    reason: String,
    date: String,
    #[serde(default)]
    advisory: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Revocations {
    pub entries: Vec<Revocation>,
}

impl Revocations {
    /// Parses verified `revocations.json` bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, RevocationsError> {
        let raw: RawList =
            serde_json::from_slice(bytes).map_err(|e| RevocationsError::Shape(e.to_string()))?;
        let mut entries = Vec::with_capacity(raw.revocations.len());
        for entry in raw.revocations {
            let range =
                VersionReq::parse(&entry.versions).map_err(|e| RevocationsError::Range {
                    id: entry.id.clone(),
                    versions: entry.versions.clone(),
                    error: e.to_string(),
                })?;
            entries.push(Revocation {
                id: entry.id,
                versions: entry.versions,
                severity: entry.severity,
                reason: entry.reason,
                date: entry.date,
                advisory: entry.advisory,
                range,
            });
        }
        Ok(Self { entries })
    }

    /// The most severe revocation covering `id@version`, if any.
    pub fn for_version(&self, id: &str, version: &str) -> Option<&Revocation> {
        self.entries
            .iter()
            .filter(|r| r.covers(id, version))
            .max_by_key(|r| r.severity)
    }
}

/// One installed listing, as the revocation check sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub source: String,
    pub id: String,
    pub version: String,
}

/// What a check decided for one installed listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub installed: Installed,
    pub revocation: Revocation,
}

impl Finding {
    /// `disable`: stop the agent and block runs.
    pub fn disables(&self) -> bool {
        self.revocation.severity == Severity::Disable
    }
}

/// Checks every installed listing against its source's revocations.
pub fn check<'a>(
    installed: &[Installed],
    revocations_of: impl Fn(&str) -> Option<&'a Revocations>,
) -> Vec<Finding> {
    installed
        .iter()
        .filter_map(|item| {
            let list = revocations_of(&item.source)?;
            list.for_version(&item.id, &item.version).map(|r| Finding {
                installed: item.clone(),
                revocation: r.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn list(versions: &str, severity: &str) -> Revocations {
        Revocations::parse(
            format!(
                r#"{{"revocations":[{{"id":"ostrium/a","versions":"{versions}","severity":"{severity}","reason":"r","date":"2026-10-22"}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap()
    }

    fn covered(versions: &str, version: &str) -> bool {
        list(versions, "warn")
            .for_version("ostrium/a", version)
            .is_some()
    }

    #[test]
    fn ranges_use_cargo_syntax_and_cover_prereleases() {
        assert!(covered("=0.1.0", "0.1.0"));
        assert!(!covered("=0.1.0", "0.1.1"));
        assert!(covered(">=0.1.0, <0.2.0", "0.1.9"));
        assert!(covered(">=0.1.0, <0.2.0", "0.1.5-rc.1"));
        assert!(covered(">=0.1.0, <0.2.0", "0.2.0-rc.1"));
        assert!(!covered(">=0.1.0, <0.2.0", "0.2.0"));
        assert!(!covered(">=0.1.0, <0.2.0", "0.0.9"));
        assert!(covered("^2.0.0", "2.9.1"));
        assert!(!covered("^2.0.0", "3.0.0"));
        assert!(covered("^0.3.1", "0.3.9"));
        assert!(!covered("^0.3.1", "0.4.0"));
        assert!(covered("~1.2", "1.2.7"));
        assert!(!covered("~1.2", "1.3.0"));
        assert!(covered("*", "9.9.9"));
        assert!(covered("<=1.2", "1.2.9"));
        assert!(!covered("<=1.2", "1.3.0"));
        assert!(covered(">1.2", "1.3.0"));
        assert!(!covered(">1.2", "1.2.9"));
        assert!(covered("=1", "1.4.0"));
    }

    #[test]
    fn a_version_that_is_not_semantic_is_covered() {
        assert!(covered("=0.1.0", "latest"));
    }

    #[test]
    fn other_listings_are_not_covered() {
        assert!(
            list("*", "disable")
                .for_version("ostrium/b", "1.0.0")
                .is_none()
        );
    }

    #[test]
    fn the_most_severe_revocation_wins() {
        let both = Revocations::parse(
            br#"{"revocations":[
              {"id":"ostrium/a","versions":"*","severity":"warn","reason":"w","date":"2026-10-22"},
              {"id":"ostrium/a","versions":"=1.0.0","severity":"disable","reason":"d","date":"2026-10-22"}]}"#,
        )
        .unwrap();
        assert_eq!(
            both.for_version("ostrium/a", "1.0.0").unwrap().severity,
            Severity::Disable
        );
        assert_eq!(
            both.for_version("ostrium/a", "1.0.1").unwrap().severity,
            Severity::Warn
        );
    }

    #[test]
    fn malformed_lists_are_refused() {
        assert!(Revocations::parse(b"{}").is_err());
        assert!(Revocations::parse(br#"{"revocations":[{"id":"a/b","versions":"soon","severity":"warn","reason":"r","date":"d"}]}"#).is_err());
        assert!(Revocations::parse(br#"{"revocations":[{"id":"a/b","versions":"*","severity":"delete","reason":"r","date":"d"}]}"#).is_err());
        assert!(Revocations::parse(br#"{"revocations":[],"extra":1}"#).is_err());
    }
}
