//! `triage.toml`: every mismatch the oracle has seen, with its status.
//!
//! ```toml
//! [[mismatch]]
//! case = "select/window"
//! target = "house-chdb"
//! kind = "values"            # error | names | types | row_count | values
//! status = "incompatibility" # bug | incompatibility | untriaged
//! reason = "…"               # required for an incompatibility
//! issue = "#1234"            # optional
//! task = "24"                # optional: the plan HE1 task that owns a bug (D1804)
//! detail = "row 3: …"        # written by the runner
//! ```
//!
//! An entry is keyed by (case, target, kind). The runner keeps the status,
//! reason and issue of a mismatch it sees again, adds a new one as
//! `untriaged`, and drops an entry it no longer sees (for the targets and cases
//! it ran). The run fails on any `untriaged` entry and on an `incompatibility`
//! without a reason (`untriaged_mismatch_fails_the_run`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use super::normalise::Kind;

/// A triage decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A bug in the target (or in the oracle's corpus), to be fixed.
    Bug,
    /// A documented difference, kept on purpose; `reason` says why.
    Incompatibility,
    /// Not looked at yet: fails the run.
    Untriaged,
}

/// One mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub case: String,
    pub target: String,
    pub kind: Kind,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// The plan HE1/HE2 task that owns a `bug` (D1804): `--task N` fails while
    /// any bug names task N.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default)]
    pub detail: String,
}

impl Entry {
    fn key(&self) -> (String, String, Kind) {
        (self.case.clone(), self.target.clone(), self.kind)
    }
}

/// The file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Triage {
    #[serde(rename = "mismatch", default)]
    pub entries: Vec<Entry>,
}

const HEADER: &str = "\
# The oracle's triage (HE0 Task 6, design §75 §15): every mismatch between the
# baseline (the pinned clickhouse-server 26.9) and another target.
# Written by `loams-house-mine oracle`; edit `status`, `reason` and `issue` by
# hand. `untriaged` fails the run; an `incompatibility` needs a `reason`.
";

impl Triage {
    /// Reads the file; a missing file is an empty triage.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Writes the file, entries sorted by case, target and kind.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let body = toml::to_string(self).context("serialising the triage")?;
        std::fs::write(path, format!("{HEADER}\n{body}"))
            .with_context(|| format!("writing {}", path.display()))
    }

    /// Why this triage fails a run: untriaged entries and undocumented
    /// incompatibilities. Empty when the run passes.
    pub fn failures(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|e| match e.status {
                Status::Untriaged => Some(format!(
                    "untriaged: {} on {} ({}): {}",
                    e.case,
                    e.target,
                    e.kind.as_str(),
                    e.detail
                )),
                Status::Incompatibility
                    if e.reason.as_deref().is_none_or(|r| r.trim().is_empty()) =>
                {
                    Some(format!(
                        "incompatibility without a reason: {} on {} ({})",
                        e.case,
                        e.target,
                        e.kind.as_str()
                    ))
                }
                _ => None,
            })
            .collect()
    }
}

impl Triage {
    /// The bugs that name `task` (D1804): a task merges with none.
    pub fn bugs_of_task(&self, task: &str) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| e.status == Status::Bug && e.task.as_deref() == Some(task))
            .collect()
    }
}

/// A mismatch one run found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub case: String,
    pub target: String,
    pub kind: Kind,
    pub detail: String,
}

/// What a run covered: entries outside it are kept as they are.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub cases: BTreeSet<String>,
    pub targets: BTreeSet<String>,
}

/// The new triage after a run, and the entries the run no longer saw.
#[derive(Debug)]
pub struct Reconciled {
    pub triage: Triage,
    pub resolved: Vec<Entry>,
}

