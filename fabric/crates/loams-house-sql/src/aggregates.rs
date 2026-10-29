//! Aggregate function names: combinators and parameters (plan HE1 Task 28).
//!
//! ClickHouse composes aggregate functions by suffixes: `sumIf` is `sum` under
//! the `-If` combinator, `groupUniqArrayArrayIf` is `-If` over `-Array` over
//! `groupUniqArray`, `sumMap` is `-Map` over `sum`. The suffixes are read from
//! the end of the name, so the last one is the outermost (ClickHouse's
//! `AggregateFunctionFactory::getImpl`). A name ClickHouse knows as a function
//! itself (`groupArray`, `sumMappedArrays`, `argMax`) is never split.
//!
//! A parametric call (`topK(10)(x)`, `windowFunnel(3600, 'strict_order')(t,
//! c1, c2)`) has its parameters validated as literals by the guard and carried
//! in the name the function is planned under ([`planned_name`]): the
//! parameters are part of the function's identity, as in ClickHouse, so its
//! result type and its checks are known when the plan is made, before any row
//! is read.

use std::fmt;

use crate::functions;

/// An aggregate function combinator (`system.aggregate_function_combinators`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Combinator {
    /// `-If`: the last argument is a condition; rows where it is false (or
    /// NULL) are not aggregated.
    If,
    /// `-Distinct`: each distinct tuple of arguments is aggregated once.
    Distinct,
    /// `-Array`: the arguments are arrays, aggregated element by element.
    Array,
    /// `-ForEach`: the arguments are arrays; one aggregate per index.
    ForEach,
    /// `-OrNull`: `NULL` when nothing was aggregated.
    OrNull,
    /// `-OrDefault`: the result type's default when nothing was aggregated.
    OrDefault,
    /// `-SimpleState`: the result as a `SimpleAggregateFunction` value.
    SimpleState,
    /// `-Map`: one aggregate per map key (or ClickHouse's mapped-arrays form).
    Map,
    /// `-State`: the aggregate state (plan HE1 Task 29).
    State,
    /// `-Merge`: merges aggregate states (plan HE1 Task 29).
    Merge,
    /// `-Resample`.
    Resample,
    /// `-ArgMax`.
    ArgMax,
    /// `-ArgMin`.
    ArgMin,
    /// `-Tuple`.
    Tuple,
    /// `-Null`.
    Null,
}

impl Combinator {
    /// The suffix as ClickHouse writes it.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::If => "If",
            Self::Distinct => "Distinct",
            Self::Array => "Array",
            Self::ForEach => "ForEach",
            Self::OrNull => "OrNull",
            Self::OrDefault => "OrDefault",
            Self::SimpleState => "SimpleState",
            Self::Map => "Map",
            Self::State => "State",
            Self::Merge => "Merge",
            Self::Resample => "Resample",
            Self::ArgMax => "ArgMax",
            Self::ArgMin => "ArgMin",
            Self::Tuple => "Tuple",
            Self::Null => "Null",
        }
    }

    fn from_suffix(s: &str) -> Option<Self> {
        Some(match s {
            "If" => Self::If,
            "Distinct" => Self::Distinct,
            "Array" => Self::Array,
            "ForEach" => Self::ForEach,
            "OrNull" => Self::OrNull,
            "OrDefault" => Self::OrDefault,
            "SimpleState" => Self::SimpleState,
            "Map" => Self::Map,
            "State" => Self::State,
            "Merge" => Self::Merge,
            "Resample" => Self::Resample,
            "ArgMax" => Self::ArgMax,
            "ArgMin" => Self::ArgMin,
            "Tuple" => Self::Tuple,
            "Null" => Self::Null,
            _ => return None,
        })
    }
}

/// A literal parameter of a parametric aggregate.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// A non-negative integer (ClickHouse's `UInt64` field).
    UInt(u64),
    /// A negative integer.
    Int(i64),
    /// A floating-point number.
    Float(f64),
    /// A string.
    String(String),
}

impl Param {
    /// The parameter as an unsigned integer, as ClickHouse's
    /// `FieldVisitorConvertToNumber<UInt64>` reads it.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt(v) => Some(*v),
            Self::Int(v) => u64::try_from(*v).ok(),
            Self::Float(v)
                if v.fract() == 0.0 && *v >= 0.0 && *v < 18_446_744_073_709_551_616.0 =>
            {
                Some(*v as u64)
            }
            _ => None,
        }
    }

    /// The parameter as a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

impl fmt::Display for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UInt(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v:?}"),
            Self::String(s) => write!(f, "'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        }
    }
}

/// An aggregate function call's name, read.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateName {
    /// The function under the combinators (`sum` in `sumIf`), canonical.
    pub base: String,
    /// The combinators, outermost first (`sumArrayIf`: `[If, Array]`).
    pub combinators: Vec<Combinator>,
    /// The parameters (`topK(10)`: `[10]`).
    pub params: Vec<Param>,
}

impl AggregateName {
    /// The canonical name with its combinators (`sumArrayIf`).
    pub fn function(&self) -> String {
        let mut s = self.base.clone();
        for c in self.combinators.iter().rev() {
            s.push_str(c.suffix());
        }
        s
    }
}

