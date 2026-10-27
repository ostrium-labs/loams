//! Merging inventories into a ranking, and the profile written from it
//! (§75 §7.1).
//!
//! Features are ranked by the number of applications that use them, then by
//! how many statements do. A feature is in the profile if `min_apps` (3) or
//! more applications use it, or if a gate application (Langfuse, OpenPanel,
//! SigNoz: D1571) does. Variants of one application (Langfuse v3 and v4) count as one
//! application.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::features::KINDS;
use crate::inventory::{Counts, Inventory};
use crate::matrix::Classes;
use crate::tiers::{self, Placement};

/// The ranking file's schema id.
pub const RANKING_SCHEMA: &str = "loams-house-mine/ranking/1";
/// The profile file's schema id.
pub const PROFILE_SCHEMA: &str = "loams-house-mine/profile/1";

/// One application in a ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSummary {
    /// Its name.
    pub app: String,
    /// The variants mined, if any.
    pub variants: Vec<String>,
    /// Its statements, summed over variants.
    pub statements: Counts,
}

/// One feature in a ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranked {
    /// `kind:name`.
    pub name: String,
    /// The applications that use it.
    pub apps: Vec<String>,
    /// Statements that use it, over every application.
    pub frequency: u64,
    /// Whether it is in the profile.
    pub in_profile: bool,
    /// Why: `apps>=3`, `gate:langfuse`, or `below threshold`.
    pub reason: String,
}

/// The output of `merge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranking {
    /// [`RANKING_SCHEMA`].
    pub schema: String,
    /// The application threshold.
    pub min_apps: usize,
    /// The gate applications.
    pub gate: Vec<String>,
    /// The applications merged.
    pub apps: Vec<AppSummary>,
    /// Every feature, ranked.
    pub features: Vec<Ranked>,
}

/// Merges inventories.
pub fn merge(inventories: &[Inventory], min_apps: usize, gate: &[String]) -> Ranking {
    let mut apps: BTreeMap<String, AppSummary> = BTreeMap::new();
    let mut uses: BTreeMap<&str, (BTreeSet<&str>, u64)> = BTreeMap::new();
    for inv in inventories {
        let summary = apps.entry(inv.app.clone()).or_insert_with(|| AppSummary {
            app: inv.app.clone(),
            variants: Vec::new(),
            statements: Counts::default(),
        });
        if let Some(v) = &inv.variant {
            summary.variants.push(v.clone());
        }
        let s = &mut summary.statements;
        s.total += inv.statements.total;
        s.parsed += inv.statements.parsed;
        s.explained += inv.statements.explained;
        s.unparsed += inv.statements.unparsed;
        for (feature, u) in &inv.features {
            let entry = uses.entry(feature).or_default();
            entry.0.insert(&inv.app);
            entry.1 += u.count;
        }
    }
    let mut features: Vec<Ranked> = uses
        .into_iter()
        .map(|(name, (users, frequency))| {
            let gate_user = gate.iter().find(|g| users.contains(g.as_str()));
            let (in_profile, reason) = if users.len() >= min_apps {
                (true, format!("apps>={min_apps}"))
            } else if let Some(g) = gate_user {
                (true, format!("gate:{g}"))
            } else {
                (false, "below threshold".to_owned())
            };
            Ranked {
                name: name.to_owned(),
                apps: users.into_iter().map(str::to_owned).collect(),
                frequency,
                in_profile,
                reason,
            }
        })
        .collect();
    features.sort_by(|a, b| {
        b.apps
            .len()
            .cmp(&a.apps.len())
            .then(b.frequency.cmp(&a.frequency))
            .then(a.name.cmp(&b.name))
    });
    for summary in apps.values_mut() {
        summary.variants.sort();
    }
    Ranking {
        schema: RANKING_SCHEMA.to_owned(),
        min_apps,
        gate: gate.to_vec(),
        apps: apps.into_values().collect(),
        features,
    }
}

/// `profile.toml`: the features the House supports, with the tier the
/// mining proposes (Task 5): Tier 0 by §75 §7.1's rule, Tier 1 for features
/// fewer applications use. Refused and disabled features are not in it; the
/// matrix's rules classify them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// [`PROFILE_SCHEMA`].
    pub schema: String,
    /// The application threshold.
    pub min_apps: usize,
    /// The gate applications.
    pub gate: Vec<String>,
    /// The applications mined.
    pub apps: Vec<String>,
    /// The features, ranked.
    pub feature: Vec<ProfileFeature>,
}

