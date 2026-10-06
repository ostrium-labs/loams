package dev.loams

/**
 * Base64, as the Connect and gRPC-Web error framing needs it.
 *
 * # Why this exists rather than `java.util.Base64`
 *
 * A failed RPC carries its `loams.errors.v1.ErrorInfo` as a **base64** string in
 * `details[].value`, and a gRPC-Web refusal carries the same detail inside
 * `grpc-status-details-bin`, which is base64 of a `google.rpc.Status`. So base64
 * appears in exactly one place in this SDK — the error path — and the JDK's decoder
 * rejects the corpus.
 *
 * It rejects **unpadded** base64. `Base64.getDecoder().decode` requires the input
 * length to be a multiple of four; the corpus's `not_implemented` detail is recorded
 * as `Cg9ub3RfaW1wbGVtZW50ZWQ`, twenty-three characters, which is valid standard
 * base64 for `0a 0f` + `not_implemented` with the padding stripped. **Seven of the
 * ten distinct `details[].value` strings in `sdks/fixtures` are unpadded.** So the
 * JDK call threw on most of the corpus's reasons, the throw was caught and turned
 * into "this error has no reason", and every structured-reason fixture reported
 * `reason: none` against a server that had said exactly which reason it meant. R8 is
 * precisely about that reason reaching the caller.
 *
 * It is also a **crash** waiting to happen rather than only a lost reason: the
 * gRPC-Web `grpc-status-details-bin` call site had no guard, so a server that
 * recorded that header unpadded would have thrown out of the response reader
 * instead of producing a refusal.
 *
 * # The rules
 *
 * The alphabet is the standard one (`A-Za-z0-9+/`, `=` padding), **not** the
 * URL-safe variant, because that is what the wire uses. Three rules, and each is a
 * way a reason goes missing without an error:
 *
 *  - whitespace is **skipped**, because HTTP header folding and a line-wrapped
 *    `grpc-status-details-bin` both put it there and neither changes the value;
 *  - padding **ends** the stream, and a partial final group of two or three
 *    characters is accepted — that is the unpadded corpus. A final group of **one**
 *    is rejected: one leftover sextet has nothing to pair it with, so there is no
 *    byte it could stand for;
 *  - anything else outside the alphabet is **rejected** rather than skipped. A
 *    decoder that quietly drops invalid characters turns a truncated detail into a
 *    shorter but still-plausible one, which is how a `reason` goes missing without
 *    an error — and it is why this cannot be "decode with the JDK after padding
 *    it".
 *
 * Padding is also validated rather than merely terminated: once a `=` has been seen,
 * a further alphabet character makes the input **invalid**. The C++ SDK stops at the
 * padding instead and ignores whatever follows; this is deliberately stricter,
 * because the two only differ on input this corpus never produces and the strict
 * answer is the one that does not hand a caller a truncated reason.
 */
object Base64 {
    private const val ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    private val VALUES = IntArray(128) { -1 }.also { table ->
        for (index in ALPHABET.indices) {
            table[ALPHABET[index].code] = index
        }
    }

    /**
     * Decodes standard base64, padded or not, or returns null for input this
     * decoder will not accept.
     *
     * Null rather than an empty array for a refusal, so "the value was empty" and
     * "the value was not base64" stay distinguishable: an empty value decodes to an
     * empty array and null means the server did not write base64 at all. Callers
     * report both the same way — a refusal with no `ErrorInfo` — but a test can
     * tell them apart, which is the point.
     */
    fun decode(text: String?): ByteArray? {
        if (text == null) return null

        val out = ByteArrayOutput()
        val group = IntArray(4)
        var filled = 0
        var padded = false

        for (c in text) {
            if (c.isWhitespace()) {
                continue
            }
            if (c == '=') {
                // Padding ends the stream. Anything but whitespace after it is
                // rejected below, which is this decoder's one deliberate divergence
                // from the C++ SDK.
                padded = true
                continue
            }
            if (padded) {
                return null
            }
            val value = valueOf(c)
            if (value < 0) {
                return null
            }
            group[filled++] = value
            if (filled == 4) {
                val packed = (group[0] shl 18) or (group[1] shl 12) or (group[2] shl 6) or group[3]
                out.write((packed ushr 16) and 0xFF)
                out.write((packed ushr 8) and 0xFF)
                out.write(packed and 0xFF)
                filled = 0
            }
        }

        // One leftover character is one sextet with nothing to pair it with: there
        // is no byte it could stand for, so it is a malformed encoding rather than a
        // short one.
        if (filled == 1) {
            return null
        }
        if (filled == 2 || filled == 3) {
            val packed = (group[0] shl 18) or (group[1] shl 12) or (if (filled == 3) group[2] shl 6 else 0)
            out.write((packed ushr 16) and 0xFF)
            if (filled == 3) {
                out.write((packed ushr 8) and 0xFF)
            }
        }
        return out.toByteArray()
    }

