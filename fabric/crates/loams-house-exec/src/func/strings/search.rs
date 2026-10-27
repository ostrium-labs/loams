//! `position` and its forms, `locate`, and `multiSearchAny` and its forms
//! (`src/Functions/PositionImpl.h`, `MultiSearchImpl.h`), as measured:
//! positions are 1-based and 0 means not found; an empty needle is found at
//! the start position; a start position of 0 is 1, and one past the end
//! finds nothing. `multiSearchAny` finds nothing in an empty haystack, not
//! even an empty needle. The case-insensitive forms fold ASCII letters only;
//! their `UTF8` forms fold each character with its simple lower-case mapping.

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, UInt8Builder, UInt64Builder};
use datafusion::arrow::datatypes::DataType;
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};

use crate::func::chudf::{ChFunction, check_arity, illegal_type, list_parts, want_string};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fold {
    /// Bytes as they are.
    Bytes,
    /// Bytes, ASCII letters folded.
    AsciiCase,
    /// Characters as they are.
    Chars,
    /// Characters, each lower-cased.
    CharsCase,
}

/// A character's simple lower-case mapping (the first of its full mapping:
/// `İ` is `i`).
fn lower_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn find<T: PartialEq>(h: &[T], n: &[T]) -> Option<usize> {
    if n.is_empty() {
        return Some(0);
    }
    if n.len() > h.len() {
        return None;
    }
    h.windows(n.len()).position(|w| w == n)
}

/// ClickHouse's 1-based position of `needle` in `haystack` from `start`.
fn position_of(haystack: &str, needle: &str, start: u64, fold: Fold) -> u64 {
    let start = start.max(1) as usize;
    let at = |len: usize, found: Option<usize>| -> u64 {
        if start > len + 1 {
            return 0;
        }
        found.map_or(0, |i| (start + i) as u64)
    };
    match fold {
        Fold::Bytes | Fold::AsciiCase => {
            let h = haystack.as_bytes();
            if start > h.len() + 1 {
                return 0;
            }
            let rest = &h[start - 1..];
            let found = if fold == Fold::Bytes {
                find(rest, needle.as_bytes())
            } else {
                let rest = rest.to_ascii_lowercase();
                find(&rest, &needle.as_bytes().to_ascii_lowercase())
            };
            at(h.len(), found)
        }
        Fold::Chars | Fold::CharsCase => {
            let map = |c: char| {
                if fold == Fold::CharsCase {
                    lower_char(c)
                } else {
                    c
                }
            };
            let h: Vec<char> = haystack.chars().map(map).collect();
            let n: Vec<char> = needle.chars().map(map).collect();
            if start > h.len() + 1 {
                return 0;
            }
            at(h.len(), find(&h[start - 1..], &n))
        }
    }
}

