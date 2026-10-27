//! Column types, as far as framing needs them: where one column's data ends.
//!
//! A block's columns are written one after another with no length, so finding
//! the next column (to add or remove the per-column serialization byte, or to
//! cut a stream into blocks) means walking each column's data by its type. The
//! walk reads lengths, offsets and dictionary sizes and skips values; it never
//! decodes a value. Layouts follow ClickHouse's `Native` format as the pinned
//! clickhouse-server 26.9.2.8 writes it at the revisions this codec serves (the
//! `String` offsets layout of revision 54492 is not one of them).
//!
//! Types whose layout the walk does not know (`Dynamic`, `Variant`, `Object`,
//! `AggregateFunction`, and `JSON` in any but its string serialization) are
//! refused with `48`, never guessed at.

use crate::Error;
use crate::wire::Reader;

/// What the walk knows of a column type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnType {
    /// A fixed number of bytes per row (integers, floats, dates, `UUID`, IPs,
    /// decimals, enums, `Bool`, `FixedString(N)`).
    Fixed(usize),
    /// `String`: a `VarUInt` length and the bytes, per row.
    String,
    /// `Nothing`: one byte per row.
    Nothing,
    /// `Nullable(T)`: a null map of one byte per row, then `T`.
    Nullable(Box<ColumnType>),
    /// `Array(T)`: cumulative `UInt64` offsets, then the flattened elements.
    Array(Box<ColumnType>),
    /// `Map(K, V)`: `Array(Tuple(K, V))`.
    Map(Box<ColumnType>, Box<ColumnType>),
    /// `Tuple(…)`: each element's column, one after another.
    Tuple(Vec<ColumnType>),
    /// `LowCardinality(T)`: a version prefix, then per granule its index type,
    /// dictionary and keys.
    LowCardinality(Box<ColumnType>),
    /// `JSON(…)`: framed only in its string serialization (prefix version 1,
    /// then a `String` column), what `output_format_native_write_json_as_string`
    /// writes and SigNoz reads; any other serialization is refused.
    Json(String),
    /// A type the walk cannot frame, by name.
    Unsupported(String),
}

/// The dictionary's version in a `LowCardinality` prefix
/// (`SharedDictionariesWithAdditionalKeys`).
const LOW_CARDINALITY_VERSION: u64 = 1;
/// A `JSON` column's serialization version for its string form.
const JSON_AS_STRING: u64 = 1;
/// `LowCardinality` index-type bits.
const NEED_GLOBAL_DICTIONARY: u64 = 1 << 8;
const HAS_ADDITIONAL_KEYS: u64 = 1 << 9;

