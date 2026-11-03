//! The dated cloud price table, embedded from `deploy/tofu/prices.toml`.
//!
//! Every entry is a public list price with the pricing page it was read
//! from. Nothing here is a bill: the estimator labels what it computes
//! `estimated` and shows [`PriceTable::read`] (D2370).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use loams_meter_core::Amount;
use serde::{Deserialize, Deserializer, Serialize};
use time::Date;

const EMBEDDED: &str = include_str!("../../../deploy/tofu/prices.toml");

/// What `usd` is per.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PriceUnit {
    /// One hour of one resource.
    Hour,
    /// One GiB held for one month.
    GibMonth,
    /// One GiB transferred.
    Gib,
    /// One resource for one month.
    Month,
}

impl PriceUnit {
    /// The unit's name as `prices.toml` and the module outputs spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            PriceUnit::Hour => "hour",
            PriceUnit::GibMonth => "gib-month",
            PriceUnit::Gib => "gib",
            PriceUnit::Month => "month",
        }
    }
}

/// Where a price came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PriceSource {
    /// A per-unit price on the provider's pricing page.
    List,
    /// Derived or approximated; the page gives no direct per-unit price.
    Estimate,
}

/// An allowance the provider does not charge for.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FreeAllowance {
    /// In the entry's unit, per month.
    pub quantity: u64,
    pub note: String,
}

/// The price of one SKU.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PriceEntry {
    pub unit: PriceUnit,
    pub usd: Amount,
    /// The regions the price is for (`us-east-1`, `us-central1`, `eastus`).
    pub regions: Vec<String>,
    pub source: PriceSource,
    /// The pricing page.
    pub url: String,
    /// Confirmed against a billed amount.
    pub verified: bool,
    #[serde(default)]
    pub free: Option<FreeAllowance>,
}

fn toml_date<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
    let datetime = toml::value::Datetime::deserialize(deserializer)?;
    let text = datetime.to_string();
    let format = time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(&text, &format).map_err(serde::de::Error::custom)
}

/// The table of list prices, by SKU (`<cloud>/<service>/<name>`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PriceTable {
    /// The day the prices were read from the pricing pages.
    #[serde(deserialize_with = "toml_date")]
    pub read: Date,
    prices: BTreeMap<String, PriceEntry>,
}

impl PriceTable {
    /// The table compiled into the binary.
    pub fn embedded() -> &'static PriceTable {
        static CELL: OnceLock<PriceTable> = OnceLock::new();
        CELL.get_or_init(|| PriceTable::parse(EMBEDDED).expect("deploy/tofu/prices.toml is valid"))
    }

    pub fn parse(text: &str) -> Result<PriceTable, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn get(&self, sku: &str) -> Option<&PriceEntry> {
        self.prices.get(sku)
    }

    /// Every entry with its SKU, ordered by SKU.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &PriceEntry)> {
        self.prices.iter().map(|(sku, entry)| (sku.as_str(), entry))
    }

    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }
}
