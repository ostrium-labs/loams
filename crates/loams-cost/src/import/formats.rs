//! Bill formats (design §84 §6.2): which columns each format gives, how a
//! row becomes a [`BillLine`], and format detection by header.
//!
//! - **FOCUS 1.0–1.4**: the core Cost and Usage columns, stable across
//!   versions (`ChargePeriodStart`, `EffectiveCost`, `BilledCost`, …).
//! - **AWS CUR 2.0**: snake_case columns (`line_item_unblended_cost`, …).
//! - **Legacy CUR**: `lineItem/UnblendedCost` style, normalised to CUR 2.0
//!   names; tags from `resourceTags/user:<key>`.
//! - **Generic CSV**: a [`ColumnMap`] names the columns.
//!
//! The CUR column names and the CSV encoding of CUR 2.0's `resource_tags`
//! map are (verify) against AWS's data dictionary; the parser accepts a JSON
//! object or `k=v;k=v`.

use std::collections::{BTreeMap, HashMap};

use loams_meter_core::{Amount, Currency};

use crate::CostError;
use crate::import::{BillLine, ChargeCategory};

/// A bill's format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BillFormat {
    Focus,
    Cur2,
    LegacyCur,
    Csv(Box<ColumnMap>),
}

/// Which generic-CSV column holds each field. `currency` may instead be a
/// constant (`currency=@USD` in the spec string).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnMap {
    pub start: String,
    pub end: String,
    pub cost: String,
    pub currency: Option<String>,
    pub currency_constant: Option<Currency>,
    pub resource: Option<String>,
    pub service: Option<String>,
    pub sku: Option<String>,
    pub region: Option<String>,
    pub quantity: Option<String>,
    pub unit: Option<String>,
    pub tags: Option<String>,
    pub category: Option<String>,
    pub invoice: Option<String>,
}

impl ColumnMap {
    /// Parses `start=From,end=To,cost=Amount,currency=@USD,service=Item`.
    pub fn parse(spec: &str) -> Result<ColumnMap, CostError> {
        let mut m = ColumnMap::default();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| CostError::invalid("map", format!("{part:?} is not key=column")))?;
            let v = v.trim().to_string();
            match k.trim() {
                "start" => m.start = v,
                "end" => m.end = v,
                "cost" => m.cost = v,
                "currency" => {
                    if let Some(c) = v.strip_prefix('@') {
                        m.currency_constant = Some(
                            Currency::parse(c)
                                .map_err(|e| CostError::invalid("map", e.to_string()))?,
                        );
                    } else {
                        m.currency = Some(v);
                    }
                }
                "resource" => m.resource = Some(v),
                "service" => m.service = Some(v),
                "sku" => m.sku = Some(v),
                "region" => m.region = Some(v),
                "quantity" => m.quantity = Some(v),
                "unit" => m.unit = Some(v),
                "tags" => m.tags = Some(v),
                "category" => m.category = Some(v),
                "invoice" => m.invoice = Some(v),
                other => {
                    return Err(CostError::invalid(
                        "map",
                        format!("unknown field {other:?}"),
                    ));
                }
            }
        }
        if m.start.is_empty() || m.end.is_empty() || m.cost.is_empty() {
            return Err(CostError::invalid(
                "map",
                "start, end and cost are required",
            ));
        }
        if m.currency.is_none() && m.currency_constant.is_none() {
            return Err(CostError::invalid(
                "map",
                "currency is required (a column or @CODE)",
            ));
        }
        Ok(m)
    }
}

impl BillFormat {
    /// Detects FOCUS and CUR by their headers; generic CSV needs a map.
    pub fn detect(headers: &[String]) -> Option<BillFormat> {
        let has = |h: &str| headers.iter().any(|x| x == h);
        if has("BilledCost") && has("ChargePeriodStart") {
            Some(BillFormat::Focus)
        } else if has("line_item_unblended_cost") {
            Some(BillFormat::Cur2)
        } else if has("lineItem/UnblendedCost") {
            Some(BillFormat::LegacyCur)
        } else {
            None
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            BillFormat::Focus => "focus",
            BillFormat::Cur2 => "cur2",
            BillFormat::LegacyCur => "cur_legacy",
            BillFormat::Csv(_) => "csv",
        }
    }

