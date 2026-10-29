//! `replaceOne`, `replaceAll` (`replace`), `replaceRegexpOne` and
//! `replaceRegexpAll` (`src/Functions/ReplaceStringImpl.h`,
//! `ReplaceRegexpImpl.h`), as measured: an empty pattern leaves the haystack
//! as it is; after an empty match the next byte is copied and the search
//! goes on from there (`replaceRegexpAll('abc', 'x*', '-')` is `-a-b-c-`);
//! in a replacement `\0`–`\9` are the match and its groups, `\\` is a
//! backslash, a backslash before anything else is dropped, and a group the
//! pattern does not have is `36`.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, StringBuilder};
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};
use regex::bytes::Regex;

use crate::func::chudf::{ChFunction, check_arity, illegal_type, not_utf8};
use crate::func::strings::re2;

fn typer(name: &'static str) -> crate::func::chudf::Typer {
    Arc::new(move |args, _| {
        check_arity(name, args.len(), 3, 3)?;
        for (i, t) in args.iter().enumerate() {
            match t {
                ChType::String => {}
                ChType::FixedString(_) if i == 0 => {
                    return Err(ChError {
                        code: 44,
                        name: "ILLEGAL_COLUMN",
                        message: format!("Illegal column {t} of first argument of function {name}"),
                    });
                }
                t => return Err(illegal_type(name, i, t)),
            }
        }
        Ok(ChType::String)
    })
}

fn replace_string(name: &'static str, one: bool) -> ChFunction {
    ChFunction::new(
        name,
        typer(name),
        Arc::new(move |call| {
            let h = call.strs(0)?;
            let from = call.strs(1)?;
            let to = call.strs(2)?;
            let mut out = StringBuilder::with_capacity(call.rows, 0);
            for row in 0..call.rows {
                match (h.get(row), from.get(row), to.get(row)) {
                    (Some(h), Some(""), Some(_)) => out.append_value(h),
                    (Some(h), Some(f), Some(t)) if one => out.append_value(h.replacen(f, t, 1)),
                    (Some(h), Some(f), Some(t)) => out.append_value(h.replace(f, t)),
                    _ => out.append_null(),
                }
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

/// A piece of a regular-expression replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(Vec<u8>),
    Group(usize),
}

/// A replacement's pieces, checked against the pattern's `groups` (the match
/// itself included).
fn pieces(replacement: &str, groups: usize) -> Result<Vec<Piece>, ChError> {
    let mut out = Vec::new();
    let mut text = Vec::new();
    let b = replacement.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            i += 2;
            if c.is_ascii_digit() {
                let n = usize::from(c - b'0');
                if n >= groups {
                    return Err(ChError::bad_arguments(format!(
                        "Substitution '\\{n}' in replacement argument is invalid, regexp has only {} capturing groups",
                        groups - 1
                    )));
                }
                if !text.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                out.push(Piece::Group(n));
            } else {
                text.push(c);
            }
        } else {
            text.push(b[i]);
            i += 1;
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    Ok(out)
}

/// `haystack` with the matches of `re` replaced (only the first when `one`).
fn replace_regexp(haystack: &[u8], re: &Regex, with: &[Piece], one: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut locs = re.capture_locations();
    let mut start = 0;
    while start <= haystack.len() {
        let Some(m) = re.captures_read_at(&mut locs, haystack, start) else {
            break;
        };
        out.extend_from_slice(&haystack[start..m.start()]);
        for p in with {
            match p {
                Piece::Text(t) => out.extend_from_slice(t),
                Piece::Group(n) => {
                    if let Some((a, b)) = locs.get(*n) {
                        out.extend_from_slice(&haystack[a..b]);
                    }
                }
            }
        }
        if m.is_empty() {
            if let Some(&b) = haystack.get(m.start()) {
                out.push(b);
            }
            start = m.start() + 1;
        } else {
            start = m.end();
        }
        if one {
            break;
        }
    }
    if start < haystack.len() {
        out.extend_from_slice(&haystack[start..]);
    }
    out
}

fn replace_regexp_fn(name: &'static str, one: bool) -> ChFunction {
    ChFunction::new(
        name,
        typer(name),
        Arc::new(move |call| {
            let h = call.strs(0)?;
            let pattern = call.strs(1)?;
            let to = call.strs(2)?;
            let mut compiled: HashMap<&str, Regex> = HashMap::new();
            let mut out = StringBuilder::with_capacity(call.rows, 0);
            for row in 0..call.rows {
                let (Some(hay), Some(p), Some(t)) = (h.get(row), pattern.get(row), to.get(row))
                else {
                    out.append_null();
                    continue;
                };
                if p.is_empty() {
                    out.append_value(hay);
                    continue;
                }
                if !compiled.contains_key(p) {
                    compiled.insert(p, re2::compile(p, false)?);
                }
                let re = &compiled[p];
                let with = pieces(t, re.captures_len())?;
                let bytes = replace_regexp(hay.as_bytes(), re, &with, one);
                out.append_value(String::from_utf8(bytes).map_err(|_| not_utf8(name))?);
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

pub(super) fn register(r: &mut Registry) {
    replace_string("replaceOne", true).register(r);
    replace_string("replaceAll", false).register(r);
    replace_regexp_fn("replaceRegexpOne", true).register(r);
    replace_regexp_fn("replaceRegexpAll", false).register(r);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn all(h: &str, p: &str, t: &str) -> String {
        let re = re2::compile(p, false).unwrap();
        let with = pieces(t, re.captures_len()).unwrap();
        String::from_utf8_lossy(&replace_regexp(h.as_bytes(), &re, &with, false)).into_owned()
    }

    #[test]
    fn regexp_replacement_as_measured() {
        assert_eq!(all("abc", "^", "x"), "xabc");
        assert_eq!(all("abc", "b*", "x"), "xaxxcx");
        assert_eq!(all("abc", "x*", "-"), "-a-b-c-");
        assert_eq!(all("aaa", "a", r"\0\0"), "aaaaaa");
        assert_eq!(all("abc", "(b)", r"[\1\\]"), r"a[b\]c");
        assert_eq!(all("héllo", ".", "x"), "xxxxx");
        assert_eq!(all("a.b", r"\.", "$0"), "a$0b");
        assert_eq!(all("abc", "b", r"\x"), "axc");
        assert_eq!(all("abc", "c", "z\\"), "abz\\");
        assert_eq!(all("", "^", "x"), "x");
        assert_eq!(all("ab", "$", "x"), "abx");
        assert_eq!(all("abc", "^a|c$", "-"), "-b-");
        assert_eq!(all("aaa", "^a", "b"), "baa");
        let re = re2::compile("(b)", false).unwrap();
        assert_eq!(pieces(r"\2", re.captures_len()).unwrap_err().code, 36);
    }
}
