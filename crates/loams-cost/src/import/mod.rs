//! Bill import (design §84 §6.2, D2354): FOCUS 1.0–1.4, AWS CUR 2.0 and
//! legacy CUR, and generic CSV with a column map, plain or gzip, streamed.
//! Each line becomes a [`BillLine`], is mapped to a pool and kind
//! ([`mapping`]), and is split over the UTC days its charge period covers
//! into [`CostSource`]s. Unmapped lines are reported, never dropped.

pub mod formats;
pub mod mapping;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};

use loams_meter_core::{Amount, Currency};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inputs::CostInputs;
use crate::money::split;
use crate::{Basis, CostError, CostSource, Day, PoolId, ResourceKind, SourceRef};

pub use formats::{BillFormat, ColumnMap};
pub use mapping::{Mapped, SHARED_POOL, map_line};

/// FOCUS `ChargeCategory`, with CUR's line types mapped onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChargeCategory {
    Usage,
    Purchase,
    Tax,
    Credit,
    Adjustment,
}

impl ChargeCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChargeCategory::Usage => "Usage",
            ChargeCategory::Purchase => "Purchase",
            ChargeCategory::Tax => "Tax",
            ChargeCategory::Credit => "Credit",
            ChargeCategory::Adjustment => "Adjustment",
        }
    }

    pub fn parse(s: &str) -> Option<ChargeCategory> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "usage" | "" => ChargeCategory::Usage,
            "purchase" => ChargeCategory::Purchase,
            "tax" => ChargeCategory::Tax,
            "credit" => ChargeCategory::Credit,
            "adjustment" => ChargeCategory::Adjustment,
            _ => return None,
        })
    }
}

/// One bill line, normalised. Times are Unix seconds, `[start, end)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillLine {
    /// 1-based data line number in the file (the header is line 0).
    pub line: u64,
    pub start: i64,
    pub end: i64,
    /// The amount allocation uses: FOCUS `EffectiveCost`, CUR net unblended
    /// (or unblended) cost, generic `cost`.
    pub cost: Amount,
    /// FOCUS `BilledCost` (or the same as `cost`), for invoice
    /// reconciliation.
    pub billed: Amount,
    pub currency: Currency,
    pub category: ChargeCategory,
    /// FOCUS `ChargeClass` = `Correction`.
    pub correction: bool,
    pub resource: Option<String>,
    pub service: Option<String>,
    pub sku: Option<String>,
    pub region: Option<String>,
    pub quantity: Option<Amount>,
    pub unit: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub invoice: Option<String>,
    pub provider: Option<String>,
}

/// A line the rules did not map, as the import report lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmappedLine {
    pub line: u64,
    pub service: Option<String>,
    pub resource: Option<String>,
    pub cost: Amount,
}

/// What an import found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSummary {
    /// SHA-256 of the file's bytes as given (compressed if gzip), hex: the
    /// import's idempotency key.
    pub digest: String,
    pub format: String,
    /// Which column `cost` came from.
    pub cost_column: String,
    pub lines: u64,
    pub mapped_lines: u64,
    pub shared_lines: u64,
    pub unmapped_lines: u64,
    /// Σ `cost` of every line, in the reporting currency (after FX).
    pub total: Amount,
    pub mapped: Amount,
    pub shared: Amount,
    pub unmapped: Amount,
    /// Σ unmapped cost per service (`""` for none).
    pub unmapped_by_service: BTreeMap<String, Amount>,
    /// Σ `billed` per invoice id (FOCUS `InvoiceId`), in the bill's currency.
    pub billed_by_invoice: BTreeMap<String, Amount>,
    /// Σ `billed` per currency, as in the file.
    pub billed_by_currency: BTreeMap<String, Amount>,
    pub first_day: Option<Day>,
    pub last_day: Option<Day>,
}

/// One imported line with its mapping and day sources (empty when
/// unmapped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedLine {
    pub line: BillLine,
    pub mapped: Mapped,
    pub sources: Vec<CostSource>,
}

