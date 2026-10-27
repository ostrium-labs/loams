//! A small JSON reader over bytes: validation, navigation by key or index,
//! and raw spans, as the `JSONExtract*` functions need them (ClickHouse reads
//! with simdjson; the answers here follow its, R33.9).
//!
//! Values are kept as spans of the input, so `JSONExtractRaw` returns the
//! text as written (`[-100, 200.0, 300]`, spaces and all).

/// The deepest nesting read; deeper documents are invalid.
pub const MAX_DEPTH: usize = 1024;

/// A JSON value's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool,
    /// A number.
    Number,
    /// A string.
    String,
    /// An array.
    Array,
    /// An object.
    Object,
}

/// A value: its kind and its span in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node<'a> {
    /// What it is.
    pub kind: Kind,
    /// Its text.
    pub raw: &'a [u8],
}

fn ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// The end of the string starting at `s[i] == b'"'`.
fn string_end(s: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < s.len() {
        match s[j] {
            b'"' => return Some(j + 1),
            b'\\' => {
                let e = *s.get(j + 1)?;
                match e {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => j += 2,
                    b'u' => {
                        let hex = s.get(j + 2..j + 6)?;
                        if !hex.iter().all(u8::is_ascii_hexdigit) {
                            return None;
                        }
                        j += 6;
                    }
                    _ => return None,
                }
            }
            c if c < 0x20 => return None,
            _ => j += 1,
        }
    }
    None
}

fn number_end(s: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if s.get(j) == Some(&b'-') {
        j += 1;
    }
    match s.get(j)? {
        b'0' => j += 1,
        b'1'..=b'9' => {
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
        }
        _ => return None,
    }
    if s.get(j) == Some(&b'.') {
        j += 1;
        let start = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            return None;
        }
    }
    if matches!(s.get(j), Some(b'e' | b'E')) {
        j += 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let start = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            return None;
        }
    }
    Some(j)
}

/// The value starting at `i` (after whitespace): its node and where it ends.
pub fn value(s: &[u8], i: usize, depth: usize) -> Option<(Node<'_>, usize)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let i = ws(s, i);
    let (kind, end) = match *s.get(i)? {
        b'n' => (Kind::Null, literal(s, i, b"null")?),
        b't' => (Kind::Bool, literal(s, i, b"true")?),
        b'f' => (Kind::Bool, literal(s, i, b"false")?),
        b'"' => (Kind::String, string_end(s, i)?),
        b'[' => {
            let mut j = ws(s, i + 1);
            if s.get(j) == Some(&b']') {
                (Kind::Array, j + 1)
            } else {
                loop {
                    let (_, e) = value(s, j, depth + 1)?;
                    j = ws(s, e);
                    match *s.get(j)? {
                        b',' => j += 1,
                        b']' => break (Kind::Array, j + 1),
                        _ => return None,
                    }
                }
            }
        }
        b'{' => {
            let mut j = ws(s, i + 1);
            if s.get(j) == Some(&b'}') {
                (Kind::Object, j + 1)
            } else {
                loop {
                    if s.get(j) != Some(&b'"') {
                        return None;
                    }
                    let k = string_end(s, j)?;
                    j = ws(s, k);
                    if s.get(j) != Some(&b':') {
                        return None;
                    }
                    let (_, e) = value(s, j + 1, depth + 1)?;
                    j = ws(s, e);
                    match *s.get(j)? {
                        b',' => j = ws(s, j + 1),
                        b'}' => break (Kind::Object, j + 1),
                        _ => return None,
                    }
                }
            }
        }
        b'-' | b'0'..=b'9' => (Kind::Number, number_end(s, i)?),
        _ => return None,
    };
    Some((
        Node {
            kind,
            raw: &s[i..end],
        },
        end,
    ))
}

fn literal(s: &[u8], i: usize, word: &[u8]) -> Option<usize> {
    (s.get(i..i + word.len())? == word).then_some(i + word.len())
}

/// The whole document, if it is one valid JSON value.
pub fn parse(doc: &[u8]) -> Option<Node<'_>> {
    let (node, end) = value(doc, 0, 0)?;
    (ws(doc, end) == doc.len()).then_some(node)
}

/// An array's elements.
pub fn elements(node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    if node.kind != Kind::Array {
        return out;
    }
    let s = node.raw;
    let mut j = ws(s, 1);
    if s.get(j) == Some(&b']') {
        return out;
    }
    while let Some((v, e)) = value(s, j, 1) {
        out.push(v);
        j = ws(s, e);
        if s.get(j) != Some(&b',') {
            break;
        }
        j += 1;
    }
    out
}

/// An object's members: decoded key and value.
pub fn members(node: Node<'_>) -> Vec<(Vec<u8>, Node<'_>)> {
    let mut out = Vec::new();
    if node.kind != Kind::Object {
        return out;
    }
    let s = node.raw;
    let mut j = ws(s, 1);
    while s.get(j) == Some(&b'"') {
        let Some(k) = string_end(s, j) else { break };
        let Some(key) = unescape(&s[j..k]) else { break };
        let colon = ws(s, k);
        if s.get(colon) != Some(&b':') {
            break;
        }
        let Some((v, e)) = value(s, colon + 1, 1) else {
            break;
        };
        out.push((key, v));
        j = ws(s, e);
        if s.get(j) != Some(&b',') {
            break;
        }
        j = ws(s, j + 1);
    }
    out
}

