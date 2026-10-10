package dev.loams

/**
 * Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
 *
 * The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
 * doubling, capped at 2 s, 3 retries, **full** jitter. Full jitter rather than
 * exponential backoff alone, because every client retrying at the same instant after a
 * node restart is how a recovering node gets knocked over again.
 *
 * Cancellation is part of the policy rather than something the loop happens to notice.
 * A caller who cancelled has already stopped wanting the answer, so a retry is never
 * started once it is cancelled, and a caller's deadline therefore covers **the whole
 * call** — retries and backoff included, which is what a deadline means.
 *
 * ## This clause has no fixture, and none is possible
 *
 * `sdks/fixtures/manifest.json` says so in as many words: a retryable `unavailable`
 * needs a dependency to be down and a `resource_exhausted` needs a loaded server, and
 * `RetryInfo` is on no proto at all. So R2 is pinned in every language against a stub,
 * and here it is pinned against a **real call** through the real `CallInvoker` rather
 * than against this function's own return value — which would be a claim about a
 * function rather than about a call.
 */
object RetryPolicy {
    /** The first backoff, and the multiplier's base, in milliseconds. */
    const val BASE_DELAY_MS: Int = 100

    /** The ceiling on one backoff, in milliseconds. */
    const val MAX_DELAY_MS: Int = 2000

    /** The retries after the first attempt, when nothing overrides it. */
    const val DEFAULT_MAX_RETRIES: Int = 3

    /**
     * The ceiling on a server-sent `RetryInfo.retry_delay`, in milliseconds.
     *
     * No proto carries `RetryInfo` today (R2), so nothing reaches this; the hook is
     * here so the day one does, the number is already written down rather than chosen
     * under pressure.
     */
    const val MAX_SERVER_DELAY_MS: Int = 30_000

    private val JITTER = java.util.Random()

    /**
     * The codes a retry may answer (D610).
     *
     * `unavailable`, `deadline_exceeded` and `resource_exhausted`, and nothing else.
     * Notably **not** `internal`: a server that answered with an internal error has
     * already run the handler, and for a mutation that means the write may have
     * happened, so repeating it on the SDK's own initiative is how one logical call
     * becomes two writes. A mutation becomes retryable when it carries an idempotency
     * key, which is a different question and is asked per call.
     */
    fun isRetryableCode(code: Code): Boolean =
        code == Code.UNAVAILABLE || code == Code.DEADLINE_EXCEEDED || code == Code.RESOURCE_EXHAUSTED

    /**
     * The wait before retry number [attempt] (zero for the first retry), with full
     * jitter: uniform over `[0, min(cap, base × 2^attempt)]`.
     *
     * @param attempt zero-based: 0 is the wait before the first retry.
     * @param serverDelayMs a server-sent `RetryInfo.retry_delay` in milliseconds, or 0.
     *   It replaces the computed backoff, capped at [MAX_SERVER_DELAY_MS].
     * @param random the jitter source, so a test can pin the draw.
     */
    fun backoff(attempt: Int, serverDelayMs: Int = 0, random: java.util.Random = JITTER): Long {
        if (serverDelayMs > 0) {
            return minOf(serverDelayMs.toLong(), MAX_SERVER_DELAY_MS.toLong())
        }
        val index = if (attempt < 0) 0 else attempt

        var ceiling = MAX_DELAY_MS.toLong()
        // The shift is guarded rather than trusted: `attempt` is a caller's int, and a
        // shift of 40 or more is zero for a Long, which would silently turn a long
        // wait into no wait.
        if (index < 16) {
            val grown = BASE_DELAY_MS.toLong() shl index
            if (grown < ceiling) {
                ceiling = grown
            }
        }
        // `ceiling` is never 0: MAX_DELAY_MS is 2000 and `index < 16` only ever
        // lowers the value.
        return random.nextLong(ceiling)
    }

    /**
     * Whether one more attempt is allowed.
     *
     * @param attempt the zero-based attempt that just failed.
     * @param maxRetries the retries after the first attempt.
     * @param retrySafe the call's class: [RetryClass.SAFE] for reads and idempotent
     *   RPCs, [RetryClass.MANUAL] for a mutation. A mutation is retryable once it
     *   carries an idempotency key, because the key is what makes the repeat safe —
     *   which is [Idempotency.apply]'s decision, made before the first attempt.
     * @param cancelled whether the caller has already given up.
     *
     * A cancelled call is never retried whatever the class: the caller has said they
     * do not want the answer, and a retry cannot change that. This is also why a
     * negative [maxRetries] is treated as none — a negative budget is a caller asking
     * for no retries, not for unlimited ones, and "unlimited retries" is not a thing an
     * SDK offers.
     */
    fun shouldRetry(attempt: Int, maxRetries: Int, retrySafe: Boolean, code: Code, cancelled: Boolean): Boolean {
        if (cancelled || maxRetries <= 0 || attempt >= maxRetries || !retrySafe) {
            return false
        }
        return isRetryableCode(code)
    }
}