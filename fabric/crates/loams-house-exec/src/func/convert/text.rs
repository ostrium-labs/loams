//! Reading values from text as ClickHouse's conversions do (`readIntText`,
//! `readFloatText`, `readDecimalText`, `readDateText`, `readDateTimeText`,
//! `src/IO/ReadHelpers.h`, Apache-2.0), and writing them back.
//!
//! A conversion from `String` must read the whole string: `'12abc'` is not a
//! number (`6 CANNOT_PARSE_TEXT`), and neither is `' 12'`.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use loams_house_types::ChError;
use loams_house_types::text::{date_text, datetime_text, decimal_text, float32, float64};
use loams_house_types::ty::quote;

use super::value::Value;
use crate::func::typed::SESSION_TIMEZONE;

/// `6 CANNOT_PARSE_TEXT`, as ClickHouse words it for `type_name`.
pub fn cannot_parse(text: &[u8], type_name: &str) -> ChError {
    ChError::cannot_parse_text(format!(
        "Cannot parse string '{}' as {type_name}: syntax error",
        String::from_utf8_lossy(text)
    ))
}

/// An integer's text: `[+-]digits`, the whole of it. Digits past `i128`'s
/// range wrap, as ClickHouse's do past the target's.
pub fn parse_int(text: &[u8]) -> Option<i128> {
    let (negative, digits) = match text.first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut v: i128 = 0;
    for d in digits {
        v = v.wrapping_mul(10).wrapping_add(i128::from(d - b'0'));
    }
    Some(if negative { v.wrapping_neg() } else { v })
}

/// A float's text: what Rust reads, without spaces (`1e3`, `.5`, `inf`,
/// `-nan`), the whole of it.
pub fn parse_float(text: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(text).ok()?;
    if s.is_empty() || s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "nan" | "+nan" | "-nan" => return Some(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    if lower.starts_with("0x") || lower.contains("nan") || lower.contains("inf") {
        return None;
    }
    f64::from_str(s).ok()
}

/// A decimal's text at `scale`: `[+-]digits[.digits][e±n]`, digits past the
/// scale dropped (truncated, as `readDecimalText` does). `None` when it is
/// not a number; `Some(Err)` when it has more digits than 38.
pub fn parse_decimal(text: &[u8], scale: u8) -> Option<Result<i128, ChError>> {
    let (negative, rest) = match text.first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let (mantissa, exp) = match rest.iter().position(|c| *c == b'e' || *c == b'E') {
        Some(i) => (&rest[..i], Some(parse_int(&rest[i + 1..])?)),
        None => (rest, None),
    };
    let (int, frac) = match mantissa.iter().position(|c| *c == b'.') {
        Some(i) => (&mantissa[..i], &mantissa[i + 1..]),
        None => (mantissa, &mantissa[mantissa.len()..]),
    };
    if (int.is_empty() && frac.is_empty())
        || !int.iter().all(u8::is_ascii_digit)
        || !frac.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    // Digits and the position of the point, shifted by the exponent.
    let digits: Vec<u8> = int.iter().chain(frac).map(|d| d - b'0').collect();
    let point = int.len() as i128 + exp.unwrap_or(0);
    let keep = point + i128::from(scale);
    let mut v: i128 = 0;
    let mut count = 0usize;
    for i in 0..keep.max(0) {
        let d = usize::try_from(i)
            .ok()
            .and_then(|i| digits.get(i))
            .copied()
            .unwrap_or(0);
        if v != 0 || d != 0 {
            count += 1;
        }
        if count > 38 {
            return Some(Err(ChError::argument_out_of_bound(format!(
                "Too many digits in decimal value '{}'",
                String::from_utf8_lossy(text)
            ))));
        }
        v = v * 10 + i128::from(d);
    }
    Some(Ok(if negative { -v } else { v }))
}

/// `Bool`'s text, as `SerializationBool` reads it.
pub fn parse_bool(text: &[u8]) -> Option<bool> {
    let s = std::str::from_utf8(text).ok()?.to_ascii_lowercase();
    match s.as_str() {
        "true" | "1" | "yes" | "on" | "enable" | "enabled" | "t" | "y" => Some(true),
        "false" | "0" | "no" | "off" | "disable" | "disabled" | "f" | "n" => Some(false),
        _ => None,
    }
}

fn digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse().ok()
}

