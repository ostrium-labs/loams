//! ClickHouse's hash functions on the Rust engine (plan HE1 Task 31):
//! `cityHash64`, `sipHash64`, `xxHash32`, `xxHash64` over any arguments, and
//! `intHash32`, `intHash64` over integers, bit-exact against the oracle.
//!
//! The variadic ones follow ClickHouse's `FunctionAnyHash`
//! (`src/Functions/FunctionsHashing.h`, Apache-2.0; see `NOTICE`): each
//! argument is hashed by its ClickHouse type and the hashes are combined
//! left to right with the function's `combineHashes`.
//!
//! * a number, date, time, `Bool`, `Enum` or small `Decimal` is hashed from
//!   its bytes in memory (little-endian, its ClickHouse width), except that
//!   `cityHash64` hashes them with `intHash64` of the value's bits widened
//!   with zeros (`use_int_hash_for_pods`);
//! * `String` and `FixedString` hash their bytes; `UUID` and 128-bit values
//!   their 16 bytes in memory (low half first);
//! * an array hashes its length with `intHash64` (`intHash32` for 32-bit
//!   hashes), then combines each element's hash; a map is an array of
//!   (key, value) tuples; a tuple's elements count as separate arguments;
//! * no arguments at all hash to `0xe28dbde7fe22e41c`.
//!
//! The argument's type comes from the column's `loams.ch.type`, so the
//! answer depends on the ClickHouse type, as it does in ClickHouse: a
//! literal's type follows the engine's literal rules (plan HE1 Task 24).

pub mod city;

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, PrimitiveArray, UInt32Builder, UInt64Builder,
};
use datafusion::arrow::datatypes::{self as adt, ArrowPrimitiveType, DataType};
use loams_house_sql::Registry;
use loams_house_types::arrow::ticks_per_unit;
use loams_house_types::{ChError, ChType};

use crate::func::chudf::{ChFunction, check_arity, illegal_type, list_parts};

/// The hash of no arguments.
const EMPTY: u64 = 0xe28d_bde7_fe22_e41c;

/// The canonical names of the functions this module registers.
pub const NAMES: &[&str] = &[
    "cityHash64",
    "sipHash64",
    "xxHash32",
    "xxHash64",
    "intHash32",
    "intHash64",
];

/// `intHash64` (`IntHash64Impl`: Murmur3's finaliser over the value XOR a
/// fixed salt).
pub fn int_hash64(x: u64) -> u64 {
    let mut x = x ^ 0x4cf2_d2ba_ae6d_a887;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    x
}

/// `intHash32` (`IntHash32Impl`: Thomas Wang's 64-to-32-bit hash over the
/// value XOR a fixed salt).
pub fn int_hash32(x: u64) -> u32 {
    let mut k = x ^ 0x75d9_543d_e018_bf45;
    k = (!k).wrapping_add(k << 18);
    k ^= k.rotate_right(31);
    k = k.wrapping_mul(21);
    k ^= k.rotate_right(11);
    k = k.wrapping_add(k << 6);
    k ^= k.rotate_right(22);
    k as u32
}

/// A hash function of the `FunctionAnyHash` family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo {
    /// `cityHash64`.
    City64,
    /// `sipHash64`.
    Sip64,
    /// `xxHash32`.
    Xx32,
    /// `xxHash64`.
    Xx64,
}

impl Algo {
    fn wide(self) -> bool {
        self != Algo::Xx32
    }

    /// The hash of `bytes`.
    pub fn bytes(self, b: &[u8]) -> u64 {
        match self {
            Algo::City64 => city::city_hash64(b),
            Algo::Sip64 => siphasher::sip::SipHasher24::new_with_keys(0, 0).hash(b),
            Algo::Xx32 => u64::from(twox_hash::XxHash32::oneshot(0, b)),
            Algo::Xx64 => twox_hash::XxHash64::oneshot(0, b),
        }
    }

    /// The function's `combineHashes`.
    pub fn combine(self, h1: u64, h2: u64) -> u64 {
        match self {
            Algo::City64 | Algo::Xx64 => city::hash128_to_64(h1, h2),
            Algo::Sip64 => {
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&h1.to_le_bytes());
                b[8..].copy_from_slice(&h2.to_le_bytes());
                self.bytes(&b)
            }
            Algo::Xx32 => u64::from(int_hash32(h1) ^ (h2 as u32)),
        }
    }

    /// The hash of an array's length.
    fn size(self, n: usize) -> u64 {
        if self.wide() {
            int_hash64(n as u64)
        } else {
            u64::from(int_hash32(n as u64))
        }
    }

    /// The hash of a fixed-width value whose little-endian bytes are `le`.
    fn pod(self, le: &[u8]) -> u64 {
        if self == Algo::City64 && le.len() <= 8 {
            let mut b = [0u8; 8];
            b[..le.len()].copy_from_slice(le);
            int_hash64(u64::from_le_bytes(b))
        } else {
            self.bytes(le)
        }
    }
}

