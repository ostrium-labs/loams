//! ClickHouse's type rules as functions of [`ChType`]s: the least supertype,
//! the result types of arithmetic and the type two operands are compared in.
//!
//! Ported from ClickHouse (Apache-2.0, see `NOTICE`):
//! `src/DataTypes/getLeastSupertype.cpp` ([`least_supertype`]),
//! `src/DataTypes/NumberTraits.h` and `src/Functions/FunctionsArithmetic.h`
//! ([`arithmetic`]), `src/Functions/FunctionsComparison.h`
//! ([`comparison`]). Every rule was measured on the 26.9.2.8 oracle
//! (`scripts/house/gen-type-cases.py`); where 26.9 answers a `Variant`
//! because no common type exists, the engine answers `48` (R21.3: `Variant`
//! has no Arrow layout yet).

use loams_house_types::{ChError, ChType};

/// `386 NO_COMMON_TYPE`.
pub fn no_common_type(message: impl Into<String>) -> ChError {
    ChError {
        code: 386,
        name: "NO_COMMON_TYPE",
        message: message.into(),
    }
}

/// `53 TYPE_MISMATCH`.
pub fn type_mismatch(message: impl Into<String>) -> ChError {
    ChError {
        code: 53,
        name: "TYPE_MISMATCH",
        message: message.into(),
    }
}

/// `59 ILLEGAL_TYPE_OF_COLUMN_FOR_FILTER`.
pub fn illegal_filter(message: impl Into<String>) -> ChError {
    ChError {
        code: 59,
        name: "ILLEGAL_TYPE_OF_COLUMN_FOR_FILTER",
        message: message.into(),
    }
}

/// A number's shape, as `NumberTraits` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Num {
    /// Signed integer or float.
    pub signed: bool,
    /// A floating-point type.
    pub float: bool,
    /// Its size in bytes (1, 2, 4, 8).
    pub size: u8,
}

/// The shape of a plain number type. `Bool` counts as `UInt8` (ClickHouse's
/// `Bool` is a `UInt8` with a custom name).
pub fn num(t: &ChType) -> Option<Num> {
    let (signed, float, size) = match t {
        ChType::UInt8 | ChType::Bool => (false, false, 1),
        ChType::UInt16 => (false, false, 2),
        ChType::UInt32 => (false, false, 4),
        ChType::UInt64 => (false, false, 8),
        ChType::Int8 => (true, false, 1),
        ChType::Int16 => (true, false, 2),
        ChType::Int32 => (true, false, 4),
        ChType::Int64 => (true, false, 8),
        ChType::Float32 => (true, true, 4),
        ChType::Float64 => (true, true, 8),
        _ => return None,
    };
    Some(Num {
        signed,
        float,
        size,
    })
}

/// `NumberTraits::Construct<signed, floating, size>`.
pub fn construct(signed: bool, float: bool, size: u8) -> ChType {
    match (float, signed, size) {
        (true, _, s) if s <= 4 => ChType::Float32,
        (true, _, _) => ChType::Float64,
        (false, false, 1) => ChType::UInt8,
        (false, false, 2) => ChType::UInt16,
        (false, false, 4) => ChType::UInt32,
        (false, false, _) => ChType::UInt64,
        (false, true, 1) => ChType::Int8,
        (false, true, 2) => ChType::Int16,
        (false, true, 4) => ChType::Int32,
        (false, true, _) => ChType::Int64,
    }
}

/// `NumberTraits::nextSize`: twice the size, at most 8 bytes.
fn next_size(size: u8) -> u8 {
    (size * 2).min(8)
}

/// An arithmetic operator or function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    /// `plus`, `+`.
    Plus,
    /// `minus`, `-`.
    Minus,
    /// `multiply`, `*`.
    Multiply,
    /// `divide`, `/`.
    Divide,
    /// `modulo`, `%`.
    Modulo,
    /// `intDiv`.
    IntDiv,
}

impl Arith {
    /// ClickHouse's function name.
    pub fn name(self) -> &'static str {
        match self {
            Arith::Plus => "plus",
            Arith::Minus => "minus",
            Arith::Multiply => "multiply",
            Arith::Divide => "divide",
            Arith::Modulo => "modulo",
            Arith::IntDiv => "intDiv",
        }
    }
}

