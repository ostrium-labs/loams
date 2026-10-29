//! Maths (plan HE1 Task 33): `round`, `roundBankers`, `floor`, `ceil`,
//! `trunc`, `negate`, `abs`, `bitAnd`, `bitOr`, `bitXor`, `bitNot`,
//! `bitShiftLeft`, `bitShiftRight`, `rand`, `rand64` and `randCanonical`,
//! from `src/Functions/{FunctionsRound.h, negate.cpp, abs.cpp, bitAnd.cpp,
//! …, FunctionsRandom.h}` (Apache-2.0). `intDiv` and `%` are Task 24's
//! (`ops/int_div.rs`); `ifNotFinite` is with the conditionals.
//!
//! * Rounding keeps the argument's type. Floats round half to even
//!   (`nearbyint`); integers and decimals round half away from zero, and
//!   `roundBankers` rounds every type half to even.
//! * `negate` widens unsigned integers to the next signed type (`UInt64`
//!   stays 64 bits), `abs` makes signed integers unsigned of the same size,
//!   and the bit functions take the wider size, signed if either is.
//! * `rand` values are compared with the oracle by type and range only.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use datafusion::arrow::array::{ArrayRef, Float64Array, UInt32Array, UInt64Array};
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};

use crate::func::convert::cast::{int_range, pow10, wrap_int};
use crate::func::convert::value::{Value, build, read};
use crate::func::typed::{
    ChFn, Nulls, arity, const_int, illegal_type, is_float, is_integer, kernel, not_constant, typer,
};

/// How a rounding function rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// `round`: half to even for floats, half away from zero otherwise.
    Round,
    /// `roundBankers`: half to even.
    Bankers,
    /// `floor`: towards negative infinity.
    Floor,
    /// `ceil`: towards positive infinity.
    Ceil,
    /// `trunc`: towards zero.
    Trunc,
}

fn float_op(x: f64, mode: Rounding) -> f64 {
    match mode {
        Rounding::Round | Rounding::Bankers => x.round_ties_even(),
        Rounding::Floor => x.floor(),
        Rounding::Ceil => x.ceil(),
        Rounding::Trunc => x.trunc(),
    }
}

fn float32_op(x: f32, mode: Rounding) -> f32 {
    match mode {
        Rounding::Round | Rounding::Bankers => x.round_ties_even(),
        Rounding::Floor => x.floor(),
        Rounding::Ceil => x.ceil(),
        Rounding::Trunc => x.trunc(),
    }
}

/// `x` rounded to `n` decimal places (`n < 0`: to tens, hundreds, …).
pub fn round_f64(x: f64, n: i64, mode: Rounding) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if n == 0 {
        return float_op(x, mode);
    }
    let scale = 10f64.powi(n.unsigned_abs().min(400) as i32);
    if n > 0 {
        let scaled = x * scale;
        if !scaled.is_finite() {
            return x;
        }
        float_op(scaled, mode) / scale
    } else {
        if !scale.is_finite() {
            return 0.0;
        }
        float_op(x / scale, mode) * scale
    }
}

/// [`round_f64`] in `Float32` arithmetic, as ClickHouse computes a
/// `Float32` column.
pub fn round_f32(x: f32, n: i64, mode: Rounding) -> f32 {
    if !x.is_finite() {
        return x;
    }
    if n == 0 {
        return float32_op(x, mode);
    }
    let scale = 10f32.powi(n.unsigned_abs().min(60) as i32);
    if n > 0 {
        let scaled = x * scale;
        if !scaled.is_finite() {
            return x;
        }
        float32_op(scaled, mode) / scale
    } else {
        if !scale.is_finite() {
            return 0.0;
        }
        float32_op(x / scale, mode) * scale
    }
}

/// `x` rounded to a multiple of `scale` (a power of ten).
pub fn round_int(x: i128, scale: i128, mode: Rounding) -> i128 {
    if scale <= 1 {
        return x;
    }
    let q = x / scale;
    let rem = x % scale;
    let q = match mode {
        Rounding::Trunc => q,
        Rounding::Floor => x.div_euclid(scale),
        Rounding::Ceil => {
            if rem > 0 {
                q + 1
            } else {
                q
            }
        }
        Rounding::Round | Rounding::Bankers => {
            let twice = rem.abs() * 2;
            let away = twice > scale || (twice == scale && (mode == Rounding::Round || q % 2 != 0));
            if away { q + x.signum() } else { q }
        }
    };
    q.saturating_mul(scale)
}

