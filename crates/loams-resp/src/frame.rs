// ported-from: https://github.com/tidb-incubator/tidis
// revision: 555856206f248737b12d3f148b10f08a604bc821
// path: src/frame.rs, src/parse.rs
// licence: Apache-2.0
// ported-from: https://github.com/tokio-rs/mini-redis
// revision: (as carried by Tidis at the revision above)
// path: src/frame.rs, src/parse.rs
// licence: MIT, Copyright (c) 2020 Tokio Contributors
// Modified by Ostrium Labs for Loams.

//! RESP frames (design §77 §8.3, "Parser and encoder"; plan KV2 Task 1).
//!
//! - [`Value`] is a reply: RESP2's five types and RESP3's additions,
//!   encoded per connection by [`Value::encode`], which downgrades RESP3
//!   types for a RESP2 client the way Valkey does.
//! - [`Decoder`] reads requests: an array of bulk strings, or an inline
//!   line (`redis-cli` and the TCL suite send both). It is incremental (a
//!   request split over many reads is parsed once, not from the start on
//!   every read), zero-copy (arguments are slices of the read buffer) and
//!   bounded: every length is checked against [`Limits`] before anything
//!   is allocated for it.
//! - [`Value::decode`] reads a reply, for tests, the harness and the
//!   differential runner.
//!
//! The structure (a frame enum, a line finder, a decimal reader, an
//! `Incomplete` outcome that means "read more") follows Tidis's `frame.rs`,
//! which follows mini-redis. The limits, inline commands, RESP3 and the
//! incremental request state are new.

use std::fmt;

use bytes::{BufMut, Bytes, BytesMut};

/// The protocol a connection speaks: RESP2 until `HELLO 3`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Proto {
    #[default]
    Resp2,
    Resp3,
}

/// A reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `+OK`
    Simple(String),
    /// `-ERR …`: the text includes the error code (`ERR`, `WRONGTYPE`, …).
    Error(String),
    Integer(i64),
    Bulk(Bytes),
    /// The null bulk string (`$-1` in RESP2, `_` in RESP3).
    Null,
    /// The null array (`*-1` in RESP2, `_` in RESP3).
    NullArray,
    Array(Vec<Value>),
    /// RESP3 `%`; a flat array of keys and values in RESP2.
    Map(Vec<(Value, Value)>),
    /// RESP3 `~`; an array in RESP2.
    Set(Vec<Value>),
    /// RESP3 `,`; a bulk string in RESP2.
    Double(f64),
    /// RESP3 `#`; the integer 1 or 0 in RESP2.
    Boolean(bool),
    /// RESP3 `(`; a bulk string in RESP2.
    BigNumber(String),
    /// RESP3 `=` with its three-letter format (`txt`, `mkd`); a bulk string
    /// in RESP2.
    Verbatim(String, Bytes),
    /// RESP3 `>`; an array in RESP2.
    Push(Vec<Value>),
}

impl Value {
    /// `+OK`.
    pub fn ok() -> Self {
        Value::Simple("OK".into())
    }

    /// An error reply; `text` starts with its code (`ERR …`).
    pub fn err(text: impl Into<String>) -> Self {
        Value::Error(text.into())
    }

    /// A bulk string.
    pub fn bulk(bytes: impl Into<Bytes>) -> Self {
        Value::Bulk(bytes.into())
    }

