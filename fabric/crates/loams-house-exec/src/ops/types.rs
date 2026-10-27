//! ClickHouse numeric promotion and least-supertype rules.
//! Derived from ClickHouse v26.9.2.8-stable src/DataTypes/{NumberTraits.h,
//! getLeastSupertype.cpp}, Apache-2.0; see the repository NOTICE.
use super::Op;
use loams_house_types::{ChError, ChType};

pub(crate) fn error(code: i32, name: &'static str, message: impl Into<String>) -> ChError {
    ChError {
        code,
        name,
        message: message.into(),
    }
}
/// Peel representation wrappers; functions generally remove LowCardinality.
pub fn base(t: &ChType) -> &ChType {
    match t {
        ChType::Nullable(t) | ChType::LowCardinality(t) => base(t),
        ChType::SimpleAggregateFunction(s) => base(&s.args[0]),
        _ => t,
    }
}
pub(crate) fn nullable(t: &ChType) -> bool {
    match t {
        ChType::Nullable(_) | ChType::Nothing => true,
        ChType::LowCardinality(t) => nullable(t),
        _ => false,
    }
}
pub(crate) fn wrap(t: ChType, null: bool) -> ChType {
    if null {
        loams_house_types::arrow::nullable(t)
    } else {
        t
    }
}
/// Signedness, bit width and whether the number is floating point.
pub fn number(t: &ChType) -> Option<(bool, u8, bool)> {
    use ChType::*;
    Some(match base(t) {
        Bool | UInt8 => (false, 8, false),
        UInt16 => (false, 16, false),
        UInt32 | IPv4 => (false, 32, false),
        UInt64 => (false, 64, false),
        Int8 | Enum8(_) => (true, 8, false),
        Int16 | Enum16(_) => (true, 16, false),
        Int32 => (true, 32, false),
        Int64 => (true, 64, false),
        Float32 => (true, 32, true),
        Float64 => (true, 64, true),
        _ => return None,
    })
}
pub(crate) fn integer(signed: bool, bits: u8) -> ChType {
    use ChType::*;
    match (signed, bits) {
        (false, 8) => UInt8,
        (false, 16) => UInt16,
        (false, 32) => UInt32,
        (false, _) => UInt64,
        (true, 8) => Int8,
        (true, 16) => Int16,
        (true, 32) => Int32,
        (true, _) => Int64,
    }
}
fn family(p: u8) -> u8 {
    if p <= 9 {
        9
    } else if p <= 18 {
        18
    } else if p <= 38 {
        38
    } else {
        76
    }
}
/// Least type representing every input without loss; unsupported wide/Variant results are 48.
pub fn least(types: &[ChType]) -> Result<ChType, ChError> {
    use ChType::*;
    if types.is_empty() {
        return Ok(Nullable(Box::new(Nothing)));
    }
    let null = types.iter().any(nullable);
    let values: Vec<_> = types
        .iter()
        .map(base)
        .filter(|t| !matches!(t, Nothing))
        .collect();
    if values.is_empty() {
        return Ok(Nullable(Box::new(Nothing)));
    }
    let first = values[0];
    if values.iter().all(|t| *t == first) {
        return Ok(wrap(first.clone(), null));
    }
    if values.iter().all(|t| matches!(t, String | FixedString(_))) {
        return Ok(wrap(String, null));
    }
    if values.iter().all(|t| matches!(t, Array(_))) {
        let inner = values
            .iter()
            .map(|t| match t {
                Array(t) => t.as_ref().clone(),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        return Ok(Array(Box::new(least(&inner)?)));
    }
    if let Tuple(fields) = first {
        if values
            .iter()
            .all(|t| matches!(t,Tuple(f) if f.len()==fields.len()))
        {
            let mut result = Vec::new();
            for (i, (name, _)) in fields.iter().enumerate() {
                let members = values
                    .iter()
                    .map(|t| match t {
                        Tuple(f) => f[i].1.clone(),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>();
                result.push((name.clone(), least(&members)?));
            }
            return Ok(Tuple(result));
        }
    }
    if let Map(_, _) = first {
        if values.iter().all(|t| matches!(t, Map(..))) {
            let keys = values
                .iter()
                .map(|t| match t {
                    Map(k, _) => k.as_ref().clone(),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            let vals = values
                .iter()
                .map(|t| match t {
                    Map(_, v) => v.as_ref().clone(),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            return Ok(Map(Box::new(least(&keys)?), Box::new(least(&vals)?)));
        }
    }
    if values.iter().any(|t| matches!(t, Decimal { .. })) {
        let mut scale = 0;
        let mut precision = 0;
        for t in &values {
            if let Decimal {
                precision: p,
                scale: s,
            } = t
            {
                scale = scale.max(*s);
                precision = precision.max(family(*p));
            }
        }
        for t in &values {
            if let Some((_, bits, false)) = number(t) {
                precision = precision.max(
                    scale
                        + match bits {
                            8 => 3,
                            16 => 5,
                            32 => 10,
                            _ => 20,
                        },
                );
            } else if !matches!(t, Decimal { .. }) {
                return Err(error(
                    386,
                    "NO_COMMON_TYPE",
                    "Decimal and non-integer types have no common type",
                ));
            }
        }
        if precision > 76 {
            return Err(ChError::unsupported("Decimal precision above 76"));
        }
        return Ok(wrap(
            Decimal {
                precision: family(precision),
                scale,
            },
            null,
        ));
    }
    if values.iter().all(|t| number(t).is_some()) {
        let (mut signed, mut unsigned, mut mantissa) = (0, 0, 0);
        for t in &values {
            let (s, b, f) = number(t).expect("number");
            if f {
                mantissa = mantissa.max(if b == 32 { 24 } else { 53 });
            } else if s {
                signed = signed.max(b);
            } else {
                unsigned = unsigned.max(b);
            }
        }
        let bits = if signed > 0 && unsigned >= signed {
            unsigned + 1
        } else {
            signed.max(unsigned)
        };
        if bits > 64 || mantissa > 0 && bits > 53 {
            return Err(ChError::unsupported(
                "A wide integer or Variant least supertype",
            ));
        }
        let t = if mantissa > 0 {
            if bits.max(mantissa) <= 24 {
                Float32
            } else {
                Float64
            }
        } else {
            integer(
                signed > 0,
                [8, 16, 32, 64]
                    .into_iter()
                    .find(|b| *b >= bits)
                    .unwrap_or(64),
            )
        };
        return Ok(wrap(t, null));
    }
    if values
        .iter()
        .all(|t| matches!(t, Date | Date32 | DateTime(_) | DateTime64(..)))
    {
        let mut precision = None;
        let mut zone = None;
        let mut datetime = false;
        let mut wide = false;
        for t in &values {
            match t {
                Date32 => wide = true,
                DateTime(z) => {
                    datetime = true;
                    if zone.is_none() {
                        zone = z.clone();
                    }
                }
                DateTime64(p, z) => {
                    datetime = true;
                    precision = Some(precision.unwrap_or(0).max(*p));
                    if zone.is_none() {
                        zone = z.clone();
                    }
                }
                _ => {}
            }
        }
        let t = if let Some(p) = precision {
            DateTime64(p, zone)
        } else if datetime && wide {
            DateTime64(0, zone)
        } else if datetime {
            DateTime(zone)
        } else {
            Date32
        };
        return Ok(wrap(t, null));
    }
    Err(error(
        386,
        "NO_COMMON_TYPE",
        format!("There is no supertype for {types:?}"),
    ))
}
/// Arithmetic's promotion differs from least-supertype (notably UInt64 + Int64).
pub fn arithmetic(op: Op, types: &[ChType]) -> Result<ChType, ChError> {
    use ChType::*;
    let null = types.iter().any(nullable);
    if types.iter().any(|t| matches!(base(t), Nothing)) {
        return Ok(Nullable(Box::new(Nothing)));
    }
    let a = base(&types[0]);
    if op == Op::Negate {
        if matches!(a, Decimal { .. }) {
            return Ok(wrap(a.clone(), null));
        }
        let (s, b, f) = number(a)
            .ok_or_else(|| ChError::illegal_type_of_argument("negate requires a number"))?;
        return Ok(wrap(
            if f {
                a.clone()
            } else {
                integer(true, if s { b } else { (b * 2).min(64) })
            },
            null,
        ));
    }
    let b = base(&types[1]);
    if matches!(a, Decimal { .. }) || matches!(b, Decimal { .. }) {
        if number(a).is_some_and(|(_, _, f)| f) || number(b).is_some_and(|(_, _, f)| f) {
            return Err(ChError::illegal_type_of_argument(
                "Decimal arithmetic with floats is not supported",
            ));
        }
        let (p1, s1) = match a {
            Decimal { precision, scale } => (*precision, *scale),
            _ if number(a).is_some() => (0, 0),
            _ => {
                return Err(ChError::illegal_type_of_argument(
                    "Decimal arithmetic requires numbers",
                ));
            }
        };
        let (p2, s2) = match b {
            Decimal { precision, scale } => (*precision, *scale),
            _ if number(b).is_some() => (0, 0),
            _ => {
                return Err(ChError::illegal_type_of_argument(
                    "Decimal arithmetic requires numbers",
                ));
            }
        };
        let precision = family(p1.max(p2));
        let scale = match op {
            Op::Add | Op::Subtract => s1.max(s2),
            Op::Multiply => s1 + s2,
            Op::Divide | Op::Modulo => {
                if s1 < s2 {
                    return Err(ChError::argument_out_of_bound(
                        "Decimal result's scale is less than argument's one",
                    ));
                }
                s1
            }
            Op::IntDiv => return Ok(wrap(Int64, null)),
            _ => unreachable!(),
        };
        if scale > precision {
            return Err(ChError::argument_out_of_bound(
                "Decimal result's scale exceeds precision",
            ));
        }
        return Ok(wrap(Decimal { precision, scale }, null));
    }
    let (sa, ba, fa) =
        number(a).ok_or_else(|| ChError::illegal_type_of_argument(format!("Arithmetic on {a}")))?;
    let (sb, bb, fb) =
        number(b).ok_or_else(|| ChError::illegal_type_of_argument(format!("Arithmetic on {b}")))?;
    let t = match op {
        Op::Divide => Float64,
        Op::IntDiv => integer(sa || sb, ba),
        Op::Modulo if fa || fb => Float64,
        Op::Modulo => integer(sa, if sa { (bb * 2).min(64) } else { bb }),
        _ if fa || fb => Float64,
        _ => integer(op == Op::Subtract || sa || sb, (ba.max(bb) * 2).min(64)),
    };
    Ok(wrap(t, null))
}