    /// Parses a CLI or API format name; `csv` needs `map`.
    pub fn parse(name: &str, map: Option<&str>) -> Result<BillFormat, CostError> {
        Ok(match name {
            "focus" => BillFormat::Focus,
            "cur" | "cur2" => BillFormat::Cur2,
            "cur_legacy" | "legacy_cur" => BillFormat::LegacyCur,
            "csv" => {
                BillFormat::Csv(Box::new(ColumnMap::parse(map.ok_or_else(|| {
                    CostError::invalid("map", "generic CSV needs a column map")
                })?)?))
            }
            other => {
                return Err(CostError::invalid(
                    "format",
                    format!("unknown format {other:?}"),
                ));
            }
        })
    }

    /// A reader for rows under `headers`.
    pub fn reader(&self, headers: &[String]) -> Result<RowReader, CostError> {
        let names: Vec<String> = match self {
            BillFormat::LegacyCur => headers.iter().map(|h| legacy_cur_name(h)).collect(),
            _ => headers.to_vec(),
        };
        let index: HashMap<String, usize> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        let r = RowReader {
            format: self.clone(),
            index,
            legacy_tags: match self {
                BillFormat::LegacyCur => headers
                    .iter()
                    .enumerate()
                    .filter_map(|(i, h)| {
                        h.strip_prefix("resourceTags/")
                            .map(|k| (i, k.strip_prefix("user:").unwrap_or(k).to_string()))
                    })
                    .collect(),
                _ => Vec::new(),
            },
        };
        for required in r.required() {
            if !r.index.contains_key(required.as_str()) {
                return Err(CostError::invalid(
                    "bill",
                    format!("{} file has no {required:?} column", self.name()),
                ));
            }
        }
        Ok(r)
    }
}

/// `lineItem/UsageStartDate` → `line_item_usage_start_date`.
pub fn legacy_cur_name(h: &str) -> String {
    let mut out = String::new();
    for (i, part) in h.split('/').enumerate() {
        if i > 0 {
            out.push('_');
        }
        for (j, c) in part.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if j > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
    }
    out
}

/// Parses a decimal as bills write it (`12`, `-0.5`, `1.2E-5`), rounded
/// half away from zero to nine decimals.
pub fn parse_decimal(s: &str) -> Result<Amount, CostError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Amount::ZERO);
    }
    let bad = || CostError::invalid("amount", format!("{s:?} is not a number"));
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], s[i + 1..].parse::<i32>().map_err(|_| bad())?),
        None => (s, 0),
    };
    let (neg, digits) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa.strip_prefix('+').unwrap_or(mantissa)),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    if int.is_empty() && frac.is_empty() {
        return Err(bad());
    }
    if !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    // value = all_digits × 10^(exp − frac.len()); raw = value × 10^9.
    let all = format!("{int}{frac}");
    let all = all.trim_start_matches('0');
    let shift = exp + 9 - i32::try_from(frac.len()).map_err(|_| bad())?;
    let mut raw: i128 = 0;
    if !all.is_empty() {
        if shift >= 0 {
            raw = all.parse::<i128>().map_err(|_| bad())?;
            for _ in 0..shift {
                raw = raw.checked_mul(10).ok_or(CostError::Overflow)?;
            }
        } else {
            let drop = usize::try_from(-shift).map_err(|_| bad())?;
            if drop > all.len() {
                raw = 0;
            } else {
                let keep_len = all.len().saturating_sub(drop);
                let keep = if keep_len == 0 {
                    0
                } else {
                    all[..keep_len].parse::<i128>().map_err(|_| bad())?
                };
                let first_dropped = all.as_bytes().get(keep_len).copied().unwrap_or(b'0');
                raw = keep + i128::from(first_dropped >= b'5');
            }
        }
    }
    Ok(Amount::from_raw(if neg { -raw } else { raw }))
}

