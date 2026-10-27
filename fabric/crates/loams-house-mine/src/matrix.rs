//! The function and engine matrix (HE0 Task 8, design §75 §15 suite 5).
//!
//! Every function, table engine and table function the pinned chDB has
//! (`system.functions`, `system.table_engines`, `system.table_functions`) is
//! marked with one class:
//!
//! - `profile`: in the mined profile (`profile.toml`), or declared Tier 0 or 1
//!   by §75 §7.3–§7.4 (`classes.toml`);
//! - `refused`: Tier 2 (§75 §7.5), answered `48 NOT_IMPLEMENTED`;
//! - `disabled`: never built, for security (D1558), answered
//!   `344 SUPPORT_IS_DISABLED`;
//! - `unclassified`: none of these yet.
//!
//! `disabled` wins over `profile`, which wins over `refused`: a feature an
//! application uses that reaches outside the namespace stays disabled. An alias
//! (`alias_to`) takes its target's class when it has none of its own.
//!
//! The check (`--check`, run in CI) fails when the committed matrix differs
//! from the generated one, and when a feature the mining found (in
//! `profile.toml` or an inventory) is `unclassified`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, bail};
use serde::Deserialize;

/// The three lists the matrix covers, with the feature-kind prefix the mining
/// uses for each (`function:countIf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Function,
    Engine,
    TableFunction,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Function, Self::Engine, Self::TableFunction];

    /// The mining's prefix.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Engine => "engine",
            Self::TableFunction => "table_function",
        }
    }

    /// The dump file under the dumps directory.
    pub fn dump_file(self) -> &'static str {
        match self {
            Self::Function => "functions.tsv",
            Self::Engine => "table_engines.tsv",
            Self::TableFunction => "table_functions.tsv",
        }
    }

    /// The query that makes the dump (`TSVWithNames`).
    pub fn dump_query(self) -> &'static str {
        match self {
            Self::Function => {
                "SELECT name, is_aggregate, case_insensitive, alias_to FROM system.functions ORDER BY name FORMAT TSVWithNames"
            }
            Self::Engine => {
                "SELECT name FROM system.table_engines ORDER BY name FORMAT TSVWithNames"
            }
            Self::TableFunction => {
                "SELECT name FROM system.table_functions ORDER BY name FORMAT TSVWithNames"
            }
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Function => "Functions (`system.functions`)",
            Self::Engine => "Table engines (`system.table_engines`)",
            Self::TableFunction => "Table functions (`system.table_functions`)",
        }
    }
}

/// A matrix class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Profile,
    Refused,
    Disabled,
    Unclassified,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Refused => "refused",
            Self::Disabled => "disabled",
            Self::Unclassified => "unclassified",
        }
    }
}

/// One entry of chDB's lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub aggregate: bool,
    pub case_insensitive: bool,
    pub alias_to: Option<String>,
}

/// Reads one dump (`TSVWithNames`). The functions dump has `name`,
/// `is_aggregate`, `case_insensitive` and `alias_to`; the others `name` only.
pub fn parse_dump(kind: Kind, text: &str) -> anyhow::Result<Vec<Item>> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().context("an empty dump")?.split('\t').collect();
    let col = |name: &str| header.iter().position(|h| *h == name);
    let name_at = col("name").context("the dump has no `name` column")?;
    let aggregate_at = col("is_aggregate");
    let insensitive_at = col("case_insensitive");
    let alias_at = col("alias_to");
    let mut items = Vec::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        let get = |at: Option<usize>| at.and_then(|i| fields.get(i)).copied().unwrap_or("");
        let name = get(Some(name_at)).to_string();
        if name.is_empty() {
            bail!("a dump line without a name: {line:?}");
        }
        items.push(Item {
            kind,
            name,
            aggregate: get(aggregate_at) == "1",
            case_insensitive: get(insensitive_at) == "1",
            alias_to: Some(get(alias_at).to_string()).filter(|a| !a.is_empty()),
        });
    }
    Ok(items)
}

