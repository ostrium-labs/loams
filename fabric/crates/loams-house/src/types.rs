//! ClickHouse types ↔ Iceberg types (HS1 Task 10, §49 §5.3: FL2 Task 5's table,
//! restated for Iceberg).
//!
//! | ClickHouse | Iceberg | ClickHouse type kept |
//! |---|---|---|
//! | `Int8`, `Int16`, `Int32` | `int` | `Int8`/`Int16` |
//! | `UInt8`, `UInt16` | `int` | yes |
//! | `UInt32` | `long` | yes |
//! | `Int64` | `long` | — |
//! | `UInt64` | `decimal(20, 0)` | yes |
//! | `Float32`, `Float64` | `float`, `double` | — |
//! | `BFloat16` | `float` (exact: every `BFloat16` is a `Float32`; HE1 R14.8) | yes |
//! | `Decimal(P, S)`, P ≤ 38 | `decimal(P, S)` | — |
//! | `Bool` | `boolean` | — |
//! | `String` | `string` | — |
//! | `FixedString(N)` | `fixed[N]` | yes |
//! | `Date`, `Date32` | `date` | `Date` |
//! | `DateTime[(tz)]`, `DateTime64(P ≤ 6[, tz])` | `timestamptz` | yes |
//! | `DateTime64(7..9[, tz])` | `long`, nanoseconds since the epoch (a column's own type; inside an `Array`, `Map` or `Tuple`, `timestamptz_ns`, format v3) | yes |
//! | `UUID` | `uuid` | — |
//! | `IPv4`, `IPv6`, `Enum8(…)`, `Enum16(…)` | `string` | yes |
//! | `LowCardinality(T)` | `T` | yes |
//! | `Nullable(T)` | `T`, optional | — |
//! | `Array(T)` | `list<T>` | — |
//! | `Map(K, V)` | `map<K, V>` | — |
//! | `Tuple(a T, …)` | `struct<a: T, …>` (unnamed elements are `1`, `2`, …) | yes when unnamed |
//! | `JSON[(…)]` (a column's own type) | `string`, the value's JSON text (interim, HE1 R9.63; Variant in HE4) | yes |
//!
//! A column that is not `Nullable` is a **required** Iceberg field. Every other
//! ClickHouse type (`JSON`, `Object`, `Variant`, `Dynamic`, `AggregateFunction`,
//! `SimpleAggregateFunction`, `Nested`, the 128- and 256-bit integers, `Decimal`
//! over 38 digits, `Time`, `Time64`, intervals, geometry) is `48
//! NOT_IMPLEMENTED`. ClickHouse's canonical type text (what `DESCRIBE` prints) is
//! kept for every column in the table property `loams.ch.types`, so `DESCRIBE`
//! and the read views (HS1 Task 11) restore it exactly; "kept" above marks the
//! types Iceberg alone could not give back.
//!
//! [`from_iceberg`] is the other direction, for tables another writer made
//! (`external`, §49 §5.2): the types chDB's Iceberg reader gives them (R1.3), with
//! `Nullable` for every optional primitive.

use loams_house_lake::spec::{
    FormatVersion, ListType, MapType, NestedField, NestedFieldRef, PrimitiveType, StructType, Type,
};

use crate::errors::{ChError, HouseError};

/// A ClickHouse type, parsed from its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChType {
    /// A name with no arguments (`UInt8`, `String`, `Date`).
    Simple(String),
    /// `Nullable(T)`.
    Nullable(Box<ChType>),
    /// `LowCardinality(T)`.
    LowCardinality(Box<ChType>),
    /// `Array(T)`.
    Array(Box<ChType>),
    /// `Map(K, V)`.
    Map(Box<ChType>, Box<ChType>),
    /// `Tuple(…)`, its elements named or not.
    Tuple(Vec<(Option<String>, ChType)>),
    /// `Decimal(P, S)`.
    Decimal(u32, u32),
    /// `FixedString(N)`.
    FixedString(u32),
    /// `DateTime[(tz)]`.
    DateTime(Option<String>),
    /// `DateTime64(P[, tz])`.
    DateTime64(u32, Option<String>),
    /// `Enum8(…)` or `Enum16(…)`, as written.
    Enum(String),
    /// `SimpleAggregateFunction(f, T)`: a `T` that an `AggregatingMergeTree`
    /// merges with `f` (HE1 R9.14); `f` as written.
    SimpleAggregate(String, Box<ChType>),
    /// `AggregateFunction(f, T…)`: a partial aggregation state (HE1 R9.14),
    /// stored as its serialized bytes.
    AggregateState,
    /// Anything else with arguments, by name.
    Other(String),
}