impl ColumnType {
    /// The type of a ClickHouse type name, as a block header carries it.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let (name, args) = split_type(text)
            .ok_or_else(|| Error::Malformed(format!("'{text}' is not a ClickHouse type name")))?;
        let one = |args: &[&str]| -> Result<Box<ColumnType>, Error> {
            match args {
                [inner] => Ok(Box::new(ColumnType::parse(inner)?)),
                _ => Err(Error::Malformed(format!("'{text}' takes one type"))),
            }
        };
        let fixed = |width: usize| Ok(ColumnType::Fixed(width));
        match (name, args.as_slice()) {
            ("UInt8" | "Int8" | "Bool" | "Enum8", _) => fixed(1),
            ("UInt16" | "Int16" | "Date" | "Enum16" | "BFloat16", _) => fixed(2),
            ("UInt32" | "Int32" | "Float32" | "Date32" | "IPv4" | "Decimal32" | "Time", _) => {
                fixed(4)
            }
            ("DateTime", _) => fixed(4),
            ("UInt64" | "Int64" | "Float64" | "DateTime64" | "Decimal64" | "Time64", _) => fixed(8),
            (interval, []) if interval.starts_with("Interval") => fixed(8),
            ("UInt128" | "Int128" | "UUID" | "IPv6" | "Decimal128", _) => fixed(16),
            ("UInt256" | "Int256" | "Decimal256", _) => fixed(32),
            ("Decimal", [precision, ..]) => {
                let precision: u32 = precision
                    .trim()
                    .parse()
                    .map_err(|_| Error::Malformed(format!("'{text}' has no numeric precision")))?;
                fixed(match precision {
                    0..=9 => 4,
                    10..=18 => 8,
                    19..=38 => 16,
                    _ => 32,
                })
            }
            ("FixedString", [width]) => width
                .trim()
                .parse()
                .map(ColumnType::Fixed)
                .map_err(|_| Error::Malformed(format!("'{text}' has no numeric width"))),
            ("String", []) => Ok(ColumnType::String),
            ("Nothing", []) => Ok(ColumnType::Nothing),
            ("Nullable", args) => one(args).map(ColumnType::Nullable),
            ("Array", args) => one(args).map(ColumnType::Array),
            ("LowCardinality", args) => one(args).map(ColumnType::LowCardinality),
            ("Map", [key, value]) => Ok(ColumnType::Map(
                Box::new(ColumnType::parse(key)?),
                Box::new(ColumnType::parse(value)?),
            )),
            ("Tuple", elements) => elements
                .iter()
                .map(|element| ColumnType::parse(tuple_element_type(element)))
                .collect::<Result<_, _>>()
                .map(ColumnType::Tuple),
            // `SimpleAggregateFunction(f, T)` is stored as `T`.
            ("SimpleAggregateFunction", [_, inner]) => ColumnType::parse(inner),
            ("JSON", _) => Ok(ColumnType::Json(text.trim().to_string())),
            // The geometry aliases.
            ("Point", []) => Ok(point()),
            ("Ring" | "LineString", []) => Ok(ColumnType::Array(Box::new(point()))),
            ("Polygon" | "MultiLineString", []) => Ok(ColumnType::Array(Box::new(
                ColumnType::Array(Box::new(point())),
            ))),
            ("MultiPolygon", []) => Ok(ColumnType::Array(Box::new(ColumnType::Array(Box::new(
                ColumnType::Array(Box::new(point())),
            ))))),
            _ => Ok(ColumnType::Unsupported(text.trim().to_string())),
        }
    }

    /// Skips `rows` rows of this column's data, its serialization prefix first.
    /// Nothing is written for a column of no rows, so nothing is read.
    pub fn skip(&self, reader: &mut Reader<'_>, rows: u64) -> Result<(), Error> {
        if rows == 0 {
            return Ok(());
        }
        self.skip_prefix(reader)?;
        self.skip_data(reader, rows)
    }

    /// The first unsupported type inside this one, if any.
    pub fn unsupported(&self) -> Option<&str> {
        match self {
            ColumnType::Unsupported(name) => Some(name),
            ColumnType::Nullable(inner)
            | ColumnType::Array(inner)
            | ColumnType::LowCardinality(inner) => inner.unsupported(),
            ColumnType::Map(key, value) => key.unsupported().or_else(|| value.unsupported()),
            ColumnType::Tuple(elements) => elements.iter().find_map(ColumnType::unsupported),
            ColumnType::Fixed(_)
            | ColumnType::String
            | ColumnType::Nothing
            | ColumnType::Json(_) => None,
        }
    }

    /// Whether a `LowCardinality` is anywhere inside this type.
    pub fn has_low_cardinality(&self) -> bool {
        match self {
            ColumnType::LowCardinality(_) => true,
            ColumnType::Nullable(inner) | ColumnType::Array(inner) => inner.has_low_cardinality(),
            ColumnType::Map(key, value) => key.has_low_cardinality() || value.has_low_cardinality(),
            ColumnType::Tuple(elements) => elements.iter().any(ColumnType::has_low_cardinality),
            _ => false,
        }
    }

    fn skip_prefix(&self, reader: &mut Reader<'_>) -> Result<(), Error> {
        match self {
            ColumnType::LowCardinality(_) => {
                let version = reader.u64()?;
                if version != LOW_CARDINALITY_VERSION {
                    return Err(Error::Malformed(format!(
                        "a LowCardinality column of dictionary version {version}"
                    )));
                }
                Ok(())
            }
            ColumnType::Nullable(inner) | ColumnType::Array(inner) => inner.skip_prefix(reader),
            ColumnType::Map(key, value) => {
                key.skip_prefix(reader)?;
                value.skip_prefix(reader)
            }
            ColumnType::Tuple(elements) => elements.iter().try_for_each(|e| e.skip_prefix(reader)),
            ColumnType::Json(name) => match reader.u64()? {
                JSON_AS_STRING => Ok(()),
                version => Err(Error::Unsupported(format!(
                    "columns of type {name} in serialization {version} over the native protocol \
                     are not supported by the House yet; read them with \
                     output_format_native_write_json_as_string = 1"
                ))),
            },
            ColumnType::Unsupported(name) => Err(unsupported(name)),
            ColumnType::Fixed(_) | ColumnType::String | ColumnType::Nothing => Ok(()),
        }
    }

    fn skip_data(&self, reader: &mut Reader<'_>, rows: u64) -> Result<(), Error> {
        if rows == 0 {
            return Ok(());
        }
        match self {
            ColumnType::Fixed(width) => reader.skip(times(rows, *width as u64)?),
            ColumnType::Nothing => reader.skip(rows),
            ColumnType::String | ColumnType::Json(_) => {
                for _ in 0..rows {
                    reader.bytes()?;
                }
                Ok(())
            }
            ColumnType::Nullable(inner) => {
                reader.skip(rows)?;
                inner.skip_data(reader, rows)
            }
            ColumnType::Array(inner) => {
                let elements = last_offset(reader, rows)?;
                inner.skip_data(reader, elements)
            }
            ColumnType::Map(key, value) => {
                let entries = last_offset(reader, rows)?;
                key.skip_data(reader, entries)?;
                value.skip_data(reader, entries)
            }
            ColumnType::Tuple(elements) if elements.is_empty() => reader.skip(rows),
            ColumnType::Tuple(elements) => elements
                .iter()
                .try_for_each(|element| element.skip_data(reader, rows)),
            ColumnType::LowCardinality(inner) => {
                // The dictionary holds the inner type's values, never NULL.
                let dictionary = match inner.as_ref() {
                    ColumnType::Nullable(value) => value.as_ref(),
                    other => other,
                };
                let mut done = 0u64;
                while done < rows {
                    let index_type = reader.u64()?;
                    let key_width = index_type & 0xff;
                    if key_width > 3 || index_type & NEED_GLOBAL_DICTIONARY != 0 {
                        return Err(Error::Malformed(format!(
                            "a LowCardinality granule of index type {index_type:#x}"
                        )));
                    }
                    if index_type & HAS_ADDITIONAL_KEYS != 0 {
                        let size = reader.u64()?;
                        dictionary.skip_data(reader, size)?;
                    }
                    let keys = reader.u64()?;
                    if keys == 0 || keys > rows - done {
                        return Err(Error::Malformed(format!(
                            "a LowCardinality granule of {keys} keys where {} rows remain",
                            rows - done
                        )));
                    }
                    reader.skip(times(keys, 1 << key_width)?)?;
                    done += keys;
                }
                Ok(())
            }
            ColumnType::Unsupported(name) => Err(unsupported(name)),
        }
    }
}

