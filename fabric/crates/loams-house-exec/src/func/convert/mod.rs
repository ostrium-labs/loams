//! Conversions (plan HE1 Task 33): `CAST`, `_CAST`, `x::T`, `accurateCast*`,
//! and the `toX`, `toXOrNull`, `toXOrZero` and `toXOrDefault` families, with
//! ClickHouse's cast semantics ([`cast`]).
//!
//! The guard rewrites every SQL cast (`CAST(x AS T)`, `x::T`,
//! `CAST(x, 'T')`) into `_CAST(x, 'T')`, so DataFusion's own casts, which
//! differ (`CAST('abc' AS TINYINT UNSIGNED)` is `NULL` there, `6` here), never
//! see a user's cast (R33.2). The plain `toDate`, `toDate32`, `toDateTime`
//! and `toDateTime64` belong to Task 30; their `OrNull`/`OrZero`/`OrDefault`
//! forms are here.

pub mod cast;
pub mod text;
pub mod value;

use std::sync::Arc;

use loams_house_sql::Registry;
use loams_house_types::arrow::nullable;
use loams_house_types::{ChError, ChType, parse};

use crate::func::typed::{
    ChFn, Nulls, Sig, arity, const_int, const_str, fits_low_cardinality, illegal_type, kernel,
    not_constant, typer,
};
use cast::{Mode, cast_column, cast_column_skipping_nulls};
use value::{default_value, read, read_all};

/// How a `toX` function treats what it cannot convert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `toX`: an error.
    Plain,
    /// `toXOrZero`: the default value.
    OrZero,
    /// `toXOrNull`: `NULL`.
    OrNull,
    /// `toXOrDefault`: the second argument, or the default value.
    OrDefault,
}

impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Kind::Plain => "",
            Kind::OrZero => "OrZero",
            Kind::OrNull => "OrNull",
            Kind::OrDefault => "OrDefault",
        }
    }
}

/// The target type of a conversion, from the call's type parameters
/// (`toDecimal64(x, S)`, `toDateTime64OrNull(x, P, 'tz')`).
type Target = Arc<dyn Fn(&Sig<'_>) -> Result<ChType, ChError> + Send + Sync>;

/// The ClickHouse type named by constant argument `i`.
pub fn type_argument(sig: &Sig<'_>, i: usize) -> Result<ChType, ChError> {
    let text = const_str(sig.consts.get(i).and_then(Option::as_ref)).ok_or_else(|| {
        ChError::illegal_type_of_argument(format!(
            "Second argument to {} must be a constant string describing type",
            sig.name
        ))
    })?;
    parse(&text)
}

fn scale_argument(sig: &Sig<'_>, i: usize, max: u8) -> Result<u8, ChError> {
    let v = const_int(sig.consts.get(i).and_then(Option::as_ref))
        .ok_or_else(|| not_constant(sig.name, i))?;
    if !(0..=i128::from(max)).contains(&v) {
        return Err(ChError::argument_out_of_bound(format!(
            "Argument {} of function {} is out of bounds: {v} (must be 0..={max})",
            i + 1,
            sig.name
        )));
    }
    Ok(v as u8)
}

fn tz_argument(sig: &Sig<'_>, i: usize) -> Result<Option<String>, ChError> {
    if sig.types.len() <= i {
        return Ok(None);
    }
    const_str(sig.consts.get(i).and_then(Option::as_ref))
        .map(Some)
        .ok_or_else(|| not_constant(sig.name, i))
}

/// A `toX…` function: `x`, then `params` type parameters (`min..=max`), then
/// the default for `OrDefault`.
fn conversion(name: &str, kind: Kind, params: (usize, usize), target: Target) -> ChFn {
    let (pmin, pmax) = params;
    let with_default = 1 + pmax + usize::from(kind == Kind::OrDefault);
    let check = move |s: &Sig<'_>| -> Result<ChType, ChError> {
        arity(s, 1 + pmin, with_default)?;
        if matches!(kind, Kind::OrZero | Kind::OrNull)
            && !matches!(s.types[0].base(), ChType::String | ChType::FixedString(_))
        {
            return Err(ChError::illegal_type_of_argument(format!(
                "Illegal type {} of first argument of function {}. Conversion functions with postfix 'OrZero' or 'OrNull' should take String argument",
                s.types[0], s.name
            )));
        }
        let t = target(s)?;
        if kind == Kind::OrDefault && s.types.len() == with_default {
            let d = &s.types[with_default - 1];
            if d.base() != t.base() {
                return Err(illegal_type(s.name, with_default - 1, d));
            }
        }
        Ok(t)
    };
    let nulls = if kind == Kind::OrNull {
        Nulls::Own
    } else {
        Nulls::Default
    };
    ChFn::new(
        name,
        nulls,
        typer(move |s| {
            let t = check(s)?;
            Ok(if kind == Kind::OrNull { nullable(t) } else { t })
        }),
        kernel(move |c| {
            let mode = match kind {
                Kind::Plain => Mode::Throw,
                Kind::OrZero => Mode::OrZero,
                Kind::OrNull => Mode::OrNull,
                Kind::OrDefault if c.args.len() == with_default => {
                    let i = with_default - 1;
                    Mode::OrDefault(read_all(c.args[i].as_ref(), &c.types[i])?)
                }
                Kind::OrDefault => Mode::OrZero,
            };
            cast_column_skipping_nulls(c.args[0].as_ref(), &c.types[0], c.ret, &mode)
        }),
    )
}

