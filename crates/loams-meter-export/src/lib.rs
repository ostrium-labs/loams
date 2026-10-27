//! Loams Meter's exporters (design §63 §16, D1130; plan ME1 Tasks 24–26).
//!
//! Every exporter is optional, off by default and one-way: it reads what the
//! meter closed (`_meter.windows`, `_meter.period_totals`, raw events) and
//! writes it to a provider. Loams never reads balances back from a provider;
//! the Stripe exporter reads only payment state. Each provider is a feature:
//!
//! | Feature | Module | Sends |
//! |---|---|---|
//! | `stripe` | `stripe` | Each closed hour's overage as a Stripe meter event; payment state from webhooks and a daily poll |
//! | `openmeter` | `openmeter` | Raw usage events, unchanged but for `loamslease`, to OpenMeter's `POST /api/v1/events` |
//! | `lago` | `lago` | Closed windows as Lago events over HTTP; a re-closed window sends the difference |
//! | `webhook` | `webhook` | Closed windows as signed CloudEvents to an in-house billing system |
//!
//! **Where this lives (D2210).** §63 §5.2 puts the exporters under
//! `crates/loams-meter/src/export/`. They are a crate of their own so the
//! provider clients (HTTP, HMAC) stay out of the `meter` role's graph unless a
//! feature asks for them: the role turns on `export-stripe` and so on, which
//! enable this crate's features, and calls the exporters from its close-job
//! scheduler. The inputs are this crate's own small types ([`Quantity`],
//! [`ledger`]) until `loams-meter-core` is on `dev`; [`Quantity`] has the same
//! representation as its `Amount` (value × 10⁹, `Decimal(38, 9)`).
//!
//! **Idempotency.** Every exporter writes what it plans to send to an
//! [`ExportLedger`] *before* it sends, under a key that does not change on a
//! retry, so a crash or a provider timeout resends the same identifier. The
//! ledger is backed by the `ControlStore` in the `meter` role (§63 §16:
//! "Each exporter's cursor … is in the `ControlStore`").

#[cfg(feature = "lago")]
pub mod lago;
pub mod ledger;
pub mod quantity;
pub mod retry;

#[cfg(feature = "openmeter")]
pub mod openmeter;
#[cfg(feature = "stripe")]
pub mod stripe;
#[cfg(feature = "webhook")]
pub mod webhook;
pub mod window;

pub use ledger::{ExportCursors, ExportLedger, LedgerEntry, LedgerKey, MemCursors, MemLedger};
pub use quantity::{Quantity, QuantityError};
pub use retry::RetryPolicy;
pub use window::{WindowRow, WindowSize};

/// What an exporter can fail with. Provider messages are cut to 300
/// characters and never carry request bodies.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The provider refused the request for good (a 4xx other than 408,
    /// 409, 425 and 429): resending it would fail the same way.
    #[error("{provider}: refused with {status}: {message}")]
    Rejected {
        provider: &'static str,
        status: u16,
        message: String,
    },
    /// Every attempt of the retry policy failed with a retryable error; the
    /// ledger keeps the planned send, and the next run resends it unchanged.
    #[error("{provider}: gave up after {attempts} attempts: {last}")]
    Exhausted {
        provider: &'static str,
        attempts: u32,
        last: String,
    },
    /// The export ledger (the `ControlStore`) failed.
    #[error("export ledger: {0}")]
    Ledger(String),
    /// The exporter is misconfigured or was given input it must not send.
    #[error("{0}")]
    Invalid(String),
    /// A Stripe billing period that does not start on a whole UTC hour: its
    /// hours would straddle the period, so the customer's export stops and
    /// the meter alerts (§63 §16).
    #[error(
        "the period of {subject} starts at {start}, not on a whole UTC hour: its export is stopped"
    )]
    OffHourPeriod { subject: String, start: i64 },
    /// A provider webhook that failed verification or could not be read.
    #[error("webhook: {0}")]
    Webhook(String),
}

#[cfg_attr(
    not(any(
        feature = "stripe",
        feature = "openmeter",
        feature = "lago",
        feature = "webhook"
    )),
    allow(dead_code)
)]
/// Cuts a provider's message for an error (300 characters, one line).
pub(crate) fn short(message: &str) -> String {
    let one_line: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut cut: String = one_line.chars().take(300).collect();
    if one_line.chars().count() > 300 {
        cut.push('…');
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::short;

    #[test]
    fn short_cuts_and_flattens() {
        assert_eq!(short("a\nb"), "a b");
        let long = "x".repeat(400);
        assert_eq!(short(&long).chars().count(), 301);
    }
}
