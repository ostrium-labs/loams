//! The House's encodings of ClickHouse types in Iceberg data files, undone
//! (plan HE1 Task 26): the conversions `loams-house`'s `views.rs` makes with
//! `CAST`s, made here on Arrow arrays.
//!
//! [`restore`] takes a column as the data files hold it (the Iceberg type of
//! `loams_house_types::storage`, as Arrow reads it) to the engine's layout
//! (`loams_house_types::arrow::data_type`): `UInt64` from `decimal(20, 0)`,
//! `UInt8`/`UInt16` from `int`, `UInt32` from `long`, `Enum` from its names,
//! `IPv4` and `IPv6` from their text, `LowCardinality` as a dictionary,
//! `DateTime64(7..9)` from a `long` of nanoseconds (R9.61), `JSON` as its text
//! (R9.63) and `AggregateFunction` as its state bytes (R9.14). [`store`] is
//! the other direction, what the House's write path stores; it writes the
//! test fixtures and, from HE2, the engine's own files.
//!
//! [`from_iceberg`] gives another writer's table (`external`) the ClickHouse
//! types chDB's Iceberg reader gives it, as `loams-house`'s `types.rs` does.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, FixedSizeBinaryArray, Int8Array, Int16Array, ListArray, MapArray,
    StringArray, StructArray, UInt32Array,
};
use datafusion::arrow::compute::{CastOptions, cast_with_options};
use datafusion::arrow::datatypes::{
    DataType, Field, Fields, Int8Type, Int16Type, Int64Type, TimeUnit, UInt32Type,
};
use iceberg_engine::spec::{PrimitiveType, Type};
use loams_house_types::arrow::data_type;
use loams_house_types::{ChError, ChType};

fn err(t: &ChType, what: impl std::fmt::Display) -> ChError {
    ChError::cannot_parse_text(format!("Reading a {t} column of a lake table: {what}"))
}

/// `array` cast to `to`, failing on a value `to` cannot hold.
pub fn cast(array: &ArrayRef, to: &DataType) -> Result<ArrayRef, ChError> {
    if array.data_type() == to {
        return Ok(array.clone());
    }
    cast_with_options(
        array,
        to,
        &CastOptions {
            safe: false,
            ..CastOptions::default()
        },
    )
    .map_err(|e| ChError::cannot_parse_text(format!("Casting {} to {to}: {e}", array.data_type())))
}

/// The UTC zone as Iceberg's Arrow schema spells it.
const UTC: &str = "+00:00";

