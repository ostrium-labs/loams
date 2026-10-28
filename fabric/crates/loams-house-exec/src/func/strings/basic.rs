//! `length`, `lengthUTF8`, `concat`, slices (`left`, `right`, `substring`
//! and their `UTF8` forms), `startsWith`/`endsWith`, `ascii` and the `trim`
//! family. Slicing follows ClickHouse's `substring`
//! (`src/Functions/substring.cpp`, `GatherUtils`), measured on the oracle:
//! offset 0 is empty, a negative offset counts from the end, a negative
//! length drops characters from the end, and windows are clipped to the
//! string.

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, Int32Builder, StringBuilder, UInt8Builder, UInt64Builder,
};
use loams_house_sql::Registry;
use loams_house_types::text::{Style, TextSettings, write_value};
use loams_house_types::{ChError, ChType};

use crate::func::chudf::{
    ChFunction, check_arity, illegal_type, list_parts, not_utf8, want_integer, want_string,
};
use crate::func::strings::case::map_strings;

/// The byte range `[start, end)` that ClickHouse's `substring(s, offset,
/// length)` takes from a string of `len` units.
pub(crate) fn window(len: usize, offset: i128, length: Option<i128>) -> (usize, usize) {
    let n = len as i128;
    let start = match offset {
        0 => return (0, 0),
        o if o > 0 => o - 1,
        o => n + o,
    };
    let end = match length {
        None => n,
        Some(l) if l >= 0 => start.saturating_add(l),
        Some(l) => n + l,
    };
    let (s, e) = (start.clamp(0, n), end.clamp(0, n));
    if e <= s {
        (0, 0)
    } else {
        (s as usize, e as usize)
    }
}

/// The `[start, end)` window counted in characters, as byte offsets.
fn char_window(s: &str, offset: i128, length: Option<i128>) -> &str {
    let bounds: Vec<usize> = s
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .collect();
    let (a, b) = window(bounds.len() - 1, offset, length);
    &s[bounds[a]..bounds[b]]
}

