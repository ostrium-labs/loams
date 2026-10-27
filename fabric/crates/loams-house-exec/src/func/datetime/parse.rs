//! Dates and times from text: ClickHouse's basic `DateTime` text (what
//! `toDateTime('…')` and `toDate('…')` read, `date_time_input_format =
//! basic`), and `parseDateTimeBestEffort`'s wider set of forms.
//!
//! The basic reader follows `readDateTimeText` (ClickHouse
//! `src/IO/ReadHelpers.h`, Apache-2.0): `YYYY-MM-DD[ hh:mm[:ss[.frac]]]`
//! with any one-character separators, `YYYYMMDD[hhmmss]`, or a Unix
//! timestamp, and a trailing `Z` or `±hh[:mm]` offset. The best-effort
//! reader takes the forms `parseDateTimeBestEffort` documents (ClickHouse
//! `src/IO/parseDateTimeBestEffort.cpp`, Apache-2.0): ISO 8601, RFC 1123,
//! month names, `DD/MM/YYYY` (`MM/DD/YYYY` in the `US` variants), `AM`/`PM`,
//! and Unix timestamps; R30.6 lists what it does not take.

use super::zone::{DAY, days, month_days};

/// A civil date and time read from text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Civil {
    /// The day number.
    pub day: i64,
    /// Seconds into the day.
    pub sod: i64,
    /// Nanoseconds into the second.
    pub nanos: u32,
    /// The text's own offset from UTC, in seconds east, if it gave one;
    /// else the time is local to the zone the caller reads it in.
    pub offset: Option<i64>,
    /// The text was a Unix timestamp: `day`/`sod` are UTC.
    pub unix: bool,
}

