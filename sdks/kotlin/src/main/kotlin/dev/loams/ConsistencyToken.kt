package dev.loams

/**
 * A parsed consistency token (runtime contract R4 as amended by SDKP §2.1): the log positions a
 * write reached, which a read can wait for (`Consistency.at_least`).
 *
 * The encoding is `crates/loams-collection/src/token.rs`'s, `v1:s<stream>/p<partition>@<offset>,…`:
 * one entry per (stream, partition), sorted, with [merge] keeping the max offset per pair. `v1:`
 * alone is the empty token. Every number is canonical decimal, a partition fits in 32 bits and a
 * stream and offset in 64 (unsigned), and anything else is `invalid_argument` raised here, before
 * the network.
 */
class ConsistencyToken private constructor(val positions: List<Position>) {
    /** One entry. `stream` and `offset` are unsigned 64-bit, `partition` unsigned 32-bit. */
    data class Position(val stream: ULong, val partition: UInt, val offset: ULong) : Comparable<Position> {
        override fun compareTo(other: Position): Int =
            compareValuesBy(this, other, { it.stream }, { it.partition }, { it.offset })

        override fun toString(): String = "s$stream/p$partition@$offset"
    }

    /** The union of two tokens, keeping the max offset per (stream, partition). */
    fun merge(other: ConsistencyToken): ConsistencyToken = of(positions + other.positions)

    /** The offset this token holds for a pair, or null. */
    fun offset(stream: ULong, partition: UInt): ULong? =
        positions.filter { it.stream == stream && it.partition == partition }.maxOfOrNull { it.offset }

    override fun toString(): String = "v1:" + positions.joinToString(",")

    override fun equals(other: Any?): Boolean = other is ConsistencyToken && other.positions == positions

    override fun hashCode(): Int = positions.hashCode()

    companion object {
        /** The empty token, `v1:`. */
        val EMPTY: ConsistencyToken = ConsistencyToken(emptyList())

        /** A token over these positions, normalised: sorted, the max offset per pair. */
        fun of(positions: List<Position>): ConsistencyToken {
            val normal = ArrayList<Position>()
            for (p in positions.sorted()) {
                val last = normal.lastOrNull()
                if (last != null && last.stream == p.stream && last.partition == p.partition) {
                    normal[normal.size - 1] = p
                } else {
                    normal.add(p)
                }
            }
            return ConsistencyToken(normal)
        }

        /** Parses a token, or answers null. */
        fun tryParse(text: String?): ConsistencyToken? {
            if (text == null || !text.startsWith("v1:")) return null
            val body = text.substring(3)
            if (body.isEmpty()) return EMPTY
            val positions = ArrayList<Position>()
            for (item in body.split(",")) {
                positions.add(item(item) ?: return null)
            }
            return ConsistencyToken(positions)
        }

        /** Parses a token, throwing `invalid_argument` when it is not a `v1:` token. */
        fun parse(text: String): ConsistencyToken = tryParse(text) ?: throw InvalidArgumentException(
            rpc = "",
            unknownReason = "invalid_consistency_token",
            metadata = mapOf("token" to text),
            message = "invalid consistency token \"$text\": expected v1:s<stream>/p<partition>@<offset>,…",
        )

        private fun item(item: String): Position? {
            if (!item.startsWith("s")) return null
            val slash = item.indexOf("/p")
            if (slash < 0) return null
            val at = item.indexOf('@', slash)
            if (at < 0) return null
            val stream = canonical(item.substring(1, slash)) ?: return null
            val partition = canonical(item.substring(slash + 2, at)) ?: return null
            val offset = canonical(item.substring(at + 1)) ?: return null
            if (partition > UInt.MAX_VALUE.toULong()) return null
            return Position(stream, partition.toUInt(), offset)
        }

        private fun canonical(digits: String): ULong? {
            if (digits.isEmpty() || (digits.length > 1 && digits[0] == '0')) return null
            if (!digits.all { it in '0'..'9' }) return null
            return digits.toULongOrNull()
        }
    }
}
