package dev.loams.core.watch

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * One message of a `Watch*` stream (AP0 Ruling 3), independent of the generated proto types.
 * Every message carries the cursor to resume from after it.
 */
sealed interface WatchEvent<out T> {
    val cursor: String

    /** Every matching object. [reset] means the resume cursor was too old. */
    data class Snapshot<T>(val items: List<T>, override val cursor: String, val reset: Boolean = false) : WatchEvent<T>

    data class Upsert<T>(val item: T, override val cursor: String) : WatchEvent<T>

    data class Remove(val id: String, override val cursor: String) : WatchEvent<Nothing>

    data class Heartbeat(override val cursor: String) : WatchEvent<Nothing>
}

/**
 * Applies a watch stream's messages to a list kept in arrival order, and remembers the cursor a
 * reconnect resumes from. Thread-confined: call it from one coroutine.
 */
class WatchState<T>(private val idOf: (T) -> String) {
    private val _items = MutableStateFlow<List<T>>(emptyList())
    val items: StateFlow<List<T>> = _items.asStateFlow()

    /** The cursor of the last applied message; null before the first snapshot. */
    var cursor: String? = null
        private set

    /** When the last message (of any kind) arrived, in the caller's clock. */
    var lastMessageAtMillis: Long = 0
        private set

    fun apply(event: WatchEvent<T>, nowMillis: Long) {
        lastMessageAtMillis = nowMillis
        cursor = event.cursor
        when (event) {
            is WatchEvent.Snapshot -> _items.value = event.items
            is WatchEvent.Upsert -> {
                val id = idOf(event.item)
                val current = _items.value
                val at = current.indexOfFirst { idOf(it) == id }
                _items.value = if (at < 0) current + event.item else current.toMutableList().also { it[at] = event.item }
            }
            is WatchEvent.Remove -> _items.value = _items.value.filterNot { idOf(it) == event.id }
            is WatchEvent.Heartbeat -> Unit
        }
    }

    /** True when nothing arrived for [DEAD_AFTER_MILLIS]: three missed 15 s heartbeats. */
    fun isDead(nowMillis: Long): Boolean = lastMessageAtMillis != 0L && nowMillis - lastMessageAtMillis > DEAD_AFTER_MILLIS

    /** Drops cached items and the cursor, for sign-out or revocation. */
    fun clear() {
        _items.value = emptyList()
        cursor = null
        lastMessageAtMillis = 0
    }

    companion object {
        const val HEARTBEAT_MILLIS = 15_000L
        const val DEAD_AFTER_MILLIS = 3 * HEARTBEAT_MILLIS
    }
}
