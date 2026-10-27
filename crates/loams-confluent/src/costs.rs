//! The Costs API (`GET /billing/v1/costs`): what Confluent billed, by day.
//!
//! Rules from CC0 (§9 of the spike report):
//! - dates are UTC calendar days, `end_date` exclusive;
//! - one request covers at most a month ("end date and start date are too far
//!   apart"), so [`list`] splits a longer range into windows of at most
//!   [`MAX_WINDOW_DAYS`] days;
//! - lines lag by up to 72 hours, and `PROMO_CREDIT` lines carry no product,
//!   resource, unit or quantity;
//! - EnvironmentAdmin and MetricsViewer keys get 403: this needs an
//!   organisation-level key (BillingAdmin or OrganizationAdmin).

use std::fmt;
use std::str::FromStr;

use reqwest::Method;
use serde_json::Value;

use crate::{Client, Error};

/// The longest window one request asks for.
pub const MAX_WINDOW_DAYS: i64 = 30;

/// A UTC calendar day (`YYYY-MM-DD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day {
    /// Days since 1970-01-01.
    days: i64,
}

impl Day {
    /// The day `y-m-d`, if it exists.
    #[must_use]
    pub fn from_ymd(y: i64, m: u32, d: u32) -> Option<Self> {
        if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
            return None;
        }
        Some(Self {
            days: days_from_civil(y, m, d),
        })
    }

    /// The UTC day that holds `unix_seconds`.
    #[must_use]
    pub fn from_unix(unix_seconds: i64) -> Self {
        Self {
            days: unix_seconds.div_euclid(86_400),
        }
    }

    #[must_use]
    pub fn plus_days(self, n: i64) -> Self {
        Self {
            days: self.days + n,
        }
    }

    /// `other - self` in days.
    #[must_use]
    pub fn days_until(self, other: Day) -> i64 {
        other.days - self.days
    }

    /// (year, month, day).
    #[must_use]
    pub fn ymd(self) -> (i64, u32, u32) {
        civil_from_days(self.days)
    }
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

