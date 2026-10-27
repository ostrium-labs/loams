//! Feature names and the rules that normalise them.
//!
//! A feature is `<kind>:<name>`. The kinds are the plan's six, `function`,
//! `engine`, `type`, `format`, `setting` and `syntax`, plus two the profile
//! needs to tell apart (HE0 Task 2 ruling): `table_function` (`numbers`,
//! `s3`, `url`: the security rule of §75 §7.5 is about these) and
//! `combinator` (`If`, `State`, `Merge`: Tier 0 names the combinators,
//! §75 §7.3).

use std::collections::BTreeSet;

/// The feature kinds, in report order.
pub const KINDS: [&str; 8] = [
    "function",
    "combinator",
    "table_function",
    "engine",
    "type",
    "format",
    "setting",
    "syntax",
];

/// The features one statement uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features(pub BTreeSet<String>);

impl Features {
    /// Adds `kind:name`.
    pub fn add(&mut self, kind: &str, name: &str) {
        if !name.is_empty() {
            self.0.insert(format!("{kind}:{name}"));
        }
    }

    /// Adds a scalar or aggregate function by the name the statement used,
    /// and for an aggregate with combinators, the base function and each
    /// combinator (`countIf` adds `function:countIf`, `function:count` and
    /// `combinator:If`).
    pub fn function(&mut self, name: &str) {
        let name = function_name(name);
        self.add("function", &name);
        if let Some((base, combinators)) = split_combinators(&name) {
            self.add("function", base);
            for c in combinators {
                self.add("combinator", c);
            }
        }
    }

    /// Adds a data type and every type family nested in it
    /// (`Map(LowCardinality(String), UInt64)` adds `Map`, `LowCardinality`,
    /// `String` and `UInt64`).
    pub fn data_type(&mut self, text: &str) {
        for family in type_families(text) {
            self.add("type", &family);
        }
    }

    /// Adds every feature of `other`.
    pub fn extend(&mut self, other: Features) {
        self.0.extend(other.0);
    }

    /// Whether `feature` is present.
    pub fn contains(&self, feature: &str) -> bool {
        self.0.contains(feature)
    }
}

/// A function name as the profile records it: as written, except that an
/// all-capitals name (`COUNT`, `IF`) is lowered, since ClickHouse resolves
/// those case-insensitively and applications write both. `CAST` keeps its
/// canonical capitals.
pub fn function_name(name: &str) -> String {
    let has_letter = name.chars().any(|c| c.is_ascii_alphabetic());
    let all_caps = has_letter && !name.chars().any(|c| c.is_ascii_lowercase());
    if name.eq_ignore_ascii_case("cast") {
        "CAST".to_owned()
    } else if all_caps {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    }
}

/// Aggregate function suffixes, longest first so `MergeState` wins over
/// `State`.
const COMBINATORS: [&str; 14] = [
    "SimpleState",
    "MergeState",
    "OrDefault",
    "Resample",
    "Distinct",
    "ForEach",
    "ArgMin",
    "ArgMax",
    "OrNull",
    "Array",
    "State",
    "Merge",
    "Map",
    "If",
];

/// Aggregate functions that take combinators: the common ones and every
/// aggregate §75 §7.3–§7.4 names.
const AGGREGATES: &[&str] = &[
    "any",
    "anyHeavy",
    "anyLast",
    "any_value",
    "argMax",
    "argMin",
    "avg",
    "avgWeighted",
    "boundingRatio",
    "corr",
    "count",
    "covarPop",
    "covarSamp",
    "deltaSum",
    "entropy",
    "first_value",
    "groupArray",
    "groupArrayInsertAt",
    "groupArrayMovingAvg",
    "groupArrayMovingSum",
    "groupArraySample",
    "groupBitAnd",
    "groupBitOr",
    "groupBitXor",
    "groupBitmap",
    "groupConcat",
    "groupUniqArray",
    "histogram",
    "last_value",
    "max",
    "maxMap",
    "median",
    "medianExact",
    "min",
    "minMap",
    "quantile",
    "quantileDeterministic",
    "quantileExact",
    "quantileExactWeighted",
    "quantileTDigest",
    "quantileTiming",
    "quantiles",
    "quantilesExact",
    "quantilesTDigest",
    "quantilesTiming",
    "retention",
    "sequenceCount",
    "sequenceMatch",
    "simpleLinearRegression",
    "stddevPop",
    "stddevSamp",
    "sum",
    "sumKahan",
    "sumMap",
    "sumWithOverflow",
    "topK",
    "topKWeighted",
    "uniq",
    "uniqCombined",
    "uniqCombined64",
    "uniqExact",
    "uniqHLL12",
    "uniqTheta",
    "uniqUpTo",
    "varPop",
    "varSamp",
    "windowFunnel",
];

/// Whether `name` is an aggregate [`AGGREGATES`] lists.
pub fn is_aggregate(name: &str) -> bool {
    AGGREGATES.contains(&name)
}

