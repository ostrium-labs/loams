package dev.loams;

import java.time.Duration;
import java.util.Collections;
import java.util.EnumSet;
import java.util.Set;
import java.util.concurrent.ThreadLocalRandom;

/**
 * Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
 *
 * <p>The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms, doubling,
 * capped at 2 s, 3 retries, <b>full</b> jitter. Full jitter rather than plain exponential backoff
 * because every client retrying at the same instant after a node restart is how a recovering node
 * gets knocked over again.
 *
 * <p>Everything here is static and takes the failure as an argument rather than reaching for a
 * client, so the policy can be tested without a transport. That is what lets
 * {@code java_retry_reuses_idempotency_key} pin the policy with a stub send instead of a server.
 */
public final class Retry {

    private Retry() {}

    /** The first backoff, and the multiplier's base. */
    public static final int BASE_DELAY_MS = 100;

    /** The ceiling on one computed backoff. */
    public static final int MAX_DELAY_MS = 2000;

    /** The retries after the first attempt, when nothing overrides it. */
    public static final int DEFAULT_MAX_RETRIES = 3;

    /**
     * The ceiling on a server-sent {@code RetryInfo.retry_delay}.
     *
     * <p>No proto carries {@code RetryInfo} yet (R2), so nothing today reaches it; the hook is
     * here so the day one does the number is already right.
     */
    public static final int MAX_SERVER_DELAY_MS = 30_000;

    /** The codes a retry may answer (D610). */
    private static final Set<Code> RETRYABLE =
            EnumSet.of(Code.UNAVAILABLE, Code.DEADLINE_EXCEEDED, Code.RESOURCE_EXHAUSTED);

    /** Whether a code is one a retry may answer. */
    public static boolean isRetryableCode(Code code) {
        return RETRYABLE.contains(code);
    }

    /** The retryable codes, for a caller that wants to log them. */
    public static Set<Code> retryableCodes() {
        return Collections.unmodifiableSet(RETRYABLE);
    }

    /**
     * The wait before retry number {@code attempt} (0 for the first retry), with full jitter:
     * uniform over {@code [0, min(cap, base << attempt)]}.
     *
     * <p>A server-sent {@code RetryInfo.retry_delay} replaces the computed backoff, up to
     * {@link #MAX_SERVER_DELAY_MS}.
     */
    public static Duration backoff(int attempt, Duration serverDelay) {
        if (serverDelay != null && !serverDelay.isNegative() && !serverDelay.isZero()) {
            long millis = serverDelay.toMillis();
            return Duration.ofMillis(Math.min(millis, MAX_SERVER_DELAY_MS));
        }
        int ceiling = MAX_DELAY_MS;
        if (attempt < 16) {
            int grown = BASE_DELAY_MS << attempt;
            if (grown < ceiling) {
                ceiling = grown;
            }
        }
        // `nextInt(bound)` is half-open, so 1 is added: a ceiling of zero would throw, and a
        // ceiling of 100 must be able to return 0 so that "full" jitter is full.
        return Duration.ofMillis(ThreadLocalRandom.current().nextInt(ceiling + 1));
    }

    /**
     * Whether one more attempt is allowed.
     *
     * <p>{@code retrySafe} is the call's class from the generated bindings: {@code safe} for
     * reads and idempotent RPCs, {@code manual} for a mutation. A mutation becomes retryable once
     * it carries an idempotency key, because the key is what makes the repeat safe — see
     * {@link Idempotency}, which sets one before the first attempt and reuses it on every retry.
     *
     * @param callerGaveUp whether the caller's context is already done. A cancelled call is never
     *     retried whatever the class: the caller has said they do not want the answer, and a
     *     retry cannot change that.
     */
    public static boolean shouldRetry(
            boolean callerGaveUp, Throwable error, boolean retrySafe, int attempt, int maxRetries) {
        if (callerGaveUp) {
            return false;
        }
        if (attempt >= maxRetries) {
            return false;
        }
        if (!retrySafe) {
            return false;
        }
        return isRetryableCode(Errors.codeOf(error));
    }

    /**
     * Wait, or return early when the caller gave up.
     *
     * <p>It reports whether the wait finished, so the retry loop can tell "the backoff elapsed"
     * from "the caller is gone": the second must not be charged to the call's retry budget,
     * because spending a retry on a cancelled call is how a shutdown turns into a stall.
     *
     * @return {@code true} when the full delay elapsed, {@code false} when the thread was
     *     interrupted first
     */
    public static boolean sleep(Duration delay) throws InterruptedException {
        long millis = delay.toMillis();
        if (millis <= 0) {
            return true;
        }
        long deadline = System.nanoTime() + millis * 1_000_000L;
        long remaining = millis;
        while (remaining > 0) {
            // Sleep in slices so an interrupt during a two-second backoff is not two seconds
            // late. This is the whole reason the loop is here rather than one Thread.sleep.
            Thread.sleep(Math.min(remaining, 50));
            long left = deadline - System.nanoTime();
            if (left <= 0) {
                return true;
            }
            remaining = left / 1_000_000L;
        }
        return true;
    }
}