impl FromStr for Day {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bad = || Error::Usage(format!("'{s}' is not a YYYY-MM-DD date"));
        // Confluent answers dates as `2026-10-24` or `2026-10-24T00:00:00Z`.
        let date = s.get(..10).ok_or_else(bad)?;
        let rest = &s[10..];
        if !(rest.is_empty() || rest.starts_with('T')) {
            return Err(bad());
        }
        let mut it = date.split('-');
        let (Some(y), Some(m), Some(d), None) = (it.next(), it.next(), it.next(), it.next()) else {
            return Err(bad());
        };
        if y.len() != 4 || m.len() != 2 || d.len() != 2 {
            return Err(bad());
        }
        let y: i64 = y.parse().map_err(|_| bad())?;
        let m: u32 = m.parse().map_err(|_| bad())?;
        let d: u32 = d.parse().map_err(|_| bad())?;
        Day::from_ymd(y, m, d).ok_or_else(bad)
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's civil-date algorithms (public domain).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // m is 1..=12 and d 1..=31 by construction; the order is (year, month, day).
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// One billed line.
#[derive(Debug, Clone, PartialEq)]
pub struct CostLine {
    pub start: Day,
    pub end: Day,
    /// `KAFKA`, `CONNECT`, … (empty on `PROMO_CREDIT` lines).
    pub product: String,
    /// `KAFKA_NUM_CKUS`, `KAFKA_NETWORK_READ`, `KAFKA_STORAGE`, `PROMO_CREDIT`, …
    pub line_type: String,
    /// The cluster (`lkc-…`) or other resource; empty when the line has none.
    pub resource_id: String,
    pub resource_name: String,
    /// The resource's environment, when the line names one.
    pub environment: String,
    pub quantity: Option<f64>,
    pub unit: String,
    pub price: Option<f64>,
    /// Before discounts and credits.
    pub original_amount: f64,
    pub discount_amount: f64,
    /// What is owed after discounts (credits are separate lines).
    pub amount: f64,
}

fn parse_line(v: &Value) -> Result<CostLine, Error> {
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    let decode = |m: String| Error::Decode {
        what: "read a cost line".into(),
        message: m,
    };
    let start: Day = v["start_date"]
        .as_str()
        .ok_or_else(|| decode("no start_date".into()))?
        .parse()
        .map_err(|e: Error| decode(e.to_string()))?;
    let end: Day = v["end_date"]
        .as_str()
        .ok_or_else(|| decode("no end_date".into()))?
        .parse()
        .map_err(|e: Error| decode(e.to_string()))?;
    Ok(CostLine {
        start,
        end,
        product: s(&v["product"]),
        line_type: s(&v["line_type"]),
        resource_id: s(&v["resource"]["id"]),
        resource_name: s(&v["resource"]["display_name"]),
        environment: s(&v["resource"]["environment"]["id"]),
        quantity: v["quantity"].as_f64(),
        unit: s(&v["unit"]),
        price: v["price"].as_f64(),
        original_amount: v["original_amount"].as_f64().unwrap_or(0.0),
        discount_amount: v["discount_amount"].as_f64().unwrap_or(0.0),
        amount: v["amount"].as_f64().unwrap_or(0.0),
    })
}

/// The `[start, end)` windows of at most [`MAX_WINDOW_DAYS`] days covering the range.
#[must_use]
pub fn windows(start: Day, end: Day) -> Vec<(Day, Day)> {
    let mut out = Vec::new();
    let mut from = start;
    while from < end {
        let to = from.plus_days(MAX_WINDOW_DAYS).min(end);
        out.push((from, to));
        from = to;
    }
    out
}

/// Every cost line for the UTC days `[start, end)`, every page, oldest window first.
pub async fn list(c: &Client, start: Day, end: Day) -> Result<Vec<CostLine>, Error> {
    if end <= start {
        return Err(Error::Usage(format!(
            "the end date {end} is not after the start date {start} (the end is exclusive)"
        )));
    }
    let mut out = Vec::new();
    for (from, to) in windows(start, end) {
        let what = format!("list the costs from {from} to {to}");
        let mut path = format!("/billing/v1/costs?start_date={from}&end_date={to}&page_size=10000");
        let mut pages = 0;
        loop {
            let v = c.call(Method::GET, &path, None, &what).await?;
            for line in v["data"].as_array().map(Vec::as_slice).unwrap_or_default() {
                out.push(parse_line(line)?);
            }
            match v["metadata"]["next"].as_str() {
                Some(next) if !next.is_empty() => path = next.to_string(),
                _ => break,
            }
            pages += 1;
            if pages >= 1000 {
                return Err(Error::Decode {
                    what,
                    message: "more than 1000 pages".into(),
                });
            }
        }
    }
    Ok(out)
}

/// The sum of `amount` per resource id (lines with no resource, such as
/// credits, under the empty string).
#[must_use]
pub fn total_by_resource(lines: &[CostLine]) -> std::collections::BTreeMap<String, f64> {
    let mut out = std::collections::BTreeMap::new();
    for l in lines {
        *out.entry(l.resource_id.clone()).or_insert(0.0) += l.amount;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip() {
        for s in ["1970-01-01", "2000-02-29", "2026-10-27", "2100-12-31"] {
            let d: Day = s.parse().unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(d.to_string(), s);
        }
        assert_eq!("1970-01-02".parse::<Day>().map(|d| d.days).ok(), Some(1));
        assert_eq!(Day::from_unix(1_800_000_000).to_string(), "2027-01-15");
        assert!("2026-02-29".parse::<Day>().is_err());
        assert!("2026-13-01".parse::<Day>().is_err());
        assert!("2026-1-01".parse::<Day>().is_err());
        assert!("2026-10-24x".parse::<Day>().is_err());
        assert_eq!(
            "2026-10-24T00:00:00Z"
                .parse::<Day>()
                .map(|d| d.to_string())
                .ok(),
            Some("2026-10-24".into())
        );
    }

    #[test]
    fn windows_split_a_long_range_end_exclusive() {
        let a: Day = "2026-01-01".parse().unwrap_or_else(|e| panic!("{e}"));
        let b: Day = "2026-03-15".parse().unwrap_or_else(|e| panic!("{e}"));
        let w = windows(a, b);
        assert_eq!(w.first().map(|x| x.0), Some(a));
        assert_eq!(w.last().map(|x| x.1), Some(b));
        for pair in w.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "contiguous");
        }
        assert!(w.iter().all(|(f, t)| f.days_until(*t) <= MAX_WINDOW_DAYS));
        assert!(windows(a, a).is_empty());
    }

    #[test]
    fn promo_credit_lines_parse_without_a_resource() {
        let v: Value = serde_json::from_str(
            r#"{"start_date":"2026-10-24","end_date":"2026-10-25","line_type":"PROMO_CREDIT",
                "original_amount":0,"amount":-0.0001}"#,
        )
        .unwrap_or_default();
        let l = parse_line(&v).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(l.line_type, "PROMO_CREDIT");
        assert!(l.resource_id.is_empty() && l.quantity.is_none());
        assert!((l.amount + 0.0001).abs() < 1e-12);
    }
}
