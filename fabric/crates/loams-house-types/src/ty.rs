//! [`ChType`], a ClickHouse data type, and its canonical text: what
//! `toTypeName` and `system.columns.type` print on the 26.9.2.8 oracle.

use std::fmt::{self, Write as _};

/// A ClickHouse data type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChType {
    /// `UInt8`.
    UInt8,
    /// `UInt16`.
    UInt16,
    /// `UInt32`.
    UInt32,
    /// `UInt64`.
    UInt64,
    /// `UInt128`.
    UInt128,
    /// `UInt256`.
    UInt256,
    /// `Int8`.
    Int8,
    /// `Int16`.
    Int16,
    /// `Int32`.
    Int32,
    /// `Int64`.
    Int64,
    /// `Int128`.
    Int128,
    /// `Int256`.
    Int256,
    /// `Float32`.
    Float32,
    /// `Float64`.
    Float64,
    /// `BFloat16`.
    BFloat16,
    /// `Bool`.
    Bool,
    /// `Decimal(P, S)`; `Decimal32(S)` and the other widths are this with
    /// their precision (9, 18, 38, 76).
    Decimal {
        /// Digits in all, 1..=76.
        precision: u8,
        /// Digits after the point, 0..=precision.
        scale: u8,
    },
    /// `String`.
    String,
    /// `FixedString(N)`.
    FixedString(u32),
    /// `Date`: days since 1970-01-01 as a `UInt16`.
    Date,
    /// `Date32`: days since 1970-01-01 as an `Int32`.
    Date32,
    /// `DateTime[('tz')]`: seconds since the epoch as a `UInt32`.
    DateTime(Option<String>),
    /// `DateTime64(P[, 'tz'])`: ticks of 10^-P seconds since the epoch.
    DateTime64(u8, Option<String>),
    /// `UUID`.
    Uuid,
    /// `IPv4`.
    IPv4,
    /// `IPv6`.
    IPv6,
    /// `Enum8('name' = value, …)`, in declaration order.
    Enum8(Vec<(String, i8)>),
    /// `Enum16('name' = value, …)`, in declaration order.
    Enum16(Vec<(String, i16)>),
    /// `Nullable(T)`.
    Nullable(Box<ChType>),
    /// `LowCardinality(T)`.
    LowCardinality(Box<ChType>),
    /// `Array(T)`.
    Array(Box<ChType>),
    /// `Map(K, V)`.
    Map(Box<ChType>, Box<ChType>),
    /// `Tuple(…)`: every element named, or none.
    Tuple(Vec<(Option<String>, ChType)>),
    /// `Nested(name T, …)`.
    Nested(Vec<(String, ChType)>),
    /// `JSON[(…)]`.
    Json(JsonParams),
    /// `AggregateFunction([version, ]f[(params)], T…)`: a partial aggregation
    /// state.
    AggregateFunction(AggregateSpec),
    /// `SimpleAggregateFunction(f, T)`: a `T` an `AggregatingMergeTree` merges
    /// with `f`.
    SimpleAggregateFunction(AggregateSpec),
    /// `Nothing`.
    Nothing,
    /// `IntervalSecond`, `IntervalDay`, …: the unit's name after `Interval`.
    Interval(String),
    /// A type ClickHouse has and the engine only names (`Dynamic`, `Variant`,
    /// `Time`, `Time64`, geometry, `QBit`): its canonical text, kept so
    /// `toTypeName` and errors print it.
    Other(String),
}

/// The parameters of a `JSON` type, in canonical order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct JsonParams {
    /// `max_dynamic_paths=N`, `max_dynamic_types=N`, as written, in order.
    pub settings: Vec<(String, u64)>,
    /// Typed paths, sorted by path (as ClickHouse prints them).
    pub typed_paths: Vec<(String, ChType)>,
    /// `SKIP path`.
    pub skip_paths: Vec<String>,
    /// `SKIP REGEXP 're'`.
    pub skip_regexps: Vec<String>,
}