/// Slices strings: `offset` and `length` from the arguments.
fn slicer(name: &'static str, utf8: bool, plan: fn(i128) -> (i128, Option<i128>)) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 2, 2)?;
            want_string(name, args, 0)?;
            want_integer(name, args, 1)?;
            Ok(ChType::String)
        }),
        Arc::new(move |call| {
            let s = call.strs(0)?;
            let n = call.ints(1)?;
            let mut out = StringBuilder::with_capacity(call.rows, 0);
            for row in 0..call.rows {
                match (s.get(row), n.get(row)) {
                    (Some(v), Some(k)) => {
                        let (offset, length) = plan(k);
                        out.append_value(slice(name, v, utf8, offset, length)?);
                    }
                    _ => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn slice<'a>(
    name: &str,
    v: &'a str,
    utf8: bool,
    offset: i128,
    length: Option<i128>,
) -> Result<&'a str, ChError> {
    if utf8 {
        return Ok(char_window(v, offset, length));
    }
    let (a, b) = window(v.len(), offset, length);
    v.get(a..b).ok_or_else(|| not_utf8(name))
}

fn substring(name: &'static str, utf8: bool) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 2, 3)?;
            want_string(name, args, 0)?;
            want_integer(name, args, 1)?;
            want_integer(name, args, 2)?;
            Ok(ChType::String)
        }),
        Arc::new(move |call| {
            let s = call.strs(0)?;
            let offset = call.ints(1)?;
            let length = if call.args.len() > 2 {
                Some(call.ints(2)?)
            } else {
                None
            };
            let mut out = StringBuilder::with_capacity(call.rows, 0);
            for row in 0..call.rows {
                let len = match &length {
                    Some(l) => match l.get(row) {
                        Some(v) => Some(Some(v)),
                        None => None,
                    },
                    None => Some(None),
                };
                match (s.get(row), offset.get(row), len) {
                    (Some(v), Some(o), Some(l)) => out.append_value(slice(name, v, utf8, o, l)?),
                    _ => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn length(name: &'static str, utf8: bool) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 1, 1)?;
            match &args[0] {
                ChType::String | ChType::FixedString(_) => Ok(ChType::UInt64),
                ChType::Array(_) | ChType::Map(..) if !utf8 => Ok(ChType::UInt64),
                t => Err(illegal_type(name, 0, t)),
            }
        }),
        Arc::new(move |call| {
            let mut out = UInt64Builder::with_capacity(call.rows);
            if matches!(call.types[0], ChType::Array(_) | ChType::Map(..)) {
                let a = call.array(0)?;
                let (offsets, _) = list_parts(a.as_ref())
                    .ok_or_else(|| illegal_type(call.name, 0, &call.types[0]))?;
                for row in 0..call.rows {
                    if a.is_null(row) {
                        out.append_null();
                    } else {
                        out.append_value((offsets[row + 1] - offsets[row]) as u64);
                    }
                }
            } else {
                let b = call.bytes(0)?;
                for row in 0..call.rows {
                    match b.get(row) {
                        // ClickHouse counts the bytes that do not continue a
                        // UTF-8 sequence.
                        Some(v) if utf8 => out
                            .append_value(v.iter().filter(|&&c| (c & 0xC0) != 0x80).count() as u64),
                        Some(v) => out.append_value(v.len() as u64),
                        None => out.append_null(),
                    }
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn concat() -> ChFunction {
    ChFunction::new(
        "concat",
        Arc::new(|_, _| Ok(ChType::String)),
        Arc::new(|call| {
            let settings = TextSettings::default();
            let arrays = (0..call.args.len())
                .map(|i| call.array(i))
                .collect::<Result<Vec<_>, _>>()?;
            let strings = (0..call.args.len())
                .map(|i| match call.types[i] {
                    ChType::String => call.strs(i).map(Some),
                    _ => Ok(None),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut out = StringBuilder::with_capacity(call.rows, 0);
            let mut text = Vec::new();
            'rows: for row in 0..call.rows {
                text.clear();
                for (i, a) in arrays.iter().enumerate() {
                    if a.is_null(row) {
                        out.append_null();
                        continue 'rows;
                    }
                    match (&strings[i], &call.types[i]) {
                        (Some(s), _) => text.extend_from_slice(s.get(row).unwrap_or("").as_bytes()),
                        (None, ChType::FixedString(_)) => {
                            let b = call.bytes(i)?;
                            text.extend_from_slice(b.get(row).unwrap_or(b""));
                        }
                        (None, t) => {
                            write_value(t, a.as_ref(), row, Style::Tsv, &settings, &mut text)?;
                        }
                    }
                }
                match std::str::from_utf8(&text) {
                    Ok(s) => out.append_value(s),
                    Err(_) => return Err(not_utf8("concat")),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn affix(name: &'static str, end: bool) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 2, 2)?;
            for (i, t) in args.iter().enumerate() {
                if !matches!(t, ChType::String | ChType::FixedString(_)) {
                    return Err(illegal_type(name, i, t));
                }
            }
            Ok(ChType::UInt8)
        }),
        Arc::new(move |call| {
            let s = call.bytes(0)?;
            let p = call.bytes(1)?;
            let mut out = UInt8Builder::with_capacity(call.rows);
            for row in 0..call.rows {
                match (s.get(row), p.get(row)) {
                    (Some(s), Some(p)) => {
                        let hit = if end {
                            s.ends_with(p)
                        } else {
                            s.starts_with(p)
                        };
                        out.append_value(u8::from(hit));
                    }
                    _ => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn ascii() -> ChFunction {
    ChFunction::new(
        "ascii",
        Arc::new(|args, _| {
            check_arity("ascii", args.len(), 1, 1)?;
            want_string("ascii", args, 0)?;
            Ok(ChType::Int32)
        }),
        Arc::new(|call| {
            let s = call.strs(0)?;
            let mut out = Int32Builder::with_capacity(call.rows);
            for row in 0..call.rows {
                match s.get(row) {
                    Some(v) => out.append_value(v.as_bytes().first().map_or(0, |b| i32::from(*b))),
                    None => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

#[derive(Clone, Copy)]
enum Side {
    Both,
    Left,
    Right,
}

fn trim(name: &'static str, side: Side) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 1, 2)?;
            want_string(name, args, 0)?;
            want_string(name, args, 1)?;
            Ok(ChType::String)
        }),
        Arc::new(move |call| {
            if call.args.len() == 1 {
                return map_strings(call, 0, |v| {
                    Ok(trim_set(v, b" ", side).unwrap_or_default().to_string())
                });
            }
            let s = call.strs(0)?;
            let set = call.strs(1)?;
            let mut out = StringBuilder::with_capacity(call.rows, 0);
            for row in 0..call.rows {
                match (s.get(row), set.get(row)) {
                    (Some(v), Some(chars)) => out.append_value(
                        trim_set(v, chars.as_bytes(), side).ok_or_else(|| not_utf8(name))?,
                    ),
                    _ => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

/// `v` without the leading and/or trailing bytes in `set`, or `None` when
/// that would cut a UTF-8 character apart (R31.2).
fn trim_set<'a>(v: &'a str, set: &[u8], side: Side) -> Option<&'a str> {
    let b = v.as_bytes();
    let mut start = 0;
    let mut end = b.len();
    if matches!(side, Side::Both | Side::Left) {
        while start < end && set.contains(&b[start]) {
            start += 1;
        }
    }
    if matches!(side, Side::Both | Side::Right) {
        while end > start && set.contains(&b[end - 1]) {
            end -= 1;
        }
    }
    v.get(start..end)
}

pub(super) fn register(r: &mut Registry) {
    // `length` itself is Task 32's (`func::arrays`): strings, arrays and
    // maps in one function.
    length("lengthUTF8", true).register(r);
    concat().register(r);
    slicer("left", false, |n| (1, Some(n))).register(r);
    slicer("leftUTF8", true, |n| (1, Some(n))).register(r);
    slicer("right", false, right_plan).register(r);
    slicer("rightUTF8", true, right_plan).register(r);
    substring("substring", false).register(r);
    substring("substringUTF8", true).register(r);
    affix("startsWith", false).register(r);
    affix("endsWith", true).register(r);
    affix("startsWithUTF8", false).register(r);
    affix("endsWithUTF8", true).register(r);
    ascii().register(r);
    trim("trimBoth", Side::Both).register(r);
    trim("trimLeft", Side::Left).register(r);
    trim("trimRight", Side::Right).register(r);
}

/// `right(s, n)`: the last `n`, or all but the first `-n`.
fn right_plan(n: i128) -> (i128, Option<i128>) {
    if n >= 0 { (-n, Some(n)) } else { (1 - n, None) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_windows_as_measured() {
        // The oracle's grid over 'abcde' (offset:length=answer).
        let grid = "-7:-=abcde -7:-7= -7:-3=ab -7:-1=abcd -7:0= -7:1= -7:2= -7:7=abcde -5:-=abcde -5:-7= -5:-3=ab -5:-1=abcd -5:0= -5:1=a -5:2=ab -5:7=abcde -3:-=cde -3:-7= -3:-3= -3:-1=cd -3:0= -3:1=c -3:2=cd -3:7=cde -1:-=e -1:-7= -1:-3= -1:-1= -1:0= -1:1=e -1:2=e -1:7=e 0:-= 0:-7= 0:-3= 0:-1= 0:0= 0:1= 0:2= 0:7= 1:-=abcde 1:-7= 1:-3=ab 1:-1=abcd 1:0= 1:1=a 1:2=ab 1:7=abcde 3:-=cde 3:-7= 3:-3= 3:-1=cd 3:0= 3:1=c 3:2=cd 3:7=cde 5:-=e 5:-7= 5:-3= 5:-1= 5:0= 5:1=e 5:2=e 5:7=e 6:-= 6:-7= 6:-3= 6:-1= 6:0= 6:1= 6:2= 6:7= 7:-= 7:-7= 7:-3= 7:-1= 7:0= 7:1= 7:2= 7:7=";
        for item in grid.split(' ') {
            let (args, want) = item.split_once('=').expect("o:l=v");
            let (o, l) = args.split_once(':').expect("o:l");
            let o: i128 = o.parse().expect("offset");
            let l: Option<i128> = (l != "-").then(|| l.parse().expect("length"));
            let (a, b) = window(5, o, l);
            assert_eq!(&"abcde"[a..b], want, "{item}");
        }
    }

    #[test]
    fn trim_refuses_to_split_characters() {
        assert_eq!(trim_set("äaä", "ä".as_bytes(), Side::Both), Some("a"));
        assert_eq!(trim_set("xxaxx", b"x", Side::Left), Some("axx"));
        assert_eq!(trim_set("  ", b" ", Side::Both), Some(""));
        // 'ã' is C3 A3: trimming the byte C3 would leave A3 alone.
        assert_eq!(trim_set("ãa", "ä".as_bytes(), Side::Left), None);
    }
}
