package dev.loams.core.watch

import kotlin.math.min
import kotlin.random.Random

/**
 * Reconnect delays: 500 ms doubling to a 10 s cap, with jitter in [delay/2, delay], so a fleet of
 * phones does not reconnect in lock-step after a server restart.
 */
class Backoff(private val random: Random = Random.Default) {
    private var attempt = 0

    fun nextDelayMillis(): Long {
        val base = min(MAX_MILLIS, INITIAL_MILLIS shl min(attempt, 10))
        attempt++
        return base / 2 + random.nextLong(base / 2 + 1)
    }

    fun reset() {
        attempt = 0
    }

    companion object {
        const val INITIAL_MILLIS = 500L
        const val MAX_MILLIS = 10_000L
    }
}
