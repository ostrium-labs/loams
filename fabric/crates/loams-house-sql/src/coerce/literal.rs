//! Literals as ClickHouse types them: a number literal is the smallest
//! unsigned type that holds it (`1` is `UInt8`, `256` is `UInt16`), or the
//! smallest signed one when negative (`-1` is `Int8`); and a string constant
//! compared with a typed value is read as that type (`d = '2026-10-01'`).

use std::net::{Ipv4Addr, Ipv6Addr};

use datafusion::arrow::datatypes::DataType;
use datafusion::common::ScalarValue;
use loams_house_types::{ChError, ChType};

use super::rules::type_mismatch;

/// The ClickHouse type of a number literal DataFusion's `SqlToRel` planned as
/// `Int64` or `UInt64`, as a value of that type (`ExpressionElementParsers`:
/// a literal without a minus sign is unsigned).
pub fn narrow(v: &ScalarValue) -> Option<ScalarValue> {
    match v {
        ScalarValue::Int64(Some(n)) => Some(narrow_i128(i128::from(*n))),
        _ => None,
    }
}

/// `n` in the smallest type a ClickHouse literal would have.
pub fn narrow_i128(n: i128) -> ScalarValue {
    if n >= 0 {
        if let Ok(v) = u8::try_from(n) {
            ScalarValue::UInt8(Some(v))
        } else if let Ok(v) = u16::try_from(n) {
            ScalarValue::UInt16(Some(v))
        } else if let Ok(v) = u32::try_from(n) {
            ScalarValue::UInt32(Some(v))
        } else if let Ok(v) = u64::try_from(n) {
            ScalarValue::UInt64(Some(v))
        } else {
            ScalarValue::Float64(Some(n as f64))
        }
    } else if let Ok(v) = i8::try_from(n) {
        ScalarValue::Int8(Some(v))
    } else if let Ok(v) = i16::try_from(n) {
        ScalarValue::Int16(Some(v))
    } else if let Ok(v) = i32::try_from(n) {
        ScalarValue::Int32(Some(v))
    } else if let Ok(v) = i64::try_from(n) {
        ScalarValue::Int64(Some(v))
    } else {
        ScalarValue::Float64(Some(n as f64))
    }
}