    /// Appends this reply, encoded for `proto`, to `out`.
    pub fn encode(&self, proto: Proto, out: &mut BytesMut) {
        let resp3 = proto == Proto::Resp3;
        match self {
            Value::Simple(s) => line(out, b'+', s.as_bytes()),
            Value::Error(s) => line(out, b'-', s.as_bytes()),
            Value::Integer(i) => line(out, b':', i.to_string().as_bytes()),
            Value::Bulk(b) => bulk(out, b),
            Value::Null if resp3 => out.put_slice(b"_\r\n"),
            Value::Null => out.put_slice(b"$-1\r\n"),
            Value::NullArray if resp3 => out.put_slice(b"_\r\n"),
            Value::NullArray => out.put_slice(b"*-1\r\n"),
            Value::Array(items) => {
                header(out, b'*', items.len());
                for item in items {
                    item.encode(proto, out);
                }
            }
            Value::Map(pairs) => {
                if resp3 {
                    header(out, b'%', pairs.len());
                } else {
                    header(out, b'*', pairs.len() * 2);
                }
                for (k, v) in pairs {
                    k.encode(proto, out);
                    v.encode(proto, out);
                }
            }
            Value::Set(items) | Value::Push(items) => {
                let kind = match (resp3, self) {
                    (true, Value::Set(_)) => b'~',
                    (true, _) => b'>',
                    (false, _) => b'*',
                };
                header(out, kind, items.len());
                for item in items {
                    item.encode(proto, out);
                }
            }
            Value::Double(d) => {
                let text = format_double(*d);
                if resp3 {
                    line(out, b',', text.as_bytes());
                } else {
                    bulk(out, text.as_bytes());
                }
            }
            Value::Boolean(b) if resp3 => out.put_slice(if *b { b"#t\r\n" } else { b"#f\r\n" }),
            Value::Boolean(b) => line(out, b':', if *b { b"1" } else { b"0" }),
            Value::BigNumber(n) if resp3 => line(out, b'(', n.as_bytes()),
            Value::BigNumber(n) => bulk(out, n.as_bytes()),
            Value::Verbatim(format, text) if resp3 => {
                header(out, b'=', format.len() + 1 + text.len());
                out.put_slice(format.as_bytes());
                out.put_u8(b':');
                out.put_slice(text);
                out.put_slice(b"\r\n");
            }
            Value::Verbatim(_, text) => bulk(out, text),
        }
    }

    /// This reply, encoded for `proto`.
    pub fn to_bytes(&self, proto: Proto) -> Bytes {
        let mut out = BytesMut::new();
        self.encode(proto, &mut out);
        out.freeze()
    }

    /// Reads one reply from the front of `buf`: `Ok(None)` when `buf` holds
    /// only part of one, else the reply and the bytes it took. Nested
    /// aggregates are limited to 64 levels.
    pub fn decode(buf: &[u8]) -> Result<Option<(Value, usize)>, ProtoError> {
        let mut at = 0;
        match parse_value(buf, &mut at, 0) {
            Ok(v) => Ok(Some((v, at))),
            Err(Short::Incomplete) => Ok(None),
            Err(Short::Invalid(e)) => Err(e),
        }
    }
}

fn line(out: &mut BytesMut, kind: u8, text: &[u8]) {
    out.reserve(text.len() + 3);
    out.put_u8(kind);
    out.put_slice(text);
    out.put_slice(b"\r\n");
}

fn header(out: &mut BytesMut, kind: u8, len: usize) {
    line(out, kind, len.to_string().as_bytes());
}

fn bulk(out: &mut BytesMut, bytes: &[u8]) {
    header(out, b'$', bytes.len());
    out.reserve(bytes.len() + 2);
    out.put_slice(bytes);
    out.put_slice(b"\r\n");
}

/// A double as Valkey writes one: the shortest text that reads back to the
/// same value, in `%.17g`'s choice of fixed or exponent notation (exponent
/// below -4 or from 17 up), `inf`, `-inf` and `nan` spelled out.
pub fn format_double(d: f64) -> String {
    if d.is_nan() {
        return "nan".into();
    }
    if d.is_infinite() {
        return if d > 0.0 { "inf" } else { "-inf" }.into();
    }
    if d == 0.0 {
        return if d.is_sign_negative() { "-0" } else { "0" }.into();
    }
    // `{:e}` gives the shortest round-tripping digits: `d.ddde±x`.
    let sci = format!("{d:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..17).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exp.unsigned_abs());
    }
    format!("{d}")
}

/// A float as `INCRBYFLOAT` and `HINCRBYFLOAT` store and reply with it:
/// plain decimal notation, never an exponent, no trailing zeros, the
/// shortest digits that read back to the same `f64` (Valkey's human mode
/// prints a long double with 17 decimals and trims; for values beyond 17
/// significant digits the two can differ in the last digits). `-0` is
/// written `0`. The caller has refused NaN and infinities.
pub fn format_float_human(d: f64) -> String {
    if d == 0.0 {
        return "0".into();
    }
    format!("{d}")
}

