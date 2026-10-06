package dev.loams.core

import app.cash.turbine.test
import dev.loams.core.watch.Backoff
import dev.loams.core.watch.ResumingWatch
import dev.loams.core.watch.StreamStatus
import dev.loams.core.watch.WatchEvent
import dev.loams.core.watch.WatchState
import java.io.IOException
import kotlin.random.Random
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class WatchTest {
    data class Item(val id: String, val v: Int)

    @Test
    fun watch_applies_snapshot_then_changes() {
        val s = WatchState<Item> { it.id }
        s.apply(WatchEvent.Snapshot(listOf(Item("a", 1), Item("b", 1)), "1"), 0)
        s.apply(WatchEvent.Upsert(Item("b", 2), "2"), 1)
        s.apply(WatchEvent.Upsert(Item("c", 1), "3"), 2)
        s.apply(WatchEvent.Remove("a", "4"), 3)
        s.apply(WatchEvent.Heartbeat("4"), 4)
        assertEquals(listOf(Item("b", 2), Item("c", 1)), s.items.value)
        assertEquals("4", s.cursor)
    }

    @Test
    fun watch_reset_replaces_items() {
        val s = WatchState<Item> { it.id }
        s.apply(WatchEvent.Snapshot(listOf(Item("a", 1)), "1"), 0)
        s.apply(WatchEvent.Snapshot(listOf(Item("z", 9)), "77", reset = true), 1)
        assertEquals(listOf(Item("z", 9)), s.items.value)
    }

    @Test
    fun watch_dead_after_three_missed_heartbeats() {
        val s = WatchState<Item> { it.id }
        s.apply(WatchEvent.Heartbeat("1"), 1_000)
        assertFalse(s.isDead(1_000 + 45_000))
        assertTrue(s.isDead(1_000 + 45_001))
    }

    @Test
    fun backoff_caps_at_10s() {
        val b = Backoff(Random(1))
        val delays = List(12) { b.nextDelayMillis() }
        assertTrue(delays.first() in 250L..500L)
        assertTrue(delays.all { it <= 10_000L })
        assertTrue(delays.last() >= 5_000L)
        b.reset()
        assertTrue(b.nextDelayMillis() <= 500L)
    }

    @Test
    fun resumes_from_cursor_after_a_drop() = runTest {
        val state = WatchState<Item> { it.id }
        val cursors = mutableListOf<String?>()
        var calls = 0
        val watch = ResumingWatch(
            state = state,
            open = { cursor ->
                cursors += cursor
                calls++
                flow {
                    if (calls == 1) {
                        emit(WatchEvent.Snapshot(listOf(Item("a", 1)), "5"))
                        throw IOException("network lost")
                    }
                    emit(WatchEvent.Upsert(Item("b", 1), "6"))
                    awaitCancellation()
                }
            },
            clock = { testScheduler.currentTime },
            backoff = Backoff(Random(0)),
        )
        val job = launch { watch.run() }
        runCurrent()
        assertTrue(watch.status.value is StreamStatus.Reconnecting)
        advanceTimeBy(600)
        runCurrent()
        assertEquals(listOf(null, "5"), cursors)
        assertEquals(StreamStatus.Live, watch.status.value)
        assertEquals(listOf(Item("a", 1), Item("b", 1)), state.items.value)
        job.cancel()
    }

    @Test
    fun a_silent_stream_is_reopened_after_45s() = runTest {
        val state = WatchState<Item> { it.id }
        var opens = 0
        val watch = ResumingWatch(
            state = state,
            open = {
                opens++
                flow {
                    emit(WatchEvent.Snapshot(emptyList(), "1"))
                    awaitCancellation()
                }
            },
            clock = { testScheduler.currentTime },
            backoff = Backoff(Random(0)),
        )
        watch.status.test {
            val job = launch { watch.run() }
            assertEquals(StreamStatus.Connecting, awaitItem())
            assertEquals(StreamStatus.Live, awaitItem())
            advanceTimeBy(45_001)
            val r = awaitItem()
            assertTrue(r is StreamStatus.Reconnecting)
            job.cancel()
            cancelAndIgnoreRemainingEvents()
        }
        assertEquals(1, opens)
    }

    @Test
    fun fatal_failure_stops() = runTest {
        val watch = ResumingWatch<Item>(
            state = WatchState { it.id },
            open = { flow { throw IllegalStateException("device revoked") } },
            isFatal = { it is IllegalStateException },
        )
        watch.run()
        assertEquals(StreamStatus.Stopped("device revoked"), watch.status.value)
    }
}
