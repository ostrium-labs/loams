//! Value text against the 26.9.2.8 oracle: each case of
//! `tests/fixtures/value_cases.tsv` is built here as an Arrow column in the
//! engine's layout and written in the four styles, then compared with what
//! the oracle wrote for `SELECT <expr> AS v FORMAT TSV | CSV | JSONEachRow |
//! Values` (`values.tsv`).

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::builder::{Float64Builder, MapBuilder, StringBuilder, UInt8Builder};
use arrow_array::types::Int32Type;
use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, DictionaryArray, FixedSizeBinaryArray,
    Float32Array, Float64Array, Int8Array, Int32Array, Int64Array, ListArray, StringArray,
    StructArray, TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
    UInt8Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field};
use loams_house_types::arrow::to_arrow;
use loams_house_types::text::{Style, TextSettings, write_value};
use loams_house_types::{ChType, parse};

fn ty(text: &str) -> ChType {
    parse(text).unwrap()
}

fn tuple_array(t: &ChType, children: Vec<ArrayRef>) -> ArrayRef {
    let DataType::Struct(fields) = to_arrow("v", t).unwrap().data_type().clone() else {
        unreachable!()
    };
    Arc::new(StructArray::try_new(fields, children, None).unwrap())
}

fn list_array(t: &ChType, values: ArrayRef) -> ArrayRef {
    let DataType::List(field) = to_arrow("v", t).unwrap().data_type().clone() else {
        unreachable!()
    };
    let len = values.len() as i32;
    Arc::new(ListArray::new(
        field,
        arrow_buffer::OffsetBuffer::new(vec![0, len].into()),
        values,
        None,
    ))
}