/// One feature of `profile.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFeature {
    /// `kind:name`.
    pub name: String,
    /// The proposed tier: 0 (in the profile by §75 §7.1) or 1.
    #[serde(default)]
    pub tier: u8,
    /// The applications that use it.
    pub apps: Vec<String>,
    /// Statements that use it.
    pub frequency: u64,
    /// Why it has its tier.
    pub reason: String,
}

impl Profile {
    /// The profile of a ranking: Tier 0 and the Tier 1 proposals, each
    /// placed by [`tiers::propose`] against `classes`.
    pub fn from_ranking(ranking: &Ranking, classes: &Classes) -> Self {
        let feature = ranking
            .features
            .iter()
            .filter_map(|f| {
                let tier = match tiers::propose(classes, &f.name, f.in_profile) {
                    Placement::Tier0 => 0,
                    Placement::Tier1 => 1,
                    Placement::Refused | Placement::Disabled => return None,
                };
                Some(ProfileFeature {
                    name: f.name.clone(),
                    tier,
                    apps: f.apps.clone(),
                    frequency: f.frequency,
                    reason: if tier == 0 {
                        f.reason.clone()
                    } else {
                        format!(
                            "Tier 1 proposal: {} application(s), below {}",
                            f.apps.len(),
                            ranking.min_apps
                        )
                    },
                })
            })
            .collect();
        Self {
            schema: PROFILE_SCHEMA.to_owned(),
            min_apps: ranking.min_apps,
            gate: ranking.gate.clone(),
            apps: ranking.apps.iter().map(|a| a.app.clone()).collect(),
            feature,
        }
    }

    /// Whether `feature` (`kind:name`) is in the profile.
    pub fn contains(&self, feature: &str) -> bool {
        self.feature.iter().any(|f| f.name == feature)
    }
}