/// The storage width's full precision of a `Decimal(precision, …)`: 9, 18,
/// 38 or 76 (`Decimal32` … `Decimal256`).
pub fn decimal_width(precision: u8) -> u8 {
    match precision {
        0..=9 => 9,
        10..=18 => 18,
        19..=38 => 38,
        _ => 76,
    }
}

/// The digits an integer type needs (`leastDecimalPrecisionFor`).
fn integer_digits(t: &ChType) -> Option<u8> {
    Some(match t {
        ChType::UInt8 | ChType::Int8 | ChType::Bool => 3,
        ChType::UInt16 | ChType::Int16 => 5,
        ChType::UInt32 | ChType::Int32 => 10,
        ChType::UInt64 => 20,
        ChType::Int64 => 19,
        _ => return None,
    })
}

/// The integer type of a decimal's storage width (`intDiv` of a decimal).
fn decimal_storage_int(precision: u8) -> Result<ChType, ChError> {
    match decimal_width(precision) {
        9 => Ok(ChType::Int32),
        18 => Ok(ChType::Int64),
        _ => Err(ChError::unsupported(format!(
            "intDiv of Decimal({precision}, …), whose result is Int128 or Int256,"
        ))),
    }
}

fn illegal_types(op: Arith, a: &ChType, b: &ChType) -> ChError {
    ChError::illegal_type_of_argument(format!(
        "Illegal types {a} and {b} of arguments of function {}",
        op.name()
    ))
}

/// The result type of `op(a, b)` for two numbers (`Bool`, integers, floats,
/// decimals), `Nullable` and `LowCardinality` already stripped
/// (`FunctionsArithmetic.h`, `NumberTraits.h`; measured in
/// `corpus/he1/24`). Dates and times are [`date_arithmetic`]'s.
pub fn arithmetic(op: Arith, a: &ChType, b: &ChType) -> Result<ChType, ChError> {
    if let (Some(x), Some(y)) = (num(a), num(b)) {
        let float = x.float || y.float;
        return Ok(match op {
            Arith::Plus | Arith::Multiply => {
                construct(x.signed || y.signed, float, next_size(x.size.max(y.size)))
            }
            Arith::Minus => construct(true, float, next_size(x.size.max(y.size))),
            Arith::Divide => ChType::Float64,
            Arith::Modulo if float => ChType::Float64,
            Arith::Modulo => construct(
                x.signed,
                false,
                if x.signed { next_size(y.size) } else { y.size },
            ),
            Arith::IntDiv => construct(x.signed || y.signed, false, x.size),
        });
    }
    match (a, b) {
        (
            ChType::Decimal {
                precision: p1,
                scale: s1,
            },
            ChType::Decimal {
                precision: p2,
                scale: s2,
            },
        ) => {
            let precision = decimal_width(*p1).max(decimal_width(*p2));
            let scale = match op {
                Arith::Plus | Arith::Minus => (*s1).max(*s2),
                Arith::Multiply => s1 + s2,
                Arith::Divide | Arith::Modulo => {
                    if s2 > s1 {
                        return Err(ChError::argument_out_of_bound(
                            "Decimal result's scale is less than argument's one",
                        ));
                    }
                    *s1
                }
                Arith::IntDiv => {
                    if s2 > s1 {
                        return Err(ChError::argument_out_of_bound(
                            "Decimal result's scale is less than argument's one",
                        ));
                    }
                    return decimal_storage_int(*p1);
                }
            };
            if scale > precision {
                return Err(ChError::argument_out_of_bound(format!(
                    "Decimal result's scale {scale} is out of range for precision {precision}"
                )));
            }
            Ok(ChType::Decimal { precision, scale })
        }
        (ChType::Decimal { precision, .. }, other) | (other, ChType::Decimal { precision, .. })
            if num(other).is_some() =>
        {
            let decimal = if matches!(a, ChType::Decimal { .. }) {
                a
            } else {
                b
            };
            if num(other).is_some_and(|n| n.float) {
                return Ok(match op {
                    Arith::IntDiv if matches!(a, ChType::Decimal { .. }) => {
                        decimal_storage_int(*precision)?
                    }
                    Arith::IntDiv => arithmetic(op, a, &ChType::UInt8)?,
                    _ => ChType::Float64,
                });
            }
            match op {
                Arith::IntDiv if matches!(a, ChType::Decimal { .. }) => {
                    decimal_storage_int(*precision)
                }
                Arith::IntDiv => arithmetic(op, a, &ChType::UInt8),
                _ => Ok(decimal.clone()),
            }
        }
        _ => Err(illegal_types(op, a, b)),
    }
}

