//! Loams one-click deploy (design 86, plan OP2-C).
//!
//! This crate grows into the `loams deploy` CLI. It starts with two pieces
//! of embedded data that every later step reads:
//!
//! | Module | Does |
//! |---|---|
//! | [`targets`] | The deploy target registry (`deploy/tofu/targets.toml`): kinds, modules, sizes, SKUs and what each needs |
//! | [`estimate`] | The monthly cost estimate shown before every apply (D2615), from plan `cost_items` and a listing's `cost_hint` |
//! | [`prices`] | The dated cloud price table (`deploy/tofu/prices.toml`): list prices by SKU, each with its source and read date |

pub mod estimate;
pub mod prices;
pub mod targets;

pub use prices::{FreeAllowance, PriceEntry, PriceSource, PriceTable, PriceUnit};
pub use targets::{Capacity, DeployTarget, DeployTargets, Size, TargetClass};
