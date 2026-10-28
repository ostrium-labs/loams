//! ClickHouse function names (plan HE1 Task 22, step 4).
//!
//! ClickHouse's function names are case-sensitive (`toStartOfDay`, `argMax`),
//! except the ones `system.functions` marks `case_insensitive` (`count`,
//! `CAST`, SQL-standard names), and some names are aliases of others
//! (`DATE_DIFF` of `dateDiff`). `functions.tsv` is `system.functions` of the
//! 26.9.2.8 oracle (name, is_aggregate, case_insensitive, alias_to).
//!
//! The engine registers each function with DataFusion under its canonical
//! ClickHouse name and quotes every function identifier before `SqlToRel` sees
//! it, because DataFusion lower-cases unquoted function names (dialect spike
//! §3.1). Names DataFusion's planner treats specially (`map`, `make_map`,
//! `unnest`) are registered under [`internal_name`] instead.

use std::collections::HashMap;
use std::sync::OnceLock;

const FUNCTIONS: &str = include_str!("functions.tsv");

/// The combinators ClickHouse accepts on an aggregate function's name
/// (`system.aggregate_function_combinators`), longest first so `MergeState`
/// is read before `State`.
pub const COMBINATORS: &[&str] = &[
    "SimpleState",
    "OrDefault",
    "Resample",
    "Distinct",
    "ForEach",
    "ArgMax",
    "ArgMin",
    "OrNull",
    "Array",
    "Merge",
    "State",
    "Tuple",
    "Null",
    "Map",
    "If",
];

/// A function ClickHouse knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    /// Its canonical name (an alias's target).
    pub canonical: String,
    /// Whether it is an aggregate function.
    pub aggregate: bool,
}

struct Table {
    exact: HashMap<String, Known>,
    folded: HashMap<String, Known>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let rows: Vec<(&str, bool, bool, &str)> = FUNCTIONS
            .lines()
            .filter_map(|l| {
                let mut c = l.split('\t');
                Some((
                    c.next()?,
                    c.next()? == "1",
                    c.next()? == "1",
                    c.next().unwrap_or(""),
                ))
            })
            .collect();
        let aggregate: HashMap<&str, bool> = rows.iter().map(|(n, a, _, _)| (*n, *a)).collect();
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        for (name, agg, ci, alias) in &rows {
            let canonical = if alias.is_empty() { *name } else { *alias };
            let known = Known {
                canonical: canonical.to_string(),
                aggregate: *agg || aggregate.get(canonical).copied().unwrap_or(false),
            };
            exact.insert((*name).to_string(), known.clone());
            if *ci {
                folded.insert(name.to_ascii_lowercase(), known);
            }
        }
        Table { exact, folded }
    })
}

/// One row of the oracle's `system.functions`: name, whether it is an
/// aggregate, whether its name is case-insensitive, and the function it is an
/// alias of (empty for none).
pub type FunctionRow = (&'static str, bool, bool, &'static str);

/// Every function the oracle has, in `functions.tsv`'s order (plan HE1 Task
/// 38's `system.functions` keeps the ones the engine registers).
pub fn all() -> Vec<FunctionRow> {
    FUNCTIONS
        .lines()
        .filter_map(|l| {
            let mut c = l.split('\t');
            Some((
                c.next()?,
                c.next()? == "1",
                c.next()? == "1",
                c.next().unwrap_or(""),
            ))
        })
        .collect()
}

/// The canonical ClickHouse name of a function registered under `internal`
/// (the inverse of [`internal_name`]).
pub fn canonical_of_internal(internal: &str) -> &str {
    internal.strip_prefix("__ch_").unwrap_or(internal)
}

/// What ClickHouse calls the function written `name`, if it has one.
pub fn resolve(name: &str) -> Option<Known> {
    let t = table();
    if let Some(k) = t.exact.get(name) {
        return Some(k.clone());
    }
    if let Some(k) = t.folded.get(&name.to_ascii_lowercase()) {
        return Some(k.clone());
    }
    // An aggregate with combinators (`sumIf`, `uniqMergeState`).
    for c in COMBINATORS {
        if let Some(base) = name.strip_suffix(c)
            && !base.is_empty()
            && let Some(k) = resolve(base)
            && k.aggregate
        {
            return Some(Known {
                canonical: format!("{}{c}", k.canonical),
                aggregate: true,
            });
        }
    }
    None
}

/// The name a canonical ClickHouse function is registered under with
/// DataFusion.
pub fn internal_name(canonical: &str) -> String {
    match canonical {
        "map" | "make_map" | "unnest" | "unnest_outer" => format!("__ch_{canonical}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_as_clickhouse() {
        assert_eq!(resolve("toStartOfDay").unwrap().canonical, "toStartOfDay");
        assert!(
            resolve("tostartofday").is_none(),
            "camelCase names are case-sensitive"
        );
        assert_eq!(resolve("COUNT").unwrap().canonical, "count");
        assert_eq!(resolve("DATE_DIFF").unwrap().canonical, "dateDiff");
        assert_eq!(
            resolve("countIf").unwrap(),
            Known {
                canonical: "countIf".into(),
                aggregate: true
            }
        );
        assert_eq!(
            resolve("uniqMergeState").unwrap().canonical,
            "uniqMergeState"
        );
        assert!(resolve("noSuchFunction").is_none());
        assert_eq!(internal_name("map"), "__ch_map");
        assert_eq!(resolve("mapContains").unwrap().canonical, "mapContainsKey");
    }
}