/// The result type of `-x` (`negate`): unsigned integers widen to the next
/// signed type, `UInt64` stays 64 bits.
pub fn negate(t: &ChType) -> Result<ChType, ChError> {
    if let Some(n) = num(t) {
        return Ok(if n.signed {
            t.clone()
        } else {
            construct(true, false, next_size(n.size))
        });
    }
    match t {
        ChType::Decimal { .. } => Ok(t.clone()),
        other => Err(ChError::illegal_type_of_argument(format!(
            "Illegal type {other} of argument of function negate"
        ))),
    }
}

/// Date and time arithmetic with a number: `Date ± n` (days), `DateTime ± n`
/// and `DateTime64 ± n` (seconds), `n + Date`, and `Date - Date` /
/// `DateTime - DateTime` (`Int32`). `None` when neither side is a date or
/// time.
pub fn date_arithmetic(op: Arith, a: &ChType, b: &ChType) -> Option<Result<ChType, ChError>> {
    let is_time = |t: &ChType| {
        matches!(
            t,
            ChType::Date | ChType::Date32 | ChType::DateTime(_) | ChType::DateTime64(..)
        )
    };
    if !is_time(a) && !is_time(b) {
        return None;
    }
    let integer = |t: &ChType| num(t).is_some_and(|n| !n.float);
    Some(match (op, a, b) {
        (Arith::Plus | Arith::Minus, t, n) if is_time(t) && integer(n) => Ok(t.clone()),
        (Arith::Plus, n, t) if is_time(t) && integer(n) => Ok(t.clone()),
        (Arith::Minus, ChType::Date, ChType::Date)
        | (Arith::Minus, ChType::Date32, ChType::Date32)
        | (Arith::Minus, ChType::DateTime(_), ChType::DateTime(_)) => Ok(ChType::Int32),
        _ => Err(ChError::not_implemented(format!(
            "{}({a}, {b}) is not supported by the Loams House engine yet (plan HE1 Task 30)",
            op.name()
        ))),
    })
}

fn same_type(a: &ChType, b: &ChType) -> bool {
    // ClickHouse's `IDataType::equals`: `Bool` equals `UInt8` (a custom name
    // over the same type), and decimals of one storage width equal when their
    // scales do.
    match (a, b) {
        (ChType::Bool, ChType::UInt8) | (ChType::UInt8, ChType::Bool) => true,
        (
            ChType::Decimal {
                precision: p1,
                scale: s1,
            },
            ChType::Decimal {
                precision: p2,
                scale: s2,
            },
        ) => decimal_width(*p1) == decimal_width(*p2) && s1 == s2,
        (ChType::Nullable(x), ChType::Nullable(y))
        | (ChType::LowCardinality(x), ChType::LowCardinality(y))
        | (ChType::Array(x), ChType::Array(y)) => same_type(x, y),
        _ => a == b,
    }
}