/// A date's text: `YYYY-MM-DD` (any one non-digit between the parts, months
/// and days of one or two digits) or `YYYYMMDD`. Returns the date and the
/// rest of the text.
pub fn parse_date_prefix(text: &[u8]) -> Option<(NaiveDate, &[u8])> {
    if text.len() >= 8 && text[..8].iter().all(u8::is_ascii_digit) {
        let y = digits(&text[..4])? as i32;
        let m = digits(&text[4..6])?;
        let d = digits(&text[6..8])?;
        return Some((NaiveDate::from_ymd_opt(y, m, d)?, &text[8..]));
    }
    if text.len() < 8 || !text[..4].iter().all(u8::is_ascii_digit) || text[4].is_ascii_digit() {
        return None;
    }
    let y = digits(&text[..4])? as i32;
    let mut i = 5;
    let start = i;
    while i < text.len() && i < start + 2 && text[i].is_ascii_digit() {
        i += 1;
    }
    let m = digits(&text[start..i])?;
    if i >= text.len() || text[i].is_ascii_digit() {
        return None;
    }
    i += 1;
    let start = i;
    while i < text.len() && i < start + 2 && text[i].is_ascii_digit() {
        i += 1;
    }
    let d = digits(&text[start..i])?;
    Some((NaiveDate::from_ymd_opt(y, m, d)?, &text[i..]))
}