fn position(name: &'static str, fold: Fold, needle_first: bool) -> ChFunction {
    let (hi, ni) = if needle_first { (1, 0) } else { (0, 1) };
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 2, 3)?;
            want_string(name, args, 0)?;
            want_string(name, args, 1)?;
            if let Some(t) = args.get(2)
                && !matches!(
                    t,
                    ChType::UInt8 | ChType::UInt16 | ChType::UInt32 | ChType::UInt64
                )
            {
                return Err(illegal_type(name, 2, t));
            }
            Ok(ChType::UInt64)
        }),
        Arc::new(move |call| {
            let h = call.strs(hi)?;
            let n = call.strs(ni)?;
            let start = if call.args.len() > 2 {
                Some(call.ints(2)?)
            } else {
                None
            };
            let mut out = UInt64Builder::with_capacity(call.rows);
            for row in 0..call.rows {
                let s = match &start {
                    Some(s) => s.get(row).map(|v| v.clamp(0, i128::from(u64::MAX)) as u64),
                    None => Some(1),
                };
                match (h.get(row), n.get(row), s) {
                    (Some(h), Some(n), Some(s)) => out.append_value(position_of(h, n, s, fold)),
                    _ => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn contains(haystack: &str, needle: &str, fold: Fold) -> bool {
    if haystack.is_empty() {
        return false;
    }
    position_of(haystack, needle, 1, fold) > 0
}

fn multi_search_any(name: &'static str, fold: Fold) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 2, 2)?;
            want_string(name, args, 0)?;
            match &args[1] {
                ChType::Array(inner) if matches!(inner.base(), ChType::String) => {}
                t => {
                    return Err(ChError::illegal_type_of_argument(format!(
                        "Illegal type {t} of argument of function {name}"
                    )));
                }
            }
            Ok(ChType::UInt8)
        }),
        Arc::new(move |call| {
            let h = call.strs(0)?;
            let lists = call.array(1)?;
            let (offsets, values) = list_parts(lists.as_ref())
                .ok_or_else(|| illegal_type(call.name, 1, &call.types[1]))?;
            let values = match values.data_type() {
                DataType::Utf8 => values,
                _ => datafusion::arrow::compute::cast(&values, &DataType::Utf8)
                    .map_err(|e| ChError::logical_error(e.to_string()))?,
            };
            let needles = values.as_string::<i32>();
            let mut out = UInt8Builder::with_capacity(call.rows);
            for row in 0..call.rows {
                let Some(hay) = h.get(row) else {
                    out.append_null();
                    continue;
                };
                if lists.is_null(row) {
                    out.append_null();
                    continue;
                }
                let hit = (offsets[row]..offsets[row + 1])
                    .any(|j| !needles.is_null(j) && contains(hay, needles.value(j), fold));
                out.append_value(u8::from(hit));
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

pub(super) fn register(r: &mut Registry) {
    position("position", Fold::Bytes, false).register(r);
    position("positionUTF8", Fold::Chars, false).register(r);
    position("positionCaseInsensitive", Fold::AsciiCase, false).register(r);
    position("positionCaseInsensitiveUTF8", Fold::CharsCase, false).register(r);
    position("locate", Fold::Bytes, true).register(r);
    multi_search_any("multiSearchAny", Fold::Bytes).register(r);
    multi_search_any("multiSearchAnyUTF8", Fold::Chars).register(r);
    multi_search_any("multiSearchAnyCaseInsensitive", Fold::AsciiCase).register(r);
    multi_search_any("multiSearchAnyCaseInsensitiveUTF8", Fold::CharsCase).register(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_as_measured() {
        assert_eq!(position_of("abcabc", "c", 1, Fold::Bytes), 3);
        assert_eq!(position_of("abcabc", "c", 4, Fold::Bytes), 6);
        assert_eq!(position_of("abc", "", 1, Fold::Bytes), 1);
        assert_eq!(position_of("abc", "", 4, Fold::Bytes), 4);
        assert_eq!(position_of("abc", "", 5, Fold::Bytes), 0);
        assert_eq!(position_of("", "", 0, Fold::Bytes), 1);
        assert_eq!(position_of("", "", 2, Fold::Bytes), 0);
        assert_eq!(position_of("abc", "b", 0, Fold::Bytes), 2);
        assert_eq!(position_of("äbc", "b", 1, Fold::Bytes), 3);
        assert_eq!(position_of("äbc", "b", 1, Fold::Chars), 2);
        assert_eq!(position_of("ÄBc", "äb", 1, Fold::AsciiCase), 0);
        assert_eq!(position_of("aBc", "B", 1, Fold::AsciiCase), 2);
        assert_eq!(position_of("ÄBc", "äb", 1, Fold::CharsCase), 1);
        assert_eq!(position_of("äbäb", "b", 3, Fold::Chars), 4);
        assert_eq!(position_of("äb", "", 3, Fold::Chars), 3);
        assert_eq!(position_of("äb", "", 4, Fold::Chars), 0);
        assert_eq!(position_of("a\u{212A}b", "k", 1, Fold::CharsCase), 2);
        assert_eq!(position_of("ΣΑΣ", "ς", 1, Fold::CharsCase), 0);
        assert!(!contains("", "", Fold::Bytes));
        assert!(contains("a", "", Fold::Bytes));
    }
}
