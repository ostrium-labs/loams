//! Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
//!
//! The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
//! doubling, capped at 2 s, 3 retries, **full jitter**. Full jitter rather than
//! exponential backoff alone, because every client retrying at the same instant
//! after a node restart is how a recovering node gets knocked over again.
//!
//! `RetryInfo.retry_delay` replaces the computed backoff, up to 30 s — but
//! **no proto carries `RetryInfo` yet**, so nothing an SDK can read today sets
//! it. [`backoff`] takes one anyway, because the moment a proto does carry it
//! the rule is already here.

use std::time::Duration;

use connectrpc::ErrorCode as Code;

use crate::error::LoamsError;

/// The first backoff, and the multiplier's base.
pub const BASE_DELAY: Duration = Duration::from_millis(100);
/// The ceiling on one computed backoff.
pub const MAX_DELAY: Duration = Duration::from_millis(2_000);
/// Retries after the first attempt, when nothing overrides it.
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// `RetryInfo.retry_delay` is honoured up to this.
pub const MAX_SERVER_DELAY: Duration = Duration::from_millis(30_000);

/// The codes a retry may answer (D610).
const RETRYABLE: &[Code] = &[
    Code::Unavailable,
    Code::DeadlineExceeded,
    Code::ResourceExhausted,
];

/// Whether a code is one a retry may answer.
#[must_use]
pub fn is_retryable_code(code: Code) -> bool {
    RETRYABLE.contains(&code)
}

/// The backoff before retry number `attempt` (0 for the first), with full
/// jitter: uniform over `[0, min(cap, base × 2^attempt)]`. A server-sent
/// `RetryInfo.retry_delay` replaces it, up to [`MAX_SERVER_DELAY`].
///
/// `random` is passed in rather than read from a global so the bounds are
/// testable and a caller can supply a seeded generator.
#[must_use]
pub fn backoff(attempt: u32, server_delay: Option<Duration>, random: f64) -> Duration {
    if let Some(server) = server_delay.filter(|d| !d.is_zero()) {
        return server.min(MAX_SERVER_DELAY);
    }
    // `2^attempt` in milliseconds, saturating: a caller that leaves a stream
    // retrying for a week must not overflow the shift into a zero-length
    // backoff, which would turn a reconnect storm into a spin.
    let ceiling_ms = BASE_DELAY
        .as_millis()
        .saturating_mul(1u128 << attempt.min(32))
        .min(MAX_DELAY.as_millis()) as u64;
    let millis = (random.clamp(0.0, 1.0) * ceiling_ms as f64).floor() as u64;
    Duration::from_millis(millis)
}

/// Whether one more attempt is allowed.
///
/// `retry_safe` is the call's class from the generated bindings: [`RetryClass::Safe`]
/// for a read or an idempotent RPC, [`RetryClass::Manual`] for a mutation. A
/// mutation becomes retryable once it carries an idempotency key, because the
/// key is what makes the repeat safe — see [`crate::request::with_idempotency_key`],
/// which sets one before the first attempt and reuses it on every retry.
#[must_use]
pub fn should_retry(error: &LoamsError, retry_safe: bool, attempt: u32, max_retries: u32) -> bool {
    if attempt >= max_retries {
        return false;
    }
    retry_safe && is_retryable_code(error.code)
}

/// How a call may be retried, as the generated bindings state it (D610).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetryClass {
    /// A read (`NO_SIDE_EFFECTS`) or an `IDEMPOTENT` RPC: it retries on its own.
    Safe,
    /// A mutation: it retries only once it carries an idempotency key.
    Manual,
}

impl RetryClass {
    /// The class a proto's `idempotency_level` implies. `NO_SIDE_EFFECTS` and
    /// `IDEMPOTENT` are safe; everything else, including "the proto says
    /// nothing", is manual. Guessing safe for an unmarked RPC is how a write
    /// gets written twice.
    #[must_use]
    pub fn of_idempotency_level(level: connectrpc::IdempotencyLevel) -> RetryClass {
        match level {
            connectrpc::IdempotencyLevel::NoSideEffects
            | connectrpc::IdempotencyLevel::Idempotent => RetryClass::Safe,
            connectrpc::IdempotencyLevel::Unknown => RetryClass::Manual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn the_backoff_is_bounded_by_the_cap_and_jittered_within_it() {
        for attempt in 0..8 {
            let ceiling = BASE_DELAY.as_millis() * 2u128.pow(attempt);
            let ceiling = ceiling.min(MAX_DELAY.as_millis());
            for step in 0..20 {
                let random = step as f64 / 20.0;
                let delay = backoff(attempt, None, random);
                assert!(
                    delay.as_millis() <= ceiling,
                    "attempt {attempt} random {random}"
                );
            }
        }
    }

    #[test]
    fn a_server_delay_replaces_the_computed_backoff_up_to_the_cap() {
        assert_eq!(
            backoff(3, Some(Duration::from_millis(7)), 0.99),
            Duration::from_millis(7)
        );
        assert_eq!(
            backoff(0, Some(Duration::from_secs(90)), 0.5),
            MAX_SERVER_DELAY
        );
        // A zero delay is not a server delay; it falls back to the computed one.
        assert!(backoff(0, Some(Duration::ZERO), 1.0) > Duration::ZERO);
    }

    #[test]
    fn only_three_codes_are_retryable() {
        for code in [
            Code::Unavailable,
            Code::DeadlineExceeded,
            Code::ResourceExhausted,
        ] {
            assert!(is_retryable_code(code), "{code:?}");
        }
        for code in [
            Code::Internal,
            Code::Unimplemented,
            Code::NotFound,
            Code::Unknown,
        ] {
            assert!(!is_retryable_code(code), "{code:?}");
        }
    }

    #[test]
    fn an_unavailable_failure_retries_only_within_the_budget_and_the_class() {
        let error = LoamsError::new(ErrorKind::Unavailable, Code::Unavailable, "restarting");
        assert!(should_retry(&error, true, 0, 3));
        assert!(should_retry(&error, true, 2, 3));
        assert!(!should_retry(&error, true, 3, 3), "the budget is spent");
        assert!(
            !should_retry(&error, false, 0, 3),
            "a mutation without a key"
        );
    }

    #[test]
    fn a_read_is_safe_and_an_unmarked_mutation_is_not() {
        use connectrpc::IdempotencyLevel;
        assert_eq!(
            RetryClass::of_idempotency_level(IdempotencyLevel::NoSideEffects),
            RetryClass::Safe
        );
        assert_eq!(
            RetryClass::of_idempotency_level(IdempotencyLevel::Idempotent),
            RetryClass::Safe
        );
        assert_eq!(
            RetryClass::of_idempotency_level(IdempotencyLevel::Unknown),
            RetryClass::Manual
        );
    }
}