fn not_supported(t: &ChType) -> ChError {
    ChError::unsupported(format!("Hashing a value of type {t}"))
}

fn null_inside(t: &ChType) -> ChError {
    ChError::unsupported(format!(
        "Hashing a NULL inside a composite value of type {t}"
    ))
}

/// Folds `h` into `out[row]`.
fn fold(algo: Algo, out: &mut [u64], row: usize, h: u64, first: bool) {
    out[row] = if first { h } else { algo.combine(out[row], h) };
}

fn prim<T: ArrowPrimitiveType>(a: &dyn Array) -> &PrimitiveArray<T> {
    a.as_primitive::<T>()
}

/// The little-endian bytes of the ClickHouse value of `t` at `row` in `a`
/// (a fixed-width type), or `None` when `t` is not one.
fn pod_bytes(t: &ChType, a: &dyn Array, row: usize) -> Option<Vec<u8>> {
    Some(match (t, a.data_type()) {
        (_, DataType::Boolean) => {
            let b: &BooleanArray = a.as_boolean();
            vec![u8::from(b.value(row))]
        }
        (ChType::Date, DataType::Date32) => (prim::<adt::Date32Type>(a).value(row) as u16)
            .to_le_bytes()
            .to_vec(),
        (ChType::Date32, DataType::Date32) => {
            prim::<adt::Date32Type>(a).value(row).to_le_bytes().to_vec()
        }
        (ChType::DateTime(_), DataType::Timestamp(_, _)) => {
            let secs = timestamp(a, row)? / ticks_in(a.data_type(), 0);
            (secs as u32).to_le_bytes().to_vec()
        }
        (ChType::DateTime64(p, _), DataType::Timestamp(_, _)) => {
            let ticks = timestamp(a, row)? / ticks_per_unit(*p);
            ticks.to_le_bytes().to_vec()
        }
        (ChType::Decimal { precision, .. }, DataType::Decimal128(..)) => {
            let v = prim::<adt::Decimal128Type>(a).value(row);
            match precision {
                0..=9 => (v as i32).to_le_bytes().to_vec(),
                10..=18 => (v as i64).to_le_bytes().to_vec(),
                _ => v.to_le_bytes().to_vec(),
            }
        }
        (_, DataType::Int8) => prim::<adt::Int8Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::Int16) => prim::<adt::Int16Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::Int32) => prim::<adt::Int32Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::Int64) => prim::<adt::Int64Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::UInt8) => prim::<adt::UInt8Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::UInt16) => prim::<adt::UInt16Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::UInt32) => prim::<adt::UInt32Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::UInt64) => prim::<adt::UInt64Type>(a).value(row).to_le_bytes().to_vec(),
        (_, DataType::Float32) => prim::<adt::Float32Type>(a)
            .value(row)
            .to_le_bytes()
            .to_vec(),
        (_, DataType::Float64) => prim::<adt::Float64Type>(a)
            .value(row)
            .to_le_bytes()
            .to_vec(),
        _ => return None,
    })
}

fn timestamp(a: &dyn Array, row: usize) -> Option<i64> {
    Some(match a.data_type() {
        DataType::Timestamp(adt::TimeUnit::Second, _) => {
            prim::<adt::TimestampSecondType>(a).value(row)
        }
        DataType::Timestamp(adt::TimeUnit::Millisecond, _) => {
            prim::<adt::TimestampMillisecondType>(a).value(row)
        }
        DataType::Timestamp(adt::TimeUnit::Microsecond, _) => {
            prim::<adt::TimestampMicrosecondType>(a).value(row)
        }
        DataType::Timestamp(adt::TimeUnit::Nanosecond, _) => {
            prim::<adt::TimestampNanosecondType>(a).value(row)
        }
        _ => return None,
    })
}

/// How many of the array's ticks make one tick of `DateTime` (`p` = 0).
fn ticks_in(dt: &DataType, _p: u8) -> i64 {
    match dt {
        DataType::Timestamp(adt::TimeUnit::Millisecond, _) => 1_000,
        DataType::Timestamp(adt::TimeUnit::Microsecond, _) => 1_000_000,
        DataType::Timestamp(adt::TimeUnit::Nanosecond, _) => 1_000_000_000,
        _ => 1,
    }
}

