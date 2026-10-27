package dev.loams

/**
 * `streams.subscribe` (runtime contract R14, SDKP §2.5, SDKP-2): a resumable subscription to one
 * partition.
 *
 * A client loop over unary `Fetch`, so it works on every transport: fetch from the next offset,
 * yield what came back, and move the offset to the last record's offset + 1. An empty fetch backs
 * off 100 ms, doubling to 1 s, and the first non-empty one resets it. [offset] is always the next
 * offset to fetch, so a caller that stops can resume from it; [close] stops the loop.
 *
 * An [Iterator] rather than a `Flow`: this SDK's only runtime dependency is protobuf-java, so it
 * has no coroutines. `client.streams.subscribe(...)` wires this to
 * `loams.stream.v1.StreamService/Fetch` when that package is on the main port (SDKP P0c).
 */
class Subscription(
    fromOffset: Long,
    private val sleep: (Long) -> Unit = { Thread.sleep(it) },
    private val fetch: (Long) -> List<Record>,
) : Iterator<Subscription.Record>, AutoCloseable {
    /** One record a subscription yields; [key] is null when it was produced without one. */
    data class Record(val partition: Int, val offset: Long, val value: ByteArray, val timestampMs: Long = 0,
                      val key: ByteArray? = null) {
        /** The value as UTF-8 text. */
        val text: String get() = value.toString(Charsets.UTF_8)

        override fun equals(other: Any?): Boolean =
            other is Record && partition == other.partition && offset == other.offset && value.contentEquals(other.value)

        override fun hashCode(): Int = offset.hashCode()
    }

    @Volatile
    private var next: Long = fromOffset

    @Volatile
    private var closed = false
    private var page: Iterator<Record> = emptyList<Record>().iterator()
    private var ready: Record? = null
    private var backoff = INITIAL_BACKOFF_MS

    /** The next offset to fetch: the last yielded record's offset + 1. Resume from it. */
    val offset: Long get() = next

    override fun hasNext(): Boolean {
        while (ready == null && !closed) {
            while (page.hasNext()) {
                val record = page.next()
                if (record.offset >= next) {
                    ready = record
                    return true
                }
            }
            val records = fetch(next)
            // A page with nothing at or past the offset is an empty one: back off rather than
            // spin on records already yielded.
            val from = next
            if (records.none { it.offset >= from }) {
                try {
                    sleep(backoff)
                } catch (interrupted: InterruptedException) {
                    Thread.currentThread().interrupt()
                    closed = true
                    return false
                }
                backoff = minOf(backoff * 2, MAX_BACKOFF_MS)
                continue
            }
            backoff = INITIAL_BACKOFF_MS
            page = records.iterator()
        }
        return ready != null
    }

    override fun next(): Record {
        if (!hasNext()) throw NoSuchElementException()
        val record = ready!!
        ready = null
        next = record.offset + 1
        return record
    }

    /** Stops the loop. */
    override fun close() {
        closed = true
    }

    companion object {
        const val INITIAL_BACKOFF_MS: Long = 100
        const val MAX_BACKOFF_MS: Long = 1000
    }
}
