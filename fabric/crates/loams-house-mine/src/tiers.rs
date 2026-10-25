//! Tier proposals for the mined features, checked against design §75
//! §7.3–§7.5 (plan HE1 Task 5).
//!
//! The mining proposes: **Tier 0** for every feature §75 §7.1's rule puts in
//! the profile (3 or more applications, or a gate application); **Tier 1** for
//! a feature fewer applications use, unless §75 refuses or disables it;
//! **refused** (Tier 2) and **disabled** as §75 and D1558 say. §75's own
//! tiers come from `classes.toml` (functions, engines, table functions, the
//! matrix's rules) and from [`SECTION_75`] (syntax, types, formats,
//! combinators). Where the two disagree, the report lists it for the owner;
//! nothing is resolved silently.

use crate::matrix::{Class, Classes, Kind};

/// §75's placement of a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Placement {
    /// Tier 0 (§7.3).
    Tier0,
    /// Tier 1 (§7.4).
    Tier1,
    /// Tier 2, `48 NOT_IMPLEMENTED` (§7.5).
    Refused,
    /// Never built, `344 SUPPORT_IS_DISABLED` (D1558).
    Disabled,
}

impl Placement {
    /// The label the report prints.
    pub fn label(self) -> &'static str {
        match self {
            Self::Tier0 => "Tier 0",
            Self::Tier1 => "Tier 1",
            Self::Refused => "Tier 2 (refused)",
            Self::Disabled => "disabled",
        }
    }
}

/// §75's tiers for the kinds `classes.toml` does not cover: `(feature,
/// placement)`. Only what §75 §7.3–§7.5 names.
pub const SECTION_75: &[(&str, Placement)] = &[
    // §7.3 Tier 0: engines' clauses, mutations, interface, types, SQL.
    ("syntax:PARTITION BY", Placement::Tier0),
    ("syntax:table ORDER BY", Placement::Tier0),
    ("syntax:table PRIMARY KEY", Placement::Tier0),
    ("syntax:TTL", Placement::Tier0),
    ("syntax:FINAL", Placement::Tier0),
    ("syntax:ALTER DELETE", Placement::Tier0),
    ("syntax:ALTER UPDATE", Placement::Tier0),
    ("syntax:ON CLUSTER", Placement::Tier0),
    ("syntax:query parameter", Placement::Tier0),
    ("syntax:INNER JOIN", Placement::Tier0),
    ("syntax:LEFT JOIN", Placement::Tier0),
    ("syntax:WITH", Placement::Tier0),
    ("syntax:window function", Placement::Tier0),
    ("syntax:ARRAY JOIN", Placement::Tier0),
    ("syntax:LIMIT BY", Placement::Tier0),
    ("syntax:WITH TOTALS", Placement::Tier0),
    ("format:JSONEachRow", Placement::Tier0),
    ("format:JSON", Placement::Tier0),
    ("format:CSV", Placement::Tier0),
    ("format:TSV", Placement::Tier0),
    ("format:TabSeparated", Placement::Tier0),
    ("format:Parquet", Placement::Tier0),
    ("type:Int8", Placement::Tier0),
    ("type:Int16", Placement::Tier0),
    ("type:Int32", Placement::Tier0),
    ("type:Int64", Placement::Tier0),
    ("type:UInt8", Placement::Tier0),
    ("type:UInt16", Placement::Tier0),
    ("type:UInt32", Placement::Tier0),
    ("type:UInt64", Placement::Tier0),
    ("type:Float32", Placement::Tier0),
    ("type:Float64", Placement::Tier0),
    ("type:Decimal", Placement::Tier0),
    ("type:String", Placement::Tier0),
    ("type:DateTime64", Placement::Tier0),
    ("type:UUID", Placement::Tier0),
    ("type:Array", Placement::Tier0),
    ("type:Map", Placement::Tier0),
    ("type:Nullable", Placement::Tier0),
    ("type:LowCardinality", Placement::Tier0),
    ("type:Enum8", Placement::Tier0),
    ("type:Enum16", Placement::Tier0),
    ("combinator:If", Placement::Tier0),
    ("combinator:State", Placement::Tier0),
    ("combinator:Merge", Placement::Tier0),
    // §7.4 Tier 1.
    ("syntax:CREATE MATERIALIZED VIEW", Placement::Tier1),
    ("syntax:MATERIALIZED VIEW TO", Placement::Tier1),
    ("syntax:skip index bloom_filter", Placement::Tier1),
    ("syntax:skip index minmax", Placement::Tier1),
    ("syntax:skip index ngrambf_v1", Placement::Tier1),
    ("syntax:skip index tokenbf_v1", Placement::Tier1),
    ("syntax:skip index text", Placement::Tier1),
    ("syntax:DELETE", Placement::Tier1),
    ("type:JSON", Placement::Tier1),
    ("type:Variant", Placement::Tier1),
    ("type:Dynamic", Placement::Tier1),
    // §7.5 Tier 2.
    ("type:Object", Placement::Refused),
    ("syntax:CREATE WINDOW VIEW", Placement::Refused),
    ("syntax:CREATE LIVE VIEW", Placement::Refused),
    ("syntax:CREATE DICTIONARY", Placement::Refused),
];