/// Hashes the column `a` of ClickHouse type `t` into `out`, combining with
/// what is there unless `*first`; tuples hash element by element.
fn hash_into(
    algo: Algo,
    t: &ChType,
    a: &ArrayRef,
    out: &mut [u64],
    first: &mut bool,
    top: bool,
) -> Result<(), ChError> {
    // Dictionaries (LowCardinality) hash as their values.
    if let DataType::Dictionary(_, values) = a.data_type() {
        let plain = datafusion::arrow::compute::cast(a, values)
            .map_err(|e| ChError::logical_error(e.to_string()))?;
        return hash_into(algo, t, &plain, out, first, top);
    }
    if let ChType::Nullable(inner) | ChType::LowCardinality(inner) = t {
        // NULLs on the top level never get here (the default implementation
        // answers NULL on those rows); inside a composite, ClickHouse's
        // answer is not measured yet, so they are refused.
        if !top && a.null_count() > 0 {
            return Err(null_inside(t));
        }
        return hash_into(algo, inner, a, out, first, top);
    }
    let rows = out.len();
    match t {
        ChType::Tuple(elements) => {
            let s = a.as_struct();
            for (i, (_, et)) in elements.iter().enumerate() {
                hash_into(algo, et, s.column(i), out, first, top)?;
            }
            return Ok(());
        }
        ChType::Array(_) | ChType::Map(..) => {
            let inner = match t {
                ChType::Array(e) => e.as_ref().clone(),
                ChType::Map(k, v) => {
                    ChType::Tuple(vec![(None, (**k).clone()), (None, (**v).clone())])
                }
                _ => unreachable!("matched above"),
            };
            let (offsets, values) = list_parts(a.as_ref()).ok_or_else(|| not_supported(t))?;
            let mut temp = vec![0u64; values.len()];
            hash_into(algo, &inner, &values, &mut temp, &mut true, false)?;
            for row in 0..rows {
                let (s, e) = (offsets[row], offsets[row + 1]);
                fold(algo, out, row, algo.size(e - s), *first);
                for h in &temp[s..e] {
                    out[row] = algo.combine(out[row], *h);
                }
            }
        }
        ChType::String => {
            let s = a.as_string::<i32>();
            for row in 0..rows {
                fold(algo, out, row, algo.bytes(s.value(row).as_bytes()), *first);
            }
        }
        ChType::FixedString(_) => {
            let s = a.as_fixed_size_binary();
            for row in 0..rows {
                fold(algo, out, row, algo.bytes(s.value(row)), *first);
            }
        }
        ChType::Uuid => {
            let s = a.as_fixed_size_binary();
            for row in 0..rows {
                let v = s.value(row);
                let mut b = [0u8; 16];
                for i in 0..8 {
                    b[i] = v[15 - i];
                    b[8 + i] = v[7 - i];
                }
                fold(algo, out, row, algo.bytes(&b), *first);
            }
        }
        ChType::IPv6
        | ChType::Json(_)
        | ChType::Nothing
        | ChType::AggregateFunction(_)
        | ChType::Interval(_) => return Err(not_supported(t)),
        _ => {
            for row in 0..rows {
                let b = pod_bytes(t, a.as_ref(), row).ok_or_else(|| not_supported(t))?;
                fold(algo, out, row, algo.pod(&b), *first);
            }
        }
    }
    *first = false;
    Ok(())
}

fn any_hash(name: &'static str, algo: Algo) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |_, _| {
            Ok(if algo.wide() {
                ChType::UInt64
            } else {
                ChType::UInt32
            })
        }),
        Arc::new(move |call| {
            let mut out = vec![EMPTY; call.rows];
            let mut first = true;
            let mut nulls = vec![false; call.rows];
            for i in 0..call.args.len() {
                let a = call.array(i)?;
                for (row, n) in nulls.iter_mut().enumerate() {
                    *n |= a.is_null(row);
                }
                if a.null_count() > 0 {
                    // The default implementation answers NULL on these rows;
                    // hash the rest with the nulls' slots filled.
                    let filled = fill_nulls(&a)?;
                    hash_into(algo, &call.types[i], &filled, &mut out, &mut first, true)?;
                } else {
                    hash_into(algo, &call.types[i], &a, &mut out, &mut first, true)?;
                }
            }
            let any_null = nulls.iter().any(|n| *n);
            Ok(if algo.wide() {
                let mut b = UInt64Builder::with_capacity(call.rows);
                for (row, h) in out.iter().enumerate() {
                    if any_null && nulls[row] {
                        b.append_null();
                    } else {
                        b.append_value(*h);
                    }
                }
                Arc::new(b.finish()) as ArrayRef
            } else {
                let mut b = UInt32Builder::with_capacity(call.rows);
                for (row, h) in out.iter().enumerate() {
                    if any_null && nulls[row] {
                        b.append_null();
                    } else {
                        b.append_value(*h as u32);
                    }
                }
                Arc::new(b.finish()) as ArrayRef
            })
        }),
    )
}