/// Parses `2026-10-01T00:00:00Z` (RFC 3339), `2026-10-01T00:00:00.000Z`,
/// `2026-10-01 00:00:00` (UTC) or `2026-10-01` into Unix seconds.
pub fn parse_time(s: &str) -> Result<i64, CostError> {
    let s = s.trim();
    let bad = || CostError::invalid("time", format!("{s:?} is not a date or time"));
    if let Ok(t) = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Ok(t.unix_timestamp());
    }
    let (date, clock) = match s.split_once(['T', ' ']) {
        Some((d, c)) => (d, Some(c.trim_end_matches('Z'))),
        None => (s, None),
    };
    let day = crate::Day::parse(date).map_err(|_| bad())?;
    let mut secs = day.start_unix();
    if let Some(c) = clock {
        let c = c.split('.').next().unwrap_or(c);
        let mut it = c.split(':');
        let h: i64 = it.next().and_then(|x| x.parse().ok()).ok_or_else(bad)?;
        let m: i64 = it
            .next()
            .map_or(Some(0), |x| x.parse().ok())
            .ok_or_else(bad)?;
        let sec: i64 = it
            .next()
            .map_or(Some(0), |x| x.parse().ok())
            .ok_or_else(bad)?;
        if h > 24 || m > 59 || sec > 60 {
            return Err(bad());
        }
        secs += h * 3600 + m * 60 + sec;
    }
    Ok(secs)
}

/// Tags as a JSON object, or `k=v;k=v`. Keys with CUR's `user:` or `user_`
/// prefix are also kept without it.
pub fn parse_tags(s: &str) -> BTreeMap<String, String> {
    let s = s.trim();
    let mut out = BTreeMap::new();
    if s.is_empty() {
        return out;
    }
    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(s) {
        for (k, v) in m {
            let v = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Null => continue,
                other => other.to_string(),
            };
            out.insert(k, v);
        }
    } else {
        for pair in s.split(';') {
            if let Some((k, v)) = pair.split_once('=') {
                out.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    let stripped: Vec<(String, String)> = out
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("user:")
                .or_else(|| k.strip_prefix("user_"))
                .map(|s| (s.to_string(), v.clone()))
        })
        .collect();
    for (k, v) in stripped {
        out.entry(k).or_insert(v);
    }
    out
}

/// CUR `line_item_line_item_type` → FOCUS category (verify the full list
/// against AWS's data dictionary).
pub fn cur_category(t: &str) -> ChargeCategory {
    match t {
        "Tax" => ChargeCategory::Tax,
        "Credit"
        | "Refund"
        | "EdpDiscount"
        | "BundledDiscount"
        | "PrivateRateDiscount"
        | "DistributorDiscount"
        | "SavingsPlanNegation" => ChargeCategory::Credit,
        "Fee" | "RIFee" | "SavingsPlanRecurringFee" | "SavingsPlanUpfrontFee" => {
            ChargeCategory::Purchase
        }
        _ => ChargeCategory::Usage,
    }
}

/// Reads rows of one format.
#[derive(Debug)]
pub struct RowReader {
    format: BillFormat,
    index: HashMap<String, usize>,
    /// Legacy CUR tag columns: (index, key).
    legacy_tags: Vec<(usize, String)>,
}

impl RowReader {
    fn required(&self) -> Vec<String> {
        match &self.format {
            BillFormat::Focus => [
                "ChargePeriodStart",
                "ChargePeriodEnd",
                "BilledCost",
                "BillingCurrency",
            ]
            .map(String::from)
            .to_vec(),
            BillFormat::Cur2 | BillFormat::LegacyCur => [
                "line_item_usage_start_date",
                "line_item_usage_end_date",
                "line_item_unblended_cost",
                "line_item_currency_code",
            ]
            .map(String::from)
            .to_vec(),
            BillFormat::Csv(m) => {
                let mut v = vec![m.start.clone(), m.end.clone(), m.cost.clone()];
                v.extend(m.currency.clone());
                for c in [
                    &m.resource,
                    &m.service,
                    &m.sku,
                    &m.region,
                    &m.quantity,
                    &m.unit,
                    &m.tags,
                    &m.category,
                    &m.invoice,
                ]
                .into_iter()
                .flatten()
                {
                    v.push(c.clone());
                }
                v
            }
        }
    }

    /// The column `cost` comes from.
    pub fn cost_column(&self) -> &str {
        match &self.format {
            BillFormat::Focus => {
                if self.index.contains_key("EffectiveCost") {
                    "EffectiveCost"
                } else {
                    "BilledCost"
                }
            }
            BillFormat::Cur2 | BillFormat::LegacyCur => {
                if self.index.contains_key("line_item_net_unblended_cost") {
                    "line_item_net_unblended_cost"
                } else {
                    "line_item_unblended_cost"
                }
            }
            BillFormat::Csv(m) => &m.cost,
        }
    }