impl Civil {
    /// The instant, reading a local time in `tz`.
    pub fn instant(&self, tz: chrono_tz::Tz) -> i64 {
        match self.offset {
            Some(o) => self.day * DAY + self.sod - o,
            None if self.unix => self.day * DAY + self.sod,
            None => super::zone::to_utc(tz, self.day, self.sod),
        }
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eof(&self) -> bool {
        self.i >= self.b.len()
    }

    /// Up to `max` digits; their value and count.
    fn digits(&mut self, max: usize) -> (i64, usize) {
        let mut v = 0i64;
        let mut n = 0;
        while n < max
            && let Some(c) = self.peek()
            && c.is_ascii_digit()
        {
            v = v * 10 + i64::from(c - b'0');
            self.i += 1;
            n += 1;
        }
        (v, n)
    }

    /// A fraction's digits as nanoseconds (more than nine are dropped).
    fn fraction(&mut self) -> u32 {
        let mut nanos = 0u32;
        let mut n = 0;
        while let Some(c) = self.peek()
            && c.is_ascii_digit()
        {
            if n < 9 {
                nanos = nanos * 10 + u32::from(c - b'0');
            }
            n += 1;
            self.i += 1;
        }
        while n < 9 {
            nanos *= 10;
            n += 1;
        }
        nanos
    }

    /// A `Z` or `±hh[[:]mm]` offset, if one is next.
    fn offset(&mut self) -> Option<Option<i64>> {
        match self.peek()? {
            b'Z' | b'z' => {
                self.i += 1;
                Some(Some(0))
            }
            c @ (b'+' | b'-') => {
                self.i += 1;
                let (h, n) = self.digits(2);
                if n == 0 {
                    return None;
                }
                if self.peek() == Some(b':') {
                    self.i += 1;
                }
                let (m, _) = self.digits(2);
                let o = h * 3600 + m * 60;
                Some(Some(if c == b'-' { -o } else { o }))
            }
            _ => None,
        }
    }
}

/// A valid day number of a year, month and day, or `None` when the month or
/// the day is out of range (ClickHouse reads such text as the zero date).
fn valid_day(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || d < 1 || d > i64::from(month_days(y, m as u32)) {
        return None;
    }
    Some(days(y, m as u32, d as u32))
}

/// Reads ClickHouse's basic `DateTime` text. `None` when it is not one.
/// A date out of range (`2024-13-45`) reads as the zero date, day 0.
pub fn basic(text: &str) -> Option<Civil> {
    let mut c = Cursor {
        b: text.as_bytes(),
        i: 0,
    };
    let all_digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if all_digits && text.len() != 8 && text.len() != 14 {
        // A Unix timestamp.
        let secs: i64 = text.parse().ok()?;
        return Some(Civil {
            day: secs.div_euclid(DAY),
            sod: secs.rem_euclid(DAY),
            unix: true,
            ..Civil::default()
        });
    }
    let (y, n) = c.digits(4);
    if n != 4 {
        return None;
    }
    let (m, d);
    if c.peek().is_some_and(|b| b.is_ascii_digit()) {
        // YYYYMMDD
        let (mm, n1) = c.digits(2);
        let (dd, n2) = c.digits(2);
        if n1 != 2 || n2 != 2 {
            return None;
        }
        (m, d) = (mm, dd);
    } else {
        c.i += 1;
        let (mm, n1) = c.digits(2);
        if n1 == 0 || c.eof() {
            return None;
        }
        c.i += 1;
        let (dd, n2) = c.digits(2);
        if n2 == 0 {
            return None;
        }
        (m, d) = (mm, dd);
    }
    let day = valid_day(y, m, d);
    let mut out = Civil {
        day: day.unwrap_or(0),
        ..Civil::default()
    };
    if c.eof() {
        return Some(out);
    }
    // The time: after a separator, or straight on in YYYYMMDDhhmmss.
    if !c.peek().is_some_and(|b| b.is_ascii_digit()) {
        c.i += 1;
    }
    let (h, n) = c.digits(2);
    if n == 0 {
        return None;
    }
    let compact = c.peek().is_some_and(|b| b.is_ascii_digit());
    if !compact && c.peek() == Some(b':') {
        c.i += 1;
    } else if !compact {
        return None;
    }
    let (mi, n) = c.digits(2);
    if n == 0 {
        return None;
    }
    let mut s = 0;
    if c.peek() == Some(b':') || (compact && c.peek().is_some_and(|b| b.is_ascii_digit())) {
        if c.peek() == Some(b':') {
            c.i += 1;
        }
        let (ss, n) = c.digits(2);
        if n == 0 {
            return None;
        }
        s = ss;
    }
    if c.peek() == Some(b'.') {
        c.i += 1;
        out.nanos = c.fraction();
    }
    if let Some(o) = c.offset() {
        out.offset = o;
    }
    if !c.eof() {
        return None;
    }
    if day.is_none() {
        return Some(Civil::default());
    }
    out.sod = h * 3600 + mi * 60 + s;
    Some(out)
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Reads text the way `parseDateTimeBestEffort` does (`us`: the `US`
/// variants, `MM/DD/YYYY`). `current_year` fills a missing year.
pub fn best_effort(text: &str, us: bool, current_year: i64) -> Option<Civil> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return match text.len() {
            8 | 14 => basic(text),
            6 => {
                // YYYYMM
                let y: i64 = text[..4].parse().ok()?;
                let m: i64 = text[4..].parse().ok()?;
                Some(Civil {
                    day: valid_day(y, m, 1)?,
                    ..Civil::default()
                })
            }
            1..=10 => basic(text),
            _ => None,
        };
    }
    let mut c = Cursor {
        b: text.as_bytes(),
        i: 0,
    };
    let (mut year, mut month, mut dom) = (None::<i64>, None::<i64>, None::<i64>);
    let (mut hour, mut minute, mut second, mut nanos) = (None::<i64>, 0i64, 0i64, 0u32);
    let mut offset = None::<i64>;
    let mut pm = None::<bool>;
    while !c.eof() {
        let b = c.peek()?;
        if b.is_ascii_digit() {
            let start = c.i;
            let (v, n) = c.digits(usize::MAX);
            match c.peek() {
                Some(b':') if n <= 2 && hour.is_none() => {
                    c.i += 1;
                    let (mi, n2) = c.digits(2);
                    if n2 == 0 {
                        return None;
                    }
                    hour = Some(v);
                    minute = mi;
                    if c.peek() == Some(b':') {
                        c.i += 1;
                        let (s, n3) = c.digits(2);
                        if n3 == 0 {
                            return None;
                        }
                        second = s;
                    }
                    if matches!(c.peek(), Some(b'.' | b',')) {
                        c.i += 1;
                        nanos = c.fraction();
                    }
                }
                Some(sep @ (b'-' | b'/' | b'.')) if year.is_none() && month.is_none() => {
                    c.i += 1;
                    if n == 4 {
                        // YYYY-MM-DD, YYYY/MM/DD
                        year = Some(v);
                        let (m, n2) = c.digits(2);
                        if n2 == 0 {
                            month = Some(month_word(&mut c)?);
                        } else {
                            month = Some(m);
                        }
                        if c.peek() == Some(sep) || c.peek() == Some(b'-') {
                            c.i += 1;
                            let (d, n3) = c.digits(2);
                            if n3 == 0 {
                                return None;
                            }
                            dom = Some(d);
                        } else {
                            dom = Some(1);
                        }
                    } else if n <= 2 {
                        // DD/MM/YYYY (MM/DD/YYYY in the US variants), DD-Mon-YYYY
                        let (m, n2) = c.digits(2);
                        let second_part = if n2 == 0 { month_word(&mut c)? } else { m };
                        if n2 == 0 || !us {
                            dom = Some(v);
                            month = Some(second_part);
                        } else {
                            month = Some(v);
                            dom = Some(second_part);
                        }
                        if matches!(c.peek(), Some(b'-' | b'/' | b'.')) {
                            c.i += 1;
                            let (y, n3) = c.digits(4);
                            year = Some(match n3 {
                                4 => y,
                                2 => 2000 + y,
                                _ => return None,
                            });
                        }
                    } else {
                        return None;
                    }
                }
                _ => match n {
                    4 if year.is_none() => year = Some(v),
                    8 if year.is_none() && month.is_none() => {
                        let s = &text[start..start + 8];
                        year = Some(s[..4].parse().ok()?);
                        month = Some(s[4..6].parse().ok()?);
                        dom = Some(s[6..].parse().ok()?);
                    }
                    6 if hour.is_none() && year.is_some() => {
                        // hhmmss after a date
                        hour = Some(v / 10_000);
                        minute = v / 100 % 100;
                        second = v % 100;
                    }
                    1 | 2 if dom.is_none() => dom = Some(v),
                    _ => return None,
                },
            }
            // An ordinal suffix: 1st, 2nd, 3rd, 4th.
            let rest = &text[c.i..];
            for suffix in ["st", "nd", "rd", "th"] {
                if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(suffix) {
                    c.i += 2;
                    break;
                }
            }
        } else if b.is_ascii_alphabetic() {
            let start = c.i;
            while c.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
                c.i += 1;
            }
            let word = text[start..c.i].to_ascii_lowercase();
            if word == "t" && hour.is_none() {
                continue;
            }
            if word == "z" || word == "utc" || word == "gmt" {
                offset = Some(0);
                continue;
            }
            if word == "am" || word == "pm" {
                pm = Some(word == "pm");
                continue;
            }
            if let Some(m) = month_of(&word) {
                if month.is_some() {
                    return None;
                }
                month = Some(m);
                continue;
            }
            if word.len() >= 3 && WEEKDAYS.iter().any(|d| word.starts_with(d)) {
                continue;
            }
            return None;
        } else if (b == b'+' || b == b'-') && hour.is_some() {
            offset = Some(c.offset()??);
        } else if matches!(b, b' ' | b',' | b'\t' | b'\n') {
            c.i += 1;
        } else {
            return None;
        }
    }
    let mut h = hour.unwrap_or(0);
    match pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    if h > 23 || minute > 59 || second > 59 {
        return None;
    }
    let y = year.unwrap_or(current_year);
    let day = if year.is_none() && month.is_none() && dom.is_none() {
        // A time alone is today in ClickHouse; the zero date here (R30.6).
        0
    } else {
        valid_day(y, month.unwrap_or(1), dom.unwrap_or(1))?
    };
    Some(Civil {
        day,
        sod: h * 3600 + minute * 60 + second,
        nanos,
        offset,
        unix: false,
    })
}