/// The engine's column for each case, with its ClickHouse type.
fn cases() -> BTreeMap<&'static str, (ChType, ArrayRef)> {
    let mut m: BTreeMap<&'static str, (ChType, ArrayRef)> = BTreeMap::new();
    let utc = Some("UTC");
    m.insert("u8", (ty("UInt8"), Arc::new(UInt8Array::from(vec![200]))));
    m.insert(
        "u64",
        (ty("UInt64"), Arc::new(UInt64Array::from(vec![u64::MAX]))),
    );
    m.insert(
        "i64",
        (ty("Int64"), Arc::new(Int64Array::from(vec![i64::MIN]))),
    );
    for (id, v) in [
        ("f64_third", 1.0 / 3.0),
        ("f64_1e21", 1e21),
        ("f64_1e20", 1e20),
        ("f64_small", 1e-7),
        ("f64_micro", 0.000001),
        ("f64_negzero", -0.0),
        ("f64_nan", f64::NAN),
        ("f64_inf", f64::NEG_INFINITY),
        ("f64_big", 123456789012345680000.0),
        ("f64_frac", 123456789.125),
    ] {
        m.insert(id, (ty("Float64"), Arc::new(Float64Array::from(vec![v]))));
    }
    m.insert(
        "f32_tenth",
        (ty("Float32"), Arc::new(Float32Array::from(vec![0.1f32]))),
    );
    m.insert(
        "f32_big",
        (
            ty("Float32"),
            Arc::new(Float32Array::from(vec![16777217.0f32])),
        ),
    );
    m.insert(
        "bool",
        (ty("Bool"), Arc::new(BooleanArray::from(vec![true]))),
    );
    let dec = |v: i128, p: u8, s: i8| -> ArrayRef {
        Arc::new(
            Decimal128Array::from(vec![v])
                .with_precision_and_scale(p, s)
                .unwrap(),
        )
    };
    m.insert("dec", (ty("Decimal(18, 2)"), dec(150, 18, 2)));
    m.insert("dec_neg", (ty("Decimal(9, 3)"), dec(-50, 9, 3)));
    m.insert("dec_int", (ty("Decimal(38, 4)"), dec(420000, 38, 4)));
    m.insert(
        "str",
        (
            ty("String"),
            Arc::new(StringArray::from(vec!["a\tb\nc\\d"])),
        ),
    );
    m.insert(
        "str_quotes",
        (
            ty("String"),
            Arc::new(StringArray::from(vec!["it's \"q\" /x"])),
        ),
    );
    m.insert(
        "fixed",
        (
            ty("FixedString(3)"),
            Arc::new(
                FixedSizeBinaryArray::try_from_iter(std::iter::once(b"ab\0".to_vec())).unwrap(),
            ),
        ),
    );
    m.insert(
        "date",
        (ty("Date"), Arc::new(Date32Array::from(vec![20749]))),
    );
    m.insert(
        "date32_old",
        (ty("Date32"), Arc::new(Date32Array::from(vec![-25567]))),
    );
    m.insert(
        "datetime_utc",
        (
            ty("DateTime('UTC')"),
            Arc::new(TimestampSecondArray::from(vec![1_792_717_323]).with_timezone_opt(utc)),
        ),
    );
    m.insert(
        "datetime_kolkata",
        (
            ty("DateTime('Asia/Kolkata')"),
            Arc::new(
                TimestampSecondArray::from(vec![1_792_697_523])
                    .with_timezone_opt(Some("Asia/Kolkata")),
            ),
        ),
    );
    m.insert(
        "dt64_3",
        (
            ty("DateTime64(3, 'UTC')"),
            Arc::new(
                TimestampMillisecondArray::from(vec![1_792_717_323_500]).with_timezone_opt(utc),
            ),
        ),
    );
    m.insert(
        "dt64_2",
        (
            ty("DateTime64(2, 'UTC')"),
            Arc::new(
                TimestampMillisecondArray::from(vec![1_792_717_323_250]).with_timezone_opt(utc),
            ),
        ),
    );
    m.insert(
        "dt64_9",
        (
            ty("DateTime64(9, 'UTC')"),
            Arc::new(
                TimestampNanosecondArray::from(vec![1_792_717_323_123_456_789])
                    .with_timezone_opt(utc),
            ),
        ),
    );
    m.insert(
        "dt64_neg",
        (
            ty("DateTime64(3, 'UTC')"),
            Arc::new(TimestampMillisecondArray::from(vec![-500]).with_timezone_opt(utc)),
        ),
    );
    let uuid: Vec<u8> = (0..16)
        .map(|i| {
            u8::from_str_radix(&"61f0c4045cb311e7907ba6006ad3dba0"[i * 2..i * 2 + 2], 16).unwrap()
        })
        .collect();
    m.insert(
        "uuid",
        (
            ty("UUID"),
            Arc::new(FixedSizeBinaryArray::try_from_iter(std::iter::once(uuid)).unwrap()),
        ),
    );
    m.insert(
        "ipv4",
        (ty("IPv4"), Arc::new(UInt32Array::from(vec![0x01020304]))),
    );
    let v6 = |a: std::net::Ipv6Addr| -> ArrayRef {
        Arc::new(FixedSizeBinaryArray::try_from_iter(std::iter::once(a.octets().to_vec())).unwrap())
    };
    m.insert("ipv6", (ty("IPv6"), v6("2001:db8::1".parse().unwrap())));
    m.insert(
        "ipv6_mapped",
        (ty("IPv6"), v6("::ffff:1.2.3.4".parse().unwrap())),
    );
    m.insert(
        "enum",
        (
            ty("Enum8('a' = 1, 'b' = 2)"),
            Arc::new(Int8Array::from(vec![2])),
        ),
    );
    m.insert(
        "null_str",
        (
            ty("Nullable(String)"),
            Arc::new(StringArray::from(vec![None::<&str>])),
        ),
    );
    m.insert(
        "lc",
        (
            ty("LowCardinality(String)"),
            Arc::new(
                DictionaryArray::<Int32Type>::try_new(
                    Int32Array::from(vec![0]),
                    Arc::new(StringArray::from(vec!["x"])),
                )
                .unwrap(),
            ),
        ),
    );
    let arr_str = ty("Array(String)");
    let a = list_array(
        &arr_str,
        Arc::new(StringArray::from(vec!["a\tb", "c'd", "e\"f"])),
    );
    m.insert("arr_str", (arr_str, a));
    let arr_null = ty("Array(Nullable(UInt8))");
    let a = list_array(&arr_null, Arc::new(UInt8Array::from(vec![None, Some(1)])));
    m.insert("arr_null", (arr_null, a));
    let arr_empty = ty("Array(UInt8)");
    let a = list_array(&arr_empty, Arc::new(UInt8Array::from(Vec::<u8>::new())));
    m.insert("arr_empty", (arr_empty, a));
    let arr_date = ty("Array(Date)");
    let a = list_array(&arr_date, Arc::new(Date32Array::from(vec![20454])));
    m.insert("arr_date", (arr_date, a));
    let tuple = ty("Tuple(UInt8, String)");
    let a = tuple_array(
        &tuple,
        vec![
            Arc::new(UInt8Array::from(vec![1])),
            Arc::new(StringArray::from(vec!["x"])),
        ],
    );
    m.insert("tuple", (tuple, a));
    let named = ty("Tuple(a UInt8, b String)");
    let a = tuple_array(
        &named,
        vec![
            Arc::new(UInt8Array::from(vec![1])),
            Arc::new(StringArray::from(vec!["y"])),
        ],
    );
    m.insert("tuple_named", (named, a));
    let mut b = MapBuilder::new(None, StringBuilder::new(), Float64Builder::new());
    b.keys().append_value("k");
    b.values().append_value(1.5);
    b.keys().append_value("l/");
    b.values().append_value(2.0);
    b.append(true).unwrap();
    m.insert("map", (ty("Map(String, Float64)"), Arc::new(b.finish())));
    let mut b = MapBuilder::new(None, UInt8Builder::new(), StringBuilder::new());
    b.keys().append_value(1);
    b.values().append_value("a");
    b.append(true).unwrap();
    m.insert(
        "map_int_key",
        (ty("Map(UInt8, String)"), Arc::new(b.finish())),
    );
    m
}

