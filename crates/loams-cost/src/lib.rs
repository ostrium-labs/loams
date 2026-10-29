//! Loams Cost (design §84): what a unit of work cost on the infrastructure
//! its owner runs, and how we know.
//!
//! The operator supplies cost inputs (amortised assets, bill imports, a
//! price table); Loams Meter supplies resource telemetry. This crate turns
//! them into allocation lines per (day, pool, kind, consumer), each with a
//! basis label and a trace. It is sans-I/O: the `meter` role and
//! `loams cost` feed it and read it.
//!
//! | Module | Does |
//! |---|---|
//! | [`model`] | Resource kinds, days, pools, consumer keys, sources, capacities, usage |
//! | [`basis`] | `measured` / `billed` / `declared` / `estimated` and line flags |
//! | [`money`] | Exact 256-bit share arithmetic and the largest-remainder split |
//! | [`allocate`] | Pool-days and proportional, capacity-aware allocation |
//! | [`trace`] | Trace ids, input digests and explanations |

pub mod allocate;
pub mod basis;
pub mod model;
pub mod money;
pub mod trace;

pub use allocate::{
    Allocation, ConsumerUsage, Line, PoolDay, PoolDayKey, SourceAmount, allocate, allocate_all,
    pool_days, split_line,
};
pub use basis::{Basis, Flag};
pub use loams_meter_core::{Amount, Currency};
pub use model::{
    Capacity, Consumer, ConsumerKey, CostSource, Day, NONE, PoolId, ResourceKind, SourceRef, Usage,
};
pub use trace::{ALLOCATION_VERSION, Explanation, digest, explain, trace_id};

/// What can go wrong in cost attribution.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CostError {
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("a quantity is negative")]
    NegativeQuantity,
    #[error("nothing to share by: every weight is zero")]
    NoWeight,
    #[error("arithmetic overflow")]
    Overflow,
}

impl CostError {
    pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> CostError {
        CostError::Invalid {
            field: field.into(),
            message: message.into(),
        }
    }

    /// The `ErrorInfo.reason` the API answers with.
    pub fn reason(&self) -> &'static str {
        match self {
            CostError::Invalid { .. } => "invalid_cost_input",
            CostError::NegativeQuantity => "negative_quantity",
            CostError::NoWeight => "no_weight",
            CostError::Overflow => "cost_overflow",
        }
    }
}