/// A column of `t` as the data files hold it, in the engine's layout.
/// `top_level` is whether `t` is the column's own type (`DateTime64(7..9)` is
/// a `long` only there, R9.61).
///
/// # Errors
///
/// A value the type cannot hold (an `Enum` name it does not have, text that is
/// not an address), or a layout the House does not write.
pub fn restore(t: &ChType, stored: &ArrayRef, top_level: bool) -> Result<ArrayRef, ChError> {
    let (target, _) = data_type(t)?;
    match t {
        ChType::Nullable(inner) => restore(inner, stored, top_level),
        ChType::SimpleAggregateFunction(spec) => restore(&spec.args[0], stored, top_level),
        ChType::LowCardinality(inner) => {
            if stored.data_type() == &target {
                return Ok(stored.clone());
            }
            let values = restore(inner, stored, top_level)?;
            cast(&values, &target)
        }
        ChType::Enum8(names) => {
            let strings = utf8(t, stored)?;
            let mut out = Vec::with_capacity(strings.len());
            for v in strings.iter() {
                out.push(match v {
                    None => None,
                    Some(name) => Some(
                        names
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, v)| *v)
                            .ok_or_else(|| err(t, format!("{name:?} is not one of its names")))?,
                    ),
                });
            }
            Ok(Arc::new(Int8Array::from(out)))
        }
        ChType::Enum16(names) => {
            let strings = utf8(t, stored)?;
            let mut out = Vec::with_capacity(strings.len());
            for v in strings.iter() {
                out.push(match v {
                    None => None,
                    Some(name) => Some(
                        names
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, v)| *v)
                            .ok_or_else(|| err(t, format!("{name:?} is not one of its names")))?,
                    ),
                });
            }
            Ok(Arc::new(Int16Array::from(out)))
        }
        ChType::IPv4 if stored.data_type() != &target => {
            let strings = utf8(t, stored)?;
            let mut out = Vec::with_capacity(strings.len());
            for v in strings.iter() {
                out.push(match v {
                    None => None,
                    Some(text) => Some(u32::from(
                        text.parse::<Ipv4Addr>()
                            .map_err(|e| err(t, format!("{text:?}: {e}")))?,
                    )),
                });
            }
            Ok(Arc::new(UInt32Array::from(out)))
        }
        ChType::IPv6 if stored.data_type() != &target => {
            let strings = utf8(t, stored)?;
            let mut out: Vec<Option<[u8; 16]>> = Vec::with_capacity(strings.len());
            for v in strings.iter() {
                out.push(match v {
                    None => None,
                    Some(text) => Some(
                        text.parse::<Ipv6Addr>()
                            .map_err(|e| err(t, format!("{text:?}: {e}")))?
                            .octets(),
                    ),
                });
            }
            let array = FixedSizeBinaryArray::try_from_sparse_iter_with_size(out.into_iter(), 16)
                .map_err(|e| err(t, e))?;
            Ok(Arc::new(array))
        }
        ChType::DateTime64(precision, _) if *precision > 6 && stored.data_type().is_integer() => {
            // R9.61: nanoseconds since the epoch in a `long`.
            let nanos = cast(stored, &DataType::Int64)?;
            let as_time = cast(&nanos, &DataType::Timestamp(TimeUnit::Nanosecond, None))?;
            relabel(&as_time, &target)
        }
        ChType::DateTime(_) | ChType::DateTime64(..) => relabel(stored, &target),
        ChType::Array(inner) => {
            let list = stored
                .as_list_opt::<i32>()
                .ok_or_else(|| err(t, format!("stored as {}", stored.data_type())))?;
            let DataType::List(field) = &target else {
                return Err(err(t, "Array is not a List"));
            };
            let values = restore(inner, list.values(), false)?;
            Ok(Arc::new(
                ListArray::try_new(
                    field.clone(),
                    list.offsets().clone(),
                    values,
                    list.nulls().cloned(),
                )
                .map_err(|e| err(t, e))?,
            ))
        }
        ChType::Map(k, v) => {
            let map = stored
                .as_map_opt()
                .ok_or_else(|| err(t, format!("stored as {}", stored.data_type())))?;
            let DataType::Map(field, sorted) = &target else {
                return Err(err(t, "Map is not a Map"));
            };
            let DataType::Struct(fields) = field.data_type() else {
                return Err(err(t, "Map entries are not a Struct"));
            };
            let keys = restore(k, map.keys(), false)?;
            let vals = restore(v, map.values(), false)?;
            let entries = StructArray::try_new(fields.clone(), vec![keys, vals], None)
                .map_err(|e| err(t, e))?;
            Ok(Arc::new(
                MapArray::try_new(
                    field.clone(),
                    map.offsets().clone(),
                    entries,
                    map.nulls().cloned(),
                    *sorted,
                )
                .map_err(|e| err(t, e))?,
            ))
        }
        ChType::Tuple(elements) => {
            let st = stored
                .as_struct_opt()
                .ok_or_else(|| err(t, format!("stored as {}", stored.data_type())))?;
            let DataType::Struct(fields) = &target else {
                return Err(err(t, "Tuple is not a Struct"));
            };
            if st.num_columns() != elements.len() {
                return Err(err(
                    t,
                    format!(
                        "{} stored elements for {}",
                        st.num_columns(),
                        elements.len()
                    ),
                ));
            }
            let children = elements
                .iter()
                .enumerate()
                .map(|(i, (_, et))| restore(et, st.column(i), false))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(
                StructArray::try_new(fields.clone(), children, st.nulls().cloned())
                    .map_err(|e| err(t, e))?,
            ))
        }
        _ => cast(stored, &target),
    }
}