/// `48` for a type the House does not store.
fn unsupported(text: &str, why: &str) -> HouseError {
    HouseError::from(ChError::not_implemented(format!(
        "Type {text} is not supported in lake tables ({why})"
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Quoted(String),
    Number(String),
    Symbol(char),
}

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit()
            || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            out.push(Tok::Number(chars[start..i].iter().collect()));
        } else if c == '\'' || c == '`' || c == '"' {
            let quote = c;
            let mut value = String::new();
            i += 1;
            loop {
                let Some(&ch) = chars.get(i) else {
                    return Err("an unterminated quote".to_string());
                };
                i += 1;
                if ch == '\\' {
                    if let Some(&next) = chars.get(i) {
                        value.push(next);
                        i += 1;
                    }
                } else if ch == quote {
                    if chars.get(i) == Some(&quote) {
                        value.push(quote);
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    value.push(ch);
                }
            }
            out.push(if quote == '\'' {
                Tok::Quoted(value)
            } else {
                Tok::Word(value)
            });
        } else {
            out.push(Tok::Symbol(c));
            i += 1;
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
    text: &'a str,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn peek2(&self) -> Option<&Tok> {
        self.toks.get(self.at + 1)
    }

    fn next(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.at).cloned();
        self.at += 1;
        tok
    }

    fn expect(&mut self, symbol: char) -> Result<(), String> {
        match self.next() {
            Some(Tok::Symbol(c)) if c == symbol => Ok(()),
            other => Err(format!("expected {symbol:?}, found {other:?}")),
        }
    }

    fn number(&mut self) -> Result<u32, String> {
        match self.next() {
            Some(Tok::Number(n)) => n.parse().map_err(|_| format!("bad number {n}")),
            other => Err(format!("expected a number, found {other:?}")),
        }
    }

    /// Skips balanced arguments, the opening `(` already read.
    fn skip_args(&mut self) -> Result<(), String> {
        let mut depth = 1;
        while depth > 0 {
            match self.next() {
                Some(Tok::Symbol('(')) => depth += 1,
                Some(Tok::Symbol(')')) => depth -= 1,
                Some(_) => {}
                None => return Err("unbalanced parentheses".to_string()),
            }
        }
        Ok(())
    }

    fn ty(&mut self) -> Result<ChType, String> {
        let name = match self.next() {
            Some(Tok::Word(w)) => w,
            other => return Err(format!("expected a type, found {other:?}")),
        };
        if self.peek() != Some(&Tok::Symbol('(')) {
            return Ok(match name.as_str() {
                "DateTime" => ChType::DateTime(None),
                _ => ChType::Simple(name),
            });
        }
        let open = self.at;
        self.next();
        let one = |p: &mut Self| -> Result<Box<ChType>, String> {
            let inner = p.ty()?;
            p.expect(')')?;
            Ok(Box::new(inner))
        };
        Ok(match name.as_str() {
            "Nullable" => ChType::Nullable(one(self)?),
            "LowCardinality" => ChType::LowCardinality(one(self)?),
            "Array" => ChType::Array(one(self)?),
            "Map" => {
                let key = self.ty()?;
                self.expect(',')?;
                let value = self.ty()?;
                self.expect(')')?;
                ChType::Map(Box::new(key), Box::new(value))
            }
            "Tuple" => {
                let mut elements = Vec::new();
                loop {
                    // `name Type`: a name is a word followed by another word.
                    let named = matches!(
                        (self.peek(), self.peek2()),
                        (Some(Tok::Word(_)), Some(Tok::Word(_)))
                    );
                    let element_name = if named {
                        match self.next() {
                            Some(Tok::Word(w)) => Some(w),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    elements.push((element_name, self.ty()?));
                    match self.next() {
                        Some(Tok::Symbol(',')) => continue,
                        Some(Tok::Symbol(')')) => break,
                        other => return Err(format!("expected , or ), found {other:?}")),
                    }
                }
                ChType::Tuple(elements)
            }
            "Decimal" => {
                let precision = self.number()?;
                self.expect(',')?;
                let scale = self.number()?;
                self.expect(')')?;
                ChType::Decimal(precision, scale)
            }
            "FixedString" => {
                let n = self.number()?;
                self.expect(')')?;
                ChType::FixedString(n)
            }
            "DateTime" => {
                let zone = match self.next() {
                    Some(Tok::Quoted(z)) => z,
                    other => return Err(format!("expected a time zone, found {other:?}")),
                };
                self.expect(')')?;
                ChType::DateTime(Some(zone))
            }
            "DateTime64" => {
                let precision = self.number()?;
                let zone = match self.next() {
                    Some(Tok::Symbol(',')) => {
                        let zone = match self.next() {
                            Some(Tok::Quoted(z)) => z,
                            other => return Err(format!("expected a time zone, found {other:?}")),
                        };
                        self.expect(')')?;
                        Some(zone)
                    }
                    Some(Tok::Symbol(')')) => None,
                    other => return Err(format!("expected , or ), found {other:?}")),
                };
                ChType::DateTime64(precision, zone)
            }
            "Enum8" | "Enum16" => {
                self.skip_args()?;
                ChType::Enum(name)
            }
            "SimpleAggregateFunction" => {
                // The function (maybe parametric: `quantiles(0.5)`) runs to the
                // first top-level comma.
                let mut function = String::new();
                let mut depth = 0;
                loop {
                    match self.next() {
                        Some(Tok::Symbol(',')) if depth == 0 => break,
                        Some(Tok::Symbol('(')) => {
                            depth += 1;
                            function.push('(');
                        }
                        Some(Tok::Symbol(')')) => {
                            depth -= 1;
                            function.push(')');
                        }
                        Some(Tok::Word(w)) | Some(Tok::Number(w)) => function.push_str(&w),
                        Some(Tok::Quoted(q)) => function.push_str(&format!("'{q}'")),
                        Some(Tok::Symbol(c)) => function.push(c),
                        None => return Err("unbalanced parentheses".to_string()),
                    }
                }
                let inner = self.ty()?;
                self.expect(')')?;
                ChType::SimpleAggregate(function, Box::new(inner))
            }
            "AggregateFunction" => {
                self.skip_args()?;
                ChType::AggregateState
            }
            _ => {
                self.skip_args()?;
                let _ = open;
                ChType::Other(name)
            }
        })
    }
}

impl ChType {
    /// Parses a type's text (ClickHouse's canonical form, as `DESCRIBE` prints
    /// it); `62` when it is not a type.
    pub fn parse(text: &str) -> Result<Self, HouseError> {
        let bad = |why: String| {
            HouseError::from(ChError::syntax_error(format!(
                "Cannot read the type {text}: {why}"
            )))
        };
        let toks = lex(text).map_err(bad)?;
        let mut parser = Parser {
            toks: &toks,
            at: 0,
            text,
        };
        let ty = parser.ty().map_err(bad)?;
        if parser.at != toks.len() {
            return Err(bad(format!(
                "unexpected text after the type in {}",
                parser.text
            )));
        }
        Ok(ty)
    }

    /// Whether it is `Nullable(…)` (also inside `LowCardinality`).
    pub fn is_nullable(&self) -> bool {
        match self {
            Self::Nullable(_) => true,
            Self::LowCardinality(inner) | Self::SimpleAggregate(_, inner) => inner.is_nullable(),
            _ => false,
        }
    }

    /// The type with `Nullable` and `LowCardinality` taken off.
    pub fn base(&self) -> &ChType {
        match self {
            Self::Nullable(inner) | Self::LowCardinality(inner) => inner.base(),
            other => other,
        }
    }

    /// Whether this is ClickHouse's `JSON` type, with or without parameters
    /// (`JSON(max_dynamic_paths = 100, message String)`).
    pub fn is_json(&self) -> bool {
        matches!(self, Self::Simple(name) | Self::Other(name) if name == "JSON")
    }

    /// For a column whose values are `DateTime64` with more than 6 digits (its
    /// own type, under `Nullable` or `SimpleAggregateFunction`), the time zone
    /// it was declared with: such a column is stored as nanoseconds in an
    /// Iceberg `long` (D1572, HE1 R9.61). `None` for any other type.
    pub fn nanos_zone(&self) -> Option<Option<&str>> {
        match self {
            Self::Nullable(inner)
            | Self::LowCardinality(inner)
            | Self::SimpleAggregate(_, inner) => inner.nanos_zone(),
            Self::DateTime64(precision, zone) if (7..=9).contains(precision) => {
                Some(zone.as_deref())
            }
            _ => None,
        }
    }
}

/// Field ids for a schema being built: Iceberg assigns fresh ids at create, so
/// these need only be unique.
#[derive(Debug)]
pub struct FieldIds(i32);

impl FieldIds {
    /// Ids after `last` (the table's `last-column-id` for a schema change).
    pub fn after(last: i32) -> Self {
        Self(last)
    }

    /// The next id.
    pub fn fresh(&mut self) -> i32 {
        self.0 += 1;
        self.0
    }
}

/// What a column needs of the table: a format version of at least this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Needs {
    /// Format v2.
    V2,
    /// Format v3 (`timestamptz_ns`).
    V3,
}

impl Needs {
    /// The Iceberg format version.
    pub fn format_version(self) -> FormatVersion {
        match self {
            Self::V2 => FormatVersion::V2,
            Self::V3 => FormatVersion::V3,
        }
    }
}

/// A column's Iceberg field: the type of `ch_text` (ClickHouse's), required
/// unless it is `Nullable` (or `force_optional`, for a column added to a table
/// with rows: Iceberg v2 cannot add a required field).
pub fn to_iceberg_field(
    name: &str,
    ch_text: &str,
    ids: &mut FieldIds,
    force_optional: bool,
    needs: &mut Needs,
) -> Result<NestedFieldRef, HouseError> {
    let ch = ChType::parse(ch_text)?;
    let id = ids.fresh();
    // D1572, HE1 R9.61: a column of nanosecond times is a `long` of
    // nanoseconds since the epoch while the House writes format v2 (D1551);
    // `loams.ch.types` keeps its type, and the read views and the write path
    // convert ([`ChType::nanos_zone`]). `timestamptz_ns` once v3 is written
    // (HE2, D1553).
    let ty = if ch.nanos_zone().is_some() {
        primitive(PrimitiveType::Long)
    } else if ch.base().is_json() {
        // HE1 R9.63 (interim; Iceberg v3's Variant in HE4): a `JSON` column is
        // an Iceberg `string` of the value's JSON text, which the write path
        // gives with `CAST(… AS String)`; `loams.ch.types` keeps its type, and
        // the read views cast the text back to it, so its subcolumns
        // (`resource.service.name`) and typed paths read as ClickHouse reads
        // them.
        primitive(PrimitiveType::String)
    } else {
        to_iceberg(&ch, ch_text, ids, needs)?
    };
    let required = !ch.is_nullable() && !force_optional;
    Ok(if required {
        NestedField::required(id, name, ty).into()
    } else {
        NestedField::optional(id, name, ty).into()
    })
}

fn primitive(p: PrimitiveType) -> Type {
    Type::Primitive(p)
}

/// The Iceberg type of a ClickHouse type (nullability is the field's).
pub fn to_iceberg(
    ch: &ChType,
    text: &str,
    ids: &mut FieldIds,
    needs: &mut Needs,
) -> Result<Type, HouseError> {
    Ok(match ch {
        ChType::Nullable(inner) => {
            if matches!(**inner, ChType::Nullable(_)) {
                return Err(unsupported(text, "nested Nullable"));
            }
            to_iceberg(inner, text, ids, needs)?
        }
        ChType::LowCardinality(inner) => to_iceberg(inner, text, ids, needs)?,
        ChType::Simple(name) => match name.as_str() {
            "Int8" | "Int16" | "Int32" | "UInt8" | "UInt16" => primitive(PrimitiveType::Int),
            "UInt32" | "Int64" => primitive(PrimitiveType::Long),
            "UInt64" => primitive(PrimitiveType::Decimal {
                precision: 20,
                scale: 0,
            }),
            "Float32" => primitive(PrimitiveType::Float),
            // HE1 R14.8: a `BFloat16` is a `Float32` with the low 16 bits of
            // its mantissa zero, so `float` holds it exactly; the write path
            // casts it to `Float32` and the read views back (Laminar's
            // `Array(BFloat16)` embeddings).
            "BFloat16" => primitive(PrimitiveType::Float),
            "Float64" => primitive(PrimitiveType::Double),
            "Bool" => primitive(PrimitiveType::Boolean),
            "String" | "IPv4" | "IPv6" => primitive(PrimitiveType::String),
            "Date" | "Date32" => primitive(PrimitiveType::Date),
            "UUID" => primitive(PrimitiveType::Uuid),
            "Int128" | "Int256" | "UInt128" | "UInt256" => {
                return Err(unsupported(text, "wider than Iceberg's 38-digit decimal"));
            }
            other => {
                return Err(unsupported(
                    text,
                    &format!("{other} has no Iceberg counterpart in chsurface"),
                ));
            }
        },
        ChType::Decimal(precision, scale) => {
            if *precision > 38 {
                return Err(unsupported(text, "Iceberg decimals hold at most 38 digits"));
            }
            primitive(PrimitiveType::Decimal {
                precision: *precision,
                scale: *scale,
            })
        }
        ChType::FixedString(n) => primitive(PrimitiveType::Fixed(u64::from(*n))),
        ChType::DateTime(_) => primitive(PrimitiveType::Timestamptz),
        ChType::DateTime64(precision, _) => match precision {
            0..=6 => primitive(PrimitiveType::Timestamptz),
            7..=9 => {
                *needs = (*needs).max(Needs::V3);
                primitive(PrimitiveType::TimestamptzNs)
            }
            _ => return Err(unsupported(text, "a precision over 9")),
        },
        ChType::Enum(_) => primitive(PrimitiveType::String),
        ChType::Array(element) => {
            let id = ids.fresh();
            let ty = to_iceberg(element, text, ids, needs)?;
            let field = if element.is_nullable() {
                NestedField::list_element(id, ty, false)
            } else {
                NestedField::list_element(id, ty, true)
            };
            Type::List(ListType {
                element_field: field.into(),
            })
        }
        ChType::Map(key, value) => {
            if key.is_nullable() {
                return Err(unsupported(text, "a Nullable map key"));
            }
            let key_id = ids.fresh();
            let key_ty = to_iceberg(key, text, ids, needs)?;
            let value_id = ids.fresh();
            let value_ty = to_iceberg(value, text, ids, needs)?;
            Type::Map(MapType {
                key_field: NestedField::map_key_element(key_id, key_ty).into(),
                value_field: NestedField::map_value_element(
                    value_id,
                    value_ty,
                    !value.is_nullable(),
                )
                .into(),
            })
        }
        ChType::Tuple(elements) => {
            let mut fields = Vec::with_capacity(elements.len());
            for (at, (name, element)) in elements.iter().enumerate() {
                let id = ids.fresh();
                let ty = to_iceberg(element, text, ids, needs)?;
                let name = name.clone().unwrap_or_else(|| (at + 1).to_string());
                fields.push(if element.is_nullable() {
                    NestedField::optional(id, name, ty).into()
                } else {
                    NestedField::required(id, name, ty).into()
                });
            }
            Type::Struct(StructType::new(fields))
        }
        // HE1 R9.14: an `AggregatingMergeTree`'s columns. A simple one stores its
        // value; a state stores ClickHouse's serialization of it (what
        // `CAST(state AS String)` gives and `CAST(bytes AS AggregateFunction(…))`
        // reads back).
        ChType::SimpleAggregate(_, inner) => to_iceberg(inner, text, ids, needs)?,
        ChType::AggregateState => primitive(PrimitiveType::Binary),
        ChType::Other(name) => {
            return Err(unsupported(
                text,
                &format!("{name} has no Iceberg counterpart in chsurface"),
            ));
        }
    })
}

/// The ClickHouse type chDB reads an Iceberg field as (R1.3): optional primitives
/// are `Nullable`.
pub fn from_iceberg(ty: &Type, required: bool) -> String {
    let base = match ty {
        Type::Primitive(p) => match p {
            PrimitiveType::Boolean => "Bool".to_string(),
            PrimitiveType::Int => "Int32".to_string(),
            PrimitiveType::Long => "Int64".to_string(),
            PrimitiveType::Float => "Float32".to_string(),
            PrimitiveType::Double => "Float64".to_string(),
            PrimitiveType::Decimal { precision, scale } => {
                format!("Decimal({precision}, {scale})")
            }
            PrimitiveType::Date => "Date32".to_string(),
            PrimitiveType::Time => "Int64".to_string(),
            PrimitiveType::Timestamp => "DateTime64(6)".to_string(),
            PrimitiveType::Timestamptz => "DateTime64(6, 'UTC')".to_string(),
            PrimitiveType::TimestampNs => "DateTime64(9)".to_string(),
            PrimitiveType::TimestamptzNs => "DateTime64(9, 'UTC')".to_string(),
            PrimitiveType::String | PrimitiveType::Binary => "String".to_string(),
            PrimitiveType::Uuid => "UUID".to_string(),
            PrimitiveType::Fixed(n) => format!("FixedString({n})"),
        },
        Type::List(list) => {
            return format!(
                "Array({})",
                from_iceberg(&list.element_field.field_type, list.element_field.required)
            );
        }
        Type::Map(map) => {
            return format!(
                "Map({}, {})",
                from_iceberg(&map.key_field.field_type, true),
                from_iceberg(&map.value_field.field_type, map.value_field.required)
            );
        }
        Type::Struct(fields) => {
            let elements: Vec<String> = fields
                .fields()
                .iter()
                .map(|f| {
                    format!(
                        "{} {}",
                        crate::ddl::quote_name(&f.name),
                        from_iceberg(&f.field_type, f.required)
                    )
                })
                .collect();
            return format!("Tuple({})", elements.join(", "));
        }
    };
    if required {
        base
    } else {
        format!("Nullable({base})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iceberg(text: &str) -> (Type, bool, Needs) {
        let mut needs = Needs::V2;
        let field = to_iceberg_field("c", text, &mut FieldIds::after(0), false, &mut needs)
            .unwrap_or_else(|err| panic!("{text}: {err}"));
        (field.field_type.as_ref().clone(), field.required, needs)
    }

    #[test]
    fn types_parse() {
        assert_eq!(
            ChType::parse("Map(String, Nullable(Decimal(10, 2)))").expect("parses"),
            ChType::Map(
                Box::new(ChType::Simple("String".into())),
                Box::new(ChType::Nullable(Box::new(ChType::Decimal(10, 2))))
            )
        );
        assert_eq!(
            ChType::parse("Tuple(\n    x UInt8,\n    `y z` String)").expect("parses"),
            ChType::Tuple(vec![
                (Some("x".into()), ChType::Simple("UInt8".into())),
                (Some("y z".into()), ChType::Simple("String".into())),
            ])
        );
        assert_eq!(
            ChType::parse("Tuple(UInt8, Nullable(String))").expect("parses"),
            ChType::Tuple(vec![
                (None, ChType::Simple("UInt8".into())),
                (
                    None,
                    ChType::Nullable(Box::new(ChType::Simple("String".into())))
                ),
            ])
        );
        assert_eq!(
            ChType::parse("DateTime64(3, 'Asia/Kolkata')").expect("parses"),
            ChType::DateTime64(3, Some("Asia/Kolkata".into()))
        );
        assert_eq!(
            ChType::parse("Enum8('a' = 1, 'b(' = -2)").expect("parses"),
            ChType::Enum("Enum8".into())
        );
        assert!(ChType::parse("Array(").is_err());
        assert!(ChType::parse("UInt8 UInt8").is_err());
    }

    #[test]
    fn the_table_maps_both_ways() {
        let long = Type::Primitive(PrimitiveType::Long);
        for (ch, ty, back) in [
            ("UInt8", Type::Primitive(PrimitiveType::Int), "Int32"),
            ("UInt32", long.clone(), "Int64"),
            ("Int64", long.clone(), "Int64"),
            (
                "UInt64",
                Type::Primitive(PrimitiveType::Decimal {
                    precision: 20,
                    scale: 0,
                }),
                "Decimal(20, 0)",
            ),
            ("Float32", Type::Primitive(PrimitiveType::Float), "Float32"),
            ("BFloat16", Type::Primitive(PrimitiveType::Float), "Float32"),
            ("Bool", Type::Primitive(PrimitiveType::Boolean), "Bool"),
            ("Date", Type::Primitive(PrimitiveType::Date), "Date32"),
            ("UUID", Type::Primitive(PrimitiveType::Uuid), "UUID"),
            (
                "DateTime('UTC')",
                Type::Primitive(PrimitiveType::Timestamptz),
                "DateTime64(6, 'UTC')",
            ),
            (
                "FixedString(4)",
                Type::Primitive(PrimitiveType::Fixed(4)),
                "FixedString(4)",
            ),
            (
                "LowCardinality(String)",
                Type::Primitive(PrimitiveType::String),
                "String",
            ),
            (
                "Enum8('a' = 1)",
                Type::Primitive(PrimitiveType::String),
                "String",
            ),
        ] {
            let (got, required, _) = iceberg(ch);
            assert_eq!(got, ty, "{ch}");
            assert!(required, "{ch} is not Nullable");
            assert_eq!(from_iceberg(&got, true), back, "{ch} back");
        }
        let (ty, required, _) = iceberg("Nullable(String)");
        assert!(!required);
        assert_eq!(from_iceberg(&ty, required), "Nullable(String)");
        // D1572, HE1 R9.61: a column of nanosecond times is a long
        // of nanoseconds in a v2 table; inside a container, v3's type.
        for text in [
            "DateTime64(9, 'UTC')",
            "DateTime64(7)",
            "Nullable(DateTime64(9))",
            "SimpleAggregateFunction(max, DateTime64(9))",
        ] {
            let (ty, _, needs) = iceberg(text);
            assert_eq!(ty, Type::Primitive(PrimitiveType::Long), "{text}");
            assert_eq!(needs, Needs::V2, "{text}");
        }
        assert_eq!(
            ChType::parse("Nullable(DateTime64(9, 'UTC'))")
                .expect("parses")
                .nanos_zone(),
            Some(Some("UTC"))
        );
        assert_eq!(
            ChType::parse("DateTime64(6)").expect("parses").nanos_zone(),
            None
        );
        let (_, _, needs) = iceberg("Array(DateTime64(9, 'UTC'))");
        assert_eq!(needs, Needs::V3);
        let (ty, required, _) = iceberg("Array(Nullable(Int64))");
        assert_eq!(from_iceberg(&ty, required), "Array(Nullable(Int64))");
        let (ty, required, _) = iceberg("Map(String, Array(UInt8))");
        assert_eq!(from_iceberg(&ty, required), "Map(String, Array(Int32))");
        let (ty, required, _) = iceberg("Tuple(a UInt8, b Nullable(String))");
        assert_eq!(
            from_iceberg(&ty, required),
            "Tuple(a Int32, b Nullable(String))"
        );
    }

    /// HE1 R9.14: an `AggregatingMergeTree`'s columns.
    #[test]
    fn aggregate_columns_are_stored() {
        let mut needs = Needs::V2;
        let field = |text: &str, needs: &mut Needs| {
            to_iceberg_field("c", text, &mut FieldIds::after(0), false, needs).expect(text)
        };
        let state = field(
            "AggregateFunction(argMax, Nullable(Bool), DateTime64(3))",
            &mut needs,
        );
        assert_eq!(*state.field_type, Type::Primitive(PrimitiveType::Binary));
        assert!(state.required);
        let simple = field(
            "SimpleAggregateFunction(maxMap, Map(String, String))",
            &mut needs,
        );
        assert!(matches!(*simple.field_type, Type::Map(_)));
        let simple = field(
            "SimpleAggregateFunction(anyLast, Nullable(String))",
            &mut needs,
        );
        assert_eq!(*simple.field_type, Type::Primitive(PrimitiveType::String));
        assert!(!simple.required, "Nullable inside stays optional");
        assert_eq!(
            ChType::parse("SimpleAggregateFunction(quantiles(0.5, 0.9), Float64)").expect("parses"),
            ChType::SimpleAggregate(
                "quantiles(0.5,0.9)".into(),
                Box::new(ChType::Simple("Float64".into()))
            )
        );
    }

    #[test]
    fn unsupported_types_are_48() {
        // HE1 R9.63: `JSON` is stored as its text only as a column's own type.
        for text in ["JSON", "JSON(max_dynamic_paths=0, message String)"] {
            let (ty, required, needs) = iceberg(text);
            assert_eq!(ty, Type::Primitive(PrimitiveType::String), "{text}");
            assert!(required, "{text}");
            assert_eq!(needs, Needs::V2, "{text}");
        }
        for text in [
            "Array(JSON)",
            "Dynamic",
            "Variant(UInt8, String)",
            "Nested(a UInt8)",
            "Int128",
            "UInt256",
            "Decimal(76, 2)",
            "Time",
            "IntervalDay",
            "Point",
        ] {
            let mut needs = Needs::V2;
            let err = to_iceberg_field("c", text, &mut FieldIds::after(0), false, &mut needs)
                .expect_err(text);
            assert_eq!(err.code(), 48, "{text}: {err}");
        }
    }
}