/// `a` without its top-level null mask (the slots hold some value; the rows
/// are answered `NULL` anyway).
fn fill_nulls(a: &ArrayRef) -> Result<ArrayRef, ChError> {
    let data = a.to_data().into_builder().nulls(None);
    let data = data
        .build()
        .map_err(|e| ChError::logical_error(e.to_string()))?;
    Ok(datafusion::arrow::array::make_array(data))
}

fn int_hash(name: &'static str, wide: bool) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 1, 1)?;
            match &args[0] {
                ChType::UInt8
                | ChType::UInt16
                | ChType::UInt32
                | ChType::UInt64
                | ChType::Int8
                | ChType::Int16
                | ChType::Int32
                | ChType::Int64 => {}
                t => return Err(illegal_type(name, 0, t)),
            }
            Ok(if wide { ChType::UInt64 } else { ChType::UInt32 })
        }),
        Arc::new(move |call| {
            let v = call.ints(0)?;
            // `static_cast<UInt64>`: signed values widen with their sign.
            let bits = |x: i128| x as i64 as u64;
            Ok(if wide {
                let mut b = UInt64Builder::with_capacity(call.rows);
                for row in 0..call.rows {
                    b.append_option(v.get(row).map(|x| int_hash64(bits(x))));
                }
                Arc::new(b.finish()) as ArrayRef
            } else {
                let mut b = UInt32Builder::with_capacity(call.rows);
                for row in 0..call.rows {
                    b.append_option(v.get(row).map(|x| int_hash32(bits(x))));
                }
                Arc::new(b.finish()) as ArrayRef
            })
        }),
    )
}

/// Registers the hash functions.
pub fn register(r: &mut Registry) {
    any_hash("cityHash64", Algo::City64).register(r);
    any_hash("sipHash64", Algo::Sip64).register(r);
    any_hash("xxHash32", Algo::Xx32).register(r);
    any_hash("xxHash64", Algo::Xx64).register(r);
    int_hash("intHash32", false).register(r);
    int_hash("intHash64", true).register(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_hashes_match_the_oracle() {
        // `SELECT …` on clickhouse-server 26.9.2.8.
        assert_eq!(int_hash64(1), 10_577_349_846_663_553_072);
        assert_eq!(int_hash64(255), 8_055_013_221_972_926_055);
        assert_eq!(int_hash64(u64::MAX), 14_600_443_904_207_254_319);
        assert_eq!(int_hash32(1), 1_343_103_100);
        assert_eq!(int_hash32(u64::MAX), 3_545_659_993);
        assert_eq!(Algo::Xx32.bytes(b""), 46_947_589);
        assert_eq!(Algo::Xx32.bytes(b"a"), 1_426_945_110);
        assert_eq!(Algo::Xx64.bytes(b""), 17_241_709_254_077_376_921);
        assert_eq!(Algo::Xx64.bytes(b"a"), 15_154_266_338_359_012_955);
        assert_eq!(Algo::Sip64.bytes(b""), 2_202_906_307_356_721_367);
        assert_eq!(Algo::Sip64.bytes(b"a"), 10_863_254_463_029_944_905);
        let two = |algo: Algo| algo.combine(algo.bytes(b"a"), algo.bytes(b"b"));
        assert_eq!(city_ax(), 946_391_537_628_605_970);
        assert_eq!(two(Algo::Xx32), 1_353_805_580);
        assert_eq!(two(Algo::Xx64), 14_756_808_377_901_732_262);
        assert_eq!(two(Algo::Sip64), 1_193_743_241_355_087_095);
    }

    /// `cityHash64('a', 'x')`.
    fn city_ax() -> u64 {
        let algo = Algo::City64;
        algo.combine(algo.bytes(b"a"), algo.bytes(b"x"))
    }
}
