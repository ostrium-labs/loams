//! The release train, `release/upgrade-path.toml` (design §54 §5, §11.1;
//! D925, D936), and the one-step-at-a-time rule.
//!
//! Adjacency (UP1 Task 3):
//! - a pre-release (`0.1.0-preview.3`) accepts exactly its `from`;
//! - a patch release of a line declares `from = "0.1"` and accepts a stored
//!   0.1.x with x below its own patch, never a later one, another line, or a
//!   pre-release;
//! - a new line's first release (0.2.0) names the last patch of the previous
//!   line, exactly;
//! - `from = "fresh"` keeps no data and accepts no stored release.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::formats::Store;
use crate::ledger::Class;

/// The release train compiled into this binary.
const COMPILED: &str = include_str!("../../../release/upgrade-path.toml");

/// `from` of a release that keeps no data.
pub const FRESH: &str = "fresh";

/// The parsed `release/upgrade-path.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradePath {
    #[serde(rename = "release")]
    pub releases: Vec<Release>,
}

/// One release of the train.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    /// The predecessor: an exact release, a minor line (`0.1`) or `fresh`.
    pub from: String,
    /// The free space `loams upgrade` requires, as a multiple of the
    /// snapshot estimate.
    #[serde(default)]
    pub free_factor: Option<f64>,
    /// The expected downtime of this step, excluding image pulls.
    #[serde(default)]
    pub downtime_seconds: Option<u64>,
    /// The migrations this release introduced, in order.
    #[serde(default, rename = "migration")]
    pub migrations: Vec<PathMigration>,
    /// Third-party pins (UP1 Task 11).
    #[serde(default)]
    pub components: BTreeMap<String, String>,
    /// `.env` changes (UP1 Task 11).
    #[serde(default)]
    pub env: Vec<toml::Value>,
    /// The oldest app per platform (`ios`, `android`, `desktop`) this
    /// release supports; the engine serves it as
    /// `GetInstance.min_app_versions` (design §54 §13.1).
    #[serde(default)]
    pub min_app_versions: BTreeMap<String, String>,
}

/// A migration as the release train lists it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathMigration {
    pub id: String,
    pub store: Store,
    pub class: Class,
    pub summary: String,
}

/// Why a stored release cannot be upgraded to the target in one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// `upgrade_step_not_adjacent` or `release_not_found`.
    pub code: &'static str,
    pub detail: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for Refusal {}

/// A release version: `major.minor.patch[-pre]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Version<'a> {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<&'a str>,
}

fn parse_version(text: &str) -> Option<Version<'_>> {
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, Some(pre)),
        Some(_) => return None,
        None => (text, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some(Version {
        major,
        minor,
        patch,
        pre,
    })
}

/// Semver order of two releases (a pre-release before its release; numeric
/// pre-release identifiers numerically); `None` when either does not parse.
pub fn compare_releases(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let (x, y) = (parse_version(a)?, parse_version(b)?);
    let core = (x.major, x.minor, x.patch).cmp(&(y.major, y.minor, y.patch));
    if core != std::cmp::Ordering::Equal {
        return Some(core);
    }
    Some(match (x.pre, y.pre) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(p), Some(q)) => {
            let ids = |s: &'_ str| -> Vec<(u64, String)> {
                s.split('.')
                    .map(|i| {
                        i.parse()
                            .map_or((u64::MAX, i.to_string()), |n| (n, String::new()))
                    })
                    .collect()
            };
            ids(p).cmp(&ids(q))
        }
    })
}