fn kind_of(feature: &str) -> Option<(Kind, &str)> {
    let (kind, name) = feature.split_once(':')?;
    let kind = match kind {
        "function" => Kind::Function,
        "engine" => Kind::Engine,
        "table_function" => Kind::TableFunction,
        _ => return None,
    };
    Some((kind, name))
}

/// §75's placement of `feature` (`kind:name`), and the rule's reason, if §75
/// names it.
pub fn section_75(classes: &Classes, feature: &str) -> Option<(Placement, String)> {
    if let Some((kind, name)) = kind_of(feature) {
        let matches = classes.matches(kind, name);
        // The matrix's precedence: disabled first.
        let pick = matches
            .iter()
            .find(|(c, _)| *c == Class::Disabled)
            .or_else(|| matches.iter().find(|(c, _)| *c == Class::Profile))
            .or_else(|| matches.first())?;
        let placement = match pick.0 {
            Class::Disabled => Placement::Disabled,
            Class::Refused => Placement::Refused,
            Class::Profile if pick.1.contains("Tier 1") => Placement::Tier1,
            Class::Profile => Placement::Tier0,
            Class::Unclassified => return None,
        };
        return Some((placement, pick.1.to_owned()));
    }
    SECTION_75
        .iter()
        .find(|(f, _)| *f == feature)
        .map(|(_, p)| (*p, "§75 §7.3–§7.5".to_owned()))
}

/// The mining's proposal for a feature: `in_profile` is §75 §7.1's rule.
pub fn propose(classes: &Classes, feature: &str, in_profile: bool) -> Placement {
    match section_75(classes, feature) {
        Some((Placement::Disabled, _)) => Placement::Disabled,
        _ if in_profile => Placement::Tier0,
        Some((Placement::Refused, _)) => Placement::Refused,
        _ => Placement::Tier1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes() -> Classes {
        Classes::parse(
            r#"
[[rule]]
class = "profile"
kind = "engine"
names = ["MergeTree"]
reason = "Tier 0 (§75 §7.3)"

[[rule]]
class = "profile"
kind = "engine"
names = ["AggregatingMergeTree"]
reason = "Tier 1 (§75 §7.4)"

[[rule]]
class = "refused"
kind = "engine"
names = ["TinyLog"]
reason = "Tier 2: legacy engines (§75 §7.5)"

[[rule]]
class = "disabled"
kind = "table_function"
names = ["url"]
reason = "reaches outside the namespace"
"#,
        )
        .unwrap()
    }

    #[test]
    fn proposals_follow_the_rule_and_never_lift_disabled() {
        let c = classes();
        assert_eq!(propose(&c, "engine:MergeTree", true), Placement::Tier0);
        assert_eq!(
            propose(&c, "engine:AggregatingMergeTree", true),
            Placement::Tier0
        );
        assert_eq!(
            propose(&c, "engine:AggregatingMergeTree", false),
            Placement::Tier1
        );
        assert_eq!(propose(&c, "engine:TinyLog", false), Placement::Refused);
        assert_eq!(propose(&c, "engine:TinyLog", true), Placement::Tier0);
        assert_eq!(propose(&c, "table_function:url", true), Placement::Disabled);
        assert_eq!(
            propose(&c, "function:somethingNew", false),
            Placement::Tier1
        );
        assert_eq!(
            section_75(&c, "syntax:FINAL").map(|p| p.0),
            Some(Placement::Tier0)
        );
        assert_eq!(section_75(&c, "syntax:unknown"), None);
    }
}