/// A time of day after a date: `[ T]hh:mm:ss` and an optional fraction, or
/// nothing. Returns the time and the fraction's digits.
fn parse_time_suffix(rest: &[u8]) -> Option<(NaiveTime, &[u8])> {
    if rest.is_empty() {
        return Some((NaiveTime::MIN, &[]));
    }
    if !matches!(rest[0], b' ' | b'T') || rest.len() < 9 {
        return None;
    }
    let t = &rest[1..];
    if t[2] != b':' || t[5] != b':' {
        return None;
    }
    let h = digits(&t[..2])?;
    let m = digits(&t[3..5])?;
    let s = digits(&t[6..8])?;
    let mut frac: &[u8] = &[];
    let mut tail = &t[8..];
    if let Some(b'.') = tail.first() {
        let n = tail[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        frac = &tail[1..1 + n];
        tail = &tail[1 + n..];
    }
    // A trailing `Z` is UTC, which the zones here read as written.
    if tail == b"Z" {
        tail = &[];
    }
    if !tail.is_empty() {
        return None;
    }
    Some((NaiveTime::from_hms_opt(h, m, s)?, frac))
}

/// A date for `Date`/`Date32`: a date's text, with any time after it
/// ignored.
pub fn parse_date(text: &[u8]) -> Option<i32> {
    let (date, rest) = parse_date_prefix(text)?;
    parse_time_suffix(rest)?;
    Some(days(date))
}

/// Days since 1970-01-01 of `date`.
pub fn days(date: NaiveDate) -> i32 {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or(NaiveDate::MIN);
    (date - epoch).num_days() as i32
}

/// The zone named `tz`, or the session's.
pub fn zone(tz: Option<&str>) -> Result<Tz, ChError> {
    let name = tz.unwrap_or(SESSION_TIMEZONE);
    Tz::from_str(name).map_err(|_| ChError::bad_arguments(format!("Unknown time zone {name}")))
}

/// Seconds since the epoch of local time `t` in `zone` (the earlier instant
/// when it is ambiguous, the instant after a gap).
pub fn local_seconds(t: NaiveDateTime, zone: Tz) -> i64 {
    match zone.from_local_datetime(&t) {
        chrono::LocalResult::Single(dt) => dt.timestamp(),
        chrono::LocalResult::Ambiguous(a, _) => a.timestamp(),
        chrono::LocalResult::None => {
            // A gap: an hour later is past it.
            zone.from_local_datetime(&(t + chrono::TimeDelta::hours(1)))
                .earliest()
                .map(|dt| dt.timestamp() - 3600)
                .unwrap_or_else(|| t.and_utc().timestamp())
        }
    }
}

/// The local date of `secs` since the epoch in `zone`, as days.
pub fn local_days(secs: i64, zone: Tz) -> i32 {
    match DateTime::<Utc>::from_timestamp(secs, 0) {
        Some(utc) => days(utc.with_timezone(&zone).date_naive()),
        None => (secs.div_euclid(86_400)) as i32,
    }
}

/// A date and time for `DateTime64(precision)` in `zone`, as ticks: a date
/// with an optional time and fraction, or the seconds since the epoch alone
/// (`'1700000000'`, with an optional fraction).
pub fn parse_datetime(text: &[u8], precision: u8, zone: Tz) -> Option<i64> {
    let scale = 10i64.pow(u32::from(precision));
    let int_len = text.iter().take_while(|c| c.is_ascii_digit()).count();
    if (int_len > 0 && int_len != 4 && int_len != 8) || (int_len == text.len() && int_len != 8) {
        // Seconds since the epoch.
        let secs: i64 = std::str::from_utf8(&text[..int_len]).ok()?.parse().ok()?;
        let rest = &text[int_len..];
        let frac = match rest.first() {
            None => &rest[..0],
            Some(b'.') if rest[1..].iter().all(u8::is_ascii_digit) => &rest[1..],
            _ => return None,
        };
        return Some(secs.saturating_mul(scale) + fraction(frac, precision));
    }
    let (date, rest) = parse_date_prefix(text)?;
    let (time, frac) = parse_time_suffix(rest)?;
    let secs = local_seconds(date.and_time(time), zone);
    Some(secs.saturating_mul(scale) + fraction(frac, precision))
}

/// The first `precision` digits of a fraction, as ticks.
fn fraction(frac: &[u8], precision: u8) -> i64 {
    let mut v = 0i64;
    for i in 0..usize::from(precision) {
        let d = frac.get(i).map(|c| i64::from(c - b'0')).unwrap_or(0);
        v = v * 10 + d;
    }
    v
}

/// A UUID's text: 36 characters with dashes, or 32 hex digits.
pub fn parse_uuid(text: &[u8]) -> Option<[u8; 16]> {
    let hex: Vec<u8> = match text.len() {
        36 => {
            if [8, 13, 18, 23].iter().any(|i| text[*i] != b'-') {
                return None;
            }
            text.iter().copied().filter(|c| *c != b'-').collect()
        }
        32 => text.to_vec(),
        _ => return None,
    };
    let mut out = [0u8; 16];
    for (i, pair) in hex.chunks(2).enumerate() {
        let s = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// An IPv4 address's text.
pub fn parse_ipv4(text: &[u8]) -> Option<u32> {
    Ipv4Addr::from_str(std::str::from_utf8(text).ok()?)
        .ok()
        .map(u32::from)
}

/// An IPv6 address's text, or an IPv4 address's (mapped).
pub fn parse_ipv6(text: &[u8]) -> Option<[u8; 16]> {
    let s = std::str::from_utf8(text).ok()?;
    if let Ok(v6) = Ipv6Addr::from_str(s) {
        return Some(v6.octets());
    }
    Ipv4Addr::from_str(s)
        .ok()
        .map(|v4| v4.to_ipv6_mapped().octets())
}

/// A composite value's text in ClickHouse's quoted style (`[1,'a',NULL]`,
/// `(1,'x')`): atoms are quoted strings or bare tokens.
#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    /// `NULL`.
    Null,
    /// A quoted string, unescaped, or a bare token.
    Text(Vec<u8>),
    /// `[…]` or `(…)`.
    List(Vec<Atom>),
}

/// Reads a whole composite value's text.
pub fn parse_quoted(text: &[u8]) -> Option<Atom> {
    let mut p = QuotedParser { s: text, i: 0 };
    let a = p.atom(0)?;
    p.ws();
    (p.i == text.len()).then_some(a)
}

struct QuotedParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl QuotedParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn atom(&mut self, depth: usize) -> Option<Atom> {
        if depth > 256 {
            return None;
        }
        self.ws();
        match *self.s.get(self.i)? {
            open @ (b'[' | b'(') => {
                let close = if open == b'[' { b']' } else { b')' };
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&close) {
                    self.i += 1;
                    return Some(Atom::List(items));
                }
                loop {
                    items.push(self.atom(depth + 1)?);
                    self.ws();
                    match *self.s.get(self.i)? {
                        b',' => self.i += 1,
                        c if c == close => {
                            self.i += 1;
                            return Some(Atom::List(items));
                        }
                        _ => return None,
                    }
                }
            }
            b'\'' => {
                self.i += 1;
                let mut out = Vec::new();
                loop {
                    let c = *self.s.get(self.i)?;
                    self.i += 1;
                    match c {
                        b'\'' => {
                            // `''` inside a string is one quote.
                            if self.s.get(self.i) == Some(&b'\'') {
                                out.push(b'\'');
                                self.i += 1;
                            } else {
                                return Some(Atom::Text(out));
                            }
                        }
                        b'\\' => {
                            let e = *self.s.get(self.i)?;
                            self.i += 1;
                            out.push(match e {
                                b'n' => b'\n',
                                b't' => b'\t',
                                b'r' => b'\r',
                                b'0' => 0,
                                b'b' => 8,
                                b'f' => 12,
                                other => other,
                            });
                        }
                        other => out.push(other),
                    }
                }
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && !matches!(self.s[self.i], b',' | b']' | b')')
                    && !self.s[self.i].is_ascii_whitespace()
                {
                    self.i += 1;
                }
                let token = &self.s[start..self.i];
                if token.is_empty() {
                    return None;
                }
                if token.eq_ignore_ascii_case(b"NULL") {
                    Some(Atom::Null)
                } else {
                    Some(Atom::Text(token.to_vec()))
                }
            }
        }
    }
}