    /** Encodes to standard base64, with padding. */
    fun encode(bytes: ByteArray): String {
        val out = StringBuilder((bytes.size + 2) / 3 * 4)
        var at = 0
        while (at + 3 <= bytes.size) {
            val packed = ((bytes[at].toInt() and 0xFF) shl 16) or
                ((bytes[at + 1].toInt() and 0xFF) shl 8) or
                (bytes[at + 2].toInt() and 0xFF)
            out.append(ALPHABET[(packed ushr 18) and 0x3F])
            out.append(ALPHABET[(packed ushr 12) and 0x3F])
            out.append(ALPHABET[(packed ushr 6) and 0x3F])
            out.append(ALPHABET[packed and 0x3F])
            at += 3
        }
        when (bytes.size - at) {
            1 -> {
                val packed = (bytes[at].toInt() and 0xFF) shl 16
                out.append(ALPHABET[(packed ushr 18) and 0x3F])
                out.append(ALPHABET[(packed ushr 12) and 0x3F])
                out.append("==")
            }
            2 -> {
                val packed = ((bytes[at].toInt() and 0xFF) shl 16) or ((bytes[at + 1].toInt() and 0xFF) shl 8)
                out.append(ALPHABET[(packed ushr 18) and 0x3F])
                out.append(ALPHABET[(packed ushr 12) and 0x3F])
                out.append(ALPHABET[(packed ushr 6) and 0x3F])
                out.append('=')
            }
        }
        return out.toString()
    }

    /**
     * The value of a base64 character, or -1.
     *
     * A 128-entry table rather than a `when` over `Char`: the table is a
     * constructor-time constant and the lookup is on a path a malformed body can
     * reach, and a `when` compiles to a linear chain of comparisons.
     */
    private fun valueOf(c: Char): Int = if (c.code < 128) VALUES[c.code] else -1
}

/**
 * A growable byte buffer.
 *
 * The SDK encodes every request and decodes every response, and a `ByteArrayOutputStream`
 * per message would be an object per message for a size the message knows. This is
 * twenty lines and it is the only place the SDK needs one.
 */
internal class ByteArrayOutput(initial: Int = 64) {
    private var buffer = ByteArray(if (initial < 8) 8 else initial)
    private var length = 0

    /** The bytes written so far, copied out. */
    fun toByteArray(): ByteArray = buffer.copyOf(length)

    /** The number of bytes written so far. */
    val size: Int get() = length

    /** Appends one byte. */
    fun write(byte: Int) {
        ensure(1)
        buffer[length++] = byte.toByte()
    }

    /** Appends [count] bytes from [bytes], starting at [from]. */
    fun write(bytes: ByteArray, from: Int = 0, count: Int = bytes.size - from) {
        ensure(count)
        System.arraycopy(bytes, from, buffer, length, count)
        length += count
    }

    /** Appends one big-endian `uint32`, which is what the 5-byte envelope's length is. */
    fun writeUInt32(value: Int) {
        ensure(4)
        buffer[length++] = (value ushr 24).toByte()
        buffer[length++] = (value ushr 16).toByte()
        buffer[length++] = (value ushr 8).toByte()
        buffer[length++] = value.toByte()
    }

    private fun ensure(extra: Int) {
        if (length + extra <= buffer.size) return
        var size = buffer.size
        while (size < length + extra) {
            size = size shl 1
        }
        buffer = buffer.copyOf(size)
    }
}