/// A minor line, `major.minor`.
fn parse_line(text: &str) -> Option<(u64, u64)> {
    let (major, minor) = text.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

impl UpgradePath {
    /// The release train compiled into this binary.
    pub fn compiled() -> &'static UpgradePath {
        static PATH: OnceLock<UpgradePath> = OnceLock::new();
        PATH.get_or_init(|| {
            UpgradePath::parse(COMPILED).expect("release/upgrade-path.toml parses (tested)")
        })
    }

    pub fn parse(text: &str) -> Result<UpgradePath, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn release(&self, version: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.version == version)
    }

    /// `Ok` when a stack at `stored` upgrades to `target` in one step.
    pub fn check_adjacent(&self, stored: &str, target: &str) -> Result<(), Refusal> {
        let Some(release) = self.release(target) else {
            return Err(Refusal {
                code: "release_not_found",
                detail: format!("{target} is not in this binary's release train"),
            });
        };
        let refuse = |why: &str| Refusal {
            code: "upgrade_step_not_adjacent",
            detail: format!(
                "the data is at {stored}; {target} upgrades from {} ({why}); chain: {}",
                release.from,
                self.chain(target).join(" -> ")
            ),
        };
        if release.from == FRESH {
            return Err(refuse("it keeps no data from an earlier release"));
        }
        if let Some((major, minor)) = parse_line(&release.from).filter(|_| {
            // A line is exactly `major.minor`.
            release.from.matches('.').count() == 1
        }) {
            let target_version = parse_version(target)
                .filter(|v| v.pre.is_none() && v.major == major && v.minor == minor)
                .ok_or_else(|| refuse("the target is not a release of that line"))?;
            return match parse_version(stored) {
                Some(v)
                    if v.pre.is_none()
                        && v.major == major
                        && v.minor == minor
                        && v.patch < target_version.patch =>
                {
                    Ok(())
                }
                Some(v) if v.major == major && v.minor == minor && v.pre.is_none() => {
                    Err(refuse("the data is not older than the target"))
                }
                _ => Err(refuse("the data is not an earlier release of the line")),
            };
        }
        if stored == release.from {
            Ok(())
        } else {
            Err(refuse("each step goes to the next release only"))
        }
    }

    /// Whether the release that introduced migration `id` is `release` or an
    /// earlier one, by the train's order (newest first). A migration the
    /// train does not list is never "at or before".
    /// Whether the release that introduced migration `id` is after `target`
    /// (the train lists releases newest first): a run to `target` leaves it
    /// for a later release. A migration the train does not list is never
    /// "after".
    pub fn introduced_after(&self, id: &str, target: &str) -> bool {
        let introduced = self
            .releases
            .iter()
            .position(|r| r.migrations.iter().any(|m| m.id == id));
        let target = self.releases.iter().position(|r| r.version == target);
        matches!((introduced, target), (Some(i), Some(t)) if i < t)
    }

    ///
    /// A `release` newer than every release the train lists (data written by
    /// a successor, which this build's train does not know yet) includes every
    /// listed migration: the older release over newer data that `rollback
    /// --keep-writes` starts (design §54 §17, T14-2) has nothing pending.
    pub fn introduced_at_or_before(&self, id: &str, release: &str) -> bool {
        let introduced = self
            .releases
            .iter()
            .position(|r| r.migrations.iter().any(|m| m.id == id));
        let stored = self.releases.iter().position(|r| r.version == release);
        match (introduced, stored) {
            (Some(i), Some(s)) => i >= s,
            (Some(_), None) => self.releases.first().is_some_and(|newest| {
                compare_releases(release, &newest.version) == Some(std::cmp::Ordering::Greater)
            }),
            (None, _) => false,
        }
    }

    /// The releases from the first one that keeps data up to `target`,
    /// oldest first (for messages and `loams upgrade`'s chain walk).
    pub fn chain(&self, target: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut at = Some(target.to_string());
        while let Some(version) = at.take() {
            if chain.contains(&version) {
                break;
            }
            chain.push(version.clone());
            at = self
                .release(&version)
                .map(|r| r.from.clone())
                .filter(|from| from != FRESH && self.release(from).is_some());
        }
        chain.reverse();
        chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_path_parses() {
        let path = UpgradePath::compiled();
        assert!(path.release("0.1.0").is_some());
        assert_eq!(
            path.release("0.1.0-preview.2").map(|r| r.from.as_str()),
            Some(FRESH)
        );
    }

    /// A migration belongs to the release that introduced it: a run to an
    /// earlier release leaves it, a stack stamped at or after it skips it.
    #[test]
    fn migrations_belong_to_their_release() {
        let path = UpgradePath::parse(
            r#"
            [[release]]
            version = "0.1.0-preview.4"
            from = "0.1.0-preview.3"
            [[release.migration]]
            id = "0002-later"
            store = "meta"
            class = "reversible"
            summary = "later"

            [[release]]
            version = "0.1.0-preview.3"
            from = "0.1.0-preview.2"
            [[release.migration]]
            id = "0001-now"
            store = "meta"
            class = "reversible"
            summary = "now"

            [[release]]
            version = "0.1.0-preview.2"
            from = "fresh"
            "#,
        )
        .expect("parse");
        assert!(path.introduced_after("0002-later", "0.1.0-preview.3"));
        assert!(!path.introduced_after("0001-now", "0.1.0-preview.3"));
        assert!(!path.introduced_after("0002-later", "0.1.0-preview.4"));
        assert!(path.introduced_at_or_before("0001-now", "0.1.0-preview.3"));
        assert!(path.introduced_at_or_before("0001-now", "0.1.0-preview.4"));
        assert!(!path.introduced_at_or_before("0002-later", "0.1.0-preview.3"));
        // A migration the train does not list is neither.
        assert!(!path.introduced_after("9999-unlisted", "0.1.0-preview.3"));
        assert!(!path.introduced_at_or_before("9999-unlisted", "0.1.0-preview.3"));
        // Data at a successor this train does not list yet includes every
        // listed migration (the older release under `rollback --keep-writes`);
        // an unknown older release includes none.
        assert!(path.introduced_at_or_before("0001-now", "0.1.0"));
        assert!(path.introduced_at_or_before("0002-later", "0.1.1"));
        assert!(!path.introduced_at_or_before("0001-now", "0.0.9"));
        assert!(!path.introduced_at_or_before("9999-unlisted", "0.1.1"));
    }

    #[test]
    fn versions_parse() {
        assert_eq!(
            parse_version("0.1.0-preview.3"),
            Some(Version {
                major: 0,
                minor: 1,
                patch: 0,
                pre: Some("preview.3")
            })
        );
        assert_eq!(parse_version("0.1"), None);
        assert_eq!(parse_version("0.1.0-"), None);
        assert_eq!(parse_line("0.1"), Some((0, 1)));
    }
}