/// An aggregate function inside `AggregateFunction` or
/// `SimpleAggregateFunction`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AggregateSpec {
    /// The state's version, when written (`AggregateFunction(1, sumMap, …)`).
    pub version: Option<u32>,
    /// The function, with its combinators (`uniqIf`, `quantilesState`).
    pub function: String,
    /// The function's parameters as ClickHouse prints them (`0.5`, `'x'`).
    pub params: Vec<String>,
    /// The argument types.
    pub args: Vec<ChType>,
}

impl ChType {
    /// `Nullable(T)`'s `T`, or `self`.
    pub fn strip_nullable(&self) -> &ChType {
        match self {
            ChType::Nullable(inner) => inner,
            other => other,
        }
    }

    /// `LowCardinality(T)`'s and `Nullable(T)`'s `T`, recursively.
    pub fn base(&self) -> &ChType {
        match self {
            ChType::Nullable(inner) | ChType::LowCardinality(inner) => inner.base(),
            other => other,
        }
    }

    /// Whether values of this type may be `NULL`.
    pub fn is_nullable(&self) -> bool {
        match self {
            ChType::Nullable(_) | ChType::Nothing => true,
            ChType::LowCardinality(inner) => inner.is_nullable(),
            ChType::SimpleAggregateFunction(spec) => {
                spec.args.first().is_some_and(ChType::is_nullable)
            }
            _ => false,
        }
    }

    /// The canonical text with every named tuple on one line, as ClickHouse
    /// writes types inside other text (`CAST(x, 'T')` in a query it prints).
    pub fn to_compact_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None).expect("writing to a String");
        out
    }

    fn write(&self, out: &mut String, depth: Option<usize>) -> fmt::Result {
        match self {
            ChType::UInt8 => out.push_str("UInt8"),
            ChType::UInt16 => out.push_str("UInt16"),
            ChType::UInt32 => out.push_str("UInt32"),
            ChType::UInt64 => out.push_str("UInt64"),
            ChType::UInt128 => out.push_str("UInt128"),
            ChType::UInt256 => out.push_str("UInt256"),
            ChType::Int8 => out.push_str("Int8"),
            ChType::Int16 => out.push_str("Int16"),
            ChType::Int32 => out.push_str("Int32"),
            ChType::Int64 => out.push_str("Int64"),
            ChType::Int128 => out.push_str("Int128"),
            ChType::Int256 => out.push_str("Int256"),
            ChType::Float32 => out.push_str("Float32"),
            ChType::Float64 => out.push_str("Float64"),
            ChType::BFloat16 => out.push_str("BFloat16"),
            ChType::Bool => out.push_str("Bool"),
            ChType::Decimal { precision, scale } => write!(out, "Decimal({precision}, {scale})")?,
            ChType::String => out.push_str("String"),
            ChType::FixedString(n) => write!(out, "FixedString({n})")?,
            ChType::Date => out.push_str("Date"),
            ChType::Date32 => out.push_str("Date32"),
            ChType::DateTime(None) => out.push_str("DateTime"),
            ChType::DateTime(Some(tz)) => write!(out, "DateTime({})", quote(tz))?,
            ChType::DateTime64(p, None) => write!(out, "DateTime64({p})")?,
            ChType::DateTime64(p, Some(tz)) => write!(out, "DateTime64({p}, {})", quote(tz))?,
            ChType::Uuid => out.push_str("UUID"),
            ChType::IPv4 => out.push_str("IPv4"),
            ChType::IPv6 => out.push_str("IPv6"),
            ChType::Enum8(values) => {
                write_enum(out, "Enum8", values.iter().map(|(n, v)| (n, i64::from(*v))))?
            }
            ChType::Enum16(values) => write_enum(
                out,
                "Enum16",
                values.iter().map(|(n, v)| (n, i64::from(*v))),
            )?,
            ChType::Nullable(inner) => wrap(out, "Nullable", inner, depth)?,
            ChType::LowCardinality(inner) => wrap(out, "LowCardinality", inner, depth)?,
            ChType::Array(inner) => wrap(out, "Array", inner, depth)?,
            ChType::Map(k, v) => {
                out.push_str("Map(");
                k.write(out, depth)?;
                out.push_str(", ");
                v.write(out, depth)?;
                out.push(')');
            }
            ChType::Tuple(elements) => {
                out.push_str("Tuple(");
                let named = elements.first().is_some_and(|(n, _)| n.is_some());
                for (i, (name, t)) in elements.iter().enumerate() {
                    match (named, depth) {
                        (true, Some(d)) => {
                            if i > 0 {
                                out.push(',');
                            }
                            out.push('\n');
                            out.push_str(&"    ".repeat(d + 1));
                        }
                        _ => {
                            if i > 0 {
                                out.push_str(", ");
                            }
                        }
                    }
                    if let Some(name) = name {
                        out.push_str(&quote_identifier(name));
                        out.push(' ');
                    }
                    let inner = if named { depth.map(|d| d + 1) } else { depth };
                    t.write(out, inner)?;
                }
                out.push(')');
            }
            ChType::Nested(elements) => {
                out.push_str("Nested(");
                for (i, (name, t)) in elements.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&quote_identifier(name));
                    out.push(' ');
                    t.write(out, depth)?;
                }
                out.push(')');
            }
            ChType::Json(params) => {
                out.push_str("JSON");
                let mut parts = Vec::new();
                for (name, value) in &params.settings {
                    parts.push(format!("{name}={value}"));
                }
                for (path, t) in &params.typed_paths {
                    let mut text = quote_identifier(path);
                    text.push(' ');
                    t.write(&mut text, None)?;
                    parts.push(text);
                }
                for path in &params.skip_paths {
                    parts.push(format!("SKIP {}", quote_identifier(path)));
                }
                for re in &params.skip_regexps {
                    parts.push(format!("SKIP REGEXP {}", quote(re)));
                }
                if !parts.is_empty() {
                    write!(out, "({})", parts.join(", "))?;
                }
            }
            ChType::AggregateFunction(spec) => {
                write_aggregate(out, "AggregateFunction", spec, depth)?
            }
            ChType::SimpleAggregateFunction(spec) => {
                write_aggregate(out, "SimpleAggregateFunction", spec, depth)?
            }
            ChType::Nothing => out.push_str("Nothing"),
            ChType::Interval(unit) => write!(out, "Interval{unit}")?,
            ChType::Other(text) => out.push_str(text),
        }
        Ok(())
    }
}

