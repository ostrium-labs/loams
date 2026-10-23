//! The KV gateways' test harness (design §77 §12, D1661; KV1 plan KV0
//! Task 8).
//!
//! A [`Scenario`] runs against a [`loams_kv::Store`] and returns a
//! [`Report`]: what it observed, in a form two backends can be compared on.
//! [`run_everywhere`] runs one scenario on every available [`Backend`]
//! (embedded and in-memory always, TiKV when `LOAMS_TEST_PD` names a
//! cluster) and [`Comparison::agree`] says whether they all saw the same.
//! The `loams-gw-test` binary lists and runs the [`scenarios`].
//!
//! The suites that drive the gateways themselves are placeholders until the
//! gateways exist; each module in [`runners`] says what it will run and
//! when.

pub mod runners;
pub mod scenarios;

use std::collections::BTreeMap;
use std::fmt;

use futures::future::BoxFuture;
use loams_kv::{Store, testing};

/// A backend a scenario runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Backend {
    /// The embedded store in a redb file.
    Embedded,
    /// The embedded store on redb's in-memory backend.
    Memory,
    /// TiKV, when `LOAMS_TEST_PD` names a cluster.
    Tikv,
}

impl Backend {
    /// Every backend, in report order.
    pub const ALL: [Backend; 3] = [Backend::Embedded, Backend::Memory, Backend::Tikv];

    /// The backend's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Backend::Embedded => "embedded",
            Backend::Memory => "memory",
            Backend::Tikv => "tikv",
        }
    }

    /// A fresh store of this backend under a fresh random root, or `None`
    /// when it is unavailable (TiKV without `LOAMS_TEST_PD`, after the
    /// factory's `skipped:` line).
    ///
    /// # Panics
    ///
    /// When an available backend's store cannot be opened.
    pub async fn store(self) -> Option<Store> {
        let factory = match self {
            Backend::Embedded => {
                return Some(testing::embedded().await);
            }
            Backend::Memory => testing::memory_factory(),
            Backend::Tikv => testing::tikv_factory(),
        };
        factory.store(testing::Spec::fresh()).await
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a scenario observed: named facts, compared exactly across
/// backends. Keep timestamps, roots and other per-run values out of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    facts: BTreeMap<String, String>,
}

impl Report {
    /// An empty report.
    pub fn new() -> Self {
        Report::default()
    }

    /// Records `value` (its `Debug` form) under `name`, replacing an earlier
    /// fact of that name.
    pub fn fact(&mut self, name: impl Into<String>, value: impl fmt::Debug) -> &mut Self {
        self.facts.insert(name.into(), format!("{value:?}"));
        self
    }

    /// The facts, by name.
    pub fn facts(&self) -> &BTreeMap<String, String> {
        &self.facts
    }

    /// The facts on which `self` and `other` differ: (name, ours, theirs),
    /// `None` where one has no fact of that name.
    pub fn diff<'a>(
        &'a self,
        other: &'a Report,
    ) -> Vec<(&'a str, Option<&'a str>, Option<&'a str>)> {
        let names: std::collections::BTreeSet<&String> =
            self.facts.keys().chain(other.facts.keys()).collect();
        names
            .into_iter()
            .filter_map(|name| {
                let (a, b) = (self.facts.get(name), other.facts.get(name));
                (a != b).then(|| (name.as_str(), a.map(String::as_str), b.map(String::as_str)))
            })
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, value) in &self.facts {
            writeln!(f, "{name} = {value}")?;
        }
        Ok(())
    }
}

/// A scenario: a workload on one store and the report of what it saw.
/// Object-safe, so the binary can list them.
pub trait Scenario: Send + Sync {
    /// The scenario's name, for `loams-gw-test run <name>`.
    fn name(&self) -> &'static str;

    /// One line saying what it checks.
    fn about(&self) -> &'static str;

    /// Runs on `store` (a fresh root) and reports.
    fn run<'a>(&'a self, store: &'a Store) -> BoxFuture<'a, Report>;
}

/// One scenario's reports, one per backend that ran it.
#[derive(Debug, Clone)]
pub struct Comparison {
    pub scenario: &'static str,
    pub reports: Vec<(Backend, Report)>,
    /// Backends that were not available.
    pub skipped: Vec<Backend>,
}

impl Comparison {
    /// Whether every backend that ran reported the same.
    pub fn agree(&self) -> bool {
        self.reports.windows(2).all(|w| w[0].1 == w[1].1)
    }

    /// The differences of each backend's report from the first one's.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        let Some((first, base)) = self.reports.first() else {
            return format!("{}: no backend ran\n", self.scenario);
        };
        for (backend, report) in &self.reports[1..] {
            for (name, a, b) in base.diff(report) {
                out.push_str(&format!(
                    "{}: {name}: {first} {a:?}, {backend} {b:?}\n",
                    self.scenario
                ));
            }
        }
        if out.is_empty() {
            let ran: Vec<&str> = self.reports.iter().map(|(b, _)| b.name()).collect();
            out = format!("{}: the same on {}\n", self.scenario, ran.join(", "));
        }
        out
    }
}

/// Runs `scenario` on every available backend, each on a fresh store.
pub async fn run_everywhere(scenario: &dyn Scenario) -> Comparison {
    let mut reports = Vec::new();
    let mut skipped = Vec::new();
    for backend in Backend::ALL {
        match backend.store().await {
            Some(store) => reports.push((backend, scenario.run(&store).await)),
            None => skipped.push(backend),
        }
    }
    Comparison {
        scenario: scenario.name(),
        reports,
        skipped,
    }
}