/// A time column cast to `target`, keeping the instant. A stored `timestamptz`
/// is UTC, and a ClickHouse time without a zone reads in the server's zone as
/// the read views' `fromUnixTimestamp64Micro(toUnixTimestamp64Micro(c))` does
/// (the engine's naive timestamps are UTC instants).
fn relabel(stored: &ArrayRef, target: &DataType) -> Result<ArrayRef, ChError> {
    match (stored.data_type(), target) {
        (DataType::Timestamp(from, _), DataType::Timestamp(_, None)) => {
            // Drop the zone first: arrow's cast to a naive timestamp would
            // shift to the zone's local time.
            let naive = cast(stored, &DataType::Timestamp(*from, None))?;
            cast(&naive, target)
        }
        _ => cast(stored, target),
    }
}

fn utf8(t: &ChType, stored: &ArrayRef) -> Result<StringArray, ChError> {
    let strings = cast(stored, &DataType::Utf8).map_err(|e| err(t, e.message))?;
    Ok(strings.as_string::<i32>().clone())
}

/// A column of `t` in the engine's layout as the House stores it: the inverse
/// of [`restore`].
///
/// # Errors
///
/// A type the House does not store in lake tables (`48`), or an array that is
/// not in the engine's layout of `t`.
pub fn store(t: &ChType, array: &ArrayRef, top_level: bool) -> Result<ArrayRef, ChError> {
    use loams_house_types::storage::{Storage, storage};
    let kind = storage(t, top_level)?;
    match t {
        ChType::Nullable(inner) => store(inner, array, top_level),
        ChType::SimpleAggregateFunction(spec) => store(&spec.args[0], array, top_level),
        ChType::LowCardinality(inner) => {
            let plain = match array.data_type() {
                DataType::Dictionary(_, values) => cast(array, values)?,
                _ => array.clone(),
            };
            store(inner, &plain, top_level)
        }
        ChType::Enum8(names) => {
            let values = array
                .as_primitive_opt::<Int8Type>()
                .ok_or_else(|| err(t, "not Int8"))?;
            let out = values
                .iter()
                .map(|v| v.map(|v| enum_name(names, v)).transpose())
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(StringArray::from(out)))
        }
        ChType::Enum16(names) => {
            let values = array
                .as_primitive_opt::<Int16Type>()
                .ok_or_else(|| err(t, "not Int16"))?;
            let out = values
                .iter()
                .map(|v| v.map(|v| enum_name(names, v)).transpose())
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Arc::new(StringArray::from(out)))
        }
        ChType::IPv4 => {
            let values = array
                .as_primitive_opt::<UInt32Type>()
                .ok_or_else(|| err(t, "not UInt32"))?;
            let out: StringArray = values
                .iter()
                .map(|v| v.map(|v| Ipv4Addr::from(v).to_string()))
                .collect();
            Ok(Arc::new(out))
        }
        ChType::IPv6 => {
            let values = array
                .as_fixed_size_binary_opt()
                .ok_or_else(|| err(t, "not FixedSizeBinary(16)"))?;
            let mut out = Vec::with_capacity(values.len());
            for i in 0..values.len() {
                out.push(if values.is_null(i) {
                    None
                } else {
                    let bytes: [u8; 16] = values
                        .value(i)
                        .try_into()
                        .map_err(|_| err(t, "not 16 bytes"))?;
                    Some(Ipv6Addr::from(bytes).to_string())
                });
            }
            Ok(Arc::new(StringArray::from(out)))
        }
        ChType::Array(inner) => {
            let list = array
                .as_list_opt::<i32>()
                .ok_or_else(|| err(t, "not a List"))?;
            let values = store(inner, list.values(), false)?;
            let field = Arc::new(Field::new("element", values.data_type().clone(), true));
            Ok(Arc::new(
                ListArray::try_new(field, list.offsets().clone(), values, list.nulls().cloned())
                    .map_err(|e| err(t, e))?,
            ))
        }
        ChType::Map(k, v) => {
            let map = array.as_map_opt().ok_or_else(|| err(t, "not a Map"))?;
            let keys = store(k, map.keys(), false)?;
            let vals = store(v, map.values(), false)?;
            let fields = Fields::from(vec![
                Field::new("key", keys.data_type().clone(), false),
                Field::new("value", vals.data_type().clone(), true),
            ]);
            let entries = StructArray::try_new(fields.clone(), vec![keys, vals], None)
                .map_err(|e| err(t, e))?;
            let field = Arc::new(Field::new("key_value", DataType::Struct(fields), false));
            Ok(Arc::new(
                MapArray::try_new(
                    field,
                    map.offsets().clone(),
                    entries,
                    map.nulls().cloned(),
                    false,
                )
                .map_err(|e| err(t, e))?,
            ))
        }
        ChType::Tuple(elements) => {
            let st = array
                .as_struct_opt()
                .ok_or_else(|| err(t, "not a Struct"))?;
            let mut fields = Vec::with_capacity(elements.len());
            let mut children = Vec::with_capacity(elements.len());
            for (i, (name, et)) in elements.iter().enumerate() {
                let child = store(et, st.column(i), false)?;
                fields.push(Field::new(
                    name.clone().unwrap_or_else(|| (i + 1).to_string()),
                    child.data_type().clone(),
                    true,
                ));
                children.push(child);
            }
            Ok(Arc::new(
                StructArray::try_new(Fields::from(fields), children, st.nulls().cloned())
                    .map_err(|e| err(t, e))?,
            ))
        }
        _ => {
            let to = match kind {
                Storage::Int => DataType::Int32,
                Storage::Long => DataType::Int64,
                Storage::LongNanos => {
                    let naive = relabel(array, &DataType::Timestamp(TimeUnit::Nanosecond, None))?;
                    let nanos = naive
                        .as_primitive_opt::<datafusion::arrow::datatypes::TimestampNanosecondType>()
                        .ok_or_else(|| err(t, "not a timestamp"))?
                        .reinterpret_cast::<Int64Type>();
                    return Ok(Arc::new(nanos));
                }
                Storage::Float => DataType::Float32,
                Storage::Double => DataType::Float64,
                Storage::Decimal(p, s) => DataType::Decimal128(p, s as i8),
                Storage::Boolean => DataType::Boolean,
                Storage::String => DataType::Utf8,
                Storage::Fixed(n) => DataType::FixedSizeBinary(n as i32),
                Storage::Date => DataType::Date32,
                Storage::TimestampTz => {
                    DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from(UTC)))
                }
                Storage::TimestampTzNs => {
                    DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from(UTC)))
                }
                Storage::Uuid => DataType::FixedSizeBinary(16),
                Storage::Binary => DataType::Binary,
                Storage::List(_) | Storage::Map(..) | Storage::Struct(_) => {
                    return Err(err(t, "a composite stored as a primitive"));
                }
            };
            if let DataType::Timestamp(..) = array.data_type() {
                // A naive ClickHouse time is a UTC instant: label it before
                // the unit changes.
                let unit = match array.data_type() {
                    DataType::Timestamp(u, _) => *u,
                    _ => TimeUnit::Microsecond,
                };
                let labelled = match array.data_type() {
                    DataType::Timestamp(_, None) => {
                        cast(array, &DataType::Timestamp(unit, Some(Arc::from(UTC))))?
                    }
                    _ => array.clone(),
                };
                return cast(&labelled, &to);
            }
            cast(array, &to)
        }
    }
}