fn point() -> ColumnType {
    ColumnType::Tuple(vec![ColumnType::Fixed(8), ColumnType::Fixed(8)])
}

fn unsupported(name: &str) -> Error {
    Error::Unsupported(format!(
        "columns of type {name} over the native protocol are not supported by the House yet"
    ))
}

fn times(rows: u64, width: u64) -> Result<u64, Error> {
    rows.checked_mul(width)
        .ok_or_else(|| Error::Malformed(format!("{rows} rows of {width} bytes")))
}

/// Reads `rows` cumulative offsets and answers the last, the element count.
fn last_offset(reader: &mut Reader<'_>, rows: u64) -> Result<u64, Error> {
    let offsets = reader.take(usize::try_from(times(rows, 8)?).map_err(|_| Error::Incomplete)?)?;
    let mut last = [0u8; 8];
    last.copy_from_slice(&offsets[offsets.len() - 8..]);
    Ok(u64::from_le_bytes(last))
}

/// `Name(arg, …)` into its name and top-level arguments; `None` when `text` is
/// not of that shape.
pub fn split_type(text: &str) -> Option<(&str, Vec<&str>)> {
    let text = text.trim();
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let (name, rest) = text.split_at(end);
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Some((name, Vec::new()));
    }
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    let mut args = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '`' | '"' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            ',' if depth == 0 => {
                args.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 || quote.is_some() {
        return None;
    }
    if !inner.trim().is_empty() {
        args.push(inner[start..].trim());
    }
    Some((name, args))
}