impl fmt::Display for AggregateName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.function())?;
        if !self.params.is_empty() {
            f.write_str("(")?;
            for (i, p) in self.params.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{p}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

/// Splits a canonical aggregate name into its base function and combinators
/// (outermost first), as ClickHouse's factory does: a name that is itself a
/// function is not split; otherwise the longest combinator suffix whose rest
/// is an aggregate is taken off, repeatedly.
///
/// The analyzer's rewrite of `…IfOrNull` to `…OrNullIf` is applied (26.9
/// measured: `sumIfOrNull(x, 0)` is `NULL`, `avgIfOrDefault(x, 0)` is `nan`).
pub fn split(canonical: &str) -> Option<(String, Vec<Combinator>)> {
    let mut combinators = Vec::new();
    let mut name = canonical.to_string();
    loop {
        if let Some(k) = functions::resolve_exact(&name) {
            if !k.aggregate {
                return None;
            }
            if combinators.len() >= 2
                && combinators[0] == Combinator::OrNull
                && combinators[1] == Combinator::If
            {
                combinators.swap(0, 1);
            }
            return Some((k.canonical, combinators));
        }
        let mut found = false;
        for c in functions::COMBINATORS {
            if let Some(rest) = name.strip_suffix(c)
                && !rest.is_empty()
                && functions::resolve(rest).is_some_and(|k| k.aggregate)
            {
                combinators.push(Combinator::from_suffix(c)?);
                name = rest.to_string();
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
}

/// The name a call of `canonical` with `params` is planned under: the
/// canonical name alone without parameters, else `name(p1, p2)` in
/// ClickHouse's literal text, which [`parse`] reads back.
pub fn planned_name(canonical: &str, params: &[Param]) -> String {
    let base = functions::internal_name(canonical);
    if params.is_empty() {
        return base;
    }
    let mut s = base;
    s.push('(');
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&p.to_string());
    }
    s.push(')');
    s
}

/// Reads a planned aggregate name ([`planned_name`]) back.
pub fn parse(planned: &str) -> Option<AggregateName> {
    let (name, params) = match planned.find('(') {
        Some(i) if planned.ends_with(')') => (
            &planned[..i],
            parse_params(&planned[i + 1..planned.len() - 1])?,
        ),
        Some(_) => return None,
        None => (planned, Vec::new()),
    };
    let (base, combinators) = split(name)?;
    Some(AggregateName {
        base,
        combinators,
        params,
    })
}

fn parse_params(text: &str) -> Option<Vec<Param>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None => break,
            Some('\'') => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next()? {
                        '\\' => s.push(chars.next()?),
                        '\'' => break,
                        c => s.push(c),
                    }
                }
                out.push(Param::String(s));
            }
            Some(_) => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ',' {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                out.push(number(s.trim())?);
            }
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.next() {
            None => break,
            Some(',') => {}
            Some(_) => return None,
        }
    }
    Some(out)
}

/// A numeric literal's text as a parameter (`10`, `-1`, `0.95`, `1e3`).
pub fn number(text: &str) -> Option<Param> {
    if let Ok(v) = text.parse::<u64>() {
        return Some(Param::UInt(v));
    }
    if let Ok(v) = text.parse::<i64>() {
        return Some(Param::Int(v));
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))
        && let Ok(v) = u64::from_str_radix(hex, 16)
    {
        return Some(Param::UInt(v));
    }
    text.parse::<f64>().ok().map(Param::Float)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_outermost_first() {
        use Combinator::*;
        let s = |n: &str| split(n).unwrap();
        assert_eq!(s("sum"), ("sum".into(), vec![]));
        assert_eq!(s("sumIf"), ("sum".into(), vec![If]));
        assert_eq!(
            s("groupUniqArrayArrayIf"),
            ("groupUniqArray".into(), vec![If, Array])
        );
        assert_eq!(s("groupArray"), ("groupArray".into(), vec![]));
        assert_eq!(s("sumMap"), ("sum".into(), vec![Map]));
        assert_eq!(s("uniqMergeState"), ("uniq".into(), vec![State, Merge]));
        assert_eq!(s("countDistinct"), ("count".into(), vec![Distinct]));
        assert_eq!(s("sumIfOrNull"), ("sum".into(), vec![If, OrNull]));
        assert_eq!(s("avgIfOrDefault"), ("avg".into(), vec![OrDefault, If]));
        assert!(split("plus").is_none());
    }

    #[test]
    fn planned_names_round_trip() {
        let params = vec![
            Param::UInt(3600),
            Param::String("strict_order".into()),
            Param::Float(0.5),
            Param::Int(-2),
            Param::String("it's, odd".into()),
        ];
        let n = planned_name("windowFunnel", &params);
        assert_eq!(
            n,
            "windowFunnel(3600, 'strict_order', 0.5, -2, 'it\\'s, odd')"
        );
        let back = parse(&n).unwrap();
        assert_eq!(back.base, "windowFunnel");
        assert_eq!(back.params, params);
        assert_eq!(
            parse("topKIf(2)").unwrap().combinators,
            vec![Combinator::If]
        );
        assert_eq!(parse("sum").unwrap().params, vec![]);
    }
}