fn enum_name<V: PartialEq + Copy + std::fmt::Display>(
    names: &[(String, V)],
    v: V,
) -> Result<String, ChError> {
    names
        .iter()
        .find(|(_, x)| *x == v)
        .map(|(n, _)| n.clone())
        .ok_or_else(|| ChError::cannot_parse_text(format!("{v} is not a value of the Enum")))
}

/// The ClickHouse type chDB's Iceberg reader gives a field of another writer's
/// table (as `loams-house`'s `types::from_iceberg`): `Nullable` for every
/// optional primitive. A type the engine cannot lay out (`variant`,
/// `geometry`, `unknown`) is `Other`, which a read refuses when it names the
/// column (R26.7).
pub fn from_iceberg(ty: &Type, required: bool) -> ChType {
    let base = match ty {
        Type::Primitive(p) => match p {
            PrimitiveType::Boolean => ChType::Bool,
            PrimitiveType::Int => ChType::Int32,
            PrimitiveType::Long | PrimitiveType::Time => ChType::Int64,
            PrimitiveType::Float => ChType::Float32,
            PrimitiveType::Double => ChType::Float64,
            PrimitiveType::Decimal { precision, scale } => ChType::Decimal {
                precision: u8::try_from(*precision).unwrap_or(38),
                scale: u8::try_from(*scale).unwrap_or(0),
            },
            PrimitiveType::Date => ChType::Date32,
            PrimitiveType::Timestamp => ChType::DateTime64(6, None),
            PrimitiveType::Timestamptz => ChType::DateTime64(6, Some("UTC".to_string())),
            PrimitiveType::TimestampNs => ChType::DateTime64(9, None),
            PrimitiveType::TimestamptzNs => ChType::DateTime64(9, Some("UTC".to_string())),
            PrimitiveType::String | PrimitiveType::Binary => ChType::String,
            PrimitiveType::Uuid => ChType::Uuid,
            PrimitiveType::Fixed(n) => ChType::FixedString(u32::try_from(*n).unwrap_or(u32::MAX)),
            other => return ChType::Other(other.to_string()),
        },
        Type::List(list) => {
            return ChType::Array(Box::new(from_iceberg(
                &list.element_field.field_type,
                list.element_field.required,
            )));
        }
        Type::Map(map) => {
            return ChType::Map(
                Box::new(from_iceberg(&map.key_field.field_type, true)),
                Box::new(from_iceberg(
                    &map.value_field.field_type,
                    map.value_field.required,
                )),
            );
        }
        Type::Struct(fields) => {
            return ChType::Tuple(
                fields
                    .fields()
                    .iter()
                    .map(|f| {
                        (
                            Some(f.name.clone()),
                            from_iceberg(&f.field_type, f.required),
                        )
                    })
                    .collect(),
            );
        }
        Type::Variant(_) => return ChType::Other("Variant".to_string()),
    };
    if required {
        base
    } else {
        ChType::Nullable(Box::new(base))
    }
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::array::{
        Decimal128Array, Int32Array, Int64Array, TimestampMicrosecondArray, UInt8Array, UInt64Array,
    };

    use super::*;

    fn t(text: &str) -> ChType {
        loams_house_types::parse(text).expect("type")
    }

    #[test]
    fn stored_values_restore_to_the_engine_layout() {
        let u64s: ArrayRef = Arc::new(
            Decimal128Array::from(vec![Some(18_446_744_073_709_551_615i128), None])
                .with_precision_and_scale(20, 0)
                .expect("decimal"),
        );
        let back = restore(&t("Nullable(UInt64)"), &u64s, true).expect("u64");
        assert_eq!(
            back.as_primitive::<datafusion::arrow::datatypes::UInt64Type>()
                .value(0),
            u64::MAX
        );
        let names: ArrayRef = Arc::new(StringArray::from(vec!["b", "a"]));
        let e = restore(&t("Enum8('a' = 1, 'b' = -2)"), &names, true).expect("enum");
        assert_eq!(e.as_primitive::<Int8Type>().values().to_vec(), vec![-2, 1]);
        let bad: ArrayRef = Arc::new(StringArray::from(vec!["c"]));
        assert_eq!(
            restore(&t("Enum8('a' = 1)"), &bad, true)
                .expect_err("no such name")
                .code,
            6
        );
        let ips: ArrayRef = Arc::new(StringArray::from(vec!["1.2.3.4"]));
        let ip = restore(&t("IPv4"), &ips, true).expect("ipv4");
        assert_eq!(ip.as_primitive::<UInt32Type>().value(0), 0x0102_0304);
        let nanos: ArrayRef = Arc::new(Int64Array::from(vec![1_700_000_000_123_456_789]));
        let ts = restore(&t("DateTime64(9, 'UTC')"), &nanos, true).expect("nanos");
        assert_eq!(
            ts.data_type(),
            &DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("UTC")))
        );
        let micros: ArrayRef =
            Arc::new(TimestampMicrosecondArray::from(vec![1_000_000]).with_timezone(UTC));
        let dt = restore(&t("DateTime"), &micros, true).expect("datetime");
        assert_eq!(
            dt.as_primitive::<datafusion::arrow::datatypes::TimestampSecondType>()
                .value(0),
            1
        );
        let small: ArrayRef = Arc::new(Int32Array::from(vec![255]));
        let u8s = restore(&t("LowCardinality(UInt8)"), &small, true).expect("lc");
        assert!(matches!(u8s.data_type(), DataType::Dictionary(..)));
    }

    #[test]
    fn store_inverts_restore() {
        let cases: Vec<(&str, ArrayRef)> = vec![
            ("UInt8", Arc::new(UInt8Array::from(vec![0, 255]))),
            ("UInt64", Arc::new(UInt64Array::from(vec![0, u64::MAX]))),
            (
                "Enum16('x' = 300, 'y' = -1)",
                Arc::new(Int16Array::from(vec![300, -1])),
            ),
            ("IPv4", Arc::new(UInt32Array::from(vec![0, u32::MAX]))),
            (
                "IPv6",
                Arc::new(
                    FixedSizeBinaryArray::try_from_iter(
                        vec![
                            Ipv6Addr::LOCALHOST.octets().to_vec(),
                            "::ffff:1.2.3.4"
                                .parse::<Ipv6Addr>()
                                .expect("ip")
                                .octets()
                                .to_vec(),
                        ]
                        .into_iter(),
                    )
                    .expect("fixed"),
                ),
            ),
            (
                "DateTime64(9)",
                Arc::new(datafusion::arrow::array::TimestampNanosecondArray::from(
                    vec![-1, 1_700_000_000_123_456_789],
                )),
            ),
            (
                "DateTime('Asia/Kolkata')",
                Arc::new(
                    datafusion::arrow::array::TimestampSecondArray::from(vec![0, 1_700_000_000])
                        .with_timezone("Asia/Kolkata"),
                ),
            ),
        ];
        for (text, array) in cases {
            let ty = t(text);
            let stored = store(&ty, &array, true).expect(text);
            let back = restore(&ty, &stored, true).expect(text);
            assert_eq!(&back, &array, "{text}");
        }
    }

    #[test]
    fn external_types_follow_chdb() {
        assert_eq!(
            from_iceberg(&Type::Primitive(PrimitiveType::Timestamptz), false).to_string(),
            "Nullable(DateTime64(6, 'UTC'))"
        );
        assert_eq!(
            from_iceberg(&Type::Primitive(PrimitiveType::Date), true).to_string(),
            "Date32"
        );
    }
}