/// Splits `quantilesIf` into `quantiles` and `[If]`; `None` when the name is
/// not an aggregate with at least one combinator.
pub fn split_combinators(name: &str) -> Option<(&str, Vec<&'static str>)> {
    if is_aggregate(name) {
        return None;
    }
    let mut rest = name;
    let mut found = Vec::new();
    loop {
        let suffix = COMBINATORS
            .iter()
            .find(|c| rest.len() > c.len() && rest.ends_with(**c))?;
        rest = &rest[..rest.len() - suffix.len()];
        found.push(*suffix);
        if is_aggregate(rest) {
            found.reverse();
            return Some((rest, found));
        }
    }
}

/// The type families in a type's text: every identifier that is not a field
/// name (an identifier followed by another identifier, as in
/// `Tuple(a String)`) and not inside a quoted string (`Enum8('a' = 1)`).
pub fn type_families(text: &str) -> Vec<String> {
    let mut words: Vec<(String, bool)> = Vec::new(); // (word, followed by another word)
    let mut chars = text.chars().peekable();
    let mut last_was_word = false;
    while let Some(&c) = chars.peek() {
        if c == '\'' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '\'' => break,
                    _ => {}
                }
            }
            last_was_word = false;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let mut w = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    w.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if last_was_word && let Some(prev) = words.last_mut() {
                prev.1 = true;
            }
            // `AggregateFunction(uniq, String)`: the first argument is the
            // aggregate function, not a type.
            let after_aggregate = words
                .last()
                .is_some_and(|(p, _)| p == "AggregateFunction" || p == "SimpleAggregateFunction")
                && !last_was_word;
            words.push((w, after_aggregate));
            last_was_word = true;
        } else if c.is_whitespace() {
            chars.next();
        } else if c == '`' || c == '"' {
            // A quoted field name.
            let q = c;
            chars.next();
            for c in chars.by_ref() {
                if c == q {
                    break;
                }
            }
            words.push((String::new(), true));
            last_was_word = true;
        } else {
            chars.next();
            last_was_word = false;
        }
    }
    let mut out: Vec<String> = words
        .into_iter()
        .filter(|(w, field)| !field && !w.is_empty())
        .map(|(w, _)| canonical_type(&w))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// ClickHouse's type families, for spelling them canonically.
const TYPES: &[&str] = &[
    "AggregateFunction",
    "Array",
    "BFloat16",
    "Bool",
    "Date",
    "Date32",
    "DateTime",
    "DateTime64",
    "Decimal",
    "Decimal128",
    "Decimal256",
    "Decimal32",
    "Decimal64",
    "Dynamic",
    "Enum",
    "Enum16",
    "Enum8",
    "FixedString",
    "Float32",
    "Float64",
    "IPv4",
    "IPv6",
    "Int128",
    "Int16",
    "Int256",
    "Int32",
    "Int64",
    "Int8",
    "IntervalDay",
    "IntervalHour",
    "IntervalMinute",
    "IntervalMonth",
    "IntervalSecond",
    "IntervalWeek",
    "IntervalYear",
    "JSON",
    "LineString",
    "LowCardinality",
    "Map",
    "MultiLineString",
    "MultiPolygon",
    "Nested",
    "Nothing",
    "Nullable",
    "Object",
    "Point",
    "Polygon",
    "Ring",
    "SimpleAggregateFunction",
    "String",
    "Time",
    "Time64",
    "Tuple",
    "UInt128",
    "UInt16",
    "UInt256",
    "UInt32",
    "UInt64",
    "UInt8",
    "UUID",
    "Variant",
];

/// sqlparser prints some ClickHouse types in SQL-standard capitals
/// (`FLOAT64`); spell a known family the way ClickHouse does.
fn canonical_type(word: &str) -> String {
    TYPES
        .iter()
        .find(|t| t.eq_ignore_ascii_case(word))
        .map_or_else(|| word.to_owned(), |t| (*t).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combinators_split_off_known_aggregates_only() {
        assert_eq!(split_combinators("countIf"), Some(("count", vec!["If"])));
        assert_eq!(
            split_combinators("uniqMergeState"),
            Some(("uniq", vec!["MergeState"]))
        );
        assert_eq!(
            split_combinators("sumArrayIf"),
            Some(("sum", vec!["Array", "If"]))
        );
        assert_eq!(split_combinators("minMap"), None);
        assert_eq!(split_combinators("multiIf"), None);
        assert_eq!(split_combinators("toStartOfDay"), None);
    }

    #[test]
    fn function_names_lower_only_all_capitals() {
        assert_eq!(function_name("COUNT"), "count");
        assert_eq!(function_name("toDate"), "toDate");
        assert_eq!(function_name("cast"), "CAST");
    }

    #[test]
    fn type_families_skip_field_names_and_strings() {
        assert_eq!(
            type_families("Map(LowCardinality(String), Nullable(UInt64))"),
            ["LowCardinality", "Map", "Nullable", "String", "UInt64"]
        );
        assert_eq!(
            type_families("AggregateFunction(uniq, LowCardinality(String))"),
            ["AggregateFunction", "LowCardinality", "String"]
        );
        assert_eq!(type_families("FLOAT64"), ["Float64"]);
        assert_eq!(type_families("Enum8('a b' = 1, 'c' = 2)"), ["Enum8"]);
        assert_eq!(
            type_families("Tuple(a String, `b c` Array(DateTime64(3, 'UTC')))"),
            ["Array", "DateTime64", "String", "Tuple"]
        );
    }
}