fn month_of(word: &str) -> Option<i64> {
    if word.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| word.starts_with(m))
        .map(|i| i as i64 + 1)
}

fn month_word(c: &mut Cursor<'_>) -> Option<i64> {
    let start = c.i;
    while c.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
        c.i += 1;
    }
    let w = String::from_utf8_lossy(&c.b[start..c.i]).to_ascii_lowercase();
    month_of(&w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_text_reads_as_clickhouse() {
        let d = days(2024, 3, 5);
        assert_eq!(basic("2024-03-05").unwrap().day, d);
        assert_eq!(basic("2024-3-5").unwrap().day, d);
        let t = basic("2024-03-05 10:07:09.123").unwrap();
        assert_eq!((t.day, t.sod, t.nanos), (d, 36_429, 123_000_000));
        assert_eq!(
            basic("2024-03-05T10:00:00+05:30").unwrap().offset,
            Some(19_800)
        );
        assert_eq!(basic("2024-13-45").unwrap().day, 0);
        assert!(basic("abc").is_none());
        assert!(basic("").is_none());
        assert!(basic("1700000000").unwrap().unix);
    }

    #[test]
    fn best_effort_reads_common_forms() {
        let d = days(2024, 3, 5);
        for text in [
            "2024-03-05T10:07:09Z",
            "2024-03-05 10:07:09",
            "5 March 2024 10:07:09",
            "Tue, 05 Mar 2024 10:07:09 GMT",
            "05/03/2024 10:07:09",
            "March 5th, 2024 10:07:09",
            "20240305100709",
        ] {
            let c = best_effort(text, false, 2026).unwrap_or_else(|| panic!("{text}"));
            assert_eq!((c.day, c.sod), (d, 36_429), "{text}");
        }
        assert_eq!(best_effort("03/05/2024", true, 2026).unwrap().day, d);
        assert_eq!(
            best_effort("2024-03-05 10:07:09.123+05:30", false, 2026)
                .unwrap()
                .offset,
            Some(19_800)
        );
        assert_eq!(
            best_effort("2024-03-05 10:07 PM", false, 2026).unwrap().sod,
            79_620
        );
        assert!(best_effort("not a date", false, 2026).is_none());
    }
}
