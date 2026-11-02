//! The monthly cost estimate shown before every apply (design 86 D2615).
//!
//! It prices the `cost_items` a module plan reports, and a listing's cost
//! hint, against the dated [`PriceTable`]. Everything is exact [`Amount`]
//! arithmetic and every result is labelled [`Basis::Estimated`] with the
//! table's read date (D2370). Nothing here is ever a silent zero:
//!
//! * a SKU the table does not know, or whose unit differs from the item's,
//!   is an [`CostFlag::Unpriced`] line and makes the estimate incomplete
//!   (units are never converted);
//! * a region outside the price's `regions` still uses that price, flagged
//!   [`CostFlag::RegionNotInTable`] with the region it is priced for;
//! * a free allowance is flagged [`CostFlag::FreeTier`].

use std::collections::BTreeMap;
use std::fmt;

use loams_cost::Basis;
use loams_meter_core::Amount;
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use time::Date;

use crate::prices::{PriceEntry, PriceTable, PriceUnit};
use crate::targets::DeployTarget;

/// The kind of managed storage a listing asks for.
///
/// Mirrors `loams_agentd_marketplace::StorageKind` (OP2-B); replaced by a
/// re-export in Task 15.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageKind {
    Object,
    SqlMysql,
    SqlPostgres,
    Kafka,
    Kv,
}

impl StorageKind {
    fn as_str(self) -> &'static str {
        match self {
            StorageKind::Object => "object",
            StorageKind::SqlMysql => "sql-mysql",
            StorageKind::SqlPostgres => "sql-postgres",
            StorageKind::Kafka => "kafka",
            StorageKind::Kv => "kv",
        }
    }
}

/// What a listing says it costs to run.
///
/// Mirrors `loams_agentd_marketplace::CostHint` (OP2-B); replaced by a
/// re-export in Task 15. Only `storage_gib`, `egress_gib_per_month` and
/// `managed` are priced here; Task 15 checks `cpu_millis` and `memory_mib`
/// against the target's capacity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostHint {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub storage_gib: u32,
    pub egress_gib_per_month: u32,
    pub managed: Vec<StorageKind>,
}

/// One billable thing a module plan reports (its `cost_items` output).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CostItem {
    /// Price table SKU (`<cloud>/<service>/<name>`).
    pub sku: String,
    /// In `unit`, per month. Tofu emits JSON numbers (`730` or `730.0`).
    #[serde(deserialize_with = "whole_quantity")]
    pub quantity: u64,
    pub unit: PriceUnit,
    /// `""` and `null` are no note.
    #[serde(default, deserialize_with = "optional_note")]
    pub note: Option<String>,
}

fn optional_note<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.filter(|note| !note.is_empty()))
}

/// A non-negative whole number, written as an integer or as `730.0`.
fn whole_quantity<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    struct Quantity;

    impl Visitor<'_> for Quantity {
        type Value = u64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a non-negative whole number")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
            u64::try_from(v).map_err(|_| E::custom(format!("negative quantity {v}")))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<u64, E> {
            // 2^64 is exactly representable; anything below it that is whole fits.
            if v.is_finite() && v >= 0.0 && v.fract() == 0.0 && v < 18_446_744_073_709_551_616.0 {
                Ok(v as u64)
            } else {
                Err(E::custom(format!(
                    "quantity {v} is not a non-negative whole number"
                )))
            }
        }
    }

    deserializer.deserialize_any(Quantity)
}

/// Why a line needs a second look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CostFlag {
    /// Some or all of the quantity is inside a free allowance.
    FreeTier { note: String },
    /// The price is for another region; the table has no price for the
    /// deploy region.
    RegionNotInTable { priced_region: String },
    /// No price applies. The line is $0 in the total, and the estimate is
    /// incomplete.
    Unpriced { reason: String },
    /// The line comes from the listing's cost hint, not from the plan.
    FromListingHint,
}

/// One priced line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostLine {
    pub sku: String,
    pub quantity: u64,
    pub unit: PriceUnit,
    pub usd_month: Amount,
    pub flags: Vec<CostFlag>,
}

/// The estimate shown before an apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostEstimate {
    pub lines: Vec<CostLine>,
    pub total_usd_month: Amount,
    /// No line is [`CostFlag::Unpriced`].
    pub complete: bool,
    /// Always [`Basis::Estimated`]: list prices, never a bill.
    pub basis: Basis,
    /// The price table's read date.
    pub prices_read: Date,
    /// The target runs on a free tier.
    pub free_tier_target: bool,
    /// At least one line is priced, and every priced line's entry is
    /// `verified` against a billed amount.
    pub all_verified: bool,
}

/// Remaining free allowance per SKU, shared by every line of that SKU.
struct Pricer<'a> {
    table: &'a PriceTable,
    region: &'a str,
    free_left: BTreeMap<String, u64>,
    lines: Vec<CostLine>,
    priced: u32,
    unverified: u32,
}

impl<'a> Pricer<'a> {
    fn add(&mut self, sku: &str, quantity: u64, unit: PriceUnit, mut flags: Vec<CostFlag>) {
        let usd_month = self.price(sku, quantity, unit, &mut flags);
        self.lines.push(CostLine {
            sku: sku.to_string(),
            quantity,
            unit,
            usd_month,
            flags,
        });
    }