fn round_value(v: Value, t: &ChType, n: i64, mode: Rounding) -> Value {
    match v {
        Value::Float(f) => {
            if matches!(t, ChType::Float32) {
                Value::Float(f64::from(round_f32(f as f32, n, mode)))
            } else {
                Value::Float(round_f64(f, n, mode))
            }
        }
        Value::Int(i) if n < 0 => {
            let scale = u32::try_from(n.unsigned_abs())
                .ok()
                .and_then(pow10)
                .unwrap_or(0);
            if scale == 0 {
                Value::Int(0)
            } else {
                Value::Int(wrap_int(round_int(i, scale, mode), t))
            }
        }
        Value::Dec(u, s) if n < i64::from(s) => {
            let digits = i64::from(s) - n;
            match u32::try_from(digits).ok().and_then(pow10) {
                Some(scale) => Value::Dec(round_int(u, scale, mode), s),
                None => Value::Dec(0, s),
            }
        }
        other => other,
    }
}

fn number_types(t: &ChType) -> bool {
    is_integer(t) || is_float(t) || matches!(t, ChType::Decimal { .. })
}

/// `NumberTraits::Construct<signed, false, size>` for integers.
fn int_type(signed: bool, bytes: u32) -> ChType {
    match (signed, bytes) {
        (false, 1) => ChType::UInt8,
        (false, 2) => ChType::UInt16,
        (false, 4) => ChType::UInt32,
        (false, _) => ChType::UInt64,
        (true, 1) => ChType::Int8,
        (true, 2) => ChType::Int16,
        (true, 4) => ChType::Int32,
        (true, _) => ChType::Int64,
    }
}

/// Whether an integer type (or `Bool`) is signed, and its size in bytes.
fn int_shape(t: &ChType) -> Option<(bool, u32)> {
    Some(match t {
        ChType::UInt8 | ChType::Bool => (false, 1),
        ChType::UInt16 => (false, 2),
        ChType::UInt32 => (false, 4),
        ChType::UInt64 => (false, 8),
        ChType::Int8 => (true, 1),
        ChType::Int16 => (true, 2),
        ChType::Int32 => (true, 4),
        ChType::Int64 => (true, 8),
        _ => return None,
    })
}

fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(i128::from(*b)),
        _ => None,
    }
}

/// A unary function over one value, typed by `ret`.
fn unary(
    name: &str,
    ret: impl Fn(&ChType) -> Option<ChType> + Send + Sync + 'static,
    f: impl Fn(Value, &ChType) -> Value + Send + Sync + 'static,
) -> ChFn {
    ChFn::new(
        name,
        Nulls::Default,
        typer(move |s| {
            arity(s, 1, 1)?;
            ret(&s.types[0]).ok_or_else(|| illegal_type(s.name, 0, &s.types[0]))
        }),
        kernel(move |c| {
            let mut out = Vec::with_capacity(c.rows);
            for row in 0..c.rows {
                let v = read(c.args[0].as_ref(), &c.types[0], row)?;
                out.push(if v.is_null() { v } else { f(v, c.ret) });
            }
            build(&crate::func::cond::plain(c.ret), &out)
        }),
    )
}