fn fixed(t: ChType) -> Target {
    Arc::new(move |_: &Sig<'_>| -> Result<ChType, ChError> { Ok(t.clone()) })
}

/// The number types and their `toX` names.
const NUMBERS: &[(&str, ChType)] = &[
    ("UInt8", ChType::UInt8),
    ("UInt16", ChType::UInt16),
    ("UInt32", ChType::UInt32),
    ("UInt64", ChType::UInt64),
    ("Int8", ChType::Int8),
    ("Int16", ChType::Int16),
    ("Int32", ChType::Int32),
    ("Int64", ChType::Int64),
    ("Float32", ChType::Float32),
    ("Float64", ChType::Float64),
];

const KINDS: [Kind; 4] = [Kind::Plain, Kind::OrZero, Kind::OrNull, Kind::OrDefault];

/// Every function this module registers, by name (for the registration
/// test).
pub fn names() -> Vec<String> {
    let mut r = Registry::default();
    register(&mut r);
    let mut names: Vec<String> = r.scalars.into_keys().collect();
    names.sort();
    names
}

/// Registers the conversion functions.
pub fn register(r: &mut Registry) {
    // CAST and its forms.
    for (name, mode) in [
        ("_CAST", Mode::Throw),
        ("CAST", Mode::Throw),
        ("accurateCast", Mode::Accurate),
        ("accurateCastOrNull", Mode::AccurateOrNull),
    ] {
        let or_null = mode == Mode::AccurateOrNull;
        ChFn::new(
            name,
            Nulls::Own,
            typer(move |s| {
                arity(s, 2, 2)?;
                let t = type_argument(s, 1)?;
                Ok(if or_null { nullable(t) } else { t })
            }),
            kernel(move |c| cast_column(c.args[0].as_ref(), &c.types[0], c.ret, &mode)),
        )
        .plain()
        .register(r);
    }
    ChFn::new(
        "accurateCastOrDefault",
        Nulls::Own,
        typer(|s| {
            arity(s, 2, 3)?;
            let t = type_argument(s, 1)?;
            if let Some(d) = s.types.get(2)
                && d.base() != t.base()
            {
                return Err(illegal_type(s.name, 2, d));
            }
            Ok(t)
        }),
        kernel(|c| {
            let defaults = match c.args.get(2) {
                Some(d) => read_all(d.as_ref(), &c.types[2])?,
                None => vec![default_value(c.ret); c.rows],
            };
            let defaults = defaults
                .into_iter()
                .map(|v| if v.is_null() { default_value(c.ret) } else { v })
                .collect();
            cast_column(
                c.args[0].as_ref(),
                &c.types[0],
                c.ret,
                &Mode::OrDefault(defaults),
            )
        }),
    )
    .plain()
    .register(r);

    // Numbers.
    for (suffix, t) in NUMBERS {
        for kind in KINDS {
            let name = format!("to{suffix}{}", kind.suffix());
            conversion(&name, kind, (0, 0), fixed(t.clone())).register(r);
        }
    }
    // Decimals: `toDecimal64(x, S)`.
    for (bits, precision) in [(32, 9u8), (64, 18), (128, 38)] {
        for kind in KINDS {
            let name = format!("toDecimal{bits}{}", kind.suffix());
            let target: Target = Arc::new(move |s: &Sig<'_>| -> Result<ChType, ChError> {
                let scale = scale_argument(s, 1, precision)?;
                Ok(ChType::Decimal { precision, scale })
            });
            conversion(&name, kind, (1, 1), target).register(r);
        }
    }
    // Dates and times: Task 30 has the plain forms.
    for kind in [Kind::OrZero, Kind::OrNull, Kind::OrDefault] {
        conversion(
            &format!("toDate{}", kind.suffix()),
            kind,
            (0, 0),
            fixed(ChType::Date),
        )
        .register(r);
        conversion(
            &format!("toDate32{}", kind.suffix()),
            kind,
            (0, 0),
            fixed(ChType::Date32),
        )
        .register(r);
        let datetime: Target = Arc::new(|s: &Sig<'_>| -> Result<ChType, ChError> {
            Ok(ChType::DateTime(tz_argument(s, 1)?))
        });
        conversion(
            &format!("toDateTime{}", kind.suffix()),
            kind,
            (0, 1),
            datetime,
        )
        .register(r);
        let datetime64: Target = Arc::new(|s: &Sig<'_>| -> Result<ChType, ChError> {
            let p = scale_argument(s, 1, 9)?;
            Ok(ChType::DateTime64(p, tz_argument(s, 2)?))
        });
        conversion(
            &format!("toDateTime64{}", kind.suffix()),
            kind,
            (1, 2),
            datetime64,
        )
        .register(r);
    }
    // Addresses.
    for (suffix, t) in [("IPv4", ChType::IPv4), ("IPv6", ChType::IPv6)] {
        for kind in KINDS {
            conversion(
                &format!("to{suffix}{}", kind.suffix()),
                kind,
                (0, 0),
                fixed(t.clone()),
            )
            .register(r);
        }
    }
    conversion("toBool", Kind::Plain, (0, 0), fixed(ChType::Bool)).register(r);
    let fixed_string: Target = Arc::new(|s: &Sig<'_>| -> Result<ChType, ChError> {
        let n = const_int(s.consts.get(1).and_then(Option::as_ref))
            .ok_or_else(|| not_constant(s.name, 1))?;
        if !(1..=i128::from(u32::MAX)).contains(&n) {
            return Err(ChError::argument_out_of_bound(format!(
                "Length of FixedString must be positive, got {n}"
            )));
        }
        Ok(ChType::FixedString(n as u32))
    });
    conversion("toFixedString", Kind::Plain, (1, 1), fixed_string).register(r);

    // `toString(x[, tz])`.
    ChFn::new(
        "toString",
        Nulls::Default,
        typer(|s| {
            arity(s, 1, 2)?;
            if s.types.len() == 2 {
                tz_argument(s, 1)?;
            }
            Ok(ChType::String)
        }),
        kernel(|c| {
            let from = match (&c.types[0], c.consts.get(1).and_then(Option::as_ref)) {
                (ChType::DateTime(_), Some(tz)) if c.args.len() == 2 => {
                    ChType::DateTime(const_str(Some(tz)))
                }
                (ChType::DateTime64(p, _), Some(tz)) if c.args.len() == 2 => {
                    ChType::DateTime64(*p, const_str(Some(tz)))
                }
                (t, _) => t.clone(),
            };
            cast_column_skipping_nulls(c.args[0].as_ref(), &from, c.ret, &Mode::Throw)
        }),
    )
    .register(r);

    // `toNullable`, `assumeNotNull`, `toLowCardinality`.
    ChFn::new(
        "toNullable",
        Nulls::Own,
        typer(|s| {
            arity(s, 1, 1)?;
            Ok(nullable(s.types[0].clone()))
        }),
        kernel(|c| Ok(c.args[0].clone())),
    )
    .register(r);
    ChFn::new(
        "assumeNotNull",
        Nulls::Own,
        typer(|s| {
            arity(s, 1, 1)?;
            Ok(not_null(&s.types[0]))
        }),
        kernel(|c| {
            let mut out = Vec::with_capacity(c.rows);
            for row in 0..c.rows {
                let v = read(c.args[0].as_ref(), &c.types[0], row)?;
                out.push(if v.is_null() { default_value(c.ret) } else { v });
            }
            value::build(c.ret, &out)
        }),
    )
    .register(r);
    ChFn::new(
        "toLowCardinality",
        Nulls::Own,
        typer(|s| {
            arity(s, 1, 1)?;
            let t = &s.types[0];
            Ok(match t {
                ChType::LowCardinality(_) => t.clone(),
                other if fits_low_cardinality(other.strip_nullable()) => {
                    ChType::LowCardinality(Box::new(other.clone()))
                }
                other => other.clone(),
            })
        }),
        kernel(|c| Ok(c.args[0].clone())),
    )
    .register(r);
}

