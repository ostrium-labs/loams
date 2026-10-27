//! The least supertype of several ClickHouse types (`getLeastSupertype`,
//! `src/DataTypes/getLeastSupertype.cpp`, Apache-2.0), for `if`, `multiIf`,
//! `coalesce`, `greatest` and the rest.
//!
//! Task 24 (PR #1300) has the same rules in `loams-house-sql`'s
//! `coerce::rules::least_supertype`; this copy stands until that merges, and
//! R33.5 records replacing it with that one.

use loams_house_types::arrow::nullable;
use loams_house_types::{ChError, ChType};

/// `386 NO_COMMON_TYPE`.
pub fn no_common_type(types: &[ChType], why: &str) -> ChError {
    let names: Vec<String> = types.iter().map(ToString::to_string).collect();
    ChError {
        code: 386,
        name: "NO_COMMON_TYPE",
        message: format!(
            "There is no supertype for types {} because {why}",
            names.join(", ")
        ),
    }
}

fn int_bits(t: &ChType) -> Option<(bool, u32)> {
    Some(match t {
        ChType::UInt8 | ChType::Bool => (false, 8),
        ChType::UInt16 => (false, 16),
        ChType::UInt32 => (false, 32),
        ChType::UInt64 => (false, 64),
        ChType::Int8 => (true, 8),
        ChType::Int16 => (true, 16),
        ChType::Int32 => (true, 32),
        ChType::Int64 => (true, 64),
        _ => return None,
    })
}

/// The decimal digits an integer type needs.
fn int_digits(t: &ChType) -> Option<u8> {
    Some(match t {
        ChType::UInt8 | ChType::Int8 | ChType::Bool => 3,
        ChType::UInt16 | ChType::Int16 => 5,
        ChType::UInt32 | ChType::Int32 => 10,
        ChType::UInt64 => 20,
        ChType::Int64 => 19,
        _ => return None,
    })
}

fn is_time(t: &ChType) -> bool {
    matches!(
        t,
        ChType::Date | ChType::Date32 | ChType::DateTime(_) | ChType::DateTime64(..)
    )
}