/// A protocol error. Its text is Valkey's (`Protocol error: …`), replied as
/// `-ERR <text>` before the connection closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtoError(pub String);

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtoError {}

fn proto(text: &str) -> ProtoError {
    ProtoError(format!("Protocol error: {text}"))
}

/// The decoder's bounds. The defaults are Valkey's: `proto-max-bulk-len`
/// 512 MiB, a multibulk count up to `i32::MAX`, a 64 KiB inline request;
/// before authentication, at most 10 arguments of at most 16 KiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bulk: usize,
    pub max_multibulk: usize,
    pub max_inline: usize,
    pub unauth_max_multibulk: usize,
    pub unauth_max_bulk: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bulk: 512 * 1024 * 1024,
            max_multibulk: i32::MAX as usize,
            max_inline: 64 * 1024,
            unauth_max_multibulk: 10,
            unauth_max_bulk: 16 * 1024,
        }
    }
}

/// Requests are never preallocated past this many arguments; a larger
/// request grows as its arguments arrive.
const PREALLOC_ARGS: usize = 1024;

/// A multibulk request part-way through: the arguments read so far, as
/// ranges of the (not yet consumed) buffer.
#[derive(Debug)]
struct Partial {
    argc: usize,
    pos: usize,
    args: Vec<(usize, usize)>,
}

/// Reads requests from a connection's read buffer.
#[derive(Debug, Default)]
pub struct Decoder {
    limits: Limits,
    authenticated: bool,
    partial: Option<Partial>,
}

impl Decoder {
    /// A decoder with `limits`, for an unauthenticated connection when
    /// `authenticated` is false.
    pub fn new(limits: Limits, authenticated: bool) -> Self {
        Decoder {
            limits,
            authenticated,
            partial: None,
        }
    }

    /// Lifts (or restores) the pre-authentication limits.
    pub fn set_authenticated(&mut self, authenticated: bool) {
        self.authenticated = authenticated;
    }

    fn max_multibulk(&self) -> usize {
        if self.authenticated {
            self.limits.max_multibulk
        } else {
            self.limits
                .unauth_max_multibulk
                .min(self.limits.max_multibulk)
        }
    }

    fn max_bulk(&self) -> usize {
        if self.authenticated {
            self.limits.max_bulk
        } else {
            self.limits.unauth_max_bulk.min(self.limits.max_bulk)
        }
    }

    /// Takes the next request off the front of `buf`: `Ok(None)` when `buf`
    /// holds no whole request yet (read more and call again). Empty
    /// requests (`*0`, `*-1`, a blank line) are skipped, as Valkey does. On
    /// an error the connection replies and closes; `buf` is left as is.
    pub fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Vec<Bytes>>, ProtoError> {
        loop {
            if buf.is_empty() {
                return Ok(None);
            }
            let step = if self.partial.is_some() || buf[0] == b'*' {
                self.multibulk(buf)?
            } else {
                self.inline(buf)?
            };
            match step {
                Step::Incomplete => return Ok(None),
                Step::Skip(n) => {
                    let _ = buf.split_to(n);
                }
                Step::Request(args) => return Ok(Some(args)),
            }
        }
    }