/// The value inside the oracle's one-row output in `style`.
fn unwrap_output(style: &str, out: &str) -> String {
    let out = out.strip_suffix('\n').unwrap_or(out);
    match style {
        "JSONEachRow" => out
            .strip_prefix("{\"v\":")
            .and_then(|s| s.strip_suffix('}'))
            .unwrap()
            .to_string(),
        "Values" => out
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(')'))
            .unwrap()
            .to_string(),
        _ => out.to_string(),
    }
}

#[test]
fn value_text_matches_the_oracle() {
    let cases = cases();
    let settings = TextSettings::default();
    let mut problems = Vec::new();
    let mut checked = 0;
    let fixture = std::fs::read_to_string(format!(
        "{}/tests/fixtures/values.tsv",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for line in fixture.lines() {
        let mut cols = line.splitn(3, '\t');
        let (id, style, json) = (
            cols.next().unwrap(),
            cols.next().unwrap(),
            cols.next().unwrap(),
        );
        let expected = unwrap_output(style, &serde_json::from_str::<String>(json).unwrap());
        let Some((t, array)) = cases.get(id) else {
            problems.push(format!("{id}: no column built for this case"));
            continue;
        };
        let s = match style {
            "TSV" => Style::Tsv,
            "CSV" => Style::Csv,
            "JSONEachRow" => Style::Json,
            _ => Style::Quoted,
        };
        let mut out = Vec::new();
        write_value(t, array.as_ref(), 0, s, &settings, &mut out).unwrap();
        checked += 1;
        let got = String::from_utf8(out).unwrap();
        if got != expected {
            problems.push(format!("{id} {style}: {got:?}, the oracle {expected:?}"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(checked, cases.len() * 4);
}

#[test]
fn field_of_a_type_carries_the_type() {
    let f: Field = to_arrow("v", &ty("LowCardinality(Nullable(String))")).unwrap();
    assert_eq!(
        f.metadata()["loams.ch.type"],
        "LowCardinality(Nullable(String))"
    );
    assert!(f.is_nullable());
}

/// `date_time_output_format` (HE1 Task 37): `iso` is always UTC with `T` and
/// `Z`; `unix_timestamp` is the seconds, with a `DateTime64`'s fraction; and a
/// zone-less `DateTime` is written in the session's zone.
#[test]
fn date_time_output_format_and_session_zone() {
    use loams_house_types::text::{DateTimeOutput, unix_timestamp_text};
    let kolkata: ArrayRef = Arc::new(
        TimestampSecondArray::from(vec![1_792_697_523]).with_timezone_opt(Some("Asia/Kolkata")),
    );
    let t = ty("DateTime('Asia/Kolkata')");
    let text = |settings: &TextSettings, t: &ChType, a: &ArrayRef, style: Style| {
        let mut out = Vec::new();
        write_value(t, a.as_ref(), 0, style, settings, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    };
    let simple = TextSettings::default();
    assert_eq!(
        text(&simple, &t, &kolkata, Style::Tsv),
        "2026-10-23 01:02:03"
    );
    let iso = TextSettings {
        date_time_output: DateTimeOutput::Iso,
        ..TextSettings::default()
    };
    assert_eq!(text(&iso, &t, &kolkata, Style::Tsv), "2026-10-22T19:32:03Z");
    assert_eq!(
        text(&iso, &t, &kolkata, Style::Json),
        "\"2026-10-22T19:32:03Z\""
    );
    let unix = TextSettings {
        date_time_output: DateTimeOutput::UnixTimestamp,
        ..TextSettings::default()
    };
    assert_eq!(text(&unix, &t, &kolkata, Style::Tsv), "1792697523");
    let ms: ArrayRef = Arc::new(
        TimestampMillisecondArray::from(vec![1_792_717_323_500]).with_timezone_opt(Some("UTC")),
    );
    let t64 = ty("DateTime64(3, 'UTC')");
    assert_eq!(text(&unix, &t64, &ms, Style::Tsv), "1792717323.500");
    assert_eq!(
        text(&iso, &t64, &ms, Style::Tsv),
        "2026-10-23T01:02:03.500Z"
    );
    // A zone-less DateTime takes the session's zone.
    let plain: ArrayRef = Arc::new(TimestampSecondArray::from(vec![1_792_697_523]));
    let session = TextSettings {
        timezone: "Asia/Kolkata".into(),
        ..TextSettings::default()
    };
    assert_eq!(
        text(&session, &ty("DateTime"), &plain, Style::Tsv),
        "2026-10-23 01:02:03"
    );
    assert_eq!(
        text(&simple, &ty("DateTime"), &plain, Style::Tsv),
        "2026-10-22 19:32:03"
    );
    // Negative values split towards zero, as ClickHouse's DecimalUtils::split.
    assert_eq!(unix_timestamp_text(-2, 5, 1), "-1.5");
    assert_eq!(unix_timestamp_text(-1, 5, 1), "-0.5");
    assert_eq!(unix_timestamp_text(-3, 0, 2), "-3.00");
    assert_eq!(DateTimeOutput::parse("iso"), Some(DateTimeOutput::Iso));
    assert_eq!(DateTimeOutput::parse("ISO"), None);
}
