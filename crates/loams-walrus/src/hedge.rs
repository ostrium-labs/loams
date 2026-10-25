//! Hedged requests across aggregators.
//!
//! An attempt starts on aggregator 0. If no attempt has succeeded after
//! `hedge_after`, the next aggregator is asked too, and so on down the
//! list; a failed attempt starts the next aggregator at once. The first
//! success wins, and the other attempts are dropped, which cancels their
//! requests.

use std::future::Future;
use std::pin::pin;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use url::Url;

use crate::error::{AttemptFailure, WalrusError};
use crate::types::ContentDigest;

/// Runs `attempt` against `aggregators` (at least one) with hedging and
/// returns the first success, or one error for all the failures.
pub(crate) async fn hedged<T, F, Fut>(
    aggregators: &[Url],
    hedge_after: Duration,
    label: &str,
    expected: Option<&ContentDigest>,
    attempt: F,
) -> Result<T, WalrusError>
where
    F: Fn(Url) -> Fut,
    Fut: Future<Output = Result<T, AttemptFailure>>,
{
    let launch = |agg: Url| {
        let fut = attempt(agg.clone());
        async move { (agg, fut.await) }
    };
    let mut pending = FuturesUnordered::new();
    let mut failures = Vec::new();
    let mut next = 0;
    if let Some(first) = aggregators.first() {
        pending.push(launch(first.clone()));
        next = 1;
    }
    let mut hedge = pin!(tokio::time::sleep(hedge_after));
    loop {
        tokio::select! {
            Some((agg, result)) = pending.next() => match result {
                Ok(value) => return Ok(value),
                Err(why) => {
                    if let AttemptFailure::Integrity { actual } = &why {
                        tracing::warn!(aggregator = %agg, blob = label, %actual,
                            "walrus: aggregator sent bytes with the wrong sha256");
                    } else {
                        tracing::debug!(aggregator = %agg, blob = label, %why,
                            "walrus: read attempt failed");
                    }
                    failures.push((agg, why));
                    if let Some(agg) = aggregators.get(next) {
                        pending.push(launch(agg.clone()));
                        next += 1;
                        hedge.as_mut().reset(tokio::time::Instant::now() + hedge_after);
                    } else if pending.is_empty() {
                        break;
                    }
                }
            },
            () = &mut hedge, if next < aggregators.len() => {
                tracing::debug!(blob = label, aggregator = %aggregators[next], "walrus: read hedged");
                pending.push(launch(aggregators[next].clone()));
                next += 1;
                hedge.as_mut().reset(tokio::time::Instant::now() + hedge_after);
            },
            else => break,
        }
    }
    Err(collapse(label, expected, failures))
}

/// One error for an operation every aggregator failed: `NotFound` when all
/// said 404, `Integrity` when all sent wrong bytes, `TooLarge` when all
/// bodies were over the limit, otherwise `AllAggregatorsFailed`.
pub(crate) fn collapse(
    label: &str,
    expected: Option<&ContentDigest>,
    attempts: Vec<(Url, AttemptFailure)>,
) -> WalrusError {
    let all =
        |f: fn(&AttemptFailure) -> bool| !attempts.is_empty() && attempts.iter().all(|(_, w)| f(w));
    if all(|w| *w == AttemptFailure::NotFound) {
        return WalrusError::NotFound(label.to_string());
    }
    if let Some(expected) = expected
        && all(|w| matches!(w, AttemptFailure::Integrity { .. }))
        && let Some((aggregator, AttemptFailure::Integrity { actual })) = attempts.last()
    {
        return WalrusError::Integrity {
            blob: label.to_string(),
            aggregator: aggregator.to_string(),
            expected: *expected,
            actual: *actual,
        };
    }
    if all(|w| matches!(w, AttemptFailure::TooLarge { .. }))
        && let Some((_, AttemptFailure::TooLarge { size, max })) = attempts.last()
    {
        return WalrusError::TooLarge {
            what: "blob",
            size: *size,
            max: *max,
        };
    }
    WalrusError::AllAggregatorsFailed {
        blob: label.to_string(),
        attempts,
    }
}