/// The day shares of `[start, end)`, in proportion to the seconds in each
/// UTC day, exactly. An instant (`start == end`) is all on its day.
pub fn day_shares(start: i64, end: i64, amount: Amount) -> Result<Vec<(Day, Amount)>, CostError> {
    if end < start {
        return Err(CostError::invalid("charge period", "ends before it starts"));
    }
    let first = Day::of_unix(start)?;
    if end == start {
        return Ok(vec![(first, amount)]);
    }
    let mut days = Vec::new();
    let mut weights = Vec::new();
    let mut day = first;
    loop {
        let ds = day.start_unix();
        let de = ds + 86_400;
        let lo = start.max(ds);
        let hi = end.min(de);
        if hi > lo {
            days.push(day);
            weights.push(Amount::from_units(hi - lo));
        }
        if de >= end {
            break;
        }
        day = day.next().ok_or(CostError::Overflow)?;
        if days.len() > 4_000 {
            return Err(CostError::invalid(
                "charge period",
                "longer than 4,000 days",
            ));
        }
    }
    let parts = split(amount, &weights)?;
    Ok(days.into_iter().zip(parts).collect())
}

/// The sources of one mapped line, converted into the reporting currency
/// with a declared FX rate when the bill's currency differs (D2357).
pub fn line_sources(
    inputs: &CostInputs,
    import: &str,
    line: &BillLine,
    pool: &PoolId,
    kind: ResourceKind,
) -> Result<Vec<CostSource>, CostError> {
    let mut out = Vec::new();
    for (day, amount) in day_shares(line.start, line.end, line.cost)? {
        let (amount, basis) = if line.currency == inputs.currency {
            (amount, Basis::Billed)
        } else {
            let rate = inputs.fx_rate(line.currency, day).ok_or_else(|| {
                CostError::invalid(
                    "currency",
                    format!(
                        "currency_mismatch: line {} is in {} and no fx rate to {} is declared for {day}",
                        line.line,
                        line.currency.as_str(),
                        inputs.currency.as_str()
                    ),
                )
            })?;
            (
                crate::prices::price_of_signed(rate.rate, amount)?,
                Basis::Declared,
            )
        };
        out.push(CostSource {
            day,
            pool: pool.clone(),
            kind,
            amount,
            basis,
            source: SourceRef::BillLine {
                import: import.to_string(),
                line: line.line,
            },
            flags: BTreeSet::new(),
        });
    }
    Ok(out)
}

/// A reader that hashes what it reads.
struct Hashing<R> {
    inner: R,
    hash: Sha256,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
}

impl<R> std::fmt::Debug for Hashing<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hashing")
    }
}

