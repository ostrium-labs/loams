//! FOCUS 1.3 export (design §84 §13.2, D2375, Q1495).
//!
//! Loams writes allocated charges as a FOCUS data generator: one row per
//! allocation line and day, with the split cost allocation columns FOCUS
//! 1.3 added (`AllocatedMethodId`, `AllocatedMethodDetails`,
//! `AllocatedResourceId`, `AllocatedResourceName`, `AllocatedTags`). The
//! `AllocatedRatio`s of one origin charge (a pool-day) sum to exactly 1, as
//! the spec requires. Columns Loams cannot know are null (empty).
//!
//! The column set and each column's feature level follow the v1.3 column
//! files (read 2026-10-29); the deprecated but still mandatory
//! `ProviderName` and `PublisherName` are written beside
//! `ServiceProviderName` and `HostProviderName`. Conformance with the FinOps
//! Foundation's validator is (verify), CA1 Task 7.

use std::io::Write;

use loams_meter_core::Amount;
use serde_json::json;

use super::{by_pool_day, split_by_amount};
use crate::{Consumer, CostError, Day, Line, ResourceKind, trace::ALLOCATION_VERSION, trace_id};

/// The columns, in the order written.
pub const COLUMNS: &[&str] = &[
    "AllocatedMethodDetails",
    "AllocatedMethodId",
    "AllocatedResourceId",
    "AllocatedResourceName",
    "AllocatedTags",
    "BilledCost",
    "BillingAccountId",
    "BillingAccountName",
    "BillingCurrency",
    "BillingPeriodEnd",
    "BillingPeriodStart",
    "ChargeCategory",
    "ChargeClass",
    "ChargeDescription",
    "ChargeFrequency",
    "ChargePeriodEnd",
    "ChargePeriodStart",
    "ConsumedQuantity",
    "ConsumedUnit",
    "ContractedCost",
    "EffectiveCost",
    "HostProviderName",
    "InvoiceIssuerName",
    "ListCost",
    "PricingQuantity",
    "PricingUnit",
    "ProviderName",
    "PublisherName",
    "ResourceId",
    "ResourceName",
    "ResourceType",
    "ServiceCategory",
    "ServiceName",
    "ServiceProviderName",
    "Tags",
];

/// Who the dataset says issued it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusOptions {
    pub billing_account_id: String,
    pub billing_account_name: String,
    /// Provider and issuer names; `Loams (allocated)` by default.
    pub provider: String,
    pub currency: String,
}

impl FocusOptions {
    pub fn new(currency: impl Into<String>) -> FocusOptions {
        FocusOptions {
            billing_account_id: "loams".into(),
            billing_account_name: "Loams cost attribution".into(),
            provider: "Loams (allocated)".into(),
            currency: currency.into(),
        }
    }
}

/// FOCUS `ServiceCategory` for a kind.
pub fn service_category(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Cpu | ResourceKind::Memory | ResourceKind::Gpu => "Compute",
        ResourceKind::Storage | ResourceKind::Io => "Storage",
        ResourceKind::NetworkEgress
        | ResourceKind::NetworkIngress
        | ResourceKind::NetworkInternal => "Networking",
        ResourceKind::Fixed => "Other",
    }
}

fn instant(day: Day) -> String {
    format!("{day}T00:00:00Z")
}

fn month_bounds(day: Day) -> Result<(Day, Day), CostError> {
    let d = day.date();
    let start = Day::new(d.year(), u8::from(d.month()), 1)?;
    let end = crate::amortise::add_months(start, 1)?;
    Ok((start, end))
}