/// The generated `docs/house/compat-profile.md`. `findings` is the
/// hand-written findings section (`conformance/clickhouse/profile/findings.md`).
pub fn render_markdown(ranking: &Ranking, classes: &Classes, findings: Option<&str>) -> String {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "<!-- Generated by `loams-house-mine report` from the mined inventories; do not edit. -->\n"
    );
    let _ = writeln!(md, "# The Loams House compatibility profile\n");
    let _ = writeln!(
        md,
        "The ClickHouse features real applications use, mined from their query logs and migrations (design §75 §7.1, plan HE1 Tasks 2 and 5). A feature is in the profile (proposed Tier 0) when {} or more applications use it, or when a gate application ({}) does; a feature fewer applications use is proposed for Tier 1 unless §75 refuses or disables it. Variants of one application count once.\n",
        ranking.min_apps,
        ranking.gate.join(", ")
    );
    let _ = writeln!(md, "## Applications\n");
    let _ = writeln!(
        md,
        "| Application | Variants | Statements | sqlparser | EXPLAIN AST | Unparsed |\n|---|---|---:|---:|---:|---:|"
    );
    for a in &ranking.apps {
        let s = a.statements;
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} |",
            a.app,
            a.variants.join(", "),
            s.total,
            s.parsed,
            s.explained,
            s.unparsed
        );
    }
    let placed: Vec<(&Ranked, Placement)> = ranking
        .features
        .iter()
        .map(|f| (f, tiers::propose(classes, &f.name, f.in_profile)))
        .collect();
    let count = |p: Placement| placed.iter().filter(|(_, q)| *q == p).count();
    let _ = writeln!(md, "\n## Tier proposals\n");
    let _ = writeln!(
        md,
        "| Proposal | Features |\n|---|---:|\n| Tier 0 | {} |\n| Tier 1 | {} |\n| Tier 2 (refused) | {} |\n| disabled | {} |",
        count(Placement::Tier0),
        count(Placement::Tier1),
        count(Placement::Refused),
        count(Placement::Disabled)
    );
    if let Some(findings) = findings {
        let _ = writeln!(md, "\n{}", findings.trim_end());
    }
    let _ = writeln!(md, "\n## Where the mining and §75 disagree\n");
    let _ = writeln!(
        md,
        "Listed for the owner, not resolved (plan HE1 Task 5). \"§75\" is the placement in design §75 §7.3–§7.5 and D1558, from `conformance/clickhouse/matrix/classes.toml` and the extractor's table of §75's syntax, types and formats.\n"
    );
    let _ = writeln!(
        md,
        "| Feature | Applications | Mining proposes | §75 says | §75's reason |\n|---|---|---|---|---|"
    );
    let mut any = false;
    for (f, proposal) in &placed {
        if let Some((s75, reason)) = tiers::section_75(classes, &f.name)
            && s75 != *proposal
        {
            any = true;
            let _ = writeln!(
                md,
                "| `{}` | {} ({}) | {} | {} | {} |",
                f.name.replace('|', "\\|"),
                f.apps.len(),
                f.apps.join(", "),
                proposal.label(),
                s75.label(),
                reason.replace('|', "\\|")
            );
        }
    }
    let mined: std::collections::BTreeSet<&str> =
        ranking.features.iter().map(|f| f.name.as_str()).collect();
    for (feature, s75) in tiers::SECTION_75 {
        if *s75 == Placement::Tier0 && !mined.contains(feature) {
            any = true;
            let _ = writeln!(
                md,
                "| `{feature}` | 0 | not mined | Tier 0 | §75 §7.3 names it; no mined application used it |"
            );
        }
    }
    if !any {
        let _ = writeln!(md, "| (none) | | | | |");
    }
    for kind in KINDS {
        let prefix = format!("{kind}:");
        let rows: Vec<&(&Ranked, Placement)> = placed
            .iter()
            .filter(|(f, _)| f.name.starts_with(&prefix))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(md, "\n## `{kind}`\n");
        let _ = writeln!(
            md,
            "| Feature | Applications | Statements | Proposed | Why |\n|---|---|---:|---|---|"
        );
        for (f, p) in rows {
            let _ = writeln!(
                md,
                "| `{}` | {} ({}) | {} | {} | {} |",
                f.name[prefix.len()..].replace('|', "\\|"),
                f.apps.len(),
                f.apps.join(", "),
                f.frequency,
                p.label(),
                f.reason
            );
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{SCHEMA, Uses};

    fn inv(app: &str, variant: Option<&str>, features: &[(&str, u64)]) -> Inventory {
        Inventory {
            schema: SCHEMA.to_owned(),
            app: app.to_owned(),
            variant: variant.map(str::to_owned),
            statements: Counts::default(),
            skipped: Default::default(),
            features: features
                .iter()
                .map(|(f, n)| {
                    (
                        (*f).to_owned(),
                        Uses {
                            count: *n,
                            sources: vec!["q:1".to_owned()],
                        },
                    )
                })
                .collect(),
            unparsed: Uses::default(),
            samples: BTreeMap::new(),
        }
    }

    #[test]
    fn three_apps_or_a_gate_app_put_a_feature_in_the_profile() {
        let gate = vec!["langfuse".to_owned(), "openpanel".to_owned()];
        let ranking = merge(
            &[
                inv(
                    "langfuse",
                    Some("v3"),
                    &[("function:a", 1), ("function:g", 5)],
                ),
                inv(
                    "langfuse",
                    Some("v4"),
                    &[("function:a", 1), ("function:v4", 1)],
                ),
                inv("umami", None, &[("function:a", 2), ("function:b", 9)]),
                inv("signoz", None, &[("function:a", 3), ("function:b", 1)]),
                inv("plausible", None, &[("function:b", 1)]),
                inv("posthog", None, &[("function:c", 7)]),
            ],
            3,
            &gate,
        );
        let get = |n: &str| ranking.features.iter().find(|f| f.name == n).unwrap();
        assert!(get("function:a").in_profile);
        assert_eq!(get("function:a").apps, ["langfuse", "signoz", "umami"]);
        assert_eq!(get("function:a").frequency, 7);
        assert!(get("function:b").in_profile);
        assert_eq!(get("function:g").reason, "gate:langfuse");
        assert!(get("function:v4").in_profile);
        assert!(!get("function:c").in_profile);
        // Variants of one application are one application.
        assert_eq!(ranking.apps.len(), 5);
        // Ranked by applications, then frequency.
        let order: Vec<&str> = ranking.features.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            order,
            [
                "function:b",
                "function:a",
                "function:c",
                "function:g",
                "function:v4"
            ]
        );
        let classes = Classes::parse(
            "[[rule]]\nclass = \"refused\"\nkind = \"function\"\nnames = [\"c\"]\nreason = \"Tier 2\"\n",
        )
        .unwrap();
        let profile = Profile::from_ranking(&ranking, &classes);
        // In the profile by the rule: Tier 0; below it: Tier 1, unless refused.
        let tier = |n: &str| profile.feature.iter().find(|f| f.name == n).map(|f| f.tier);
        assert_eq!(tier("function:a"), Some(0));
        assert_eq!(tier("function:g"), Some(0));
        assert_eq!(tier("function:c"), None);
        let md = render_markdown(&ranking, &classes, Some("## Findings\n\nx"));
        assert!(
            md.contains("| `a` | 3 (langfuse, signoz, umami) | 7 | Tier 0 | apps>=3 |"),
            "{md}"
        );
        assert!(md.contains("## Findings"));
        assert!(md.contains("| `syntax:FINAL` | 0 | not mined | Tier 0 |"));
    }
}