    fn multibulk(&mut self, buf: &mut BytesMut) -> Result<Step, ProtoError> {
        let mut partial = match self.partial.take() {
            Some(p) => p,
            None => {
                let Some((text, next)) = find_line(buf, 1, self.limits.max_inline) else {
                    if buf.len() > self.limits.max_inline {
                        return Err(proto("too big mbulk count string"));
                    }
                    return Ok(Step::Incomplete);
                };
                let count = parse_int(text).ok_or_else(|| proto("invalid multibulk length"))?;
                if count <= 0 {
                    return Ok(Step::Skip(next));
                }
                let argc = usize::try_from(count).unwrap_or(usize::MAX);
                if argc > self.max_multibulk() {
                    return Err(proto("invalid multibulk length"));
                }
                Partial {
                    argc,
                    pos: next,
                    args: Vec::with_capacity(argc.min(PREALLOC_ARGS)),
                }
            }
        };
        while partial.args.len() < partial.argc {
            let at = partial.pos;
            if at >= buf.len() {
                self.partial = Some(partial);
                return Ok(Step::Incomplete);
            }
            if buf[at] != b'$' {
                return Err(proto(&format!("expected '$', got '{}'", buf[at] as char)));
            }
            let Some((text, start)) = find_line(buf, at + 1, self.limits.max_inline) else {
                if buf.len() - at > self.limits.max_inline {
                    return Err(proto("too big bulk count string"));
                }
                self.partial = Some(partial);
                return Ok(Step::Incomplete);
            };
            let len = parse_int(text)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n <= self.max_bulk())
                .ok_or_else(|| proto("invalid bulk length"))?;
            if buf.len() < start + len + 2 {
                // The caller reads more; the header is not parsed again.
                partial.pos = at;
                self.partial = Some(partial);
                if buf.capacity() - buf.len() < start + len + 2 - buf.len() {
                    // One reservation for the whole argument, now that its
                    // length is known and within the limit.
                    buf.reserve(start + len + 2 - buf.len());
                }
                return Ok(Step::Incomplete);
            }
            partial.args.push((start, start + len));
            partial.pos = start + len + 2;
        }
        let frame = buf.split_to(partial.pos).freeze();
        Ok(Step::Request(
            partial
                .args
                .into_iter()
                .map(|(s, e)| frame.slice(s..e))
                .collect(),
        ))
    }

    fn inline(&mut self, buf: &mut BytesMut) -> Result<Step, ProtoError> {
        let Some(nl) = buf
            .iter()
            .take(self.limits.max_inline + 1)
            .position(|b| *b == b'\n')
        else {
            if buf.len() > self.limits.max_inline {
                return Err(proto("too big inline request"));
            }
            return Ok(Step::Incomplete);
        };
        let mut text = &buf[..nl];
        if let [rest @ .., b'\r'] = text {
            text = rest;
        }
        let args = split_args(text).ok_or_else(|| proto("unbalanced quotes in request"))?;
        if args.len() > self.max_multibulk() {
            return Err(proto("invalid multibulk length"));
        }
        let _ = buf.split_to(nl + 1);
        if args.is_empty() {
            return Ok(Step::Skip(0));
        }
        Ok(Step::Request(args.into_iter().map(Bytes::from).collect()))
    }
}

enum Step {
    Incomplete,
    Skip(usize),
    Request(Vec<Bytes>),
}

/// The line starting at `from`, without its `\r\n`, and the offset after
/// it; `None` if no `\r\n` lies within `max` bytes.
fn find_line(buf: &[u8], from: usize, max: usize) -> Option<(&[u8], usize)> {
    let window = buf.get(from..)?;
    let window = &window[..window.len().min(max + 2)];
    let at = window.windows(2).position(|w| w == b"\r\n")?;
    Some((&window[..at], from + at + 2))
}

/// A strict decimal: an optional `-`, digits, no leading `+`, spaces or
/// leading zeros (Valkey's `string2ll`).
pub fn parse_int(text: &[u8]) -> Option<i64> {
    let digits = text.strip_prefix(b"-").unwrap_or(text);
    if digits.is_empty()
        || digits.len() > 19
        || !digits.iter().all(u8::is_ascii_digit)
        || (digits.len() > 1 && digits[0] == b'0')
        || (digits == b"0" && text.len() > 1)
    {
        return None;
    }
    std::str::from_utf8(text).ok()?.parse().ok()
}

