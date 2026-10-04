// Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
//
// The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
// doubling, capped at 2 s, 3 retries, **full** jitter. Full jitter rather than
// exponential backoff alone, because every client retrying at the same instant
// after a node restart is how a recovering node gets knocked over again.
//
// `Task.checkCancellation()` does the rest. In Swift a retry loop that does not
// check is a bug rather than a missing feature: `Task.sleep` throws on
// cancellation, so a shutdown never waits out a backoff, and a caller who set a
// deadline gets it honoured for the whole call rather than per attempt.

import Foundation

/// The first backoff, and the multiplier's base.
public let loamsBaseDelayMilliseconds = 100

/// The ceiling on one backoff.
public let loamsMaxDelayMilliseconds = 2000

/// The retries after the first attempt, when nothing overrides it.
public let loamsDefaultMaxRetries = 3

/// The ceiling on a server-sent `RetryInfo.retry_delay`.
///
/// No proto carries `RetryInfo` yet (R2), so nothing today reaches it; the hook
/// is here so the day one does, the numbers are already right.
public let loamsMaxServerDelayMilliseconds = 30_000

/// The codes a retry may answer (D610).
public func loamsIsRetryableCode(_ code: Code) -> Bool {
    switch code {
    case .unavailable, .deadlineExceeded, .resourceExhausted: return true
    default: return false
    }
}

/// The wait before retry number `attempt` (0 for the first retry), with **full
/// jitter**: uniform over `[0, min(cap, base × 2^attempt)]`.
///
/// A server-sent `RetryInfo.retry_delay` replaces it, up to
/// ``loamsMaxServerDelayMilliseconds``. This is the whole of R2's backoff: there
/// is no exponential-only variant, because a synchronised fleet is the failure
/// the cap and the jitter exist to prevent.
public func loamsBackoff(attempt: Int, serverDelayMilliseconds: Int = 0) -> Int {
    if serverDelayMilliseconds > 0 {
        return min(serverDelayMilliseconds, loamsMaxServerDelayMilliseconds)
    }
    var ceiling = loamsMaxDelayMilliseconds
    // Shifted rather than multiplied, and bounded at 16 so a large `attempt`
    // cannot overflow into a negative ceiling — which would make `Double.random`
    // below trap. A caller passing `attempt = 1000` gets the cap, not a crash.
    if attempt >= 0 && attempt < 16 {
        let grown = loamsBaseDelayMilliseconds << attempt
        if grown < ceiling { ceiling = grown }
    }
    return Int.random(in: 0...ceiling)
}

/// Whether one more attempt is allowed.
///
/// `retrySafe` is the call's class from the generated bindings: `.safe` for reads
/// and idempotent RPCs, `.manual` for a mutation. A mutation becomes retryable
/// once it carries an idempotency key, because the key is what makes the repeat
/// safe — see ``loamsApplyIdempotencyKey``, which sets one before the first
/// attempt and reuses it on every retry.
///
/// Three refusals are absolute, and all three are the contract's:
///
///  - a **cancelled** task, whatever the class: the caller has said they do not
///    want the answer, and a retry cannot change that;
///  - a **deadline** the caller already spent, for the same reason — retrying
///    past a deadline the caller set would answer later than they asked;
///  - the **retry budget**, which bounds the whole call rather than one attempt.
public func loamsShouldRetry(
    error: any Error,
    retrySafe: Bool,
    attempt: Int,
    maxRetries: Int,
    isCancelled: @Sendable () -> Bool = { Task.isCancelled }
) -> Bool {
    if isCancelled() { return false }
    if attempt >= maxRetries { return false }
    if !retrySafe { return false }
    let code = LoamsError.code(of: error)
    if code == .canceled { return false }
    return loamsIsRetryableCode(code)
}

/// Sleeps for `milliseconds`, or throws immediately when the task is cancelled.
///
/// It propagates the cancellation rather than swallowing it, so the retry loop
/// can distinguish "the wait finished" from "the caller gave up": the second
/// must not be charged to the call's retry budget, because spending a retry on a
/// cancelled call is how a shutdown turns into a stall.
public func loamsSleep(milliseconds: Int) async throws {
    guard milliseconds > 0 else {
        try Task.checkCancellation()
        return
    }
    try await Task.sleep(nanoseconds: UInt64(milliseconds) * 1_000_000)
}