/// The process's random stream: SplitMix64 over a seed from the clock and a
/// counter (R33.8: ClickHouse's own generator is not reproduced; the oracle
/// compares `rand` by type and range).
fn next_random() -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut z = STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    if z == 0 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5EED);
        let _ = STATE.compare_exchange(
            0x9E37_79B9_7F4A_7C15,
            seed | 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        z = seed;
    }
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Registers the maths functions.
pub fn register(r: &mut Registry) {
    for (name, mode) in [
        ("round", Rounding::Round),
        ("roundBankers", Rounding::Bankers),
        ("floor", Rounding::Floor),
        ("ceil", Rounding::Ceil),
        ("trunc", Rounding::Trunc),
    ] {
        ChFn::new(
            name,
            Nulls::Default,
            typer(|s| {
                arity(s, 1, 2)?;
                if !number_types(&s.types[0]) {
                    return Err(illegal_type(s.name, 0, &s.types[0]));
                }
                if s.types.len() == 2 {
                    if !is_integer(&s.types[1]) {
                        return Err(illegal_type(s.name, 1, &s.types[1]));
                    }
                    if const_int(s.consts.get(1).and_then(Option::as_ref)).is_none() {
                        return Err(not_constant(s.name, 1));
                    }
                }
                Ok(s.types[0].clone())
            }),
            kernel(move |c| {
                let n = c
                    .consts
                    .get(1)
                    .and_then(Option::as_ref)
                    .and_then(|v| const_int(Some(v)))
                    .unwrap_or(0)
                    .clamp(-1000, 1000) as i64;
                let mut out = Vec::with_capacity(c.rows);
                for row in 0..c.rows {
                    let v = read(c.args[0].as_ref(), &c.types[0], row)?;
                    out.push(round_value(v, &c.types[0], n, mode));
                }
                build(c.ret, &out)
            }),
        )
        .register(r);
    }

    unary(
        "negate",
        |t| match int_shape(t) {
            Some((true, _)) => Some(t.clone()),
            Some((false, bytes)) => Some(int_type(true, (bytes * 2).min(8))),
            None if is_float(t) || matches!(t, ChType::Decimal { .. }) => Some(t.clone()),
            None => None,
        },
        |v, ret| match v {
            Value::Int(i) => Value::Int(wrap_int(i.wrapping_neg(), ret)),
            Value::Bool(b) => Value::Int(-i128::from(b)),
            Value::Float(f) => Value::Float(-f),
            Value::Dec(u, s) => Value::Dec(-u, s),
            other => other,
        },
    )
    .register(r);

    unary(
        "abs",
        |t| match int_shape(t) {
            Some((_, bytes)) => Some(int_type(false, bytes)),
            None if is_float(t) || matches!(t, ChType::Decimal { .. }) => Some(t.clone()),
            None => None,
        },
        |v, ret| match v {
            Value::Int(i) => Value::Int(wrap_int(i.abs(), ret)),
            Value::Bool(b) => Value::Int(i128::from(b)),
            Value::Float(f) => Value::Float(f.abs()),
            Value::Dec(u, s) => Value::Dec(u.abs(), s),
            other => other,
        },
    )
    .register(r);

    unary(
        "bitNot",
        |t| int_shape(t).map(|(signed, bytes)| int_type(signed, bytes)),
        |v, ret| match int_of(&v) {
            Some(i) => Value::Int(wrap_int(!i, ret)),
            None => v,
        },
    )
    .register(r);

    type BitOp = fn(i128, i128) -> i128;
    let ops: [(&str, BitOp); 3] = [
        ("bitAnd", |a: i128, b: i128| a & b),
        ("bitOr", |a: i128, b: i128| a | b),
        ("bitXor", |a: i128, b: i128| a ^ b),
    ];
    for (name, op) in ops {
        ChFn::new(
            name,
            Nulls::Default,
            typer(|s| {
                arity(s, 2, 2)?;
                let (sa, ba) =
                    int_shape(&s.types[0]).ok_or_else(|| illegal_type(s.name, 0, &s.types[0]))?;
                let (sb, bb) =
                    int_shape(&s.types[1]).ok_or_else(|| illegal_type(s.name, 1, &s.types[1]))?;
                Ok(int_type(sa || sb, ba.max(bb)))
            }),
            kernel(move |c| {
                let mut out = Vec::with_capacity(c.rows);
                for row in 0..c.rows {
                    let a = read(c.args[0].as_ref(), &c.types[0], row)?;
                    let b = read(c.args[1].as_ref(), &c.types[1], row)?;
                    out.push(match (int_of(&a), int_of(&b)) {
                        (Some(a), Some(b)) => Value::Int(wrap_int(op(a, b), c.ret)),
                        _ => Value::Int(0),
                    });
                }
                build(c.ret, &out)
            }),
        )
        .register(r);
    }

    for (name, left) in [("bitShiftLeft", true), ("bitShiftRight", false)] {
        ChFn::new(
            name,
            Nulls::Default,
            typer(|s| {
                arity(s, 2, 2)?;
                let (signed, bytes) =
                    int_shape(&s.types[0]).ok_or_else(|| illegal_type(s.name, 0, &s.types[0]))?;
                if int_shape(&s.types[1]).is_none() {
                    return Err(illegal_type(s.name, 1, &s.types[1]));
                }
                Ok(int_type(signed, bytes))
            }),
            kernel(move |c| {
                let bits = int_range(c.ret)
                    .map(|(min, max)| (max - min + 1).ilog2() as i128)
                    .unwrap_or(64);
                let mut out = Vec::with_capacity(c.rows);
                for row in 0..c.rows {
                    let a = int_of(&read(c.args[0].as_ref(), &c.types[0], row)?).unwrap_or(0);
                    let b = int_of(&read(c.args[1].as_ref(), &c.types[1], row)?).unwrap_or(0);
                    if b < 0 {
                        return Err(ChError::argument_out_of_bound(format!(
                            "The number of shift positions needs to be a non-negative value, got {b}"
                        )));
                    }
                    let v = if b >= bits {
                        if left || a >= 0 { 0 } else { -1 }
                    } else if left {
                        a << b
                    } else {
                        a >> b
                    };
                    out.push(Value::Int(wrap_int(v, c.ret)));
                }
                build(c.ret, &out)
            }),
        )
        .register(r);
    }

    // `rand([x])`, `rand64([x])`, `randCanonical([x])`: the argument only
    // tells calls apart.
    ChFn::new(
        "rand",
        Nulls::Own,
        typer(|s| {
            arity(s, 0, 1)?;
            Ok(ChType::UInt32)
        }),
        kernel(|c| {
            let values: Vec<u32> = (0..c.rows).map(|_| (next_random() >> 32) as u32).collect();
            Ok(Arc::new(UInt32Array::from(values)) as ArrayRef)
        }),
    )
    .volatile()
    .register(r);
    ChFn::new(
        "rand64",
        Nulls::Own,
        typer(|s| {
            arity(s, 0, 1)?;
            Ok(ChType::UInt64)
        }),
        kernel(|c| {
            let values: Vec<u64> = (0..c.rows).map(|_| next_random()).collect();
            Ok(Arc::new(UInt64Array::from(values)) as ArrayRef)
        }),
    )
    .volatile()
    .register(r);
    ChFn::new(
        "randCanonical",
        Nulls::Own,
        typer(|s| {
            arity(s, 0, 1)?;
            Ok(ChType::Float64)
        }),
        kernel(|c| {
            let values: Vec<f64> = (0..c.rows)
                .map(|_| (next_random() >> 11) as f64 / (1u64 << 53) as f64)
                .collect();
            Ok(Arc::new(Float64Array::from(values)) as ArrayRef)
        }),
    )
    .volatile()
    .register(r);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn floats_round_half_to_even() {
        assert_eq!(round_f64(2.5, 0, Rounding::Round), 2.0);
        assert_eq!(round_f64(3.5, 0, Rounding::Round), 4.0);
        assert_eq!(round_f64(1.2345, 2, Rounding::Round), 1.23);
        assert_eq!(round_f64(1234.0, -2, Rounding::Round), 1200.0);
        assert_eq!(round_f64(-1.5, 0, Rounding::Floor), -2.0);
        assert_eq!(round_f64(-1.5, 0, Rounding::Ceil), -1.0);
        assert_eq!(round_f64(1e300, 400, Rounding::Round), 1e300);
        assert!(round_f64(f64::NAN, 1, Rounding::Round).is_nan());
    }

    #[test]
    fn integers_round_half_away_from_zero() {
        assert_eq!(round_int(15, 10, Rounding::Round), 20);
        assert_eq!(round_int(25, 10, Rounding::Round), 30);
        assert_eq!(round_int(-15, 10, Rounding::Round), -20);
        assert_eq!(round_int(25, 10, Rounding::Bankers), 20);
        assert_eq!(round_int(35, 10, Rounding::Bankers), 40);
        assert_eq!(round_int(-15, 10, Rounding::Floor), -20);
        assert_eq!(round_int(11, 10, Rounding::Ceil), 20);
        assert_eq!(round_int(-19, 10, Rounding::Trunc), -10);
        assert_eq!(
            round_value(
                Value::Dec(12345, 3),
                &ChType::Decimal {
                    precision: 9,
                    scale: 3
                },
                2,
                Rounding::Round
            ),
            Value::Dec(12350, 3)
        );
        assert_eq!(
            round_value(Value::Int(1234), &ChType::UInt16, -2, Rounding::Round),
            Value::Int(1200)
        );
    }

    #[test]
    fn random_values_vary() {
        let a = next_random();
        let b = next_random();
        assert_ne!(a, b);
        assert_eq!(int_type(true, 2), ChType::Int16);
        assert_eq!(int_shape(&ChType::Bool), Some((false, 1)));
    }
}