/// Splits an inline request as Valkey's `sdssplitargs` does: arguments
/// separated by spaces; `"…"` with `\n`, `\r`, `\t`, `\b`, `\a`, `\xHH`
/// and `\<c>` escapes; `'…'` with only `\'`; a closing quote must be
/// followed by a space or the end. `None` on unbalanced quotes.
pub fn split_args(text: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut args = Vec::new();
    let mut i = 0;
    let n = text.len();
    loop {
        while i < n && text[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            return Some(args);
        }
        let mut arg = Vec::new();
        let mut in_dq = false;
        let mut in_sq = false;
        loop {
            if in_dq {
                if i >= n {
                    return None;
                }
                let c = text[i];
                if c == b'\\'
                    && i + 3 < n
                    && text[i + 1] == b'x'
                    && text[i + 2].is_ascii_hexdigit()
                    && text[i + 3].is_ascii_hexdigit()
                {
                    arg.push(hex(text[i + 2]) * 16 + hex(text[i + 3]));
                    i += 3;
                } else if c == b'\\' && i + 1 < n {
                    i += 1;
                    arg.push(match text[i] {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'b' => 0x08,
                        b'a' => 0x07,
                        other => other,
                    });
                } else if c == b'"' {
                    if i + 1 < n && !text[i + 1].is_ascii_whitespace() {
                        return None;
                    }
                    i += 1;
                    break;
                } else {
                    arg.push(c);
                }
            } else if in_sq {
                if i >= n {
                    return None;
                }
                let c = text[i];
                if c == b'\\' && i + 1 < n && text[i + 1] == b'\'' {
                    i += 1;
                    arg.push(b'\'');
                } else if c == b'\'' {
                    if i + 1 < n && !text[i + 1].is_ascii_whitespace() {
                        return None;
                    }
                    i += 1;
                    break;
                } else {
                    arg.push(c);
                }
            } else {
                if i >= n {
                    break;
                }
                match text[i] {
                    b' ' | b'\n' | b'\r' | b'\t' | 0 => break,
                    b'"' => in_dq = true,
                    b'\'' => in_sq = true,
                    c => arg.push(c),
                }
            }
            i += 1;
        }
        args.push(arg);
    }
}

fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

enum Short {
    Incomplete,
    Invalid(ProtoError),
}

impl From<ProtoError> for Short {
    fn from(e: ProtoError) -> Self {
        Short::Invalid(e)
    }
}

const MAX_DEPTH: usize = 64;

fn reply_line<'a>(buf: &'a [u8], at: &mut usize) -> Result<&'a [u8], Short> {
    let window = buf.get(*at..).ok_or(Short::Incomplete)?;
    let end = window
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or(Short::Incomplete)?;
    let text = &window[..end];
    *at += end + 2;
    Ok(text)
}

fn reply_int(buf: &[u8], at: &mut usize) -> Result<i64, Short> {
    let text = reply_line(buf, at)?;
    parse_int(text).ok_or_else(|| Short::Invalid(proto("invalid integer in reply")))
}

fn reply_text(buf: &[u8], at: &mut usize) -> Result<String, Short> {
    let text = reply_line(buf, at)?;
    String::from_utf8(text.to_vec()).map_err(|_| Short::Invalid(proto("invalid text in reply")))
}

fn reply_blob(buf: &[u8], at: &mut usize) -> Result<Option<Bytes>, Short> {
    let len = reply_int(buf, at)?;
    if len < 0 {
        return Ok(None);
    }
    let len = usize::try_from(len).map_err(|_| Short::Invalid(proto("invalid bulk length")))?;
    let end = at.checked_add(len + 2).ok_or(Short::Incomplete)?;
    if buf.len() < end {
        return Err(Short::Incomplete);
    }
    let blob = Bytes::copy_from_slice(&buf[*at..*at + len]);
    *at = end;
    Ok(Some(blob))
}

fn reply_count(buf: &[u8], at: &mut usize) -> Result<Option<usize>, Short> {
    let n = reply_int(buf, at)?;
    if n < 0 {
        return Ok(None);
    }
    usize::try_from(n)
        .map(Some)
        .map_err(|_| Short::Invalid(proto("invalid aggregate length")))
}

fn reply_items(buf: &[u8], at: &mut usize, n: usize, depth: usize) -> Result<Vec<Value>, Short> {
    let mut items = Vec::with_capacity(n.min(PREALLOC_ARGS));
    for _ in 0..n {
        items.push(parse_value(buf, at, depth + 1)?);
    }
    Ok(items)
}