/// Imports a bill: detects the format (unless `format` names it), parses
/// every line, maps it with `inputs.mappings`, and hands each line to
/// `each`. `import` names the import in the sources' trace. Streams: memory
/// does not grow with the file.
pub fn import_bill<R: Read>(
    inputs: &CostInputs,
    import: &str,
    reader: R,
    format: Option<BillFormat>,
    mut each: impl FnMut(ImportedLine) -> Result<(), CostError>,
) -> Result<ImportSummary, CostError> {
    let io = |e: std::io::Error| CostError::invalid("bill", e.to_string());
    let mut hashing = BufReader::new(Hashing {
        inner: reader,
        hash: Sha256::new(),
    });
    // gzip by its magic number.
    let gz = hashing.fill_buf().map_err(io)?.starts_with(&[0x1f, 0x8b]);
    let body: Box<dyn Read + '_> = if gz {
        Box::new(flate2::read::MultiGzDecoder::new(&mut hashing))
    } else {
        Box::new(&mut hashing)
    };
    let mut csv = ::csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(body);
    let headers: Vec<String> = csv
        .headers()
        .map_err(|e| CostError::invalid("bill", e.to_string()))?
        .iter()
        .map(|h| h.trim_start_matches('\u{feff}').to_string())
        .collect();
    let format = match format {
        Some(f) => f,
        None => BillFormat::detect(&headers).ok_or_else(|| {
            CostError::invalid(
                "format",
                "not a FOCUS or CUR file; give the format and a column map for generic CSV",
            )
        })?,
    };
    let reader = format.reader(&headers)?;
    let mut summary = ImportSummary {
        format: format.name().to_string(),
        cost_column: reader.cost_column().to_string(),
        ..Default::default()
    };
    let mut record = ::csv::StringRecord::new();
    let mut n: u64 = 0;
    while csv
        .read_record(&mut record)
        .map_err(|e| CostError::invalid("bill", format!("line {}: {e}", n + 1)))?
    {
        n += 1;
        let line = reader.line(n, &record)?;
        let mapped = map_line(&inputs.mappings, &line)?;
        let sources = match &mapped {
            Mapped::Pool { pool, kind, .. } => line_sources(inputs, import, &line, pool, *kind)?,
            Mapped::Shared => {
                let pool = PoolId::new(SHARED_POOL)?;
                line_sources(inputs, import, &line, &pool, ResourceKind::Fixed)?
            }
            Mapped::Unmapped => {
                // Converted the same way, so the totals are in one currency.
                let pool = PoolId::new(SHARED_POOL)?;
                let s = line_sources(inputs, import, &line, &pool, ResourceKind::Fixed)?;
                let cost = s.iter().map(|s| s.amount).sum::<Amount>();
                summary.unmapped_lines += 1;
                summary.unmapped = summary
                    .unmapped
                    .checked_add(cost)
                    .ok_or(CostError::Overflow)?;
                let slot = summary
                    .unmapped_by_service
                    .entry(line.service.clone().unwrap_or_default())
                    .or_insert(Amount::ZERO);
                *slot = slot.checked_add(cost).ok_or(CostError::Overflow)?;
                summary.total = summary.total.checked_add(cost).ok_or(CostError::Overflow)?;
                Vec::new()
            }
        };
        let cost: Amount = sources.iter().map(|s| s.amount).sum();
        match &mapped {
            Mapped::Pool { .. } => {
                summary.mapped_lines += 1;
                summary.mapped = summary
                    .mapped
                    .checked_add(cost)
                    .ok_or(CostError::Overflow)?;
                summary.total = summary.total.checked_add(cost).ok_or(CostError::Overflow)?;
            }
            Mapped::Shared => {
                summary.shared_lines += 1;
                summary.shared = summary
                    .shared
                    .checked_add(cost)
                    .ok_or(CostError::Overflow)?;
                summary.total = summary.total.checked_add(cost).ok_or(CostError::Overflow)?;
            }
            Mapped::Unmapped => {}
        }
        for s in &sources {
            summary.first_day = Some(summary.first_day.map_or(s.day, |d| d.min(s.day)));
            summary.last_day = Some(summary.last_day.map_or(s.day, |d| d.max(s.day)));
        }
        if let Some(inv) = &line.invoice {
            let slot = summary
                .billed_by_invoice
                .entry(inv.clone())
                .or_insert(Amount::ZERO);
            *slot = slot.checked_add(line.billed).ok_or(CostError::Overflow)?;
        }
        let slot = summary
            .billed_by_currency
            .entry(line.currency.as_str().to_string())
            .or_insert(Amount::ZERO);
        *slot = slot.checked_add(line.billed).ok_or(CostError::Overflow)?;
        each(ImportedLine {
            line,
            mapped,
            sources,
        })?;
    }
    summary.lines = n;
    drop(csv);
    // Read to the end so the digest covers trailing bytes too.
    std::io::copy(&mut hashing, &mut std::io::sink()).map_err(io)?;
    summary.digest = hex::encode(hashing.into_inner().hash.finalize());
    Ok(summary)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn line() -> BillLine {
        BillLine {
            line: 1,
            start: 0,
            end: 86_400,
            cost: Amount::from_units(1),
            billed: Amount::from_units(1),
            currency: Currency::USD,
            category: ChargeCategory::Usage,
            correction: false,
            resource: Some("i-1".into()),
            service: None,
            sku: None,
            region: None,
            quantity: None,
            unit: None,
            tags: BTreeMap::new(),
            invoice: None,
            provider: None,
        }
    }

    #[test]
    fn line_split_across_days() {
        // 2026-10-28T18:00Z to 2026-10-29T06:00Z: half and half.
        let start = Day::parse("2026-10-28").expect("d").start_unix() + 18 * 3600;
        let shares = day_shares(
            start,
            start + 12 * 3600,
            Amount::parse("1.000000001").expect("a"),
        )
        .expect("shares");
        assert_eq!(shares.len(), 2);
        assert_eq!(shares[0].0.to_string(), "2026-10-28");
        assert_eq!(
            shares[0].1 + shares[1].1,
            Amount::parse("1.000000001").expect("a")
        );
        assert_eq!(shares[0].1, Amount::parse("0.500000001").expect("a"));
        // A month spreads evenly over its days.
        let s = Day::parse("2026-10-01").expect("d").start_unix();
        let e = Day::parse("2026-11-01").expect("d").start_unix();
        let shares = day_shares(s, e, Amount::from_units(31)).expect("shares");
        assert_eq!(shares.len(), 31);
        assert!(shares.iter().all(|(_, a)| *a == Amount::from_units(1)));
        assert!(day_shares(e, s, Amount::from_units(1)).is_err());
    }
}