/// How a value of `t`'s family is written as text by `toString`: plain for
/// the top level, quoted inside composites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `toString(x)`: strings and dates bare.
    Plain,
    /// Inside an array or tuple: strings and dates in single quotes.
    Quoted,
}

/// Formatting context for [`write`].
#[derive(Debug, Clone)]
pub struct TextOf<'a> {
    /// Whether the source float was a `Float32`.
    pub float32: bool,
    /// The zone of the source `DateTime`.
    pub tz: Option<&'a str>,
}

/// `v`'s text in `style`.
pub fn write(v: &Value, how: &TextOf<'_>, style: Style) -> Result<Vec<u8>, ChError> {
    let quoted = |s: String| -> Vec<u8> {
        match style {
            Style::Plain => s.into_bytes(),
            Style::Quoted => quote(&s).into_bytes(),
        }
    };
    Ok(match v {
        Value::Null => match style {
            Style::Plain => b"\\N".to_vec(),
            Style::Quoted => b"NULL".to_vec(),
        },
        Value::Int(i) => i.to_string().into_bytes(),
        Value::Float(f) => {
            if how.float32 {
                float32(*f as f32).into_bytes()
            } else {
                float64(*f).into_bytes()
            }
        }
        Value::Bool(b) => {
            let text: &str = if *b { "true" } else { "false" };
            text.as_bytes().to_vec()
        }
        Value::Dec(u, s) => decimal_text(&u.to_string(), *s, false).into_bytes(),
        Value::Str(b) => match style {
            Style::Plain => b.clone(),
            Style::Quoted => quote(&String::from_utf8_lossy(b)).into_bytes(),
        },
        Value::Date(d) => quoted(date_text(*d)),
        Value::Time(ticks, p) => {
            let scale = 10i64.pow(u32::from(*p));
            let secs = ticks.div_euclid(scale);
            let frac = ticks.rem_euclid(scale);
            quoted(datetime_text(
                secs,
                frac,
                *p,
                how.tz.unwrap_or(SESSION_TIMEZONE),
            )?)
        }
        Value::Uuid(b) => {
            let mut s = String::with_capacity(36);
            for (i, byte) in b.iter().enumerate() {
                if matches!(i, 4 | 6 | 8 | 10) {
                    s.push('-');
                }
                let _ = write!(s, "{byte:02x}");
            }
            quoted(s)
        }
        Value::Ip4(v) => quoted(Ipv4Addr::from(*v).to_string()),
        Value::Ip6(b) => quoted(Ipv6Addr::from(*b).to_string()),
        Value::Enum(_, name) => quoted(name.clone()),
        Value::Array(items) => {
            let mut out = vec![b'['];
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(write(item, how, Style::Quoted)?);
            }
            out.push(b']');
            out
        }
        Value::Tuple(items) => {
            let mut out = vec![b'('];
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(write(item, how, Style::Quoted)?);
            }
            out.push(b')');
            out
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn integers_read_whole() {
        assert_eq!(parse_int(b"42"), Some(42));
        assert_eq!(parse_int(b"-7"), Some(-7));
        assert_eq!(parse_int(b"+7"), Some(7));
        assert_eq!(parse_int(b""), None);
        assert_eq!(parse_int(b"12abc"), None);
        assert_eq!(parse_int(b" 1"), None);
        assert_eq!(parse_int(b"1.5"), None);
    }

    #[test]
    fn floats_read_as_clickhouse() {
        assert_eq!(parse_float(b"1.5"), Some(1.5));
        assert_eq!(parse_float(b"1e3"), Some(1000.0));
        assert_eq!(parse_float(b"-inf"), Some(f64::NEG_INFINITY));
        assert!(parse_float(b"nan").unwrap().is_nan());
        assert_eq!(parse_float(b"abc"), None);
        assert_eq!(parse_float(b"0x10"), None);
        assert_eq!(parse_float(b" 1"), None);
    }

    #[test]
    fn decimals_truncate_past_the_scale() {
        assert_eq!(parse_decimal(b"1.239", 2).unwrap().unwrap(), 123);
        assert_eq!(parse_decimal(b"-1.2", 3).unwrap().unwrap(), -1200);
        assert_eq!(parse_decimal(b"12", 0).unwrap().unwrap(), 12);
        assert_eq!(parse_decimal(b"1.5e2", 1).unwrap().unwrap(), 1500);
        assert!(parse_decimal(b"x", 2).is_none());
    }

    #[test]
    fn dates_and_times_read() {
        assert_eq!(parse_date(b"1970-01-02"), Some(1));
        assert_eq!(
            parse_date(b"2026-1-2"),
            Some(parse_date(b"2026-01-02").unwrap())
        );
        assert_eq!(parse_date(b"20260102"), parse_date(b"2026-01-02"));
        assert_eq!(
            parse_date(b"2026-01-02 10:00:00"),
            parse_date(b"2026-01-02")
        );
        assert_eq!(parse_date(b"2026-13-02"), None);
        assert_eq!(parse_date(b"abc"), None);
        let utc = zone(None).unwrap();
        assert_eq!(parse_datetime(b"1970-01-01 00:00:10", 0, utc), Some(10));
        assert_eq!(
            parse_datetime(b"1970-01-01 00:00:10.25", 3, utc),
            Some(10_250)
        );
        assert_eq!(parse_datetime(b"1970-01-02", 0, utc), Some(86_400));
        assert_eq!(parse_datetime(b"1700000000", 0, utc), Some(1_700_000_000));
        assert_eq!(parse_datetime(b"2026-01-01 25:00:00", 0, utc), None);
        let berlin = zone(Some("Europe/Berlin")).unwrap();
        assert_eq!(parse_datetime(b"1970-01-01 01:00:00", 0, berlin), Some(0));
    }

    #[test]
    fn uuids_and_addresses_read() {
        let u = parse_uuid(b"61f0c404-5cb3-11e7-907b-a6006ad3dba0").unwrap();
        assert_eq!(u[0], 0x61);
        assert_eq!(u[15], 0xa0);
        assert!(parse_uuid(b"61f0c404").is_none());
        assert_eq!(parse_ipv4(b"1.2.3.4"), Some(0x0102_0304));
        assert_eq!(parse_ipv6(b"1.2.3.4").unwrap()[10..12], [0xff, 0xff]);
    }

    #[test]
    fn composites_read_and_write() {
        let a = parse_quoted(b"[1, 'a\\'b', NULL]").unwrap();
        assert_eq!(
            a,
            Atom::List(vec![
                Atom::Text(b"1".to_vec()),
                Atom::Text(b"a'b".to_vec()),
                Atom::Null
            ])
        );
        assert!(parse_quoted(b"[1,").is_none());
        let how = TextOf {
            float32: false,
            tz: None,
        };
        let v = Value::Array(vec![Value::Str(b"x".to_vec()), Value::Int(2)]);
        assert_eq!(write(&v, &how, Style::Plain).unwrap(), b"['x',2]");
        assert_eq!(
            write(&Value::Date(0), &how, Style::Plain).unwrap(),
            b"1970-01-01"
        );
        assert_eq!(
            write(&Value::Time(1_500, 3), &how, Style::Plain).unwrap(),
            b"1970-01-01 00:00:01.500"
        );
        assert_eq!(parse_bool(b"True"), Some(true));
        assert_eq!(parse_bool(b"maybe"), None);
    }
}
