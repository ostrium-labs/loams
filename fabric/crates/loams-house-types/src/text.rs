//! Value text: how ClickHouse writes one value of a type, per format family.
//!
//! Measured on the 26.9.2.8 oracle (plan HE1 Task 21; `tests/text.rs` keeps
//! the captured answers):
//!
//! * **TSV** (`TabSeparated*`): strings bare with backslash escapes, `\N` for
//!   `NULL`; composite values (arrays, tuples, maps) in the quoted style;
//! * **CSV**: strings, dates and other text-like values in double quotes with
//!   `"` doubled, numbers and `true`/`false` bare, `\N` for `NULL`, a top-level
//!   tuple as one field per element, other composites as a quoted string of
//!   their quoted-style text;
//! * **JSON** (`JSON*`): JSON values; `nan` and `inf` are `null`, 64-bit
//!   integers bare (`output_format_json_quote_64bit_integers = 0` is 26.9's
//!   default), `/` escaped, named tuples as objects;
//! * **Quoted** (`Values`, and every element inside a composite): strings and
//!   dates in single quotes, `NULL`, `[…]`, `(…)`, `{k:v}` without spaces.
//!
//! Floats are written in their shortest round-trip form, laid out as
//! ClickHouse does: plain digits while the decimal exponent is in
//! `-7 < e < 21`, else `d.ddde±x` (`1e21`, `1e-7`), with `nan`, `inf`, `-inf`
//! and `-0`.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal128Type, Decimal256Type, Float32Type, Float64Type, Int8Type, Int16Type,
    Int32Type, Int64Type, TimestampMicrosecondType, TimestampMillisecondType,
    TimestampNanosecondType, TimestampSecondType, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use arrow_array::{Array, ArrayRef};
use arrow_schema::{DataType, TimeUnit};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

use crate::arrow::ticks_per_unit;
use crate::error::ChError;
use crate::ty::ChType;

/// A format family's way of writing values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `TabSeparated` and its variants.
    Tsv,
    /// `CSV` and its variants.
    Csv,
    /// The `JSON*` formats.
    Json,
    /// `Values`, and values inside arrays, tuples and maps.
    Quoted,
}

/// `date_time_output_format`: how `DateTime` and `DateTime64` values are
/// written (ClickHouse's `FormatSettings::DateTimeOutputFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateTimeOutput {
    /// `simple` (the default): `YYYY-MM-DD hh:mm:ss[.fff]` in the value's zone.
    #[default]
    Simple,
    /// `iso`: `YYYY-MM-DDThh:mm:ss[.fff]Z`, always in UTC.
    Iso,
    /// `unix_timestamp`: seconds since the epoch, with the fraction of a
    /// `DateTime64`.
    UnixTimestamp,
}

impl DateTimeOutput {
    /// The setting's value as ClickHouse spells it.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "simple" => Some(Self::Simple),
            "iso" => Some(Self::Iso),
            "unix_timestamp" => Some(Self::UnixTimestamp),
            _ => None,
        }
    }
}

/// The settings that change how values are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSettings {
    /// The zone for `DateTime` columns without one of their own: the
    /// session's `session_timezone`, else the server's.
    pub timezone: String,
    /// `output_format_json_quote_64bit_integers` (26.9 default: off).
    pub json_quote_64bit_integers: bool,
    /// `output_format_json_quote_denormals` (default: off, so `null`).
    pub json_quote_denormals: bool,
    /// `output_format_json_escape_forward_slashes` (default: on).
    pub json_escape_forward_slashes: bool,
    /// `output_format_json_named_tuples_as_objects` (default: on).
    pub json_named_tuples_as_objects: bool,
    /// `output_format_decimal_trailing_zeros` (default: off).
    pub decimal_trailing_zeros: bool,
    /// `date_time_output_format` (default: `simple`).
    pub date_time_output: DateTimeOutput,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            timezone: "UTC".to_string(),
            json_quote_64bit_integers: false,
            json_quote_denormals: false,
            json_escape_forward_slashes: true,
            json_named_tuples_as_objects: true,
            decimal_trailing_zeros: false,
            date_time_output: DateTimeOutput::Simple,
        }
    }
}

