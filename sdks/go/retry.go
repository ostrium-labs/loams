// Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
//
// The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
// doubling, capped at 2 s, 3 retries, **full** jitter. Full jitter rather than
// exponential backoff alone, because every client retrying at the same instant
// after a node restart is how a recovering node gets knocked over again.
//
// The context does the rest. In Go a retry loop that does not watch its context
// is a bug rather than a missing feature: `sleep` returns as soon as `ctx` is
// done, so a shutdown never waits out a backoff, and a caller who set a deadline
// gets it honoured for the whole call rather than per attempt.

package loams

import (
	"context"
	"math/rand/v2"
	"time"

	connect "connectrpc.com/connect"
)

// BaseDelayMS is the first backoff, and the multiplier's base.
const BaseDelayMS = 100

// MaxDelayMS is the ceiling on one backoff.
const MaxDelayMS = 2000

// DefaultMaxRetries is the retries after the first attempt, when nothing
// overrides it.
const DefaultMaxRetries = 3

// MaxServerDelayMS is the ceiling on a server-sent `RetryInfo.retry_delay`.
// No proto carries `RetryInfo` yet (R2), so nothing today reaches it; the hook
// is here so the day one does, the numbers are already right.
const MaxServerDelayMS = 30_000

// retryable is the set of codes a retry may answer (D610).
var retryable = map[Code]struct{}{
	connect.CodeUnavailable:       {},
	connect.CodeDeadlineExceeded:  {},
	connect.CodeResourceExhausted: {},
}

// IsRetryableCode whether a code is one a retry may answer.
func IsRetryableCode(code Code) bool {
	_, ok := retryable[code]
	return ok
}

// Backoff is the wait before retry number attempt (0 for the first retry), with
// full jitter: uniform over [0, min(cap, base × 2^attempt)]. A server-sent
// `RetryInfo.retry_delay` replaces it, up to MaxServerDelayMS.
func Backoff(attempt int, serverDelay time.Duration) time.Duration {
	if serverDelay > 0 {
		if serverDelay > MaxServerDelayMS*time.Millisecond {
			return MaxServerDelayMS * time.Millisecond
		}
		return serverDelay
	}
	ceiling := MaxDelayMS
	if attempt < 16 {
		if grown := BaseDelayMS << attempt; grown < ceiling {
			ceiling = grown
		}
	}
	return time.Duration(rand.Int64N(int64(ceiling))) * time.Millisecond
}

// Sleep waits, or returns early when the context is done.
//
// It reports the context's error, so the retry loop can distinguish "the wait
// finished" from "the caller gave up": the second must not be charged to the
// call's retry budget, because spending a retry on a cancelled call is how a
// shutdown turns into a stall.
func Sleep(ctx context.Context, d time.Duration) error {
	if d <= 0 {
		return ctx.Err()
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

// ShouldRetry whether one more attempt is allowed.
//
// retrySafe is the call's class from the generated bindings: `safe` for reads
// and idempotent RPCs, `manual` for a mutation. A mutation becomes retryable
// once it carries an idempotency key, because the key is what makes the repeat
// safe — see `withIdempotencyKey`, which sets one before the first attempt and
// reuses it on every retry.
//
// A cancelled context is never retried, whatever the class: the caller has said
// they do not want the answer, and a retry cannot change that.
func ShouldRetry(ctx context.Context, err error, retrySafe bool, attempt, maxRetries int) bool {
	if ctx.Err() != nil {
		return false
	}
	if attempt >= maxRetries {
		return false
	}
	if !retrySafe {
		return false
	}
	return IsRetryableCode(CodeOf(err))
}