/// The canonical text (`toTypeName`): named tuples over several lines, as
/// ClickHouse 26.9 prints them.
impl fmt::Display for ChType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        self.write(&mut out, Some(0))?;
        f.write_str(&out)
    }
}

fn wrap(out: &mut String, name: &str, inner: &ChType, depth: Option<usize>) -> fmt::Result {
    out.push_str(name);
    out.push('(');
    inner.write(out, depth)?;
    out.push(')');
    Ok(())
}

fn write_enum<'a>(
    out: &mut String,
    name: &str,
    values: impl Iterator<Item = (&'a String, i64)>,
) -> fmt::Result {
    out.push_str(name);
    out.push('(');
    for (i, (n, v)) in values.enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{} = {v}", quote(n))?;
    }
    out.push(')');
    Ok(())
}

fn write_aggregate(
    out: &mut String,
    name: &str,
    spec: &AggregateSpec,
    depth: Option<usize>,
) -> fmt::Result {
    out.push_str(name);
    out.push('(');
    if let Some(v) = spec.version {
        write!(out, "{v}, ")?;
    }
    out.push_str(&spec.function);
    if !spec.params.is_empty() {
        write!(out, "({})", spec.params.join(", "))?;
    }
    for t in &spec.args {
        out.push_str(", ");
        t.write(out, depth)?;
    }
    out.push(')');
    Ok(())
}

/// A string literal as ClickHouse quotes one: single quotes, with `\`, `'`
/// and control characters escaped by a backslash.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// An identifier as ClickHouse prints one: bare when it is a plain word
/// (letters, digits and `_`, not starting with a digit), else in backquotes.
pub fn quote_identifier(s: &str) -> String {
    let plain = !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('`');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            c => out.push(c),
        }
    }
    out.push('`');
    out
}