/// `classes.toml`: the hand-kept classes (§75's tiers and the security list).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classes {
    #[serde(rename = "rule", default)]
    pub rules: Vec<Rule>,
}

/// One `[[rule]]`: a class for named features (exact names or name prefixes).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub class: Class,
    pub kind: Kind,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub prefixes: Vec<String>,
    pub reason: String,
}

impl Classes {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let classes: Self = toml::from_str(text).context("parsing classes.toml")?;
        if let Some(rule) = classes
            .rules
            .iter()
            .find(|r| r.class == Class::Unclassified)
        {
            bail!("a rule cannot classify as unclassified ({})", rule.reason);
        }
        Ok(classes)
    }

    /// The rules that match a feature, with their class and reason.
    pub fn matches(&self, kind: Kind, name: &str) -> Vec<(Class, &str)> {
        self.rules
            .iter()
            .filter(|r| r.kind == kind)
            .filter(|r| {
                r.names.iter().any(|n| n == name)
                    || r.prefixes.iter().any(|p| name.starts_with(p.as_str()))
            })
            .map(|r| (r.class, r.reason.as_str()))
            .collect()
    }
}

/// The mining's view: the features in the profile (with an optional tier) and
/// every feature found, by `kind:name`.
#[derive(Debug, Default)]
pub struct Mined {
    pub profile: BTreeMap<String, Option<u8>>,
    pub found: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    feature: Vec<ProfileFeature>,
}

#[derive(Debug, Deserialize)]
struct ProfileFeature {
    name: String,
    #[serde(default)]
    apps: Vec<String>,
    #[serde(default)]
    tier: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct InventoryFile {
    app: String,
    #[serde(default)]
    features: BTreeMap<String, serde_json::Value>,
}

impl Mined {
    /// Reads `profile.toml` (Task 2's `report`; missing is empty) and every
    /// `*.json` inventory in `inventories` (missing is none).
    pub fn load(profile: &Path, inventories: &Path) -> anyhow::Result<Self> {
        let mut mined = Self::default();
        match std::fs::read_to_string(profile) {
            Ok(text) => {
                let file: ProfileFile = toml::from_str(&text)
                    .with_context(|| format!("parsing {}", profile.display()))?;
                for f in file.feature {
                    mined
                        .found
                        .entry(f.name.clone())
                        .or_default()
                        .extend(f.apps);
                    mined.profile.insert(f.name, f.tier);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", profile.display())),
        }
        if let Ok(dir) = std::fs::read_dir(inventories) {
            let mut files: Vec<_> = dir
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            files.sort();
            for file in files {
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading {}", file.display()))?;
                let inv: InventoryFile = serde_json::from_str(&text)
                    .with_context(|| format!("parsing {}", file.display()))?;
                for feature in inv.features.into_keys() {
                    mined
                        .found
                        .entry(feature)
                        .or_default()
                        .insert(inv.app.clone());
                }
            }
        }
        Ok(mined)
    }

    fn key(kind: Kind, name: &str) -> String {
        format!("{}:{name}", kind.prefix())
    }

    /// Finds a mined feature for `item`: exactly, or without case when the
    /// function is case-insensitive.
    fn lookup<'a, V>(map: &'a BTreeMap<String, V>, item: &Item, name: &str) -> Option<&'a V> {
        let key = Self::key(item.kind, name);
        map.get(&key).or_else(|| {
            item.case_insensitive
                .then(|| {
                    map.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(&key))
                        .map(|(_, v)| v)
                })
                .flatten()
        })
    }
}

/// One classified entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub item: Item,
    pub class: Class,
    /// Why: the rule's reason, `mined profile`, or `alias of x`.
    pub why: String,
    /// The applications the mining found using it.
    pub apps: BTreeSet<String>,
}