/// A string constant read as a value of `target` (its base type, without
/// `Nullable` and `LowCardinality`), for a comparison or an `IN` list:
/// ClickHouse converts the constant to the other operand's type
/// (`FunctionsComparison.h`, `executeWithConstString`). The result is the
/// literal and its ClickHouse type; a number is typed as a literal of its own
/// value, so `u8 = '300'` compares exactly (false) instead of wrapping.
/// `Ok(None)`: an `Enum` name that is not an element, which compares by
/// name instead.
///
/// # Errors
///
/// `53 TYPE_MISMATCH` when the text is not a value of the type.
pub fn convert(s: &str, target: &ChType) -> Result<Option<(ScalarValue, ChType)>, ChError> {
    let mismatch = || type_mismatch(format!("Cannot convert string '{s}' to type {target}"));
    let t = s.trim();
    Ok(Some(match target {
        ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Int8
        | ChType::Int16
        | ChType::Int32
        | ChType::Int64 => {
            let n: i128 = t.parse().map_err(|_| mismatch())?;
            let v = narrow_i128(n);
            let ty = loams_house_types::arrow::from_arrow(
                &datafusion::arrow::datatypes::Field::new("", v.data_type(), false),
            )?;
            (v, ty)
        }
        ChType::Float32 | ChType::Float64 => {
            let f: f64 = t.parse().map_err(|_| mismatch())?;
            (ScalarValue::Float64(Some(f)), ChType::Float64)
        }
        ChType::Bool => {
            let b = match t.to_ascii_lowercase().as_str() {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => return Err(mismatch()),
            };
            (ScalarValue::Boolean(Some(b)), ChType::Bool)
        }
        ChType::Decimal { precision, scale } => {
            let (dt, _) = loams_house_types::arrow::data_type(target)?;
            if t.parse::<f64>().is_err() {
                return Err(mismatch());
            }
            let v = ScalarValue::Utf8(Some(t.to_string()))
                .cast_to(&dt)
                .map_err(|_| mismatch())?;
            (
                v,
                ChType::Decimal {
                    precision: *precision,
                    scale: *scale,
                },
            )
        }
        ChType::Date | ChType::Date32 => {
            // `YYYY-MM-DD` only: a time part is `53` for a `Date`.
            let b = t.as_bytes();
            if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
                return Err(mismatch());
            }
            let v = ScalarValue::Utf8(Some(t.to_string()))
                .cast_to(&DataType::Date32)
                .map_err(|_| mismatch())?;
            (v, target.clone())
        }
        ChType::DateTime(_) | ChType::DateTime64(..) => {
            let (dt, _) = loams_house_types::arrow::data_type(target)?;
            let v = ScalarValue::Utf8(Some(t.to_string()))
                .cast_to(&dt)
                .map_err(|_| mismatch())?;
            (v, target.clone())
        }
        ChType::Enum8(values) => match values.iter().find(|(n, _)| n == s) {
            Some((_, v)) => (ScalarValue::Int8(Some(*v)), target.clone()),
            None => return Ok(None),
        },
        ChType::Enum16(values) => match values.iter().find(|(n, _)| n == s) {
            Some((_, v)) => (ScalarValue::Int16(Some(*v)), target.clone()),
            None => return Ok(None),
        },
        ChType::Uuid => {
            let hex: String = t.chars().filter(|c| *c != '-').collect();
            if hex.len() != 32 {
                return Err(mismatch());
            }
            let mut bytes = Vec::with_capacity(16);
            for i in 0..16 {
                bytes.push(u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| mismatch())?);
            }
            (ScalarValue::FixedSizeBinary(16, Some(bytes)), ChType::Uuid)
        }
        ChType::IPv4 => {
            let ip: Ipv4Addr = t.parse().map_err(|_| mismatch())?;
            (ScalarValue::UInt32(Some(u32::from(ip))), ChType::IPv4)
        }
        ChType::IPv6 => {
            let ip: Ipv6Addr = match t.parse::<Ipv6Addr>() {
                Ok(ip) => ip,
                Err(_) => t
                    .parse::<Ipv4Addr>()
                    .map(|v4| v4.to_ipv6_mapped())
                    .map_err(|_| mismatch())?,
            };
            (
                ScalarValue::FixedSizeBinary(16, Some(ip.octets().to_vec())),
                ChType::IPv6,
            )
        }
        ChType::String => (ScalarValue::Utf8(Some(s.to_string())), ChType::String),
        _ => {
            return Err(ChError::unsupported(format!(
                "Comparing {target} with a string constant"
            )));
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_take_the_smallest_type() {
        assert_eq!(narrow_i128(1), ScalarValue::UInt8(Some(1)));
        assert_eq!(narrow_i128(256), ScalarValue::UInt16(Some(256)));
        assert_eq!(narrow_i128(-1), ScalarValue::Int8(Some(-1)));
        assert_eq!(narrow_i128(-129), ScalarValue::Int16(Some(-129)));
        assert_eq!(
            narrow_i128(9_223_372_036_854_775_807),
            ScalarValue::UInt64(Some(9_223_372_036_854_775_807))
        );
        assert_eq!(
            narrow_i128(-9_223_372_036_854_775_808),
            ScalarValue::Int64(Some(i64::MIN))
        );
    }

    #[test]
    fn strings_read_as_the_other_type() {
        let p = |s, t: &str| convert(s, &loams_house_types::parse(t).unwrap());
        assert_eq!(
            p("300", "UInt8").unwrap().unwrap().0,
            ScalarValue::UInt16(Some(300))
        );
        assert_eq!(p("x", "UInt8").unwrap_err().code, 53);
        assert_eq!(p("1.5", "UInt8").unwrap_err().code, 53);
        assert_eq!(p("2026-10-01 00:00:00", "Date").unwrap_err().code, 53);
        assert_eq!(
            p("2026-10-01", "Date").unwrap().unwrap().0,
            ScalarValue::Date32(Some(20_727))
        );
        assert_eq!(
            p("b", "Enum8('a' = 1, 'b' = 2)").unwrap().unwrap().0,
            ScalarValue::Int8(Some(2))
        );
        assert!(p("z", "Enum8('a' = 1)").unwrap().is_none());
        assert_eq!(
            p("1.2.3.4", "IPv4").unwrap().unwrap().0,
            ScalarValue::UInt32(Some(0x0102_0304))
        );
    }
}