fn no_common(types: &[ChType]) -> ChError {
    ChError::unsupported(format!(
        "A common type of {} (ClickHouse 26.9 answers a Variant here)",
        types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The least supertype of `types` (`getLeastSupertype`), the type `if`,
/// `multiIf`, `CASE`, `coalesce`, array literals and `UNION` branches are
/// converted to. Where ClickHouse 26.9 answers a `Variant` (no common type),
/// this is `48`.
pub fn least_supertype(types: &[ChType]) -> Result<ChType, ChError> {
    let Some(first) = types.first() else {
        return Ok(ChType::Nothing);
    };
    if types.iter().all(|t| same_type(t, first)) {
        return Ok(first.clone());
    }
    // LowCardinality: all of them keeps it, any other type drops it (a
    // `Nothing`, `NULL`, counts as another type).
    if types.iter().any(|t| matches!(t, ChType::LowCardinality(_))) {
        let all = types.iter().all(|t| matches!(t, ChType::LowCardinality(_)));
        let inner: Vec<ChType> = types
            .iter()
            .map(|t| match t {
                ChType::LowCardinality(x) => (**x).clone(),
                other => other.clone(),
            })
            .collect();
        let s = least_supertype(&inner)?;
        return Ok(if all {
            ChType::LowCardinality(Box::new(s))
        } else {
            s
        });
    }
    // Nothing and Nullable.
    let have_null = types
        .iter()
        .any(|t| matches!(t, ChType::Nothing | ChType::Nullable(_)));
    if have_null {
        let rest: Vec<ChType> = types
            .iter()
            .filter(|t| !matches!(t, ChType::Nothing))
            .map(|t| t.strip_nullable().clone())
            .collect();
        if rest.is_empty() {
            return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
        }
        let s = least_supertype(&rest)?;
        return Ok(loams_house_types::arrow::nullable(s));
    }
    // Arrays and tuples, element by element.
    if types.iter().all(|t| matches!(t, ChType::Array(_))) {
        let inner: Vec<ChType> = types
            .iter()
            .map(|t| match t {
                ChType::Array(x) => (**x).clone(),
                _ => unreachable!("all arrays"),
            })
            .collect();
        return Ok(ChType::Array(Box::new(least_supertype(&inner)?)));
    }
    if let ChType::Tuple(first_elems) = first
        && types
            .iter()
            .all(|t| matches!(t, ChType::Tuple(e) if e.len() == first_elems.len()))
    {
        let mut out = Vec::with_capacity(first_elems.len());
        for (i, (name, _)) in first_elems.iter().enumerate() {
            let column: Vec<ChType> = types
                .iter()
                .map(|t| match t {
                    ChType::Tuple(e) => e[i].1.clone(),
                    _ => unreachable!("all tuples"),
                })
                .collect();
            let same_name = types
                .iter()
                .all(|t| matches!(t, ChType::Tuple(e) if e[i].0 == *name));
            out.push((
                if same_name { name.clone() } else { None },
                least_supertype(&column)?,
            ));
        }
        if out.iter().any(|(n, _)| n.is_none()) {
            for e in &mut out {
                e.0 = None;
            }
        }
        return Ok(ChType::Tuple(out));
    }
    // Strings: String, FixedString and Enum (by name) give String.
    let stringy = |t: &ChType| matches!(t, ChType::String | ChType::FixedString(_)) || is_enum(t);
    if types
        .iter()
        .any(|t| matches!(t, ChType::String | ChType::FixedString(_)))
    {
        if types.iter().all(stringy) {
            return Ok(ChType::String);
        }
        return Err(no_common(types));
    }
    // Dates and times.
    let timey = |t: &ChType| {
        matches!(
            t,
            ChType::Date | ChType::Date32 | ChType::DateTime(_) | ChType::DateTime64(..)
        )
    };
    if types.iter().any(timey) {
        if !types.iter().all(timey) {
            return Err(no_common(types));
        }
        let max_scale = types
            .iter()
            .filter_map(|t| match t {
                ChType::DateTime64(p, _) => Some(*p),
                _ => None,
            })
            .max();
        let has_date32 = types.iter().any(|t| matches!(t, ChType::Date32));
        let has_datetime = types.iter().any(|t| matches!(t, ChType::DateTime(_)));
        if let Some(p) = max_scale {
            // The time zone of the (first) DateTime64 of the largest scale.
            let tz = types.iter().find_map(|t| match t {
                ChType::DateTime64(q, tz) if *q == p => Some(tz.clone()),
                _ => None,
            });
            return Ok(ChType::DateTime64(p, tz.flatten()));
        }
        if has_datetime {
            let tz = types.iter().find_map(|t| match t {
                ChType::DateTime(tz) => Some(tz.clone()),
                _ => None,
            });
            if has_date32 {
                return Ok(ChType::DateTime64(0, tz.flatten()));
            }
            return Ok(ChType::DateTime(tz.flatten()));
        }
        return Ok(if has_date32 {
            ChType::Date32
        } else {
            ChType::Date
        });
    }
    // Decimals, with integers.
    if types.iter().any(|t| matches!(t, ChType::Decimal { .. })) {
        let mut scale = 0u8;
        let mut digits = 0u8;
        let mut width = 9u8;
        for t in types {
            match t {
                ChType::Decimal {
                    precision,
                    scale: s,
                } => {
                    scale = scale.max(*s);
                    digits = digits.max(precision - s);
                    width = width.max(decimal_width(*precision));
                }
                other => match integer_digits(other) {
                    Some(d) => digits = digits.max(d),
                    None => return Err(no_common(types)),
                },
            }
        }
        let needed = digits.saturating_add(scale);
        if needed > 76 {
            return Err(no_common(types));
        }
        let precision = width.max(decimal_width(needed));
        return Ok(ChType::Decimal { precision, scale });
    }
    // Numbers; an Enum is its integer.
    let mut signed_bits = 0u16;
    let mut unsigned_bits = 0u16;
    let mut mantissa_bits = 0u16;
    for t in types {
        match t {
            ChType::Enum8(_) => signed_bits = signed_bits.max(8),
            ChType::Enum16(_) => signed_bits = signed_bits.max(16),
            ChType::Float32 => mantissa_bits = mantissa_bits.max(24),
            ChType::Float64 => mantissa_bits = mantissa_bits.max(53),
            other => match num(other) {
                Some(n) if n.signed => signed_bits = signed_bits.max(u16::from(n.size) * 8),
                Some(n) => unsigned_bits = unsigned_bits.max(u16::from(n.size) * 8),
                None => return Err(no_common(types)),
            },
        }
    }
    let mut int_bits = signed_bits.max(unsigned_bits);
    if signed_bits > 0 && unsigned_bits >= signed_bits {
        int_bits += 1;
    }
    if mantissa_bits > 0 {
        let needed = int_bits.max(mantissa_bits);
        return match needed {
            0..=24 => Ok(ChType::Float32),
            25..=53 => Ok(ChType::Float64),
            _ => Err(no_common(types)),
        };
    }
    if signed_bits > 0 {
        return match int_bits {
            0..=8 => Ok(ChType::Int8),
            9..=16 => Ok(ChType::Int16),
            17..=32 => Ok(ChType::Int32),
            33..=64 => Ok(ChType::Int64),
            _ => Err(no_common(types)),
        };
    }
    Ok(match int_bits {
        0..=8 => ChType::UInt8,
        9..=16 => ChType::UInt16,
        17..=32 => ChType::UInt32,
        _ => ChType::UInt64,
    })
}

/// Whether `t` is an `Enum8` or `Enum16`.
pub fn is_enum(t: &ChType) -> bool {
    matches!(t, ChType::Enum8(_) | ChType::Enum16(_))
}

/// How two operands are compared: the type both are converted to. ClickHouse
/// compares numbers exactly across signedness and width (`accurate::lessOp`),
/// so `UInt64` against a signed type compares as `Decimal(20, 0)`, which holds
/// both, and an unsigned type against a wider signed one in the signed type.
/// `Date` against `DateTime` compares as `DateTime` (`FunctionsComparison.h`).
pub fn comparison(a: &ChType, b: &ChType) -> Result<ChType, ChError> {
    if same_type(a, b) && !matches!((a, b), (ChType::Bool, _) | (_, ChType::Bool)) {
        return Ok(match (a, b) {
            (
                ChType::Decimal {
                    precision: p1,
                    scale,
                },
                ChType::Decimal { precision: p2, .. },
            ) => ChType::Decimal {
                precision: (*p1).max(*p2),
                scale: *scale,
            },
            _ => a.clone(),
        });
    }
    let enum_int = |t: &ChType| match t {
        ChType::Enum8(_) => ChType::Int8,
        ChType::Enum16(_) => ChType::Int16,
        other => other.clone(),
    };
    let (a, b) = (enum_int(a), enum_int(b));
    if let (Some(x), Some(y)) = (num(&a), num(&b)) {
        if x.float || y.float {
            return Ok(if a == ChType::Float32 && b == ChType::Float32 {
                ChType::Float32
            } else {
                ChType::Float64
            });
        }
        if x.signed == y.signed {
            return Ok(construct(x.signed, false, x.size.max(y.size)));
        }
        let (s, u) = if x.signed { (x, y) } else { (y, x) };
        if u.size < s.size {
            return Ok(construct(true, false, s.size));
        }
        if u.size < 8 {
            return Ok(construct(true, false, (u.size * 2).max(s.size)));
        }
        return Ok(ChType::Decimal {
            precision: 20,
            scale: 0,
        });
    }
    let decimal_parts = |t: &ChType| match t {
        ChType::Decimal { precision, scale } => Some((precision - scale, *scale)),
        other => integer_digits(other).map(|d| (d, 0)),
    };
    if matches!(a, ChType::Decimal { .. }) || matches!(b, ChType::Decimal { .. }) {
        if num(&a).is_some_and(|n| n.float) || num(&b).is_some_and(|n| n.float) {
            return Ok(ChType::Float64);
        }
        if let (Some((d1, s1)), Some((d2, s2))) = (decimal_parts(&a), decimal_parts(&b)) {
            let scale = s1.max(s2);
            let precision = (d1.max(d2) + scale).min(76);
            return Ok(ChType::Decimal {
                precision: precision.max(1),
                scale,
            });
        }
    }
    let timey = |t: &ChType| {
        matches!(
            t,
            ChType::Date | ChType::Date32 | ChType::DateTime(_) | ChType::DateTime64(..)
        )
    };
    if timey(&a) && timey(&b) {
        return Ok(match (&a, &b) {
            (ChType::Date | ChType::Date32, ChType::Date | ChType::Date32) => ChType::Date32,
            (ChType::DateTime64(p, tz), ChType::DateTime64(q, _)) => {
                ChType::DateTime64((*p).max(*q), tz.clone())
            }
            (ChType::DateTime64(p, tz), _) | (_, ChType::DateTime64(p, tz)) => {
                ChType::DateTime64(*p, tz.clone())
            }
            (ChType::DateTime(tz), _) | (_, ChType::DateTime(tz)) => ChType::DateTime(tz.clone()),
            _ => unreachable!("both dates or times"),
        });
    }
    let stringy = |t: &ChType| matches!(t, ChType::String | ChType::FixedString(_));
    if stringy(&a) && stringy(&b) {
        return Ok(ChType::String);
    }
    Err(no_common_type(format!(
        "There is no supertype for types {a}, {b} because some of them are String/FixedString/Enum and some of them are not"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_house_types::parse;

    fn t(s: &str) -> ChType {
        parse(s).expect("type")
    }

    fn sup(list: &[&str]) -> String {
        least_supertype(&list.iter().map(|s| t(s)).collect::<Vec<_>>())
            .map(|t| t.to_string())
            .unwrap_or_else(|e| format!("ERR {}", e.code))
    }

    #[test]
    fn supertypes_as_measured() {
        // Rows of the oracle's `toTypeName([a, b])` matrix (26.9.2.8).
        for (types, want) in [
            (&["UInt8", "Int8"][..], "Int16"),
            (&["UInt32", "Int8"], "Int64"),
            (&["UInt64", "Int8"], "ERR 48"),
            (&["Float32", "Int32"], "Float64"),
            (&["Float32", "Int16"], "Float32"),
            (&["Int64", "Float64"], "ERR 48"),
            (&["Bool", "UInt8"], "Bool"),
            (&["UInt8", "Bool"], "UInt8"),
            (&["Bool", "Int8"], "Int16"),
            (&["Decimal(9, 2)", "UInt32"], "Decimal(18, 2)"),
            (&["Decimal(9, 2)", "Int64"], "Decimal(38, 2)"),
            (&["Decimal(5, 2)", "Decimal(6, 2)"], "Decimal(5, 2)"),
            (&["Decimal(5, 2)", "Decimal(6, 3)"], "Decimal(9, 3)"),
            (&["String", "FixedString(2)"], "String"),
            (&["Enum8('a' = 1)", "String"], "String"),
            (&["Enum8('a' = 1)", "UInt8"], "Int16"),
            (&["Date", "Date32"], "Date32"),
            (&["Date32", "DateTime"], "DateTime64(0)"),
            (&["Date", "DateTime('UTC')"], "DateTime('UTC')"),
            (&["DateTime64(3, 'UTC')", "DateTime64(6)"], "DateTime64(6)"),
            (&["Nothing", "UInt8"], "Nullable(UInt8)"),
            (&["Nullable(UInt8)", "Int8"], "Nullable(Int16)"),
            (
                &["LowCardinality(String)", "LowCardinality(String)"],
                "LowCardinality(String)",
            ),
            (&["LowCardinality(String)", "String"], "String"),
            (&["LowCardinality(String)", "Nothing"], "Nullable(String)"),
            (&["String", "UInt8"], "ERR 48"),
        ] {
            assert_eq!(sup(types), want, "{types:?}");
        }
    }

    #[test]
    fn arithmetic_as_measured() {
        let a = |op, x: &str, y: &str| arithmetic(op, &t(x), &t(y)).unwrap().to_string();
        assert_eq!(a(Arith::Plus, "UInt8", "UInt8"), "UInt16");
        assert_eq!(a(Arith::Minus, "UInt8", "UInt8"), "Int16");
        assert_eq!(a(Arith::Plus, "UInt64", "Int8"), "Int64");
        assert_eq!(a(Arith::Multiply, "Float32", "Float32"), "Float64");
        assert_eq!(a(Arith::Divide, "UInt8", "UInt8"), "Float64");
        assert_eq!(a(Arith::Modulo, "UInt16", "UInt8"), "UInt8");
        assert_eq!(a(Arith::Modulo, "Int64", "UInt8"), "Int16");
        assert_eq!(a(Arith::Modulo, "UInt8", "Int8"), "UInt8");
        assert_eq!(a(Arith::IntDiv, "UInt8", "Int64"), "Int8");
        assert_eq!(a(Arith::IntDiv, "Float32", "UInt8"), "Int32");
        assert_eq!(a(Arith::Plus, "Bool", "Bool"), "UInt16");
        assert_eq!(
            a(Arith::Plus, "Decimal(5, 2)", "Decimal(5, 2)"),
            "Decimal(9, 2)"
        );
        assert_eq!(a(Arith::Plus, "Decimal(5, 2)", "UInt64"), "Decimal(5, 2)");
        assert_eq!(
            a(Arith::Multiply, "Decimal(10, 2)", "Decimal(9, 3)"),
            "Decimal(18, 5)"
        );
        assert_eq!(a(Arith::Divide, "Decimal(9, 2)", "Float32"), "Float64");
        assert_eq!(a(Arith::IntDiv, "Decimal(9, 2)", "UInt8"), "Int32");
        assert_eq!(negate(&t("UInt8")).unwrap(), ChType::Int16);
        assert_eq!(negate(&t("UInt64")).unwrap(), ChType::Int64);
    }

    #[test]
    fn comparisons_are_exact() {
        let c = |x: &str, y: &str| comparison(&t(x), &t(y)).unwrap().to_string();
        assert_eq!(c("Int8", "UInt64"), "Decimal(20, 0)");
        assert_eq!(c("UInt8", "Int8"), "Int16");
        assert_eq!(c("UInt32", "Int64"), "Int64");
        assert_eq!(c("Date", "DateTime('UTC')"), "DateTime('UTC')");
        assert_eq!(c("Float32", "Float64"), "Float64");
        assert_eq!(c("Decimal(9, 2)", "Int64"), "Decimal(21, 2)");
        assert_eq!(comparison(&t("String"), &t("UInt8")).unwrap_err().code, 386);
    }
}