/// Classifies every item.
pub fn classify(items: &[Item], classes: &Classes, mined: &Mined) -> Vec<Entry> {
    let direct = |item: &Item, name: &str| -> Option<(Class, String)> {
        let rules = classes.matches(item.kind, name);
        let rule = |class: Class| {
            rules
                .iter()
                .find(|(c, _)| *c == class)
                .map(|(_, why)| (class, (*why).to_string()))
        };
        rule(Class::Disabled)
            .or_else(|| {
                Mined::lookup(&mined.profile, item, name).map(|tier| {
                    let why = match tier {
                        Some(t) => format!("mined profile, Tier {t}"),
                        None => "mined profile".to_string(),
                    };
                    (Class::Profile, why)
                })
            })
            .or_else(|| rule(Class::Profile))
            .or_else(|| rule(Class::Refused))
    };
    let by_name: BTreeMap<(Kind, &str), &Item> = items
        .iter()
        .map(|i| ((i.kind, i.name.as_str()), i))
        .collect();
    items
        .iter()
        .map(|item| {
            let (class, why) = direct(item, &item.name)
                .or_else(|| {
                    let target = item.alias_to.as_deref()?;
                    let target_item = by_name.get(&(item.kind, target)).copied().unwrap_or(item);
                    direct(target_item, target)
                        .map(|(c, why)| (c, format!("alias of `{target}`: {why}")))
                })
                .unwrap_or((Class::Unclassified, String::new()));
            let apps = Mined::lookup(&mined.found, item, &item.name)
                .cloned()
                .unwrap_or_default();
            Entry {
                item: item.clone(),
                class,
                why,
                apps,
            }
        })
        .collect()
}

/// The check's failures: mined features left unclassified.
pub fn unclassified_mined(entries: &[Entry]) -> Vec<&Entry> {
    entries
        .iter()
        .filter(|e| e.class == Class::Unclassified && !e.apps.is_empty())
        .collect()
}

