//! Per-thread usage (design §53 §6.5 G20; plan SD1 Task 14).
//!
//! Each turn's tokens and duration, as the harness reported them, for the
//! thread header. This is informational: it has no billing meaning
//! (billing-grade metering is a separate design, §63). The daemon keeps no price table, so
//! `costUsd` is present only when the harness itself reported a cost.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The daemon feature this method adds to `Hello` and `GetHost`.
pub const FEATURE: &str = "usage";

/// `GetThreadUsage`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GetThreadUsageParams {
    pub chat_id: String,
}

/// One turn's usage. A count the harness did not report is absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    /// The id of the user message that started the turn (as checkpoints
    /// name a turn).
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Tokens read from or written to the provider's prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_tokens: Option<u64>,
    /// Only when the harness reported a cost; never computed by the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// From the turn's start to its end.
    pub duration_ms: i64,
}

/// The sums over a thread's turns. A sum is absent when no turn reported
/// that count.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub duration_ms: i64,
}

/// `GetThreadUsage`'s result, oldest turn first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUsage {
    pub turns: Vec<TurnUsage>,
    pub totals: UsageTotals,
}

impl UsageTotals {
    /// The sums of `turns`.
    pub fn of(turns: &[TurnUsage]) -> Self {
        fn count(sum: Option<u64>, value: Option<u64>) -> Option<u64> {
            match (sum, value) {
                (Some(a), Some(b)) => Some(a.saturating_add(b)),
                (a, b) => a.or(b),
            }
        }
        fn cost(sum: Option<f64>, value: Option<f64>) -> Option<f64> {
            match (sum, value) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            }
        }
        turns.iter().fold(Self::default(), |sum, turn| Self {
            input_tokens: count(sum.input_tokens, turn.input_tokens),
            output_tokens: count(sum.output_tokens, turn.output_tokens),
            cache_tokens: count(sum.cache_tokens, turn.cache_tokens),
            cost_usd: cost(sum.cost_usd, turn.cost_usd),
            duration_ms: sum.duration_ms.saturating_add(turn.duration_ms),
        })
    }
}

impl ThreadUsage {
    /// The result for `turns`, with their totals.
    pub fn new(turns: Vec<TurnUsage>) -> Self {
        let totals = UsageTotals::of(&turns);
        Self { turns, totals }
    }
}