    fn get<'r>(&self, record: &'r csv::StringRecord, col: &str) -> Option<&'r str> {
        self.index
            .get(col)
            .and_then(|i| record.get(*i))
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    fn opt(&self, record: &csv::StringRecord, col: Option<&str>) -> Option<String> {
        col.and_then(|c| self.get(record, c)).map(str::to_string)
    }

    /// Row `n` as a bill line.
    pub fn line(&self, n: u64, record: &csv::StringRecord) -> Result<BillLine, CostError> {
        let at = |e: CostError| CostError::invalid("bill", format!("line {n}: {e}"));
        let need = |col: &str| {
            self.get(record, col)
                .ok_or_else(|| CostError::invalid("bill", format!("line {n}: {col} is empty")))
        };
        let currency = |s: &str| {
            Currency::parse(s).map_err(|e| at(CostError::invalid("currency", e.to_string())))
        };
        let decimal = |s: Option<&str>| s.map(parse_decimal).transpose().map_err(at);
        match &self.format {
            BillFormat::Focus => {
                let billed = parse_decimal(need("BilledCost")?).map_err(at)?;
                let cost = match self.get(record, "EffectiveCost") {
                    Some(c) => parse_decimal(c).map_err(at)?,
                    None => billed,
                };
                let category = match self.get(record, "ChargeCategory") {
                    Some(c) => ChargeCategory::parse(c)
                        .ok_or_else(|| at(CostError::invalid("ChargeCategory", c.to_string())))?,
                    None => ChargeCategory::Usage,
                };
                Ok(BillLine {
                    line: n,
                    start: parse_time(need("ChargePeriodStart")?).map_err(at)?,
                    end: parse_time(need("ChargePeriodEnd")?).map_err(at)?,
                    cost,
                    billed,
                    currency: currency(need("BillingCurrency")?)?,
                    category,
                    correction: self
                        .get(record, "ChargeClass")
                        .is_some_and(|c| c.eq_ignore_ascii_case("correction")),
                    resource: self.opt(record, Some("ResourceId")),
                    service: self.opt(record, Some("ServiceName")),
                    sku: self.opt(record, Some("SkuId")),
                    region: self
                        .opt(record, Some("RegionId"))
                        .or_else(|| self.opt(record, Some("Region"))),
                    quantity: decimal(self.get(record, "ConsumedQuantity"))?,
                    unit: self.opt(record, Some("ConsumedUnit")),
                    tags: self.get(record, "Tags").map(parse_tags).unwrap_or_default(),
                    invoice: self.opt(record, Some("InvoiceId")),
                    provider: self
                        .opt(record, Some("ServiceProviderName"))
                        .or_else(|| self.opt(record, Some("ProviderName"))),
                })
            }
            BillFormat::Cur2 | BillFormat::LegacyCur => {
                let unblended = parse_decimal(need("line_item_unblended_cost")?).map_err(at)?;
                let cost = match self.get(record, "line_item_net_unblended_cost") {
                    Some(c) => parse_decimal(c).map_err(at)?,
                    None => unblended,
                };
                let mut tags = self
                    .get(record, "resource_tags")
                    .map(parse_tags)
                    .unwrap_or_default();
                for (i, key) in &self.legacy_tags {
                    if let Some(v) = record.get(*i).map(str::trim).filter(|v| !v.is_empty()) {
                        tags.insert(key.clone(), v.to_string());
                    }
                }
                Ok(BillLine {
                    line: n,
                    start: parse_time(need("line_item_usage_start_date")?).map_err(at)?,
                    end: parse_time(need("line_item_usage_end_date")?).map_err(at)?,
                    cost,
                    billed: unblended,
                    currency: currency(need("line_item_currency_code")?)?,
                    category: cur_category(
                        self.get(record, "line_item_line_item_type")
                            .unwrap_or("Usage"),
                    ),
                    correction: false,
                    resource: self.opt(record, Some("line_item_resource_id")),
                    service: self.opt(record, Some("line_item_product_code")),
                    sku: self.opt(record, Some("line_item_usage_type")),
                    region: self
                        .opt(record, Some("product_region_code"))
                        .or_else(|| self.opt(record, Some("product_region"))),
                    quantity: decimal(self.get(record, "line_item_usage_amount"))?,
                    unit: self.opt(record, Some("pricing_unit")),
                    tags,
                    invoice: self.opt(record, Some("bill_invoice_id")),
                    provider: Some("AWS".to_string()),
                })
            }
            BillFormat::Csv(m) => {
                let cost = parse_decimal(need(&m.cost)?).map_err(at)?;
                let currency = match (&m.currency_constant, &m.currency) {
                    (Some(c), _) => *c,
                    (None, Some(col)) => currency(need(col)?)?,
                    (None, None) => return Err(at(CostError::invalid("currency", "no currency"))),
                };
                let category = match m.category.as_deref().and_then(|c| self.get(record, c)) {
                    Some(c) => ChargeCategory::parse(c)
                        .ok_or_else(|| at(CostError::invalid("category", c.to_string())))?,
                    None => ChargeCategory::Usage,
                };
                Ok(BillLine {
                    line: n,
                    start: parse_time(need(&m.start)?).map_err(at)?,
                    end: parse_time(need(&m.end)?).map_err(at)?,
                    cost,
                    billed: cost,
                    currency,
                    category,
                    correction: false,
                    resource: self.opt(record, m.resource.as_deref()),
                    service: self.opt(record, m.service.as_deref()),
                    sku: self.opt(record, m.sku.as_deref()),
                    region: self.opt(record, m.region.as_deref()),
                    quantity: decimal(m.quantity.as_deref().and_then(|c| self.get(record, c)))?,
                    unit: self.opt(record, m.unit.as_deref()),
                    tags: m
                        .tags
                        .as_deref()
                        .and_then(|c| self.get(record, c))
                        .map(parse_tags)
                        .unwrap_or_default(),
                    invoice: self.opt(record, m.invoice.as_deref()),
                    provider: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_as_bills_write_them() {
        let p = |s: &str| parse_decimal(s).expect(s).to_string();
        assert_eq!(p("12"), "12");
        assert_eq!(p("-0.5"), "-0.5");
        assert_eq!(p("1.2E-5"), "0.000012");
        assert_eq!(p("1.5e2"), "150");
        assert_eq!(p("0.0000000004"), "0");
        assert_eq!(p("0.0000000005"), "0.000000001");
        assert_eq!(p("-0.00000000149"), "-0.000000001");
        assert_eq!(p(""), "0");
        assert_eq!(p(".25"), "0.25");
        assert!(parse_decimal("1,5").is_err());
        assert!(parse_decimal("abc").is_err());
    }

    #[test]
    fn times() {
        let d = crate::Day::parse("2026-10-01").expect("d").start_unix();
        assert_eq!(parse_time("2026-10-01T00:00:00Z").expect("t"), d);
        assert_eq!(parse_time("2026-10-01T01:00:00.000Z").expect("t"), d + 3600);
        assert_eq!(parse_time("2026-10-01 00:00:30").expect("t"), d + 30);
        assert_eq!(parse_time("2026-10-01").expect("t"), d);
        assert_eq!(parse_time("2026-10-01T02:00:00+02:00").expect("t"), d);
        assert!(parse_time("10/01/2026").is_err());
    }

    #[test]
    fn legacy_cur_names_normalised() {
        assert_eq!(
            legacy_cur_name("lineItem/UsageStartDate"),
            "line_item_usage_start_date"
        );
        assert_eq!(
            legacy_cur_name("lineItem/UnblendedCost"),
            "line_item_unblended_cost"
        );
        assert_eq!(legacy_cur_name("product/regionCode"), "product_region_code");
    }

    #[test]
    fn tags_json_or_pairs() {
        let t = parse_tags(r#"{"user:loams-pool":"eu/compute","env":"prod"}"#);
        assert_eq!(t.get("loams-pool").map(String::as_str), Some("eu/compute"));
        assert_eq!(t.get("env").map(String::as_str), Some("prod"));
        let t = parse_tags("loams-pool=eu/compute; loams-kind=storage");
        assert_eq!(t.get("loams-kind").map(String::as_str), Some("storage"));
    }

    #[test]
    fn column_map_spec() {
        let m = ColumnMap::parse("start=From,end=To,cost=Amount,currency=@EUR,service=Item")
            .expect("map");
        assert_eq!(m.currency_constant, Some(Currency::EUR));
        assert_eq!(m.service.as_deref(), Some("Item"));
        assert!(ColumnMap::parse("start=a,end=b").is_err());
        assert!(ColumnMap::parse("start=a,end=b,cost=c").is_err());
        assert!(ColumnMap::parse("start=a,end=b,cost=c,currency=@USD,colour=x").is_err());
    }
}