/// Mined features of the three kinds that chDB does not list (the mining ran
/// on clickhouse-server, which may have more): reported, not failed.
pub fn mined_not_in_chdb(items: &[Item], mined: &Mined) -> Vec<String> {
    let known: BTreeSet<String> = items
        .iter()
        .map(|i| Mined::key(i.kind, &i.name).to_ascii_lowercase())
        .collect();
    mined
        .found
        .keys()
        .filter(|k| {
            Kind::ALL
                .iter()
                .any(|kind| k.starts_with(&format!("{}:", kind.prefix())))
        })
        .filter(|k| !known.contains(&k.to_ascii_lowercase()))
        .filter(|k| {
            // `countIf` is `count` with the `If` combinator: chDB lists the base.
            let Some(name) = k.strip_prefix("function:") else {
                return true;
            };
            let mut base = name;
            while let Some(stripped) = COMBINATORS.iter().find_map(|c| base.strip_suffix(c)) {
                base = stripped;
                if known.contains(&format!("function:{base}").to_ascii_lowercase()) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// ClickHouse's aggregate function combinators, longest first where one ends
/// another (`MergeState` before `State`).
const COMBINATORS: &[&str] = &[
    "SimpleState",
    "MergeState",
    "OrDefault",
    "Resample",
    "Distinct",
    "ForEach",
    "OrNull",
    "ArgMin",
    "ArgMax",
    "State",
    "Merge",
    "Array",
    "Map",
    "If",
];

/// The generated `docs/house/compat-matrix.md`.
pub fn render(entries: &[Entry], source: &str, not_in_chdb: &[String]) -> String {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "<!-- Generated by `loams-house-mine matrix` from chDB's system tables, \
         conformance/clickhouse/matrix/classes.toml and the mined profile; do not edit. -->\n"
    );
    let _ = writeln!(md, "# The Loams House function and engine matrix\n");
    let _ = writeln!(
        md,
        "Every function, table engine and table function of the pinned chDB ({source}), \
         each marked with what the Loams House engine does with it (design §75 §7, §15 suite 5, \
         plan HE1 Task 8):\n\n\
         - **profile**: supported: in the mined profile, or Tier 0 or 1 of §75 §7.3–§7.4;\n\
         - **refused**: Tier 2 (§75 §7.5), answered `48 NOT_IMPLEMENTED` with the feature's name;\n\
         - **disabled**: never built, for security (D1558), answered `344 SUPPORT_IS_DISABLED`;\n\
         - **unclassified**: not decided yet. CI fails when a feature the mining found is unclassified.\n"
    );
    let _ = writeln!(md, "## Summary\n");
    let _ = writeln!(
        md,
        "| List | profile | refused | disabled | unclassified | mined |\n|---|---:|---:|---:|---:|---:|"
    );
    for kind in Kind::ALL {
        let of = |class| {
            entries
                .iter()
                .filter(|e| e.item.kind == kind && e.class == class)
                .count()
        };
        let mined = entries
            .iter()
            .filter(|e| e.item.kind == kind && !e.apps.is_empty())
            .count();
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {mined} |",
            kind.title(),
            of(Class::Profile),
            of(Class::Refused),
            of(Class::Disabled),
            of(Class::Unclassified)
        );
    }
    for kind in Kind::ALL {
        let _ = writeln!(md, "\n## {}\n", kind.title());
        let rows: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.item.kind == kind && e.class != Class::Unclassified)
            .collect();
        if rows.is_empty() {
            let _ = writeln!(md, "None classified yet.");
        } else {
            let _ = writeln!(md, "| Name | Class | Why | Mined in |\n|---|---|---|---|");
            for e in rows {
                let _ = writeln!(
                    md,
                    "| `{}`{} | {} | {} | {} |",
                    e.item.name.replace('|', "\\|"),
                    if e.item.aggregate { " (aggregate)" } else { "" },
                    e.class.as_str(),
                    e.why.replace('|', "\\|"),
                    e.apps.iter().cloned().collect::<Vec<_>>().join(", ")
                );
            }
        }
        let unclassified: Vec<String> = entries
            .iter()
            .filter(|e| e.item.kind == kind && e.class == Class::Unclassified)
            .map(|e| {
                if e.apps.is_empty() {
                    format!("`{}`", e.item.name)
                } else {
                    format!("**`{}`** (mined)", e.item.name)
                }
            })
            .collect();
        if !unclassified.is_empty() {
            let _ = writeln!(
                md,
                "\nUnclassified ({}): {}",
                unclassified.len(),
                unclassified.join(", ")
            );
        }
    }
    if !not_in_chdb.is_empty() {
        let _ = writeln!(md, "\n## Mined, not in chDB\n");
        let _ = writeln!(
            md,
            "Found by the mining (on `clickhouse-server`) and absent from chDB's lists: {}",
            not_in_chdb
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    md
}

/// Reads the dumps directory: the three lists and `source.txt`.
pub fn load_dumps(dir: &Path) -> anyhow::Result<(Vec<Item>, String)> {
    let mut items = Vec::new();
    for kind in Kind::ALL {
        let path = dir.join(kind.dump_file());
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        items.extend(parse_dump(kind, &text).with_context(|| format!("in {}", path.display()))?);
    }
    let source_path = dir.join("source.txt");
    let source = std::fs::read_to_string(&source_path)
        .with_context(|| format!("reading {}", source_path.display()))?
        .trim()
        .to_string();
    Ok((items, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_parse_with_their_header() {
        let items = parse_dump(
            Kind::Function,
            "name\tis_aggregate\tcase_insensitive\talias_to\ncount\t1\t1\t\nlcase\t0\t1\tlower\n",
        )
        .expect("dump");
        assert_eq!(items.len(), 2);
        assert!(items[0].aggregate && items[0].case_insensitive);
        assert_eq!(items[1].alias_to.as_deref(), Some("lower"));
        assert!(parse_dump(Kind::Engine, "").is_err());
    }
}