/// The least supertype of `types`.
pub fn least_supertype(types: &[ChType]) -> Result<ChType, ChError> {
    if types.is_empty() {
        return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
    }
    // `Nothing` (a `NULL`) joins anything, as `Nullable`.
    let mut had_nothing = false;
    let rest: Vec<ChType> = types
        .iter()
        .filter(|t| {
            let nothing = matches!(t.strip_nullable(), ChType::Nothing);
            had_nothing |= nothing;
            !nothing
        })
        .cloned()
        .collect();
    if rest.is_empty() {
        return Ok(ChType::Nullable(Box::new(ChType::Nothing)));
    }
    if had_nothing {
        let inner = least_supertype(&rest)?;
        return Ok(nullable(inner));
    }
    // `LowCardinality`: all of them gives `LowCardinality`, some of them
    // gives the plain type.
    let lc = rest
        .iter()
        .filter(|t| matches!(t, ChType::LowCardinality(_)))
        .count();
    if lc > 0 {
        let inner: Vec<ChType> = rest
            .iter()
            .map(|t| match t {
                ChType::LowCardinality(inner) => inner.as_ref().clone(),
                other => other.clone(),
            })
            .collect();
        let s = least_supertype(&inner)?;
        return Ok(if lc == rest.len() {
            ChType::LowCardinality(Box::new(s))
        } else {
            s
        });
    }
    if rest.iter().any(|t| matches!(t, ChType::Nullable(_))) {
        let inner: Vec<ChType> = rest.iter().map(|t| t.strip_nullable().clone()).collect();
        return Ok(nullable(least_supertype(&inner)?));
    }
    let first = &rest[0];
    if rest.iter().all(|t| t == first) {
        return Ok(first.clone());
    }
    // Arrays, element by element.
    if rest.iter().all(|t| matches!(t, ChType::Array(_))) {
        let inner: Vec<ChType> = rest
            .iter()
            .filter_map(|t| match t {
                ChType::Array(inner) => Some(inner.as_ref().clone()),
                _ => None,
            })
            .collect();
        return Ok(ChType::Array(Box::new(least_supertype(&inner)?)));
    }
    // Tuples of one arity, element by element.
    if let ChType::Tuple(first_elements) = first
        && rest
            .iter()
            .all(|t| matches!(t, ChType::Tuple(e) if e.len() == first_elements.len()))
    {
        let mut out = Vec::with_capacity(first_elements.len());
        for (i, (first_name, _)) in first_elements.iter().enumerate() {
            let column: Vec<ChType> = rest
                .iter()
                .filter_map(|t| match t {
                    ChType::Tuple(e) => Some(e[i].1.clone()),
                    _ => None,
                })
                .collect();
            let same_name = rest.iter().all(|t| match t {
                ChType::Tuple(e) => e[i].0 == *first_name,
                _ => false,
            });
            let name = if same_name { first_name.clone() } else { None };
            out.push((name, least_supertype(&column)?));
        }
        // Names on every element or on none.
        if out.iter().any(|(n, _)| n.is_none()) {
            for e in &mut out {
                e.0 = None;
            }
        }
        return Ok(ChType::Tuple(out));
    }
    // Strings.
    if rest
        .iter()
        .all(|t| matches!(t, ChType::String | ChType::FixedString(_)))
    {
        return Ok(ChType::String);
    }
    // Dates and times.
    if rest.iter().all(is_time) {
        let max_p = rest
            .iter()
            .filter_map(|t| match t {
                ChType::DateTime64(p, _) => Some(*p),
                _ => None,
            })
            .max();
        let zones: Vec<&Option<String>> = rest
            .iter()
            .filter_map(|t| match t {
                ChType::DateTime(tz) | ChType::DateTime64(_, tz) => Some(tz),
                _ => None,
            })
            .collect();
        let tz = zones.first().and_then(|z| (*z).clone());
        if let Some(p) = max_p {
            return Ok(ChType::DateTime64(p, tz));
        }
        if !zones.is_empty() {
            return Ok(ChType::DateTime(tz));
        }
        if rest.contains(&ChType::Date32) {
            return Ok(ChType::Date32);
        }
        return Ok(ChType::Date);
    }
    // Decimals with integers.
    if rest.iter().any(|t| matches!(t, ChType::Decimal { .. })) {
        let mut scale = 0u8;
        let mut int_part = 0u8;
        for t in &rest {
            match t {
                ChType::Decimal {
                    precision,
                    scale: s,
                } => {
                    scale = scale.max(*s);
                    int_part = int_part.max(precision - s);
                }
                other => match int_digits(other) {
                    Some(d) => int_part = int_part.max(d),
                    None => {
                        return Err(no_common_type(
                            types,
                            "some of them are Decimal and some are not integers",
                        ));
                    }
                },
            }
        }
        let digits = u16::from(int_part) + u16::from(scale);
        let precision = match digits {
            0..=9 => 9,
            10..=18 => 18,
            19..=38 => 38,
            _ => {
                return Err(ChError::unsupported(format!(
                    "The supertype Decimal of {digits} digits"
                )));
            }
        };
        return Ok(ChType::Decimal { precision, scale });
    }
    // Numbers.
    let mut signed_bits = 0u32;
    let mut unsigned_bits = 0u32;
    let mut float_bits = 0u32;
    for t in &rest {
        match t {
            ChType::Float32 => float_bits = float_bits.max(24),
            ChType::Float64 => float_bits = float_bits.max(53),
            other => match int_bits(other) {
                Some((true, b)) => signed_bits = signed_bits.max(b),
                Some((false, b)) => unsigned_bits = unsigned_bits.max(b),
                None => {
                    return Err(no_common_type(
                        types,
                        "some of them are numbers and some of them are not",
                    ));
                }
            },
        }
    }
    if float_bits > 0 {
        let needed = float_bits.max(signed_bits).max(unsigned_bits);
        return match needed {
            0..=24 => Ok(ChType::Float32),
            25..=53 => Ok(ChType::Float64),
            _ => Err(no_common_type(
                types,
                "some of them are integers and some are floating point, but there is no floating point type, that can exactly represent all required integers",
            )),
        };
    }
    if signed_bits > 0 {
        let needed = signed_bits.max(if unsigned_bits > 0 {
            unsigned_bits * 2
        } else {
            0
        });
        // `UInt8` with `Int8` is `Int16`: an unsigned type needs the next
        // signed width.
        return match needed {
            0..=8 => Ok(ChType::Int8),
            9..=16 => Ok(ChType::Int16),
            17..=32 => Ok(ChType::Int32),
            33..=64 => Ok(ChType::Int64),
            _ => Err(ChError::unsupported(
                "The supertype Int128 (UInt64 with a signed integer)",
            )),
        };
    }
    Ok(match unsigned_bits {
        0..=8 => ChType::UInt8,
        9..=16 => ChType::UInt16,
        17..=32 => ChType::UInt32,
        _ => ChType::UInt64,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use loams_house_types::parse;

    fn s(types: &[&str]) -> Result<String, ChError> {
        let types: Vec<ChType> = types.iter().map(|t| parse(t).unwrap()).collect();
        least_supertype(&types).map(|t| t.to_string())
    }

    #[test]
    fn supertypes_follow_clickhouse() {
        assert_eq!(s(&["UInt8", "UInt16"]).unwrap(), "UInt16");
        assert_eq!(s(&["UInt8", "Int8"]).unwrap(), "Int16");
        assert_eq!(s(&["UInt32", "Int8"]).unwrap(), "Int64");
        assert_eq!(s(&["Int16", "Float32"]).unwrap(), "Float32");
        assert_eq!(s(&["Int32", "Float32"]).unwrap(), "Float64");
        assert_eq!(s(&["Int64", "Float64"]).unwrap_err().code, 386);
        assert_eq!(s(&["UInt64", "Int64"]).unwrap_err().code, 48);
        assert_eq!(s(&["String", "FixedString(3)"]).unwrap(), "String");
        assert_eq!(s(&["String", "UInt8"]).unwrap_err().code, 386);
        assert_eq!(
            s(&["Nullable(Nothing)", "UInt8"]).unwrap(),
            "Nullable(UInt8)"
        );
        assert_eq!(s(&["Nullable(UInt8)", "Int8"]).unwrap(), "Nullable(Int16)");
        assert_eq!(
            s(&["LowCardinality(String)", "LowCardinality(String)"]).unwrap(),
            "LowCardinality(String)"
        );
        assert_eq!(s(&["LowCardinality(String)", "String"]).unwrap(), "String");
        assert_eq!(s(&["Date", "DateTime"]).unwrap(), "DateTime");
        assert_eq!(s(&["DateTime", "DateTime64(3)"]).unwrap(), "DateTime64(3)");
        assert_eq!(s(&["Decimal(9, 2)", "Int32"]).unwrap(), "Decimal(18, 2)");
        assert_eq!(s(&["Array(UInt8)", "Array(Int8)"]).unwrap(), "Array(Int16)");
        assert_eq!(s(&["Bool", "Bool"]).unwrap(), "Bool");
        assert_eq!(s(&["Bool", "UInt8"]).unwrap(), "UInt8");
    }
}