/// A `Float64` as ClickHouse writes it.
pub fn float64(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    layout(&format!("{f:e}"))
}

/// A `Float32` as ClickHouse writes it (its own shortest form, not the
/// `Float64` one: `toFloat32(0.1)` is `0.1`).
pub fn float32(f: f32) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    layout(&format!("{f:e}"))
}

/// Lays out Rust's shortest `LowerExp` text (`-1.25e3`) the way ClickHouse
/// (and JavaScript's `Number#toString`) does.
fn layout(exp_text: &str) -> String {
    let (mantissa, exp) = exp_text.split_once('e').unwrap_or((exp_text, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (negative, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if digits == "0" {
        out.push('0');
    } else if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(out, "e{}", n - 1);
    }
    out
}

/// Writes row `row` of `array`, a column of `t`, in `style`.
pub fn write_value(
    t: &ChType,
    array: &dyn Array,
    row: usize,
    style: Style,
    settings: &TextSettings,
    out: &mut Vec<u8>,
) -> Result<(), ChError> {
    Writer { settings }.value(t, array, row, style, out)
}

struct Writer<'a> {
    settings: &'a TextSettings,
}

fn bad_array(t: &ChType, array: &dyn Array) -> ChError {
    ChError::logical_error(format!(
        "A column of {t} holds an Arrow {} array",
        array.data_type()
    ))
}

impl Writer<'_> {
    fn null(&self, style: Style, out: &mut Vec<u8>) {
        out.extend_from_slice(match style {
            Style::Tsv | Style::Csv => b"\\N",
            Style::Json => b"null",
            Style::Quoted => b"NULL",
        });
    }

    fn value(
        &self,
        t: &ChType,
        array: &dyn Array,
        row: usize,
        style: Style,
        out: &mut Vec<u8>,
    ) -> Result<(), ChError> {
        if let DataType::Dictionary(..) = array.data_type() {
            let inner = match t {
                ChType::LowCardinality(inner) => inner.as_ref(),
                other => other,
            };
            let dict = array
                .as_any_dictionary_opt()
                .ok_or_else(|| bad_array(t, array))?;
            if dict.keys().is_null(row) {
                self.null(style, out);
                return Ok(());
            }
            let key = dict.normalized_keys()[row];
            return self.value(inner, dict.values().as_ref(), key, style, out);
        }
        if array.is_null(row) {
            self.null(style, out);
            return Ok(());
        }
        let t = match t {
            ChType::Nullable(inner) | ChType::LowCardinality(inner) => inner.as_ref(),
            ChType::SimpleAggregateFunction(spec) => &spec.args[0],
            other => other,
        };
        if matches!(t, ChType::Nullable(_) | ChType::LowCardinality(_)) {
            return self.value(t, array, row, style, out);
        }
        match t {
            ChType::UInt8 => self.int(
                array
                    .as_primitive_opt::<UInt8Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                false,
                style,
                out,
            ),
            ChType::UInt16 => self.int(
                array
                    .as_primitive_opt::<UInt16Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                false,
                style,
                out,
            ),
            ChType::UInt32 => self.int(
                array
                    .as_primitive_opt::<UInt32Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                false,
                style,
                out,
            ),
            ChType::UInt64 => self.int(
                array
                    .as_primitive_opt::<UInt64Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                true,
                style,
                out,
            ),
            ChType::Int8 => self.int(
                array
                    .as_primitive_opt::<Int8Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                false,
                style,
                out,
            ),
            ChType::Int16 => self.int(
                array
                    .as_primitive_opt::<Int16Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                false,
                style,
                out,
            ),
            ChType::Int32 => self.int(
                array
                    .as_primitive_opt::<Int32Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                false,
                style,
                out,
            ),
            ChType::Int64 | ChType::Interval(_) => self.int(
                array
                    .as_primitive_opt::<Int64Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row),
                true,
                style,
                out,
            ),
            ChType::Float32 => {
                let v = array
                    .as_primitive_opt::<Float32Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                self.float(float32(v), v.is_finite(), style, out);
            }
            ChType::Float64 => {
                let v = array
                    .as_primitive_opt::<Float64Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                self.float(float64(v), v.is_finite(), style, out);
            }
            ChType::Bool => {
                let v = array
                    .as_boolean_opt()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                out.extend_from_slice(if v { b"true" } else { b"false" });
            }
            ChType::Decimal { scale, .. } => {
                let digits = match array.data_type() {
                    DataType::Decimal128(..) => array
                        .as_primitive::<Decimal128Type>()
                        .value(row)
                        .to_string(),
                    DataType::Decimal256(..) => array
                        .as_primitive::<Decimal256Type>()
                        .value(row)
                        .to_string(),
                    _ => return Err(bad_array(t, array)),
                };
                out.extend_from_slice(
                    decimal_text(&digits, *scale, self.settings.decimal_trailing_zeros).as_bytes(),
                );
            }
            ChType::String | ChType::Json(_) | ChType::AggregateFunction(_) => {
                let bytes: &[u8] = match array.data_type() {
                    DataType::Utf8 => array.as_string::<i32>().value(row).as_bytes(),
                    DataType::LargeUtf8 => array.as_string::<i64>().value(row).as_bytes(),
                    DataType::Utf8View => array.as_string_view().value(row).as_bytes(),
                    DataType::Binary => array.as_binary::<i32>().value(row),
                    DataType::LargeBinary => array.as_binary::<i64>().value(row),
                    DataType::BinaryView => array.as_binary_view().value(row),
                    _ => return Err(bad_array(t, array)),
                };
                if matches!(t, ChType::Json(_)) && style == Style::Json {
                    out.extend_from_slice(bytes);
                } else {
                    self.string(bytes, style, out);
                }
            }
            ChType::FixedString(_) => {
                let v = array
                    .as_fixed_size_binary_opt()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                self.string(v, style, out);
            }
            ChType::Uuid => {
                let v = array
                    .as_fixed_size_binary_opt()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                self.text_like(&uuid_text(v), style, out);
            }
            ChType::IPv6 => {
                let v = array
                    .as_fixed_size_binary_opt()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                let bytes: [u8; 16] = v.try_into().map_err(|_| bad_array(t, array))?;
                self.text_like(&Ipv6Addr::from(bytes).to_string(), style, out);
            }
            ChType::IPv4 => {
                let v = array
                    .as_primitive_opt::<UInt32Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                self.text_like(&Ipv4Addr::from(v).to_string(), style, out);
            }
            ChType::Enum8(values) => {
                let v = array
                    .as_primitive_opt::<Int8Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                match values.iter().find(|(_, x)| *x == v) {
                    Some((name, _)) => self.string(name.as_bytes(), style, out),
                    None => self.int(v, false, style, out),
                }
            }
            ChType::Enum16(values) => {
                let v = array
                    .as_primitive_opt::<Int16Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                match values.iter().find(|(_, x)| *x == v) {
                    Some((name, _)) => self.string(name.as_bytes(), style, out),
                    None => self.int(v, false, style, out),
                }
            }
            ChType::Date | ChType::Date32 => {
                let days = array
                    .as_primitive_opt::<Date32Type>()
                    .ok_or_else(|| bad_array(t, array))?
                    .value(row);
                self.text_like(&date_text(days), style, out);
            }
            ChType::DateTime(tz) => {
                let secs = timestamp_value(array, row).ok_or_else(|| bad_array(t, array))?;
                let text = self.datetime(
                    secs.0,
                    0,
                    0,
                    tz.as_deref().unwrap_or(&self.settings.timezone),
                )?;
                self.text_like(&text, style, out);
            }
            ChType::DateTime64(p, tz) => {
                let (value, unit) =
                    timestamp_value(array, row).ok_or_else(|| bad_array(t, array))?;
                let _ = unit;
                let ticks = value.div_euclid(ticks_per_unit(*p));
                let scale = 10i64.pow(u32::from(*p));
                let text = self.datetime(
                    ticks.div_euclid(scale),
                    ticks.rem_euclid(scale),
                    *p,
                    tz.as_deref().unwrap_or(&self.settings.timezone),
                )?;
                self.text_like(&text, style, out);
            }
            ChType::Array(inner) => {
                let list = array
                    .as_list_opt::<i32>()
                    .ok_or_else(|| bad_array(t, array))?;
                let values = list.value(row);
                self.sequence(inner, &values, style, b'[', b']', out)?;
            }
            ChType::Map(k, v) => {
                let map = array.as_map_opt().ok_or_else(|| bad_array(t, array))?;
                let entries = map.value(row);
                let (keys, vals) = (entries.column(0), entries.column(1));
                self.composite(style, out, |w, inner_style, out| {
                    out.push(b'{');
                    for i in 0..entries.len() {
                        if i > 0 {
                            out.push(b',');
                        }
                        if inner_style == Style::Json {
                            let mut key = Vec::new();
                            w.value(k, keys.as_ref(), i, Style::Tsv, &mut key)?;
                            if matches!(k.base(), ChType::String | ChType::FixedString(_)) {
                                key.clear();
                                w.raw_string(keys.as_ref(), k, i, &mut key)?;
                            }
                            w.json_string(&key, out);
                        } else {
                            w.value(k, keys.as_ref(), i, inner_style, out)?;
                        }
                        out.push(b':');
                        w.value(v, vals.as_ref(), i, inner_style, out)?;
                    }
                    out.push(b'}');
                    Ok(())
                })?;
            }
            ChType::Tuple(elements) => {
                let st = array.as_struct_opt().ok_or_else(|| bad_array(t, array))?;
                if style == Style::Csv {
                    for (i, (_, et)) in elements.iter().enumerate() {
                        if i > 0 {
                            out.push(b',');
                        }
                        self.value(et, st.column(i).as_ref(), row, Style::Csv, out)?;
                    }
                    return Ok(());
                }
                let named = elements.first().is_some_and(|(n, _)| n.is_some());
                if style == Style::Json && named && self.settings.json_named_tuples_as_objects {
                    out.push(b'{');
                    for (i, (name, et)) in elements.iter().enumerate() {
                        if i > 0 {
                            out.push(b',');
                        }
                        self.json_string(name.as_deref().unwrap_or_default().as_bytes(), out);
                        out.push(b':');
                        self.value(et, st.column(i).as_ref(), row, Style::Json, out)?;
                    }
                    out.push(b'}');
                    return Ok(());
                }
                let (open, close) = if style == Style::Json {
                    (b'[', b']')
                } else {
                    (b'(', b')')
                };
                let inner_style = if style == Style::Json {
                    Style::Json
                } else {
                    Style::Quoted
                };
                out.push(open);
                for (i, (_, et)) in elements.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    self.value(et, st.column(i).as_ref(), row, inner_style, out)?;
                }
                out.push(close);
            }
            ChType::Nothing => self.null(style, out),
            other => return Err(ChError::unsupported(format!("Writing values of {other}"))),
        }
        Ok(())
    }

    /// The bytes of a `String` or `FixedString` key, unescaped.
    fn raw_string(
        &self,
        array: &dyn Array,
        t: &ChType,
        row: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ChError> {
        match array.data_type() {
            DataType::Utf8 => out.extend_from_slice(array.as_string::<i32>().value(row).as_bytes()),
            DataType::FixedSizeBinary(_) => {
                out.extend_from_slice(array.as_fixed_size_binary().value(row))
            }
            DataType::Dictionary(..) => {
                let dict = array.as_any_dictionary();
                let key = dict.normalized_keys()[row];
                self.raw_string(dict.values().as_ref(), t, key, out)?;
            }
            _ => return Err(bad_array(t, array)),
        }
        Ok(())
    }

    fn sequence(
        &self,
        inner: &ChType,
        values: &ArrayRef,
        style: Style,
        open: u8,
        close: u8,
        out: &mut Vec<u8>,
    ) -> Result<(), ChError> {
        self.composite(style, out, |w, inner_style, out| {
            out.push(open);
            for i in 0..values.len() {
                if i > 0 {
                    out.push(b',');
                }
                w.value(inner, values.as_ref(), i, inner_style, out)?;
            }
            out.push(close);
            Ok(())
        })
    }

    /// A composite value: JSON writes JSON inside; TSV and Values write the
    /// quoted style; CSV writes the quoted style as one quoted string.
    fn composite(
        &self,
        style: Style,
        out: &mut Vec<u8>,
        body: impl FnOnce(&Self, Style, &mut Vec<u8>) -> Result<(), ChError>,
    ) -> Result<(), ChError> {
        match style {
            Style::Json => body(self, Style::Json, out),
            Style::Tsv | Style::Quoted => body(self, Style::Quoted, out),
            Style::Csv => {
                let mut inner = Vec::new();
                body(self, Style::Quoted, &mut inner)?;
                csv_string(&inner, out);
                Ok(())
            }
        }
    }

    fn int<T: std::fmt::Display>(&self, v: T, wide: bool, style: Style, out: &mut Vec<u8>) {
        let quote = wide && style == Style::Json && self.settings.json_quote_64bit_integers;
        if quote {
            out.push(b'"');
        }
        let _ = write!(Bytes(out), "{v}");
        if quote {
            out.push(b'"');
        }
    }

    fn float(&self, text: String, finite: bool, style: Style, out: &mut Vec<u8>) {
        if style == Style::Json && !finite {
            if self.settings.json_quote_denormals {
                out.push(b'"');
                out.extend_from_slice(text.as_bytes());
                out.push(b'"');
            } else {
                out.extend_from_slice(b"null");
            }
            return;
        }
        out.extend_from_slice(text.as_bytes());
    }

    /// A value ClickHouse writes as text: dates, times, UUIDs, IPs.
    /// A `DateTime` or `DateTime64` value's text under
    /// `date_time_output_format` (ClickHouse's `SerializationDateTime` and
    /// `SerializationDateTime64`): `secs` and `frac` are the floor of the
    /// seconds and the non-negative remainder, as [`datetime_text`] takes them.
    fn datetime(&self, secs: i64, frac: i64, precision: u8, tz: &str) -> Result<String, ChError> {
        match self.settings.date_time_output {
            DateTimeOutput::Simple => datetime_text(secs, frac, precision, tz),
            DateTimeOutput::Iso => datetime_iso_text(secs, frac, precision),
            DateTimeOutput::UnixTimestamp => Ok(unix_timestamp_text(secs, frac, precision)),
        }
    }

    fn text_like(&self, text: &str, style: Style, out: &mut Vec<u8>) {
        match style {
            Style::Tsv => out.extend_from_slice(text.as_bytes()),
            Style::Csv => csv_string(text.as_bytes(), out),
            Style::Json => self.json_string(text.as_bytes(), out),
            Style::Quoted => {
                out.push(b'\'');
                out.extend_from_slice(text.as_bytes());
                out.push(b'\'');
            }
        }
    }

    fn string(&self, bytes: &[u8], style: Style, out: &mut Vec<u8>) {
        match style {
            Style::Tsv => escape(bytes, out),
            Style::Csv => csv_string(bytes, out),
            Style::Json => self.json_string(bytes, out),
            Style::Quoted => {
                out.push(b'\'');
                escape(bytes, out);
                out.push(b'\'');
            }
        }
    }

    fn json_string(&self, bytes: &[u8], out: &mut Vec<u8>) {
        out.push(b'"');
        for &b in bytes {
            match b {
                b'"' => out.extend_from_slice(b"\\\""),
                b'\\' => out.extend_from_slice(b"\\\\"),
                b'/' if self.settings.json_escape_forward_slashes => out.extend_from_slice(b"\\/"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\t' => out.extend_from_slice(b"\\t"),
                b'\r' => out.extend_from_slice(b"\\r"),
                0x08 => out.extend_from_slice(b"\\b"),
                0x0c => out.extend_from_slice(b"\\f"),
                b if b < 0x20 => {
                    let _ = write!(Bytes(out), "\\u{b:04x}");
                }
                b => out.push(b),
            }
        }
        out.push(b'"');
    }
}

/// `std::fmt::Write` into a byte vector.
struct Bytes<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for Bytes<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// ClickHouse's backslash escapes (TSV, and inside single quotes): TSV
/// escapes `'` too (measured: `it's` is `it\'s`).
fn escape(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0 => out.extend_from_slice(b"\\0"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'\'' => out.extend_from_slice(b"\\'"),
            b => out.push(b),
        }
    }
}

