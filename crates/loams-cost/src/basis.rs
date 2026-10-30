//! Basis labels and flags (design §84 §10.1, D2367).
//!
//! Every input and every output says how it was obtained. An output takes
//! the **weakest** basis of its inputs, in the order
//! `measured` = `billed` > `declared` > `estimated`.

use serde::{Deserialize, Serialize};

/// How a number was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// A driver quantity read from telemetry measured outside tenant code.
    Measured,
    /// An amount from an imported bill line in the reporting currency.
    Billed,
    /// An amount or rule the operator stated (assets, contract prices, FX,
    /// declared splits and capacities).
    Declared,
    /// A default, a list price, an unmeasured split, a spread, or a
    /// benchmark example.
    Estimated,
}

impl Basis {
    /// Strength rank: lower is stronger.
    pub const fn rank(self) -> u8 {
        match self {
            Basis::Measured | Basis::Billed => 0,
            Basis::Declared => 1,
            Basis::Estimated => 2,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Basis::Measured => "measured",
            Basis::Billed => "billed",
            Basis::Declared => "declared",
            Basis::Estimated => "estimated",
        }
    }

    /// The weakest of `bases`. Among the strongest rank, `measured` wins
    /// over `billed` when both are present (a measured share of a billed
    /// amount is a measured line). `None` for no bases.
    pub fn weakest(bases: impl IntoIterator<Item = Basis>) -> Option<Basis> {
        let mut out: Option<Basis> = None;
        for b in bases {
            out = Some(match out {
                None => b,
                Some(o) if b.rank() > o.rank() => b,
                Some(o) if b.rank() == o.rank() && b == Basis::Measured => b,
                Some(o) => o,
            });
        }
        out
    }

    /// The weaker of two.
    pub fn and(self, other: Basis) -> Basis {
        Basis::weakest([self, other]).unwrap_or(self)
    }
}

impl std::fmt::Display for Basis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Facts about a line that a reader must see (design §84 §9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The pool has no declared capacity: the line is fully loaded and idle
    /// cannot be separated.
    IdleUnknown,
    /// Measured usage exceeded the declared capacity; the split falls back
    /// to usage and is an estimate.
    OverCapacity,
    /// A node's cost went wholly to `cpu` because it declared no split.
    DefaultSplitCpuOnly,
    /// The engine has no ops counter yet, so it has no $ per 1M ops.
    OpsUnavailable,
    /// Below the minimum sample (D2369): the unit cost is withheld.
    InsufficientSample,
    /// Idle or shared cost spread over consumers (D2364).
    Spread,
    /// The pool-day has usage but no cost input at all.
    NoCostInput,
    /// A default input value was used (named in the trace).
    DefaultInput,
}

impl Flag {
    pub const fn as_str(self) -> &'static str {
        match self {
            Flag::IdleUnknown => "idle_unknown",
            Flag::OverCapacity => "over_capacity",
            Flag::DefaultSplitCpuOnly => "default_split_cpu_only",
            Flag::OpsUnavailable => "ops_unavailable",
            Flag::InsufficientSample => "insufficient_sample",
            Flag::Spread => "spread",
            Flag::NoCostInput => "no_cost_input",
            Flag::DefaultInput => "default_input",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_takes_weakest_input() {
        use Basis::*;
        assert_eq!(Basis::weakest([Billed, Measured]), Some(Measured));
        assert_eq!(Basis::weakest([Measured, Billed]), Some(Measured));
        assert_eq!(Basis::weakest([Billed, Billed]), Some(Billed));
        assert_eq!(Basis::weakest([Measured, Declared]), Some(Declared));
        assert_eq!(
            Basis::weakest([Declared, Estimated, Billed]),
            Some(Estimated)
        );
        assert_eq!(Basis::weakest([]), None);
        assert_eq!(Billed.and(Estimated), Estimated);
    }

    #[test]
    fn labels_are_snake_case() {
        assert_eq!(
            serde_json::to_string(&Basis::Estimated).expect("json"),
            "\"estimated\""
        );
        assert_eq!(
            serde_json::to_string(&Flag::DefaultSplitCpuOnly).expect("json"),
            format!("\"{}\"", Flag::DefaultSplitCpuOnly.as_str())
        );
    }
}