/// A tuple element's type: `T`, `name T` or `` `a name` T ``.
fn tuple_element_type(element: &str) -> &str {
    let element = element.trim();
    if let Some(quoted) = element.strip_prefix('`')
        && let Some(end) = quoted.find('`')
    {
        return quoted[end + 1..].trim();
    }
    if split_type(element).is_some() {
        return element;
    }
    match element.split_once(char::is_whitespace) {
        Some((_, ty)) => ty.trim(),
        None => element,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn skip_all(ty: &str, rows: u64, data: &[u8]) -> Result<usize, Error> {
        let mut reader = Reader::new(data);
        ColumnType::parse(ty)?.skip(&mut reader, rows)?;
        Ok(reader.position())
    }

    #[test]
    fn names_and_arguments() {
        assert_eq!(
            split_type("Enum8('a,b' = 1, 'c)' = 2)"),
            Some(("Enum8", vec!["'a,b' = 1", "'c)' = 2"]))
        );
        assert_eq!(
            ColumnType::parse("Tuple(a UInt8, `b c` String, Nullable(Int64))").expect("parses"),
            ColumnType::Tuple(vec![
                ColumnType::Fixed(1),
                ColumnType::String,
                ColumnType::Nullable(Box::new(ColumnType::Fixed(8)))
            ])
        );
        assert_eq!(
            ColumnType::parse("DateTime64(3, 'Asia/Kolkata')").expect("parses"),
            ColumnType::Fixed(8)
        );
        assert_eq!(
            ColumnType::parse("Decimal(18, 4)").expect("parses"),
            ColumnType::Fixed(8)
        );
        assert_eq!(
            ColumnType::parse("SimpleAggregateFunction(sum, UInt64)").expect("parses"),
            ColumnType::Fixed(8)
        );
        assert_eq!(
            ColumnType::parse("Dynamic").expect("parses").unsupported(),
            Some("Dynamic")
        );
        assert!(ColumnType::parse("Array(").is_err());
    }

    #[test]
    fn columns_from_the_oracle() {
        // SELECT number, toString(number) AS s FROM system.numbers LIMIT 5, as
        // clickhouse-server 26.9.2.8 sends it.
        let numbers =
            hex("00000000000000000100000000000000020000000000000003000000000000000400000000000000");
        assert_eq!(skip_all("UInt64", 5, &numbers).expect("frames"), 40);
        let strings = hex("01300131013201330134");
        assert_eq!(skip_all("String", 5, &strings).expect("frames"), 10);
        assert!(matches!(
            skip_all("String", 6, &strings),
            Err(Error::Incomplete)
        ));
    }

    #[test]
    fn nested_layouts() {
        // Array(Nullable(String)) of [['a', NULL], []]: offsets 2, 2; null map
        // 0 1; values 'a', ''.
        let mut data = Vec::new();
        data.extend_from_slice(&2u64.to_le_bytes());
        data.extend_from_slice(&2u64.to_le_bytes());
        data.extend_from_slice(&[0, 1, 1, b'a', 0]);
        assert_eq!(
            skip_all("Array(Nullable(String))", 2, &data).expect("frames"),
            data.len()
        );
        // LowCardinality(String) of ['x', 'y', 'x']: version, index type 0x600
        // (UInt8 keys, additional keys), dictionary of 3 ('' reserved), keys.
        let mut data = Vec::new();
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(&0x600u64.to_le_bytes());
        data.extend_from_slice(&3u64.to_le_bytes());
        data.extend_from_slice(&[0, 1, b'x', 1, b'y']);
        data.extend_from_slice(&3u64.to_le_bytes());
        data.extend_from_slice(&[1, 2, 1]);
        assert_eq!(
            skip_all("LowCardinality(String)", 3, &data).expect("frames"),
            data.len()
        );
        // Map(String, UInt32) of [{'n': 7}].
        let mut data = Vec::new();
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(&[1, b'n', 7, 0, 0, 0]);
        assert_eq!(
            skip_all("Map(String, UInt32)", 1, &data).expect("frames"),
            data.len()
        );
    }

    #[test]
    fn json_as_strings() {
        // chDB 26.7's `SELECT '{"a":1}'::JSON FORMAT Native` with
        // output_format_native_write_json_as_string = 1, after the header.
        let data = hex("0100000000000000077b2261223a317d");
        assert_eq!(skip_all("JSON", 1, &data).expect("frames"), data.len());
        let mut object = data.clone();
        object[0] = 0;
        assert!(matches!(
            skip_all("JSON(max_dynamic_paths=0)", 1, &object),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn unsupported_types_are_refused_not_guessed() {
        assert!(matches!(
            skip_all("Variant(String, UInt64)", 1, &[0; 64]),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(
            skip_all("Variant(String, UInt64)", 0, &[]).expect("empty"),
            0
        );
    }

    pub(crate) fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }
}