/// Merges a run's mismatches into the previous triage.
pub fn reconcile(previous: &Triage, found: Vec<Found>, scope: &Scope) -> Reconciled {
    let mut known: BTreeMap<(String, String, Kind), Entry> = previous
        .entries
        .iter()
        .map(|e| (e.key(), e.clone()))
        .collect();
    let mut next: BTreeMap<(String, String, Kind), Entry> = BTreeMap::new();
    for f in found {
        let key = (f.case.clone(), f.target.clone(), f.kind);
        let entry = match known.remove(&key) {
            Some(mut e) => {
                e.detail = f.detail;
                e
            }
            None => Entry {
                case: f.case,
                target: f.target,
                kind: f.kind,
                status: Status::Untriaged,
                reason: None,
                issue: None,
                task: None,
                detail: f.detail,
            },
        };
        next.insert(key, entry);
    }
    let mut resolved = Vec::new();
    for (key, entry) in known {
        if scope.cases.contains(&entry.case) && scope.targets.contains(&entry.target) {
            resolved.push(entry);
        } else {
            next.insert(key, entry);
        }
    }
    Reconciled {
        triage: Triage {
            entries: next.into_values().collect(),
        },
        resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(case: &str, kind: Kind) -> Found {
        Found {
            case: case.into(),
            target: "house-chdb".into(),
            kind,
            detail: "d".into(),
        }
    }

    #[test]
    fn statuses_survive_and_stale_entries_drop_within_scope_only() {
        let previous = Triage {
            entries: vec![
                Entry {
                    case: "s/a".into(),
                    target: "house-chdb".into(),
                    kind: Kind::Values,
                    status: Status::Bug,
                    reason: Some("known".into()),
                    issue: None,
                    task: None,
                    detail: "old".into(),
                },
                Entry {
                    case: "s/gone".into(),
                    target: "house-chdb".into(),
                    kind: Kind::Types,
                    status: Status::Bug,
                    reason: None,
                    issue: None,
                    task: None,
                    detail: String::new(),
                },
                Entry {
                    case: "other/x".into(),
                    target: "house-chdb".into(),
                    kind: Kind::Types,
                    status: Status::Bug,
                    reason: None,
                    issue: None,
                    task: None,
                    detail: String::new(),
                },
            ],
        };
        let scope = Scope {
            cases: ["s/a", "s/b", "s/gone"].map(String::from).into(),
            targets: ["house-chdb".to_string()].into(),
        };
        let r = reconcile(
            &previous,
            vec![
                found("s/a", Kind::Values),
                found("s/a", Kind::RowCount),
                found("s/b", Kind::Error),
            ],
            &scope,
        );
        let statuses: Vec<_> = r
            .triage
            .entries
            .iter()
            .map(|e| (e.case.as_str(), e.kind, e.status))
            .collect();
        assert_eq!(
            statuses,
            [
                ("other/x", Kind::Types, Status::Bug),
                ("s/a", Kind::RowCount, Status::Untriaged),
                ("s/a", Kind::Values, Status::Bug),
                ("s/b", Kind::Error, Status::Untriaged),
            ]
        );
        assert_eq!(r.resolved.len(), 1);
        assert_eq!(r.resolved[0].case, "s/gone");
        assert_eq!(r.triage.failures().len(), 2);
    }

    #[test]
    fn the_file_round_trips() {
        let t = Triage {
            entries: vec![Entry {
                case: "s/a".into(),
                target: "clickhouse-server@24.3".into(),
                kind: Kind::RowCount,
                status: Status::Incompatibility,
                reason: Some("24.3 lacks it".into()),
                issue: None,
                task: Some("24".into()),
                detail: "1 rows vs 2 rows".into(),
            }],
        };
        let text = toml::to_string(&t).expect("toml");
        assert!(text.contains("[[mismatch]]"), "{text}");
        assert!(text.contains("kind = \"row_count\""), "{text}");
        assert_eq!(toml::from_str::<Triage>(&text).expect("parse"), t);
    }

    #[test]
    fn task_bugs_block_their_task() {
        let entry = |status, task: Option<&str>| Entry {
            case: "he1/a".into(),
            target: "house-rust".into(),
            kind: Kind::Types,
            status,
            reason: Some("r".into()),
            issue: None,
            task: task.map(String::from),
            detail: String::new(),
        };
        let t = Triage {
            entries: vec![
                entry(Status::Bug, Some("24")),
                entry(Status::Bug, Some("30")),
                entry(Status::Incompatibility, Some("24")),
                entry(Status::Bug, None),
            ],
        };
        assert_eq!(t.bugs_of_task("24").len(), 1);
        assert_eq!(t.bugs_of_task("30").len(), 1);
        assert!(t.bugs_of_task("25").is_empty());
    }
}
