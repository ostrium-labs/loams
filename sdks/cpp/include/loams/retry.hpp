// Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
//
// The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
// doubling, capped at 2 s, 3 retries, **full** jitter. Full jitter rather than
// exponential backoff alone, because every client retrying at the same instant
// after a node restart is how a recovering node gets knocked over again.
//
// A call's retry class comes from the **generated bindings**, not from a
// guess: reads (`NO_SIDE_EFFECTS`) and idempotent RPCs retry on their own; a
// mutation does not, unless it carries an idempotency key, in which case R3's
// key makes the repeat safe and the class becomes safe too. A retryable code is
// `unavailable`, `deadline_exceeded` or `resource_exhausted`.
//
// No proto carries `RetryInfo` today, so nothing reaches the server-delay path
// yet; `BackoffMs` takes one anyway so the numbers are already right when a
// proto does (R2).

#ifndef LOAMS_RETRY_HPP
#define LOAMS_RETRY_HPP

#include "loams/error.hpp"

namespace loams {

/// The first backoff, and the multiplier's base.
inline constexpr int kBaseDelayMs = 100;

/// The ceiling on one computed backoff.
inline constexpr int kMaxDelayMs = 2000;

/// The retries after the first attempt, when nothing overrides it.
inline constexpr int kDefaultMaxRetries = 3;

/// The ceiling on a server-sent `RetryInfo.retry_delay`, which replaces the
/// computed backoff.
inline constexpr int kMaxServerDelayMs = 30000;

/// A call's retry class, as the generated bindings declare it.
enum class RetryClass {
  /// A read (`NO_SIDE_EFFECTS`) or an `IDEMPOTENT` RPC: retries on its own.
  kSafe,
  /// A mutation: retries only once it carries an idempotency key.
  kManual,
};

/// Whether a code is one a retry may answer (D610).
bool IsRetryableCode(Code code);

/// The wait before retry number `attempt` (0 for the first retry), with full
/// jitter: uniform over `[0, min(kMaxDelayMs, kBaseDelayMs << attempt)]`.
/// A positive `server_delay_ms` replaces it, capped at `kMaxServerDelayMs`.
///
/// `attempt` is clamped: `kBaseDelayMs << attempt` is a signed shift, and a
/// caller that passed 64 to mean "a lot" would otherwise shift into the sign
/// bit and get a **negative** ceiling, which `rand() % negative` turns into a
/// negative delay and a sleep of undefined behaviour.
int BackoffMs(int attempt, int server_delay_ms);

/// Whether one more attempt is allowed.
///
/// `retry_safe` is the call's class, already resolved against R3's idempotency
/// key by the caller of this function. A cancelled call is never retried,
/// whatever the class: the caller has said they do not want the answer, and a
/// retry cannot change that.
bool ShouldRetry(const std::exception& error, bool retry_safe, int attempt, int max_retries);

}  // namespace loams

#endif  // LOAMS_RETRY_HPP