/// `t` without `Nullable` (inside `LowCardinality` too).
pub fn not_null(t: &ChType) -> ChType {
    match t {
        ChType::Nullable(inner) => inner.as_ref().clone(),
        ChType::LowCardinality(inner) => ChType::LowCardinality(Box::new(not_null(inner))),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn every_tier0_conversion_function_is_registered() {
        let names = names();
        for f in [
            "CAST",
            "_CAST",
            "toString",
            "toUInt64",
            "toInt64",
            "toFloat64",
            "toBool",
            "toNullable",
            "toUInt8",
            "toInt32",
            "toUInt32",
            "toFloat64OrZero",
            "toInt64OrZero",
            "toInt32OrZero",
            "toFloat64OrDefault",
            "toDecimal64",
            "accurateCast",
            "accurateCastOrNull",
        ] {
            assert!(names.iter().any(|n| n == f), "{f} is not registered");
        }
        // Task 30's.
        for f in ["toDate", "toDateTime", "toDateTime64", "toDate32"] {
            assert!(!names.iter().any(|n| n == f), "{f} belongs to Task 30");
        }
    }

    #[test]
    fn every_name_is_a_clickhouse_function() {
        for n in names() {
            let known = loams_house_sql::functions::resolve(&n)
                .unwrap_or_else(|| panic!("{n} is not in functions.tsv"));
            assert_eq!(known.canonical, n, "{n} is registered under an alias");
        }
    }

    #[test]
    fn nullable_is_taken_off_inside_low_cardinality() {
        let t = parse("LowCardinality(Nullable(String))").unwrap();
        assert_eq!(not_null(&t), parse("LowCardinality(String)").unwrap());
    }
}
