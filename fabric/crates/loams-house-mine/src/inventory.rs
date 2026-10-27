//! An application's feature inventory (`inventory/<app>.json`).
//!
//! Every statement is read once, by the first path that accepts it:
//! sqlparser's ClickHouse dialect ([`crate::ast`]), else the pinned server's
//! `EXPLAIN AST` ([`crate::explain`]), else it is counted as unparsed. No
//! statement is dropped: `statements.total` is always `parsed + explained +
//! unparsed`.
//!
//! Statements whose scrubbed text and client settings are equal are one
//! sample. The sample's id is its first source's (`q:<query_id>` for a query
//! log row, `m:<path>:<line>` for a migration statement) and `executions`
//! counts its sources. A feature's `count` is the sum of `executions` over
//! the samples in its `sources`, so every count traces to the queries and
//! migration lines it came from (Review Focus 1).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::explain::Explainer;
use crate::features::Features;
use crate::{ast, explain, lexical, scrub};

/// The inventory file's schema id.
pub const SCHEMA: &str = "loams-house-mine/inventory/1";

/// Settings the harness's server profile changes, which `query_log` reports
/// on every query of every application; they are never the application's.
pub const HARNESS_SETTINGS: [&str; 4] = [
    "log_queries",
    "log_query_threads",
    "log_processors_profiles",
    "log_queries_min_type",
];

/// One statement to mine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `q:<query_id>` or `m:<path>:<line>`.
    pub id: String,
    /// The statement.
    pub text: String,
    /// Settings the client changed for it (query log only).
    pub settings: Vec<String>,
}

/// How a statement was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Path {
    /// sqlparser's ClickHouse dialect.
    Parsed,
    /// The server's `EXPLAIN AST`.
    Explained,
    /// Neither: counted, with no features but the client's settings.
    Unparsed,
}

/// A feature's uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    /// Statements that use it (executions, not distinct texts).
    pub count: u64,
    /// The ids of the samples that use it.
    pub sources: Vec<String>,
}

/// A distinct statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// The scrubbed text; `None` when it could not be tokenized, so no text
    /// is kept.
    pub query: Option<String>,
    /// How it was read.
    pub path: Path,
    /// How many sources it stands for.
    pub executions: u64,
}

/// Statement counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Every statement read.
    pub total: u64,
    /// Read by sqlparser.
    pub parsed: u64,
    /// Read by `EXPLAIN AST`.
    pub explained: u64,
    /// Read by neither.
    pub unparsed: u64,
}

/// Rows of the query log that were not the application's statements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedRows {
    /// `ExceptionBeforeStart`.
    pub failed_before_start: u64,
    /// `is_initial_query = 0`.
    pub not_initial: u64,
    /// A second row for one `query_id`.
    pub duplicate: u64,
    /// The harness's own reads of the logs.
    pub harness: u64,
}

impl From<crate::querylog::Skipped> for SkippedRows {
    fn from(s: crate::querylog::Skipped) -> Self {
        Self {
            failed_before_start: s.failed_before_start,
            not_initial: s.not_initial,
            duplicate: s.duplicate,
            harness: s.harness,
        }
    }
}

/// `inventory/<app>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// [`SCHEMA`].
    pub schema: String,
    /// The application, as the apps manifest names it (`langfuse`).
    pub app: String,
    /// The application version or variant mined (`v3`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Statement counts by path.
    pub statements: Counts,
    /// Query-log rows skipped, by reason.
    #[serde(default)]
    pub skipped: SkippedRows,
    /// Features, `kind:name`, and their uses.
    pub features: BTreeMap<String, Uses>,
    /// The statements neither parser read.
    pub unparsed: Uses,
    /// Distinct statements by id.
    pub samples: BTreeMap<String, Sample>,
}

/// Options for [`build`].
#[derive(Debug, Default)]
pub struct Options {
    /// Setting names never recorded (in addition to [`HARNESS_SETTINGS`]).
    pub ignore_settings: BTreeSet<String>,
}