/// Writes the dataset as CSV. Returns the number of rows. Lines with a
/// zero amount are left out (FOCUS rows are charges).
pub fn write_focus<W: Write>(
    out: W,
    lines: &[Line],
    opts: &FocusOptions,
) -> Result<u64, CostError> {
    let io = |e: csv::Error| CostError::invalid("export", e.to_string());
    let mut w = csv::Writer::from_writer(out);
    w.write_record(COLUMNS).map_err(io)?;
    let mut rows = 0u64;
    for (key, idx) in by_pool_day(lines) {
        let Some(ratios) = split_by_amount(Amount::from_units(1), lines, &idx)? else {
            continue;
        };
        let (bp_start, bp_end) = month_bounds(key.day)?;
        let next = key.day.next().ok_or(CostError::Overflow)?;
        for (i, ratio) in idx.iter().zip(ratios) {
            let l = &lines[*i];
            if l.amount.is_zero() {
                continue;
            }
            let (engine, tenant, project, class) = match &l.consumer {
                Consumer::Work(k) => (
                    k.engine.as_str(),
                    k.tenant.as_str(),
                    k.project.as_str(),
                    k.query_class.as_str(),
                ),
                other => (other.engine(), "_", "_", "_"),
            };
            let allocated_id = format!("{engine}/{tenant}/{project}/{class}");
            let unit = key.kind.driver_unit();
            let details = json!({
                "Elements": [{
                    "AllocatedRatio": ratio.to_string().parse::<serde_json::Number>().map_err(|e| CostError::invalid("export", e.to_string()))?,
                    "UsageUnit": unit,
                    "UsageQuantity": l.quantity.to_string().parse::<serde_json::Number>().map_err(|e| CostError::invalid("export", e.to_string()))?,
                    "x_LoamsBasis": l.basis.as_str(),
                    "x_LoamsAmountBasis": l.amount_basis.as_str(),
                    "x_LoamsShareBasis": l.share_basis.as_str(),
                    "x_LoamsFlags": l.flags.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
                    "x_LoamsTrace": trace_id(l),
                }],
                "x_LoamsAllocationVersion": ALLOCATION_VERSION,
            });
            let tags = json!({
                "loams:engine": engine,
                "loams:tenant": tenant,
                "loams:project": project,
                "loams:query_class": class,
            });
            let amount = l.amount.to_string();
            let quantity = if key.kind == ResourceKind::Fixed {
                String::new()
            } else {
                l.quantity.to_string()
            };
            let qunit = if key.kind == ResourceKind::Fixed {
                String::new()
            } else {
                unit.to_string()
            };
            let category = if l.amount.is_negative() {
                "Credit"
            } else {
                "Usage"
            };
            let record: Vec<String> = vec![
                details.to_string(),
                ALLOCATION_VERSION.to_string(),
                allocated_id.clone(),
                allocated_id,
                tags.to_string(),
                amount.clone(),
                opts.billing_account_id.clone(),
                opts.billing_account_name.clone(),
                opts.currency.clone(),
                instant(bp_end),
                instant(bp_start),
                category.to_string(),
                String::new(),
                format!("Loams allocation of {} {}", key.pool, key.kind),
                "Usage-Based".to_string(),
                instant(next),
                instant(key.day),
                quantity.clone(),
                qunit.clone(),
                amount.clone(),
                amount.clone(),
                opts.provider.clone(),
                opts.provider.clone(),
                amount,
                quantity,
                qunit,
                opts.provider.clone(),
                opts.provider.clone(),
                key.pool.to_string(),
                key.pool.to_string(),
                key.kind.to_string(),
                service_category(key.kind).to_string(),
                format!("Loams {engine}"),
                opts.provider.clone(),
                "{}".to_string(),
            ];
            debug_assert_eq!(record.len(), COLUMNS.len());
            w.write_record(&record).map_err(io)?;
            rows += 1;
        }
    }
    w.flush()
        .map_err(|e| CostError::invalid("export", e.to_string()))?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Basis, Capacity, ConsumerKey, CostSource, PoolId, SourceRef, Usage, allocate_all};
    use std::collections::BTreeMap;

    fn lines() -> Vec<Line> {
        let day = Day::parse("2026-10-28").expect("d");
        let pool = PoolId::new("eu/compute").expect("p");
        let src = CostSource {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            amount: Amount::from_units(10),
            basis: Basis::Billed,
            source: SourceRef::Other { note: "x".into() },
            flags: Default::default(),
        };
        let cap = Capacity {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            quantity: Amount::from_units(3),
            basis: Basis::Declared,
        };
        let u = |e: &str| Usage {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            consumer: ConsumerKey::new(e, "org_1", "7", "_").expect("k"),
            quantity: Amount::from_units(1),
            basis: Basis::Measured,
        };
        allocate_all(&[src], &[cap], &[u("house"), u("pg")])
            .expect("alloc")
            .lines
    }

    fn parse(text: &str) -> Vec<BTreeMap<String, String>> {
        let mut r = csv::Reader::from_reader(text.as_bytes());
        let headers: Vec<String> = r.headers().expect("h").iter().map(String::from).collect();
        r.records()
            .map(|rec| {
                headers
                    .iter()
                    .cloned()
                    .zip(rec.expect("rec").iter().map(String::from))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn focus_columns_match_v1_3() {
        let mut out = Vec::new();
        let n = write_focus(&mut out, &lines(), &FocusOptions::new("USD")).expect("write");
        assert_eq!(n, 3);
        let text = String::from_utf8(out).expect("utf8");
        let header = text.lines().next().expect("header");
        assert_eq!(header.split(',').count(), COLUMNS.len());
        for mandatory in [
            "BilledCost",
            "EffectiveCost",
            "ChargePeriodStart",
            "BillingCurrency",
            "AllocatedMethodId",
            "ServiceProviderName",
            "ProviderName",
        ] {
            assert!(COLUMNS.contains(&mandatory), "{mandatory}");
        }
        let rows = parse(&text);
        assert_eq!(rows[0]["ChargePeriodStart"], "2026-10-28T00:00:00Z");
        assert_eq!(rows[0]["ChargePeriodEnd"], "2026-10-29T00:00:00Z");
        assert_eq!(rows[0]["BillingPeriodStart"], "2026-10-01T00:00:00Z");
        assert_eq!(rows[0]["BillingPeriodEnd"], "2026-11-01T00:00:00Z");
        assert_eq!(rows[0]["ServiceCategory"], "Compute");
        assert_eq!(rows[2]["ServiceName"], "Loams idle");
        let tags: serde_json::Value =
            serde_json::from_str(&rows[0]["AllocatedTags"]).expect("json");
        assert_eq!(tags["loams:engine"], "house");
    }

    #[test]
    fn allocated_ratios_sum_to_one() {
        let mut out = Vec::new();
        write_focus(&mut out, &lines(), &FocusOptions::new("USD")).expect("write");
        let rows = parse(&String::from_utf8(out).expect("utf8"));
        let mut sum = Amount::ZERO;
        let mut billed = Amount::ZERO;
        for r in &rows {
            let d: serde_json::Value =
                serde_json::from_str(&r["AllocatedMethodDetails"]).expect("json");
            let ratio =
                Amount::parse(&d["Elements"][0]["AllocatedRatio"].to_string()).expect("ratio");
            sum += ratio;
            billed += Amount::parse(&r["BilledCost"]).expect("cost");
            assert!(d["Elements"][0]["x_LoamsTrace"].is_string());
        }
        assert_eq!(sum, Amount::from_units(1));
        assert_eq!(billed, Amount::from_units(10));
    }
}