fn csv_string(bytes: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in bytes {
        if b == b'"' {
            out.push(b'"');
        }
        out.push(b);
    }
    out.push(b'"');
}

/// A decimal's text from its unscaled integer's digits.
pub fn decimal_text(digits: &str, scale: u8, trailing_zeros: bool) -> String {
    let (negative, digits) = match digits.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, digits),
    };
    let scale = scale as usize;
    let padded = if digits.len() <= scale {
        format!("{}{digits}", "0".repeat(scale + 1 - digits.len()))
    } else {
        digits.to_string()
    };
    let (int, frac) = padded.split_at(padded.len() - scale);
    let frac = if trailing_zeros {
        frac
    } else {
        frac.trim_end_matches('0')
    };
    let mut out = String::new();
    if negative && (!int.trim_start_matches('0').is_empty() || !frac.is_empty()) {
        out.push('-');
    }
    out.push_str(int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

fn uuid_text(b: &[u8]) -> String {
    let mut out = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// `YYYY-MM-DD` of a day count since 1970-01-01.
pub fn date_text(days: i32) -> String {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("the epoch");
    match epoch.checked_add_signed(chrono::TimeDelta::days(i64::from(days))) {
        Some(d) => format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()),
        None => days.to_string(),
    }
}

fn timestamp_value(array: &dyn Array, row: usize) -> Option<(i64, TimeUnit)> {
    Some(match array.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => (
            array.as_primitive::<TimestampSecondType>().value(row),
            TimeUnit::Second,
        ),
        DataType::Timestamp(TimeUnit::Millisecond, _) => (
            array.as_primitive::<TimestampMillisecondType>().value(row),
            TimeUnit::Millisecond,
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => (
            array.as_primitive::<TimestampMicrosecondType>().value(row),
            TimeUnit::Microsecond,
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => (
            array.as_primitive::<TimestampNanosecondType>().value(row),
            TimeUnit::Nanosecond,
        ),
        _ => return None,
    })
}

/// `YYYY-MM-DDThh:mm:ss[.fff…]Z` of `secs` since the epoch, in UTC whatever
/// the value's zone (ClickHouse's `writeDateTimeTextISO`, used by
/// `date_time_output_format = 'iso'`).
pub fn datetime_iso_text(secs: i64, frac: i64, precision: u8) -> Result<String, ChError> {
    let mut text = datetime_text(secs, frac, precision, "UTC")?;
    text.replace_range(10..11, "T");
    text.push('Z');
    Ok(text)
}

/// Seconds since the epoch, then `.` and `precision` digits of the fraction
/// (ClickHouse's `writeDateTimeUnixTimestamp`, used by
/// `date_time_output_format = 'unix_timestamp'`). `secs` is the floor and
/// `frac` the non-negative remainder; ClickHouse splits a negative value
/// towards zero (`-1.5` is `-1` and `.5`), so the parts are turned that way.
pub fn unix_timestamp_text(secs: i64, frac: i64, precision: u8) -> String {
    if precision == 0 {
        return secs.to_string();
    }
    let scale = 10i64.pow(u32::from(precision));
    let (whole, part) = if secs < 0 && frac > 0 {
        (secs + 1, scale - frac)
    } else {
        (secs, frac)
    };
    let sign = if whole == 0 && secs < 0 { "-" } else { "" };
    format!("{sign}{whole}.{part:0width$}", width = precision as usize)
}

/// `YYYY-MM-DD hh:mm:ss[.fff…]` of `secs` since the epoch in zone `tz`, with
/// `precision` digits of `frac`.
pub fn datetime_text(secs: i64, frac: i64, precision: u8, tz: &str) -> Result<String, ChError> {
    let zone =
        Tz::from_str(tz).map_err(|_| ChError::bad_arguments(format!("Unknown time zone {tz}")))?;
    let utc = DateTime::<Utc>::from_timestamp(secs, 0)
        .ok_or_else(|| ChError::argument_out_of_bound(format!("Time {secs} is out of range")))?;
    let local = zone.from_utc_datetime(&utc.naive_utc());
    let mut out = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second()
    );
    if precision > 0 {
        let _ = write!(out, ".{:0width$}", frac, width = precision as usize);
    }
    Ok(out)
}
