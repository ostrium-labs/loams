package dev.loams

/**
 * A protobuf wire reader, for the one message the SDK reads by hand.
 *
 * ## Why a hand-written reader rather than `DynamicMessage`
 *
 * `google.rpc.Status` — which is what a `grpc-status-details-bin` trailer carries, and
 * therefore the only route by which an `ErrorInfo` reaches a gRPC-Web client — is
 * declared by **no proto in this repository**. Adding one would be a second source of
 * truth for a message that only ever appears inside an error trailer; generating one
 * would commit 8 files for 3 fields. So the fields are walked directly.
 *
 * That is the only use. Every Loams message goes through `DynamicMessage` and
 * `protobuf-java`'s own parser, which is a great deal more thorough about a malformed
 * body than 150 lines of arithmetic.
 *
 * ## What it refuses
 *
 * A truncated length-delimited field, a varint that runs off the end, a group with no
 * matching end marker and a wire type protobuf does not emit all throw
 * [IllegalArgumentException] rather than returning short data. A caller that got a
 * truncated `ErrorInfo` would report a `reason` the server did not send, and R8 is
 * about that reason being right.
 *
 * ## The one field-number rule that is not obvious
 *
 * A **start group** (wire type 3) is skipped by walking to its matching end group —
 * field number equal, wire type 4 — and **not** by nesting count. Nesting count is
 * what the wire format's own decoders use, but a nested start group increments it and
 * an unbalanced body then reads forever; the field-number pairing is self-delimiting
 * and terminates on the only byte that can close it.
 */
internal class StatusBytesReader(private val bytes: ByteArray) {
    private var at = 0
    private var pendingKey: Long? = null

    /** Whether another field follows, having read its key. */
    fun hasMore(): Boolean {
        if (pendingKey != null) {
            return true
        }
        if (at >= bytes.size) {
            return false
        }
        pendingKey = varint()
        return true
    }

    /**
     * The current field's number.
     *
     * The wire type is read and dropped: this reader only ever walks `varint` and
     * `length-delimited` fields, and [skipField] handles whatever it did not expect.
     */
    fun tag(): Int {
        val key = pendingKey ?: throw IllegalStateException("tag() before hasMore()")
        val number = key ushr 3
        if (number == 0L) {
            throw IllegalArgumentException("the message has a field number 0, which no encoder emits")
        }
        return number.toInt()
    }

    /** The current field's bytes, for a `length-delimited` field. */
    fun lengthDelimited(): ByteArray {
        consumeKey()
        if (wireTypeOf(key) != 2L) {
            throw IllegalArgumentException("field ${key ushr 3} is wire type ${wireTypeOf(key)}, not length-delimited")
        }
        val length = varint()
        if (length < 0 || length > Int.MAX_VALUE || length > bytes.size - at) {
            throw IllegalArgumentException("a field declares $length bytes and only ${bytes.size - at} are left")
        }
        val slice = bytes.copyOfRange(at, at + length.toInt())
        at += length.toInt()
        return slice
    }

    /**
     * Skips the current field, whatever its wire type.
     *
     * Every wire type is handled rather than only the two this reader wants, because
     * the field it is skipping is one this reader does **not** understand — which is
     * exactly the case where a newer server added something and the reader must not
     * stop.
     */
    fun skipField() {
        consumeKey()
        skipValue()
    }

    private var key: Long = 0

    private fun consumeKey() {
        key = pendingKey ?: throw IllegalStateException("a field read before hasMore()")
        pendingKey = null
    }

    private fun skipValue() {
        when (wireTypeOf(key)) {
            0L -> varint()
            1L -> at = advance(at, 8)
            2L -> {
                val length = varint()
                if (length < 0 || length > Int.MAX_VALUE) {
                    throw IllegalArgumentException("a field declares $length bytes, which cannot be a length")
                }
                at = advance(at, length.toInt())
            }
            3L -> {
                // A start group: walk to the matching end group, which is the same
                // field number with wire type 4.
                val groupTag = key ushr 3
                while (at < bytes.size) {
                    val inner = varint()
                    if (inner and 0x7L == 4L) {
                        if (inner ushr 3 == groupTag) {
                            return
                        }
                        continue
                    }
                    skipValueOf(inner)
                }
                throw IllegalArgumentException("a group starting at field $groupTag never ends")
            }
            5L -> at = advance(at, 4)
            else -> throw IllegalArgumentException("wire type ${wireTypeOf(key)} is not one protobuf emits")
        }
    }

    private fun skipValueOf(innerKey: Long) {
        val saved = key
        key = innerKey
        try {
            skipValue()
        } finally {
            key = saved
        }
    }

    private fun wireTypeOf(value: Long): Long = value and 0x7L

    private fun advance(from: Int, count: Int): Int {
        if (count < 0 || from + count > bytes.size) {
            throw IllegalArgumentException("a field declares $count bytes and only ${bytes.size - from} are left")
        }
        return from + count
    }

    private fun varint(): Long {
        var result = 0L
        var shift = 0
        while (true) {
            if (at >= bytes.size) {
                throw IllegalArgumentException("a varint runs off the end of the message")
            }
            val b = bytes[at].toInt() and 0xFF
            at++
            if (shift < 64) {
                result = result or ((b.toLong() and 0x7F) shl shift)
            }
            if (b and 0x80 == 0) {
                return result
            }
            shift += 7
            if (shift > 70) {
                throw IllegalArgumentException("a varint is longer than ten bytes, which no encoder emits")
            }
        }
    }
}