/// A key or 1-based index (negative from the end) in a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// An object member by name.
    Key(Vec<u8>),
    /// An element (of an array) or member (of an object) by position.
    Index(i64),
}

/// The value at `path` under `node`.
pub fn find<'a>(node: Node<'a>, path: &[Step]) -> Option<Node<'a>> {
    let mut cur = node;
    for step in path {
        cur = match step {
            Step::Key(k) => members(cur).into_iter().find(|(name, _)| name == k)?.1,
            Step::Index(i) => {
                let items: Vec<Node<'a>> = match cur.kind {
                    Kind::Array => elements(cur),
                    Kind::Object => members(cur).into_iter().map(|(_, v)| v).collect(),
                    _ => return None,
                };
                let n = items.len() as i64;
                let at = match *i {
                    0 => return None,
                    i if i > 0 => i - 1,
                    i => n + i,
                };
                if !(0..n).contains(&at) {
                    return None;
                }
                items[at as usize]
            }
        };
    }
    Some(cur)
}

/// A JSON string's text (with its quotes) decoded; `None` when an escape is
/// broken.
pub fn unescape(raw: &[u8]) -> Option<Vec<u8>> {
    let inner = raw.strip_prefix(b"\"")?.strip_suffix(b"\"")?;
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        let e = *inner.get(i + 1)?;
        i += 2;
        match e {
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            b'/' => out.push(b'/'),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'u' => {
                let mut code = hex4(inner.get(i..i + 4)?)?;
                i += 4;
                if (0xD800..0xDC00).contains(&code) {
                    // A surrogate pair.
                    if inner.get(i..i + 2)? != b"\\u" {
                        return None;
                    }
                    let low = hex4(inner.get(i + 2..i + 6)?)?;
                    if !(0xDC00..0xE000).contains(&low) {
                        return None;
                    }
                    i += 6;
                    code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                }
                let ch = char::from_u32(code)?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return None,
        }
    }
    Some(out)
}

fn hex4(b: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok()
}

/// A number node's value class: `Int64`, `UInt64` or `Double`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// An integer that fits `Int64`.
    Int(i64),
    /// An integer above `Int64`'s range that fits `UInt64`.
    UInt(u64),
    /// Anything else.
    Double(f64),
}

/// A number node's value, as simdjson classifies it.
pub fn number(raw: &[u8]) -> Option<Number> {
    let s = std::str::from_utf8(raw).ok()?;
    let integral = !s.contains(['.', 'e', 'E']);
    if integral {
        if let Ok(i) = s.parse::<i64>() {
            return Some(Number::Int(i));
        }
        if let Ok(u) = s.parse::<u64>() {
            return Some(Number::UInt(u));
        }
    }
    s.parse::<f64>().ok().map(Number::Double)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn documents_validate() {
        assert!(parse(br#"{"a": [1, 2.5, "x", null, true], "b": {}}"#).is_some());
        assert!(parse(b" 1 ").is_some());
        assert!(parse(b"{").is_none());
        assert!(parse(b"{'a':1}").is_none());
        assert!(parse(b"[1,]").is_none());
        assert!(parse(b"01").is_none());
        assert!(parse(b"1 2").is_none());
    }

    #[test]
    fn paths_find_values() {
        let doc = parse(br#"{"a": {"b": [10, 20, 30]}, "c": "x\u00e9"}"#).unwrap();
        let b = find(doc, &[Step::Key(b"a".to_vec()), Step::Key(b"b".to_vec())]).unwrap();
        assert_eq!(b.raw, b"[10, 20, 30]");
        let last = find(b, &[Step::Index(-1)]).unwrap();
        assert_eq!(last.raw, b"30");
        assert!(find(b, &[Step::Index(0)]).is_none());
        assert!(find(b, &[Step::Index(4)]).is_none());
        let first_member = find(doc, &[Step::Index(1)]).unwrap();
        assert_eq!(first_member.kind, Kind::Object);
        let c = find(doc, &[Step::Key(b"c".to_vec())]).unwrap();
        assert_eq!(unescape(c.raw).unwrap(), "xé".as_bytes());
    }

    #[test]
    fn strings_unescape() {
        assert_eq!(unescape(br#""a\nb""#).unwrap(), b"a\nb");
        assert_eq!(
            unescape(br#""\ud83d\ude00""#).unwrap(),
            "\u{1F600}".as_bytes()
        );
        assert!(unescape(br#""\x""#).is_none());
    }

    #[test]
    fn numbers_classify() {
        assert_eq!(number(b"-5"), Some(Number::Int(-5)));
        assert_eq!(
            number(b"18446744073709551615"),
            Some(Number::UInt(u64::MAX))
        );
        assert_eq!(number(b"1.5"), Some(Number::Double(1.5)));
        assert_eq!(number(b"1e2"), Some(Number::Double(100.0)));
    }
}
