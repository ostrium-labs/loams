package dev.loams.core.watch

import kotlin.coroutines.cancellation.CancellationException
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.timeout

/** What a screen shows about its live stream. */
sealed interface StreamStatus {
    data object Connecting : StreamStatus

    data object Live : StreamStatus

    /** The stream failed; showing cached data while waiting [retryInMillis] before reconnecting. */
    data class Reconnecting(val retryInMillis: Long, val cause: String?) : StreamStatus

    /** A failure that retrying will not fix (revoked device, signed out). */
    data class Stopped(val cause: String?) : StreamStatus
}

/**
 * Keeps one `Watch*` stream open while the caller's coroutine is active: opens it with the last
 * cursor, applies every message to [state], treats 45 s of silence (three missed heartbeats) as a
 * dead stream, and reconnects with [Backoff]. Mobile networks drop streams; a resume skips the
 * snapshot when the server still has the cursor (AP0 Ruling 3).
 *
 * @param open opens the stream from a cursor (null for a fresh snapshot).
 * @param isFatal failures that end the loop instead of retrying.
 */
class ResumingWatch<T>(
    private val state: WatchState<T>,
    private val open: (cursor: String?) -> Flow<WatchEvent<T>>,
    private val clock: () -> Long = System::currentTimeMillis,
    private val backoff: Backoff = Backoff(),
    private val isFatal: (Throwable) -> Boolean = { false },
) {
    private val _status = MutableStateFlow<StreamStatus>(StreamStatus.Connecting)
    val status: StateFlow<StreamStatus> = _status.asStateFlow()

    /** Runs until cancelled or until a fatal failure. */
    @OptIn(FlowPreview::class)
    suspend fun run() {
        while (true) {
            try {
                _status.value = StreamStatus.Connecting
                open(state.cursor)
                    // Apply upstream of timeout(): its channel drops a buffered message when the
                    // stream fails right after it, and a lost message would lose its cursor.
                    .onEach { event ->
                        state.apply(event, clock())
                        if (_status.value != StreamStatus.Live) {
                            _status.value = StreamStatus.Live
                            backoff.reset()
                        }
                    }
                    .timeout(WatchState.DEAD_AFTER_MILLIS.milliseconds)
                    .collect()
                // The server ended the stream cleanly: resume from the cursor after a short wait.
                retry(null)
            } catch (e: TimeoutCancellationException) {
                retry("no message for ${WatchState.DEAD_AFTER_MILLIS / 1000} s")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                if (isFatal(e)) {
                    _status.value = StreamStatus.Stopped(e.message)
                    return
                }
                retry(e.message)
            }
        }
    }

    private suspend fun retry(cause: String?) {
        val wait = backoff.nextDelayMillis()
        _status.value = StreamStatus.Reconnecting(wait, cause)
        delay(wait)
    }
}