fn parse_value(buf: &[u8], at: &mut usize, depth: usize) -> Result<Value, Short> {
    if depth > MAX_DEPTH {
        return Err(Short::Invalid(proto("reply nested too deeply")));
    }
    let kind = *buf.get(*at).ok_or(Short::Incomplete)?;
    *at += 1;
    Ok(match kind {
        b'+' => Value::Simple(reply_text(buf, at)?),
        b'-' => Value::Error(reply_text(buf, at)?),
        b':' => Value::Integer(reply_int(buf, at)?),
        b'$' => reply_blob(buf, at)?.map_or(Value::Null, Value::Bulk),
        b'*' => match reply_count(buf, at)? {
            None => Value::NullArray,
            Some(n) => Value::Array(reply_items(buf, at, n, depth)?),
        },
        b'_' => {
            reply_line(buf, at)?;
            Value::Null
        }
        b'%' => {
            let n = reply_count(buf, at)?.unwrap_or(0);
            let mut pairs = Vec::with_capacity(n.min(PREALLOC_ARGS));
            for _ in 0..n {
                let k = parse_value(buf, at, depth + 1)?;
                let v = parse_value(buf, at, depth + 1)?;
                pairs.push((k, v));
            }
            Value::Map(pairs)
        }
        b'~' => {
            let n = reply_count(buf, at)?.unwrap_or(0);
            Value::Set(reply_items(buf, at, n, depth)?)
        }
        b'>' => {
            let n = reply_count(buf, at)?.unwrap_or(0);
            Value::Push(reply_items(buf, at, n, depth)?)
        }
        b',' => {
            let text = reply_text(buf, at)?;
            Value::Double(match text.as_str() {
                "inf" => f64::INFINITY,
                "-inf" => f64::NEG_INFINITY,
                "nan" => f64::NAN,
                t => t
                    .parse()
                    .map_err(|_| Short::Invalid(proto("invalid double in reply")))?,
            })
        }
        b'#' => match reply_line(buf, at)? {
            b"t" => Value::Boolean(true),
            b"f" => Value::Boolean(false),
            _ => return Err(Short::Invalid(proto("invalid boolean in reply"))),
        },
        b'(' => Value::BigNumber(reply_text(buf, at)?),
        b'=' => {
            let blob = reply_blob(buf, at)?.unwrap_or_default();
            if blob.len() < 4 || blob[3] != b':' {
                return Err(Short::Invalid(proto("invalid verbatim string in reply")));
            }
            let format = String::from_utf8_lossy(&blob[..3]).into_owned();
            Value::Verbatim(format, blob.slice(4..))
        }
        other => {
            return Err(Short::Invalid(proto(&format!(
                "invalid reply type byte '{}'",
                other as char
            ))));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_is_strict() {
        assert_eq!(parse_int(b"0"), Some(0));
        assert_eq!(parse_int(b"-12"), Some(-12));
        assert_eq!(parse_int(b"9223372036854775807"), Some(i64::MAX));
        for bad in [
            &b""[..],
            b"-",
            b"+1",
            b" 1",
            b"01",
            b"-0",
            b"1x",
            b"99999999999999999999",
        ] {
            assert_eq!(parse_int(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn doubles_format_like_valkey() {
        for (d, text) in [
            (1.0, "1"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (-2.25, "-2.25"),
            (1e20, "1e+20"),
            (1e16, "10000000000000000"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
        ] {
            assert_eq!(format_double(d), text);
        }
    }

    #[test]
    fn human_floats() {
        assert_eq!(super::format_float_human(10.5 + 0.1), "10.6");
        assert_eq!(super::format_float_human(5200.0), "5200");
        assert_eq!(super::format_float_human(-0.0), "0");
        assert_eq!(super::format_float_human(-1.25), "-1.25");
        assert_eq!(super::format_float_human(1e20), "100000000000000000000");
        assert_eq!(super::format_float_human(1e-7), "0.0000001");
    }
}