    fn unpriced(flags: &mut Vec<CostFlag>, reason: String) -> Amount {
        flags.push(CostFlag::Unpriced { reason });
        Amount::ZERO
    }

    fn price(
        &mut self,
        sku: &str,
        quantity: u64,
        unit: PriceUnit,
        flags: &mut Vec<CostFlag>,
    ) -> Amount {
        let Some(entry) = self.table.get(sku) else {
            return Self::unpriced(flags, format!("{sku} is not in the price table"));
        };
        if entry.unit != unit {
            return Self::unpriced(
                flags,
                format!(
                    "{sku} is priced per {}, the plan counts {}",
                    entry.unit.as_str(),
                    unit.as_str()
                ),
            );
        }
        self.priced += 1;
        if !entry.verified {
            self.unverified += 1;
        }
        if !entry.regions.iter().any(|region| region == self.region) {
            flags.push(CostFlag::RegionNotInTable {
                priced_region: entry.regions.first().cloned().unwrap_or_default(),
            });
        }
        let billable = self.consume_free(sku, entry, quantity, flags);
        entry.usd.mul_int(i128::from(billable))
    }

    /// Subtracts up to the SKU's remaining free allowance; returns what is left to bill.
    fn consume_free(
        &mut self,
        sku: &str,
        entry: &PriceEntry,
        quantity: u64,
        flags: &mut Vec<CostFlag>,
    ) -> u64 {
        let Some(free) = &entry.free else {
            return quantity;
        };
        let left = self
            .free_left
            .entry(sku.to_string())
            .or_insert(free.quantity);
        let covered = quantity.min(*left);
        *left -= covered;
        if covered > 0 {
            flags.push(CostFlag::FreeTier {
                note: free.note.clone(),
            });
        }
        quantity - covered
    }
}

/// The estimated monthly cost of `items` (and of `hint`, when the listing
/// gives one) on `target` in `region`.
pub fn estimate_monthly_cost(
    target: &DeployTarget,
    region: &str,
    items: &[CostItem],
    hint: Option<&CostHint>,
    prices: &PriceTable,
) -> CostEstimate {
    let mut pricer = Pricer {
        table: prices,
        region,
        free_left: BTreeMap::new(),
        lines: Vec::new(),
        priced: 0,
        unverified: 0,
    };
    for item in items {
        pricer.add(&item.sku, item.quantity, item.unit, Vec::new());
    }
    if let Some(hint) = hint {
        add_hint(&mut pricer, target, hint);
    }

    let mut total = Amount::ZERO;
    let mut complete = true;
    for line in &pricer.lines {
        total = total.saturating_add(line.usd_month);
        complete &= !line
            .flags
            .iter()
            .any(|flag| matches!(flag, CostFlag::Unpriced { .. }));
    }
    CostEstimate {
        total_usd_month: total,
        complete,
        basis: Basis::Estimated,
        prices_read: prices.read,
        free_tier_target: target.free_tier,
        all_verified: pricer.priced > 0 && pricer.unverified == 0,
        lines: pricer.lines,
    }
}

fn add_hint(pricer: &mut Pricer<'_>, target: &DeployTarget, hint: &CostHint) {
    let from_hint = || vec![CostFlag::FromListingHint];
    let wanted = [
        (
            "disk",
            &target.disk_sku,
            hint.storage_gib,
            PriceUnit::GibMonth,
        ),
        (
            "egress",
            &target.egress_sku,
            hint.egress_gib_per_month,
            PriceUnit::Gib,
        ),
    ];
    for (what, sku, quantity, unit) in wanted {
        if quantity == 0 {
            continue;
        }
        if sku.is_empty() {
            pricer.lines.push(CostLine {
                sku: format!("hint/{what}"),
                quantity: u64::from(quantity),
                unit,
                usd_month: Amount::ZERO,
                flags: vec![
                    CostFlag::FromListingHint,
                    CostFlag::Unpriced {
                        reason: format!("{} has no {what} price in the table", target.kind),
                    },
                ],
            });
        } else {
            pricer.add(sku, u64::from(quantity), unit, from_hint());
        }
    }
    for kind in &hint.managed {
        pricer.lines.push(CostLine {
            sku: format!("hint/managed/{}", kind.as_str()),
            quantity: 1,
            unit: PriceUnit::Month,
            usd_month: Amount::ZERO,
            flags: vec![
                CostFlag::FromListingHint,
                CostFlag::Unpriced {
                    reason: "billed by the managed service (DEP2)".to_string(),
                },
            ],
        });
    }
}

/// `a` in dollars with cents, rounded half up (away from zero for a
/// negative), for example `1.24`.
pub fn format_usd(a: Amount) -> String {
    const NANO_PER_CENT: u128 = 10_000_000;
    let abs = a.raw().unsigned_abs();
    let cents = (abs + NANO_PER_CENT / 2) / NANO_PER_CENT;
    let sign = if a.is_negative() && cents > 0 {
        "-"
    } else {
        ""
    };
    format!("{sign}{}.{:02}", cents / 100, cents % 100)
}