/// The features one statement uses and how it was read.
pub fn read_statement(text: &str, mut explainer: Option<&mut dyn Explainer>) -> (Features, Path) {
    let first: Option<&mut dyn Explainer> = match explainer.as_mut() {
        Some(e) => Some(&mut **e),
        None => None,
    };
    let (features, path) = read_one(text, first);
    if path != Path::Unparsed {
        return (features, path);
    }
    // A migration template: read it with its `{PLACEHOLDER}`s removed
    // (HE0 Task 2 ruling R2.13).
    if let Some(stripped) = lexical::strip_template_placeholders(text) {
        let (mut features, path) = read_one(&stripped, explainer);
        if path != Path::Unparsed {
            features.add("syntax", "template placeholder");
            return (features, path);
        }
    }
    (features, path)
}

fn read_one(text: &str, explainer: Option<&mut dyn Explainer>) -> (Features, Path) {
    if let Some(mut f) = ast::features(text) {
        f.extend(lexical::features(text));
        return (f, Path::Parsed);
    }
    if let Some(explainer) = explainer
        && let Some(dump) = explainer.explain_ast(text)
    {
        let mut f = explain::features(&dump);
        f.extend(explain::lexical_extras(text));
        f.extend(lexical::features(text));
        return (f, Path::Explained);
    }
    (Features::default(), Path::Unparsed)
}

/// The names the sources define with `CREATE FUNCTION`.
pub fn defined_functions(sources: &[Source]) -> BTreeSet<String> {
    sources
        .iter()
        .filter_map(|s| lexical::defined_function(&s.text))
        .collect()
}

/// Whether a setting is never recorded.
fn ignored_setting(name: &str, options: &Options) -> bool {
    HARNESS_SETTINGS.contains(&name) || options.ignore_settings.contains(name)
}

/// The features one source contributes: its statement's, its client
/// settings, less the harness's settings, with calls to the application's
/// own functions (`udfs`) recorded as `syntax:user-defined function call`.
pub fn mine_statement(
    source: &Source,
    explainer: Option<&mut dyn Explainer>,
    udfs: &BTreeSet<String>,
    options: &Options,
) -> (Features, Path) {
    let (mut features, path) = read_statement(&source.text, explainer);
    for s in &source.settings {
        features.add("setting", s);
    }
    features.0.retain(|f| {
        f.strip_prefix("setting:")
            .is_none_or(|name| !ignored_setting(name, options))
    });
    let before = features.0.len();
    features.0.retain(|f| {
        f.strip_prefix("function:")
            .is_none_or(|name| !udfs.contains(name))
    });
    if features.0.len() != before {
        features.add("syntax", "user-defined function call");
    }
    (features, path)
}

