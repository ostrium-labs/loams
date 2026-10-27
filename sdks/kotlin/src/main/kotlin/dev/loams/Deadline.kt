package dev.loams

import java.time.Duration

/**
 * One logical call's budget (runtime contract R12, SDKP §2.1).
 *
 * A unary call has one budget, 30 s unless the client or the call names another, covering every
 * attempt and every backoff. Each attempt tells the server what is left (`connect-timeout-ms`).
 * When the budget runs out the call fails with `deadline_exceeded`, reason
 * `client_deadline_exceeded`, and is not retried: there is no time left to retry in.
 */
class Deadline private constructor(private val budget: Duration) {
    private val end: Long = if (live(budget)) System.nanoTime() + budget.toNanos() else 0

    /** Whether there is a budget at all. */
    val active: Boolean get() = end != 0L

    /** What is left, or null when there is no budget. */
    fun remaining(): Duration? =
        if (end == 0L) null else Duration.ofNanos(maxOf(end - System.nanoTime(), 1_000_000L))

    /** Whether the budget is spent. */
    fun expired(): Boolean = end != 0L && System.nanoTime() - end >= 0

    /** Whether waiting [waitMs] would spend the budget. */
    fun wouldPass(waitMs: Long): Boolean = end != 0L && System.nanoTime() + waitMs * 1_000_000 - end >= 0

    /** The error a spent budget raises. */
    fun exceeded(rpc: String): DeadlineExceededException {
        val ms = budget.toMillis().toString()
        return DeadlineExceededException(
            rpc = rpc,
            unknownReason = CLIENT_DEADLINE_EXCEEDED,
            metadata = mapOf("timeout_ms" to ms),
            message = "$rpc: deadline_exceeded ($CLIENT_DEADLINE_EXCEEDED): the call did not finish in $ms ms",
        )
    }

    companion object {
        /** The client-side reason a spent budget carries (docs/api/reasons.md, client-side). */
        const val CLIENT_DEADLINE_EXCEEDED: String = "client_deadline_exceeded"

        private fun live(budget: Duration?) = budget != null && !budget.isZero && !budget.isNegative

        /** Starts a budget; zero means none. */
        fun start(budget: Duration): Deadline = Deadline(budget)

        /** Whether an error is a spent client budget. */
        fun isClientDeadline(error: Throwable): Boolean =
            error is LoamsException && error.unknownReason == CLIENT_DEADLINE_EXCEEDED
    }
}