/// Mines `sources` into an inventory.
pub fn build(
    app: &str,
    variant: Option<&str>,
    sources: &[Source],
    mut explainer: Option<&mut dyn Explainer>,
    options: &Options,
) -> Inventory {
    struct Group {
        id: String,
        query: Option<String>,
        features: Features,
        path: Path,
        executions: u64,
    }
    let ignored = |name: &str| ignored_setting(name, options);
    // The application's own SQL functions (`CREATE FUNCTION`): calls to them
    // are not ClickHouse functions (HE0 Task 5 ruling).
    let udfs = defined_functions(sources);
    // Source ids name one statement each; a repeated id would merge two
    // statements' counts.
    debug_assert!(
        {
            let mut seen = BTreeSet::new();
            sources.iter().all(|s| seen.insert(s.id.as_str()))
        },
        "source ids repeat"
    );
    let mut groups: Vec<Group> = Vec::new();
    let mut by_key: HashMap<(String, Vec<String>), usize> = HashMap::new();
    for source in sources {
        let scrubbed = scrub::scrub(&source.text)
            .ok()
            .map(|s| s.text)
            .filter(|s| scrub::is_literal_free(s));
        let settings: Vec<String> = source
            .settings
            .iter()
            .filter(|s| !ignored(s))
            .cloned()
            .collect();
        // Unscrubbable text groups by its raw text; that key never leaves
        // this function.
        let key = (
            scrubbed.clone().unwrap_or_else(|| source.text.clone()),
            settings.clone(),
        );
        if let Some(&g) = by_key.get(&key) {
            groups[g].executions += 1;
            continue;
        }
        let reborrow: Option<&mut dyn Explainer> = match explainer.as_mut() {
            Some(e) => Some(&mut **e),
            None => None,
        };
        let (features, path) = mine_statement(source, reborrow, &udfs, options);
        by_key.insert(key, groups.len());
        groups.push(Group {
            id: source.id.clone(),
            query: scrubbed,
            features,
            path,
            executions: 1,
        });
    }

    let mut inventory = Inventory {
        schema: SCHEMA.to_owned(),
        app: app.to_owned(),
        variant: variant.map(str::to_owned),
        statements: Counts::default(),
        skipped: SkippedRows::default(),
        features: BTreeMap::new(),
        unparsed: Uses::default(),
        samples: BTreeMap::new(),
    };
    for g in groups {
        let c = &mut inventory.statements;
        c.total += g.executions;
        match g.path {
            Path::Parsed => c.parsed += g.executions,
            Path::Explained => c.explained += g.executions,
            Path::Unparsed => {
                c.unparsed += g.executions;
                inventory.unparsed.count += g.executions;
                inventory.unparsed.sources.push(g.id.clone());
            }
        }
        for f in g.features.0 {
            let uses = inventory.features.entry(f).or_default();
            uses.count += g.executions;
            uses.sources.push(g.id.clone());
        }
        inventory.samples.insert(
            g.id,
            Sample {
                query: g.query,
                path: g.path,
                executions: g.executions,
            },
        );
    }
    for uses in inventory.features.values_mut() {
        uses.sources.sort();
    }
    inventory.unparsed.sources.sort();
    inventory
}

/// The invariants a written inventory keeps, checked before it is written
/// and by `merge` when it is read: every feature's count is the executions
/// of its sources, every source is a sample, the statement counts add up,
/// and no sample holds a literal.
///
/// # Errors
///
/// The first invariant broken.
pub fn check(inventory: &Inventory) -> anyhow::Result<()> {
    use anyhow::{bail, ensure};
    ensure!(
        inventory.schema == SCHEMA,
        "{}: schema {:?}, expected {SCHEMA:?}",
        inventory.app,
        inventory.schema
    );
    let c = inventory.statements;
    ensure!(
        c.total == c.parsed + c.explained + c.unparsed,
        "{}: statement counts do not add up",
        inventory.app
    );
    let executions = |sources: &[String]| -> anyhow::Result<u64> {
        let mut sum = 0;
        for s in sources {
            match inventory.samples.get(s) {
                Some(sample) => sum += sample.executions,
                None => bail!("{}: source {s} is not a sample", inventory.app),
            }
        }
        Ok(sum)
    };
    for (feature, uses) in &inventory.features {
        ensure!(
            !uses.sources.is_empty(),
            "{}: {feature} has no source",
            inventory.app
        );
        ensure!(
            executions(&uses.sources)? == uses.count,
            "{}: {feature}'s count is not its sources' executions",
            inventory.app
        );
    }
    ensure!(
        executions(&inventory.unparsed.sources)? == inventory.unparsed.count
            && inventory.unparsed.count == c.unparsed,
        "{}: unparsed count does not trace to its sources",
        inventory.app
    );
    let sampled: u64 = inventory.samples.values().map(|s| s.executions).sum();
    ensure!(
        sampled == c.total,
        "{}: samples do not cover every statement",
        inventory.app
    );
    for (id, sample) in &inventory.samples {
        if let Some(q) = &sample.query {
            ensure!(
                scrub::is_literal_free(q),
                "{}: sample {id} holds a literal",
                inventory.app
            );
        }
    }
    Ok(())
}
